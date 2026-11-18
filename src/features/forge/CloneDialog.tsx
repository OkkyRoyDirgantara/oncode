// Dialog clone: URL atau repositori dari akun yang login, folder tujuan, lalu progres dengan tombol batal.
import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { Channel } from "@tauri-apps/api/core";
import { FolderOpen, GitFork, Lock, Search } from "lucide-react";
import { commands, errorCode, errorDetail, errorMessage, newOperationId, type Account, type Progress, type RemoteRepo } from "@/ipc";
import { choose, openDialog, toast } from "@/app/overlay";
import { openFolder, pickFolder, useApp } from "@/app/store";
import { joinPath, repoNameFromUrl, validateCloneUrl, validateFolderName } from "./clone";
import { loadAccounts, useForge } from "./store";
import { describeForgeError, forgeName, hostName, relativeTime } from "./util";

const PARENT_KEY = "oncode.clone.parentDir";
export const REPO_SEARCH_DEBOUNCE_MS = 300;

function rememberedParent(): string {
  try {
    return window.localStorage.getItem(PARENT_KEY) ?? "";
  } catch {
    return "";
  }
}

function rememberParent(path: string) {
  try {
    window.localStorage.setItem(PARENT_KEY, path);
  } catch {
    // Penyimpanan lokal tidak tersedia: abaikan.
  }
}

/** Daftar repositori sebuah akun dengan pencarian (di-debounce) dan "Load more". */
function RepoPicker({ accounts, disabled, onPick }: { accounts: Account[]; disabled: boolean; onPick: (repo: RemoteRepo) => void }) {
  const [accountId, setAccountId] = useState(accounts[0]?.id ?? "");
  const [query, setQuery] = useState("");
  const [applied, setApplied] = useState("");
  const [repos, setRepos] = useState<RemoteRepo[]>([]);
  const [nextPage, setNextPage] = useState<number | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const seq = useRef(0);
  const account = accounts.find((a) => a.id === accountId) ?? accounts[0];

  useEffect(() => {
    const handle = setTimeout(() => setApplied(query.trim()), REPO_SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [query]);

  useEffect(() => {
    if (!account) return;
    const current = ++seq.current;
    setStatus("loading");
    setError(null);
    commands.forgeRepos(account.id, applied, 1).then(
      (page) => {
        if (current !== seq.current) return;
        setRepos(page.items);
        setNextPage(page.nextPage);
        setStatus("ready");
      },
      (e: unknown) => {
        if (current !== seq.current) return;
        setRepos([]);
        setNextPage(null);
        setError(describeForgeError(e, account).title);
        setStatus("error");
      },
    );
    return () => {
      seq.current += 1;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account?.id, applied]);

  const loadMore = async () => {
    if (!account || nextPage === null || loadingMore) return;
    const current = seq.current;
    setLoadingMore(true);
    try {
      const page = await commands.forgeRepos(account.id, applied, nextPage);
      if (current !== seq.current) return;
      setRepos((list) => {
        const known = new Set(list.map((r) => r.fullName));
        return [...list, ...page.items.filter((r) => !known.has(r.fullName))];
      });
      setNextPage(page.nextPage);
    } catch (e) {
      if (current === seq.current) setError(describeForgeError(e, account).title);
    } finally {
      if (current === seq.current) setLoadingMore(false);
    }
  };

  if (!account) return null;
  return (
    <div className="fg-repos">
      <div className="fg-repos-bar">
        {accounts.length > 1 && (
          <select className="oc-select fg-repos-account" aria-label="Account" value={account.id} disabled={disabled} onChange={(e) => setAccountId(e.target.value)}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.username} ({hostName(a.host)})
              </option>
            ))}
          </select>
        )}
        <div className="fg-search">
          <Search size={13} className="fg-search-icon" />
          <input
            className="oc-input"
            placeholder={`Search ${account.username}'s repositories on ${forgeName(account.kind)}`}
            aria-label="Search repositories"
            value={query}
            disabled={disabled}
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      </div>
      <div className="fg-repos-list" role="listbox" aria-label="Repositories">
        {status === "loading" && <div className="fg-empty">Loading repositories…</div>}
        {status === "error" && <div className="fg-empty oc-error-text" role="alert">{error}</div>}
        {status === "ready" && repos.length === 0 && nextPage === null && <div className="fg-empty">No repositories found.</div>}
        {status === "ready" &&
          repos.map((r) => (
            <div
              key={r.fullName}
              role="option"
              tabIndex={0}
              aria-selected={selected === r.fullName}
              aria-disabled={disabled || undefined}
              className="fg-repo-row"
              title={r.description || r.fullName}
              onClick={() => {
                if (disabled) return;
                setSelected(r.fullName);
                onPick(r);
              }}
              onKeyDown={(e) => {
                if (e.key !== "Enter" || disabled) return;
                e.preventDefault();
                setSelected(r.fullName);
                onPick(r);
              }}
            >
              <div className="fg-repo-name">
                <span className="oc-ellipsis">{r.fullName}</span>
                {r.private && <Lock size={11} aria-label="Private" className="oc-faint" />}
                {r.fork && <GitFork size={11} aria-label="Fork" className="oc-faint" />}
                <span className="oc-faint fg-small fg-nowrap" style={{ marginLeft: "auto" }}>{relativeTime(r.updatedAt)}</span>
              </div>
              {r.description && <div className="oc-faint fg-small oc-ellipsis">{r.description}</div>}
            </div>
          ))}
        {status === "ready" && error && <div className="fg-empty oc-error-text" role="alert">{error}</div>}
        {status === "ready" && nextPage !== null && (
          <div className="fg-buttons">
            <button type="button" className="oc-btn oc-btn-sm oc-btn-block" disabled={loadingMore || disabled} onClick={() => void loadMore()}>
              {loadingMore ? "Loading…" : "Load more"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

type ClonePhase = { name: "form" } | { name: "cloning"; operationId: string; progress: Progress | null; cancelling: boolean };

export interface CloneDialogProps {
  close: (dest: string | null) => void;
  initialUrl?: string;
}

export function CloneDialog({ close, initialUrl = "" }: CloneDialogProps) {
  const accounts = useForge((s) => s.accounts);
  const homeDir = useApp((s) => s.info?.homeDir ?? null);
  const [url, setUrl] = useState(initialUrl);
  const [name, setName] = useState(repoNameFromUrl(initialUrl));
  /** Nama sudah disunting sendiri: tidak lagi diturunkan dari URL. */
  const [nameEdited, setNameEdited] = useState(false);
  const [parent, setParent] = useState(() => rememberedParent() || homeDir || "");
  const [submodules, setSubmodules] = useState(false);
  const [touched, setTouched] = useState(false);
  const [phase, setPhase] = useState<ClonePhase>({ name: "form" });
  const [error, setError] = useState<{ title: string; detail?: string } | null>(null);
  const urlInput = useRef<HTMLInputElement>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    urlInput.current?.focus();
    void loadAccounts();
    return () => {
      mounted.current = false;
    };
  }, []);

  const changeUrl = (value: string) => {
    setUrl(value);
    if (!nameEdited) setName(repoNameFromUrl(value));
  };

  const urlProblem = validateCloneUrl(url);
  const nameProblem = validateFolderName(name);
  const parentProblem = parent.trim() === "" ? "Choose a destination folder" : null;
  const cloning = phase.name === "cloning";
  const canSubmit = !urlProblem && !nameProblem && !parentProblem && !cloning;

  const browse = async () => {
    const picked = await pickFolder("Choose Destination Folder");
    if (picked) setParent(picked);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (!canSubmit) return;
    setError(null);
    const operationId = newOperationId();
    const channel = new Channel<Progress>();
    channel.onmessage = (progress) => {
      if (mounted.current) setPhase((p) => (p.name === "cloning" && p.operationId === operationId ? { ...p, progress } : p));
    };
    setPhase({ name: "cloning", operationId, progress: null, cancelling: false });
    try {
      const dest = await commands.gitClone(url.trim(), parent, name, submodules, operationId, channel);
      rememberParent(parent);
      close(dest);
    } catch (err) {
      if (!mounted.current) return;
      setPhase({ name: "form" });
      const code = errorCode(err);
      if (code === "cancelled") {
        toast.info("Clone cancelled.");
        return;
      }
      if (code === "alreadyExists") setError({ title: `${joinPath(parent, name)} already exists and is not empty. Choose another folder name or destination.` });
      else if (code === "authFailed") setError({ title: "Authentication failed. Check the URL and your credentials, or sign in under Pull Requests → Accounts.", detail: errorDetail(err) ?? errorMessage(err) });
      else setError({ title: `Clone failed: ${errorMessage(err)}`, detail: errorDetail(err) });
    }
  };

  const cancelClone = () => {
    if (phase.name !== "cloning") return;
    setPhase({ ...phase, cancelling: true });
    void commands.appCancel(phase.operationId).catch(() => undefined);
  };

  const percent = phase.name === "cloning" ? (phase.progress?.percent ?? null) : null;

  return (
    <form onSubmit={(e) => void submit(e)}>
      <div className="oc-dialog-header">Clone Repository</div>
      <div className="oc-dialog-body">
        <div className="oc-field">
          <label htmlFor="fg-clone-url">Repository URL</label>
          <input
            id="fg-clone-url"
            ref={urlInput}
            className="oc-input oc-mono"
            placeholder="https://github.com/owner/repo.git"
            value={url}
            disabled={cloning}
            spellCheck={false}
            autoComplete="off"
            aria-invalid={touched && !!urlProblem ? true : undefined}
            onChange={(e) => changeUrl(e.target.value)}
          />
          {touched && urlProblem && <div className="oc-error-text">{urlProblem}</div>}
        </div>
        {accounts.length > 0 && (
          <RepoPicker
            accounts={accounts}
            disabled={cloning}
            onPick={(repo) => {
              setUrl(repo.cloneUrlHttps);
              if (!nameEdited) setName(repoNameFromUrl(repo.cloneUrlHttps));
            }}
          />
        )}
        <div className="oc-field">
          <label htmlFor="fg-clone-parent">Destination</label>
          <div className="oc-row">
            <input id="fg-clone-parent" className="oc-input" value={parent} readOnly placeholder="Choose a folder…" title={parent} aria-invalid={touched && !!parentProblem ? true : undefined} />
            <button type="button" className="oc-btn" disabled={cloning} onClick={() => void browse()}>
              <FolderOpen size={14} /> Browse…
            </button>
          </div>
          {touched && parentProblem && <div className="oc-error-text">{parentProblem}</div>}
        </div>
        <div className="oc-field">
          <label htmlFor="fg-clone-name">Folder name</label>
          <input
            id="fg-clone-name"
            className="oc-input"
            value={name}
            disabled={cloning}
            spellCheck={false}
            aria-invalid={(touched || nameEdited) && !!nameProblem ? true : undefined}
            onChange={(e) => {
              setName(e.target.value);
              setNameEdited(e.target.value !== "");
            }}
          />
          {(touched || nameEdited) && nameProblem && <div className="oc-error-text">{nameProblem}</div>}
          {!nameProblem && !parentProblem && <div className="oc-faint fg-small oc-selectable">Clones into {joinPath(parent, name)}</div>}
        </div>
        <label className="oc-check">
          <input type="checkbox" checked={submodules} disabled={cloning} onChange={(e) => setSubmodules(e.target.checked)} />
          Clone submodules
        </label>
        {error && (
          <div role="alert" style={{ marginTop: 10 }}>
            <div className="oc-error-text oc-selectable">{error.title}</div>
            {error.detail && error.detail !== error.title && <pre className="oc-dialog-detail oc-mono" style={{ marginTop: 6 }}>{error.detail}</pre>}
          </div>
        )}
        {phase.name === "cloning" && (
          <div className="fg-progress" role="status" aria-live="polite">
            <div className="fg-progress-text">
              <span className="oc-ellipsis">{phase.cancelling ? "Cancelling…" : (phase.progress?.phase ?? "Starting clone")}</span>
              {percent !== null && <span>{Math.round(percent)}%</span>}
            </div>
            <div className="oc-progress fg-progress-bar" data-indeterminate={percent === null ? true : undefined} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}>
              <div style={{ width: percent !== null ? `${Math.max(0, Math.min(100, percent))}%` : undefined }} />
            </div>
          </div>
        )}
      </div>
      <div className="oc-dialog-footer">
        {phase.name === "cloning" ? (
          <button type="button" className="oc-btn" disabled={phase.cancelling} onClick={cancelClone}>
            Cancel Clone
          </button>
        ) : (
          <>
            <button type="button" className="oc-btn" onClick={() => close(null)}>Cancel</button>
            <button type="submit" className="oc-btn oc-btn-primary" disabled={touched && !canSubmit}>
              Clone
            </button>
          </>
        )}
      </div>
    </form>
  );
}

/** Command `git.clone`: dialog clone, lalu tawaran membuka hasilnya. */
export async function cloneRepository(initialUrl?: string): Promise<string | null> {
  const dest = await openDialog<string>((close) => <CloneDialog close={close} initialUrl={typeof initialUrl === "string" ? initialUrl : ""} />, { wide: true, dismissible: false });
  if (!dest) return null;
  const answer = await choose({
    title: "Repository cloned",
    message: `The repository was cloned into\n${dest}`,
    buttons: [
      { id: "later", label: "Not Now" },
      { id: "window", label: "Open in New Window" },
      { id: "open", label: "Open", kind: "primary" },
    ],
  });
  if (answer === "open") await openFolder(dest);
  else if (answer === "window") {
    try {
      await commands.windowOpen(dest);
    } catch (e) {
      toast.error(`Could not open a new window: ${errorMessage(e)}`);
    }
  }
  return dest;
}

/** Command `git.init`: menjadikan folder proyek sebuah repositori lalu memuat ulang workspace. */
export async function initRepository(): Promise<void> {
  const workspace = useApp.getState().workspace;
  if (!workspace) {
    toast.info("Open a folder first.");
    return;
  }
  if (!workspace.trusted) {
    // `git init` menandai folder sebagai dipercaya; di mode terbatas itu akan melewati pertanyaan kepercayaan.
    toast.info("Git is disabled in restricted mode. Trust this folder first.");
    return;
  }
  if (workspace.repoRoot) {
    toast.info("This folder is already inside a Git repository.");
    return;
  }
  try {
    await commands.gitInit(workspace.root);
  } catch (e) {
    toast.error(`Could not initialize a repository: ${errorMessage(e)}`);
    return;
  }
  // Folder hasil `git init` langsung dipercaya oleh backend; buka ulang agar fitur Git aktif.
  if (await openFolder(workspace.root)) toast.success("Initialized an empty Git repository.");
}

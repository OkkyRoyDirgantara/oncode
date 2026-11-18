// Dialog membuat PR/MR dari branch aktif. Branch di-push lebih dulu bila belum ada di remote atau tertinggal.
import { useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { commands, errorCode, type ForgeContext, type NewPullRequest, type PullRequest, type Refs, type RepoSnapshot } from "@/ipc";
import { openDialog, toast } from "@/app/overlay";
import { revealSidebar } from "@/app/store";
import { push, requireRepo } from "@/features/scm/actions";
import { useScm } from "@/features/scm/store";
import { signIn } from "./LoginDialog";
import { loadPrs, useForge } from "./store";
import { describeForgeError, forgeName, loginHostFor, openExternal, prNoun, prNumber } from "./util";

/** Yang perlu dilakukan sebelum PR bisa dibuat. */
export type PushNeed = { kind: "none" } | { kind: "publish" } | { kind: "push"; ahead: number };

export function pushNeed(snapshot: Pick<RepoSnapshot, "upstream">): PushNeed {
  const upstream = snapshot.upstream;
  if (!upstream || !upstream.exists) return { kind: "publish" };
  if (upstream.ahead > 0) return { kind: "push", ahead: upstream.ahead };
  return { kind: "none" };
}

/** Nama branch di remote: nama upstream tanpa awalan remote, atau nama branch lokal bila belum dipublikasikan. */
export function sourceBranchName(localBranch: string, snapshot: Pick<RepoSnapshot, "upstream">, refs: Pick<Refs, "remotes"> | null): string {
  const upstream = snapshot.upstream;
  if (!upstream || !upstream.exists) return localBranch;
  const remotes = [...(refs?.remotes ?? [])].sort((a, b) => b.name.length - a.name.length);
  for (const remote of remotes) {
    if (upstream.name.startsWith(`${remote.name}/`)) return upstream.name.slice(remote.name.length + 1);
  }
  const slash = upstream.name.indexOf("/");
  return slash >= 0 ? upstream.name.slice(slash + 1) : upstream.name;
}

/** Branch tujuan yang bisa dipilih: branch remote milik `remote`, tanpa awalan remote dan tanpa `HEAD`. */
export function targetBranchOptions(refs: Pick<Refs, "remoteBranches"> | null, remote: string, extra: string[] = []): string[] {
  const names = new Set<string>();
  for (const branch of refs?.remoteBranches ?? []) {
    if (branch.remote !== remote) continue;
    const name = branch.name.startsWith(`${remote}/`) ? branch.name.slice(remote.length + 1) : branch.name;
    if (name && name !== "HEAD") names.add(name);
  }
  for (const name of extra) if (name) names.add(name);
  return [...names].sort((a, b) => a.localeCompare(b));
}

export interface CreatePrDialogProps {
  root: string;
  context: ForgeContext;
  /** Branch lokal yang sedang aktif. */
  branch: string;
  close: (created: PullRequest | null) => void;
}

export function CreatePrDialog({ root, context, branch, close }: CreatePrDialogProps) {
  const account = context.account!;
  const kind = context.kind ?? account.kind;
  const noun = prNoun(kind);
  const repo = useScm((s) => s.repos[root] ?? null);
  const snapshot = repo?.snapshot ?? null;
  const refs = repo?.refs ?? null;
  const subject = refs?.branches.find((b) => b.name === branch)?.subject ?? "";

  const [title, setTitle] = useState(subject);
  const [body, setBody] = useState("");
  const [target, setTarget] = useState("");
  const [targetTouched, setTargetTouched] = useState(false);
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const [draft, setDraft] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<{ title: string; detail?: string } | null>(null);
  const titleInput = useRef<HTMLInputElement>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    titleInput.current?.focus();
    titleInput.current?.select();
    return () => {
      mounted.current = false;
    };
  }, []);

  // Branch bawaan repositori menjadi tujuan awal, kecuali pengguna sudah memilih sendiri.
  useEffect(() => {
    let cancelled = false;
    commands.forgeDefaultBranch(account.id, context.repo).then(
      (name) => {
        if (cancelled) return;
        setDefaultBranch(name);
      },
      () => {
        if (!cancelled) setDefaultBranch("");
      },
    );
    return () => {
      cancelled = true;
    };
  }, [account.id, context.repo]);

  const need: PushNeed = snapshot ? pushNeed(snapshot) : { kind: "none" };
  const source = snapshot ? sourceBranchName(branch, snapshot, refs) : branch;
  const options = useMemo(() => targetBranchOptions(refs, context.remote, [defaultBranch ?? ""]).filter((name) => name !== source), [refs, context.remote, defaultBranch, source]);
  // Tanpa branch bawaan yang diketahui, tujuan harus dipilih sendiri; tidak ada tebakan diam-diam.
  const effectiveTarget = targetTouched ? target : defaultBranch && defaultBranch !== source ? defaultBranch : "";

  const problem = !title.trim()
    ? "Enter a title."
    : defaultBranch === null && !targetTouched
      ? null
      : !effectiveTarget
        ? "Choose a target branch."
        : effectiveTarget === source
          ? "The source and target branch are the same."
          : null;
  const loadingTarget = defaultBranch === null && !targetTouched;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy || problem || loadingTarget) return;
    setError(null);
    if (need.kind !== "none") {
      setBusy(need.kind === "publish" ? "Publishing branch…" : "Pushing commits…");
      const pushed = need.kind === "publish" ? await push(root, { setUpstream: true, remote: context.remote }) : await push(root);
      if (!mounted.current) return;
      if (!pushed) {
        setBusy(null);
        setError({ title: `The branch could not be pushed, so the ${noun.toLowerCase()} was not created. See the notification for the reason.` });
        return;
      }
    }
    setBusy(`Creating ${noun.toLowerCase()}…`);
    const input: NewPullRequest = { title: title.trim(), body, sourceBranch: source, targetBranch: effectiveTarget, draft };
    try {
      const created = await commands.forgePrCreate(account.id, context.repo, input);
      toast.success(`${noun} ${prNumber(kind, created.number)} created.`, [{ label: "Open in browser", run: () => openExternal(created.webUrl) }]);
      if (useForge.getState().root === root) void loadPrs();
      close(created);
    } catch (err) {
      if (!mounted.current) return;
      setBusy(null);
      const text = describeForgeError(err, account);
      // `invalidInput` biasanya berarti PR untuk branch ini sudah ada, atau tidak ada commit baru.
      setError(errorCode(err) === "invalidInput" ? { title: `${forgeName(kind)} rejected this ${noun.toLowerCase()}. A ${noun.toLowerCase()} for this branch may already exist, or the branch has no new commits.`, detail: text.detail } : text);
    }
  };

  return (
    <form onSubmit={(e) => void submit(e)}>
      <div className="oc-dialog-header">Create {noun}</div>
      <div className="oc-dialog-body">
        <p className="oc-muted">
          From <code className="fg-code">{source}</code> in {context.repo.owner}/{context.repo.name} on {context.repo.host}, as {account.username}.
        </p>
        {need.kind === "publish" && (
          <p className="fg-note" role="status">
            This branch has not been published yet. It will be pushed to <strong>{context.remote}</strong> first.
          </p>
        )}
        {need.kind === "push" && (
          <p className="fg-note" role="status">
            This branch has {need.ahead} commit{need.ahead === 1 ? "" : "s"} that {need.ahead === 1 ? "is" : "are"} not on the remote yet. {need.ahead === 1 ? "It" : "They"} will be pushed first.
          </p>
        )}
        <div className="oc-field">
          <label htmlFor="fg-pr-title">Title</label>
          <input id="fg-pr-title" ref={titleInput} className="oc-input" value={title} disabled={busy !== null} spellCheck={false} onChange={(e) => setTitle(e.target.value)} />
        </div>
        <div className="oc-field">
          <label htmlFor="fg-pr-body">Description (Markdown)</label>
          <textarea id="fg-pr-body" className="oc-textarea" rows={7} value={body} disabled={busy !== null} onChange={(e) => setBody(e.target.value)} />
        </div>
        <div className="oc-field">
          <label htmlFor="fg-pr-target">Target branch</label>
          <select
            id="fg-pr-target"
            className="oc-select"
            value={effectiveTarget}
            disabled={busy !== null || loadingTarget}
            onChange={(e) => {
              setTarget(e.target.value);
              setTargetTouched(true);
            }}
          >
            {loadingTarget && <option value="">Loading…</option>}
            {!loadingTarget && effectiveTarget === "" && <option value="">{options.length === 0 ? `No other branch known on ${context.remote}` : "Choose a branch…"}</option>}
            {options.map((name) => (
              <option key={name} value={name}>
                {name}
                {name === defaultBranch ? " (default)" : ""}
              </option>
            ))}
          </select>
        </div>
        <label className="oc-check">
          <input type="checkbox" checked={draft} disabled={busy !== null} onChange={(e) => setDraft(e.target.checked)} />
          Create as draft
        </label>
        {error && (
          <div role="alert" style={{ marginTop: 10 }}>
            <div className="oc-error-text">{error.title}</div>
            {error.detail && error.detail !== error.title && <div className="oc-muted oc-selectable fg-small" style={{ marginTop: 2 }}>{error.detail}</div>}
          </div>
        )}
        {busy && <div className="oc-muted" role="status" style={{ marginTop: 10 }}>{busy}</div>}
      </div>
      <div className="oc-dialog-footer">
        <button type="button" className="oc-btn" disabled={busy !== null} onClick={() => close(null)}>Cancel</button>
        <button type="submit" className="oc-btn oc-btn-primary" disabled={busy !== null || !!problem || loadingTarget} title={problem ?? undefined}>
          {need.kind === "publish" ? `Publish & Create ${noun}` : need.kind === "push" ? `Push & Create ${noun}` : `Create ${noun}`}
        </button>
      </div>
    </form>
  );
}

/** Command `forge.createPullRequest`: memeriksa prasyarat lalu membuka dialog. */
export async function createPullRequest(): Promise<PullRequest | null> {
  const root = requireRepo();
  if (!root) return null;
  const snapshot = useScm.getState().repos[root]?.snapshot;
  if (!snapshot) return null;
  if (snapshot.head.kind !== "branch") {
    toast.info(snapshot.head.kind === "unborn" ? "Make a first commit before creating a pull request." : "You are not on a branch. Check out or create a branch before creating a pull request.");
    return null;
  }
  const branch = snapshot.head.name;
  let context: ForgeContext | null;
  try {
    context = await commands.forgeContext(root);
  } catch (e) {
    toast.error(describeForgeError(e).title);
    return null;
  }
  if (!context) {
    toast.info("No remote of this repository points to GitHub or GitLab, so there is nowhere to create a pull request.");
    return null;
  }
  if (!context.account) {
    const kind = context.kind;
    const host = context.repo.host;
    toast.info(`Sign in to ${host} to create a ${prNoun(kind, { lower: true })}.`, [
      kind ? { label: "Sign in", run: () => signIn(kind, loginHostFor(kind, host)) } : { label: "Accounts", run: () => revealSidebar("pullRequests") },
    ]);
    return null;
  }
  const ready = context;
  return openDialog<PullRequest>((close) => <CreatePrDialog root={root} context={ready} branch={branch} close={close} />, { wide: true, dismissible: false });
}

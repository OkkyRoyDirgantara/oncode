// Tab detail PR/MR: kepala, pemeriksaan CI, label, reviewer, deskripsi, checkout, dan berkas yang berubah.
import { useCallback, useEffect, useRef, useState } from "react";
import { CircleCheck, CircleDashed, CircleDot, CircleMinus, CircleSlash, CircleX, Download, ExternalLink, GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft, LoaderCircle, RefreshCw } from "lucide-react";
import type { TabProps } from "@/app/registry";
import { toast } from "@/app/overlay";
import { commands, type ChangeKind, type Check, type CheckState, type CommitFile, type PullRequest, type PullRequestDetail } from "@/ipc";
import { cancelBusy, fetch as gitFetch, runWithProgress } from "@/features/scm/actions";
import { openDiff } from "@/features/scm/open";
import { useRepo } from "@/features/scm/store";
import { Avatar, type PrTabParams } from "./ForgeView";
import { Markdown } from "./markdown";
import { useForge } from "./store";
import { describeForgeError, forgeName, openExternal, prNoun, prNumber, prShort, relativeTime, safeExternalUrl } from "./util";
import "./forge.css";

const CHECK_LABEL: Record<CheckState, string> = {
  pending: "Pending",
  running: "Running",
  success: "Passed",
  failure: "Failed",
  cancelled: "Cancelled",
  skipped: "Skipped",
  neutral: "Neutral",
};

function CheckIcon({ state }: { state: CheckState }) {
  const props = { size: 14, className: "fg-check-icon", "data-state": state, "aria-label": CHECK_LABEL[state] };
  switch (state) {
    case "success":
      return <CircleCheck {...props} />;
    case "failure":
      return <CircleX {...props} />;
    case "running":
      return <LoaderCircle {...props} className="fg-check-icon oc-spin" />;
    case "pending":
      return <CircleDot {...props} />;
    case "cancelled":
      return <CircleSlash {...props} />;
    case "skipped":
      return <CircleMinus {...props} />;
    case "neutral":
      return <CircleDashed {...props} />;
  }
}

/** Ringkasan pemeriksaan CI, mis. "3 passed, 1 failed". */
export function summarizeChecks(checks: Check[]): string {
  if (checks.length === 0) return "";
  const count = (states: CheckState[]) => checks.filter((c) => states.includes(c.state)).length;
  const parts: string[] = [];
  const failed = count(["failure"]);
  const running = count(["pending", "running"]);
  const passed = count(["success"]);
  const other = checks.length - failed - running - passed;
  if (failed) parts.push(`${failed} failed`);
  if (running) parts.push(`${running} in progress`);
  if (passed) parts.push(`${passed} passed`);
  if (other) parts.push(`${other} skipped or cancelled`);
  return parts.join(", ");
}

function StateBadge({ pr, noun }: { pr: PullRequest; noun: string }) {
  const state = pr.state === "open" && pr.draft ? "draft" : pr.state;
  const label = state === "draft" ? "Draft" : state === "open" ? "Open" : state === "merged" ? "Merged" : "Closed";
  const Icon = state === "merged" ? GitMerge : state === "closed" ? GitPullRequestClosed : state === "draft" ? GitPullRequestDraft : GitPullRequest;
  return (
    <span className="fg-state-badge" data-state={state} title={`${noun} is ${label.toLowerCase()}`}>
      <Icon size={14} /> {label}
    </span>
  );
}

const KIND_LETTER: Record<ChangeKind, string> = { added: "A", modified: "M", deleted: "D", renamed: "R", copied: "C", typeChanged: "T" };
const KIND_CLASS: Record<ChangeKind, string> = {
  added: "oc-git-added",
  modified: "oc-git-modified",
  deleted: "oc-git-deleted",
  renamed: "oc-git-renamed",
  copied: "oc-git-renamed",
  typeChanged: "oc-git-modified",
};

/** Ref pembanding untuk berkas yang berubah: `remote/target` bila ada, kalau tidak branch lokal `target`. */
export function baseRefFor(remote: string, target: string, refs: { branches: { name: string }[]; remoteBranches: { name: string }[] } | null): string | null {
  if (!refs) return null;
  const tracking = `${remote}/${target}`;
  if (refs.remoteBranches.some((b) => b.name === tracking)) return tracking;
  if (refs.branches.some((b) => b.name === target)) return target;
  return null;
}

/** Dua titik yang dibandingkan untuk daftar berkas yang berubah. */
interface Comparison {
  /** Ref branch tujuan, mis. `origin/main`. */
  base: string;
  /** Revisi awal diff: merge base bila ada, kalau tidak `base` itu sendiri. */
  from: string;
  /** Oid merge base, atau null bila tidak ada (riwayat tidak berhubungan). */
  mergeBase: string | null;
}

type Loaded<T> = { status: "loading" } | { status: "ready"; value: T } | { status: "error"; message: string; detail?: string };

export function PullRequestTab({ tab }: TabProps) {
  const params = tab.params as unknown as PrTabParams;
  const { root, accountId, repo, kind, remote, number } = params;
  const account = useForge((s) => s.accounts.find((a) => a.id === accountId) ?? null);
  const repoState = useRepo(root);
  const [detail, setDetail] = useState<Loaded<PullRequestDetail>>({ status: "loading" });
  const [checks, setChecks] = useState<Loaded<Check[]>>({ status: "loading" });
  const [changes, setChanges] = useState<(Loaded<CommitFile[]> & Partial<Comparison>) | null>(null);
  const [checkingOut, setCheckingOut] = useState(false);
  const seq = useRef(0);

  const noun = prNoun(kind);
  const short = prShort(kind);
  const branch = `pr/${number}`;
  const localBranch = repoState?.refs?.branches.find((b) => b.name === branch) ?? null;
  const onBranch = repoState?.snapshot?.head.kind === "branch" && repoState.snapshot.head.name === branch;

  const load = useCallback(async () => {
    const current = ++seq.current;
    setDetail({ status: "loading" });
    setChecks({ status: "loading" });
    try {
      const value = await commands.forgePr(accountId, repo, number);
      if (current !== seq.current) return;
      setDetail({ status: "ready", value });
      try {
        const list = await commands.forgeChecks(accountId, repo, value.pullRequest.headSha);
        if (current === seq.current) setChecks({ status: "ready", value: list });
      } catch (e) {
        if (current !== seq.current) return;
        const text = describeForgeError(e);
        setChecks({ status: "error", message: text.title, detail: text.detail });
      }
    } catch (e) {
      if (current !== seq.current) return;
      const text = describeForgeError(e);
      setDetail({ status: "error", message: text.title, detail: text.detail });
    }
    // `repo` berasal dari parameter tab dan tidak berubah selama tab hidup.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountId, repo.host, repo.owner, repo.name, number]);

  useEffect(() => {
    void load();
    return () => {
      seq.current += 1;
    };
  }, [load]);

  const target = detail.status === "ready" ? detail.value.pullRequest.targetBranch : null;

  const loadChanges = async (targetBranch: string) => {
    const base = baseRefFor(remote, targetBranch, repoState?.refs ?? null);
    if (!base) {
      setChanges({ status: "error", message: `The target branch ${targetBranch} is not available locally (neither ${remote}/${targetBranch} nor ${targetBranch}). Fetch ${remote} and try again.` });
      return;
    }
    setChanges({ status: "loading", base });
    // Dibandingkan dari merge base agar commit yang masuk ke branch tujuan setelah PR bercabang
    // tidak tampil sebagai perubahan terbalik. Tanpa merge base: perbandingan dua titik biasa.
    const mergeBase = await commands.gitMergeBase(root, base, branch).catch(() => null);
    const comparison: Comparison = { base, from: mergeBase ?? base, mergeBase };
    try {
      const files = await commands.gitDiffFiles(root, comparison.from, branch);
      setChanges({ status: "ready", value: files, ...comparison });
    } catch (e) {
      const text = describeForgeError(e);
      setChanges({ status: "error", message: text.title, detail: text.detail, ...comparison });
    }
  };

  const checkout = async (thenViewChanges: boolean) => {
    if (checkingOut) return;
    setCheckingOut(true);
    const result: { branch: string | null } = { branch: null };
    const ok = await runWithProgress(root, `Checkout ${short} ${prNumber(kind, number)}`, async (operationId, progress) => {
      result.branch = await commands.forgePrCheckout(root, kind, remote, number, operationId, progress);
    });
    setCheckingOut(false);
    if (!ok || !result.branch) return;
    toast.success(`Checked out ${noun.toLowerCase()} ${prNumber(kind, number)} as local branch ${result.branch}.`);
    if (thenViewChanges && target) await loadChanges(target);
    else setChanges(null);
  };

  if (detail.status === "loading") {
    return (
      <div className="fg-detail">
        <div className="fg-empty" role="status">
          <LoaderCircle size={14} className="oc-spin" /> Loading {noun.toLowerCase()} {prNumber(kind, number)}…
        </div>
      </div>
    );
  }
  if (detail.status === "error") {
    return (
      <div className="fg-detail">
        <div className="fg-detail-inner" role="alert">
          <h1 className="fg-title">{noun} {prNumber(kind, number)}</h1>
          <p className="oc-selectable">{detail.message}</p>
          {detail.detail && detail.detail !== detail.message && <p className="oc-muted oc-selectable">{detail.detail}</p>}
          <button className="oc-btn" onClick={() => void load()}>Retry</button>
        </div>
      </div>
    );
  }

  const { pullRequest: pr, body, reviewers, labels, mergeable, commits, changedFiles, additions, deletions } = detail.value;
  const busy = repoState?.busy ?? null;
  const stale = localBranch !== null && localBranch.oid !== pr.headSha;
  const showLines = additions !== 0 || deletions !== 0;
  const webUrl = safeExternalUrl(pr.webUrl);

  return (
    <div className="fg-detail">
      <div className="fg-detail-inner">
        <header className="fg-header">
          <h1 className="fg-title oc-selectable">
            {pr.title} <span className="oc-faint">{prNumber(kind, pr.number)}</span>
          </h1>
          <div className="fg-header-meta">
            <StateBadge pr={pr} noun={noun} />
            <span className="fg-author">
              <Avatar url={pr.authorAvatar} name={pr.author} size={18} />
              <span className="oc-selectable">{pr.author}</span>
            </span>
            <span className="oc-muted">
              wants to merge <code className="fg-code oc-selectable">{pr.sourceBranch}</code> into <code className="fg-code oc-selectable">{pr.targetBranch}</code>
              {pr.isCrossRepo && " (from a fork)"}
            </span>
            <span className="oc-faint">updated {relativeTime(pr.updatedAt)}</span>
          </div>
          <div className="fg-actions">
            <button className="oc-btn oc-btn-primary" disabled={checkingOut || busy !== null || onBranch} title={onBranch ? `You are already on ${branch}` : `Fetch this ${short} into the local branch ${branch} and switch to it`} onClick={() => void checkout(false)}>
              <Download size={14} /> {onBranch ? `On ${branch}` : localBranch ? "Update & Checkout" : "Checkout"}
            </button>
            {localBranch ? (
              <button className="oc-btn" disabled={checkingOut} onClick={() => void loadChanges(pr.targetBranch)}>
                View Changes
              </button>
            ) : (
              <button className="oc-btn" disabled={checkingOut || busy !== null} onClick={() => void checkout(true)}>
                Checkout to view changes
              </button>
            )}
            <button className="oc-btn" disabled={!webUrl} onClick={() => void openExternal(pr.webUrl)}>
              <ExternalLink size={14} /> Open on {forgeName(kind)}
            </button>
            <button className="oc-iconbtn" title="Reload" aria-label="Reload" onClick={() => void load()}>
              <RefreshCw size={14} />
            </button>
          </div>
          {busy && (
            <div className="fg-busy" role="status">
              <LoaderCircle size={13} className="oc-spin" />
              <span className="oc-ellipsis">{busy.label}{busy.percent != null ? ` ${busy.percent}%` : "…"}</span>
              {busy.operationId && (
                <button className="oc-btn oc-btn-sm" onClick={() => cancelBusy(root)}>Cancel</button>
              )}
            </div>
          )}
          {!account && <div className="fg-note" role="status">The account used to open this tab is no longer signed in. Sign in again to reload it.</div>}
        </header>

        <section className="fg-section" aria-label="Summary">
          <div className="fg-stats">
            <span>{commits} commit{commits === 1 ? "" : "s"}</span>
            <span>{changedFiles} file{changedFiles === 1 ? "" : "s"} changed</span>
            {showLines && (
              <span>
                <span className="oc-git-added">+{additions}</span> <span className="oc-git-deleted">−{deletions}</span>
              </span>
            )}
            {pr.state === "open" && <span className={mergeable === false ? "oc-git-deleted" : "oc-muted"}>{mergeable === true ? "No conflicts with the target branch" : mergeable === false ? "Conflicts with the target branch" : "Mergeability not known yet"}</span>}
          </div>
          {labels.length > 0 && (
            <div className="fg-chips" aria-label="Labels">
              {labels.map((l) => (
                <span key={l} className="fg-chip">{l}</span>
              ))}
            </div>
          )}
          <div className="fg-reviewers">
            <span className="oc-muted">Reviewers:</span> {reviewers.length > 0 ? <span className="oc-selectable">{reviewers.join(", ")}</span> : <span className="oc-faint">none requested</span>}
          </div>
        </section>

        <section className="fg-section" aria-label="Checks">
          <h2 className="fg-h2">
            Checks {checks.status === "ready" && checks.value.length > 0 && <span className="oc-faint fg-h2-note">{summarizeChecks(checks.value)}</span>}
          </h2>
          {checks.status === "loading" && <div className="oc-faint">Loading checks…</div>}
          {checks.status === "error" && <div className="oc-muted oc-selectable">Could not load checks: {checks.message}</div>}
          {checks.status === "ready" && checks.value.length === 0 && <div className="oc-faint">No checks reported for {pr.headSha.slice(0, 8)}.</div>}
          {checks.status === "ready" &&
            checks.value.map((c, i) => {
              const url = safeExternalUrl(c.url);
              return (
                <div key={`${c.name}:${i}`} className="fg-check">
                  <CheckIcon state={c.state} />
                  <span className="oc-ellipsis oc-selectable">{c.name}</span>
                  <span className="oc-faint fg-small">{CHECK_LABEL[c.state]}</span>
                  {url && (
                    <button className="fg-link fg-check-link" onClick={() => void openExternal(url)}>
                      Details
                    </button>
                  )}
                </div>
              );
            })}
        </section>

        <section className="fg-section" aria-label="Changed files">
          <h2 className="fg-h2">Changed files</h2>
          {!localBranch && !changes && (
            <div className="oc-muted">
              Files can be compared after this {short} is checked out: OnCode fetches it into the local branch <code className="fg-code">{branch}</code> and diffs that branch. Use “Checkout to view changes” above.
            </div>
          )}
          {localBranch && !changes && <div className="oc-muted">Use “View Changes” above to see what the local branch <code className="fg-code">{branch}</code> changed since it branched off <code className="fg-code">{pr.targetBranch}</code>.</div>}
          {stale && (
            <div className="fg-note" role="status">
              The local branch {branch} is at {localBranch!.oid.slice(0, 8)}, but the {short} head is {pr.headSha.slice(0, 8)}. {onBranch ? "Switch to another branch, then check out again to update it." : "Check out again to update it."}
            </div>
          )}
          {changes?.status === "loading" && <div className="oc-faint">Comparing…</div>}
          {changes?.status === "error" && (
            <div role="alert">
              <div className="oc-selectable">{changes.message}</div>
              {changes.detail && changes.detail !== changes.message && <div className="oc-muted oc-selectable">{changes.detail}</div>}
              <button className="oc-btn oc-btn-sm" style={{ marginTop: 8 }} disabled={busy !== null} onClick={() => void gitFetch(root, remote).then((ok) => (ok ? loadChanges(pr.targetBranch) : undefined))}>
                Fetch {remote} and retry
              </button>
            </div>
          )}
          {changes?.status === "ready" && (
            <>
              <div className="oc-faint fg-small fg-compare-note">
                {changes.mergeBase ? (
                  <>
                    Showing what <code className="fg-code">{branch}</code> changed since it branched off <code className="fg-code">{changes.base}</code> (merge base <code className="fg-code">{changes.mergeBase.slice(0, 8)}</code>). Both are local refs: {changes.base} as last fetched, {branch} as last checked out.
                  </>
                ) : (
                  <>
                    <code className="fg-code">{changes.base}</code> and <code className="fg-code">{branch}</code> have no common ancestor, so this is a direct comparison of the two as they are locally; it can differ from what {forgeName(kind)} shows.
                  </>
                )}
              </div>
              {changes.value.length === 0 && <div className="oc-muted">No differences.</div>}
              {changes.value.map((f) => (
                <div
                  key={f.path}
                  className="oc-list-row fg-file"
                  role="button"
                  tabIndex={0}
                  title={`${f.origPath ? `${f.origPath} → ` : ""}${f.path}`}
                  onClick={() => openDiff({ root, path: f.path, origPath: f.origPath, source: { kind: "range", from: changes.from!, to: branch } })}
                  onKeyDown={(e) => e.key === "Enter" && openDiff({ root, path: f.path, origPath: f.origPath, source: { kind: "range", from: changes.from!, to: branch } }, { preview: false })}
                >
                  <span className={`oc-status-letter ${KIND_CLASS[f.kind]}`}>{KIND_LETTER[f.kind]}</span>
                  <span className="oc-ellipsis">{f.path}</span>
                  {(f.additions !== null || f.deletions !== null) && (
                    <span className="fg-file-stat">
                      <span className="oc-git-added">+{f.additions ?? 0}</span> <span className="oc-git-deleted">−{f.deletions ?? 0}</span>
                    </span>
                  )}
                </div>
              ))}
            </>
          )}
        </section>

        <section className="fg-section" aria-label="Description">
          <h2 className="fg-h2">Description</h2>
          {body.trim() ? <Markdown source={body} /> : <div className="oc-faint">No description provided.</div>}
        </section>
      </div>
    </div>
  );
}

// Sidebar Pull Requests: akun yang login dan daftar PR/MR repositori aktif.
import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { ChevronDown, ChevronRight, ExternalLink, GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft, LogOut, Plus, RefreshCw } from "lucide-react";
import { executeCommand } from "@/app/commands";
import { confirm, showError } from "@/app/overlay";
import { useApp } from "@/app/store";
import { openTab } from "@/app/workbench";
import { commands, type Account, type ForgeContext, type ForgeKind, type PrFilter, type PullRequest } from "@/ipc";
import { useActiveRepo } from "@/features/scm/store";
import { signIn, signInOtherHost } from "./LoginDialog";
import { installForgeStore, loadMorePrs, loadPrs, refreshForge, resolveContext, setFilter, useForge } from "./store";
import { forgeName, hostName, loginHostFor, openExternal, prNoun, prNumber, relativeTime, safeImageUrl } from "./util";
import "./forge.css";

const FILTERS: { value: PrFilter; label: string }[] = [
  { value: "open", label: "Open" },
  { value: "mine", label: "Created by me" },
  { value: "reviewRequested", label: "Review requested" },
  { value: "all", label: "All" },
];

export interface PrTabParams {
  root: string;
  accountId: string;
  repo: ForgeContext["repo"];
  kind: ForgeKind;
  remote: string;
  number: number;
}

function shorten(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text;
}

/** Membuka tab detail sebuah PR/MR. */
export function openPullRequest(root: string, context: ForgeContext, pr: Pick<PullRequest, "number" | "title">, options: { preview?: boolean } = {}): void {
  if (!context.account) return;
  const kind = context.kind ?? context.account.kind;
  const params: PrTabParams = { root, accountId: context.account.id, repo: context.repo, kind, remote: context.remote, number: pr.number };
  openTab(
    {
      id: `pullRequest:${context.account.id}:${context.repo.host}/${context.repo.owner}/${context.repo.name}:${pr.number}`,
      kind: "pullRequest",
      title: `${prNumber(kind, pr.number)} ${shorten(pr.title, 40)}`,
      description: `${prNoun(kind)} ${prNumber(kind, pr.number)} · ${context.repo.owner}/${context.repo.name}\n${pr.title}`,
      params: { ...params },
    },
    { preview: options.preview ?? false },
  );
}

function Section({ title, count, actions, children }: { title: string; count?: number; actions?: ReactNode; children: ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <div>
      <div className="oc-section-header" role="button" tabIndex={0} aria-expanded={open} onClick={() => setOpen(!open)} onKeyDown={(e) => e.key === "Enter" && setOpen(!open)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span>{title}</span>
        {count !== undefined && <span className="oc-count">{count}</span>}
        <div className="oc-row-actions" onClick={(e) => e.stopPropagation()}>
          {actions}
        </div>
      </div>
      {open && children}
    </div>
  );
}

/** Avatar dari forge: hanya URL https, tanpa referrer; selain itu inisial. */
export function Avatar({ url, name, size = 18 }: { url: string | null; name: string; size?: number }) {
  const safe = safeImageUrl(url);
  const [failed, setFailed] = useState(false);
  const style = { width: size, height: size };
  if (safe && !failed) {
    return <img className="fg-avatar" style={style} src={safe} alt="" referrerPolicy="no-referrer" loading="lazy" draggable={false} onError={() => setFailed(true)} />;
  }
  return (
    <span className="fg-avatar fg-avatar-fallback" style={{ ...style, fontSize: Math.round(size * 0.55) }} aria-hidden="true">
      {(name.trim()[0] ?? "?").toUpperCase()}
    </span>
  );
}

export async function signOut(account: Account): Promise<boolean> {
  const ok = await confirm({
    title: `Sign out ${account.username}?`,
    message: `OnCode removes the saved sign-in for ${hostName(account.host)} from the system keychain. The token itself stays valid on ${forgeName(account.kind)} until you revoke it there.`,
    confirmLabel: "Sign Out",
  });
  if (!ok) return false;
  try {
    await commands.forgeLogout(account.id);
    return true;
  } catch (e) {
    showError(e, "Could not sign out");
    return false;
  }
}

function AccountsSection() {
  const accounts = useForge((s) => s.accounts);
  return (
    <Section title="Accounts" count={accounts.length}>
      {accounts.map((a) => (
        <div key={a.id} className="fg-account" title={`${a.displayName} (${a.username}) · ${a.host}`}>
          <Avatar url={a.avatarUrl} name={a.username} size={22} />
          <div className="fg-account-text">
            <div className="oc-ellipsis">{a.username}</div>
            <div className="oc-faint oc-ellipsis fg-small">
              {forgeName(a.kind)} · {hostName(a.host)}
            </div>
          </div>
          <button className="oc-iconbtn" title={`Sign out ${a.username}`} aria-label={`Sign out ${a.username}`} onClick={() => void signOut(a)}>
            <LogOut size={14} />
          </button>
        </div>
      ))}
      {accounts.length === 0 && <div className="fg-empty">No account is signed in.</div>}
      <div className="fg-buttons">
        <button className="oc-btn oc-btn-sm" onClick={() => void signIn("github")}>Sign in to GitHub</button>
        <button className="oc-btn oc-btn-sm" onClick={() => void signIn("gitlab")}>Sign in to GitLab</button>
        <button className="oc-btn oc-btn-sm" onClick={() => void signInOtherHost()}>Other host…</button>
      </div>
    </Section>
  );
}

function StateIcon({ pr }: { pr: PullRequest }) {
  if (pr.state === "merged") return <GitMerge size={15} className="fg-state" data-state="merged" aria-label="Merged" />;
  if (pr.state === "closed") return <GitPullRequestClosed size={15} className="fg-state" data-state="closed" aria-label="Closed" />;
  if (pr.draft) return <GitPullRequestDraft size={15} className="fg-state" data-state="draft" aria-label="Draft" />;
  return <GitPullRequest size={15} className="fg-state" data-state="open" aria-label="Open" />;
}

function PrRow({ root, context, pr }: { root: string; context: ForgeContext; pr: PullRequest }) {
  const kind = context.kind ?? context.account?.kind ?? null;
  const open = (preview: boolean) => openPullRequest(root, context, pr, { preview });
  return (
    <div
      className="fg-pr"
      role="button"
      tabIndex={0}
      title={`${prNumber(kind, pr.number)} ${pr.title}`}
      onClick={() => open(true)}
      onDoubleClick={() => open(false)}
      onKeyDown={(e) => e.key === "Enter" && open(false)}
    >
      <StateIcon pr={pr} />
      <div className="fg-pr-text">
        <div className="fg-pr-title">
          <span className="oc-faint">{prNumber(kind, pr.number)}</span> <span>{pr.title}</span>
          {pr.draft && pr.state === "open" && <span className="fg-badge">Draft</span>}
        </div>
        <div className="fg-pr-meta oc-faint">
          <span className="oc-ellipsis">{pr.author}</span>
          <span className="oc-ellipsis fg-branches" title={`${pr.sourceBranch} → ${pr.targetBranch}`}>
            {pr.sourceBranch} → {pr.targetBranch}
          </span>
          <span className="fg-nowrap">{relativeTime(pr.updatedAt)}</span>
        </div>
      </div>
    </div>
  );
}

function SignInPrompt({ context }: { context: ForgeContext }) {
  const host = context.repo.host;
  const noun = prNoun(context.kind, { plural: true, lower: true });
  return (
    <div className="fg-empty">
      <div>
        This repository is hosted on <strong>{host}</strong>. Sign in to see its {noun}.
      </div>
      <div className="fg-buttons" style={{ padding: "8px 0 0" }}>
        {context.kind ? (
          <button className="oc-btn oc-btn-sm oc-btn-primary" onClick={() => void signIn(context.kind!, loginHostFor(context.kind!, host))}>
            Sign in to {host}
          </button>
        ) : (
          <>
            <button className="oc-btn oc-btn-sm" onClick={() => void signIn("github", `https://${host}`)}>Sign in (GitHub Enterprise)</button>
            <button className="oc-btn oc-btn-sm" onClick={() => void signIn("gitlab", `https://${host}`)}>Sign in (GitLab)</button>
          </>
        )}
        <button className="oc-btn oc-btn-sm" onClick={() => void openExternal(context.webUrl)}>
          <ExternalLink size={13} /> Open in browser
        </button>
      </div>
    </div>
  );
}

function PullRequestsSection() {
  const workspace = useApp((s) => s.workspace);
  const repo = useActiveRepo();
  const root = repo?.root ?? null;
  const accounts = useForge((s) => s.accounts);
  const context = useForge((s) => s.context);
  const contextStatus = useForge((s) => s.contextStatus);
  const contextError = useForge((s) => s.contextError);
  const filter = useForge((s) => s.filter);
  const prs = useForge((s) => s.prs);
  const nextPage = useForge((s) => s.nextPage);
  const prStatus = useForge((s) => s.prStatus);
  const loadingMore = useForge((s) => s.loadingMore);
  const prError = useForge((s) => s.prError);

  // Konteks di-resolve ulang saat repositori, remote-nya, atau daftar akun berubah.
  const remotesKey = (repo?.refs?.remotes ?? []).map((r) => `${r.name}=${r.fetchUrl}`).join("\n");
  const accountsKey = accounts.map((a) => a.id).join("\n");
  useEffect(() => {
    void resolveContext(root);
  }, [root, remotesKey, accountsKey]);

  const kind = context?.kind ?? context?.account?.kind ?? null;
  const title = prNoun(kind, { plural: true });
  const ready = !!root && contextStatus === "ready" && !!context?.account;

  let body: ReactNode;
  if (!workspace) body = <div className="fg-empty">Open a folder to see pull requests.</div>;
  else if (!workspace.trusted) body = <div className="fg-empty">Git features are disabled in restricted mode.</div>;
  else if (!root) body = <div className="fg-empty">Open a Git repository to see its pull requests.</div>;
  else if (contextStatus === "error") {
    body = (
      <div className="fg-empty" role="alert">
        <div className="oc-selectable">Could not read the remotes of this repository: {contextError}</div>
        <button className="oc-btn oc-btn-sm" style={{ marginTop: 8 }} onClick={() => void resolveContext(root)}>Retry</button>
      </div>
    );
  } else if (contextStatus !== "ready") body = <div className="fg-empty">Loading…</div>;
  else if (!context) body = <div className="fg-empty">No remote of this repository points to GitHub or GitLab.</div>;
  else if (!context.account) body = <SignInPrompt context={context} />;
  else {
    const noun = prNoun(kind, { plural: true, lower: true });
    body = (
      <>
        <div className="fg-toolbar">
          <select className="oc-select" aria-label="Filter" value={filter} onChange={(e) => setFilter(e.target.value as PrFilter)}>
            {FILTERS.map((f) => (
              <option key={f.value} value={f.value}>{f.label}</option>
            ))}
          </select>
        </div>
        <div className="fg-buttons">
          <button className="oc-btn oc-btn-sm oc-btn-primary" onClick={() => void executeCommand("forge.createPullRequest").catch(showError)}>
            <Plus size={13} /> Create {prNoun(kind)}…
          </button>
          <button className="oc-btn oc-btn-sm" onClick={() => void openExternal(context.webUrl)}>
            <ExternalLink size={13} /> Open on {forgeName(kind)}
          </button>
        </div>
        <div className="fg-repo oc-faint oc-ellipsis" title={`${context.repo.host}/${context.repo.owner}/${context.repo.name} (remote ${context.remote})`}>
          {context.repo.owner}/{context.repo.name}
        </div>
        {prStatus === "loading" && <div className="fg-empty">Loading {noun}…</div>}
        {prStatus === "error" && (
          <div className="fg-empty" role="alert">
            <div className="oc-selectable">{prError}</div>
            <button className="oc-btn oc-btn-sm" style={{ marginTop: 8 }} onClick={() => void loadPrs()}>Retry</button>
          </div>
        )}
        {prStatus === "ready" && prs.length === 0 && nextPage === null && <div className="fg-empty">No {noun} match this filter.</div>}
        {prStatus === "ready" && prs.map((pr) => <PrRow key={pr.number} root={root!} context={context} pr={pr} />)}
        {prStatus === "ready" && prError && <div className="fg-empty oc-error-text" role="alert">{prError}</div>}
        {prStatus === "ready" && nextPage !== null && (
          <div className="fg-buttons">
            <button className="oc-btn oc-btn-sm oc-btn-block" disabled={loadingMore} onClick={() => void loadMorePrs()}>
              {loadingMore ? "Loading…" : "Load more"}
            </button>
          </div>
        )}
      </>
    );
  }

  return (
    <Section
      title={title}
      count={ready && prStatus === "ready" ? prs.length : undefined}
      actions={
        ready ? (
          <button className="oc-iconbtn" title="Refresh" aria-label={`Refresh ${title.toLowerCase()}`} onClick={() => void loadPrs()}>
            <RefreshCw size={13} className={prStatus === "loading" ? "oc-spin" : undefined} />
          </button>
        ) : undefined
      }
    >
      {body}
    </Section>
  );
}

export function ForgeView() {
  useEffect(() => installForgeStore(), []);
  const busy = useForge((s) => s.contextStatus === "loading" || s.prStatus === "loading");
  return (
    <>
      <div className="oc-sidebar-title">
        <span className="oc-ellipsis">Pull Requests</span>
        <div className="oc-grow" />
        <button className="oc-iconbtn" title="Refresh" aria-label="Refresh" onClick={() => void refreshForge()}>
          <RefreshCw size={14} className={busy ? "oc-spin" : undefined} />
        </button>
      </div>
      <div className="oc-sidebar-body">
        <PullRequestsSection />
        <AccountsSection />
      </div>
    </>
  );
}

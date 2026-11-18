// Aksi Git dari sisi UI: konfirmasi untuk aksi destruktif, progres, dan galat yang bisa ditindaklanjuti.
import { Channel } from "@tauri-apps/api/core";
import { commands, errorCode, errorDetail, errorMessage, newOperationId, type MergeMode, type Progress, type PushOptions, type ResetMode, type RepoSnapshot, type StashOptions } from "@/ipc";
import { choose, confirm, promptInput, quickPick, showError, toast, type PickItem } from "@/app/overlay";
import { revealSidebar, showPanel, useApp } from "@/app/store";
import { activeRepo, headLabel, setBusy, useScm } from "./store";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Menjalankan operasi Git dengan indikator sibuk; galat ditangani dan mengembalikan false. */
export async function run(root: string, label: string, operation: () => Promise<unknown>, context?: { retry?: () => Promise<unknown> }): Promise<boolean> {
  setBusy(root, { label, percent: null });
  try {
    await operation();
    return true;
  } catch (e) {
    handleGitError(root, e, label, context?.retry);
    return false;
  } finally {
    setBusy(root, null);
  }
}

/** Menjalankan operasi jaringan dengan progres dan tombol batal. */
export async function runWithProgress(root: string, label: string, operation: (operationId: string, onProgress: Channel<Progress>) => Promise<unknown>): Promise<boolean> {
  const operationId = newOperationId();
  const channel = new Channel<Progress>();
  channel.onmessage = (p) => setBusy(root, { label: `${label}: ${p.phase}`, percent: p.percent, operationId });
  setBusy(root, { label, percent: null, operationId });
  try {
    await operation(operationId, channel);
    return true;
  } catch (e) {
    handleGitError(root, e, label);
    return false;
  } finally {
    setBusy(root, null);
  }
}

export function cancelBusy(root: string): void {
  const id = useScm.getState().repos[root]?.busy?.operationId;
  if (id) void commands.appCancel(id);
}

/** Menerjemahkan kode galat Git menjadi pesan dengan tombol aksi berikutnya. */
export function handleGitError(root: string, error: unknown, label: string, retry?: () => Promise<unknown>): void {
  const code = errorCode(error);
  const message = errorMessage(error);
  const detail = errorDetail(error);
  const details = detail
    ? [{ label: "Details", run: () => void choose({ title: label, message, detail, buttons: [{ id: "ok", label: "Close", kind: "primary" as const }] }) }]
    : [];
  switch (code) {
    case "cancelled":
      return;
    case "pushRejected":
      toast.error(`${label} was rejected because the remote has commits you don't have locally.`, [
        { label: "Pull, then push", run: async () => (await pull(root)) && push(root) },
        { label: "Force push…", run: () => push(root, { forceWithLease: true }) },
        ...details,
      ]);
      return;
    case "noUpstream":
      toast.error("This branch is not published yet.", [{ label: "Publish branch", run: () => push(root, { setUpstream: true }) }]);
      return;
    case "dirtyWorkTree":
      toast.error(`${label} would overwrite your local changes.`, [
        ...(retry
          ? [
              {
                label: "Stash and continue",
                run: async () => {
                  if (await run(root, "Stash", () => commands.gitStashPush(root, { message: `Auto-stash before ${label.toLowerCase()}`, includeUntracked: false, stagedOnly: false, paths: [] }))) {
                    if (await run(root, label, retry)) toast.info("Your changes were stashed. Use Stashes → Pop to restore them.");
                  }
                },
              },
            ]
          : []),
        ...details,
      ]);
      return;
    case "conflict":
      revealSidebar("scm");
      toast.warning(`${label} stopped because of conflicts. Resolve them in Source Control, then continue.`, details);
      return;
    case "indexLocked":
      toast.error("Another Git process seems to be running in this repository (index.lock exists). Try again in a moment.", details);
      return;
    case "hookFailed":
      toast.error(`${label} was stopped by a Git hook.`, [{ label: "Show output", run: () => showPanel("gitOutput") }, ...details]);
      return;
    case "authFailed":
      toast.error(`${label} failed: authentication was rejected.`, [{ label: "Accounts", run: () => revealSidebar("pullRequests") }, ...details]);
      return;
    case "nothingToCommit":
      toast.info("There is nothing to commit.");
      return;
    case "stale":
      toast.warning(message);
      return;
    default:
      toast.error(`${label} failed: ${message}`, details);
  }
}

// ----- Staging -----

export const stage = (root: string, paths: string[]) => run(root, "Stage", () => commands.gitStage(root, paths));
export const stageAll = (root: string) => run(root, "Stage all", () => commands.gitStageAll(root));
export const unstage = (root: string, paths: string[]) => run(root, "Unstage", () => commands.gitUnstage(root, paths));
export const unstageAll = (root: string) => run(root, "Unstage all", () => commands.gitUnstageAll(root));

/** Membuang perubahan setelah konfirmasi yang menyebut apa yang akan hilang. */
export async function discard(root: string, tracked: string[], untracked: string[]): Promise<boolean> {
  const total = tracked.length + untracked.length;
  if (total === 0) return false;
  const single = total === 1 ? (tracked[0] ?? untracked[0]) : null;
  const parts: string[] = [];
  if (tracked.length) parts.push(`Changes to ${plural(tracked.length, "tracked file")} will be lost permanently.`);
  if (untracked.length) parts.push(`${plural(untracked.length, "untracked file")} will be moved to the trash.`);
  const ok = await confirm({
    title: single ? `Discard changes in ${single}?` : `Discard changes in ${plural(total, "file")}?`,
    message: parts.join("\n"),
    confirmLabel: "Discard",
    danger: true,
  });
  return ok && run(root, "Discard", () => commands.gitDiscard(root, tracked, untracked));
}

/** Membagi daftar path menjadi berkas terlacak dan untracked menurut snapshot. */
export function splitTracked(snapshot: RepoSnapshot, paths: string[]): { tracked: string[]; untracked: string[] } {
  const untrackedSet = new Set(snapshot.untracked);
  return { tracked: paths.filter((p) => !untrackedSet.has(p)), untracked: paths.filter((p) => untrackedSet.has(p)) };
}

// ----- Commit -----

export interface CommitRequest {
  message: string;
  amend: boolean;
  signoff: boolean;
  then?: "push" | "sync";
}

async function ensureIdentity(root: string): Promise<boolean> {
  const identity = await commands.gitIdentity(root);
  if (identity.name && identity.email) return true;
  const name = await promptInput({ title: "Git needs to know who you are", label: "Your name (user.name)", value: identity.name ?? "" });
  if (!name) return false;
  const email = await promptInput({
    title: "Git needs to know who you are",
    label: "Your email (user.email)",
    value: identity.email ?? "",
    validate: (v) => (/^\S+@\S+$/.test(v.trim()) ? null : "Enter an email address"),
  });
  if (!email) return false;
  return run(root, "Set identity", () => commands.gitSetIdentity(root, name.trim(), email.trim(), true));
}

export async function commit(root: string, request: CommitRequest): Promise<boolean> {
  const snapshot = useScm.getState().repos[root]?.snapshot;
  if (!snapshot) return false;
  if (!request.message.trim()) {
    toast.info("Enter a commit message first.");
    return false;
  }
  if (!(await ensureIdentity(root))) return false;
  if (snapshot.staged.length === 0 && !request.amend) {
    const count = snapshot.unstaged.length + snapshot.untracked.length;
    if (count === 0) {
      toast.info("There are no changes to commit.");
      return false;
    }
    const ok = await confirm({
      title: "No staged changes",
      message: `Stage all ${plural(count, "changed file")} and commit them?`,
      confirmLabel: "Stage All and Commit",
    });
    if (!ok || !(await stageAll(root))) return false;
  }
  const done = await run(root, "Commit", () =>
    commands.gitCommit(root, request.message, { amend: request.amend, signoff: request.signoff, gpgSign: false, noVerify: false, allowEmpty: false }),
  );
  if (!done) return false;
  if (request.then === "push") await push(root);
  if (request.then === "sync") await sync(root);
  return true;
}

export async function undoLastCommit(root: string): Promise<void> {
  const ok = await confirm({
    title: "Undo last commit?",
    message: "The commit is removed from the branch and its changes return to the staging area. Nothing is lost.",
    confirmLabel: "Undo Commit",
  });
  if (ok) await run(root, "Undo commit", () => commands.gitUndoCommit(root));
}

// ----- Remote -----

export const fetch = (root: string, remote: string | null = null) =>
  runWithProgress(root, "Fetch", (id, progress) => commands.gitFetch(root, remote, id, progress));

export async function pull(root: string): Promise<boolean> {
  const strategy = useApp.getState().settings?.git.pullStrategy ?? "ffOnly";
  const ok = await runWithProgressRaw(root, "Pull", (id, progress) => commands.gitPull(root, strategy, id, progress));
  if (ok === true) return true;
  // ff-only gagal karena riwayat bercabang: tawarkan cara lain.
  if (strategy === "ffOnly" && ok !== "cancelled" && /fast-forward|divergent|diverg/i.test(`${errorMessage(ok)} ${errorDetail(ok) ?? ""}`)) {
    const answer = await choose({
      title: "Branches have diverged",
      message: "Your branch and its upstream both have new commits, so a fast-forward is not possible. How do you want to combine them?",
      buttons: [
        { id: "cancel", label: "Cancel" },
        { id: "merge", label: "Merge" },
        { id: "rebase", label: "Rebase", kind: "primary" },
      ],
    });
    if (answer === "merge" || answer === "rebase") {
      return runWithProgress(root, "Pull", (id, progress) => commands.gitPull(root, answer, id, progress));
    }
    return false;
  }
  if (ok !== "cancelled") handleGitError(root, ok, "Pull");
  return false;
}

/** Seperti `runWithProgress`, tetapi mengembalikan galatnya agar pemanggil bisa memutuskan. */
async function runWithProgressRaw(root: string, label: string, operation: (operationId: string, onProgress: Channel<Progress>) => Promise<unknown>): Promise<true | "cancelled" | unknown> {
  const operationId = newOperationId();
  const channel = new Channel<Progress>();
  channel.onmessage = (p) => setBusy(root, { label: `${label}: ${p.phase}`, percent: p.percent, operationId });
  setBusy(root, { label, percent: null, operationId });
  try {
    await operation(operationId, channel);
    return true;
  } catch (e) {
    return errorCode(e) === "cancelled" ? "cancelled" : e;
  } finally {
    setBusy(root, null);
  }
}

export async function push(root: string, options: Partial<PushOptions> = {}): Promise<boolean> {
  const snapshot = useScm.getState().repos[root]?.snapshot;
  const full: PushOptions = { remote: null, branch: null, setUpstream: false, forceWithLease: false, tags: false, ...options };
  if (snapshot && snapshot.head.kind === "detached" && !full.branch) {
    toast.info("You are not on a branch. Create a branch before pushing.");
    return false;
  }
  if (snapshot && !snapshot.upstream && !full.setUpstream && !full.branch) full.setUpstream = true;
  if (full.setUpstream && !full.remote) {
    const remotes = useScm.getState().repos[root]?.refs?.remotes ?? [];
    if (remotes.length === 0) {
      toast.error("This repository has no remote yet.", [{ label: "Add remote…", run: () => addRemote(root) }]);
      return false;
    }
    if (remotes.length > 1) {
      const picked = await quickPick({ placeholder: "Publish to which remote?", items: remotes.map((r) => ({ label: r.name, description: r.pushUrl || r.fetchUrl, value: r.name })) });
      if (!picked) return false;
      full.remote = picked;
    }
  }
  if (full.forceWithLease && (useApp.getState().settings?.git.confirmForcePush ?? true)) {
    const target = snapshot?.upstream?.name ?? headLabel(snapshot ?? null);
    const ok = await confirm({
      title: "Force push?",
      message: `This overwrites ${target} on the remote with your local branch. Commits that exist only on the remote will be removed from that branch.\n\nThe push is refused if someone else pushed since your last fetch (--force-with-lease).`,
      confirmLabel: "Force Push",
      danger: true,
    });
    if (!ok) return false;
  }
  return runWithProgress(root, full.setUpstream ? "Publish" : "Push", (id, progress) => commands.gitPush(root, full, id, progress));
}

/** Pull lalu push. */
export async function sync(root: string): Promise<boolean> {
  const snapshot = useScm.getState().repos[root]?.snapshot;
  if (!snapshot?.upstream?.exists) return push(root, { setUpstream: true });
  if (!(await pull(root))) return false;
  // Push tanpa commit baru tidak berbahaya ("Everything up-to-date").
  return push(root);
}

export async function addRemote(root: string): Promise<void> {
  const name = await promptInput({ title: "Add remote", label: "Remote name", value: "origin", validate: validateRefName });
  if (!name) return;
  const url = await promptInput({ title: "Add remote", label: "Remote URL", placeholder: "https://example.com/owner/repo.git" });
  if (!url) return;
  await run(root, "Add remote", () => commands.gitRemoteAdd(root, name.trim(), url.trim()));
}

export async function removeRemote(root: string, name: string): Promise<void> {
  const ok = await confirm({ title: `Remove remote ${name}?`, message: "Its remote-tracking branches are removed locally. Nothing changes on the server.", confirmLabel: "Remove", danger: true });
  if (ok) await run(root, "Remove remote", () => commands.gitRemoteRemove(root, name));
}

// ----- Branch -----

/** Aturan dasar nama ref Git, agar galat muncul sebelum perintah dijalankan. */
export function validateRefName(value: string): string | null {
  const name = value.trim();
  if (!name) return "Enter a name";
  if (name.startsWith("-")) return "Names cannot start with a dash";
  if (/[\s~^:?*[\\]|\.\.|@\{|\/\/|^\/|\/$|\.$|\.lock$/.test(name) || /[\x00-\x1f\x7f]/.test(name)) return "This is not a valid Git ref name";
  return null;
}

export async function createBranch(root: string, start?: string): Promise<void> {
  const name = await promptInput({
    title: start ? `Create branch at ${start.length >= 40 ? start.slice(0, 8) : start}` : "Create branch",
    label: "Branch name",
    placeholder: "feature/my-change",
    validate: validateRefName,
  });
  if (!name) return;
  const branch = name.trim();
  await run(root, "Create branch", () => commands.gitBranchCreate(root, branch, start ?? null, true), {
    retry: () => commands.gitBranchCreate(root, branch, start ?? null, true),
  });
}

export async function checkout(root: string, target: { kind: "branch" | "remote" | "detached"; name: string }): Promise<boolean> {
  const call =
    target.kind === "branch"
      ? () => commands.gitSwitch(root, target.name)
      : target.kind === "remote"
        ? () => commands.gitSwitchTracking(root, target.name)
        : () => commands.gitSwitchDetached(root, target.name);
  if (target.kind === "remote") {
    // Bila branch lokal dengan nama yang sama sudah ada, pindah ke sana saja.
    const local = target.name.split("/").slice(1).join("/");
    if (useScm.getState().repos[root]?.refs?.branches.some((b) => b.name === local)) {
      return run(root, "Checkout", () => commands.gitSwitch(root, local), { retry: () => commands.gitSwitch(root, local) });
    }
  }
  return run(root, "Checkout", call, { retry: call });
}

/** Daftar pilih branch untuk berpindah (dari status bar). */
export async function pickBranch(root: string): Promise<void> {
  const refs = useScm.getState().repos[root]?.refs;
  if (!refs) return;
  type Target = { kind: "branch" | "remote" | "detached" | "create"; name: string };
  const items: PickItem<Target>[] = [
    { label: "+ Create new branch…", value: { kind: "create", name: "" } },
    ...refs.branches.map((b) => ({ label: b.name, description: b.isHead ? "current" : b.upstream ? `→ ${b.upstream}` : "", value: { kind: "branch" as const, name: b.name } })),
    ...refs.remoteBranches.map((b) => ({ label: b.name, description: "remote", value: { kind: "remote" as const, name: b.name } })),
    ...refs.tags.map((t) => ({ label: t.name, description: "tag", value: { kind: "detached" as const, name: t.name } })),
  ];
  const picked = await quickPick({ placeholder: "Switch to branch, remote branch or tag", items });
  if (!picked) return;
  if (picked.kind === "create") return createBranch(root);
  await checkout(root, picked as { kind: "branch" | "remote" | "detached"; name: string });
}

export async function renameBranch(root: string, name: string): Promise<void> {
  const next = await promptInput({ title: `Rename branch ${name}`, label: "New name", value: name, validate: validateRefName });
  if (next && next.trim() !== name) await run(root, "Rename branch", () => commands.gitBranchRename(root, name, next.trim()));
}

export async function deleteBranch(root: string, name: string): Promise<void> {
  try {
    setBusy(root, { label: "Delete branch", percent: null });
    await commands.gitBranchDelete(root, name, false);
  } catch (e) {
    setBusy(root, null);
    if (errorCode(e) !== "branchNotMerged") return handleGitError(root, e, "Delete branch");
    const count = await commands.gitBranchUnmerged(root, name).catch(() => 0);
    const ok = await confirm({
      title: `Delete unmerged branch ${name}?`,
      message: `${plural(count, "commit")} on this branch ${count === 1 ? "exists" : "exist"} nowhere else. Deleting the branch makes ${count === 1 ? "it" : "them"} unreachable (recoverable only through the reflog for a limited time).`,
      confirmLabel: "Delete Anyway",
      danger: true,
    });
    if (ok) await run(root, "Delete branch", () => commands.gitBranchDelete(root, name, true));
  } finally {
    setBusy(root, null);
  }
}

export async function deleteRemoteBranch(root: string, fullName: string): Promise<void> {
  const [remote, ...rest] = fullName.split("/");
  const branch = rest.join("/");
  const ok = await confirm({ title: `Delete ${branch} on ${remote}?`, message: "This deletes the branch on the server for everyone.", confirmLabel: "Delete Remote Branch", danger: true });
  if (ok) await run(root, "Delete remote branch", () => commands.gitBranchDeleteRemote(root, remote, branch));
}

// ----- Merge, rebase, operasi berlanjut -----

export async function merge(root: string, rev: string, mode: MergeMode = "default"): Promise<void> {
  const current = headLabel(useScm.getState().repos[root]?.snapshot ?? null);
  const ok = await confirm({ title: `Merge ${rev} into ${current}?`, confirmLabel: "Merge" });
  if (ok) await run(root, "Merge", () => commands.gitMerge(root, rev, mode), { retry: () => commands.gitMerge(root, rev, mode) });
}

export async function rebaseOnto(root: string, onto: string): Promise<void> {
  const current = headLabel(useScm.getState().repos[root]?.snapshot ?? null);
  const ok = await confirm({
    title: `Rebase ${current} onto ${onto}?`,
    message: "Your commits are re-created on top of the target. If they were already pushed, you will need to force push afterwards.",
    confirmLabel: "Rebase",
  });
  if (ok) await run(root, "Rebase", () => commands.gitRebase(root, onto), { retry: () => commands.gitRebase(root, onto) });
}

export async function operation(root: string, action: "continue" | "skip" | "abort"): Promise<void> {
  const snapshot = useScm.getState().repos[root]?.snapshot;
  const name = snapshot?.operation?.kind ?? "operation";
  const pretty = name === "cherryPick" ? "cherry-pick" : name;
  if (action === "abort") {
    const ok = await confirm({
      title: `Abort ${pretty}?`,
      message: `The repository returns to the state before the ${pretty} started. Conflict resolutions made so far are discarded.`,
      confirmLabel: "Abort",
      danger: true,
    });
    if (!ok) return;
  }
  if (action === "continue" && snapshot && snapshot.conflicted.length > 0) {
    toast.warning(`Resolve the remaining ${plural(snapshot.conflicted.length, "conflict")} before continuing.`);
    return;
  }
  const label = `${action.charAt(0).toUpperCase()}${action.slice(1)} ${pretty}`;
  await run(root, label, () => commands.gitOperation(root, action));
}

// ----- Cherry-pick, revert, reset, tag -----

export const cherryPick = (root: string, oids: string[]) => run(root, "Cherry-pick", () => commands.gitCherryPick(root, oids), { retry: () => commands.gitCherryPick(root, oids) });

export async function revert(root: string, oid: string, subject: string): Promise<void> {
  const ok = await confirm({ title: "Revert commit?", message: `A new commit is created that undoes:\n${oid.slice(0, 8)}  ${subject}`, confirmLabel: "Revert" });
  if (ok) await run(root, "Revert", () => commands.gitRevert(root, oid), { retry: () => commands.gitRevert(root, oid) });
}

export async function reset(root: string, rev: string, mode: ResetMode): Promise<void> {
  const snapshot = useScm.getState().repos[root]?.snapshot;
  const short = rev.length >= 40 ? rev.slice(0, 8) : rev;
  const current = headLabel(snapshot ?? null);
  let message: string;
  if (mode === "hard") {
    const changed = snapshot ? new Set([...snapshot.staged, ...snapshot.unstaged].map((c) => c.path)).size : 0;
    message =
      `${current} moves to ${short} and ALL uncommitted changes in tracked files are discarded` +
      (changed > 0 ? ` (${plural(changed, "file")} currently changed).` : ".") +
      "\n\nCommits that become unreachable can still be found in the reflog (View → Reflog) for a limited time; discarded uncommitted changes cannot be recovered.";
  } else if (mode === "soft") {
    message = `${current} moves to ${short}. The changes from the commits in between stay staged. Nothing is lost.`;
  } else {
    message = `${current} moves to ${short}. The changes from the commits in between stay in your files, unstaged. Nothing is lost.`;
  }
  const ok = await confirm({ title: `Reset ${current} to ${short} (${mode})?`, message, confirmLabel: mode === "hard" ? "Reset and Discard" : "Reset", danger: mode === "hard" });
  if (ok) await run(root, "Reset", () => commands.gitReset(root, rev, mode));
}

export async function createTag(root: string, rev?: string): Promise<void> {
  const name = await promptInput({ title: "Create tag", label: "Tag name", placeholder: "v1.0.0", validate: validateRefName });
  if (!name) return;
  const message = await promptInput({ title: `Create tag ${name.trim()}`, label: "Message (leave empty for a lightweight tag)", allowEmpty: true, confirmLabel: "Create Tag" });
  if (message === null) return;
  const created = await run(root, "Create tag", () => commands.gitTagCreate(root, name.trim(), rev ?? null, message.trim() || null));
  const remotes = useScm.getState().repos[root]?.refs?.remotes ?? [];
  if (created && remotes.length > 0) {
    toast.success(`Tag ${name.trim()} created.`, [{ label: `Push to ${remotes[0].name}`, run: () => run(root, "Push tag", () => commands.gitTagPush(root, remotes[0].name, name.trim())) }]);
  }
}

export async function deleteTag(root: string, name: string): Promise<void> {
  const remotes = useScm.getState().repos[root]?.refs?.remotes ?? [];
  const answer = await choose({
    title: `Delete tag ${name}?`,
    message: remotes.length ? `You can also delete it on ${remotes[0].name}.` : undefined,
    buttons: [
      { id: "cancel", label: "Cancel" },
      ...(remotes.length ? [{ id: "both", label: `Delete Locally and on ${remotes[0].name}`, kind: "danger" as const }] : []),
      { id: "local", label: "Delete Locally", kind: "danger" as const },
    ],
  });
  if (answer !== "local" && answer !== "both") return;
  const ok = await run(root, "Delete tag", () => commands.gitTagDelete(root, name));
  if (ok && answer === "both") await run(root, "Delete remote tag", () => commands.gitTagDeleteRemote(root, remotes[0].name, name));
}

// ----- Stash -----

export async function stashPush(root: string, options: Partial<StashOptions> = {}): Promise<void> {
  const message = await promptInput({ title: "Stash changes", label: "Message (optional)", allowEmpty: true, confirmLabel: "Stash" });
  if (message === null) return;
  const snapshot = useScm.getState().repos[root]?.snapshot;
  let includeUntracked = options.includeUntracked ?? false;
  if (options.includeUntracked === undefined && snapshot && snapshot.untracked.length > 0 && !(options.paths?.length)) {
    const answer = await choose({
      title: "Include untracked files?",
      message: `${plural(snapshot.untracked.length, "untracked file")} can be stashed too.`,
      buttons: [
        { id: "cancel", label: "Cancel" },
        { id: "no", label: "Tracked Only" },
        { id: "yes", label: "Include Untracked", kind: "primary" },
      ],
    });
    if (answer === null || answer === "cancel") return;
    includeUntracked = answer === "yes";
  }
  await run(root, "Stash", () => commands.gitStashPush(root, { message: message.trim() || null, includeUntracked, stagedOnly: options.stagedOnly ?? false, paths: options.paths ?? [] }));
}

export const stashApply = (root: string, name: string, pop: boolean) => run(root, pop ? "Pop stash" : "Apply stash", () => commands.gitStashApply(root, name, pop));

export async function stashDrop(root: string, name: string, message: string): Promise<void> {
  const ok = await confirm({ title: `Drop ${name}?`, message: `${message}\n\nThe stashed changes are deleted permanently.`, confirmLabel: "Drop Stash", danger: true });
  if (ok) await run(root, "Drop stash", () => commands.gitStashDrop(root, name));
}

export async function stashBranch(root: string, name: string): Promise<void> {
  const branch = await promptInput({ title: `Create branch from ${name}`, label: "Branch name", validate: validateRefName });
  if (branch) await run(root, "Stash to branch", () => commands.gitStashBranch(root, name, branch.trim()));
}

// ----- Konflik -----

export const resolveWithSide = (root: string, path: string, side: "current" | "incoming") => run(root, "Resolve conflict", () => commands.gitResolveSide(root, path, side));
export const markResolved = (root: string, path: string) => run(root, "Mark resolved", () => commands.gitMarkResolved(root, path));
export const resolveByDeleting = (root: string, path: string) => run(root, "Resolve conflict", () => commands.gitResolveDelete(root, path));

/** Repositori aktif, atau pemberitahuan bila tidak ada. */
export function requireRepo(): string | null {
  const repo = activeRepo();
  if (!repo) {
    toast.info(useApp.getState().workspace?.trusted === false ? "Git is disabled in restricted mode. Trust this folder to enable it." : "This folder is not a Git repository.");
    return null;
  }
  return repo.root;
}

export { showError };

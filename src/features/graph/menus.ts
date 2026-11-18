// Menu konteks untuk commit dan ref. Semua mutasi memakai aksi dari fitur Source Control.
import { executeCommand } from "@/app/commands";
import { quickPick, toast, type MenuItem, type PickItem } from "@/app/overlay";
import { commands, type Branch, type Commit, type GraphRow, type RemoteBranch, type Stash, type Tag } from "@/ipc";
import * as git from "@/features/scm/actions";
import { useScm } from "@/features/scm/store";
import { shortOid } from "./format";
import { isAncestorOfHead } from "./refs";

export async function copyText(text: string, what = "Copied"): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast.info(`${what} copied to clipboard.`);
  } catch {
    toast.error("Could not access the clipboard.");
  }
}

export interface CommitMenuContext {
  root: string;
  commit: Commit;
  /** Baris yang sudah dimuat, untuk memeriksa apakah commit ini leluhur HEAD. */
  rows: readonly GraphRow[];
  /** Daftar disaring (grep/penulis), sehingga hubungan leluhur tidak bisa disimpulkan dari daftar. */
  filtered: boolean;
  /** Commit terpilih lain yang ikut di-cherry-pick (urut dari yang tertua). */
  selection?: string[];
}

export function commitMenu({ root, commit, rows, filtered, selection }: CommitMenuContext): MenuItem[] {
  const repo = useScm.getState().repos[root];
  const headOid = repo?.snapshot?.headOid ?? null;
  const currentBranch = repo?.snapshot?.head.kind === "branch" ? repo.snapshot.head.name : null;
  const oid = commit.oid;
  const isHead = oid === headOid;
  const ancestor = filtered ? null : isAncestorOfHead(rows, headOid, oid);
  const branches = commit.refs.filter((r) => r.kind === "branch" && r.name !== currentBranch);
  const picks = selection && selection.length > 1 && selection.includes(oid) ? selection : [oid];
  return [
    ...branches.map((b) => ({ label: `Checkout Branch ${b.name}`, run: () => git.checkout(root, { kind: "branch", name: b.name }) })),
    { label: "Checkout (detached)", disabled: isHead && currentBranch === null, run: () => git.checkout(root, { kind: "detached", name: oid }) },
    { separator: true },
    { label: "Create Branch Here…", run: () => git.createBranch(root, oid) },
    { label: "Create Tag Here…", run: () => git.createTag(root, oid) },
    { separator: true },
    { label: picks.length > 1 ? `Cherry-pick ${picks.length} Commits` : "Cherry-pick", disabled: picks.length === 1 && isHead, run: () => git.cherryPick(root, picks) },
    { label: "Revert…", run: () => git.revert(root, oid, commit.subject) },
    { separator: true },
    { label: "Reset Current Branch to Here (Soft)…", disabled: isHead, run: () => git.reset(root, oid, "soft") },
    { label: "Reset Current Branch to Here (Mixed)…", disabled: isHead, run: () => git.reset(root, oid, "mixed") },
    { label: "Reset Current Branch to Here (Hard)…", danger: true, run: () => git.reset(root, oid, "hard") },
    { separator: true },
    // Rebase menyusun ulang commit SETELAH commit ini; hanya masuk akal untuk leluhur HEAD.
    { label: "Interactive Rebase from Here…", disabled: isHead || ancestor === false, run: () => executeCommand("git.interactiveRebase", root, oid) },
    { separator: true },
    { label: "Copy Hash", run: () => copyText(oid, "Hash") },
    { label: "Copy Subject", run: () => copyText(commit.subject, "Subject") },
  ];
}

/** Memilih upstream untuk sebuah branch lokal dari daftar branch remote. */
export async function setUpstream(root: string, branch: Branch): Promise<void> {
  const remoteBranches = useScm.getState().repos[root]?.refs?.remoteBranches ?? [];
  const items: PickItem<{ upstream: string | null }>[] = remoteBranches
    .filter((b) => !b.name.endsWith("/HEAD"))
    .map((b) => ({ label: b.name, description: b.name === branch.upstream ? "current upstream" : "", value: { upstream: b.name } }));
  if (branch.upstream) items.push({ label: "Remove upstream", description: branch.upstream, value: { upstream: null } });
  if (items.length === 0) {
    toast.info("There are no remote branches yet. Push the branch to publish it.");
    return;
  }
  const picked = await quickPick({ placeholder: `Upstream for ${branch.name}`, items });
  if (!picked) return;
  await git.run(root, "Set upstream", () => commands.gitBranchSetUpstream(root, branch.name, picked.upstream));
}

export function branchMenu(root: string, branch: Branch): MenuItem[] {
  const name = branch.name;
  const elsewhere = branch.worktree !== null && !branch.isHead;
  return [
    { label: "Checkout", disabled: branch.isHead || elsewhere, run: () => git.checkout(root, { kind: "branch", name }) },
    { label: "Merge into Current…", disabled: branch.isHead, run: () => git.merge(root, name) },
    { label: "Rebase Current onto…", disabled: branch.isHead, run: () => git.rebaseOnto(root, name) },
    { separator: true },
    { label: "Rename…", run: () => git.renameBranch(root, name) },
    { label: "Delete…", danger: true, disabled: branch.isHead || elsewhere, run: () => git.deleteBranch(root, name) },
    { separator: true },
    { label: branch.upstream && !branch.upstreamGone ? "Push" : "Publish", run: () => (branch.isHead ? git.push(root) : git.push(root, { branch: name, setUpstream: !branch.upstream || branch.upstreamGone })) },
    { label: "Set Upstream…", run: () => setUpstream(root, branch) },
    { separator: true },
    { label: "Copy Name", run: () => copyText(name, "Branch name") },
  ];
}

export function remoteBranchMenu(root: string, branch: RemoteBranch): MenuItem[] {
  return [
    { label: "Checkout (tracking)", run: () => git.checkout(root, { kind: "remote", name: branch.name }) },
    { label: "Merge into Current…", run: () => git.merge(root, branch.name) },
    { label: "Rebase Current onto…", run: () => git.rebaseOnto(root, branch.name) },
    { separator: true },
    { label: "Delete on Remote…", danger: true, run: () => git.deleteRemoteBranch(root, branch.name) },
    { separator: true },
    { label: "Copy Name", run: () => copyText(branch.name, "Branch name") },
  ];
}

export function tagMenu(root: string, tag: Tag): MenuItem[] {
  return [
    { label: "Checkout (detached)", run: () => git.checkout(root, { kind: "detached", name: tag.name }) },
    { label: "Delete…", danger: true, run: () => git.deleteTag(root, tag.name) },
    { separator: true },
    { label: "Copy Name", run: () => copyText(tag.name, "Tag name") },
  ];
}

export function stashMenu(root: string, stash: Stash): MenuItem[] {
  return [
    { label: "Show Changes", run: () => executeCommand("git.showStash", root, stash.name) },
    { label: "Apply", run: () => git.stashApply(root, stash.name, false) },
    { label: "Pop (apply and drop)", run: () => git.stashApply(root, stash.name, true) },
    { label: "Create Branch from Stash…", run: () => git.stashBranch(root, stash.name) },
    { separator: true },
    { label: "Drop…", danger: true, run: () => git.stashDrop(root, stash.name, stash.message) },
  ];
}

/** Menu konteks untuk entri reflog (atau commit mana pun yang hanya dikenal lewat hash). */
export function reflogMenu(root: string, oid: string): MenuItem[] {
  return [
    { label: "Create Branch Here…", run: () => git.createBranch(root, oid) },
    { label: "Checkout (detached)", run: () => git.checkout(root, { kind: "detached", name: oid }) },
    { separator: true },
    { label: "Reset Current Branch to Here (Mixed)…", run: () => git.reset(root, oid, "mixed") },
    { label: "Reset Current Branch to Here (Hard)…", danger: true, run: () => git.reset(root, oid, "hard") },
    { separator: true },
    { label: `Copy Hash (${shortOid(oid)})`, run: () => copyText(oid, "Hash") },
  ];
}

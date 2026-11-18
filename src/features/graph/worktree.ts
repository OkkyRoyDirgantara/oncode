// Command worktree dan submodule yang dipanggil dari panel Source Control.
import { confirm, promptInput, quickPick, toast, type PickItem } from "@/app/overlay";
import { baseName, pickFolder } from "@/app/store";
import { commands, errorCode, errorDetail, errorMessage } from "@/ipc";
import * as git from "@/features/scm/actions";
import { reloadRefs, setBusy, useScm } from "@/features/scm/store";
import { joinPath } from "./paths";

/** Nama folder bawaan untuk worktree baru: `<folder-repo>-<branch>` tanpa garis miring. */
export function defaultWorktreeName(root: string, branch: string): string {
  return `${baseName(root)}-${branch.replace(/[\\/]+/g, "-")}`;
}

/** Galat `git worktree remove` karena worktree masih berisi perubahan atau berkas untracked. */
export function isDirtyWorktreeError(error: unknown): boolean {
  if (errorCode(error) === "dirtyWorkTree") return true;
  return /modified or untracked|contains modified|untracked files|use --force/i.test(`${errorMessage(error)} ${errorDetail(error) ?? ""}`);
}

function validateFolderName(value: string): string | null {
  const name = value.trim();
  if (!name) return "Enter a folder name";
  if (/[\\/]/.test(name)) return "Enter a single folder name, not a path";
  if (name === "." || name === "..") return "This is not a valid folder name";
  return null;
}

export async function addWorktree(): Promise<void> {
  const root = git.requireRepo();
  if (!root) return;
  const refs = useScm.getState().repos[root]?.refs;
  type Choice = { create: true } | { create: false; branch: string };
  const items: PickItem<Choice>[] = [
    { label: "+ Create new branch…", value: { create: true } },
    // Sebuah branch hanya bisa di-checkout di satu worktree.
    ...(refs?.branches ?? []).filter((b) => !b.isHead && !b.worktree).map((b) => ({ label: b.name, description: b.subject, value: { create: false as const, branch: b.name } })),
  ];
  const choice = await quickPick({ placeholder: "Branch to check out in the new worktree", items });
  if (!choice) return;

  let branch: string;
  if (choice.create) {
    const name = await promptInput({
      title: "Add worktree",
      message: "The new branch starts at the current commit.",
      label: "New branch name",
      placeholder: "feature/my-change",
      validate: (v) => git.validateRefName(v) ?? (refs?.branches.some((b) => b.name === v.trim()) ? "A branch with this name already exists" : null),
    });
    if (!name) return;
    branch = name.trim();
  } else {
    branch = choice.branch;
  }

  const parent = await pickFolder("Choose the Parent Folder for the Worktree");
  if (!parent) return;
  const folder = await promptInput({
    title: "Add worktree",
    message: `The worktree is created as a new folder inside ${parent}.`,
    label: "Folder name",
    value: defaultWorktreeName(root, branch),
    validate: validateFolderName,
    confirmLabel: "Create Worktree",
  });
  if (!folder) return;
  const path = joinPath(parent, folder.trim());

  const created = await git.run(root, "Add worktree", () => commands.gitWorktreeAdd(root, path, choice.create ? branch : null, choice.create ? null : branch));
  if (!created) return;
  void reloadRefs(root);
  toast.success(`Worktree for ${branch} created at ${path}.`, [{ label: "Open in New Window", run: () => commands.windowOpen(path) }]);
}

export async function removeWorktree(root: string, path: string): Promise<void> {
  setBusy(root, { label: "Remove worktree", percent: null });
  try {
    await commands.gitWorktreeRemove(root, path, false);
  } catch (e) {
    setBusy(root, null);
    if (!isDirtyWorktreeError(e)) return git.handleGitError(root, e, "Remove worktree");
    const ok = await confirm({
      title: `Remove worktree ${baseName(path)} and its changes?`,
      message: `${path} contains modified or untracked files. Removing the worktree deletes the folder, and those uncommitted changes are lost permanently.\n\nCommits and the branch itself are kept.`,
      confirmLabel: "Remove and Discard Changes",
      danger: true,
    });
    if (!ok) return;
    if (!(await git.run(root, "Remove worktree", () => commands.gitWorktreeRemove(root, path, true)))) return;
  } finally {
    setBusy(root, null);
  }
  void reloadRefs(root);
}

/** `git submodule update --init` untuk path tertentu; daftar kosong berarti semua submodule. */
export async function updateSubmodules(root: string, paths: string[]): Promise<boolean> {
  const done = await git.runWithProgress(root, "Update submodules", (operationId, onProgress) => commands.gitSubmoduleUpdate(root, paths, true, operationId, onProgress));
  if (done) void reloadRefs(root);
  return done;
}

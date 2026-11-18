// Susunan interactive rebase: validasi (sama dengan `build_todo` di backend) dan pengurutan ulang.
import type { RebaseAction, RebaseTodoItem } from "@/ipc";

export const REBASE_ACTIONS: readonly RebaseAction[] = ["pick", "reword", "edit", "squash", "fixup", "drop"];

export const ACTION_HINT: Record<RebaseAction, string> = {
  pick: "Keep the commit as it is",
  reword: "Keep the changes, edit the message",
  edit: "Stop at this commit so you can amend it",
  squash: "Combine with the commit above, merging both messages",
  fixup: "Combine with the commit above, discarding this message",
  drop: "Remove the commit",
};

/**
 * Pesan galat bila susunan tidak sah, atau null bila boleh dijalankan.
 * Aturannya mengikuti backend: squash/fixup membutuhkan pick/reword/edit sebelumnya,
 * dan harus ada minimal satu commit yang tersisa.
 */
export function validatePlan(items: readonly RebaseTodoItem[]): string | null {
  if (items.length === 0) return "There are no commits to rebase.";
  let hasBase = false;
  for (const item of items) {
    if ((item.action === "squash" || item.action === "fixup") && !hasBase) {
      return `"${item.action}" on ${item.oid.slice(0, 8)} needs an earlier commit to combine with. Put a pick, reword or edit above it.`;
    }
    if (item.action === "pick" || item.action === "reword" || item.action === "edit") hasBase = true;
  }
  if (!hasBase) return "Every commit is dropped; nothing would be left to rebase.";
  return null;
}

/** Memindahkan item dari indeks `from` ke indeks `to`; indeks di luar batas tidak mengubah apa pun. */
export function moveItem<T>(items: readonly T[], from: number, to: number): T[] {
  const next = [...items];
  if (from === to || from < 0 || to < 0 || from >= next.length || to >= next.length) return next;
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

export function setAction(items: readonly RebaseTodoItem[], index: number, action: RebaseAction): RebaseTodoItem[] {
  return items.map((item, i) => (i === index ? { ...item, action } : item));
}

// Membuka tab diff, konflik, dan berkas milik sebuah repositori.
import { openFile } from "@/app/coreCommands";
import { baseName } from "@/app/store";
import { openTab, type OpenOptions } from "@/app/workbench";
import type { DiffSource } from "@/ipc";

export interface DiffParams {
  root: string;
  path: string;
  origPath: string | null;
  source: DiffSource;
}

/** Path absolut sebuah berkas repositori, dengan pemisah milik OS. */
export function repoFile(root: string, path: string): string {
  const sep = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  return root.replace(/[\\/]+$/, "") + sep + path.replace(/\//g, sep);
}

function sourceKey(source: DiffSource): string {
  switch (source.kind) {
    case "worktree":
    case "staged":
      return source.kind;
    case "commit":
      return `commit:${source.oid}:${source.parent ?? ""}`;
    case "range":
      return `range:${source.from}:${source.to}`;
    case "stash":
      return `stash:${source.name}`;
  }
}

export function sourceLabel(source: DiffSource): string {
  switch (source.kind) {
    case "worktree":
      return "Working Tree";
    case "staged":
      return "Staged";
    case "commit":
      return source.oid.slice(0, 8);
    case "range":
      return `${source.from.slice(0, 8)}..${source.to.slice(0, 8)}`;
    case "stash":
      return source.name;
  }
}

export function openDiff(params: DiffParams, options: OpenOptions = { preview: true }): void {
  openTab(
    {
      id: `diff:${params.root}:${sourceKey(params.source)}:${params.path}`,
      kind: "diff",
      title: `${baseName(params.path)} (${sourceLabel(params.source)})`,
      description: `${params.path} — ${sourceLabel(params.source)}`,
      params: { ...params },
    },
    options,
  );
}

export function openConflict(root: string, path: string): void {
  openTab({ id: `conflict:${root}:${path}`, kind: "conflict", title: `${baseName(path)} (Conflict)`, description: path, params: { root, path } });
}

export function openRepoFile(root: string, path: string, options: { preview?: boolean; line?: number } = {}): void {
  openFile(repoFile(root, path), options);
}

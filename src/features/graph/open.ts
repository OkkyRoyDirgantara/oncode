// Membuka tab milik fitur Graph.
import { baseName } from "@/app/store";
import { openTab } from "@/app/workbench";
import { requestReveal, type RevealTarget } from "./bus";

export function openGraph(root: string, reveal?: RevealTarget): void {
  openTab({ id: `graph:${root}`, kind: "graph", title: "Graph", description: `Commit graph — ${root}`, params: { root } });
  if (reveal) requestReveal(root, reveal);
}

export function openReflog(root: string): void {
  openTab({ id: `reflog:${root}`, kind: "reflog", title: "Reflog", description: `Reflog — ${root}`, params: { root } });
}

export function openFileHistory(root: string, path: string): void {
  openTab({ id: `fileHistory:${root}:${path}`, kind: "fileHistory", title: `History: ${baseName(path)}`, description: `File history — ${path}`, params: { root, path } });
}

export function openStash(root: string, name: string): void {
  openTab({ id: `stash:${root}:${name}`, kind: "stash", title: name, description: `Stash ${name}`, params: { root, name } });
}

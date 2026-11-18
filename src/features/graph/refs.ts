// Penyaringan dan pengelompokan ref untuk panel Referensi, serta ringkasan lencana ref pada baris commit.
import type { Branch, GraphRow, RefLabel, Refs, RemoteBranch, Stash, Tag } from "@/ipc";

export interface RemoteGroup {
  remote: string;
  branches: RemoteBranch[];
}

export interface RefGroups {
  branches: Branch[];
  remotes: RemoteGroup[];
  tags: Tag[];
  stashes: Stash[];
}

/** Nama branch remote tanpa awalan nama remote, mis. `origin/feature/x` → `feature/x`. */
export function remoteShortName(branch: RemoteBranch): string {
  return branch.name.startsWith(`${branch.remote}/`) ? branch.name.slice(branch.remote.length + 1) : branch.name;
}

/**
 * Ref yang cocok dengan `filter` (substring, tidak peka huruf besar), dikelompokkan untuk ditampilkan.
 * Branch remote dikelompokkan per remote mengikuti urutan `refs.remotes`; `<remote>/HEAD` dibuang.
 */
export function groupRefs(refs: Refs | null, stashes: readonly Stash[], filter: string): RefGroups {
  const needle = filter.trim().toLowerCase();
  const match = (...texts: string[]) => needle === "" || texts.some((t) => t.toLowerCase().includes(needle));

  const order = new Map<string, RemoteGroup>();
  for (const remote of refs?.remotes ?? []) order.set(remote.name, { remote: remote.name, branches: [] });
  for (const branch of refs?.remoteBranches ?? []) {
    if (remoteShortName(branch) === "HEAD" || !match(branch.name)) continue;
    let group = order.get(branch.remote);
    if (!group) {
      group = { remote: branch.remote, branches: [] };
      order.set(branch.remote, group);
    }
    group.branches.push(branch);
  }

  return {
    branches: (refs?.branches ?? []).filter((b) => match(b.name)),
    // Saat menyaring, remote tanpa branch yang cocok disembunyikan.
    remotes: [...order.values()].filter((g) => g.branches.length > 0 || needle === ""),
    tags: (refs?.tags ?? []).filter((t) => match(t.name)),
    stashes: stashes.filter((s) => match(s.name, s.message)),
  };
}

export function countRemoteBranches(groups: readonly RemoteGroup[]): number {
  return groups.reduce((n, g) => n + g.branches.length, 0);
}

/** Membagi lencana ref menjadi yang ditampilkan dan yang diringkas sebagai "+N". */
export function summarizeLabels(labels: readonly RefLabel[], max = 3): { visible: RefLabel[]; hidden: RefLabel[] } {
  if (labels.length <= max) return { visible: [...labels], hidden: [] };
  return { visible: labels.slice(0, max), hidden: labels.slice(max) };
}

/**
 * Apakah `oid` leluhur dari `headOid`, menurut baris yang sudah dimuat.
 * Urutan topologis menjamin semua commit di antara HEAD dan leluhurnya muncul lebih dulu,
 * sehingga jawabannya pasti selama daftar tidak disaring. Mengembalikan null bila tidak bisa
 * ditentukan (HEAD tidak muncul sebelum commit itu, atau commit tidak ada di daftar).
 */
export function isAncestorOfHead(rows: readonly GraphRow[], headOid: string | null, oid: string): boolean | null {
  if (!headOid) return null;
  if (oid === headOid) return true;
  const parents = new Map<string, string[]>();
  let seenHead = false;
  let seenTarget = false;
  for (const row of rows) {
    parents.set(row.commit.oid, row.commit.parents);
    if (row.commit.oid === headOid) seenHead = true;
    if (row.commit.oid === oid) {
      seenTarget = true;
      break;
    }
  }
  if (!seenTarget || !seenHead) return null;
  const queue = [headOid];
  const visited = new Set(queue);
  while (queue.length > 0) {
    const current = queue.pop()!;
    for (const parent of parents.get(current) ?? []) {
      if (parent === oid) return true;
      if (!visited.has(parent)) {
        visited.add(parent);
        queue.push(parent);
      }
    }
  }
  return false;
}

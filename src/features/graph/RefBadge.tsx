// Lencana ref pada baris commit: HEAD, branch, remote, tag, dan stash dengan gaya berbeda.
import type { RefLabel } from "@/ipc";
import { summarizeLabels } from "./refs";

const KIND_TITLE: Record<RefLabel["kind"], string> = { head: "Current commit (HEAD)", branch: "Local branch", remote: "Remote branch", tag: "Tag", stash: "Stash" };

export function RefBadge({ label }: { label: RefLabel }) {
  return (
    <span className="gr-badge" data-kind={label.kind} title={`${KIND_TITLE[label.kind]}: ${label.name}`}>
      {label.name}
    </span>
  );
}

/** Lencana untuk satu baris; daftar panjang diringkas menjadi "+N". */
export function RefBadges({ labels, max = 3 }: { labels: readonly RefLabel[]; max?: number }) {
  if (labels.length === 0) return null;
  const { visible, hidden } = summarizeLabels(labels, max);
  return (
    <span className="gr-badges">
      {visible.map((l) => (
        <RefBadge key={`${l.kind}:${l.name}`} label={l} />
      ))}
      {hidden.length > 0 && (
        <span className="gr-badge" data-kind="more" title={hidden.map((l) => l.name).join("\n")}>
          +{hidden.length}
        </span>
      )}
    </span>
  );
}

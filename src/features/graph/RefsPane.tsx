// Panel Referensi: branch lokal, remote, tag, dan stash, dengan penyaring dan menu konteks.
import { useMemo, useState } from "react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { Archive, ArrowDown, ArrowUp, Check, ChevronDown, ChevronRight, Cloud, FolderGit2, GitBranch, Tag as TagIcon } from "lucide-react";
import { executeCommand } from "@/app/commands";
import { showMenu, type MenuItem } from "@/app/overlay";
import * as git from "@/features/scm/actions";
import { useRepo } from "@/features/scm/store";
import type { RevealTarget } from "./bus";
import { relativeDate } from "./format";
import { branchMenu, remoteBranchMenu, stashMenu, tagMenu } from "./menus";
import { countRemoteBranches, groupRefs, remoteShortName } from "./refs";

function Group({ title, count, depth = 0, defaultOpen = true, forceOpen, children }: { title: string; count: number; depth?: number; defaultOpen?: boolean; forceOpen: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  // Saat menyaring, semua kelompok dibuka agar hasilnya terlihat.
  const shown = open || forceOpen;
  return (
    <div role="group" aria-label={title}>
      <div
        className="oc-section-header gr-refs-header"
        style={{ paddingLeft: 4 + depth * 12 }}
        role="button"
        tabIndex={0}
        aria-expanded={shown}
        onClick={() => setOpen(!shown)}
        onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && setOpen(!shown)}
      >
        {shown ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span className="oc-ellipsis">{title}</span>
        <span className="oc-count">{count}</span>
      </div>
      {shown && children}
    </div>
  );
}

interface RowProps {
  depth: number;
  icon: ReactNode;
  label: string;
  title: string;
  current?: boolean;
  trailing?: ReactNode;
  onClick: () => void;
  onDoubleClick?: () => void;
  menu: () => MenuItem[];
}

function Row({ depth, icon, label, title, current, trailing, onClick, onDoubleClick, menu }: RowProps) {
  const onContextMenu = (e: ReactMouseEvent) => {
    e.preventDefault();
    showMenu(e, menu());
  };
  return (
    <div
      className="oc-list-row gr-ref"
      style={{ paddingLeft: 10 + depth * 12 }}
      data-current={current || undefined}
      title={title}
      role="button"
      tabIndex={0}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
      onKeyDown={(e) => e.key === "Enter" && onClick()}
    >
      <span className="gr-ref-icon">{icon}</span>
      <span className="oc-ellipsis gr-ref-name">{label}</span>
      {trailing && <span className="gr-ref-trailing">{trailing}</span>}
    </div>
  );
}

export interface RefsPaneProps {
  root: string;
  /** Pengguna memilih sebuah ref: tampilkan commit-nya di graph. */
  onReveal: (target: RevealTarget) => void;
}

export function RefsPane({ root, onReveal }: RefsPaneProps) {
  const repo = useRepo(root);
  const [filter, setFilter] = useState("");
  const refs = repo?.refs ?? null;
  const stashes = repo?.stashes;
  const groups = useMemo(() => groupRefs(refs, stashes ?? [], filter), [refs, stashes, filter]);
  const filtering = filter.trim() !== "";
  const remoteCount = countRemoteBranches(groups.remotes);
  const empty = groups.branches.length + remoteCount + groups.tags.length + groups.stashes.length === 0;

  return (
    <div className="gr-refs" role="region" aria-label="References">
      <div className="gr-refs-filter">
        <input className="oc-input" type="search" placeholder="Filter branches and tags" aria-label="Filter references" value={filter} spellCheck={false} onChange={(e) => setFilter(e.target.value)} />
      </div>
      <div className="gr-refs-body">
        {!refs && <div className="gr-hint">Loading references…</div>}
        {refs && empty && <div className="gr-hint">{filtering ? "No references match the filter." : "This repository has no branches yet."}</div>}

        {groups.branches.length > 0 && (
          <Group title="Local" count={groups.branches.length} forceOpen={filtering}>
            {groups.branches.map((b) => (
              <Row
                key={b.name}
                depth={1}
                icon={b.isHead ? <Check size={13} /> : <GitBranch size={13} />}
                label={b.name}
                current={b.isHead}
                title={[
                  b.isHead ? `${b.name} (current branch)` : b.name,
                  b.upstream ? (b.upstreamGone ? `Upstream ${b.upstream} no longer exists` : `Tracks ${b.upstream}`) : "No upstream",
                  b.worktree && !b.isHead ? `Checked out in worktree ${b.worktree}` : "",
                  b.subject,
                ]
                  .filter(Boolean)
                  .join("\n")}
                trailing={
                  <>
                    {b.worktree && !b.isHead && <FolderGit2 size={12} aria-label="Checked out in another worktree" />}
                    {b.upstreamGone && <span className="gr-ref-gone">gone</span>}
                    {b.ahead > 0 && (
                      <span className="gr-ref-count" aria-label={`${b.ahead} ahead`}>
                        {b.ahead}
                        <ArrowUp size={11} />
                      </span>
                    )}
                    {b.behind > 0 && (
                      <span className="gr-ref-count" aria-label={`${b.behind} behind`}>
                        {b.behind}
                        <ArrowDown size={11} />
                      </span>
                    )}
                  </>
                }
                onClick={() => onReveal({ oid: b.oid, label: b.name, hunt: true })}
                onDoubleClick={() => !b.isHead && void git.checkout(root, { kind: "branch", name: b.name })}
                menu={() => branchMenu(root, b)}
              />
            ))}
          </Group>
        )}

        {groups.remotes.length > 0 && (
          <Group title="Remotes" count={remoteCount} forceOpen={filtering}>
            {groups.remotes.map((g) => (
              <Group key={g.remote} title={g.remote} count={g.branches.length} depth={1} forceOpen={filtering}>
                {g.branches.map((b) => (
                  <Row
                    key={b.name}
                    depth={2}
                    icon={<Cloud size={13} />}
                    label={remoteShortName(b)}
                    title={b.name}
                    onClick={() => onReveal({ oid: b.oid, label: b.name, hunt: true })}
                    onDoubleClick={() => void git.checkout(root, { kind: "remote", name: b.name })}
                    menu={() => remoteBranchMenu(root, b)}
                  />
                ))}
              </Group>
            ))}
          </Group>
        )}

        {groups.tags.length > 0 && (
          <Group title="Tags" count={groups.tags.length} defaultOpen={false} forceOpen={filtering}>
            {groups.tags.map((t) => (
              <Row
                key={t.name}
                depth={1}
                icon={<TagIcon size={13} />}
                label={t.name}
                title={t.annotated && t.message ? `${t.name}\n${t.message}` : t.name}
                onClick={() => onReveal({ oid: t.oid, label: t.name, hunt: true })}
                menu={() => tagMenu(root, t)}
              />
            ))}
          </Group>
        )}

        {groups.stashes.length > 0 && (
          <Group title="Stashes" count={groups.stashes.length} defaultOpen={false} forceOpen={filtering}>
            {groups.stashes.map((s) => (
              <Row
                key={s.oid}
                depth={1}
                icon={<Archive size={13} />}
                label={s.message || s.name}
                title={`${s.name}: ${s.message}\nDouble-click to show its changes`}
                trailing={<span className="gr-ref-time">{relativeDate(s.time)}</span>}
                // Hanya stash terbaru yang muncul di graph, jadi stash cukup dipilih tanpa dicari.
                onClick={() => onReveal({ oid: s.oid, label: s.name, hunt: false })}
                onDoubleClick={() => void executeCommand("git.showStash", root, s.name)}
                menu={() => stashMenu(root, s)}
              />
            ))}
          </Group>
        )}
      </div>
    </div>
  );
}

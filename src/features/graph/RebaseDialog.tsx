// Dialog interactive rebase: mengatur urutan dan aksi tiap commit sebelum rebase dimulai.
import { useEffect, useState } from "react";
import { ArrowDown, ArrowUp, GripVertical, TriangleAlert } from "lucide-react";
import { openDialog } from "@/app/overlay";
import { commands, errorMessage, type RebaseAction, type RebasePlan, type RebaseTodoItem } from "@/ipc";
import * as git from "@/features/scm/actions";
import { plural, shortOid } from "./format";
import { ACTION_HINT, moveItem, REBASE_ACTIONS, setAction, validatePlan } from "./rebasePlan";
import "./graph.css";

interface Props {
  root: string;
  /** Commit sebelum commit pertama yang disusun ulang; null = dari commit pertama repositori. */
  base: string | null;
  close: (items: RebaseTodoItem[] | null) => void;
}

export function RebaseDialog({ root, base, close }: Props) {
  const [plan, setPlan] = useState<RebasePlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [items, setItems] = useState<RebaseTodoItem[]>([]);
  const [dragging, setDragging] = useState<number | null>(null);
  const [dropAt, setDropAt] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    commands.gitRebasePlan(root, base).then(
      (loaded) => {
        if (cancelled) return;
        setPlan(loaded);
        setItems(loaded.items);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [root, base]);

  const problem = plan ? validatePlan(items) : null;
  const title = plan
    ? `Interactive rebase: ${plural(plan.items.length, "commit")} ${base ? `on top of ${base.length >= 40 ? shortOid(base) : base}` : "from the first commit"}`
    : "Interactive rebase";

  const move = (from: number, to: number) => setItems((current) => moveItem(current, from, to));
  const endDrag = () => {
    setDragging(null);
    setDropAt(null);
  };

  return (
    <>
      <div className="oc-dialog-header">{title}</div>
      <div className="oc-dialog-body gr-rebase">
        {error && (
          <div className="oc-error-text oc-selectable" role="alert">
            Could not prepare the rebase: {error}
          </div>
        )}
        {!plan && !error && <div className="oc-muted">Loading commits…</div>}
        {plan && plan.items.length === 0 && <div className="oc-muted">There are no commits to rebase here. Merge commits cannot be rearranged.</div>}
        {plan && plan.items.length > 0 && (
          <>
            {plan.published > 0 && (
              <div className="gr-rebase-warning" role="status">
                <TriangleAlert size={15} />
                <span>
                  {plan.published} of these commits {plan.published === 1 ? "is" : "are"} already on a remote; you will need to force push afterwards.
                </span>
              </div>
            )}
            <div className="gr-rebase-list" role="list" aria-label="Rebase plan">
              {items.map((item, index) => (
                <div
                  key={item.oid}
                  className="gr-rebase-row"
                  role="listitem"
                  data-action={item.action}
                  data-dragging={dragging === index || undefined}
                  data-drop={dropAt === index && dragging !== null && dragging !== index ? (dragging < index ? "after" : "before") : undefined}
                  onDragOver={(e) => {
                    if (dragging === null) return;
                    e.preventDefault();
                    if (dropAt !== index) setDropAt(index);
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    if (dragging !== null) move(dragging, index);
                    endDrag();
                  }}
                >
                  <span
                    className="gr-rebase-handle"
                    draggable
                    title="Drag to reorder"
                    aria-hidden="true"
                    onDragStart={(e) => {
                      setDragging(index);
                      // jsdom dan sebagian webview tidak selalu menyediakan dataTransfer.
                      e.dataTransfer?.setData("text/plain", item.oid);
                      if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                    }}
                    onDragEnd={endDrag}
                  >
                    <GripVertical size={14} />
                  </span>
                  <button className="oc-iconbtn" title="Move up" aria-label={`Move ${shortOid(item.oid)} up`} disabled={index === 0} onClick={() => move(index, index - 1)}>
                    <ArrowUp size={13} />
                  </button>
                  <button className="oc-iconbtn" title="Move down" aria-label={`Move ${shortOid(item.oid)} down`} disabled={index === items.length - 1} onClick={() => move(index, index + 1)}>
                    <ArrowDown size={13} />
                  </button>
                  <select
                    className="oc-select gr-rebase-action"
                    aria-label={`Action for ${shortOid(item.oid)}`}
                    title={ACTION_HINT[item.action]}
                    value={item.action}
                    onChange={(e) => setItems((current) => setAction(current, index, e.target.value as RebaseAction))}
                  >
                    {REBASE_ACTIONS.map((action) => (
                      <option key={action} value={action}>
                        {action}
                      </option>
                    ))}
                  </select>
                  <span className="oc-mono gr-rebase-hash">{shortOid(item.oid)}</span>
                  <span className="oc-ellipsis gr-rebase-subject" title={item.subject}>
                    {item.subject}
                  </span>
                </div>
              ))}
            </div>
            <div className="oc-faint gr-rebase-help">Commits are applied top to bottom (oldest first). Drag the handle or use the arrows to reorder.</div>
            {problem && (
              <div className="oc-error-text" role="alert">
                {problem}
              </div>
            )}
          </>
        )}
      </div>
      <div className="oc-dialog-footer">
        <button className="oc-btn" onClick={() => close(null)}>
          Cancel
        </button>
        <button className="oc-btn oc-btn-primary" disabled={!plan || problem !== null} onClick={() => close(items)}>
          Start Rebase
        </button>
      </div>
    </>
  );
}

/** Membuka dialog; setelah pengguna menekan Start, dialog ditutup lalu rebase dijalankan. */
export async function interactiveRebase(root: string, base: string | null): Promise<boolean> {
  const items = await openDialog<RebaseTodoItem[]>((close) => <RebaseDialog root={root} base={base} close={close} />, { wide: true });
  if (!items) return false;
  return git.run(root, "Interactive rebase", () => commands.gitRebaseInteractive(root, base, items));
}

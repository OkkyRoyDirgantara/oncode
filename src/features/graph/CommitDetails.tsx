// Panel detail: satu commit (metadata, pesan, berkas) atau perbandingan dua commit.
import { Copy, RefreshCw } from "lucide-react";
import { commands, type RefLabel, type Signature } from "@/ipc";
import { FileList, totals } from "./FileList";
import { fullDate, plural, relativeDate, shortOid } from "./format";
import { copyText } from "./menus";
import { RefBadge } from "./RefBadge";
import { useAsync } from "./useAsync";

function Person({ label, who }: { label: string; who: Signature }) {
  return (
    <div className="gr-meta-row">
      <span className="gr-meta-label">{label}</span>
      <span className="gr-meta-value oc-selectable">
        {who.name} <span className="oc-muted">&lt;{who.email}&gt;</span>
        <br />
        <span className="oc-muted" title={relativeDate(who.time)}>
          {fullDate(who.time)}
        </span>
      </span>
    </div>
  );
}

function ErrorBox({ message, retry }: { message: string; retry: () => void }) {
  return (
    <div className="gr-error" role="alert">
      <div className="oc-selectable">{message}</div>
      <button className="oc-btn oc-btn-sm" onClick={retry}>
        <RefreshCw size={13} /> Retry
      </button>
    </div>
  );
}

function FilesHeader({ count, additions, deletions }: { count: number; additions: number; deletions: number }) {
  return (
    <div className="gr-files-header">
      <span>{plural(count, "changed file")}</span>
      {additions > 0 && <span className="oc-git-added">+{additions}</span>}
      {deletions > 0 && <span className="oc-git-deleted">−{deletions}</span>}
    </div>
  );
}

function sameSignature(a: Signature, b: Signature): boolean {
  return a.name === b.name && a.email === b.email && a.time === b.time;
}

export function CommitDetailView({ root, oid, onSelectCommit }: { root: string; oid: string; onSelectCommit: (oid: string) => void }) {
  const detail = useAsync(() => commands.gitCommitDetail(root, oid), [root, oid]);
  if (detail.status === "error") return <ErrorBox message={`Could not load commit ${shortOid(oid)}: ${detail.error}`} retry={detail.reload} />;
  if (!detail.data) return <div className="gr-hint">Loading commit {shortOid(oid)}…</div>;
  const { commit, body, files } = detail.data;
  const sum = totals(files);
  const message = body.trim() ? body.replace(/\s+$/, "") : commit.subject;
  return (
    <div className="gr-details" aria-label={`Commit ${shortOid(commit.oid)}`}>
      <div className="gr-details-hash">
        <span className="oc-mono oc-selectable oc-ellipsis" title={commit.oid}>
          {commit.oid}
        </span>
        <button className="oc-iconbtn" title="Copy hash" aria-label="Copy hash" onClick={() => void copyText(commit.oid, "Hash")}>
          <Copy size={13} />
        </button>
      </div>
      {commit.refs.length > 0 && (
        <div className="gr-details-refs">
          {commit.refs.map((r: RefLabel) => (
            <RefBadge key={`${r.kind}:${r.name}`} label={r} />
          ))}
        </div>
      )}
      <pre className="gr-message oc-selectable">{message}</pre>
      <div className="gr-meta">
        <Person label="Author" who={commit.author} />
        {!sameSignature(commit.author, commit.committer) && <Person label="Committer" who={commit.committer} />}
        <div className="gr-meta-row">
          <span className="gr-meta-label">{commit.parents.length === 1 ? "Parent" : "Parents"}</span>
          <span className="gr-meta-value">
            {commit.parents.length === 0 && <span className="oc-muted">none (root commit)</span>}
            {commit.parents.map((p) => (
              <button key={p} className="gr-link oc-mono" title={`Go to parent ${p}`} onClick={() => onSelectCommit(p)}>
                {shortOid(p)}
              </button>
            ))}
          </span>
        </div>
      </div>
      <FilesHeader count={files.length} additions={sum.additions} deletions={sum.deletions} />
      {commit.parents.length > 1 && <div className="gr-hint gr-hint-tight">Merge commit: changes are shown against the first parent.</div>}
      <FileList root={root} files={files} source={{ kind: "commit", oid: commit.oid, parent: null }} />
    </div>
  );
}

export function CompareView({ root, from, to }: { root: string; from: string; to: string }) {
  const files = useAsync(() => commands.gitDiffFiles(root, from, to), [root, from, to]);
  const sum = totals(files.data ?? []);
  return (
    <div className="gr-details" aria-label="Comparison">
      <div className="gr-details-hash">
        <span className="oc-ellipsis">
          Comparing{" "}
          <span className="oc-mono oc-selectable">
            {shortOid(from)}..{shortOid(to)}
          </span>
        </span>
      </div>
      <div className="gr-hint gr-hint-tight">Changes needed to get from the older commit to the newer one.</div>
      {files.status === "error" && <ErrorBox message={`Could not compare the commits: ${files.error}`} retry={files.reload} />}
      {files.status === "loading" && !files.data && <div className="gr-hint">Loading changes…</div>}
      {files.data && (
        <>
          <FilesHeader count={files.data.length} additions={sum.additions} deletions={sum.deletions} />
          <FileList root={root} files={files.data} source={{ kind: "range", from, to }} />
        </>
      )}
    </div>
  );
}

export { ErrorBox };

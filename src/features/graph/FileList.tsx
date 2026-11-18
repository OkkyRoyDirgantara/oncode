// Daftar berkas yang berubah pada sebuah commit, rentang, atau stash.
import type { MouseEvent as ReactMouseEvent } from "react";
import { executeCommand } from "@/app/commands";
import { showMenu } from "@/app/overlay";
import type { ChangeKind, CommitFile, DiffSource } from "@/ipc";
import { openDiff, openRepoFile } from "@/features/scm/open";
import { copyText } from "./menus";

const KIND_LETTER: Record<ChangeKind, string> = { added: "A", modified: "M", deleted: "D", renamed: "R", copied: "C", typeChanged: "T" };
const KIND_CLASS: Record<ChangeKind, string> = {
  added: "oc-git-added",
  modified: "oc-git-modified",
  deleted: "oc-git-deleted",
  renamed: "oc-git-renamed",
  copied: "oc-git-renamed",
  typeChanged: "oc-git-modified",
};
const KIND_TITLE: Record<ChangeKind, string> = { added: "Added", modified: "Modified", deleted: "Deleted", renamed: "Renamed", copied: "Copied", typeChanged: "Type changed" };

function splitPath(path: string): { name: string; dir: string } {
  const i = path.lastIndexOf("/");
  return i < 0 ? { name: path, dir: "" } : { name: path.slice(i + 1), dir: path.slice(0, i) };
}

/** Jumlah baris bertambah dan berkurang dari daftar berkas (berkas biner tidak dihitung). */
export function totals(files: readonly CommitFile[]): { additions: number; deletions: number } {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    additions += f.additions ?? 0;
    deletions += f.deletions ?? 0;
  }
  return { additions, deletions };
}

/** `stats: false` untuk sumber yang tidak menyediakan jumlah baris (mis. stash). */
export function FileList({ root, files, source, stats = true }: { root: string; files: readonly CommitFile[]; source: DiffSource; stats?: boolean }) {
  if (files.length === 0) return <div className="gr-hint">No files changed.</div>;
  return (
    <div className="gr-files" role="list">
      {files.map((file) => {
        const { name, dir } = splitPath(file.path);
        const open = (preview: boolean) => openDiff({ root, path: file.path, origPath: file.origPath, source }, { preview });
        const renamed = file.origPath && file.origPath !== file.path;
        const onContextMenu = (e: ReactMouseEvent) => {
          e.preventDefault();
          showMenu(e, [
            { label: "Open Changes", run: () => open(false) },
            { label: "Open Current File", run: () => openRepoFile(root, file.path, { preview: false }) },
            { label: "File History", run: () => executeCommand("git.fileHistory", root, file.path) },
            { separator: true },
            { label: "Copy Path", run: () => copyText(file.path, "Path") },
          ]);
        };
        return (
          <div
            key={file.path}
            className="oc-list-row gr-file"
            role="listitem"
            tabIndex={0}
            title={`${renamed ? `${file.origPath} → ` : ""}${file.path} • ${KIND_TITLE[file.kind]}`}
            onClick={() => open(true)}
            onDoubleClick={() => open(false)}
            onKeyDown={(e) => e.key === "Enter" && open(false)}
            onContextMenu={onContextMenu}
          >
            <span className={`oc-status-letter ${KIND_CLASS[file.kind]}`}>{KIND_LETTER[file.kind]}</span>
            <span className="oc-ellipsis gr-file-name">
              {renamed ? (
                <>
                  <span className="gr-file-old">{file.origPath}</span>
                  {" → "}
                  {file.path}
                </>
              ) : (
                <>
                  {name}
                  {dir && <span className="gr-file-dir">{dir}</span>}
                </>
              )}
            </span>
            {stats && (
            <span className="gr-file-stat">
              {file.additions === null && file.deletions === null ? (
                <span className="oc-faint">binary</span>
              ) : (
                <>
                  {(file.additions ?? 0) > 0 && <span className="oc-git-added">+{file.additions}</span>}
                  {(file.deletions ?? 0) > 0 && <span className="oc-git-deleted">−{file.deletions}</span>}
                </>
              )}
            </span>
            )}
          </div>
        );
      })}
    </div>
  );
}

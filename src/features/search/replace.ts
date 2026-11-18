// "Replace All": berkas yang terbuka diubah lewat editornya (agar undo berfungsi), sisanya ditulis backend.
import { confirm, showError, toast } from "@/app/overlay";
import { absolutePath } from "@/app/store";
import { commands, errorMessage, type FileMatches, type SearchQuery } from "@/ipc";
import { getBuffer, isDirty, saveBuffer, type Buffer } from "../editor/buffers";
import { applyEdits, compileMatcher, computeEdits, type Matcher } from "./matcher";
import { resultCounts, searchNow, useSearch, type MatchKey } from "./store";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

type Skip = ((line: number, column: number, length: number) => boolean) | undefined;

function skipper(dismissed: readonly MatchKey[] | undefined): Skip {
  if (!dismissed || dismissed.length === 0) return undefined;
  return (line, column, length) => dismissed.some((d) => d.line === line && d.column === column && d.length === length);
}

/**
 * Mengganti semua kecocokan di buffer lewat SATU transaksi pada salah satu editornya (buffer
 * menyebarkannya ke editor lain), sehingga satu undo membatalkan semuanya. Mengembalikan jumlah penggantian.
 */
export function replaceInBuffer(buffer: Buffer, matcher: Matcher, replacement: string, interpolate: boolean, skip?: Skip): number {
  const view = buffer.views.values().next().value;
  if (!view) return 0;
  const doc = view.state.doc;
  const edits = computeEdits(matcher, doc.iterLines(), replacement, interpolate, skip);
  if (edits.length === 0) return 0;
  const changes = edits.map((edit) => {
    const from = doc.line(edit.line).from;
    return { from: from + edit.from, to: from + edit.to, insert: edit.insert };
  });
  view.dispatch({ changes, userEvent: "input.replace.all" });
  return edits.length;
}

/** Untuk berkas tertutup yang sebagian kecocokannya disingkirkan: backend hanya bisa mengganti seluruh isi berkas. */
async function replaceOnDisk(path: string, matcher: Matcher, replacement: string, interpolate: boolean, skip: Skip): Promise<number> {
  const opened = await commands.fsOpen(path);
  if (opened.kind !== "text") return 0;
  const lines = opened.text.split("\n");
  const edits = computeEdits(matcher, lines, replacement, interpolate, skip);
  if (edits.length === 0) return 0;
  await commands.fsSave(path, applyEdits(lines, edits).join("\n"), opened.encoding, opened.bom, opened.eol);
  return edits.length;
}

export interface ReplaceOutcome {
  replacements: number;
  files: number;
  /** Berkas yang diubah di editor tetapi tidak disimpan (sudah berisi perubahan lain sebelumnya, atau gagal disimpan). */
  unsaved: number;
  /** Terbuka di editor, tetapi polanya tidak bisa dijalankan di sisi editor. */
  skippedOpen: string[];
  /** Punya kecocokan yang disingkirkan, tetapi polanya tidak bisa dijalankan di sisi editor. */
  skippedPartial: string[];
  skippedReadOnly: string[];
  /** Alasan pola tidak bisa dijalankan di sisi editor, bila ada berkas yang dilewati karenanya. */
  reason: string | null;
  errors: string[];
}

/** Menjalankan penggantian untuk `files` (hasil pencarian yang masih tampil) tanpa konfirmasi maupun toast. */
export async function performReplace(query: SearchQuery, replacement: string, files: readonly FileMatches[], dismissed: Record<string, MatchKey[]>): Promise<ReplaceOutcome> {
  const outcome: ReplaceOutcome = { replacements: 0, files: 0, unsaved: 0, skippedOpen: [], skippedPartial: [], skippedReadOnly: [], reason: null, errors: [] };
  const compiled = compileMatcher(query);
  const interpolate = query.isRegex;
  const backend: string[] = [];
  const count = (n: number) => {
    if (n === 0) return false;
    outcome.replacements += n;
    outcome.files += 1;
    return true;
  };

  for (const file of files) {
    const path = absolutePath(file.path);
    const skip = skipper(dismissed[file.path]);
    const buffer = getBuffer(path);
    try {
      if (buffer && buffer.status === "ready" && buffer.views.size > 0) {
        if (buffer.readOnly) {
          outcome.skippedReadOnly.push(file.path);
        } else if (!compiled.ok) {
          outcome.skippedOpen.push(file.path);
          outcome.reason = compiled.reason;
        } else {
          const wasDirty = isDirty(path);
          if (!count(replaceInBuffer(buffer, compiled.matcher, replacement, interpolate, skip))) continue;
          // Buffer yang tadinya bersih langsung disimpan (setara dengan berkas yang ditulis backend).
          // Buffer yang sudah berisi perubahan lain tidak disimpan tanpa izin pengguna.
          if (wasDirty) outcome.unsaved += 1;
          else if (!(await saveBuffer(path).catch(() => false))) outcome.unsaved += 1;
        }
      } else if (skip) {
        if (!compiled.ok) {
          outcome.skippedPartial.push(file.path);
          outcome.reason = compiled.reason;
        } else {
          count(await replaceOnDisk(path, compiled.matcher, replacement, interpolate, skip));
        }
      } else {
        backend.push(file.path);
      }
    } catch (e) {
      outcome.errors.push(`${file.path}: ${errorMessage(e)}`);
    }
  }

  if (backend.length > 0) {
    try {
      const summary = await commands.searchReplace(query, replacement, backend);
      outcome.replacements += summary.replacements;
      outcome.files += summary.files;
    } catch (e) {
      outcome.errors.push(errorMessage(e));
    }
  }
  return outcome;
}

function report(outcome: ReplaceOutcome): void {
  if (outcome.replacements > 0) {
    const unsaved = outcome.unsaved > 0 ? ` ${plural(outcome.unsaved, "open file")} ${outcome.unsaved === 1 ? "was" : "were"} changed in the editor and left unsaved.` : "";
    toast.success(`Replaced ${plural(outcome.replacements, "occurrence")} across ${plural(outcome.files, "file")}.${unsaved}`);
  } else if (outcome.errors.length === 0) {
    toast.info("Nothing was replaced.");
  }

  const skipped: string[] = [];
  if (outcome.skippedOpen.length > 0) skipped.push(`Skipped ${plural(outcome.skippedOpen.length, "open file")}: save and close ${outcome.skippedOpen.length === 1 ? "it" : "them"}, then replace again.`);
  if (outcome.skippedPartial.length > 0) skipped.push(`Skipped ${plural(outcome.skippedPartial.length, "file")} with dismissed matches: open ${outcome.skippedPartial.length === 1 ? "it" : "them"} in the editor to replace selectively.`);
  if (outcome.reason && skipped.length > 0) skipped.push(`${outcome.reason}.`);
  if (outcome.skippedReadOnly.length > 0) skipped.push(`Skipped ${plural(outcome.skippedReadOnly.length, "read-only file")}.`);
  if (skipped.length > 0) toast.warning(skipped.join(" "));

  if (outcome.errors.length > 0) showError({ code: "replace", message: `${plural(outcome.errors.length, "replacement")} failed`, detail: outcome.errors.join("\n") }, "Replace");
}

/** Pertanyaan konfirmasi untuk hasil yang sedang tampil. */
export function confirmationText(files: readonly FileMatches[], replacement: string, truncated: boolean): { title: string; message: string } {
  const counts = resultCounts(files);
  const what = replacement === "" ? "Each occurrence will be deleted (the replacement is empty)." : `Each occurrence will be replaced with "${replacement}".`;
  const scope = truncated ? " The results were truncated: every occurrence in the listed files is replaced, including ones not shown." : "";
  return {
    title: `Replace ${plural(counts.matches, "occurrence")} across ${plural(counts.files, "file")}?`,
    message: `${what}${scope} Files that are not open in the editor are changed on disk immediately.`,
  };
}

/** Mengganti semua kecocokan yang masih tampil, setelah konfirmasi; lalu mencari ulang dan melaporkan jumlahnya. */
export async function replaceAll(): Promise<void> {
  const state = useSearch.getState();
  const query = state.resultQuery;
  if (!query || state.status !== "done" || state.pending || state.replacing || state.files.length === 0) return;
  const text = confirmationText(state.files, state.replacement, state.summary?.truncated ?? false);
  if (!(await confirm({ ...text, confirmLabel: "Replace" }))) return;

  useSearch.setState({ replacing: true });
  try {
    report(await performReplace(query, state.replacement, state.files, state.dismissed));
  } finally {
    useSearch.setState({ replacing: false });
    searchNow();
  }
}

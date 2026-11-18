// Menghubungkan sebuah EditorView ke buffer bersama: perubahan diteruskan ke buffer
// dan ke semua editor lain yang menampilkan berkas yang sama (tab lain, split, sisi kanan diff).
import { Annotation, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";
import { setBufferDoc, type Buffer } from "./buffers";

/** Menandai transaksi hasil sinkronisasi agar tidak diteruskan lagi. */
const syncAnnotation = Annotation.define<boolean>();

export function bufferSync(buffer: Buffer): Extension {
  return [
    ViewPlugin.define((view) => {
      buffer.views.add(view);
      return {
        destroy() {
          buffer.views.delete(view);
        },
      };
    }),
    EditorView.updateListener.of((update) => {
      if (!update.docChanged) return;
      setBufferDoc(buffer.path, update.state.doc);
      for (const tr of update.transactions) {
        if (!tr.docChanged || tr.annotation(syncAnnotation)) continue;
        for (const other of buffer.views) {
          if (other !== update.view) other.dispatch({ changes: tr.changes, annotations: syncAnnotation.of(true) });
        }
      }
    }),
  ];
}

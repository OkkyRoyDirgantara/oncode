// Ekstensi editor dari fitur lain (penanda Git, blame, LSP) didaftarkan di sini
// agar editor tidak perlu mengenal fitur-fitur itu.
import type { Extension } from "@codemirror/state";

export interface EditorContext {
  /** Path absolut berkas. */
  path: string;
  /** Id bahasa LSP, bila dikenali. */
  languageId: string | null;
  /** Berkas besar: fitur berat sebaiknya tidak aktif. */
  large: boolean;
}

/** Mengembalikan ekstensi untuk sebuah editor berkas; boleh asinkron atau null bila tidak berlaku. */
export type EditorExtensionProvider = (context: EditorContext) => Extension | null | Promise<Extension | null>;

const providers: EditorExtensionProvider[] = [];

export function registerEditorExtension(provider: EditorExtensionProvider): void {
  providers.push(provider);
}

export function editorExtensionProviders(): readonly EditorExtensionProvider[] {
  return providers;
}

// Data contoh untuk pengujian halaman pengaturan. Tidak dipakai oleh kode aplikasi.
import type { Settings } from "@/ipc";

/** Sama dengan nilai bawaan di `src-tauri/src/settings.rs` (font diperpendek). */
export const defaults: Settings = {
  editor: { fontFamily: '"JetBrains Mono", monospace', fontSize: 14, tabSize: 4, insertSpaces: true, wordWrap: false, lineNumbers: true, inlineBlame: false },
  files: { autoSave: "off" },
  workbench: { theme: "system", startupView: "scm" },
  git: { path: null, autoFetchMinutes: 5, pullStrategy: "ffOnly", confirmForcePush: true },
  terminal: { shell: null, fontSize: null },
  lsp: { enabled: true, servers: {} },
  forge: { githubClientId: null, gitlabClientId: null },
};

export const cloneSettings = (s: Settings): Settings => JSON.parse(JSON.stringify(s)) as Settings;

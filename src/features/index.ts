// Daftar fitur. Setiap fitur mendaftarkan tab, tampilan, status bar, dan command miliknya.
// Urutan penting: fitur yang memakai store Source Control didaftarkan setelahnya, dan
// terminal harus terdaftar sebelum penangan pintasan global dipasang (lihat terminal/shield.ts).
import { registerEditorFeature } from "./editor";
import { registerExplorerFeature } from "./explorer";
import { registerForgeFeature } from "./forge";
import { registerGraphFeature } from "./graph";
import { registerLspFeature } from "./lsp";
import { registerProblemsFeature } from "./problems";
import { registerScmFeature } from "./scm";
import { registerSearchFeature } from "./search";
import { registerSettingsFeature } from "./settings";
import { registerTerminalFeature } from "./terminal";

export function registerFeatures(): void {
  registerEditorFeature();
  registerScmFeature();
  registerExplorerFeature();
  registerSearchFeature();
  registerGraphFeature();
  registerForgeFeature();
  registerTerminalFeature();
  registerLspFeature();
  registerProblemsFeature();
  registerSettingsFeature();
}

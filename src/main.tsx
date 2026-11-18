import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles/theme.css";
import "./styles/base.css";
import { App } from "./App";
import { installKeybindings } from "./app/commands";
import { registerCoreCommands } from "./app/coreCommands";
import { installKeybindingOverrides } from "./app/keybindings";
import { installNotices } from "./app/notices";
import { showError } from "./app/overlay";
import { installSession } from "./app/session";
import { initApp } from "./app/store";
import { registerFeatures } from "./features";

registerFeatures();
registerCoreCommands();
installKeybindings();
installKeybindingOverrides();
installSession();
installNotices();

// Menu konteks bawaan WebView hanya berguna di kolom teks.
window.addEventListener("contextmenu", (e) => {
  const target = e.target as HTMLElement | null;
  if (!target?.closest("input, textarea, .cm-content, .xterm, .oc-selectable")) e.preventDefault();
});
window.addEventListener("unhandledrejection", (e) => console.error("Unhandled rejection", e.reason));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

initApp().catch((e) => showError(e, "OnCode failed to start"));

// Fitur terminal terintegrasi: panel bawah `terminal` dan command-nya.
import { SquareTerminal } from "lucide-react";
import { executeCommand, formatKeybinding, registerCommands } from "@/app/commands";
import { registerPanelView, registerStatusItem } from "@/app/registry";
import { onWorkspaceChanged, showPanel, togglePanel, useApp } from "@/app/store";
import { activeEditor } from "@/features/editor/CodeEditor";
import { installTerminalKeyShield } from "./shield";
import { clearActiveSession, closeAllSessions, closeSession, createSession, ensureSession, focusActiveTerminal, focusNextSession, setAdapterFactory, useTerminals } from "./store";
import { TerminalActions, TerminalPanel } from "./TerminalPanel";

const PANEL_ID = "terminal";

function terminalShown(): boolean {
  const { panelVisible, panelView } = useApp.getState();
  return panelVisible && panelView === PANEL_ID;
}

function toggleTerminal(): void {
  togglePanel(PANEL_ID);
  if (terminalShown()) focusActiveTerminal();
  // Panel disembunyikan: kembalikan fokus ke editor agar ketikan tidak hilang.
  else activeEditor()?.view.focus();
}

async function newTerminal(): Promise<void> {
  showPanel(PANEL_ID);
  await createSession();
}

/** Tombol di status bar: jalan yang terlihat untuk membuka terminal tanpa menghafal pintasan. */
function TerminalStatusButton() {
  const open = useApp((s) => s.panelVisible && s.panelView === PANEL_ID);
  const hasWorkspace = useApp((s) => s.workspace !== null);
  if (!hasWorkspace) return null;
  return (
    <button
      className="oc-status-item"
      title={`${open ? "Hide" : "Show"} Terminal (${formatKeybinding("Mod+`")})`}
      aria-pressed={open}
      onClick={() => void executeCommand("terminal.toggle")}
    >
      <SquareTerminal size={13} />
      Terminal
    </button>
  );
}

export function registerTerminalFeature(): void {
  // xterm dimuat saat terminal pertama dibuat, bukan saat aplikasi mulai.
  setAdapterFactory(async (events) => (await import("./xterm")).createXtermAdapter(events));

  registerPanelView({ id: PANEL_ID, title: "Terminal", order: 10, component: TerminalPanel, actions: TerminalActions });
  registerStatusItem({ id: "terminal.toggle", align: "left", order: 30, component: TerminalStatusButton });

  const hasWorkspace = () => useApp.getState().workspace !== null;
  const hasTerminal = () => useTerminals.getState().activeId !== null;
  registerCommands([
    { id: "terminal.toggle", title: "Toggle Terminal", category: "Terminal", keybinding: "Mod+`", when: hasWorkspace, run: toggleTerminal },
    { id: "terminal.new", title: "New Terminal", category: "Terminal", keybinding: "Mod+Shift+`", when: hasWorkspace, run: newTerminal },
    {
      id: "terminal.kill",
      title: "Kill Active Terminal",
      category: "Terminal",
      when: hasTerminal,
      run: () => {
        const id = useTerminals.getState().activeId;
        if (id !== null) closeSession(id);
      },
    },
    { id: "terminal.clear", title: "Clear Terminal", category: "Terminal", when: hasTerminal, run: clearActiveSession },
    {
      id: "terminal.focusNext",
      title: "Focus Next Terminal",
      category: "Terminal",
      when: hasTerminal,
      run: () => {
        showPanel(PANEL_ID);
        focusNextSession();
      },
    },
  ]);

  // Harus dipasang sebelum penangan keybinding global (lihat shield.ts).
  installTerminalKeyShield();

  // Backend menghentikan semua PTY setiap kali workspace dibuka ulang (folder diganti, ditutup,
  // atau status kepercayaannya berubah), jadi semua sesi di sini ikut dibuang.
  onWorkspaceChanged((workspace) => {
    closeAllSessions();
    if (workspace && terminalShown()) ensureSession();
  });
}

// Menghubungkan penyaring tombol dengan registry command aplikasi.
import { commandForKeybinding, getCommand, isMac, keybindingFromEvent, keybindingsFor } from "@/app/commands";
import { COMMANDS_TO_SKIP_SHELL, classifyKey } from "./keys";

/** Keybinding (kanonis) milik command yang tetap berlaku saat terminal fokus. */
export function appBindings(): Set<string> {
  const bindings = new Set<string>();
  for (const id of COMMANDS_TO_SKIP_SHELL) {
    const command = getCommand(id);
    if (!command) continue;
    for (const binding of keybindingsFor(command)) bindings.add(binding);
  }
  return bindings;
}

/**
 * Penangan keybinding global berjalan pada fase capture di `window`, yaitu sebelum xterm melihat
 * tombolnya, sehingga tanpa ini Ctrl+W, Ctrl+S, dsb. akan dijalankan aplikasi alih-alih sampai ke shell.
 * Penangan ini dipasang lebih dulu (saat fitur didaftarkan, sebelum `installKeybindings`) dan menandai
 * tombol milik terminal dengan `preventDefault`; penangan global melewatkan event yang sudah
 * `defaultPrevented`, sedangkan xterm tetap memprosesnya.
 */
export function installTerminalKeyShield(target: Window = window): () => void {
  const handler = (event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
    const element = event.target as Element | null;
    if (!element || typeof element.closest !== "function" || !element.closest(".tm-host")) return;
    if (classifyKey(event, { isMac, appBindings: appBindings() }) === "app") return;
    const binding = keybindingFromEvent(event);
    if (binding && commandForKeybinding(binding)) event.preventDefault();
  };
  target.addEventListener("keydown", handler, true);
  return () => target.removeEventListener("keydown", handler, true);
}

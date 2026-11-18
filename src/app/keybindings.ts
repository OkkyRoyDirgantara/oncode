// Penimpaan pintasan keyboard dari `keybindings.json` milik pengguna.
import { commands } from "@/ipc";
import { registerCommands, setKeybindingOverrides } from "./commands";
import { openFile } from "./coreCommands";
import { showError, toast } from "./overlay";

/**
 * Mengurai isi keybindings.json: objek `{ "<id command>": "<pintasan>" | ["<pintasan>", ...] }`.
 * Baris komentar `//` diabaikan. Daftar kosong menghapus pintasan bawaan.
 */
export function parseKeybindings(text: string): { overrides: Record<string, string | string[]>; error: string | null } {
  const stripped = text
    .split("\n")
    .filter((line) => !line.trim().startsWith("//"))
    .join("\n");
  let data: unknown;
  try {
    data = JSON.parse(stripped.trim() === "" ? "{}" : stripped);
  } catch (e) {
    return { overrides: {}, error: e instanceof Error ? e.message : String(e) };
  }
  if (typeof data !== "object" || data === null || Array.isArray(data)) return { overrides: {}, error: "The file must contain a JSON object" };
  const overrides: Record<string, string | string[]> = {};
  for (const [id, value] of Object.entries(data)) {
    if (typeof value === "string") overrides[id] = value;
    else if (Array.isArray(value) && value.every((v) => typeof v === "string")) overrides[id] = value as string[];
    else return { overrides: {}, error: `The value for "${id}" must be a string or a list of strings` };
  }
  return { overrides, error: null };
}

export async function loadKeybindings(notify = false): Promise<void> {
  const text = await commands.keybindingsGet().catch(() => "{}");
  const { overrides, error } = parseKeybindings(text);
  if (error) {
    toast.error(`keybindings.json could not be read: ${error}`);
    return;
  }
  setKeybindingOverrides(overrides);
  if (notify) toast.success("Keyboard shortcuts reloaded.");
}

export function installKeybindingOverrides(): void {
  void loadKeybindings();
  registerCommands([
    {
      id: "keybindings.openJson",
      title: "Open Keyboard Shortcuts (JSON)",
      category: "Preferences",
      run: async () => {
        try {
          openFile(await commands.keybindingsEnsure());
        } catch (e) {
          showError(e);
        }
      },
    },
    { id: "keybindings.reload", title: "Reload Keyboard Shortcuts", category: "Preferences", run: () => loadKeybindings(true) },
  ]);
  // Menyimpan keybindings.json dari editor langsung menerapkannya.
  window.addEventListener("oncode:saved", (e) => {
    const path = (e as CustomEvent<{ path: string }>).detail.path;
    if (/[\\/]keybindings\.json$/.test(path)) void loadKeybindings(true);
  });
}

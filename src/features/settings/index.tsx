// Fitur pengaturan: tab `settings` dan command untuk membukanya.
import { Settings as SettingsIcon } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { openFile } from "@/app/coreCommands";
import { registerTabType } from "@/app/registry";
import { useApp } from "@/app/store";
import { openTab } from "@/app/workbench";
import { SettingsTab } from "./SettingsTab";

/** Membuka (atau mengaktifkan) tab pengaturan. Tab juga bisa dibuka tanpa folder proyek. */
export function openSettings(): void {
  openTab({ id: "settings", kind: "settings", title: "Settings", description: "User settings", params: {} });
}

function openSettingsJson(): void {
  const path = useApp.getState().info?.settingsPath;
  if (path) openFile(path);
}

export function registerSettingsFeature(): void {
  registerTabType({
    kind: "settings",
    component: SettingsTab,
    icon: () => <SettingsIcon size={14} />,
  });
  registerCommands([
    { id: "settings.open", title: "Open Settings", category: "Preferences", keybinding: "Mod+,", run: openSettings },
    { id: "settings.openJson", title: "Open settings.json", category: "Preferences", run: openSettingsJson },
  ]);
}

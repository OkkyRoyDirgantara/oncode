// Kerangka aplikasi: layar awal, pengaturan tanpa folder, dan penimpaan pintasan keyboard.
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export const tests = {
  async "layar awal tanpa folder; Settings tetap bisa dibuka"(t) {
    const app = await t.launch(null);
    await app.waitForText(".oc-welcome h1", "OnCode");
    t.assert(await app.exists(".oc-welcome button"), "tombol di layar awal");
    await app.clickText(".oc-welcome button", "Settings");
    await app.waitForSelector(".st-page");
    t.assert(!(await app.exists(".oc-welcome")), "tab Settings menggantikan layar awal");
    t.assert(!(await app.exists(".oc-activitybar")), "tanpa folder tidak ada sidebar");

    // Menutup tab terakhir kembali ke layar awal.
    await app.click(".oc-tab-close");
    await app.waitForSelector(".oc-welcome");
  },

  async "keybindings.json menimpa pintasan bawaan"(t) {
    mkdirSync(t.configDir, { recursive: true });
    const file = join(t.configDir, "keybindings.json");
    writeFileSync(file, '// uji\n{ "workbench.toggleSidebar": "Mod+Shift+L" }\n');
    try {
      const repo = t.repo();
      t.write(repo, "a.txt", "a\n");
      t.commitAll(repo, "Commit pertama");
      const app = await t.launch(repo);
      await app.waitForSelector(".scm-commit");
      const sidebarVisible = () => app.exec("return document.querySelector('.scm-commit').getBoundingClientRect().width > 50");
      t.assert(await sidebarVisible(), "sidebar awalnya terlihat");

      await app.shortcut("b", { ctrl: true });
      await t.sleep(400);
      t.assert(await sidebarVisible(), "pintasan bawaan (Ctrl+B) tidak lagi berlaku");

      await app.shortcut("L", { ctrl: true, shift: true });
      await app.waitFor("sidebar tersembunyi lewat pintasan baru", "return document.querySelector('.scm-commit').getBoundingClientRect().width < 50");
      await app.shortcut("L", { ctrl: true, shift: true });
      await app.waitFor("sidebar tampil lagi", "return document.querySelector('.scm-commit').getBoundingClientRect().width > 50");
    } finally {
      rmSync(file, { force: true });
    }
  },
};

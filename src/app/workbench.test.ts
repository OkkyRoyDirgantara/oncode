import { beforeEach, describe, expect, it } from "vitest";
import { registerTabType } from "./registry";
import { activateTab, activeTab, closeTab, closeTabs, cycleTab, openTab, pinTab, resetWorkbenchForTest, splitActiveTab, updateTabs, useWorkbench } from "./workbench";

const file = (path: string) => ({ kind: "file", title: path, params: { path } });
const state = () => useWorkbench.getState();
const titles = (groupIndex = 0) => state().groups[groupIndex].tabs.map((t) => t.title);

describe("workbench tabs", () => {
  beforeEach(() => {
    resetWorkbenchForTest();
    registerTabType({ kind: "file", component: () => null });
  });

  it("membuka tab sekali saja dan mengaktifkannya", () => {
    openTab(file("a"));
    openTab(file("b"));
    openTab(file("a"));
    expect(titles()).toEqual(["a", "b"]);
    expect(activeTab()?.tab.title).toBe("a");
  });

  it("menyisipkan tab baru setelah tab aktif", () => {
    openTab(file("a"));
    openTab(file("b"));
    activateTab(state().groups[0].id, "file:" + JSON.stringify({ path: "a" }));
    openTab(file("c"));
    expect(titles()).toEqual(["a", "c", "b"]);
  });

  it("tab pratinjau diganti oleh pratinjau berikutnya, kecuali sudah disematkan", () => {
    openTab(file("a"), { preview: true });
    openTab(file("b"), { preview: true });
    expect(titles()).toEqual(["b"]);
    pinTab(activeTab()!.tab.id);
    openTab(file("c"), { preview: true });
    expect(titles()).toEqual(["b", "c"]);
    // Membuka ulang tanpa pratinjau menyematkan tab.
    openTab(file("c"));
    expect(state().groups[0].tabs[1].preview).toBe(false);
  });

  it("menutup tab aktif memilih tetangga kanan, lalu kiri", async () => {
    openTab(file("a"));
    openTab(file("b"));
    openTab(file("c"));
    const g = state().groups[0];
    activateTab(g.id, g.tabs[1].id);
    await closeTab(g.id, g.tabs[1].id);
    expect(activeTab()?.tab.title).toBe("c");
    await closeTab(g.id, activeTab()!.tab.id);
    expect(activeTab()?.tab.title).toBe("a");
    await closeTab(g.id, activeTab()!.tab.id);
    expect(activeTab()).toBeNull();
    expect(state().groups).toHaveLength(1);
  });

  it("beforeClose bisa membatalkan penutupan", async () => {
    registerTabType({ kind: "file", component: () => null, beforeClose: (t) => t.title !== "kotor" });
    openTab(file("bersih"));
    openTab(file("kotor"));
    const g = state().groups[0];
    const ok = await closeTabs(g.id, g.tabs.map((t) => t.id));
    expect(ok).toBe(false);
    expect(titles()).toEqual(["kotor"]);
    expect(await closeTab(g.id, state().groups[0].tabs[0].id, true)).toBe(true);
    expect(titles()).toEqual([]);
  });

  it("split membuat grup kedua dan grup kosong dibuang", async () => {
    openTab(file("a"));
    splitActiveTab();
    expect(state().groups).toHaveLength(2);
    expect(titles(1)).toEqual(["a"]);
    expect(state().activeGroupId).toBe(state().groups[1].id);
    // Tab yang juga terbuka di grup lain ditutup tanpa bertanya.
    registerTabType({ kind: "file", component: () => null, beforeClose: () => false });
    const g2 = state().groups[1];
    expect(await closeTab(g2.id, g2.tabs[0].id)).toBe(true);
    expect(state().groups).toHaveLength(1);
    expect(state().activeGroupId).toBe(state().groups[0].id);
  });

  it("membuka tab yang sudah ada di grup lain mengaktifkan grup itu", () => {
    openTab(file("a"));
    openTab(file("b"), { toSide: true });
    openTab(file("a"));
    expect(state().activeGroupId).toBe(state().groups[0].id);
    expect(titles(1)).toEqual(["b"]);
  });

  it("updateTabs mengganti isi tab dan menjaga tab aktif", () => {
    openTab(file("lama.txt"));
    updateTabs((t) => (t.params.path === "lama.txt" ? { ...t, id: "file:baru", title: "baru.txt", params: { path: "baru.txt" } } : null));
    expect(titles()).toEqual(["baru.txt"]);
    expect(activeTab()?.tab.id).toBe("file:baru");
  });

  it("cycleTab berputar", () => {
    openTab(file("a"));
    openTab(file("b"));
    cycleTab(1);
    expect(activeTab()?.tab.title).toBe("a");
    cycleTab(-1);
    expect(activeTab()?.tab.title).toBe("b");
  });
});

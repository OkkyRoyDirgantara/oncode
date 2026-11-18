// Klien WebDriver minimal untuk menguji aplikasi lewat tauri-driver (tanpa dependensi tambahan).
import { writeFileSync } from "node:fs";

const BASE = process.env.WEBDRIVER_URL ?? "http://127.0.0.1:4444";
const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";

async function call(method, path, body) {
  const response = await fetch(BASE + path, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = await response.json();
  if (json.value && typeof json.value === "object" && json.value.error) {
    throw new Error(`${method} ${path}: ${json.value.error}: ${json.value.message}`);
  }
  return json.value;
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export class App {
  constructor(sessionId) {
    this.id = sessionId;
  }

  static async launch(application, args = []) {
    const value = await call("POST", "/session", { capabilities: { alwaysMatch: { "tauri:options": { application, args } } } });
    return new App(value.sessionId);
  }

  async quit() {
    await call("DELETE", `/session/${this.id}`).catch(() => {});
  }

  /** Menjalankan skrip di halaman; `arguments` berisi `args`. */
  exec(script, ...args) {
    return call("POST", `/session/${this.id}/execute/sync`, { script, args });
  }

  async find(selector) {
    const value = await call("POST", `/session/${this.id}/element`, { using: "css selector", value: selector });
    return value[ELEMENT];
  }

  async exists(selector) {
    return this.exec("return document.querySelector(arguments[0]) !== null", selector);
  }

  async click(selector) {
    const element = await this.find(selector);
    await call("POST", `/session/${this.id}/element/${element}/click`, {});
  }

  /** Mengeklik elemen pertama yang cocok dengan `selector` dan teksnya memuat `text`. */
  async clickText(selector, text) {
    const ok = await this.exec(
      `const el = [...document.querySelectorAll(arguments[0])].find((e) => e.textContent.includes(arguments[1]));
       if (!el) return false;
       el.scrollIntoView({ block: "center" });
       const r = el.getBoundingClientRect();
       const init = { bubbles: true, cancelable: true, view: window, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, button: 0 };
       el.dispatchEvent(new MouseEvent("mousedown", init));
       el.dispatchEvent(new MouseEvent("mouseup", init));
       el.dispatchEvent(new MouseEvent("click", init));
       return true;`,
      selector,
      text,
    );
    if (!ok) throw new Error(`Tidak ada ${selector} dengan teks ${JSON.stringify(text)}`);
  }

  /** Mengeklik lewat DOM; untuk tombol yang baru terlihat saat baris disorot (hover). */
  async clickDom(selector) {
    const ok = await this.exec("const el = document.querySelector(arguments[0]); if (!el) return false; el.click(); return true;", selector);
    if (!ok) throw new Error(`Tidak ada elemen ${selector}`);
  }

  /** Membuka menu konteks pada elemen pertama yang cocok dan teksnya memuat `text`. */
  async contextMenu(selector, text = "") {
    const ok = await this.exec(
      `const el = [...document.querySelectorAll(arguments[0])].find((e) => e.textContent.includes(arguments[1]));
       if (!el) return false;
       el.scrollIntoView({ block: "center" });
       const r = el.getBoundingClientRect();
       el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, view: window, clientX: r.left + 20, clientY: r.top + r.height / 2, button: 2 }));
       return true;`,
      selector,
      text,
    );
    if (!ok) throw new Error(`Tidak ada ${selector} dengan teks ${JSON.stringify(text)}`);
  }

  /** Menekan pintasan global aplikasi, mis. `shortcut("H", { ctrl: true, shift: true })`. */
  async shortcut(key, { ctrl = false, shift = false, alt = false } = {}) {
    const code = /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : key === "`" ? "Backquote" : key === "," ? "Comma" : key;
    await this.exec(
      "window.dispatchEvent(new KeyboardEvent('keydown', { key: arguments[0], code: arguments[1], ctrlKey: arguments[2], shiftKey: arguments[3], altKey: arguments[4], bubbles: true, cancelable: true }))",
      key,
      code,
      ctrl,
      shift,
      alt,
    );
  }

  /** Mengisi input, textarea, atau select yang dikendalikan React. */
  async setValue(selector, value) {
    const ok = await this.exec(
      `const el = document.querySelector(arguments[0]);
       if (!el) return false;
       const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
       Object.getOwnPropertyDescriptor(proto, "value").set.call(el, arguments[1]);
       el.dispatchEvent(new Event("input", { bubbles: true }));
       el.dispatchEvent(new Event("change", { bubbles: true }));
       return true;`,
      selector,
      value,
    );
    if (!ok) throw new Error(`Tidak ada elemen ${selector}`);
  }

  async type(selector, text) {
    const element = await this.find(selector);
    await call("POST", `/session/${this.id}/element/${element}/value`, { text });
  }

  /** Mengirim tombol ke elemen yang sedang fokus (kode WebDriver, mis. "" untuk Ctrl). */
  async keys(...sequence) {
    const actions = [];
    for (const key of sequence) actions.push({ type: "keyDown", value: key });
    for (const key of [...sequence].reverse()) actions.push({ type: "keyUp", value: key });
    await call("POST", `/session/${this.id}/actions`, { actions: [{ type: "key", id: "kbd", actions }] });
    await call("DELETE", `/session/${this.id}/actions`).catch(() => {});
  }

  text(selector) {
    return this.exec("const e = document.querySelector(arguments[0]); return e ? e.textContent : null", selector);
  }

  texts(selector) {
    return this.exec("return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)", selector);
  }

  /** Menunggu sampai `predicate` (skrip yang mengembalikan nilai truthy) terpenuhi. */
  async waitFor(description, script, { timeout = 10000, args = [] } = {}) {
    const started = Date.now();
    for (;;) {
      const value = await this.exec(script, ...args).catch(() => null);
      if (value) return value;
      if (Date.now() - started > timeout) throw new Error(`Menunggu terlalu lama: ${description}`);
      await sleep(150);
    }
  }

  waitForSelector(selector, options) {
    return this.waitFor(`elemen ${selector}`, "return document.querySelector(arguments[0]) !== null", { ...options, args: [selector] });
  }

  waitForText(selector, text, options) {
    return this.waitFor(
      `${selector} memuat ${JSON.stringify(text)}`,
      "return [...document.querySelectorAll(arguments[0])].some((e) => e.textContent.includes(arguments[1]))",
      { ...options, args: [selector, text] },
    );
  }

  async screenshot(file) {
    const data = await call("GET", `/session/${this.id}/screenshot`);
    writeFileSync(file, Buffer.from(data, "base64"));
    return file;
  }
}

// Kode tombol WebDriver (area Unicode privat).
export const Key = { Ctrl: "\uE009", Shift: "\uE008", Alt: "\uE00A", Enter: "\uE007", Escape: "\uE00C", Down: "\uE015", Up: "\uE013", Left: "\uE012", Right: "\uE014", Home: "\uE011", End: "\uE010", F2: "\uE032", F12: "\uE03C" };

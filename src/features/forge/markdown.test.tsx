import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Markdown, renderMarkdown, sanitizeHtml } from "./markdown";
import { safeExternalUrl, safeImageUrl } from "./util";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }));

/** Mem-parse HTML hasil sanitasi tanpa menjalankannya. */
function parse(source: string): HTMLElement {
  const box = document.createElement("div");
  box.innerHTML = renderMarkdown(source);
  return box;
}

/** Tidak ada satu pun atribut penangan event atau URL berbahaya yang tersisa. */
function expectInert(root: HTMLElement) {
  for (const el of root.querySelectorAll("*")) {
    for (const attr of el.attributes) {
      expect(attr.name.startsWith("on"), `atribut ${attr.name} pada <${el.tagName}>`).toBe(false);
      expect(/^\s*(javascript|vbscript|data|file):/i.test(attr.value), `nilai ${attr.name}="${attr.value}"`).toBe(false);
    }
  }
  expect(root.querySelector("script, style, iframe, object, embed, form, input, button, svg, math, link, meta, base")).toBeNull();
}

describe("renderMarkdown", () => {
  it("merender Markdown biasa", () => {
    const root = parse("# Title\n\nSome **bold** text and `code`.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |");
    expect(root.querySelector("h1")?.textContent).toBe("Title");
    expect(root.querySelector("strong")?.textContent).toBe("bold");
    expect(root.querySelector("code")?.textContent).toBe("code");
    expect(root.querySelectorAll("li")).toHaveLength(2);
    expect(root.querySelector("table td")?.textContent).toBe("1");
  });

  it("tag script tidak pernah menjadi elemen", () => {
    const root = parse("before <script>window.pwned = 1</script> after\n\n<script src=\"https://evil.example/x.js\"></script>");
    expectInert(root);
    // HTML mentah dimatikan: isinya tampil sebagai teks.
    expect(root.textContent).toContain("<script>window.pwned = 1</script>");
  });

  it("HTML mentah lain ditampilkan sebagai teks, bukan dijalankan", () => {
    const root = parse(
      [
        '<img src="x" onerror="window.pwned = 1">',
        '<iframe src="https://evil.example"></iframe>',
        '<a href="https://example.com" onclick="window.pwned = 1">x</a>',
        '<div style="position:fixed;inset:0" onmouseover="window.pwned = 1">overlay</div>',
        "<svg><script>window.pwned = 1</script></svg>",
        '<form action="https://evil.example"><input name="token"></form>',
        '<base href="https://evil.example/">',
      ].join("\n\n"),
    );
    expectInert(root);
    expect(root.querySelector("img, iframe, div")).toBeNull();
    // URL di dalam teks boleh ditautkan otomatis (linkify), tetapi hanya dengan href dan rel.
    for (const a of root.querySelectorAll("a")) expect([...a.attributes].map((x) => x.name).sort()).toEqual(["href", "rel"]);
    expect(root.textContent).toContain('<img src="x" onerror="window.pwned = 1">');
  });

  it("tautan javascript:, data:, vbscript:, file: dan tautan relatif tidak punya tujuan", () => {
    const root = parse(
      [
        "[js](javascript:alert(1))",
        "[JS](JaVaScRiPt:alert(1))",
        "[data](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)",
        "[vb](vbscript:msgbox(1))",
        "[file](file:///etc/passwd)",
        "[rel](/settings)",
        "[hash](#top)",
        "[proto](//evil.example/x)",
        "<javascript:alert(1)>",
      ].join("\n\n"),
    );
    expectInert(root);
    for (const a of root.querySelectorAll("a")) expect(a.hasAttribute("href"), a.outerHTML).toBe(false);
  });

  it("penangan event yang diselipkan lewat atribut tautan atau gambar tidak lolos", () => {
    const root = parse('[x](https://example.com "t\\" onmouseover=\\"window.pwned=1")\n\n![alt" onerror="window.pwned=1](https://example.com/a.png)');
    expectInert(root);
    const a = root.querySelector("a");
    expect(a?.getAttribute("href")).toBe("https://example.com/");
    expect(a?.getAttribute("onmouseover")).toBeNull();
    expect(root.querySelector("img")?.getAttribute("onerror")).toBeNull();
  });

  it("tautan http(s) dipertahankan dengan rel yang aman dan tanpa target", () => {
    const root = parse("[docs](https://example.com/docs?a=1#x) and http://plain.example/page");
    const links = [...root.querySelectorAll("a")];
    expect(links.map((a) => a.getAttribute("href"))).toEqual(["https://example.com/docs?a=1#x", "http://plain.example/page"]);
    for (const a of links) {
      expect(a.getAttribute("rel")).toBe("noopener noreferrer nofollow");
      expect(a.hasAttribute("target")).toBe(false);
    }
  });

  it("gambar hanya dari https, tanpa referrer; sumber lain diganti teks", () => {
    const root = parse("![ok](https://img.example/a.png)\n\n![plain](http://img.example/a.png)\n\n![inline](data:image/png;base64,AAAA)\n\n![local](file:///tmp/a.png)\n\n![rel](a.png)");
    const images = [...root.querySelectorAll("img")];
    expect(images).toHaveLength(1);
    expect(images[0].getAttribute("src")).toBe("https://img.example/a.png");
    expect(images[0].getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(root.textContent).toContain("[image: plain]");
    expect(root.textContent).toContain("[image: rel]");
    expectInert(root);
  });

  it("keluaran tidak menjalankan apa pun saat dipasang ke dokumen", () => {
    const w = window as unknown as { pwned?: number };
    delete w.pwned;
    const { container } = render(<Markdown source={'<img src=x onerror="window.pwned=1">\n\n<script>window.pwned=1</script>\n\n[x](javascript:window.pwned=1)'} />);
    for (const a of container.querySelectorAll("a")) fireEvent.click(a);
    expect(w.pwned).toBeUndefined();
  });
});

describe("sanitizeHtml (lapisan DOMPurify, tanpa bantuan markdown-it)", () => {
  function clean(html: string): HTMLElement {
    const box = document.createElement("div");
    box.innerHTML = sanitizeHtml(html);
    return box;
  }

  it("membuang script, penangan event, dan elemen di luar daftar izin", () => {
    const root = clean(
      '<p onclick="pwn()">hi<script>pwn()</script></p><img src="https://img.example/a.png" onerror="pwn()"><iframe src="https://evil.example"></iframe>' +
        '<svg onload="pwn()"><a xlink:href="javascript:pwn()">x</a></svg><style>body{display:none}</style><form><input name="t"><button>go</button></form>' +
        '<div style="position:fixed" class="oc-overlay" id="root" data-x="1">overlay</div><math><mtext><script>pwn()</script></mtext></math>',
    );
    expectInert(root);
    expect(root.querySelector("p")?.textContent).toBe("hi");
    expect(root.querySelector("img")?.getAttribute("src")).toBe("https://img.example/a.png");
    expect(root.querySelector("div")).toBeNull();
    expect(root.querySelector("[style], [class], [id], [data-x]")).toBeNull();
  });

  it("menghapus href yang bukan http(s) dan gambar yang bukan https", () => {
    const root = clean(
      '<a href="javascript:pwn()">a</a><a href="data:text/html,x">b</a><a href="/relative">c</a><a href="tauri://localhost/x">d</a>' +
        '<a href="https://ok.example/" target="_top">e</a><img src="http://img.example/a.png" alt="plain"><img src="data:image/svg+xml,<svg onload=pwn()>">',
    );
    expectInert(root);
    const links = [...root.querySelectorAll("a")];
    expect(links.map((a) => a.getAttribute("href"))).toEqual([null, null, null, null, "https://ok.example/"]);
    expect(links[4].hasAttribute("target")).toBe(false);
    expect(root.querySelector("img")).toBeNull();
    expect(root.textContent).toContain("[image: plain]");
  });
});

describe("Markdown", () => {
  beforeEach(() => vi.mocked(openUrl).mockClear());

  it("klik tautan membuka browser sistem dan tidak bernavigasi", () => {
    const { container } = render(<Markdown source="See [the docs](https://example.com/docs)." />);
    const link = container.querySelector("a")!;
    expect(link.getAttribute("href")).toBe("https://example.com/docs");
    // `fireEvent` mengembalikan false bila preventDefault dipanggil (navigasi dibatalkan).
    const notPrevented = fireEvent.click(link);
    expect(notPrevented).toBe(false);
    expect(openUrl).toHaveBeenCalledTimes(1);
    expect(openUrl).toHaveBeenCalledWith("https://example.com/docs");
  });

  it("klik tengah juga ditangkap", () => {
    const { container } = render(<Markdown source="[x](https://example.com/)" />);
    const link = container.querySelector("a")!;
    const event = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    link.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(openUrl).toHaveBeenCalledWith("https://example.com/");
  });

  it("klik pada elemen di dalam tautan tetap ditangani", () => {
    const { container } = render(<Markdown source="[**bold link**](https://example.com/b)" />);
    expect(fireEvent.click(container.querySelector("strong")!)).toBe(false);
    expect(openUrl).toHaveBeenCalledWith("https://example.com/b");
  });

  it("tautan tanpa tujuan yang sah tidak membuka apa pun", () => {
    const { container } = render(<Markdown source="[x](javascript:alert(1)) [y](/relative)" />);
    for (const a of container.querySelectorAll("a")) expect(fireEvent.click(a)).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });
});

describe("safeExternalUrl / safeImageUrl", () => {
  it("hanya menerima http(s) absolut", () => {
    expect(safeExternalUrl("https://example.com/a b")).toBe("https://example.com/a%20b");
    expect(safeExternalUrl("http://example.com")).toBe("http://example.com/");
    for (const bad of ["javascript:alert(1)", " javascript:alert(1)", "data:text/html,x", "file:///etc/passwd", "/path", "//host/x", "mailto:a@b.c", "tauri://localhost", "", null, undefined, "https://"]) {
      expect(safeExternalUrl(bad), String(bad)).toBeNull();
    }
  });

  it("gambar hanya https", () => {
    expect(safeImageUrl("https://avatars.example/u/1")).toBe("https://avatars.example/u/1");
    expect(safeImageUrl("http://avatars.example/u/1")).toBeNull();
    expect(safeImageUrl("data:image/png;base64,AAAA")).toBeNull();
    expect(safeImageUrl(null)).toBeNull();
  });
});

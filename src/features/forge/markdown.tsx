// Merender Markdown dari forge (deskripsi PR/MR). Isinya tidak dipercaya:
// HTML mentah dimatikan di markdown-it, hasilnya disanitasi DOMPurify dengan daftar izin sempit,
// tautan hanya http(s) dan dibuka di browser sistem, gambar hanya dari https.
import { useMemo } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import createDOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import { openExternal, safeExternalUrl, safeImageUrl } from "./util";

const md = new MarkdownIt({ html: false, linkify: true, breaks: true, typographer: false });

const ALLOWED_TAGS = [
  "a", "p", "br", "hr", "blockquote", "pre", "code", "em", "strong", "s", "del",
  "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "img",
  "table", "thead", "tbody", "tr", "th", "td",
];
const ALLOWED_ATTR = ["href", "src", "alt", "title", "start", "align"];

// Instance tersendiri agar hook di bawah tidak memengaruhi pemakai DOMPurify lain.
const purifier = createDOMPurify(window);

purifier.addHook("afterSanitizeAttributes", (node) => {
  const tag = node.nodeName.toLowerCase();
  if (tag === "a") {
    const href = safeExternalUrl(node.getAttribute("href"));
    if (href) {
      node.setAttribute("href", href);
      node.setAttribute("rel", "noopener noreferrer nofollow");
    } else {
      // Tautan relatif, `javascript:`, `data:`, dll.: tetap teks biasa, tanpa tujuan.
      node.removeAttribute("href");
    }
    node.removeAttribute("target");
  } else if (tag === "img") {
    const src = safeImageUrl(node.getAttribute("src"));
    if (src) {
      node.setAttribute("src", src);
      node.setAttribute("referrerpolicy", "no-referrer");
      node.setAttribute("loading", "lazy");
    } else {
      // Gambar dari sumber selain https diganti teks alternatifnya.
      const alt = node.getAttribute("alt") ?? "";
      node.parentNode?.replaceChild(node.ownerDocument.createTextNode(alt ? `[image: ${alt}]` : "[image]"), node);
    }
  }
});

/** Lapisan kedua: menyanitasi HTML apa pun dengan daftar izin sempit, terlepas dari apa yang dihasilkan markdown-it. */
export function sanitizeHtml(html: string): string {
  return purifier.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    ALLOWED_URI_REGEXP: /^https?:\/\//i,
  });
}

/** Markdown → HTML yang aman untuk `dangerouslySetInnerHTML`. */
export function renderMarkdown(source: string): string {
  return sanitizeHtml(md.render(source));
}

/** Menangani klik pada tautan di dalam Markdown: tidak pernah bernavigasi, hanya membuka browser sistem. */
function onLinkActivate(e: ReactMouseEvent<HTMLDivElement>) {
  const target = e.target instanceof Element ? e.target.closest("a") : null;
  if (!target || !e.currentTarget.contains(target)) return;
  e.preventDefault();
  void openExternal(target.getAttribute("href"));
}

export function Markdown({ source, className }: { source: string; className?: string }) {
  const html = useMemo(() => renderMarkdown(source), [source]);
  return (
    <div
      className={`fg-markdown oc-selectable${className ? ` ${className}` : ""}`}
      onClick={onLinkActivate}
      onAuxClick={onLinkActivate}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

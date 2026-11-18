// Pemetaan nama berkas ke bahasa (highlight dimuat saat dibutuhkan) dan ke id bahasa LSP.
import { LanguageDescription, type LanguageSupport } from "@codemirror/language";
import { languages } from "@codemirror/language-data";

export function describeLanguage(path: string): LanguageDescription | null {
  const name = path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
  return LanguageDescription.matchFilename(languages, name);
}

export async function loadLanguage(path: string): Promise<LanguageSupport | null> {
  const description = describeLanguage(path);
  if (!description) return null;
  try {
    return await description.load();
  } catch (e) {
    console.warn(`Gagal memuat bahasa ${description.name}`, e);
    return null;
  }
}

const LSP_IDS: Record<string, string> = {
  Rust: "rust",
  TypeScript: "typescript",
  TSX: "typescriptreact",
  JavaScript: "javascript",
  JSX: "javascriptreact",
  Python: "python",
  Go: "go",
  C: "c",
  "C++": "cpp",
  JSON: "json",
  CSS: "css",
  HTML: "html",
  Markdown: "markdown",
  YAML: "yaml",
  TOML: "toml",
  Java: "java",
  PHP: "php",
  Ruby: "ruby",
  Shell: "shellscript",
};

/** Nama bahasa untuk status bar, mis. "Rust"; "Plain Text" bila tidak dikenali. */
export function languageName(path: string): string {
  return describeLanguage(path)?.name ?? "Plain Text";
}

/** Id bahasa menurut LSP (mis. `typescriptreact`), atau null bila tidak dikenali. */
export function languageId(path: string): string | null {
  const name = describeLanguage(path)?.name;
  if (!name) return null;
  return LSP_IDS[name] ?? name.toLowerCase();
}

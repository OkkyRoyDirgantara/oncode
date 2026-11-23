//! Bahasa sebuah berkas: nama tampilan, id highlighter, dan id bahasa LSP.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageInfo {
    /// Nama untuk status bar.
    pub name: &'static str,
    /// Nama bahasa untuk highlighter (`EditorState::language`); `text` bila tidak ada grammar.
    pub highlighter: &'static str,
    /// `languageId` menurut LSP; juga kunci `lsp.servers` di pengaturan.
    pub lsp: &'static str,
}

const PLAIN: LanguageInfo = LanguageInfo { name: "Plain Text", highlighter: "text", lsp: "plaintext" };

const fn lang(name: &'static str, highlighter: &'static str, lsp: &'static str) -> LanguageInfo {
    LanguageInfo { name, highlighter, lsp }
}

/// Bahasa menurut nama berkas atau ekstensinya.
pub fn language_for_path(path: &str) -> LanguageInfo {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = file.to_lowercase();
    match lower.as_str() {
        "makefile" | "gnumakefile" => return lang("Makefile", "text", "makefile"),
        "dockerfile" => return lang("Dockerfile", "text", "dockerfile"),
        "cargo.lock" => return lang("TOML", "toml", "toml"),
        ".gitignore" | ".gitattributes" | ".gitmodules" => return lang("Git Config", "text", "plaintext"),
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return lang("Shell Script", "bash", "shellscript"),
        _ => {}
    }
    let Some((_, extension)) = lower.rsplit_once('.') else { return PLAIN };
    match extension {
        "rs" => lang("Rust", "rust", "rust"),
        "ts" | "mts" | "cts" => lang("TypeScript", "typescript", "typescript"),
        "tsx" => lang("TypeScript JSX", "tsx", "typescriptreact"),
        "js" | "mjs" | "cjs" => lang("JavaScript", "javascript", "javascript"),
        "jsx" => lang("JavaScript JSX", "javascript", "javascriptreact"),
        "json" | "jsonc" => lang("JSON", "json", "json"),
        "py" | "pyi" => lang("Python", "python", "python"),
        "go" => lang("Go", "go", "go"),
        "c" | "h" => lang("C", "c", "c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => lang("C++", "cpp", "cpp"),
        "java" => lang("Java", "java", "java"),
        "rb" => lang("Ruby", "ruby", "ruby"),
        "html" | "htm" => lang("HTML", "html", "html"),
        "css" => lang("CSS", "css", "css"),
        "scss" => lang("SCSS", "css", "scss"),
        "md" | "markdown" => lang("Markdown", "markdown", "markdown"),
        "toml" => lang("TOML", "toml", "toml"),
        "yml" | "yaml" => lang("YAML", "yaml", "yaml"),
        "sh" | "bash" | "zsh" => lang("Shell Script", "bash", "shellscript"),
        "sql" => lang("SQL", "sql", "sql"),
        "diff" | "patch" => lang("Diff", "diff", "diff"),
        "php" => lang("PHP", "text", "php"),
        "lua" => lang("Lua", "text", "lua"),
        "kt" | "kts" => lang("Kotlin", "text", "kotlin"),
        "swift" => lang("Swift", "text", "swift"),
        "cs" => lang("C#", "text", "csharp"),
        "vue" => lang("Vue", "html", "vue"),
        "svelte" => lang("Svelte", "html", "svelte"),
        "xml" | "svg" => lang("XML", "html", "xml"),
        "txt" | "log" => PLAIN,
        _ => PLAIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_extension_and_file_name() {
        assert_eq!(language_for_path("/a/b/main.rs").name, "Rust");
        assert_eq!(language_for_path("C:\\a\\App.TSX").lsp, "typescriptreact");
        assert_eq!(language_for_path("Makefile").lsp, "makefile");
        assert_eq!(language_for_path("/x/Cargo.lock").highlighter, "toml");
        assert_eq!(language_for_path("README").name, "Plain Text");
        assert_eq!(language_for_path("notes.unknown").highlighter, "text");
    }
}

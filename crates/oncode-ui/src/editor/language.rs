//! Bahasa sebuah berkas: nama tampilan, id highlighter, dan id bahasa LSP.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageInfo {
    /// Nama untuk status bar.
    pub name: &'static str,
    /// Nama bahasa untuk highlighter (`EditorState::language`); `text` bila tidak ada grammar.
    pub highlighter: &'static str,
    /// `languageId` menurut LSP; juga kunci `lsp.servers` di pengaturan.
    pub lsp: &'static str,
    /// Highlight-nya berasal dari ekstensi "Extra languages"; tanpa ekstensi itu ditampilkan polos.
    pub extra: bool,
}

impl LanguageInfo {
    /// Nama bahasa untuk highlighter, dengan memperhitungkan ekstensi "Extra languages".
    pub fn highlighter_with(&self, extra_languages: bool) -> &'static str {
        if self.extra && !extra_languages {
            "text"
        } else {
            self.highlighter
        }
    }
}

const PLAIN: LanguageInfo = LanguageInfo { name: "Plain Text", highlighter: "text", lsp: "plaintext", extra: false };

const fn lang(name: &'static str, highlighter: &'static str, lsp: &'static str) -> LanguageInfo {
    LanguageInfo { name, highlighter, lsp, extra: false }
}

/// Bahasa yang highlight-nya disediakan ekstensi "Extra languages".
const fn extra(name: &'static str, highlighter: &'static str, lsp: &'static str) -> LanguageInfo {
    LanguageInfo { name, highlighter, lsp, extra: true }
}

/// Bahasa menurut nama berkas atau ekstensinya.
pub fn language_for_path(path: &str) -> LanguageInfo {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = file.to_lowercase();
    match lower.as_str() {
        "makefile" | "gnumakefile" => return extra("Makefile", "make", "makefile"),
        "cmakelists.txt" => return extra("CMake", "cmake", "cmake"),
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
        "json" | "jsonc" | "json5" | "jsonl" | "ndjson" => lang("JSON", "json", "json"),
        // Berkas terjemahan Flutter.
        "arb" => lang("JSON", "json", "json"),
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
        "sql" | "psql" | "mysql" => lang("SQL", "sql", "sql"),
        // NoSQL: Cassandra (CQL) dan playground MongoDB.
        "cql" => lang("CQL", "sql", "sql"),
        "mongodb" => lang("MongoDB", "javascript", "javascript"),
        "diff" | "patch" => lang("Diff", "diff", "diff"),
        "php" => extra("PHP", "php", "php"),
        "lua" => extra("Lua", "lua", "lua"),
        "kt" | "kts" => extra("Kotlin", "kotlin", "kotlin"),
        "dart" => extra("Dart", "dart", "dart"),
        "swift" => extra("Swift", "swift", "swift"),
        "cs" | "csx" => extra("C#", "csharp", "csharp"),
        "scala" | "sc" | "sbt" => extra("Scala", "scala", "scala"),
        "graphql" | "gql" => extra("GraphQL", "graphql", "graphql"),
        "proto" => extra("Protocol Buffers", "proto", "proto"),
        "cmake" => extra("CMake", "cmake", "cmake"),
        "mk" => extra("Makefile", "make", "makefile"),
        "zig" => extra("Zig", "zig", "zig"),
        "ex" | "exs" => extra("Elixir", "elixir", "elixir"),
        "vue" => lang("Vue", "html", "vue"),
        "svelte" => extra("Svelte", "svelte", "svelte"),
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

    #[test]
    fn extra_languages_follow_their_extension_setting() {
        for (file, highlighter, lsp) in [
            ("Main.kt", "kotlin", "kotlin"),
            ("build.gradle.kts", "kotlin", "kotlin"),
            ("lib/main.dart", "dart", "dart"),
            ("Program.cs", "csharp", "csharp"),
            ("index.php", "php", "php"),
            ("init.lua", "lua", "lua"),
            ("App.swift", "swift", "swift"),
            ("schema.graphql", "graphql", "graphql"),
            ("api.proto", "proto", "proto"),
            ("Makefile", "make", "makefile"),
            ("CMakeLists.txt", "cmake", "cmake"),
        ] {
            let info = language_for_path(file);
            assert_eq!((info.highlighter, info.lsp, info.extra), (highlighter, lsp, true), "{file}");
            assert_eq!(info.highlighter_with(true), highlighter, "{file}");
            assert_eq!(info.highlighter_with(false), "text", "{file} tanpa ekstensi");
        }
        // Bahasa bawaan tidak terpengaruh ekstensi.
        assert_eq!(language_for_path("main.rs").highlighter_with(false), "rust");
    }

    #[test]
    fn database_and_flutter_files() {
        assert_eq!(language_for_path("query.sql").highlighter, "sql");
        assert_eq!(language_for_path("keyspace.cql").highlighter, "sql");
        assert_eq!(language_for_path("playground.mongodb").highlighter, "javascript");
        assert_eq!(language_for_path("export.jsonl").highlighter, "json");
        assert_eq!(language_for_path("lib/l10n/app_id.arb").highlighter, "json");
        assert_eq!(language_for_path("pubspec.yaml").highlighter, "yaml");
    }
}

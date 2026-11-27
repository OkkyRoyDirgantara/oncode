//! The language of a file: display name, highlighter id, and LSP language id.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageInfo {
    /// Name for the status bar.
    pub name: &'static str,
    /// Language name for the highlighter (`EditorState::language`); `text` when there is no grammar.
    pub highlighter: &'static str,
    /// The LSP `languageId`; also the `lsp.servers` key in settings.
    pub lsp: &'static str,
    /// Its highlighting comes from the "Extra languages" extension; without that extension it is shown plain.
    pub extra: bool,
}

impl LanguageInfo {
    /// Language name for the highlighter, taking the "Extra languages" extension into account.
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

/// Languages whose highlighting is provided by the "Extra languages" extension.
const fn extra(name: &'static str, highlighter: &'static str, lsp: &'static str) -> LanguageInfo {
    LanguageInfo { name, highlighter, lsp, extra: true }
}

/// The language by file name or extension.
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
        // Flutter translation files.
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
        // NoSQL: Cassandra (CQL) and the MongoDB playground.
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

/// The comment tokens of a file's language, for Toggle Line/Block Comment.
pub fn comment_tokens(path: &str) -> super::lines::CommentTokens {
    use super::lines::CommentTokens;
    const C: CommentTokens = CommentTokens { line: Some("//"), block: Some(("/*", "*/")) };
    const HASH: CommentTokens = CommentTokens { line: Some("#"), block: None };
    const MARKUP: CommentTokens = CommentTokens { line: None, block: Some(("<!--", "-->")) };
    let language = language_for_path(path);
    match language.lsp {
        "rust" | "typescript" | "typescriptreact" | "javascript" | "javascriptreact" | "go" | "c" | "cpp" | "java" | "scss" | "kotlin" | "dart" | "swift" | "csharp" | "scala" | "proto" | "zig" => C,
        "php" => C,
        "css" => CommentTokens { line: None, block: Some(("/*", "*/")) },
        // Plain JSON has no comments; JSON with comments (`.jsonc`, `.json5`, and VS Code's settings files) does.
        "json" => {
            let lower = path.to_lowercase();
            let with_comments = lower.ends_with(".jsonc") || lower.ends_with(".json5") || ["tsconfig.json", "jsconfig.json", "settings.json", "keybindings.json", "launch.json", "tasks.json", "devcontainer.json"].iter().any(|name| lower.ends_with(&format!("/{name}")) || lower.ends_with(&format!("\\{name}")) || lower == *name);
            if with_comments {
                C
            } else {
                CommentTokens::default()
            }
        }
        // Python has no block comments: a `"""` string is a string (or a docstring), not a comment.
        "python" | "ruby" | "toml" | "yaml" | "shellscript" | "makefile" | "dockerfile" | "elixir" | "graphql" => HASH,
        "cmake" => CommentTokens { line: Some("#"), block: Some(("#[[", "]]")) },
        "sql" => CommentTokens { line: Some("--"), block: Some(("/*", "*/")) },
        "lua" => CommentTokens { line: Some("--"), block: Some(("--[[", "]]")) },
        "html" | "xml" | "markdown" | "vue" | "svelte" => MARKUP,
        // `.gitignore` and friends share the `plaintext` id but use `#` comments.
        "plaintext" if language.name == "Git Config" => HASH,
        _ => CommentTokens::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_tokens_by_language() {
        assert_eq!(comment_tokens("main.rs").line, Some("//"));
        assert_eq!(comment_tokens("app.py").line, Some("#"));
        assert_eq!(comment_tokens("app.py").block, None, "a triple-quoted string is not a comment");
        assert_eq!(comment_tokens("q.sql").line, Some("--"));
        assert_eq!(comment_tokens("index.html").line, None);
        assert_eq!(comment_tokens("index.html").block, Some(("<!--", "-->")));
        assert_eq!(comment_tokens("style.css").block, Some(("/*", "*/")));
        assert_eq!(comment_tokens("package.json"), Default::default());
        assert_eq!(comment_tokens("/p/tsconfig.json").line, Some("//"));
        assert_eq!(comment_tokens("conf.jsonc").line, Some("//"));
        assert_eq!(comment_tokens(".gitignore").line, Some("#"));
        assert_eq!(comment_tokens("notes.txt"), Default::default());
    }

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
            assert_eq!(info.highlighter_with(false), "text", "{file} without the extension");
        }
        // Built-in languages are not affected by the extension.
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

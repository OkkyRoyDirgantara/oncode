//! Data structures sent to the UI. All fields use camelCase on the TypeScript side.

use serde::{Deserialize, Serialize};

/// How to run a single language server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSpec {
    /// Command name (looked up on PATH) or path to the program.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables on top of the app's environment.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// An entry in the default server list for one language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDefault {
    /// LSP language id, e.g. `rust` or `typescriptreact`.
    pub language: String,
    pub spec: ServerSpec,
    /// One-line installation hint for the status bar.
    pub install_hint: String,
}

/// Final state of a server process, sent via `on_exit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerExitInfo {
    /// Exit code; `None` if the process was terminated by a signal.
    pub code: Option<i32>,
    /// At most the last 4 KiB of the server's stderr, to show when the server dies.
    pub stderr_tail: String,
}

const TABLE: &[(&[&str], &str, &[&str], &str)] = &[
    (
        &["rust"],
        "rust-analyzer",
        &[],
        "Install rust-analyzer with `rustup component add rust-analyzer`.",
    ),
    (
        &["typescript", "javascript", "typescriptreact", "javascriptreact"],
        "typescript-language-server",
        &["--stdio"],
        "Install with `npm install -g typescript-language-server typescript`.",
    ),
    (
        &["python"],
        "pyright-langserver",
        &["--stdio"],
        "Install Pyright with `npm install -g pyright` or `pip install pyright`.",
    ),
    (
        &["go"],
        "gopls",
        &[],
        "Install gopls with `go install golang.org/x/tools/gopls@latest`.",
    ),
    (
        &["c", "cpp"],
        "clangd",
        &[],
        "Install clangd through your system package manager, e.g. `apt install clangd` or `brew install llvm`.",
    ),
    (&["java"], "jdtls", &[], "Install the Eclipse JDT Language Server (jdtls) and put it on PATH, e.g. `brew install jdtls`."),
    (&["kotlin"], "kotlin-language-server", &[], "Install kotlin-language-server from github.com/fwcd/kotlin-language-server and put it on PATH."),
    (&["dart"], "dart", &["language-server", "--protocol=lsp"], "Install the Dart or Flutter SDK; `dart language-server` comes with it."),
    (&["php"], "intelephense", &["--stdio"], "Install Intelephense with `npm install -g intelephense`."),
    (&["csharp"], "csharp-ls", &[], "Install csharp-ls with `dotnet tool install --global csharp-ls`."),
    (&["lua"], "lua-language-server", &[], "Install lua-language-server through your package manager, e.g. `brew install lua-language-server`."),
    (&["swift"], "sourcekit-lsp", &[], "Install a Swift toolchain (Xcode on macOS); sourcekit-lsp comes with it."),
    (&["sql"], "sqls", &[], "Install sqls with `go install github.com/sqls-server/sqls@latest`."),
];

/// The default server list per language. User settings may override or extend it.
pub fn default_servers() -> Vec<ServerDefault> {
    TABLE
        .iter()
        .flat_map(|(languages, command, args, hint)| {
            languages.iter().map(move |language| ServerDefault {
                language: (*language).to_string(),
                spec: ServerSpec {
                    command: (*command).to_string(),
                    args: args.iter().map(|arg| (*arg).to_string()).collect(),
                    env: Vec::new(),
                },
                install_hint: (*hint).to_string(),
            })
        })
        .collect()
}

/// The installation hint for a default command, if known.
pub(crate) fn install_hint_for(command: &str) -> Option<&'static str> {
    TABLE.iter().find(|(_, known, _, _)| *known == command).map(|(_, _, _, hint)| *hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_line(language: &str) -> String {
        let servers = default_servers();
        let server = servers.iter().find(|s| s.language == language);
        let server = server.unwrap_or_else(|| panic!("language {language} is not in the default list"));
        std::iter::once(server.spec.command.clone()).chain(server.spec.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn default_table_matches_the_design() {
        assert_eq!(command_line("rust"), "rust-analyzer");
        for language in ["typescript", "javascript", "typescriptreact", "javascriptreact"] {
            assert_eq!(command_line(language), "typescript-language-server --stdio");
        }
        assert_eq!(command_line("python"), "pyright-langserver --stdio");
        assert_eq!(command_line("go"), "gopls");
        assert_eq!(command_line("c"), "clangd");
        assert_eq!(command_line("cpp"), "clangd");
        assert_eq!(command_line("dart"), "dart language-server --protocol=lsp");
        assert_eq!(command_line("kotlin"), "kotlin-language-server");
        assert_eq!(command_line("php"), "intelephense --stdio");
        assert_eq!(default_servers().len(), 17);
    }

    #[test]
    fn languages_are_unique_and_hints_are_one_line() {
        let servers = default_servers();
        let mut languages: Vec<&str> = servers.iter().map(|s| s.language.as_str()).collect();
        languages.sort_unstable();
        languages.dedup();
        assert_eq!(languages.len(), servers.len());
        for server in &servers {
            assert!(!server.install_hint.trim().is_empty());
            assert!(!server.install_hint.contains('\n'));
        }
        assert!(install_hint_for("gopls").is_some());
        assert!(install_hint_for("unknown-command").is_none());
    }

    #[test]
    fn serde_uses_camel_case_and_defaults() {
        let spec: ServerSpec = serde_json::from_str(r#"{"command":"gopls"}"#).expect("minimal spec");
        assert_eq!(spec, ServerSpec { command: "gopls".into(), args: vec![], env: vec![] });

        let default = &default_servers()[0];
        let json = serde_json::to_value(default).expect("serialization");
        assert_eq!(json["language"], "rust");
        assert_eq!(json["spec"]["command"], "rust-analyzer");
        assert!(json["installHint"].is_string());

        let exit = ServerExitInfo { code: Some(1), stderr_tail: "x".into() };
        let json = serde_json::to_value(&exit).expect("serialization");
        assert_eq!(json["code"], 1);
        assert_eq!(json["stderrTail"], "x");
    }
}

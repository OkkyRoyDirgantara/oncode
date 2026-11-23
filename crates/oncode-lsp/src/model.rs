//! Struktur data yang dikirim ke UI. Semua field memakai camelCase di sisi TypeScript.

use serde::{Deserialize, Serialize};

/// Cara menjalankan satu language server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSpec {
    /// Nama perintah (dicari di PATH) atau path ke program.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Variabel lingkungan tambahan di atas lingkungan aplikasi.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// Entri daftar server bawaan untuk satu bahasa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDefault {
    /// Id bahasa LSP, mis. `rust` atau `typescriptreact`.
    pub language: String,
    pub spec: ServerSpec,
    /// Petunjuk pemasangan satu baris untuk status bar.
    pub install_hint: String,
}

/// Keadaan akhir proses server, dikirim lewat `on_exit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerExitInfo {
    /// Kode keluar; `None` bila proses dihentikan oleh sinyal.
    pub code: Option<i32>,
    /// Paling banyak 4 KiB terakhir dari stderr server, untuk ditampilkan saat server mati.
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
];

/// Daftar server bawaan per bahasa. Pengaturan pengguna boleh menimpa atau menambahnya.
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

/// Petunjuk pemasangan untuk perintah bawaan, bila dikenal.
pub(crate) fn install_hint_for(command: &str) -> Option<&'static str> {
    TABLE.iter().find(|(_, known, _, _)| *known == command).map(|(_, _, _, hint)| *hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_line(language: &str) -> String {
        let servers = default_servers();
        let server = servers.iter().find(|s| s.language == language);
        let server = server.unwrap_or_else(|| panic!("bahasa {language} tidak ada di daftar bawaan"));
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
        assert_eq!(default_servers().len(), 9);
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
        assert!(install_hint_for("tidak-dikenal").is_none());
    }

    #[test]
    fn serde_uses_camel_case_and_defaults() {
        let spec: ServerSpec = serde_json::from_str(r#"{"command":"gopls"}"#).expect("spec minimal");
        assert_eq!(spec, ServerSpec { command: "gopls".into(), args: vec![], env: vec![] });

        let default = &default_servers()[0];
        let json = serde_json::to_value(default).expect("serialisasi");
        assert_eq!(json["language"], "rust");
        assert_eq!(json["spec"]["command"], "rust-analyzer");
        assert!(json["installHint"].is_string());

        let exit = ServerExitInfo { code: Some(1), stderr_tail: "x".into() };
        let json = serde_json::to_value(&exit).expect("serialisasi");
        assert_eq!(json["code"], 1);
        assert_eq!(json["stderrTail"], "x");
    }
}

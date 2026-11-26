//! The Formatters extension: formats file contents with the language's own formatter (Prettier, rustfmt,
//! gofmt, ruff, dart format, …). The content is sent via stdin, so unsaved changes
//! are formatted too. Only in trusted folders: a project's Prettier is project code.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncWriteExt as _;

use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Time limit for a single format run.
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Formatted {
    pub text: String,
    /// Name of the formatter used, e.g. "Prettier".
    pub formatter: String,
}

/// Formats `text` (the contents of file `path`, language `language` per the LSP id). `None` if the
/// Formatters extension is off or no formatter is installed for that language; the caller may try
/// the language server instead.
pub async fn format_text(core: &Shared, window: &str, path: String, language: String, text: String) -> AppResult<Option<Formatted>> {
    if !core.workspace_trusted(window) {
        return Err(AppError::untrusted());
    }
    if !core.effective_settings(window).extensions.formatters {
        return Ok(None);
    }
    let root = core.workspace_root(window)?;
    let Some(formatter) = oncode_format::resolve(&language, Path::new(&path), &root, oncode_lsp::find_on_path) else { return Ok(None) };
    let name = formatter.name;
    let failed = |detail: String| AppError::new("formatFailed", format!("{name} could not format this file: {detail}"));

    let mut child = super::hidden_command(&formatter.program)
        .args(&formatter.args)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| failed(error.to_string()))?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    let input = text.clone();
    // Written concurrently with reading the output so full pipes do not wait on each other.
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.map_err(|error| failed(error.to_string()))?,
        Err(_) => return Err(failed(format!("it took longer than {} seconds", TIMEOUT.as_secs()))),
    };
    let _ = writer.await;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: Vec<&str> = stderr.lines().map(str::trim).filter(|line| !line.is_empty()).take(3).collect();
        return Err(failed(if detail.is_empty() { format!("it exited with {}", output.status) } else { detail.join(" ") }));
    }
    let mut formatted = String::from_utf8(output.stdout).map_err(|_| failed("its output is not valid UTF-8".into()))?;
    // The editor uses LF; the original file's line endings are restored on save. Formatters that
    // write CRLF on Windows (rustfmt with `newline_style = "Native"`) are normalized.
    if !text.contains('\r') {
        formatted = formatted.replace("\r\n", "\n");
    }
    // Empty output for non-empty content means the formatter did not write to stdout.
    if formatted.trim().is_empty() && !text.trim().is_empty() {
        return Err(failed("it returned nothing".into()));
    }
    Ok(Some(Formatted { text: formatted, formatter: name.to_string() }))
}

/// Installation hint for a language's formatter, if known.
pub fn formatter_install_hint(language: &str) -> Option<&'static str> {
    oncode_format::install_hint(language)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::app::workspace_open;
    use crate::testing::TestCore;

    #[tokio::test]
    async fn untrusted_folders_and_disabled_extension_do_not_format() {
        let t = TestCore::new();
        let project = t.project("project");
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        let path = project.join("a.rs").to_string_lossy().into_owned();
        let error = format_text(&t.core, "w1", path.clone(), "rust".into(), "fn  main(){}".into()).await.unwrap_err();
        assert_eq!(error.code, "untrusted");

        let project = t.trusted_repo("trusted");
        workspace_open(&t.core, "w2", project.to_string_lossy().into_owned()).await.unwrap();
        let mut settings = t.core.user_settings();
        settings.extensions.formatters = false;
        t.core.set_user_settings(settings).unwrap();
        assert_eq!(format_text(&t.core, "w2", path, "rust".into(), "fn  main(){}".into()).await.unwrap(), None);
        // A language without a formatter.
        let mut settings = t.core.user_settings();
        settings.extensions.formatters = true;
        t.core.set_user_settings(settings).unwrap();
        assert_eq!(format_text(&t.core, "w2", "x.txt".into(), "plaintext".into(), "x".into()).await.unwrap(), None);
    }

    /// A real format run with rustfmt if installed on the test machine (skipped if not).
    #[tokio::test]
    async fn rustfmt_formats_text_from_stdin() {
        if oncode_lsp::find_on_path("rustfmt").is_none() {
            return;
        }
        let t = TestCore::new();
        let project = t.trusted_repo("project");
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        let path = project.join("main.rs").to_string_lossy().into_owned();
        let formatted = format_text(&t.core, "w1", path.clone(), "rust".into(), "fn  main( ){let x=1;}".into()).await.unwrap().expect("rustfmt is installed");
        assert_eq!(formatted.formatter, "rustfmt");
        assert_eq!(formatted.text, "fn main() {\n    let x = 1;\n}\n");

        let error = format_text(&t.core, "w1", path, "rust".into(), "fn main( {".into()).await.unwrap_err();
        assert_eq!(error.code, "formatFailed");
        assert!(error.message.starts_with("rustfmt could not format this file"), "{}", error.message);
    }
}

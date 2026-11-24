//! Menulis pesan commit lewat Claude Code CLI (`claude -p`) alih-alih API: memakai login Claude
//! Code pengguna (termasuk langganan claude.ai), jadi tidak perlu API key.
//!
//! Murni: argumen dan pengurai keluaran `--output-format stream-json`. Prosesnya dijalankan
//! pemanggil (oncode-core).

use serde_json::Value;

use crate::{clean_message, user_message, AiError, AiErrorCode, CommitPrompt, SYSTEM_PROMPT};

/// System prompt yang ditulis ke berkas untuk `--system-prompt-file`.
pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
}

/// Isi stdin: bahan pesan commit (sama dengan pesan pengguna versi API).
pub fn input(prompt: &CommitPrompt) -> String {
    user_message(prompt)
}

/// Argumen `claude`: mode print tanpa tool apa pun, tanpa settings/hook/MCP pengguna maupun
/// proyek, tanpa menyimpan sesi, dan keluaran stream-json agar teks bisa dialirkan.
pub fn args(system_prompt_file: &str, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--restricted",
        "--strict-mcp-config",
        "--tools",
        "",
        "--no-session-persistence",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--system-prompt-file",
        system_prompt_file,
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        args.push("--model".into());
        args.push(model.into());
    }
    args
}

/// Pengurai keluaran `claude -p --output-format stream-json`, baris demi baris.
#[derive(Debug, Default)]
pub struct ClaudeCodeStream {
    text: String,
    result: Option<Result<String, AiError>>,
}

impl ClaudeCodeStream {
    /// Satu baris NDJSON. Mengembalikan potongan teks baru bila ada; baris lain diabaikan.
    pub fn push_line(&mut self, line: &str) -> Option<String> {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        match value["type"].as_str()? {
            "stream_event" => {
                let event = &value["event"];
                if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                    let piece = event["delta"]["text"].as_str().unwrap_or_default();
                    if !piece.is_empty() {
                        self.text.push_str(piece);
                        return Some(piece.to_string());
                    }
                }
                None
            }
            "result" => {
                self.result = Some(result_of(&value));
                None
            }
            _ => None,
        }
    }

    /// Hasil akhir setelah proses selesai. `exit_ok`: kode keluar 0; `stderr` untuk pesan galat.
    pub fn finish(self, exit_ok: bool, stderr: &str) -> Result<String, AiError> {
        if let Some(result) = self.result {
            return result;
        }
        if !exit_ok {
            let detail: Vec<&str> = stderr.lines().map(str::trim).filter(|line| !line.is_empty()).take(3).collect();
            let detail = if detail.is_empty() { "it exited without an answer".to_string() } else { detail.join(" ") };
            return Err(AiError::new(AiErrorCode::ClaudeCodeFailed, format!("Claude Code could not write the message: {detail}")));
        }
        let message = clean_message(&self.text);
        if message.is_empty() {
            return Err(AiError::new(AiErrorCode::InvalidResponse, "Claude Code returned an empty message. Try again."));
        }
        Ok(message)
    }
}

fn result_of(value: &Value) -> Result<String, AiError> {
    let text = value["result"].as_str().unwrap_or_default();
    if value["stop_reason"] == "refusal" {
        return Err(AiError::new(AiErrorCode::Refused, "Claude declined to write a message for these changes. Write it yourself or try again."));
    }
    if value["is_error"].as_bool().unwrap_or(false) || value["subtype"].as_str().is_some_and(|subtype| subtype != "success") {
        let detail = if text.trim().is_empty() { value["subtype"].as_str().unwrap_or("unknown error").to_string() } else { text.trim().to_string() };
        return Err(AiError::new(AiErrorCode::ClaudeCodeFailed, format!("Claude Code could not write the message: {detail}")));
    }
    let message = clean_message(text);
    if message.is_empty() {
        return Err(AiError::new(AiErrorCode::InvalidResponse, "Claude Code returned an empty message. Try again."));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn delta(text: &str) -> String {
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, "session_id": "s"}).to_string()
    }

    fn result(extra: Value) -> String {
        let mut value = json!({"type": "result", "subtype": "success", "is_error": false, "result": "fix: perbaiki login", "stop_reason": "end_turn"});
        for (key, field) in extra.as_object().unwrap() {
            value[key] = field.clone();
        }
        value.to_string()
    }

    #[test]
    fn args_run_without_tools_settings_or_saved_sessions() {
        let args = args("C:\\temp\\prompt.txt", Some("sonnet"));
        for flag in ["-p", "--restricted", "--strict-mcp-config", "--no-session-persistence", "--include-partial-messages"] {
            assert!(args.iter().any(|arg| arg == flag), "{flag}");
        }
        let tools = args.iter().position(|arg| arg == "--tools").unwrap();
        assert_eq!(args[tools + 1], "", "tanpa tool apa pun");
        let format = args.iter().position(|arg| arg == "--output-format").unwrap();
        assert_eq!(args[format + 1], "stream-json");
        assert_eq!(args[args.len() - 2..], ["--model", "sonnet"]);
        assert!(!self::args("p.txt", Some("  ")).iter().any(|arg| arg == "--model"));
    }

    #[test]
    fn streams_text_and_takes_the_final_result() {
        let mut stream = ClaudeCodeStream::default();
        let lines = [
            json!({"type": "system", "subtype": "init", "tools": []}).to_string(),
            json!({"type": "stream_event", "event": {"type": "message_start", "message": {}}}).to_string(),
            delta("fix: perbaiki "),
            delta("login"),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "fix: perbaiki login"}]}}).to_string(),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}}).to_string(),
            result(json!({})),
        ];
        let pieces: Vec<String> = lines.iter().filter_map(|line| stream.push_line(line)).collect();
        assert_eq!(pieces, ["fix: perbaiki ", "login"]);
        assert_eq!(stream.finish(true, "").unwrap(), "fix: perbaiki login");
    }

    #[test]
    fn errors_from_the_cli_are_reported() {
        // Belum login, kuota habis, dsb.: `result` dengan `is_error`.
        let mut stream = ClaudeCodeStream::default();
        stream.push_line(&result(json!({"is_error": true, "subtype": "success", "result": "Invalid API key · Please run /login"})));
        let error = stream.finish(false, "").unwrap_err();
        assert_eq!(error.code, AiErrorCode::ClaudeCodeFailed);
        assert!(error.message.contains("Please run /login"), "{}", error.message);

        let mut stream = ClaudeCodeStream::default();
        stream.push_line(&result(json!({"stop_reason": "refusal", "result": ""})));
        assert_eq!(stream.finish(true, "").unwrap_err().code, AiErrorCode::Refused);

        // Proses gagal tanpa baris `result`: stderr dipakai.
        let stream = ClaudeCodeStream::default();
        let error = stream.finish(false, "error: unknown option '--restricted'\n").unwrap_err();
        assert!(error.message.contains("unknown option"), "{}", error.message);

        // Baris yang bukan JSON diabaikan.
        let mut stream = ClaudeCodeStream::default();
        assert_eq!(stream.push_line("bukan json"), None);
        assert_eq!(stream.finish(true, "").unwrap_err().code, AiErrorCode::InvalidResponse);
    }

    #[test]
    fn input_and_system_prompt_match_the_api_version() {
        let prompt = CommitPrompt { recent_subjects: vec!["feat: a".into()], stat: "1 file".into(), patch: "+x".into(), truncated: false, draft: String::new() };
        assert!(input(&prompt).contains("- feat: a") && input(&prompt).contains("+x"));
        assert!(system_prompt().contains("commit message only"));
    }
}

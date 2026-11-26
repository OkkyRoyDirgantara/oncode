//! Development helper (debug builds only): runs a series of steps after the window
//! opens, to check the UI through screenshots without manual input.
//!
//! `ONCODE_DEV_SCRIPT` holds a JSON list, e.g.
//! `[{"wait":800},{"open":"/project/src/main.rs"},{"command":"workbench.togglePanel"}]`.

use std::time::Duration;

use gpui_kit::App;
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::Workbench;
use serde_json::Value;

pub fn run_script(cx: &mut App) {
    let Ok(script) = std::env::var("ONCODE_DEV_SCRIPT") else { return };
    let steps: Vec<Value> = match serde_json::from_str(&script) {
        Ok(steps) => steps,
        Err(error) => {
            eprintln!("oncode: invalid ONCODE_DEV_SCRIPT: {error}");
            return;
        }
    };
    cx.spawn(async move |cx| {
        for step in steps {
            if let Some(ms) = step.get("wait").and_then(Value::as_u64) {
                cx.background_executor().timer(Duration::from_millis(ms)).await;
                continue;
            }
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let _ = cx.update(|cx| {
                let Some(handle) = cx.windows().into_iter().next() else { return };
                let _ = handle.update(cx, |_, window, cx| {
                    if let Some(path) = step.get("open").and_then(Value::as_str) {
                        let line = step.get("line").and_then(Value::as_u64).map(|l| l as u32);
                        if let Some(workbench) = Workbench::for_window(window, cx) {
                            workbench.update(cx, |workbench, cx| {
                                let mut options = OpenFile::new(path);
                                if let Some(line) = line {
                                    options = options.at(line, 0, None);
                                }
                                if step.get("side").and_then(Value::as_bool) == Some(true) {
                                    options = options.to_side();
                                }
                                open_file(workbench, options, window, cx);
                            });
                        }
                    } else if let (Some(root), Some(path)) = (step.get("diff_root").and_then(Value::as_str), step.get("diff_path").and_then(Value::as_str)) {
                        if let Some(workbench) = Workbench::for_window(window, cx) {
                            let request = oncode_ui_diff::DiffRequest {
                                root: root.to_string(),
                                path: path.to_string(),
                                orig_path: None,
                                source: oncode_git::DiffSource::Worktree,
                                preview: false,
                            };
                            workbench.update(cx, |workbench, cx| oncode_ui_diff::open_diff(workbench, request, window, cx));
                        }
                    } else if let Some(id) = step.get("command").and_then(Value::as_str) {
                        let args = step.get("args").cloned().unwrap_or(Value::Null);
                        if !oncode_ui::execute_now(id, &args, window, cx) {
                            eprintln!("oncode: command did not run: {id}");
                        }
                    } else if let Some(text) = step.get("type").and_then(Value::as_str) {
                        // Type into the element that is currently focused.
                        for ch in text.chars() {
                            let keystroke = gpui_kit::Keystroke { modifiers: Default::default(), key: ch.to_string(), key_char: Some(ch.to_string()) };
                            window.dispatch_keystroke(keystroke, cx);
                        }
                    } else if let Some(key) = step.get("key").and_then(Value::as_str) {
                        let key: &'static str = Box::leak(key.to_string().into_boxed_str());
                        if let Ok(keystroke) = gpui_kit::Keystroke::parse(key) {
                            window.dispatch_keystroke(keystroke, cx);
                        }
                    }
                });
            });
        }
    })
    .detach();
}

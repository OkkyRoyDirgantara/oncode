//! The "Go to Definition without a language server" extension: finds a symbol's definition across
//! the whole project (see `oncode_workspace::symbols`). Used by Ctrl+click and F12 when no
//! language server answers.

use std::path::Path;

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Maximum number of candidates.
const LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolLocation {
    /// Absolute path.
    pub path: String,
    /// Starts from 1.
    pub line: u32,
    /// UTF-16 units, starts from 0.
    pub column: u32,
    pub length: u32,
    pub preview: String,
}

/// Definition candidates for `symbol` in this window's folder, the most convincing first. Empty if
/// the extension is turned off. `current` is the path of the file currently open (given priority).
pub async fn symbol_definitions(core: &Shared, window: &str, symbol: String, current: Option<String>) -> AppResult<Vec<SymbolLocation>> {
    if !core.effective_settings(window).extensions.symbol_search {
        return Ok(Vec::new());
    }
    let root = core.workspace_root(window)?;
    let current = current.and_then(|path| Path::new(&path).strip_prefix(&root).ok().map(|rel| rel.to_string_lossy().replace('\\', "/")));
    let search_root = root.clone();
    let found = core
        .runtime()
        .spawn_blocking(move || oncode_workspace::symbols::find_definitions(&search_root, &symbol, current.as_deref(), LIMIT))
        .await
        .map_err(|e| AppError::new("io", e.to_string()))??;
    Ok(found
        .into_iter()
        .map(|d| SymbolLocation { path: root.join(&d.path).to_string_lossy().into_owned(), line: d.line, column: d.column, length: d.length, preview: d.preview })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::app::workspace_open;
    use crate::testing::TestCore;

    #[tokio::test]
    async fn finds_definitions_in_the_open_folder_and_respects_the_setting() {
        let t = TestCore::new();
        let project = t.project("project");
        std::fs::write(project.join("util.py"), "def normalize(text):\n    return text\n").unwrap();
        std::fs::write(project.join("main.py"), "from util import normalize\nnormalize(' a ')\n").unwrap();
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let current = project.join("main.py").to_string_lossy().into_owned();
        let found = symbol_definitions(&t.core, "w1", "normalize".into(), Some(current.clone())).await.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(Path::new(&found[0].path), project.join("util.py"));
        assert_eq!((found[0].line, found[0].column, found[0].length), (1, 4, 9));

        let mut settings = t.core.user_settings();
        settings.extensions.symbol_search = false;
        t.core.set_user_settings(settings).unwrap();
        assert!(symbol_definitions(&t.core, "w1", "normalize".into(), Some(current)).await.unwrap().is_empty());
    }
}

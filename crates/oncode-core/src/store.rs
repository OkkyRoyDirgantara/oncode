//! Application data that persists across sessions: recent projects, trusted folders,
//! accounts (without tokens), and per-project sessions. Stored as `state.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use oncode_forge::Account;
use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 20;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    recent: Vec<String>,
    trusted: Vec<String>,
    accounts: Vec<Account>,
    /// Sessions per project root, as opaque JSON owned by the frontend.
    sessions: BTreeMap<String, String>,
}

pub struct Store {
    path: PathBuf,
    data: Mutex<Persisted>,
    /// Accounts whose token exists only in memory (keychain unavailable); not written to disk
    /// so they do not show up as token-less accounts after the app is reopened.
    session_accounts: Mutex<Vec<Account>>,
}

impl Store {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path, data: Mutex::new(data), session_accounts: Mutex::new(Vec::new()) }
    }

    fn update<T>(&self, change: impl FnOnce(&mut Persisted) -> T) -> T {
        let mut data = self.data.lock().unwrap();
        let result = change(&mut data);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&*data) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, &self.path);
            }
        }
        result
    }

    pub fn recent(&self) -> Vec<String> {
        self.data.lock().unwrap().recent.clone()
    }

    pub fn add_recent(&self, root: &str) {
        self.update(|d| {
            d.recent.retain(|r| r != root);
            d.recent.insert(0, root.to_string());
            d.recent.truncate(MAX_RECENT);
        });
    }

    pub fn remove_recent(&self, root: &str) {
        self.update(|d| d.recent.retain(|r| r != root));
    }

    /// A folder is trusted if it or one of its parents is in the list.
    pub fn is_trusted(&self, path: &Path) -> bool {
        self.data.lock().unwrap().trusted.iter().any(|t| path.starts_with(t))
    }

    pub fn set_trusted(&self, root: &str, trusted: bool) {
        self.update(|d| {
            d.trusted.retain(|t| t != root);
            if trusted {
                d.trusted.push(root.to_string());
            }
        });
    }

    pub fn accounts(&self) -> Vec<Account> {
        let mut accounts = self.data.lock().unwrap().accounts.clone();
        accounts.extend(self.session_accounts.lock().unwrap().iter().cloned());
        accounts
    }

    /// Records a logged-in account. `persist: false` for accounts whose token only lasts for this session.
    pub fn upsert_account(&self, account: Account, persist: bool) {
        self.session_accounts.lock().unwrap().retain(|a| a.id != account.id);
        if persist {
            self.update(|d| {
                d.accounts.retain(|a| a.id != account.id);
                d.accounts.push(account);
            });
        } else {
            self.update(|d| d.accounts.retain(|a| a.id != account.id));
            self.session_accounts.lock().unwrap().push(account);
        }
    }

    pub fn remove_account(&self, id: &str) {
        self.session_accounts.lock().unwrap().retain(|a| a.id != id);
        self.update(|d| d.accounts.retain(|a| a.id != id));
    }

    pub fn session(&self, root: &str) -> Option<String> {
        self.data.lock().unwrap().sessions.get(root).cloned()
    }

    pub fn set_session(&self, root: &str, json: String) {
        self.update(|d| {
            d.sessions.insert(root.to_string(), json);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_is_deduplicated_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = Store::load(path.clone());
        store.add_recent("/a");
        store.add_recent("/b");
        store.add_recent("/a");
        assert_eq!(store.recent(), vec!["/a", "/b"]);
        assert_eq!(Store::load(path).recent(), vec!["/a", "/b"]);
    }

    #[test]
    fn trust_covers_subfolders_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(dir.path().join("state.json"));
        store.set_trusted("/project/app", true);
        assert!(store.is_trusted(Path::new("/project/app")));
        assert!(store.is_trusted(Path::new("/project/app/vendor/lib")));
        assert!(!store.is_trusted(Path::new("/project/app-other")));
        assert!(!store.is_trusted(Path::new("/project")));
        store.set_trusted("/project/app", false);
        assert!(!store.is_trusted(Path::new("/project/app")));
    }

    #[test]
    fn session_only_accounts_are_not_written_to_disk() {
        use oncode_forge::ForgeKind;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let account = |name: &str| Account {
            id: format!("github:github.com:{name}"),
            kind: ForgeKind::GitHub,
            host: "https://github.com".into(),
            username: name.into(),
            display_name: name.into(),
            avatar_url: None,
        };
        let store = Store::load(path.clone());
        store.upsert_account(account("persistent"), true);
        store.upsert_account(account("session"), false);
        assert_eq!(store.accounts().iter().map(|a| a.username.as_str()).collect::<Vec<_>>(), vec!["persistent", "session"]);
        // After the app is reopened only the accounts whose token is stored in the keychain remain.
        assert_eq!(Store::load(path.clone()).accounts().iter().map(|a| a.username.as_str()).collect::<Vec<_>>(), vec!["persistent"]);

        // An account that was originally persisted and then logs in again without a keychain becomes a session account.
        store.upsert_account(account("persistent"), false);
        assert!(Store::load(path.clone()).accounts().is_empty());
        assert_eq!(store.accounts().len(), 2);
        store.remove_account("github:github.com:session");
        assert_eq!(store.accounts().len(), 1);
    }

    #[test]
    fn sessions_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(dir.path().join("state.json"));
        assert_eq!(store.session("/a"), None);
        store.set_session("/a", "{\"tabs\":[]}".into());
        assert_eq!(store.session("/a").as_deref(), Some("{\"tabs\":[]}"));
    }
}

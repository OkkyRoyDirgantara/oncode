//! Token storage: the OS keychain for real use, memory for tests and as a fallback.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard};

use crate::auth::TokenSet;
use crate::error::{ForgeError, Result};

/// Service name in the OS keychain.
const SERVICE: &str = "OnCode";

/// Where tokens are stored per account. `account_id` is [`Account::id`](crate::Account).
pub trait TokenStore: Send + Sync {
    /// Saves a token, overwriting the old one if present.
    fn save(&self, account_id: &str, tokens: &TokenSet) -> Result<()>;
    /// Loads a token; `None` if this account has no stored token yet.
    fn load(&self, account_id: &str) -> Result<Option<TokenSet>>;
    /// Deletes a token; not an error if there is none.
    fn delete(&self, account_id: &str) -> Result<()>;
}

/// Storage in the OS keychain via `keyring`: Keychain on macOS, Credential Manager on
/// Windows, Secret Service (GNOME Keyring, KWallet) on Linux. The service name is `OnCode`,
/// the entry name is the account id, and the value is the JSON of the [`TokenSet`].
///
/// All of its methods block (on Linux they talk over D-Bus and may wait for the user to
/// unlock the keyring), so from an async context call them via `tokio::task::spawn_blocking`.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeychainStore;

impl KeychainStore {
    pub fn new() -> Self {
        Self
    }

    /// `true` if the OS keychain is usable. If `false`, the caller can switch to
    /// [`MemoryStore`] (the token is lost when the app closes).
    pub fn is_available() -> bool {
        keyring::Entry::store_status().is_ok()
    }

    fn entry(account_id: &str) -> Result<keyring::Entry> {
        if account_id.trim().is_empty() {
            return Err(ForgeError::invalid("Account ID must not be empty"));
        }
        keyring::Entry::new(SERVICE, account_id).map_err(keychain_error)
    }
}

impl TokenStore for KeychainStore {
    fn save(&self, account_id: &str, tokens: &TokenSet) -> Result<()> {
        let entry = Self::entry(account_id)?;
        // The serialization error is not passed along as-is so the token contents are not printed with it.
        let json = serde_json::to_string(tokens)
            .map_err(|_| ForgeError::keychain("The token could not be prepared for storage"))?;
        entry.set_password(&json).map_err(keychain_error)
    }

    fn load(&self, account_id: &str) -> Result<Option<TokenSet>> {
        let entry = Self::entry(account_id)?;
        let json = match entry.get_password() {
            Ok(json) => json,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(err) => return Err(keychain_error(err)),
        };
        serde_json::from_str::<TokenSet>(&json)
            .map(Some)
            .map_err(|_| ForgeError::keychain("The token stored in the keychain is corrupted. Please log in again."))
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        let entry = Self::entry(account_id)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(keychain_error(err)),
        }
    }
}

/// Maps a `keyring` error to a safe message. Variants that carry raw data
/// (`BadEncoding`, `BadDataFormat`) never have their contents printed.
fn keychain_error(err: keyring::Error) -> ForgeError {
    let message = match &err {
        keyring::Error::NoStorageAccess(cause) => {
            format!("The system keychain could not be accessed (it may be locked): {cause}")
        }
        keyring::Error::PlatformFailure(cause) => format!("The system keychain failed: {cause}"),
        keyring::Error::NoDefaultStore => "The system keychain is not available on this device".to_string(),
        keyring::Error::NoEntry => "Token not found in the keychain".to_string(),
        keyring::Error::BadEncoding(_) | keyring::Error::BadDataFormat(_, _) => {
            "The token stored in the keychain is corrupted. Please log in again.".to_string()
        }
        keyring::Error::TooLong(field, limit) => {
            format!("The value of '{field}' exceeds the system keychain limit ({limit} characters)")
        }
        keyring::Error::Invalid(field, reason) => format!("The value of '{field}' is not valid for the keychain: {reason}"),
        keyring::Error::Ambiguous(entries) => {
            format!("There are {} matching keychain entries for this account; remove the duplicates", entries.len())
        }
        keyring::Error::NotSupportedByStore(reason) => format!("The system keychain does not support this operation: {reason}"),
        _ => "The system keychain returned an unknown error".to_string(),
    };
    ForgeError::keychain(message)
}

/// In-memory storage, for tests and as a fallback when the keychain is unavailable.
/// Its contents are lost when the process ends.
#[derive(Default)]
pub struct MemoryStore {
    tokens: Mutex<HashMap<String, TokenSet>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, TokenSet>> {
        // A poisoned mutex is still used: it only holds a token map, with no other invariants.
        self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore").field("accounts", &self.lock().len()).finish()
    }
}

impl TokenStore for MemoryStore {
    fn save(&self, account_id: &str, tokens: &TokenSet) -> Result<()> {
        if account_id.trim().is_empty() {
            return Err(ForgeError::invalid("Account ID must not be empty"));
        }
        self.lock().insert(account_id.to_string(), tokens.clone());
        Ok(())
    }

    fn load(&self, account_id: &str) -> Result<Option<TokenSet>> {
        Ok(self.lock().get(account_id).cloned())
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        self.lock().remove(account_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ForgeErrorCode;

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::new();
        let id = "github:github.com:octocat";
        assert_eq!(store.load(id).expect("load"), None);

        let tokens = TokenSet { access_token: "gho_1".into(), refresh_token: Some("r1".into()), expires_at: Some(10.0) };
        store.save(id, &tokens).expect("save");
        assert_eq!(store.load(id).expect("load"), Some(tokens));

        // Saving again overwrites the old one; other accounts are unaffected.
        let newer = TokenSet::new("gho_2");
        store.save(id, &newer).expect("save");
        store.save("gitlab:gitlab.com:alice", &TokenSet::new("glpat")).expect("save");
        assert_eq!(store.load(id).expect("load"), Some(newer));

        store.delete(id).expect("delete");
        assert_eq!(store.load(id).expect("load"), None);
        assert_eq!(store.load("gitlab:gitlab.com:alice").expect("load"), Some(TokenSet::new("glpat")));
        // Deleting a nonexistent entry is not an error.
        store.delete(id).expect("delete");
    }

    #[test]
    fn memory_store_rejects_empty_id_and_hides_tokens() {
        let store = MemoryStore::new();
        let err = store.save(" ", &TokenSet::new("secret")).expect_err("must be rejected");
        assert_eq!(err.code, ForgeErrorCode::InvalidInput);
        store.save("a", &TokenSet::new("secret")).expect("save");
        assert!(!format!("{store:?}").contains("secret"));
    }

    #[test]
    fn stores_are_usable_as_trait_objects() {
        // Compile-only test: the real keychain is not touched.
        fn assert_store<T: TokenStore + 'static>() {}
        assert_store::<KeychainStore>();
        assert_store::<MemoryStore>();
        let stores: Vec<Box<dyn TokenStore>> = vec![Box::new(MemoryStore::new())];
        assert_eq!(stores.len(), 1);
        let _unused: fn() -> KeychainStore = KeychainStore::new;
    }

    #[test]
    fn keychain_errors_never_carry_secret_bytes() {
        let err = keychain_error(keyring::Error::BadEncoding(b"token_secret".to_vec()));
        assert_eq!(err.code, ForgeErrorCode::Keychain);
        assert!(!err.message.contains("token_secret"));
        assert!(!err.message.contains("116"), "raw bytes must not be printed: {}", err.message);
    }
}

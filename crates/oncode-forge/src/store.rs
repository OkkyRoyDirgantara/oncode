//! Penyimpanan token: keychain OS untuk pemakaian nyata, memori untuk tes dan cadangan.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard};

use crate::auth::TokenSet;
use crate::error::{ForgeError, Result};

/// Nama layanan di keychain OS.
const SERVICE: &str = "OnCode";

/// Tempat menyimpan token per akun. `account_id` adalah [`Account::id`](crate::Account).
pub trait TokenStore: Send + Sync {
    /// Menyimpan token, menimpa yang lama bila ada.
    fn save(&self, account_id: &str, tokens: &TokenSet) -> Result<()>;
    /// Memuat token; `None` bila akun ini belum punya token tersimpan.
    fn load(&self, account_id: &str) -> Result<Option<TokenSet>>;
    /// Menghapus token; tidak dianggap galat bila memang tidak ada.
    fn delete(&self, account_id: &str) -> Result<()>;
}

/// Penyimpanan di keychain OS lewat `keyring`: Keychain di macOS, Credential Manager di
/// Windows, Secret Service (GNOME Keyring, KWallet) di Linux. Nama layanannya `OnCode`,
/// nama entrinya id akun, dan nilainya JSON dari [`TokenSet`].
///
/// Semua metodenya memblokir (di Linux berbicara lewat D-Bus dan bisa menunggu pengguna
/// membuka keyring), jadi dari konteks async panggil lewat `tokio::task::spawn_blocking`.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeychainStore;

impl KeychainStore {
    pub fn new() -> Self {
        Self
    }

    /// `true` bila keychain OS bisa dipakai. Bila `false`, pemanggil dapat beralih ke
    /// [`MemoryStore`] (token hilang saat aplikasi ditutup).
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
        // Galat serialisasi tidak diteruskan apa adanya agar isi token tidak ikut tercetak.
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

/// Memetakan galat `keyring` ke pesan yang aman. Varian yang membawa data mentah
/// (`BadEncoding`, `BadDataFormat`) tidak pernah dicetak isinya.
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

/// Penyimpanan di memori, untuk tes dan sebagai cadangan saat keychain tidak tersedia.
/// Isinya hilang saat proses berakhir.
#[derive(Default)]
pub struct MemoryStore {
    tokens: Mutex<HashMap<String, TokenSet>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, TokenSet>> {
        // Mutex yang teracuni tetap dipakai: isinya hanya peta token, tanpa invarian lain.
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

        // Menyimpan lagi menimpa yang lama; akun lain tidak terpengaruh.
        let newer = TokenSet::new("gho_2");
        store.save(id, &newer).expect("save");
        store.save("gitlab:gitlab.com:budi", &TokenSet::new("glpat")).expect("save");
        assert_eq!(store.load(id).expect("load"), Some(newer));

        store.delete(id).expect("delete");
        assert_eq!(store.load(id).expect("load"), None);
        assert_eq!(store.load("gitlab:gitlab.com:budi").expect("load"), Some(TokenSet::new("glpat")));
        // Menghapus yang tidak ada bukan galat.
        store.delete(id).expect("delete");
    }

    #[test]
    fn memory_store_rejects_empty_id_and_hides_tokens() {
        let store = MemoryStore::new();
        let err = store.save(" ", &TokenSet::new("rahasia")).expect_err("harus ditolak");
        assert_eq!(err.code, ForgeErrorCode::InvalidInput);
        store.save("a", &TokenSet::new("rahasia")).expect("save");
        assert!(!format!("{store:?}").contains("rahasia"));
    }

    #[test]
    fn stores_are_usable_as_trait_objects() {
        // Hanya uji kompilasi: keychain asli tidak disentuh.
        fn assert_store<T: TokenStore + 'static>() {}
        assert_store::<KeychainStore>();
        assert_store::<MemoryStore>();
        let stores: Vec<Box<dyn TokenStore>> = vec![Box::new(MemoryStore::new())];
        assert_eq!(stores.len(), 1);
        let _unused: fn() -> KeychainStore = KeychainStore::new;
    }

    #[test]
    fn keychain_errors_never_carry_secret_bytes() {
        let err = keychain_error(keyring::Error::BadEncoding(b"token_rahasia".to_vec()));
        assert_eq!(err.code, ForgeErrorCode::Keychain);
        assert!(!err.message.contains("token_rahasia"));
        assert!(!err.message.contains("116"), "byte mentah tidak boleh tercetak: {}", err.message);
    }
}

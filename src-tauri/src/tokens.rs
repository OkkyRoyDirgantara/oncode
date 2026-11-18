//! Penyimpanan token: keychain OS, dengan cadangan di memori bila keychain tidak tersedia
//! (mis. Linux tanpa layanan Secret Service). Token di memori hilang saat aplikasi ditutup.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use oncode_forge::{ForgeError, ForgeErrorCode, MemoryStore, Result, TokenSet, TokenStore};

/// Batas waktu satu operasi keychain. Tanpa layanan keychain, panggilan D-Bus bisa
/// menunggu puluhan detik sebelum gagal; pengguna tidak boleh ikut menunggu selama itu.
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(4);

pub struct FallbackStore {
    primary: Arc<dyn TokenStore>,
    memory: MemoryStore,
    memory_only: AtomicBool,
    timeout: Duration,
}

impl FallbackStore {
    pub fn new(primary: Arc<dyn TokenStore>) -> Self {
        Self::with_timeout(primary, KEYCHAIN_TIMEOUT)
    }

    pub fn with_timeout(primary: Arc<dyn TokenStore>, timeout: Duration) -> Self {
        Self { primary, memory: MemoryStore::new(), memory_only: AtomicBool::new(false), timeout }
    }

    /// `true` setelah keychain gagal dipakai: token hanya bertahan selama sesi ini.
    pub fn is_memory_only(&self) -> bool {
        self.memory_only.load(Ordering::SeqCst)
    }

    /// Menjalankan operasi keychain di thread tersendiri; melewati batas waktu dianggap keychain tidak tersedia.
    fn primary<T: Send + 'static>(&self, operation: impl FnOnce(&dyn TokenStore) -> Result<T> + Send + 'static) -> Result<T> {
        let store = self.primary.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(operation(store.as_ref()));
        });
        match rx.recv_timeout(self.timeout) {
            Ok(result) => result,
            Err(_) => {
                self.memory_only.store(true, Ordering::SeqCst);
                Err(ForgeError { code: ForgeErrorCode::Keychain, message: "The system keychain did not respond".into() })
            }
        }
    }
}

impl TokenStore for FallbackStore {
    fn save(&self, account_id: &str, tokens: &TokenSet) -> Result<()> {
        if !self.is_memory_only() {
            let (id, value) = (account_id.to_string(), tokens.clone());
            match self.primary(move |store| store.save(&id, &value)) {
                Ok(()) => return Ok(()),
                Err(e) if e.code == ForgeErrorCode::Keychain => self.memory_only.store(true, Ordering::SeqCst),
                Err(e) => return Err(e),
            }
        }
        self.memory.save(account_id, tokens)
    }

    fn load(&self, account_id: &str) -> Result<Option<TokenSet>> {
        if let Some(tokens) = self.memory.load(account_id)? {
            return Ok(Some(tokens));
        }
        if self.is_memory_only() {
            return Ok(None);
        }
        let id = account_id.to_string();
        match self.primary(move |store| store.load(&id)) {
            // Keychain yang tidak bisa dibuka diperlakukan seperti token tidak ada: pengguna diminta login lagi.
            Err(e) if e.code == ForgeErrorCode::Keychain => Ok(None),
            other => other,
        }
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        self.memory.delete(account_id)?;
        if self.is_memory_only() {
            return Ok(());
        }
        let id = account_id.to_string();
        match self.primary(move |store| store.delete(&id)) {
            Err(e) if e.code == ForgeErrorCode::Keychain => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keychain yang selalu gagal, seperti di sistem tanpa Secret Service.
    struct Broken;
    impl TokenStore for Broken {
        fn save(&self, _: &str, _: &TokenSet) -> Result<()> {
            Err(ForgeError { code: ForgeErrorCode::Keychain, message: "no keychain".into() })
        }
        fn load(&self, _: &str) -> Result<Option<TokenSet>> {
            Err(ForgeError { code: ForgeErrorCode::Keychain, message: "no keychain".into() })
        }
        fn delete(&self, _: &str) -> Result<()> {
            Err(ForgeError { code: ForgeErrorCode::Keychain, message: "no keychain".into() })
        }
    }

    /// Keychain yang tidak pernah menjawab tepat waktu.
    struct Hanging;
    impl TokenStore for Hanging {
        fn save(&self, _: &str, _: &TokenSet) -> Result<()> {
            std::thread::sleep(Duration::from_secs(3));
            Ok(())
        }
        fn load(&self, _: &str) -> Result<Option<TokenSet>> {
            std::thread::sleep(Duration::from_secs(3));
            Ok(None)
        }
        fn delete(&self, _: &str) -> Result<()> {
            std::thread::sleep(Duration::from_secs(3));
            Ok(())
        }
    }

    #[test]
    fn unresponsive_keychain_times_out_once_then_uses_memory() {
        let store = FallbackStore::with_timeout(Arc::new(Hanging), Duration::from_millis(100));
        let started = std::time::Instant::now();
        store.save("a", &tokens("t1")).unwrap();
        assert!(store.is_memory_only());
        assert_eq!(store.load("a").unwrap().unwrap().access_token, "t1");
        // Operasi berikutnya tidak menunggu keychain lagi.
        store.save("b", &tokens("t2")).unwrap();
        assert!(store.load("tidak-ada").unwrap().is_none());
        store.delete("a").unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "hanya operasi pertama yang boleh menunggu batas waktu");
    }

    fn tokens(value: &str) -> TokenSet {
        TokenSet { access_token: value.into(), refresh_token: None, expires_at: None }
    }

    #[test]
    fn working_keychain_is_used_and_nothing_stays_in_memory() {
        let store = FallbackStore::new(Arc::new(MemoryStore::new()));
        store.save("a", &tokens("t1")).unwrap();
        assert!(!store.is_memory_only());
        assert_eq!(store.load("a").unwrap().unwrap().access_token, "t1");
        store.delete("a").unwrap();
        assert!(store.load("a").unwrap().is_none());
    }

    #[test]
    fn broken_keychain_falls_back_to_memory_for_the_session() {
        let store = FallbackStore::new(Arc::new(Broken));
        // Sebelum ada yang disimpan, keychain rusak berarti "tidak ada token", bukan galat.
        assert!(store.load("a").unwrap().is_none());
        store.save("a", &tokens("t1")).unwrap();
        assert!(store.is_memory_only());
        assert_eq!(store.load("a").unwrap().unwrap().access_token, "t1");
        store.save("a", &tokens("t2")).unwrap();
        assert_eq!(store.load("a").unwrap().unwrap().access_token, "t2");
        store.delete("a").unwrap();
        assert!(store.load("a").unwrap().is_none());
    }
}

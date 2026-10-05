//! The extension's credentials on the daemon (spec
//! 2026-10-05-chrome-overlay-design §5.3, §10): an in-memory map from each
//! live credential's hash to its extension ID, loaded on start and replaced
//! after every mint and revoke, so checking a credential reads no store. A
//! live credential is the owner identity (spec L6); it names no viewer.

use clax_core::Store;
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

/// How often one credential's `last_used_at` is written.
const TOUCH_EVERY: Duration = Duration::from_secs(3600);

/// What a live credential stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cred {
    pub extension_id: String,
}

/// The live credentials, by hash.
#[derive(Default)]
pub struct Credentials {
    map: RwLock<HashMap<String, Cred>>,
    touched: Mutex<HashMap<String, Instant>>,
}

impl Credentials {
    /// The live credentials the store holds now.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<Credentials> {
        let c = Credentials::default();
        for k in st.live_extension_credentials()? {
            c.insert(
                &k.hash,
                Cred {
                    extension_id: k.extension_id,
                },
            );
        }
        Ok(c)
    }

    /// The credential whose hash is `hash`, when it is live.
    pub fn get(&self, hash: &str) -> Option<Cred> {
        self.map
            .read()
            .expect("credentials lock")
            .get(hash)
            .cloned()
    }

    pub fn insert(&self, hash: &str, c: Cred) {
        self.map
            .write()
            .expect("credentials lock")
            .insert(hash.to_string(), c);
    }

    /// Takes `other`'s credentials in place of these (after a mint, which
    /// may revoke the oldest, and after a revoke).
    pub fn replace_with(&self, other: Credentials) {
        let map = other.map.into_inner().expect("credentials lock");
        self.touched
            .lock()
            .expect("touch lock")
            .retain(|h, _| map.contains_key(h));
        *self.map.write().expect("credentials lock") = map;
    }

    /// Whether a use of the credential whose hash is `hash` should be written
    /// to the store now: at most once per [`TOUCH_EVERY`] per credential, and
    /// on its first use since the daemon started.
    pub fn due_for_touch(&self, hash: &str) -> bool {
        let mut t = self.touched.lock().expect("touch lock");
        let now = Instant::now();
        match t.get(hash) {
            Some(at) if now.duration_since(*at) < TOUCH_EVERY => false,
            _ => {
                t.insert(hash.to_string(), now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;

    #[test]
    fn the_cache_holds_the_live_credentials_and_a_touch_is_due_once() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let a = st.mint_extension_credential("abc").unwrap();
        let c = Credentials::load(&st).unwrap();
        assert_eq!(
            c.get(&a.hash),
            Some(Cred {
                extension_id: "abc".into()
            })
        );
        assert_eq!(c.get(&a.credential), None, "keyed by hash only");
        assert!(c.due_for_touch(&a.hash));
        assert!(!c.due_for_touch(&a.hash));
        st.revoke_extension_credentials().unwrap();
        c.replace_with(Credentials::load(&st).unwrap());
        assert_eq!(c.get(&a.hash), None);
        assert!(
            c.due_for_touch(&a.hash),
            "a dropped credential's touch is forgotten"
        );
    }
}

//! The extension's credentials on the daemon (spec
//! 2026-10-05-chrome-overlay-design §5.3, §10): an in-memory map from each
//! live credential's hash to its extension ID and last use, loaded on start
//! and reloaded after every mint and revoke, so checking a credential reads
//! no store. A credential unused for [`CREDENTIAL_TTL_DAYS`] stops being
//! accepted while the daemon runs, as it would after a restart. A live
//! credential is the owner identity (spec L6); it names no viewer.

use chrono::{DateTime, Duration, Utc};
use clax_core::Store;
use clax_core::extension::CREDENTIAL_TTL_DAYS;
use clax_core::working::{Clock, SystemClock};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// How often one credential's `last_used_at` is written.
const TOUCH_EVERY: Duration = Duration::hours(1);

/// What a live credential stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cred {
    pub extension_id: String,
}

struct Entry {
    cred: Cred,
    /// The last use the store holds (or this daemon is about to write).
    last_used: DateTime<Utc>,
}

/// The live credentials, by hash.
pub struct Credentials {
    clock: Arc<dyn Clock>,
    map: RwLock<HashMap<String, Entry>>,
    /// Held across a store change and the reload after it, so two changes
    /// never replace the map out of order.
    refreshing: Mutex<()>,
}

impl Credentials {
    /// No credentials, reading the time from `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Credentials {
        Credentials {
            clock,
            map: RwLock::default(),
            refreshing: Mutex::new(()),
        }
    }

    /// The live credentials the store holds now, on the system clock.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<Credentials> {
        Self::load_with(st, Arc::new(SystemClock))
    }

    /// The live credentials the store holds now, reading the time from `clock`.
    ///
    /// # Errors
    /// The store's.
    pub fn load_with(st: &Store, clock: Arc<dyn Clock>) -> clax_core::Result<Credentials> {
        let c = Credentials::new(clock);
        *c.map.write().expect("credentials lock") = read_map(st)?;
        Ok(c)
    }

    /// The credential whose hash is `hash`, while it is live. One unused for
    /// [`CREDENTIAL_TTL_DAYS`] is dropped and answers `None`.
    pub fn get(&self, hash: &str) -> Option<Cred> {
        let now = self.clock.now();
        {
            let map = self.map.read().expect("credentials lock");
            match map.get(hash) {
                None => return None,
                Some(e) if live(e, now) => return Some(e.cred.clone()),
                Some(_) => {}
            }
        }
        let mut map = self.map.write().expect("credentials lock");
        if map.get(hash).is_some_and(|e| !live(e, now)) {
            map.remove(hash);
        }
        None
    }

    /// Adds a credential, last used now.
    pub fn insert(&self, hash: &str, c: Cred) {
        let last_used = self.clock.now();
        self.map
            .write()
            .expect("credentials lock")
            .insert(hash.to_string(), Entry { cred: c, last_used });
    }

    /// Takes `other`'s credentials in place of these.
    pub fn replace_with(&self, other: Credentials) {
        let map = other.map.into_inner().expect("credentials lock");
        *self.map.write().expect("credentials lock") = map;
    }

    /// Runs `change` on the store (a mint or a revoke), then reloads these
    /// credentials from it; concurrent refreshes run one at a time, so the
    /// map always ends as the store's latest state.
    ///
    /// # Errors
    /// `change`'s, or the store's.
    pub fn refresh<T>(
        &self,
        st: &Store,
        change: impl FnOnce(&Store) -> clax_core::Result<T>,
    ) -> clax_core::Result<T> {
        let _one = self.refreshing.lock().expect("refresh lock");
        let out = change(st)?;
        *self.map.write().expect("credentials lock") = read_map(st)?;
        Ok(out)
    }

    /// Whether a use, now, of the live credential whose hash is `hash` should
    /// be written to the store: when it was last used [`TOUCH_EVERY`] ago or
    /// more. Answering `true` records the use here.
    pub fn due_for_touch(&self, hash: &str) -> bool {
        let now = self.clock.now();
        let mut map = self.map.write().expect("credentials lock");
        match map.get_mut(hash) {
            Some(e) if live(e, now) && now - e.last_used >= TOUCH_EVERY => {
                e.last_used = now;
                true
            }
            _ => false,
        }
    }
}

/// Whether `e` has been used within [`CREDENTIAL_TTL_DAYS`] of `now`.
fn live(e: &Entry, now: DateTime<Utc>) -> bool {
    now - e.last_used <= Duration::days(CREDENTIAL_TTL_DAYS)
}

fn read_map(st: &Store) -> clax_core::Result<HashMap<String, Entry>> {
    Ok(st
        .live_extension_credentials()?
        .into_iter()
        .filter_map(|k| {
            let last_used = DateTime::parse_from_rfc3339(&k.last_used_at)
                .ok()?
                .with_timezone(&Utc);
            Some((
                k.hash,
                Entry {
                    cred: Cred {
                        extension_id: k.extension_id,
                    },
                    last_used,
                },
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::working::ManualClock;

    const DAY: i64 = 86_400;

    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, Arc::new(st))
    }

    #[test]
    fn the_cache_holds_the_live_credentials_by_hash() {
        let (_dir, st) = store();
        let a = st.mint_extension_credential("abc").unwrap();
        let c = Credentials::load(&st).unwrap();
        assert_eq!(
            c.get(&a.hash),
            Some(Cred {
                extension_id: "abc".into()
            })
        );
        assert_eq!(c.get(&a.credential), None, "keyed by hash only");
        c.refresh(&st, |st| st.revoke_extension_credentials())
            .unwrap();
        assert_eq!(c.get(&a.hash), None);
    }

    #[test]
    fn an_idle_credential_expires_while_the_daemon_runs_and_a_use_keeps_one_live() {
        let (_dir, st) = store();
        let idle = st.mint_extension_credential("abc").unwrap();
        let used = st.mint_extension_credential("abc").unwrap();
        let clock = Arc::new(ManualClock::at(&Store::now()));
        let c = Credentials::load_with(&st, clock.clone()).unwrap();
        assert!(!c.due_for_touch(&used.hash), "used just now");
        clock.advance(29 * DAY);
        assert!(c.due_for_touch(&used.hash));
        assert!(!c.due_for_touch(&used.hash), "at most hourly");
        clock.advance(DAY + 1);
        assert_eq!(c.get(&idle.hash), None, "unused for more than 30 days");
        assert!(
            !c.due_for_touch(&idle.hash),
            "an expired credential is dropped"
        );
        assert!(c.get(&used.hash).is_some(), "used a day ago");
        clock.advance(30 * DAY);
        assert_eq!(c.get(&used.hash), None);
    }

    #[test]
    fn concurrent_mints_never_drop_a_fresh_credential() {
        let (_dir, st) = store();
        let c = Arc::new(Credentials::load(&st).unwrap());
        let mints: Vec<_> = (0..clax_core::extension::MAX_CREDENTIALS)
            .map(|_| {
                let (st, c) = (st.clone(), c.clone());
                std::thread::spawn(move || {
                    c.refresh(&st, |st| st.mint_extension_credential("abc"))
                        .unwrap()
                })
            })
            .collect();
        for m in mints {
            let m = m.join().unwrap();
            assert!(c.get(&m.hash).is_some());
        }
    }
}

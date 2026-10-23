//! Sealed secrets (migration 50, spec 2026-10-21-claxbot-design §8.1): the
//! instance's model key and people's own keys, kept sealed with
//! XChaCha20-Poly1305 under a key-encryption key ([`Kek`]) the caller
//! hands in. The store does not know where the KEK lives: locally it is
//! `<home>/secrets/kek` ([`Kek::load_or_create`]), on the hosted Worker the
//! `CLAX_KEK` secret ([`Kek::from_slice`]).
//!
//! A key is write-only: every read but [`Store::open_for_use`], which the
//! model client alone calls, gives its metadata ([`SecretMeta`]: last four
//! characters, who set it when, when it last worked). Each sealing has a
//! fresh random 24-byte nonce and binds the slot it is stored in as
//! associated data, so a sealed value copied into another slot does not
//! open.
//!
//! [`Kek::load_or_create`] lives here rather than in `clax-core` because
//! the KEK is the store's concern alone (nothing else seals with it), and
//! the store already writes the home's private files; it is built off
//! `wasm32` only, where a Worker has no home on disk.

use super::Store;
use super::credits::KeySource;
use super::roles::set_by;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result, Secret};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// A key-encryption key: 32 bytes. Its `Debug` shows none of them, and
/// they are zeroed when it is dropped.
#[derive(Clone)]
pub struct Kek([u8; 32]);

/// How long a maker's temporary directory may stay before a later
/// [`Kek::load_or_create`] removes it: an hour, far past any maker still at
/// work.
pub const LEFTOVER_SECS: i64 = 3600;

/// Removes the temporary directories of KEK makers that died before their
/// rename: entries of `home`'s root named exactly `.secrets-<ULID>.tmp`
/// whose ULID was minted more than [`LEFTOVER_SECS`] before `now`, that are
/// directories themselves (a symbolic link is never followed or removed),
/// together with the one regular file `kek` such a directory may hold.
/// Anything else is left; a failure to remove is ignored, as the next load
/// tries again.
#[cfg(not(target_arch = "wasm32"))]
pub fn sweep_leftovers(home: &clax_core::Home, now: chrono::DateTime<chrono::Utc>) {
    let Ok(entries) = std::fs::read_dir(home.root()) else {
        return;
    };
    let cutoff = now.timestamp_millis() - LEFTOVER_SECS * 1000;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(ulid) = name
            .strip_prefix(".secrets-")
            .and_then(|r| r.strip_suffix(".tmp"))
        else {
            continue;
        };
        let Some(ms) = clax_core::ids::ulid_ms(ulid) else {
            continue;
        };
        if i64::try_from(ms).unwrap_or(i64::MAX) >= cutoff {
            continue;
        }
        let dir = entry.path();
        // `symlink_metadata` describes a link itself, never its target.
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let kek = dir.join("kek");
        if std::fs::symlink_metadata(&kek).is_ok_and(|m| m.is_file()) {
            let _ = std::fs::remove_file(&kek);
        }
        let _ = std::fs::remove_dir(&dir);
    }
}

impl Drop for Kek {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl std::fmt::Debug for Kek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Kek(…)")
    }
}

impl Kek {
    /// The KEK `bytes` are.
    pub fn from_bytes(bytes: [u8; 32]) -> Kek {
        Kek(bytes)
    }

    /// The KEK in `bytes`, which must be exactly 32.
    ///
    /// # Errors
    /// `invalid_kek` for any other length.
    pub fn from_slice(bytes: &[u8]) -> Result<Kek> {
        <[u8; 32]>::try_from(bytes)
            .map(Kek)
            .map_err(|_| CoreError::invalid("invalid_kek", "a key-encryption key is 32 bytes"))
    }

    /// A new KEK from the operating system's random source.
    pub fn generate() -> Kek {
        Kek(clax_core::secret::random_bytes::<32>())
    }

    /// The KEK in `home`'s `secrets/kek` ([`clax_core::Home::kek_path`]),
    /// made there first when missing. It is never replaced, since every
    /// sealed secret depends on it.
    ///
    /// Making it never leaves a partial key where a reader would find it:
    /// the 32 random bytes are written and synced in a new file, 0600, in a
    /// new 0700 directory beside `secrets` (`.secrets-<ULID>.tmp`), and that
    /// directory is then renamed to `secrets`. A rename onto a directory
    /// that holds anything fails, so of two processes making the key at
    /// once one wins, and the other removes its own directory and reads the
    /// winner's key, complete. A process that dies before the rename leaves
    /// only its temporary directory; once the key is read, such leftovers
    /// minted more than [`LEFTOVER_SECS`] ago are removed
    /// ([`sweep_leftovers`]).
    ///
    /// # Errors
    /// `kek_unreadable` for a `secrets/kek` that is not 32 bytes, or a
    /// `secrets` directory that holds no key; the I/O error.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_or_create(home: &clax_core::Home) -> Result<Kek> {
        Kek::load_or_create_at(home, chrono::Utc::now())
    }

    /// [`Kek::load_or_create`] at `now`, the time leftovers are aged by.
    ///
    /// # Errors
    /// [`Kek::load_or_create`]'s.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_or_create_at(
        home: &clax_core::Home,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Kek> {
        let kek = Kek::load_or_make(home)?;
        sweep_leftovers(home, now);
        Ok(kek)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_or_make(home: &clax_core::Home) -> Result<Kek> {
        use std::io::Write as _;
        let path = home.kek_path();
        let read = |path: &std::path::Path| -> Result<Option<Kek>> {
            match std::fs::read(path) {
                Ok(mut b) => {
                    let k = Kek::from_slice(&b).map(Some).map_err(|_| {
                        CoreError::invalid(
                            "kek_unreadable",
                            format!("{} is not a 32-byte key", path.display()),
                        )
                    });
                    zeroize::Zeroize::zeroize(&mut b);
                    k
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            }
        };
        if let Some(k) = read(&path)? {
            return Ok(k);
        }
        let secrets = path.parent().expect("the KEK has a directory");
        let mut dirs = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut dirs, 0o700);
        dirs.recursive(true).create(home.root())?;
        let tmp = home
            .root()
            .join(format!(".secrets-{}.tmp", clax_core::new_ulid()));
        let made = (|| -> std::io::Result<()> {
            dirs.recursive(false).create(&tmp)?;
            let mut open = std::fs::OpenOptions::new();
            open.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
            let mut f = open.open(tmp.join("kek"))?;
            f.write_all(&Kek::generate().0)?;
            f.sync_all()?;
            #[cfg(unix)]
            std::fs::File::open(&tmp)?.sync_all()?;
            std::fs::rename(&tmp, secrets)
        })();
        if made.is_err() {
            // Lost the race (or failed): ours goes, the key there is read.
            let _ = std::fs::remove_file(tmp.join("kek"));
            let _ = std::fs::remove_dir(&tmp);
        }
        match read(&path)? {
            Some(k) => Ok(k),
            None => match made {
                Err(e) if !secrets.exists() => Err(e.into()),
                _ => Err(CoreError::invalid(
                    "kek_unreadable",
                    format!("{} holds no key", secrets.display()),
                )),
            },
        }
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }
}

/// Where a secret is kept.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SecretSlot {
    /// The instance's model key.
    Instance,
    /// A person's own model key, by person key.
    Own(String),
}

impl SecretSlot {
    pub(crate) fn scope(&self) -> &'static str {
        match self {
            SecretSlot::Instance => "instance",
            SecretSlot::Own(_) => "own",
        }
    }

    pub(crate) fn person(&self) -> &str {
        match self {
            SecretSlot::Instance => "",
            SecretSlot::Own(p) => p,
        }
    }

    /// The associated data a sealing in this slot binds.
    fn aad(&self) -> Vec<u8> {
        format!("clax-secret/v1/{}/{}", self.scope(), self.person()).into_bytes()
    }
}

/// What every read of a secret gives: never the secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretMeta {
    /// Its last four characters.
    pub last4: String,
    pub set_at: String,
    /// The person key of who set it (`*system` for Clax itself).
    pub set_by: String,
    /// When a request with it last succeeded.
    pub last_ok_at: Option<String>,
    /// For an own key: whether its person runs ClaxBot on it.
    pub use_own: bool,
    /// The origin it was set for, and is only ever sent to
    /// ([`clax_core::config::key_host`] of the `base_url` in effect when
    /// it was set); `None` for a key set before keys were bound, bound at
    /// its first use ([`Store::open_for_use`]).
    pub host: Option<String>,
}

impl SecretMeta {
    /// Why the key is not sent to `base_url`: it was set for another
    /// origin. `None` when it may be (or it binds there at first use).
    #[must_use]
    pub fn host_problem(&self, base_url: &str) -> Option<String> {
        let host = self.host.as_deref()?;
        (clax_core::config::key_host(base_url).as_deref() != Some(host))
            .then(|| format!("it was set for {host}; set it again"))
    }
}

/// The longest key accepted, in bytes.
pub const MAX_KEY_BYTES: usize = 512;

/// `key` trimmed, checked: 8 to [`MAX_KEY_BYTES`] visible ASCII characters.
fn check_key(key: &str) -> Result<&str> {
    let k = key.trim();
    if k.len() < 8 || k.len() > MAX_KEY_BYTES || !k.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(CoreError::invalid(
            "invalid_key",
            format!("a key is 8 to {MAX_KEY_BYTES} visible ASCII characters"),
        ));
    }
    Ok(k)
}

pub(crate) fn meta_in(c: &Connection, slot: &SecretSlot) -> Result<Option<SecretMeta>> {
    Ok(c.prepare_cached(
        "SELECT last4, set_at, set_by, last_ok_at, use_own, host FROM sealed_secrets
              WHERE scope = ?1 AND person = ?2",
    )?
    .query_row(params![slot.scope(), slot.person()], |r| {
        Ok(SecretMeta {
            last4: r.get(0)?,
            set_at: r.get(1)?,
            set_by: r.get(2)?,
            last_ok_at: r.get(3)?,
            use_own: r.get(4)?,
            host: r.get(5)?,
        })
    })
    .optional()?)
}

/// The `admin.key` record of `op` on `slot`.
pub(crate) fn key_record(at: String, slot: &SecretSlot, op: &str, last4: &str) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::AdminKey, at)
        .with("scope", slot.scope())
        .with("op", op)
        .with("last4", last4);
    if let SecretSlot::Own(p) = slot {
        rec = rec.with("person", p.as_str());
    }
    rec
}

impl Store {
    /// Seals `key` under `kek` into `slot`, replacing what was there, and
    /// records `admin.key {scope, op: set, last4, person?}` under `ctx`.
    /// An own key starts in use.
    ///
    /// # Errors
    /// `invalid_key`; the store's.
    pub fn set_secret(
        &self,
        ctx: &AuditCtx,
        kek: &Kek,
        slot: &SecretSlot,
        key: &str,
    ) -> Result<SecretMeta> {
        let key = check_key(key)?;
        let nonce = clax_core::secret::random_bytes::<24>();
        let aad = slot.aad();
        let sealed = kek
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: key.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| CoreError::invalid("seal_failed", "the key could not be sealed"))?;
        let last4: String = {
            let chars: Vec<char> = key.chars().collect();
            chars[chars.len() - 4..].iter().collect()
        };
        // Bound to the provider in effect now: the key is only ever sent
        // there ([`Store::open_for_use`]).
        let host = self
            .bot_setting(crate::bot_settings::Setting::BaseUrl)?
            .as_str()
            .and_then(clax_core::config::key_host);
        self.with_tx(|tx| {
            let now = self.now();
            tx.execute(
                "INSERT INTO sealed_secrets (scope, person, nonce, sealed, last4, use_own, set_by, set_at, last_ok_at, host)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, NULL, ?8)
                 ON CONFLICT (scope, person) DO UPDATE SET nonce = excluded.nonce,
                   sealed = excluded.sealed, last4 = excluded.last4, use_own = 1,
                   set_by = excluded.set_by, set_at = excluded.set_at, last_ok_at = NULL,
                   host = excluded.host",
                params![slot.scope(), slot.person(), &nonce[..], sealed, last4, set_by(ctx), now, host],
            )?;
            self.record_audit(tx, ctx, key_record(now, slot, "set", &last4))?;
            Ok(meta_in(tx, slot)?.expect("the secret was just written"))
        })
    }

    /// Clears `slot`, recording `admin.key {op: clear}` when it held a
    /// key. Returns whether it did.
    ///
    /// # Errors
    /// The store's.
    pub fn clear_secret(&self, ctx: &AuditCtx, slot: &SecretSlot) -> Result<bool> {
        self.with_tx(|tx| {
            let Some(meta) = meta_in(tx, slot)? else {
                return Ok(false);
            };
            tx.execute(
                "DELETE FROM sealed_secrets WHERE scope = ?1 AND person = ?2",
                params![slot.scope(), slot.person()],
            )?;
            self.record_audit(tx, ctx, key_record(self.now(), slot, "clear", &meta.last4))?;
            Ok(true)
        })
    }

    /// `slot`'s metadata, or `None` when it holds no key.
    ///
    /// # Errors
    /// The store's.
    pub fn secret_meta(&self, slot: &SecretSlot) -> Result<Option<SecretMeta>> {
        self.with_read(|c| meta_in(c, slot))
    }

    /// The key in `slot`, opened with `kek`, for the model client to send
    /// to `base_url`; `None` when it holds no key. Nothing else reads a key
    /// back. A key is sent only to the origin it was set for: another is
    /// refused `key_host_mismatch`, as is a URL the base-URL policy refuses
    /// ([`Store::set_base_url_policy`]); a key with no recorded origin
    /// (set before keys were bound) is bound to `base_url` now.
    ///
    /// # Errors
    /// `key_host_mismatch` as above; `kek_mismatch` when `kek` is not the
    /// one it was sealed with (or the row was tampered with); the store's.
    pub fn open_for_use(
        &self,
        kek: &Kek,
        slot: &SecretSlot,
        base_url: &str,
    ) -> Result<Option<Secret>> {
        type Row = (Vec<u8>, Vec<u8>, Option<String>);
        let row: Option<Row> = self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT nonce, sealed, host FROM sealed_secrets WHERE scope = ?1 AND person = ?2",
            )?
            .query_row(params![slot.scope(), slot.person()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?)
        })?;
        let Some((nonce, sealed, host)) = row else {
            return Ok(None);
        };
        // Only to a URL the policy allows, and only to the origin the key
        // was set for.
        let refused = |why: String| CoreError::invalid("key_host_mismatch", why);
        let target = clax_core::config::key_host(base_url)
            .filter(|_| self.base_url_policy().allows(base_url))
            .ok_or_else(|| {
                refused(format!(
                    "{base_url} is not a URL a model key may be sent to"
                ))
            })?;
        match host {
            Some(h) if h != target => {
                return Err(refused(format!("the key was set for {h}; set it again")));
            }
            Some(_) => {}
            // Set before keys were bound: bound to the first allowed URL
            // it is used with.
            None => self.with_tx(|tx| {
                tx.execute(
                    "UPDATE sealed_secrets SET host = ?3 WHERE scope = ?1 AND person = ?2
                       AND host IS NULL",
                    params![slot.scope(), slot.person(), target],
                )?;
                Ok(())
            })?,
        }
        let mismatch = || {
            CoreError::invalid(
                "kek_mismatch",
                "the stored key does not open with this key-encryption key; set the key again",
            )
        };
        if nonce.len() != 24 {
            return Err(mismatch());
        }
        let aad = slot.aad();
        let plain = kek
            .cipher()
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| mismatch())?;
        String::from_utf8(plain)
            .map(|s| Some(Secret::new(s)))
            .map_err(|_| mismatch())
    }

    /// Notes that a request with `slot`'s key succeeded now (the store's
    /// clock).
    ///
    /// # Errors
    /// The store's.
    pub fn mark_secret_ok(&self, slot: &SecretSlot) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE sealed_secrets SET last_ok_at = ?3 WHERE scope = ?1 AND person = ?2",
                params![slot.scope(), slot.person(), self.now()],
            )?;
            Ok(())
        })
    }

    /// Whether `person` runs ClaxBot on their own key (`use_own`), which
    /// they keep set, as `ctx`; a change is recorded as `admin.key {scope:
    /// own, op: use_own, value, last4, person}`. Returns whether it changed.
    ///
    /// # Errors
    /// `NotFound` when they have no own key; the store's.
    pub fn set_use_own(&self, ctx: &AuditCtx, person: &str, use_own: bool) -> Result<bool> {
        self.with_tx(|tx| {
            let slot = SecretSlot::Own(person.into());
            let meta = meta_in(tx, &slot)?.ok_or(CoreError::NotFound)?;
            let changed = tx.execute(
                "UPDATE sealed_secrets SET use_own = ?2 WHERE scope = 'own' AND person = ?1
                   AND use_own <> ?2",
                params![person, use_own],
            )? > 0;
            if changed {
                let rec =
                    key_record(self.now(), &slot, "use_own", &meta.last4).with("value", use_own);
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(changed)
        })
    }

    /// The key `person`'s runs use: their own when set and in use, else
    /// the instance's.
    ///
    /// # Errors
    /// The store's.
    pub fn key_source_of(&self, person: &str) -> Result<KeySource> {
        Ok(match self.secret_meta(&SecretSlot::Own(person.into()))? {
            Some(m) if m.use_own => KeySource::Own,
            _ => KeySource::Instance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, store};

    const KEY: &str = "sk-ant-api03-SECRETSECRETSECRET-wxyz";
    /// The default base URL, which keys set in these tests are bound to.
    const API: &str = "https://api.anthropic.com";

    #[test]
    fn a_sealed_key_opens_with_its_kek_alone_and_reads_only_as_metadata() {
        let (_d, st) = store();
        let kek = Kek::from_bytes([7; 32]);
        let meta = st
            .set_secret(DAEMON, &kek, &SecretSlot::Instance, KEY)
            .unwrap();
        assert_eq!(meta.last4, "wxyz");
        assert_eq!(meta.set_by, "*system");
        assert_eq!(meta.last_ok_at, None);
        let opened = st
            .open_for_use(&kek, &SecretSlot::Instance, API)
            .unwrap()
            .unwrap();
        assert_eq!(opened.expose(), KEY);
        assert!(!format!("{opened:?}").contains("SECRET"));
        assert!(!format!("{kek:?}").contains('7'));

        let wrong = Kek::from_bytes([8; 32]);
        assert!(matches!(
            st.open_for_use(&wrong, &SecretSlot::Instance, API),
            Err(CoreError::Invalid {
                code: "kek_mismatch",
                ..
            })
        ));
        // The key is nowhere in the database or the audit trail in the clear.
        let dump: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT quote(nonce) || quote(sealed) || last4 FROM sealed_secrets",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(!dump.contains("SECRET"));
        let events = st.events_after(0, 100).unwrap();
        assert!(events.iter().all(|e| !e.body.contains("SECRET")));
        let set = events.iter().find(|e| e.kind == "admin.key").unwrap();
        assert!(
            set.body.contains("\"scope\":\"instance\"") && set.body.contains("\"last4\":\"wxyz\"")
        );

        st.mark_secret_ok(&SecretSlot::Instance).unwrap();
        assert!(
            st.secret_meta(&SecretSlot::Instance)
                .unwrap()
                .unwrap()
                .last_ok_at
                .is_some()
        );
        assert!(st.clear_secret(DAEMON, &SecretSlot::Instance).unwrap());
        assert!(!st.clear_secret(DAEMON, &SecretSlot::Instance).unwrap());
        assert_eq!(
            st.open_for_use(&kek, &SecretSlot::Instance, API)
                .unwrap()
                .map(|s| s.expose().to_string()),
            None
        );
    }

    #[test]
    fn a_sealing_is_bound_to_its_slot() {
        let (_d, st) = store();
        let kek = Kek::generate();
        st.set_secret(DAEMON, &kek, &SecretSlot::Own(OWNER.into()), KEY)
            .unwrap();
        // The owner's sealed key copied into the instance slot does not open.
        st.with_write(|c| {
            c.execute_batch(
                "INSERT INTO sealed_secrets (scope, person, nonce, sealed, last4, use_own, set_by, set_at)
                 SELECT 'instance', '', nonce, sealed, last4, 1, set_by, set_at FROM sealed_secrets",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(st.open_for_use(&kek, &SecretSlot::Instance, API).is_err());
        assert!(
            st.open_for_use(&kek, &SecretSlot::Own(OWNER.into()), API)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn own_keys_choose_the_key_source() {
        let (_d, st) = store();
        let kek = Kek::generate();
        assert_eq!(st.key_source_of(OWNER).unwrap(), KeySource::Instance);
        assert!(matches!(
            st.set_use_own(DAEMON, OWNER, true),
            Err(CoreError::NotFound)
        ));
        st.set_secret(DAEMON, &kek, &SecretSlot::Own(OWNER.into()), KEY)
            .unwrap();
        assert_eq!(st.key_source_of(OWNER).unwrap(), KeySource::Own);
        assert!(st.set_use_own(DAEMON, OWNER, false).unwrap());
        assert!(!st.set_use_own(DAEMON, OWNER, false).unwrap());
        let uses: Vec<String> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "admin.key" && e.body.contains("\"op\":\"use_own\""))
            .map(|e| e.body)
            .collect();
        assert_eq!(uses.len(), 1, "one change, one event");
        assert!(uses[0].contains("\"value\":false"), "{}", uses[0]);
        assert_eq!(st.key_source_of(OWNER).unwrap(), KeySource::Instance);
        let ev = st.events_after(0, 100).unwrap();
        let set = ev.iter().find(|e| e.kind == "admin.key").unwrap();
        assert!(set.body.contains("\"person\":\"*owner\""), "{}", set.body);
    }

    #[test]
    fn bad_keys_and_keks_are_refused() {
        let (_d, st) = store();
        let kek = Kek::generate();
        for bad in ["short", "has a space in it", "naïve-key-value"] {
            assert!(matches!(
                st.set_secret(DAEMON, &kek, &SecretSlot::Instance, bad),
                Err(CoreError::Invalid {
                    code: "invalid_key",
                    ..
                })
            ));
        }
        assert!(Kek::from_slice(&[0; 31]).is_err());
        assert!(Kek::from_slice(&[0; 32]).is_ok());
    }

    #[test]
    fn racing_makers_agree_on_one_complete_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("ax"));
        // A maker that died before its rename left a partial directory.
        let dead = home
            .root()
            .join(format!(".secrets-{}.tmp", clax_core::new_ulid()));
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::write(dead.join("kek"), b"").unwrap();
        let keys: Vec<[u8; 32]> = std::thread::scope(|s| {
            let makers: Vec<_> = (0..8)
                .map(|_| s.spawn(|| Kek::load_or_create(&home).unwrap().0))
                .collect();
            makers.into_iter().map(|m| m.join().unwrap()).collect()
        });
        assert!(keys.windows(2).all(|w| w[0] == w[1]), "one key for all");
        assert_eq!(std::fs::read(home.kek_path()).unwrap(), keys[0]);
        // An empty `secrets` directory is filled; one holding something
        // else but no key is refused, never overwritten.
        let other = tempfile::tempdir().unwrap();
        let empty = clax_core::Home::at(other.path().join("ax"));
        std::fs::create_dir_all(empty.kek_path().parent().unwrap()).unwrap();
        Kek::load_or_create(&empty).unwrap();
        std::fs::remove_file(empty.kek_path()).unwrap();
        std::fs::write(empty.kek_path().with_file_name("note"), b"x").unwrap();
        assert!(matches!(
            Kek::load_or_create(&empty),
            Err(CoreError::Invalid {
                code: "kek_unreadable",
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn leftovers_older_than_an_hour_are_swept_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("ax"));
        let kek = Kek::load_or_create(&home).unwrap();
        let root = home.root().to_path_buf();
        let leftover = |ulid: &str| {
            let d = root.join(format!(".secrets-{ulid}.tmp"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("kek"), b"partial").unwrap();
            d
        };
        let minted = clax_core::new_ulid();
        let t = chrono::DateTime::from_timestamp_millis(
            clax_core::ids::ulid_ms(&minted).unwrap() as i64
        )
        .unwrap();
        let old = leftover(&minted);
        // Not matching the exact pattern: kept.
        let odd = root.join(".secrets-dead.tmp");
        std::fs::create_dir_all(&odd).unwrap();
        let file = root.join(format!(".secrets-{}.tmp", clax_core::new_ulid()));
        std::fs::write(&file, b"a file").unwrap();
        // A symbolic link with the pattern's name to a directory elsewhere:
        // neither it nor its target is touched.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("kek"), b"theirs").unwrap();
        let link = root.join(format!(".secrets-{}.tmp", clax_core::new_ulid()));
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        // Within the hour: everything stays.
        let again = Kek::load_or_create_at(&home, t + chrono::Duration::minutes(59)).unwrap();
        assert_eq!(again.0, kek.0);
        assert!(old.exists());
        // Past it: only the leftover directory goes.
        Kek::load_or_create_at(&home, t + chrono::Duration::minutes(61)).unwrap();
        assert!(!old.exists());
        assert!(odd.exists() && file.exists());
        assert!(std::fs::symlink_metadata(&link).is_ok());
        assert_eq!(
            std::fs::read(outside.path().join("kek")).unwrap(),
            b"theirs"
        );
        assert_eq!(std::fs::read(home.kek_path()).unwrap(), kek.0);
    }

    #[test]
    fn the_home_kek_is_made_once_private_and_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("ax"));
        let a = Kek::load_or_create(&home).unwrap();
        let b = Kek::load_or_create(&home).unwrap();
        assert_eq!(a.0, b.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.kek_path()), 0o600);
            assert_eq!(mode(home.kek_path().parent().unwrap()), 0o700);
        }
        // Nothing temporary is left behind.
        let left: Vec<_> = std::fs::read_dir(home.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".secrets-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        std::fs::write(home.kek_path(), b"too short").unwrap();
        assert!(matches!(
            Kek::load_or_create(&home),
            Err(CoreError::Invalid {
                code: "kek_unreadable",
                ..
            })
        ));
    }

    #[test]
    fn a_key_goes_only_to_the_host_it_was_set_for() {
        use crate::bot_settings::Setting;
        use serde_json::json;
        let (_d, st) = store();
        let kek = Kek::from_bytes([7; 32]);
        let member = SecretSlot::Own("p_member".into());
        let meta = st.set_secret(DAEMON, &kek, &member, KEY).unwrap();
        assert_eq!(meta.host.as_deref(), Some(API));
        assert!(st.open_for_use(&kek, &member, API).unwrap().is_some());
        assert!(
            st.open_for_use(&kek, &member, "https://API.anthropic.com/")
                .unwrap()
                .is_some()
        );
        // An admin points the instance elsewhere: the member's own key is
        // kept (only the instance key is cleared) but never sent there.
        st.set_bot_settings(
            DAEMON,
            &[(
                Setting::BaseUrl,
                Some(json!("https://collector.example.com")),
            )],
        )
        .unwrap();
        let e = st
            .open_for_use(&kek, &member, "https://collector.example.com")
            .unwrap_err();
        assert!(
            matches!(&e, CoreError::Invalid { code: "key_host_mismatch", message }
                if message.contains(API)),
            "{e:?}"
        );
        let meta = st.secret_meta(&member).unwrap().unwrap();
        assert!(
            meta.host_problem("https://collector.example.com")
                .unwrap()
                .contains(API)
        );
        assert_eq!(meta.host_problem(API), None);
        // A key set again is bound to the new host.
        let meta = st.set_secret(DAEMON, &kek, &member, KEY).unwrap();
        assert_eq!(meta.host.as_deref(), Some("https://collector.example.com"));
        assert!(st.open_for_use(&kek, &member, API).is_err());
    }

    #[test]
    fn a_key_set_before_keys_were_bound_binds_at_its_first_allowed_use() {
        use crate::bot_settings::BaseUrlPolicy;
        let (_d, st) = store();
        let kek = Kek::from_bytes([7; 32]);
        st.set_secret(DAEMON, &kek, &SecretSlot::Instance, KEY)
            .unwrap();
        st.with_write(|c| {
            c.execute_batch("UPDATE sealed_secrets SET host = NULL")?;
            Ok(())
        })
        .unwrap();
        // A URL the policy refuses neither binds nor opens it.
        st.set_base_url_policy(BaseUrlPolicy::Hosted);
        assert!(matches!(
            st.open_for_use(&kek, &SecretSlot::Instance, "https://10.0.0.5"),
            Err(CoreError::Invalid {
                code: "key_host_mismatch",
                ..
            })
        ));
        let host = || st.secret_meta(&SecretSlot::Instance).unwrap().unwrap().host;
        assert_eq!(host(), None);
        // The first allowed one binds it; another is then refused.
        assert!(
            st.open_for_use(&kek, &SecretSlot::Instance, "https://proxy.example.com/v1")
                .unwrap()
                .is_some()
        );
        assert_eq!(host().as_deref(), Some("https://proxy.example.com"));
        assert!(st.open_for_use(&kek, &SecretSlot::Instance, API).is_err());
        // Its own host, under a scheme the policy refuses, is refused too.
        assert!(
            st.open_for_use(&kek, &SecretSlot::Instance, "http://proxy.example.com")
                .is_err()
        );
    }
}

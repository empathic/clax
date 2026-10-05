//! The extension's credentials (spec 2026-10-05-chrome-overlay-design §5.3).
//! Only each credential's SHA-256 is stored. A credential names no viewer:
//! every live one acts as the owner identity (spec L6).

use super::Store;
use crate::extension::{CREDENTIAL_TTL_DAYS, MAX_CREDENTIALS, credential_hash, new_credential};
use crate::{Result, new_ulid};
use rusqlite::params;

/// A credential just minted.
#[derive(Clone, Debug)]
pub struct MintedCredential {
    /// The credential itself: handed to the extension once, never stored.
    pub credential: String,
    /// Its SHA-256 in lowercase hex, as stored.
    pub hash: String,
}

/// A credential neither revoked nor unused for [`CREDENTIAL_TTL_DAYS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveCredential {
    pub hash: String,
    pub extension_id: String,
    pub last_used_at: String,
}

/// The oldest `last_used_at` still live.
fn cutoff() -> String {
    (chrono::Utc::now() - chrono::Duration::days(CREDENTIAL_TTL_DAYS))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Store {
    /// Mints a credential for `extension_id`. Past [`MAX_CREDENTIALS`] live
    /// ones for that ID, the oldest are revoked, in the same transaction.
    pub fn mint_extension_credential(&self, extension_id: &str) -> Result<MintedCredential> {
        let credential = new_credential();
        let hash = credential_hash(&credential);
        self.with_tx(|tx| {
            let now = Store::now();
            tx.execute(
                "INSERT INTO extension_credentials (id, extension_id, secret_sha256, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![new_ulid(), extension_id, hash, now],
            )?;
            tx.execute(
                "UPDATE extension_credentials SET revoked_at = ?3
                 WHERE extension_id = ?1 AND revoked_at IS NULL AND id NOT IN (
                    SELECT id FROM extension_credentials
                    WHERE extension_id = ?1 AND revoked_at IS NULL
                    ORDER BY created_at DESC, id DESC LIMIT ?2)",
                params![extension_id, MAX_CREDENTIALS as i64, now],
            )?;
            Ok(())
        })?;
        Ok(MintedCredential { credential, hash })
    }

    /// Every live credential, oldest first.
    pub fn live_extension_credentials(&self) -> Result<Vec<LiveCredential>> {
        self.with_read(|c| {
            let mut st = c.prepare(
                "SELECT secret_sha256, extension_id, last_used_at FROM extension_credentials
                 WHERE revoked_at IS NULL AND last_used_at >= ?1 ORDER BY created_at, id",
            )?;
            let rows = st
                .query_map(params![cutoff()], |r| {
                    Ok(LiveCredential {
                        hash: r.get(0)?,
                        extension_id: r.get(1)?,
                        last_used_at: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Records a use, now, of the credential whose hash is `hash`.
    pub fn touch_extension_credential(&self, hash: &str) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "UPDATE extension_credentials SET last_used_at = ?2 WHERE secret_sha256 = ?1",
                params![hash, Store::now()],
            )?;
            Ok(())
        })
    }

    /// Revokes every live credential; returns how many.
    pub fn revoke_extension_credentials(&self) -> Result<usize> {
        self.with_write(|c| {
            Ok(c.execute(
                "UPDATE extension_credentials SET revoked_at = ?1 WHERE revoked_at IS NULL",
                params![Store::now()],
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::extension::{MAX_CREDENTIALS, credential_hash};
    use crate::{Home, Store};
    use rusqlite::params;

    const EXT: &str = "bhhldgpcjhfhmcfjjnelbbdcefnocaln";

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }

    #[test]
    fn minting_past_the_cap_revokes_the_oldest_and_no_credential_carries_a_viewer() {
        let (_dir, st) = store();
        let viewers = || -> i64 {
            st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))?))
                .unwrap()
        };
        let before = viewers();
        let first = st.mint_extension_credential(EXT).unwrap();
        assert_eq!(first.hash, credential_hash(&first.credential));
        let mut last = first.clone();
        for _ in 0..MAX_CREDENTIALS {
            last = st.mint_extension_credential(EXT).unwrap();
        }
        assert_eq!(
            viewers(),
            before,
            "minting creates no viewer: the extension is the owner (spec L6)"
        );
        let live = st.live_extension_credentials().unwrap();
        assert_eq!(live.len(), MAX_CREDENTIALS);
        assert!(
            !live.iter().any(|c| c.hash == first.hash),
            "the oldest was revoked"
        );
        assert!(live.iter().any(|c| c.hash == last.hash));
        assert!(live.iter().all(|c| c.extension_id == EXT));
        let stored: String = st
            .with_read(|c| {
                let mut s = c.prepare("SELECT * FROM extension_credentials")?;
                let n = s.column_count();
                let rows = s
                    .query_map([], |r| {
                        (0..n)
                            .map(|i| r.get::<_, Option<String>>(i))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(format!("{rows:?}"))
            })
            .unwrap();
        assert!(
            !stored.contains(&last.credential[4..]),
            "only the hash is stored"
        );
        assert_eq!(st.revoke_extension_credentials().unwrap(), MAX_CREDENTIALS);
        assert!(st.live_extension_credentials().unwrap().is_empty());
        assert_eq!(st.revoke_extension_credentials().unwrap(), 0);
    }

    #[test]
    fn a_credential_unused_for_the_ttl_is_not_live_and_a_touch_records_a_use() {
        let (_dir, st) = store();
        let m = st.mint_extension_credential(EXT).unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(31))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let set = |at: &str| {
            st.with_write(|c| {
                Ok(c.execute(
                    "UPDATE extension_credentials SET last_used_at = ?2 WHERE secret_sha256 = ?1",
                    params![m.hash, at],
                )?)
            })
            .unwrap()
        };
        set(&old);
        assert!(st.live_extension_credentials().unwrap().is_empty());
        set("2000-01-01T00:00:00.000Z");
        st.touch_extension_credential(&m.hash).unwrap();
        let live = st.live_extension_credentials().unwrap();
        assert_eq!(live.len(), 1);
        assert!(live[0].last_used_at > old);
    }
}

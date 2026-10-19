//! Model use by apps (spec 2026-10-12-flutter-real-apps-design C7): the
//! owner's grant letting an app call `sample()` (`sample_grants`, migration
//! 42), kept until revoked, and what the app's calls cost each local day
//! (`sample_spend`), for the daily cost cap. Amounts are millionths of a US
//! dollar. Deleting the artifact removes its rows (a trigger on the soft
//! delete, the foreign keys on a row delete).

use super::Store;
use super::docs::caps_in;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{ArtifactId, Result};
use rusqlite::{OptionalExtension, params};

/// The `artifact.update` record of a grant change: the field
/// `sample_grant` holds `"grant"` or `"revoke"`.
fn grant_record(now: String, id: &ArtifactId, op: &str) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, now)
        .with("fields", serde_json::json!({"sample_grant": op}));
    rec.ids.artifact = Some(id.as_str().to_string());
    rec
}

impl Store {
    /// Lets `id` call `sample()` until revoked. Granting again keeps the
    /// first time; a new grant is recorded as `artifact.update` under `audit`.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn grant_sample(&self, audit: &AuditCtx, id: &ArtifactId) -> Result<String> {
        self.with_tx(|tx| {
            caps_in(tx, id)?;
            let n = tx.execute(
                "INSERT OR IGNORE INTO sample_grants (artifact_id, granted_at) VALUES (?1, ?2)",
                params![id.as_str(), self.now()],
            )?;
            if n > 0 {
                self.record_audit(tx, audit, grant_record(self.now(), id, "grant"))?;
            }
            Ok(tx.query_row(
                "SELECT granted_at FROM sample_grants WHERE artifact_id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )?)
        })
    }

    /// Removes the owner's grant to `id`; whether there was one. The day's
    /// spend stays, so revoking and granting again does not reset the cap.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn revoke_sample(&self, audit: &AuditCtx, id: &ArtifactId) -> Result<bool> {
        self.with_tx(|tx| {
            caps_in(tx, id)?;
            let n = tx.execute(
                "DELETE FROM sample_grants WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            if n > 0 {
                self.record_audit(tx, audit, grant_record(self.now(), id, "revoke"))?;
            }
            Ok(n > 0)
        })
    }

    /// When the owner let `id` call `sample()`; `None` without a grant.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn sample_grant(&self, id: &ArtifactId) -> Result<Option<String>> {
        self.with_read(|c| {
            caps_in(c, id)?;
            Ok(c.query_row(
                "SELECT granted_at FROM sample_grants WHERE artifact_id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// What `id`'s calls cost on `day` (`YYYY-MM-DD`), in micro-USD.
    ///
    /// # Errors
    /// Storage errors only; an unknown artifact has spent nothing.
    pub fn sample_spent(&self, id: &ArtifactId, day: &str) -> Result<u64> {
        self.with_read(|c| {
            let n: Option<i64> = c
                .query_row(
                    "SELECT micro_usd FROM sample_spend WHERE artifact_id = ?1 AND day = ?2",
                    params![id.as_str(), day],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(n.unwrap_or(0).max(0) as u64)
        })
    }

    /// Adds `micro_usd` to `id`'s spend on `day`; returns the day's total.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn add_sample_spend(&self, id: &ArtifactId, day: &str, micro_usd: u64) -> Result<u64> {
        let add = i64::try_from(micro_usd).unwrap_or(i64::MAX);
        self.with_tx(|tx| {
            caps_in(tx, id)?;
            tx.execute(
                "INSERT INTO sample_spend (artifact_id, day, micro_usd) VALUES (?1, ?2, ?3)
                 ON CONFLICT (artifact_id, day) DO UPDATE SET micro_usd = micro_usd + excluded.micro_usd",
                params![id.as_str(), day, add],
            )?;
            let n: i64 = tx.query_row(
                "SELECT micro_usd FROM sample_spend WHERE artifact_id = ?1 AND day = ?2",
                params![id.as_str(), day],
                |r| r.get(0),
            )?;
            Ok(n.max(0) as u64)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::test_util::{DAEMON, artifact_with_caps, store};
    use clax_core::CoreError;

    #[test]
    fn a_grant_is_kept_until_revoked_and_spend_adds_up_per_day() {
        let (_d, s) = store();
        let a = artifact_with_caps(&s, serde_json::json!({"sample": {}}));
        assert_eq!(s.sample_grant(&a).unwrap(), None);
        let at = s.grant_sample(DAEMON, &a).unwrap();
        assert_eq!(s.grant_sample(DAEMON, &a).unwrap(), at);
        assert_eq!(s.sample_grant(&a).unwrap(), Some(at));
        assert_eq!(s.add_sample_spend(&a, "2026-10-12", 300).unwrap(), 300);
        assert_eq!(s.add_sample_spend(&a, "2026-10-12", 200).unwrap(), 500);
        assert_eq!(s.sample_spent(&a, "2026-10-12").unwrap(), 500);
        assert_eq!(s.sample_spent(&a, "2026-10-13").unwrap(), 0);
        assert!(s.revoke_sample(DAEMON, &a).unwrap());
        assert!(!s.revoke_sample(DAEMON, &a).unwrap());
        assert_eq!(s.sample_grant(&a).unwrap(), None);
        assert_eq!(s.sample_spent(&a, "2026-10-12").unwrap(), 500);
    }

    #[test]
    fn deleting_the_app_removes_its_grant_and_spend() {
        let (_d, s) = store();
        let a = artifact_with_caps(&s, serde_json::json!({"sample": {}}));
        s.grant_sample(DAEMON, &a).unwrap();
        s.add_sample_spend(&a, "2026-10-12", 7).unwrap();
        s.delete_artifact(DAEMON, &a).unwrap();
        assert!(matches!(s.sample_grant(&a), Err(CoreError::NotFound)));
        assert_eq!(s.sample_spent(&a, "2026-10-12").unwrap(), 0);
    }
}

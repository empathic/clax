//! Network consent (spec 2026-10-12-flutter-real-apps-design C6): the
//! owner's grants (`net_grants`, migration 26) letting an app fetch from the
//! hosts its `capabilities.net.hosts` declares.
//!
//! A grant row names one host. A fetch may reach a host only while the
//! artifact's current declaration lists it and a row grants it, so a host a
//! later version adds is not reachable until the owner allows it, and a
//! host a version drops is unreachable whatever its row says. Deleting the
//! artifact removes its rows (a trigger on the soft delete, the foreign key
//! on a row delete).

use super::Store;
use super::docs::caps_in;
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::capabilities::net_hosts;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, params};
use serde::Serialize;

/// One host the owner let the artifact fetch from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NetGrant {
    pub host: String,
    pub granted_at: String,
}

/// An artifact's network declaration and the owner's grants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NetAccess {
    /// The hosts the current version declares, in declaration order (empty
    /// when it declares no `net`, or a declaration that does not parse).
    pub declared: Vec<String>,
    /// The owner's grants, oldest first, including hosts the current
    /// version no longer declares (they reach nothing).
    pub grants: Vec<NetGrant>,
}

impl NetAccess {
    /// Whether a fetch may reach `host`: declared now and granted.
    #[must_use]
    pub fn allows(&self, host: &str) -> bool {
        self.declared.iter().any(|h| h == host) && self.grants.iter().any(|g| g.host == host)
    }

    /// Whether every declared host is granted (and at least one is declared).
    #[must_use]
    pub fn all_granted(&self) -> bool {
        !self.declared.is_empty() && self.declared.iter().all(|h| self.allows(h))
    }
}

/// The `artifact.update` record of a grant change: the field `net_grants`
/// holds `{"grant": [hosts]}` or `{"revoke": [hosts]}`.
fn grant_record(id: &ArtifactId, op: &str, hosts: &[String]) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, Store::now())
        .with("fields", serde_json::json!({"net_grants": {op: hosts}}));
    rec.ids.artifact = Some(id.as_str().to_string());
    rec
}

fn access_in(c: &Connection, id: &ArtifactId) -> Result<NetAccess> {
    let caps = caps_in(c, id)?;
    let declared = net_hosts(&caps).ok().flatten().unwrap_or_default();
    let mut stmt = c.prepare(
        "SELECT host, granted_at FROM net_grants WHERE artifact_id = ?1 ORDER BY granted_at, host",
    )?;
    let grants = stmt
        .query_map(params![id.as_str()], |r| {
            Ok(NetGrant {
                host: r.get(0)?,
                granted_at: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(NetAccess { declared, grants })
}

impl Store {
    /// Grants every host the artifact's current version declares in
    /// `capabilities.net.hosts`; hosts already granted keep their time. The
    /// new grants are recorded as one `artifact.update` under `audit`.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted; `not_declared`
    /// (invalid) when it declares no `net`.
    pub fn grant_net(&self, audit: &AuditCtx, id: &ArtifactId) -> Result<NetAccess> {
        self.with_tx(|tx| {
            let before = access_in(tx, id)?;
            if before.declared.is_empty() {
                return Err(CoreError::invalid(
                    "not_declared",
                    "this artifact declares no network hosts (capabilities.net.hosts)",
                ));
            }
            let now = Store::now();
            let mut added = Vec::new();
            for h in &before.declared {
                let n = tx.execute(
                    "INSERT OR IGNORE INTO net_grants (artifact_id, host, granted_at) VALUES (?1, ?2, ?3)",
                    params![id.as_str(), h, now],
                )?;
                if n > 0 {
                    added.push(h.clone());
                }
            }
            if !added.is_empty() {
                self.record_audit(tx, audit, grant_record(id, "grant", &added))?;
            }
            access_in(tx, id)
        })
    }

    /// Removes every network grant of the artifact; how many there were.
    /// A removal is recorded as `artifact.update` under `audit`.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn revoke_net(&self, audit: &AuditCtx, id: &ArtifactId) -> Result<usize> {
        self.with_tx(|tx| {
            let before = access_in(tx, id)?;
            let n = tx.execute(
                "DELETE FROM net_grants WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            if n > 0 {
                let hosts: Vec<String> = before.grants.into_iter().map(|g| g.host).collect();
                self.record_audit(tx, audit, grant_record(id, "revoke", &hosts))?;
            }
            Ok(n)
        })
    }

    /// The artifact's declared hosts and the owner's network grants.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn net_access(&self, id: &ArtifactId) -> Result<NetAccess> {
        self.with_read(|c| access_in(c, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetaPatch;
    use crate::store::test_util::{DAEMON, artifact_with_caps, store};
    use serde_json::{Value, json};

    fn set_caps(st: &Store, id: &ArtifactId, caps: Value) {
        st.update_meta(
            DAEMON,
            id,
            MetaPatch {
                capabilities: Some(caps),
                ..MetaPatch::default()
            },
        )
        .unwrap();
    }

    fn rows(st: &Store) -> i64 {
        st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM net_grants", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn a_host_is_allowed_while_declared_and_granted() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({"net": {"hosts": ["api.example.com"]}}));
        let acc = st.net_access(&a).unwrap();
        assert_eq!(acc.declared, vec!["api.example.com"]);
        assert!(!acc.allows("api.example.com") && !acc.all_granted());

        let acc = st.grant_net(DAEMON, &a).unwrap();
        assert!(acc.allows("api.example.com") && acc.all_granted());
        assert!(!acc.allows("other.example.com"));
        st.grant_net(DAEMON, &a).unwrap();
        assert_eq!(rows(&st), 1, "granting again changes nothing");

        set_caps(
            &st,
            &a,
            json!({"net": {"hosts": ["api.example.com", "new.example.com"]}}),
        );
        let acc = st.net_access(&a).unwrap();
        assert!(acc.allows("api.example.com"));
        assert!(
            !acc.allows("new.example.com"),
            "a newly declared host waits for consent"
        );
        assert!(!acc.all_granted());

        set_caps(&st, &a, json!({"db": {}}));
        let acc = st.net_access(&a).unwrap();
        assert!(
            !acc.allows("api.example.com"),
            "a dropped declaration reaches nothing"
        );
        assert!(matches!(
            st.grant_net(DAEMON, &a),
            Err(CoreError::Invalid {
                code: "not_declared",
                ..
            })
        ));

        set_caps(&st, &a, json!({"net": {"hosts": ["api.example.com"]}}));
        assert_eq!(st.revoke_net(DAEMON, &a).unwrap(), 1);
        assert_eq!(st.revoke_net(DAEMON, &a).unwrap(), 0);
        assert!(!st.net_access(&a).unwrap().allows("api.example.com"));
    }

    #[test]
    fn grants_go_with_the_artifact_and_are_audited() {
        let (_d, st) = store();
        let a = artifact_with_caps(&st, json!({"net": {"hosts": ["api.example.com"]}}));
        st.grant_net(DAEMON, &a).unwrap();
        st.revoke_net(DAEMON, &a).unwrap();
        let bodies: Vec<String> = st
            .with_read(|c| {
                let mut s = c.prepare(
                    "SELECT body FROM audit_events WHERE kind = 'artifact.update' AND artifact_id = ?1 ORDER BY seq",
                )?;
                let r = s.query_map(params![a.as_str()], |r| r.get(0))?;
                Ok(r.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        let ops: Vec<Value> = bodies
            .iter()
            .map(|b| serde_json::from_str::<Value>(b).unwrap()["fields"]["net_grants"].clone())
            .filter(|v| !v.is_null())
            .collect();
        assert_eq!(
            ops,
            vec![
                json!({"grant": ["api.example.com"]}),
                json!({"revoke": ["api.example.com"]})
            ]
        );

        st.grant_net(DAEMON, &a).unwrap();
        st.delete_artifact(DAEMON, &a).unwrap();
        assert_eq!(rows(&st), 0);
        assert!(matches!(st.net_access(&a), Err(CoreError::NotFound)));
        assert!(matches!(st.grant_net(DAEMON, &a), Err(CoreError::NotFound)));
    }
}

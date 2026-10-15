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
use rusqlite::{Connection, OptionalExtension, params};
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

/// The app whose grants the draft `id` uses: the one it is a draft of,
/// unless it was made offline (a new app's draft is of nothing).
fn live_app_in(c: &Connection, id: &ArtifactId) -> Result<Option<ArtifactId>> {
    let of: Option<String> = c
        .query_row(
            "SELECT draft_of FROM artifacts WHERE id = ?1 AND hidden = 1 AND draft_inherit_net = 1",
            params![id.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(of.and_then(|s| ArtifactId::parse(&s).ok()))
}

/// The artifact's access. A draft (spec 2026-10-12-flutter-app-round2-design
/// §5.1) has no grants of its own: its `grants` are its live app's grants
/// for the hosts that app's current version declares, so a host is allowed
/// only while the draft declares it and the owner's grant to the app stands.
/// A new app's draft, a draft made offline ([`Store::create_draft_with`]),
/// or a draft whose app is gone, has none.
fn access_in(c: &Connection, id: &ArtifactId) -> Result<NetAccess> {
    if !super::drafts::is_hidden_in(c, id.as_str())? {
        return own_access_in(c, id);
    }
    let declared = net_hosts(&caps_in(c, id)?)
        .ok()
        .flatten()
        .unwrap_or_default();
    let grants = match live_app_in(c, id)? {
        Some(app) => match own_access_in(c, &app) {
            Ok(live) => live
                .grants
                .iter()
                .filter(|g| live.allows(&g.host))
                .cloned()
                .collect(),
            Err(CoreError::NotFound) => Vec::new(),
            Err(e) => return Err(e),
        },
        None => Vec::new(),
    };
    Ok(NetAccess { declared, grants })
}

fn refuse_draft(c: &Connection, id: &ArtifactId) -> Result<()> {
    if super::drafts::is_hidden_in(c, id.as_str())? {
        return Err(CoreError::invalid(
            "draft",
            "a draft takes no network grants; it uses its app's",
        ));
    }
    Ok(())
}

/// `id`'s own declaration and grant rows.
fn own_access_in(c: &Connection, id: &ArtifactId) -> Result<NetAccess> {
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
    /// Grants each host of `hosts` (the list the owner was shown) that the
    /// artifact's current version declares in `capabilities.net.hosts`;
    /// other hosts are ignored, so a host the owner did not see is never
    /// granted. Hosts already granted keep their time. The new grants are
    /// recorded as one `artifact.update` under `audit`.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted; `not_declared`
    /// (invalid) when it declares no `net`; `draft` (invalid) for a draft,
    /// which uses its app's grants and is never granted itself.
    pub fn grant_net(
        &self,
        audit: &AuditCtx,
        id: &ArtifactId,
        hosts: &[String],
    ) -> Result<NetAccess> {
        self.with_tx(|tx| {
            refuse_draft(tx, id)?;
            let before = access_in(tx, id)?;
            if before.declared.is_empty() {
                return Err(CoreError::invalid(
                    "not_declared",
                    "this artifact declares no network hosts (capabilities.net.hosts)",
                ));
            }
            let now = Store::now();
            let mut added = Vec::new();
            for h in before.declared.iter().filter(|h| hosts.contains(h)) {
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
    /// `NotFound` when the artifact is missing or deleted; `draft` (invalid)
    /// for a draft, which holds no grants (revoke its app's).
    pub fn revoke_net(&self, audit: &AuditCtx, id: &ArtifactId) -> Result<usize> {
        self.with_tx(|tx| {
            caps_in(tx, id)?;
            refuse_draft(tx, id)?;
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

    /// The artifact's declared hosts and the owner's network grants (for a
    /// draft, its live app's grants on hosts that app declares).
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

    fn hosts(h: &[&str]) -> Vec<String> {
        h.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_the_shown_hosts_that_are_declared_are_granted() {
        let (_d, st) = store();
        let a = artifact_with_caps(
            &st,
            json!({"net": {"hosts": ["a.example.com", "b.example.com"]}}),
        );
        let acc = st
            .grant_net(DAEMON, &a, &hosts(&["a.example.com", "x.example.com"]))
            .unwrap();
        assert!(acc.allows("a.example.com"));
        assert!(!acc.allows("b.example.com"), "not shown, not granted");
        assert!(!acc.all_granted());
        assert_eq!(acc.grants.len(), 1, "an undeclared host is never stored");
        let acc = st.grant_net(DAEMON, &a, &hosts(&[])).unwrap();
        assert_eq!(acc.grants.len(), 1);
    }

    fn draft(st: &Store, of: Option<&ArtifactId>, caps: Value) -> ArtifactId {
        let req = serde_json::from_value(json!({
            "title": "Draft",
            "capabilities": caps,
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let d = st
            .create_draft(DAEMON, of, crate::publish::validate(req).unwrap())
            .unwrap();
        ArtifactId::parse(&d.artifact.id).unwrap()
    }

    #[test]
    fn a_draft_uses_its_live_apps_grants_for_hosts_both_declare() {
        let (_d, st) = store();
        let app = artifact_with_caps(
            &st,
            json!({"net": {"hosts": ["a.example.com", "b.example.com"]}}),
        );
        st.grant_net(DAEMON, &app, &hosts(&["a.example.com"]))
            .unwrap();
        let caps = json!({"net": {"hosts": ["a.example.com", "c.example.com"]}});
        let d = draft(&st, Some(&app), caps.clone());
        let acc = st.net_access(&d).unwrap();
        assert!(acc.allows("a.example.com"), "the app's grant carries over");
        assert!(
            !acc.allows("b.example.com"),
            "the draft does not declare it"
        );
        assert!(!acc.allows("c.example.com"), "the app has no grant for it");

        let narrow = draft(
            &st,
            Some(&app),
            json!({"net": {"hosts": ["b.example.com"]}}),
        );
        assert!(!st.net_access(&narrow).unwrap().allows("a.example.com"));

        set_caps(&st, &app, json!({"net": {"hosts": ["b.example.com"]}}));
        assert!(
            !st.net_access(&d).unwrap().allows("a.example.com"),
            "a host the app no longer declares reaches nothing"
        );
        set_caps(&st, &app, json!({"net": {"hosts": ["a.example.com"]}}));
        assert!(st.net_access(&d).unwrap().allows("a.example.com"));

        st.revoke_net(DAEMON, &app).unwrap();
        assert!(
            !st.net_access(&d).unwrap().allows("a.example.com"),
            "a revocation reaches the draft on its next check"
        );

        let fresh = draft(&st, None, caps);
        assert!(st.net_access(&fresh).unwrap().grants.is_empty());
    }

    #[test]
    fn an_offline_draft_or_one_whose_app_is_gone_reaches_nothing() {
        let (_d, st) = store();
        let app = artifact_with_caps(&st, json!({"net": {"hosts": ["a.example.com"]}}));
        st.grant_net(DAEMON, &app, &hosts(&["a.example.com"]))
            .unwrap();
        let caps = json!({"net": {"hosts": ["a.example.com"]}});
        let req = serde_json::from_value(json!({
            "title": "Check",
            "capabilities": caps,
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let d = st
            .create_draft_with(
                DAEMON,
                Some(&app),
                crate::publish::validate(req).unwrap(),
                false,
            )
            .unwrap();
        let offline = ArtifactId::parse(&d.artifact.id).unwrap();
        assert!(!st.net_access(&offline).unwrap().allows("a.example.com"));

        let inherits = draft(&st, Some(&app), caps);
        assert!(st.net_access(&inherits).unwrap().allows("a.example.com"));
        st.delete_artifact(DAEMON, &app).unwrap();
        let acc = st.net_access(&inherits).unwrap();
        assert!(acc.grants.is_empty() && !acc.allows("a.example.com"));
    }

    #[test]
    fn a_draft_is_never_granted_and_reads_no_other_app() {
        let (_d, st) = store();
        let app = artifact_with_caps(&st, json!({"net": {"hosts": ["a.example.com"]}}));
        let caps = json!({"net": {"hosts": ["a.example.com"]}});
        for d in [
            draft(&st, Some(&app), caps.clone()),
            draft(&st, None, caps.clone()),
        ] {
            for r in [
                st.grant_net(DAEMON, &d, &hosts(&["a.example.com"]))
                    .map(|_| ()),
                st.revoke_net(DAEMON, &d).map(|_| ()),
            ] {
                assert!(matches!(r, Err(CoreError::Invalid { code: "draft", .. })));
            }
        }
        assert_eq!(rows(&st), 0, "no grant rows were made");
        assert!(!st.net_access(&app).unwrap().allows("a.example.com"));

        let source = artifact_with_caps(&st, json!({"db": {}}));
        let reader_caps = json!({"db": {"read": [source.as_str()]}});
        let reader = artifact_with_caps(&st, reader_caps.clone());
        st.grant_read(DAEMON, &reader, &source).unwrap();
        let d = draft(&st, Some(&reader), reader_caps);
        assert_eq!(
            st.may_read(&d, &source).unwrap(),
            crate::store::grants::ReadAccess::NotGranted,
            "cross-app reads stay refused in a draft"
        );
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

        let acc = st
            .grant_net(DAEMON, &a, &hosts(&["api.example.com", "new.example.com"]))
            .unwrap();
        assert!(acc.allows("api.example.com") && acc.all_granted());
        assert!(!acc.allows("other.example.com"));
        st.grant_net(DAEMON, &a, &hosts(&["api.example.com", "new.example.com"]))
            .unwrap();
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
            st.grant_net(DAEMON, &a, &hosts(&["api.example.com", "new.example.com"])),
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
        st.grant_net(DAEMON, &a, &hosts(&["api.example.com", "new.example.com"]))
            .unwrap();
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

        st.grant_net(DAEMON, &a, &hosts(&["api.example.com", "new.example.com"]))
            .unwrap();
        st.delete_artifact(DAEMON, &a).unwrap();
        assert_eq!(rows(&st), 0);
        assert!(matches!(st.net_access(&a), Err(CoreError::NotFound)));
        assert!(matches!(
            st.grant_net(DAEMON, &a, &hosts(&["api.example.com", "new.example.com"])),
            Err(CoreError::NotFound)
        ));
    }
}

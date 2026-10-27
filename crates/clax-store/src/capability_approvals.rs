//! Capability approvals: the declaration each artifact's administrator last
//! approved (`capability_approvals`, migration 58), one row per artifact,
//! and the declaration in force that follows from it.
//!
//! A declaration's [`GATED`] names reach past the page, so a name or `net`
//! host that the approved declaration lacks waits for approval
//! ([`clax_core::capabilities::pending`]); with no approval on record the
//! approved declaration is `{}`, so every first declaration of a gated name
//! waits. Until it is approved, the declaration in force is the current
//! one without what waits ([`clax_core::capabilities::in_force`]). A hidden
//! draft is held to its live app's approval and is never approved itself.
//! Deleting the artifact removes its row (a trigger on the soft delete,
//! the foreign key on a row delete); each approval and withdrawal is
//! recorded in the audit journal.

use super::Store;
use super::docs::caps_in;
use super::drafts::is_hidden_in;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::capabilities::{Added, GATED, in_force, pending};
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

/// The declaration an artifact's administrator last approved.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Approval {
    /// The version current when it was approved.
    pub version: u32,
    /// The declaration approved, less any names withdrawn since.
    pub capabilities: Value,
    pub approved_at: String,
    /// `owner`, `person:<ID>`, or `migration` for a declaration in force
    /// before approvals were kept.
    pub by: String,
}

/// What the current version declares that waits for approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pending {
    /// The current version, the one an approval names.
    pub version: u32,
    #[serde(flatten)]
    pub added: Added,
}

/// `id`'s own approval row.
fn approval_in(c: &Connection, id: &ArtifactId) -> Result<Option<Approval>> {
    let row: Option<(u32, String, String, String)> = c
        .query_row(
            "SELECT version_n, capabilities_json, approved_at, by FROM capability_approvals WHERE artifact_id = ?1",
            params![id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    row.map(|(version, raw, approved_at, by)| {
        let capabilities = serde_json::from_str(&raw).map_err(|_| CoreError::Corrupt {
            artifact_id: id.as_str().to_string(),
            column: "capability_approvals.capabilities_json",
            version: None,
        })?;
        Ok(Approval {
            version,
            capabilities,
            approved_at,
            by,
        })
    })
    .transpose()
}

/// The declaration that holds `id`'s gated names: its own approval's, or
/// for a hidden draft its live app's (`None` for a new app's draft, one
/// made offline, or one whose app is gone).
fn approved_in(c: &Connection, id: &ArtifactId) -> Result<Option<Value>> {
    let holder = if is_hidden_in(c, id.as_str())? {
        super::net_grants::live_app_in(c, id)?
    } else {
        Some(id.clone())
    };
    Ok(match holder {
        Some(h) => approval_in(c, &h)?.map(|a| a.capabilities),
        None => None,
    })
}

/// The live artifact's current version; `NotFound` when it is missing or
/// deleted.
fn current_version_in(c: &Connection, id: &ArtifactId) -> Result<u32> {
    c.query_row(
        "SELECT current_version FROM artifacts WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0",
        params![id.as_str()],
        |r| r.get(0),
    )
    .optional()?
    .ok_or(CoreError::NotFound)
}

/// [`Store::capabilities_in_force`] on `c`.
pub(crate) fn in_force_in(c: &Connection, id: &ArtifactId) -> Result<Value> {
    let caps = caps_in(c, id)?;
    Ok(in_force(approved_in(c, id)?.as_ref(), &caps))
}

/// [`Store::pending_capabilities`] on `c`.
fn pending_in(c: &Connection, id: &ArtifactId) -> Result<Option<Pending>> {
    let caps = caps_in(c, id)?;
    let added = pending(approved_in(c, id)?.as_ref(), &caps);
    if added.is_empty() {
        return Ok(None);
    }
    Ok(Some(Pending {
        version: current_version_in(c, id)?,
        added,
    }))
}

/// Records `caps` as `id`'s approved declaration at `version`, replacing
/// any earlier approval; nothing is granted or audited.
pub(crate) fn record_in(
    c: &Connection,
    id: &ArtifactId,
    version: u32,
    caps: &Value,
    by: &str,
    now: &str,
) -> Result<Approval> {
    c.execute(
        "INSERT INTO capability_approvals (artifact_id, version_n, capabilities_json, approved_at, by)
            VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (artifact_id) DO UPDATE SET version_n = excluded.version_n,
            capabilities_json = excluded.capabilities_json,
            approved_at = excluded.approved_at, by = excluded.by",
        params![id.as_str(), version, caps.to_string(), now, by],
    )?;
    Ok(Approval {
        version,
        capabilities: caps.clone(),
        approved_at: now.to_string(),
        by: by.to_string(),
    })
}

/// The `artifact.update` record of an approval change, with `fields`.
fn approval_record(now: String, id: &ArtifactId, fields: Value) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ArtifactUpdate, now).with("fields", fields);
    rec.ids.artifact = Some(id.as_str().to_string());
    rec
}

impl Store {
    /// The declaration `id`'s administrator last approved; `None` when
    /// none is on record (a hidden draft never has one).
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn capability_approval(&self, id: &ArtifactId) -> Result<Option<Approval>> {
        self.with_read(|c| {
            caps_in(c, id)?;
            approval_in(c, id)
        })
    }

    /// The declaration in force: the current declaration without the
    /// gated names and `net` hosts that wait for approval. A hidden draft's
    /// declaration is held to its live app's approval.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn capabilities_in_force(&self, id: &ArtifactId) -> Result<Value> {
        self.with_read(|c| in_force_in(c, id))
    }

    /// What the current version declares that waits for approval; `None`
    /// when nothing does.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn pending_capabilities(&self, id: &ArtifactId) -> Result<Option<Pending>> {
        self.with_read(|c| pending_in(c, id))
    }

    /// Approves `id`'s current declaration as of `version`, which must be
    /// the current version, so a version published since is never approved
    /// unseen. In one transaction it records the declaration, grants the
    /// `net` hosts that waited and, when `sample` waited, the sample grant,
    /// and records one `artifact.update` under `audit` whose fields hold
    /// `capability_approval` and any `net_grants` and `sample_grant` made.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted; `draft`
    /// (invalid) for a hidden draft, held to its app's approval;
    /// `stale_version` (invalid) when `version` is not the current one.
    pub fn approve_capabilities(
        &self,
        audit: &AuditCtx,
        id: &ArtifactId,
        version: u32,
        by: &str,
    ) -> Result<Approval> {
        self.with_tx(|tx| {
            let caps = caps_in(tx, id)?;
            if is_hidden_in(tx, id.as_str())? {
                return Err(CoreError::invalid(
                    "draft",
                    "a draft is held to its app's approval; approve the app's version",
                ));
            }
            let current = current_version_in(tx, id)?;
            if version != current {
                return Err(CoreError::invalid(
                    "stale_version",
                    format!(
                        "version {version} is not the current version; review version {current}"
                    ),
                ));
            }
            let before = approval_in(tx, id)?.map(|a| a.capabilities);
            let waited = pending(before.as_ref(), &caps);
            let now = self.now();
            let approval = record_in(tx, id, version, &caps, by, &now)?;
            let mut fields = json!({"capability_approval": {
                "version": version, "by": by, "capabilities": caps,
            }});
            let mut granted = Vec::new();
            for h in &waited.hosts {
                let n = tx.execute(
                    "INSERT OR IGNORE INTO net_grants (artifact_id, host, granted_at) VALUES (?1, ?2, ?3)",
                    params![id.as_str(), h, now],
                )?;
                if n > 0 {
                    granted.push(h.clone());
                }
            }
            if !granted.is_empty() {
                fields["net_grants"] = json!({"grant": granted});
            }
            if waited.capabilities.iter().any(|c| c == "sample")
                && tx.execute(
                    "INSERT OR IGNORE INTO sample_grants (artifact_id, granted_at) VALUES (?1, ?2)",
                    params![id.as_str(), now],
                )? > 0
            {
                fields["sample_grant"] = json!("grant");
            }
            self.record_audit(tx, audit, approval_record(self.now(), id, fields))?;
            Ok(approval)
        })
    }

    /// Takes `name` out of `id`'s approved declaration, so the current
    /// version's declaration of it waits for approval again; whether it was
    /// approved. A withdrawal is recorded as `artifact.update` under
    /// `audit`. Grants made on approval stay: revoke them on their own.
    ///
    /// # Errors
    /// `invalid_args` (invalid) when `name` is not one of [`GATED`];
    /// `NotFound` when the artifact is missing or deleted.
    pub fn withdraw_capability(
        &self,
        audit: &AuditCtx,
        id: &ArtifactId,
        name: &str,
    ) -> Result<bool> {
        if !GATED.contains(&name) {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("{name} needs no approval, so it has none to withdraw"),
            ));
        }
        self.with_tx(|tx| {
            caps_in(tx, id)?;
            let Some(mut approval) = approval_in(tx, id)? else {
                return Ok(false);
            };
            let removed = approval
                .capabilities
                .as_object_mut()
                .is_some_and(|m| m.remove(name).is_some());
            if !removed {
                return Ok(false);
            }
            tx.execute(
                "UPDATE capability_approvals SET capabilities_json = ?2 WHERE artifact_id = ?1",
                params![id.as_str(), approval.capabilities.to_string()],
            )?;
            let fields = json!({"capability_approval": {"withdraw": name}});
            self.record_audit(tx, audit, approval_record(self.now(), id, fields))?;
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, artifact_declaring, artifact_with_caps, store};
    use clax_core::capabilities::Added;
    use serde_json::{Value, json};

    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    /// Publishes the next version of `id` (after `if_version`) declaring `caps`.
    fn publish(st: &Store, id: &ArtifactId, if_version: u32, caps: Value) {
        let req = serde_json::from_value(json!({
            "if_version": if_version,
            "capabilities": caps,
            "files": {"index.html": {"content": format!("<p>{if_version}</p>"), "encoding": "utf8"}}
        }))
        .unwrap();
        st.publish_version(DAEMON, id, clax_core::publish::validate(req).unwrap(), None)
            .unwrap();
    }

    fn rows(st: &Store) -> i64 {
        st.with_read(|c| {
            Ok(
                c.query_row("SELECT COUNT(*) FROM capability_approvals", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap()
    }

    #[test]
    fn a_first_declaration_waits_for_approval() {
        let (_d, st) = store();
        let a = artifact_declaring(&st, json!({"db": {}, "net": {"hosts": ["a.example.com"]}}));
        assert_eq!(st.capabilities_in_force(&a).unwrap(), json!({"db": {}}));
        assert_eq!(
            st.pending_capabilities(&a).unwrap(),
            Some(Pending {
                version: 1,
                added: Added {
                    capabilities: strings(&["net"]),
                    hosts: strings(&["a.example.com"]),
                },
            })
        );
        assert_eq!(st.capability_approval(&a).unwrap(), None);
        let acc = st.net_access(&a).unwrap();
        assert!(acc.declared.is_empty());
        assert_eq!(acc.waiting, strings(&["a.example.com"]));

        let plain = artifact_declaring(&st, json!({"db": {}, "comments": {}}));
        assert_eq!(
            st.pending_capabilities(&plain).unwrap(),
            None,
            "nothing gated, nothing to approve"
        );
        assert_eq!(
            st.capabilities_in_force(&plain).unwrap(),
            json!({"db": {}, "comments": {}})
        );
    }

    #[test]
    fn approving_puts_it_in_force_and_grants_its_hosts_and_sample() {
        let (_d, st) = store();
        let caps = json!({"net": {"hosts": ["a.example.com"]}, "sample": {}});
        let a = artifact_declaring(&st, caps.clone());
        let approval = st.approve_capabilities(DAEMON, &a, 1, "owner").unwrap();
        assert_eq!(
            (
                approval.version,
                &approval.capabilities,
                approval.by.as_str()
            ),
            (1, &caps, "owner")
        );
        assert_eq!(st.capability_approval(&a).unwrap(), Some(approval));
        assert_eq!(st.capabilities_in_force(&a).unwrap(), caps);
        assert_eq!(st.pending_capabilities(&a).unwrap(), None);
        let acc = st.net_access(&a).unwrap();
        assert!(acc.allows("a.example.com"));
        assert!(acc.waiting.is_empty());
        assert!(st.sample_grant(&a).unwrap().is_some());

        let bodies: Vec<String> = st
            .with_read(|c| {
                let mut s = c.prepare(
                    "SELECT body FROM audit_events WHERE kind = 'artifact.update' AND artifact_id = ?1 ORDER BY seq",
                )?;
                let r = s.query_map(params![a.as_str()], |r| r.get::<_, String>(0))?;
                Ok(r.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        let fields: Vec<Value> = bodies
            .iter()
            .map(|b| serde_json::from_str::<Value>(b).unwrap()["fields"].clone())
            .collect();
        assert_eq!(fields.len(), 1, "one entry: {fields:?}");
        assert_eq!(
            fields[0],
            json!({
                "capability_approval": {"version": 1, "by": "owner", "capabilities": caps},
                "net_grants": {"grant": ["a.example.com"]},
                "sample_grant": "grant"
            })
        );
    }

    #[test]
    fn approving_a_version_that_is_not_current_is_refused() {
        let (_d, st) = store();
        let a = artifact_declaring(&st, json!({"sample": {}}));
        publish(&st, &a, 1, json!({"sample": {}, "shell": {}}));
        assert!(matches!(
            st.approve_capabilities(DAEMON, &a, 1, "owner"),
            Err(CoreError::Invalid {
                code: "stale_version",
                ..
            })
        ));
        assert_eq!(rows(&st), 0, "nothing recorded");
        assert!(st.sample_grant(&a).unwrap().is_none(), "nothing granted");
        assert_eq!(st.pending_capabilities(&a).unwrap().unwrap().version, 2);
    }

    #[test]
    fn a_version_that_adds_a_host_keeps_the_approved_ones() {
        let (_d, st) = store();
        let a = artifact_declaring(&st, json!({"net": {"hosts": ["a.example.com"]}}));
        st.approve_capabilities(DAEMON, &a, 1, "owner").unwrap();
        publish(
            &st,
            &a,
            1,
            json!({"net": {"hosts": ["a.example.com", "b.example.com"]}}),
        );
        assert_eq!(
            st.capabilities_in_force(&a).unwrap(),
            json!({"net": {"hosts": ["a.example.com"]}})
        );
        let acc = st.net_access(&a).unwrap();
        assert_eq!(acc.declared, strings(&["a.example.com"]));
        assert_eq!(acc.waiting, strings(&["b.example.com"]));
        assert!(acc.allows("a.example.com") && !acc.allows("b.example.com"));
        assert_eq!(
            st.pending_capabilities(&a).unwrap(),
            Some(Pending {
                version: 2,
                added: Added {
                    capabilities: vec![],
                    hosts: strings(&["b.example.com"]),
                },
            })
        );
    }

    #[test]
    fn a_restore_of_a_dropped_host_asks_again() {
        let (_d, st) = store();
        let a = artifact_declaring(&st, json!({"net": {"hosts": ["a.example.com"]}}));
        st.approve_capabilities(DAEMON, &a, 1, "owner").unwrap();
        publish(&st, &a, 1, json!({"net": {"hosts": ["b.example.com"]}}));
        st.approve_capabilities(DAEMON, &a, 2, "owner").unwrap();
        st.restore_version(DAEMON, &a, 1, 2, None).unwrap();
        let p = st.pending_capabilities(&a).unwrap().unwrap();
        assert_eq!((p.version, p.added.hosts), (3, strings(&["a.example.com"])));
        assert!(p.added.capabilities.is_empty());
        assert_eq!(st.capabilities_in_force(&a).unwrap(), json!({}));
        assert!(!st.net_access(&a).unwrap().allows("a.example.com"));
    }

    #[test]
    fn withdrawing_a_name_narrows_the_approval() {
        let (_d, st) = store();
        let a = artifact_declaring(&st, json!({"shell": {}, "sample": {}}));
        st.approve_capabilities(DAEMON, &a, 1, "owner").unwrap();
        assert!(st.withdraw_capability(DAEMON, &a, "shell").unwrap());
        assert_eq!(st.capabilities_in_force(&a).unwrap(), json!({"sample": {}}));
        assert_eq!(
            st.capability_approval(&a).unwrap().unwrap().capabilities,
            json!({"sample": {}})
        );
        assert_eq!(
            st.pending_capabilities(&a)
                .unwrap()
                .unwrap()
                .added
                .capabilities,
            strings(&["shell"])
        );
        assert!(!st.withdraw_capability(DAEMON, &a, "shell").unwrap());
        assert!(!st.withdraw_capability(DAEMON, &a, "camera").unwrap());
        assert!(matches!(
            st.withdraw_capability(DAEMON, &a, "db"),
            Err(CoreError::Invalid {
                code: "invalid_args",
                ..
            })
        ));
        let unapproved = artifact_declaring(&st, json!({"shell": {}}));
        assert!(
            !st.withdraw_capability(DAEMON, &unapproved, "shell")
                .unwrap()
        );
    }

    fn draft(st: &Store, of: &ArtifactId, caps: Value) -> (ArtifactId, u32) {
        let req = serde_json::from_value(json!({
            "title": "Draft",
            "capabilities": caps,
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let d = st
            .create_draft(DAEMON, Some(of), clax_core::publish::validate(req).unwrap())
            .unwrap();
        (
            ArtifactId::parse(&d.artifact.id).unwrap(),
            d.artifact.current_version,
        )
    }

    #[test]
    fn a_draft_is_held_to_its_live_apps_approval() {
        let (_d, st) = store();
        let app = artifact_with_caps(&st, json!({"net": {"hosts": ["a.example.com"]}}));
        let (d, n) = draft(
            &st,
            &app,
            json!({"db": {}, "sample": {}, "net": {"hosts": ["a.example.com", "b.example.com"]}}),
        );
        assert_eq!(
            st.capabilities_in_force(&d).unwrap(),
            json!({"db": {}, "net": {"hosts": ["a.example.com"]}})
        );
        let acc = st.net_access(&d).unwrap();
        assert_eq!(acc.declared, strings(&["a.example.com"]));
        assert_eq!(acc.waiting, strings(&["b.example.com"]));
        let p = st.pending_capabilities(&d).unwrap().unwrap();
        assert_eq!(
            (p.added.capabilities, p.added.hosts),
            (strings(&["sample"]), strings(&["b.example.com"]))
        );
        let before = rows(&st);
        assert!(matches!(
            st.approve_capabilities(DAEMON, &d, n, "owner"),
            Err(CoreError::Invalid { code: "draft", .. })
        ));
        assert_eq!(rows(&st), before, "nothing recorded");
        assert!(st.sample_grant(&d).unwrap().is_none());
    }

    #[test]
    fn deleting_the_artifact_removes_its_approval() {
        let (_d, st) = store();
        let a = artifact_declaring(&st, json!({"sample": {}}));
        let kept = artifact_declaring(&st, json!({"sample": {}}));
        st.approve_capabilities(DAEMON, &a, 1, "owner").unwrap();
        st.approve_capabilities(DAEMON, &kept, 1, "owner").unwrap();
        st.delete_artifact(DAEMON, &a).unwrap();
        assert_eq!(rows(&st), 1);
        for r in [
            st.capability_approval(&a).map(|_| ()),
            st.capabilities_in_force(&a).map(|_| ()),
            st.pending_capabilities(&a).map(|_| ()),
            st.approve_capabilities(DAEMON, &a, 1, "owner").map(|_| ()),
        ] {
            assert!(matches!(r, Err(CoreError::NotFound)), "{r:?}");
        }
    }
}

//! A queue's installs ([`clax_core::install`]): the sites that embed the
//! overlay and file into the queue, each with the config its overlay
//! fetches. Their changes are recorded as `queue.update` with `op`
//! `install_create`, `install_update` or `install_delete`.

use super::{QueueRow, normalize_origins, queue_in, queue_record};
use crate::Store;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_core::install::{
    Install, InstallConfig, MAX_QUEUE_INSTALLS, check_install_config, check_install_name,
};
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, params};

/// An install to make ([`Store::create_install`]).
#[derive(Clone, Debug)]
pub struct NewInstall {
    pub name: String,
    /// Origins as given; each is normalized.
    pub origins: Vec<String>,
    pub config: InstallConfig,
}

/// A change to an install ([`Store::update_install`]): each field given is
/// set; `config` is set whole.
#[derive(Clone, Debug, Default)]
pub struct InstallPatch {
    pub name: Option<String>,
    pub origins: Option<Vec<String>>,
    pub config: Option<InstallConfig>,
}

const INSTALL_SELECT: &str = "SELECT id, queue_id, name, origins_json, config_json, created_at, updated_at
     FROM installs";

fn install_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Install> {
    let origins: String = r.get(3)?;
    let config: String = r.get(4)?;
    Ok(Install {
        id: r.get(0)?,
        queue_id: r.get(1)?,
        name: r.get(2)?,
        origins: serde_json::from_str(&origins).unwrap_or_default(),
        config: serde_json::from_str(&config).unwrap_or_default(),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

/// Live install `iid` of live queue `qid`.
fn install_in(c: &Connection, qid: &str, iid: &str) -> Result<Option<Install>> {
    Ok(c.prepare_cached(&format!(
        "{INSTALL_SELECT} WHERE id = ?1 AND queue_id = ?2 AND deleted_at IS NULL"
    ))?
    .query_row(params![iid, qid], install_row)
    .optional()?)
}

fn config_json(c: &InstallConfig) -> String {
    serde_json::to_string(c).expect("config serialises")
}

impl Store {
    /// Makes an install of queue `qid`, recorded as `queue.update
    /// {queue_id, op: "install_create", install_id, name, origins}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `invalid_install` or
    /// `invalid_origin` for bad input, or a queue already holding
    /// [`MAX_QUEUE_INSTALLS`].
    pub fn create_install(&self, ctx: &AuditCtx, qid: &str, i: NewInstall) -> Result<Install> {
        let name = check_install_name(&i.name)?;
        let origins = normalize_origins(&i.origins)?;
        let config = check_install_config(i.config)?;
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let held: i64 = tx.query_row(
                "SELECT COUNT(*) FROM installs WHERE queue_id = ?1 AND deleted_at IS NULL",
                params![qid],
                |r| r.get(0),
            )?;
            if held as usize >= MAX_QUEUE_INSTALLS {
                return Err(CoreError::invalid(
                    "invalid_install",
                    format!("a queue holds at most {MAX_QUEUE_INSTALLS} installs"),
                ));
            }
            let id = new_ulid();
            let now = self.now();
            tx.execute(
                "INSERT INTO installs (id, queue_id, name, origins_json, config_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    id,
                    qid,
                    name,
                    serde_json::to_string(&origins).expect("strings serialise"),
                    config_json(&config),
                    now
                ],
            )?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "install_create")
                .with("install_id", id.as_str())
                .with("name", name.as_str())
                .with("origins", origins.clone());
            self.record_audit(tx, ctx, rec)?;
            install_in(tx, qid, &id)?.ok_or(CoreError::NotFound)
        })
    }

    /// Queue `qid`'s live installs, oldest first.
    ///
    /// # Errors
    /// Database errors.
    pub fn installs_of_queue(&self, qid: &str) -> Result<Vec<Install>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "{INSTALL_SELECT} WHERE queue_id = ?1 AND deleted_at IS NULL ORDER BY created_at, id"
            ))?
            .query_map(params![qid], install_row)?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Live install `iid` and its live queue, for the config its overlay
    /// fetches; `None` when either is gone.
    ///
    /// # Errors
    /// Database errors.
    pub fn install_with_queue(&self, iid: &str) -> Result<Option<(Install, QueueRow)>> {
        self.with_read(|c| {
            let i = c
                .prepare_cached(&format!(
                    "{INSTALL_SELECT} WHERE id = ?1 AND deleted_at IS NULL"
                ))?
                .query_row(params![iid], install_row)
                .optional()?;
            let Some(i) = i else { return Ok(None) };
            Ok(queue_in(c, &i.queue_id)?.map(|q| (i, q)))
        })
    }

    /// Changes install `iid` of queue `qid` as `p` says, recorded as
    /// `queue.update {queue_id, op: "install_update", install_id, <each
    /// field set>}`; a patch that changes nothing records nothing.
    ///
    /// # Errors
    /// [`CoreError::NotFound`] for no such install of the queue; the
    /// errors of [`Store::create_install`] for bad input.
    pub fn update_install(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        iid: &str,
        p: InstallPatch,
    ) -> Result<Install> {
        let name = p.name.as_deref().map(check_install_name).transpose()?;
        let origins = p.origins.as_deref().map(normalize_origins).transpose()?;
        let config = p.config.map(check_install_config).transpose()?;
        self.with_tx(|tx| {
            let i = install_in(tx, qid, iid)?.ok_or(CoreError::NotFound)?;
            let now = self.now();
            let mut rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "install_update")
                .with("install_id", iid);
            let mut changed = false;
            if let Some(n) = name.filter(|n| *n != i.name) {
                tx.execute("UPDATE installs SET name = ?2 WHERE id = ?1", params![iid, n])?;
                rec = rec.with("name", n);
                changed = true;
            }
            if let Some(o) = origins.filter(|o| *o != i.origins) {
                tx.execute(
                    "UPDATE installs SET origins_json = ?2 WHERE id = ?1",
                    params![iid, serde_json::to_string(&o).expect("strings serialise")],
                )?;
                rec = rec.with("origins", o);
                changed = true;
            }
            if let Some(c) = config.filter(|c| *c != i.config) {
                tx.execute(
                    "UPDATE installs SET config_json = ?2 WHERE id = ?1",
                    params![iid, config_json(&c)],
                )?;
                rec = rec.with("config", serde_json::to_value(&c).expect("config serialises"));
                changed = true;
            }
            if changed {
                tx.execute(
                    "UPDATE installs SET updated_at = ?2 WHERE id = ?1",
                    params![iid, now],
                )?;
                self.record_audit(tx, ctx, rec)?;
            }
            install_in(tx, qid, iid)?.ok_or(CoreError::NotFound)
        })
    }

    /// Deletes install `iid` of queue `qid`: its config answers 404 from
    /// then on. Recorded as `queue.update {queue_id, op: "install_delete",
    /// install_id}`.
    ///
    /// # Errors
    /// [`CoreError::NotFound`] for no such install of the queue.
    pub fn delete_install(&self, ctx: &AuditCtx, qid: &str, iid: &str) -> Result<()> {
        self.with_tx(|tx| {
            install_in(tx, qid, iid)?.ok_or(CoreError::NotFound)?;
            let now = self.now();
            tx.execute(
                "UPDATE installs SET deleted_at = ?2 WHERE id = ?1",
                params![iid, now],
            )?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "install_delete")
                .with("install_id", iid);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queues::NewQueue;
    use crate::test_util::{DAEMON, store};
    use clax_core::queue::ReportersSee;

    fn queue(st: &Store) -> String {
        st.create_queue(
            DAEMON,
            None,
            NewQueue {
                name: "App reports".into(),
                origins: vec!["https://app.example.com".into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id
    }

    fn new(name: &str) -> NewInstall {
        NewInstall {
            name: name.into(),
            origins: vec!["https://App.Example.com/".into()],
            config: InstallConfig {
                product_name: Some("Acme".into()),
                ..InstallConfig::default()
            },
        }
    }

    #[test]
    fn an_install_is_made_listed_changed_and_deleted() {
        let (_d, st) = store();
        let qid = queue(&st);
        let i = st.create_install(DAEMON, &qid, new(" Changelist ")).unwrap();
        assert_eq!(i.name, "Changelist");
        assert_eq!(i.origins, vec!["https://app.example.com".to_string()]);
        assert_eq!(i.config.product_name.as_deref(), Some("Acme"));
        assert_eq!(st.installs_of_queue(&qid).unwrap(), vec![i.clone()]);

        let (got, q) = st.install_with_queue(&i.id).unwrap().unwrap();
        assert_eq!((got.id.as_str(), q.id.as_str()), (i.id.as_str(), qid.as_str()));

        let changed = st
            .update_install(
                DAEMON,
                &qid,
                &i.id,
                InstallPatch {
                    config: Some(InstallConfig {
                        reports_button: false,
                        ..InstallConfig::default()
                    }),
                    ..InstallPatch::default()
                },
            )
            .unwrap();
        assert!(!changed.config.reports_button);
        assert_eq!(changed.config.product_name, None, "config is set whole");
        assert_eq!(changed.name, "Changelist");

        st.delete_install(DAEMON, &qid, &i.id).unwrap();
        assert!(st.install_with_queue(&i.id).unwrap().is_none());
        assert!(st.installs_of_queue(&qid).unwrap().is_empty());
        assert!(matches!(
            st.delete_install(DAEMON, &qid, &i.id),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn an_install_is_reached_through_its_own_queue_alone() {
        let (_d, st) = store();
        let qid = queue(&st);
        let other = queue(&st);
        let i = st.create_install(DAEMON, &qid, new("Basis")).unwrap();
        assert!(matches!(
            st.update_install(DAEMON, &other, &i.id, InstallPatch::default()),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.delete_install(DAEMON, &other, &i.id),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.create_install(DAEMON, "nope", new("x")),
            Err(CoreError::NotAMember)
        ));
    }

    #[test]
    fn a_deleted_queue_takes_its_installs_config_with_it() {
        let (_d, st) = store();
        let qid = queue(&st);
        let i = st.create_install(DAEMON, &qid, new("Basis")).unwrap();
        st.delete_queue(DAEMON, &qid).unwrap();
        assert!(st.install_with_queue(&i.id).unwrap().is_none());
    }

    #[test]
    fn bad_input_is_refused() {
        let (_d, st) = store();
        let qid = queue(&st);
        let mut bad = new("x");
        bad.origins = vec!["ftp://x".into()];
        assert!(st.create_install(DAEMON, &qid, bad).is_err());
        let mut bad = new("x");
        bad.config.brand_color = Some("blue".into());
        assert!(st.create_install(DAEMON, &qid, bad).is_err());
        assert!(st.create_install(DAEMON, &qid, new("  ")).is_err());
    }
}

//! SQLite-backed store. One connection behind a mutex. Metadata operations are
//! single transactions; version writes, deletes, and asset writes combine a
//! transaction with file-system work.

pub mod artifacts;
pub mod assets;
pub mod docs;
pub mod feedback;
pub mod migrations;
pub mod sessions;
pub mod threads;
pub mod viewers;
pub mod watches;

use crate::{Home, Result};
use rusqlite::Connection;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
    home: Home,
}

impl Store {
    pub fn open(home: &Home) -> Result<Store> {
        home.ensure_dirs()?;
        let conn = Connection::open(home.db_path())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let store = Store {
            conn: Mutex::new(conn),
            home: home.clone(),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    /// First row of `PRAGMA integrity_check`; "ok" when the database is sound.
    pub fn integrity_check(&self) -> Result<String> {
        self.with_conn(|c| Ok(c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?))
    }

    pub fn now() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in migrations::MIGRATIONS.iter().enumerate() {
            let target = i as u32 + 1;
            if target > version {
                let tx = conn.transaction()?;
                tx.execute_batch(sql)?;
                tx.pragma_update(None, "user_version", target)?;
                tx.commit()?;
            }
        }
        Ok(())
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }

    pub(crate) fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use crate::anchor::{Anchor, AnchorKind};
    use crate::publish::{PublishRequest, validate};
    use crate::{ArtifactId, Home, RegisterSession, Store};

    pub fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    /// A one-version artifact titled "Quarterly Review", owned by `session`.
    pub fn artifact(store: &Store, session: Option<&str>) -> ArtifactId {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "title": "Quarterly Review",
            "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let (a, _) = store
            .create_artifact(validate(req).unwrap(), session)
            .unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    /// A one-version artifact declaring `caps`.
    pub fn artifact_with_caps(store: &Store, caps: serde_json::Value) -> ArtifactId {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "title": "Tracker",
            "capabilities": caps,
            "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let (a, _) = store.create_artifact(validate(req).unwrap(), None).unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    /// A live session of `harness` with harness session ID `hsid`; returns its ID.
    pub fn session(store: &Store, harness: &str, hsid: &str) -> String {
        store
            .register_session(RegisterSession {
                harness: harness.into(),
                harness_session_id: Some(hsid.into()),
                cwd: "/w".into(),
                pid: None,
                parent_pid: None,
            })
            .unwrap()
            .id
    }

    pub fn anchor() -> Anchor {
        Anchor {
            kind: AnchorKind::Element,
            selector: Some("body > main > h2".into()),
            quote: Some("Quarterly goals".into()),
            prefix: Some(String::new()),
            suffix: Some(String::new()),
            html_hash: Some("sha256:00".into()),
            rect: None,
            custom_name: None,
            file: "index.html".into(),
        }
    }
}

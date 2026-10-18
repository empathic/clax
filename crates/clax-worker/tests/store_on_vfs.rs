//! clax-core's store on a `clax-do-vfs` connection, natively, as a Durable
//! Object opens it (`Store::open_on`): its migrations, maintenance and
//! writes pass the connection's authorizer, every commit reaches the page
//! store, and a store reopened on the page store sees them.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use clax_core::audit::AuditCtx;
use clax_core::content::memory::MemContent;
use clax_core::home::Home;
use clax_core::ids::ArtifactId;
use clax_core::publish::{FileInput, PublishRequest, validate};
use clax_core::working::SystemClock;
use clax_do_vfs::{Chunks, PageStore, StoreError};
use clax_store::Store;

#[derive(Default)]
struct MemPages {
    files: RefCell<HashMap<String, (u64, Chunks)>>,
    commits: Cell<u64>,
}

impl PageStore for MemPages {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        Ok(self.files.borrow().get(file).map(|f| f.0))
    }
    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .files
            .borrow()
            .get(file)
            .and_then(|f| f.1.get(&index).cloned()))
    }
    fn commit(
        &self,
        file: &str,
        size: u64,
        chunks: &Rc<Chunks>,
        keep_below: u64,
    ) -> Result<(), StoreError> {
        let mut files = self.files.borrow_mut();
        let f = files.entry(file.to_owned()).or_default();
        f.0 = size;
        f.1.extend(chunks.iter().map(|(i, c)| (*i, c.clone())));
        f.1.split_off(&keep_below);
        self.commits.set(self.commits.get() + 1);
        Ok(())
    }
    fn delete(&self, file: &str) -> Result<(), StoreError> {
        self.files.borrow_mut().remove(file);
        Ok(())
    }
}

fn open_store(pages: &Rc<MemPages>, home: &Home) -> (Store, clax_do_vfs::Registered) {
    let (conn, registered) = clax_do_vfs::open("instance.db", pages.clone())
        .expect("the database opens")
        .into_parts();
    let store = Store::open_on(
        home,
        conn,
        Arc::new(SystemClock),
        Arc::new(MemContent::new()),
    )
    .expect("the store opens on the VFS");
    (store, registered)
}

fn page(title: &str, body: &str) -> PublishRequest {
    PublishRequest {
        title: Some(title.into()),
        files: BTreeMap::from([(
            "index.html".to_string(),
            Some(FileInput {
                content: format!("<!doctype html><title>{title}</title><p>{body}</p>"),
                encoding: Default::default(),
                content_type: None,
            }),
        )]),
        ..PublishRequest::default()
    }
}

#[test]
fn the_store_opens_writes_and_reopens_on_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("clax"));
    let pages = Rc::new(MemPages::default());
    let (store, registered) = open_store(&pages, &home);
    let migrated = pages.commits.get();
    assert!(migrated > 0, "the migrations reached the page store");
    let ctx = AuditCtx::DAEMON;
    let mut ids = Vec::new();
    for i in 0..40 {
        let (a, v) = store
            .create_artifact(
                &ctx,
                validate(page(&format!("Page {i}"), "one")).unwrap(),
                None,
            )
            .unwrap();
        assert_eq!(v.n, 1);
        let mut next = page(&format!("Page {i}"), &"two ".repeat(2000));
        next.if_version = Some(1);
        store
            .publish_version(
                &ctx,
                &ArtifactId::parse(&a.id).unwrap(),
                validate(next).unwrap(),
                None,
            )
            .unwrap();
        ids.push(a.id);
    }
    let commits = pages.commits.get();
    assert!(commits >= migrated + 80, "each publish was stored");
    store.optimize().unwrap();
    assert_eq!(store.integrity_check().unwrap(), "ok");
    drop(store);
    drop(registered);

    let (store, _registered) = open_store(&pages, &home);
    let listed = store.list_artifacts().unwrap();
    assert_eq!(listed.len(), ids.len());
    assert!(listed.iter().all(|a| a.current_version == 2));
    assert_eq!(store.integrity_check().unwrap(), "ok");
}

/// Migrations 24 to 35 as commit 84b5e5c28 (branch
/// akesling/clax-artifacts-pre-rebase-2026-10-16) wrote them: earlier
/// copies of 24 and 25, which a hosted daemon's database was migrated with.
const PRE_REBASE_MIGRATIONS: &str =
    include_str!("../../clax-store/tests/fixtures/old-migrations/pre-rebase-84b5e5c28.sql");

#[test]
fn the_store_repairs_a_database_an_older_build_migrated_on_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("clax"));
    let pages = Rc::new(MemPages::default());
    {
        let (conn, _registered) = clax_do_vfs::open("instance.db", pages.clone())
            .expect("the database opens")
            .into_parts();
        let mut sql = String::new();
        for m in &clax_store::migrations::MIGRATIONS[..23] {
            sql.push_str(m);
            sql.push_str(";\n");
        }
        sql.push_str(
            "INSERT INTO install (k, v) VALUES ('id', '000102030405060708090a0b0c0d0e0f');\n",
        );
        sql.push_str(PRE_REBASE_MIGRATIONS);
        let rights = clax_core::rights::Rights::preset(clax_core::rights::PRESET_READER)
            .unwrap()
            .to_json()
            .to_string();
        sql.push_str(&format!(
            r#"INSERT INTO approvals (id, approver, author, scope, created_at, created_via)
                VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FB0', NULL, '*unconnected', 'everywhere',
                        '2026-10-15T09:00:00.000Z', 'migration');
             INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                VALUES ('7q3k9mzx2b4t', 'Plan', '2026-10-15T10:00:00.000Z', '2026-10-15T10:00:00.000Z', 1, '0');
             INSERT INTO versions (artifact_id, n, created_at, files_json)
                VALUES ('7q3k9mzx2b4t', 1, '2026-10-15T10:00:00.000Z', '{{}}');
             INSERT INTO viewers (id, display_name, created_at, public_id)
                VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAV', 'Sam', '2026-10-15T09:30:00.000Z',
                        'u_0123456789abcdef012345');
             INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, agent_handle, elevate_json)
                VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAX', 'claude-code', '/w', '2026-10-15T10:00:00.000Z',
                        '2026-10-15T10:05:00.000Z', 'a_0123456789abcdef012345', '{rights}');
             INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at)
                VALUES ('t1', '7q3k9mzx2b4t', 1,
                        '{{"kind":"element","selector":"main","quote":"Plan","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}}',
                        '2026-10-15T10:01:00.000Z');
             INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, body, created_at)
                VALUES ('c1', 't1', 'viewer', 'Sam', 'u_0123456789abcdef012345', 'Shorter, please',
                        '2026-10-15T10:01:00.000Z');
             INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                VALUES ('f1', 't1', 'c1', '01ARZ3NDEKTSV4RRFFQ69G5FAX', '2026-10-15T10:01:00.000Z');
             PRAGMA user_version = 35;"#
        ));
        conn.execute_batch(&sql).unwrap();
    }

    let (store, _registered) = open_store(&pages, &home);
    let aid = ArtifactId::parse("7q3k9mzx2b4t").unwrap();
    let (threads, _) = store.list_threads(&aid, true, None, 50).unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].comments[0].body, "Shorter, please");
    store.thread_extras(&threads, false).unwrap();
    let raised = store
        .agent_rights("01ARZ3NDEKTSV4RRFFQ69G5FAX")
        .unwrap()
        .expect("a live agent");
    assert!(
        raised.elevation_by(None).is_some(),
        "the owner's elevation is kept"
    );
    assert!(
        store
            .get_viewer("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .unwrap()
            .is_some(),
        "the row-ID cookie names its viewer"
    );
    assert_eq!(store.integrity_check().unwrap(), "ok");
}

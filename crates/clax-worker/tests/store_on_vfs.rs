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
    /// Each commit that changed the database's schema version, in order:
    /// the version it left ([`user_version`]) and the chunks it wrote.
    steps: RefCell<Vec<(u32, usize)>>,
}

/// The schema version (`PRAGMA user_version`) of the database whose first
/// chunk is `first`: bytes 60 to 63 of SQLite's file header, big-endian. 0
/// for a file without a whole header, as for a database never written.
fn user_version(first: Option<&Vec<u8>>) -> u32 {
    first
        .and_then(|c| c.get(60..64))
        .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
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
        let before = user_version(f.1.get(&0));
        f.0 = size;
        f.1.extend(chunks.iter().map(|(i, c)| (*i, c.clone())));
        f.1.split_off(&keep_below);
        self.commits.set(self.commits.get() + 1);
        let after = user_version(f.1.get(&0));
        if after != before {
            self.steps.borrow_mut().push((after, chunks.len()));
        }
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

/// The runs [`migration_64_rebuilds_bot_runs_within_its_cost_a_run_on_the_vfs`]
/// migrates.
const RUNS_MIGRATED: u64 = 4_000;

/// The most chunks migration 64's transaction may write for each thousand
/// runs it copies: 200, which is 819 bytes a run. Measured at 176 (703
/// chunks, 720 bytes a run) for [`RUNS_MIGRATED`] runs of one to three
/// threads. The VFS refuses a transaction over 4,096 chunks
/// (`clax_do_vfs::MAX_TRANSACTION_BYTES`), so at this bound a database of
/// 20,000 runs still migrates; a change that makes a run's row or its
/// indexes wider shows here before it lowers that number unseen.
const CHUNKS_PER_THOUSAND_RUNS: usize = 200;

/// The most chunks migration 65's transaction may write, however many
/// runs the database holds: it makes five empty tables, adds columns to
/// `bot_runs` and `bot_steps` without rewriting a row and makes one index
/// again, over the runs that have not ended. Measured at 25 chunks beside
/// [`RUNS_MIGRATED`] ended runs. A change that makes it copy `bot_runs`
/// (hundreds of chunks here) fails this bound.
const MIGRATION_65_CHUNKS: usize = 32;

/// Migration 64 copies every run in one transaction, which on the VFS is
/// one commit of at most 16 MiB: its cost a run is held under
/// [`CHUNKS_PER_THOUSAND_RUNS`], and the runs come through it.
///
/// The database starts at 63 as the SQL of migrations 1 to 63 leaves it,
/// without their Rust steps (the install's ID, the seeded cost lines),
/// none of which writes `bot_runs`. The commit measured is the one that
/// takes the schema version from 63 to 64 ([`MemPages::steps`]), so a
/// later migration's commit, however large, is not counted as this one's.
///
/// The commit that takes the version from 64 to 65 on the same database
/// is held under [`MIGRATION_65_CHUNKS`]: it copies no run.
#[test]
fn migration_64_rebuilds_bot_runs_within_its_cost_a_run_on_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("clax"));
    let pages = Rc::new(MemPages::default());
    let before = clax_store::migrations::RUN_BINDINGS_MIGRATION - 1;
    {
        let (conn, _registered) = clax_do_vfs::open("instance.db", pages.clone())
            .expect("the database opens")
            .into_parts();
        let mut sql = String::new();
        for m in &clax_store::migrations::MIGRATIONS[..before as usize] {
            sql.push_str(m);
            sql.push_str(";\n");
        }
        sql.push_str(&format!("PRAGMA user_version = {before};"));
        conn.execute_batch(&sql)
            .expect("the migrations before 64 apply");
        // Ended thread runs as a hosted instance holds them: ULIDs, an
        // artifact and a person among a few hundred, one to three threads,
        // both web tools, three timestamps.
        for from in (0..RUNS_MIGRATED).step_by(1_000) {
            conn.execute_batch(&format!(
                "BEGIN;
                 INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json, status,
                        reason, model, key_source, base_version, credits_milli, agent_grant,
                        claimed_by, claimed_at, created_at, updated_at, ended_at, due_at, web)
                 WITH RECURSIVE n(i) AS
                    (SELECT {from} UNION ALL SELECT i + 1 FROM n WHERE i + 1 < {from} + 1000)
                 SELECT printf('01JC%022d', i), printf('a%011d', i % 500),
                        printf('u_%022d', i % 50), 'thread',
                        '[' || printf('\"01JD%022d\"', i)
                            || CASE WHEN i % 3 > 0 THEN printf(',\"01JE%022d\"', i) ELSE '' END
                            || CASE WHEN i % 3 > 1 THEN printf(',\"01JF%022d\"', i) ELSE '' END
                            || ']',
                        'finished', NULL, 'claude-sonnet-5-5', 'instance', 3, 1200, NULL, NULL,
                        NULL,
                        printf('2026-11-%02dT%02d:%02d:01.000Z', 1 + i / 1440 % 28, i / 60 % 24, i % 60),
                        printf('2026-11-%02dT%02d:%02d:41.000Z', 1 + i / 1440 % 28, i / 60 % 24, i % 60),
                        printf('2026-11-%02dT%02d:%02d:41.000Z', 1 + i / 1440 % 28, i / 60 % 24, i % 60),
                        NULL, 'search,fetch'
                   FROM n;
                 COMMIT;"
            ))
            .unwrap();
        }
    }

    pages.steps.borrow_mut().clear();
    let (store, registered) = open_store(&pages, &home);
    let steps = pages.steps.borrow().clone();
    let rebuilds: Vec<usize> = steps
        .iter()
        .filter(|(version, _)| *version == before + 1)
        .map(|(_, chunks)| *chunks)
        .collect();
    let [chunks] = rebuilds[..] else {
        panic!(
            "expected one commit taking the schema version from {before} to {}; the open's \
             commits that changed the version were (version, chunks) {steps:?}",
            before + 1
        );
    };
    assert!(
        chunks * 1_000 <= CHUNKS_PER_THOUSAND_RUNS * RUNS_MIGRATED as usize,
        "migration 64 wrote {chunks} chunks for {RUNS_MIGRATED} runs, over \
         {CHUNKS_PER_THOUSAND_RUNS} a thousand"
    );
    assert!(
        chunks * 1_000 >= 100 * RUNS_MIGRATED as usize,
        "migration 64 wrote {chunks} chunks for {RUNS_MIGRATED} runs, under 100 a thousand: \
         fewer than the runs' own rows fill, so the commit did not copy them"
    );
    let surfaces: Vec<usize> = steps
        .iter()
        .filter(|(version, _)| *version == before + 2)
        .map(|(_, chunks)| *chunks)
        .collect();
    let [added] = surfaces[..] else {
        panic!(
            "expected one commit taking the schema version from {} to {}; the open's commits \
             that changed the version were (version, chunks) {steps:?}",
            before + 1,
            before + 2
        );
    };
    eprintln!("migration 65 wrote {added} chunks beside {RUNS_MIGRATED} runs");
    assert!(
        added <= MIGRATION_65_CHUNKS,
        "migration 65 wrote {added} chunks beside {RUNS_MIGRATED} runs, over \
         {MIGRATION_65_CHUNKS}: it adds tables and columns and copies no run"
    );
    assert_eq!(store.integrity_check().unwrap(), "ok");
    let live = store.live_runs().unwrap();
    assert!(live.is_empty(), "every run had ended");
    let last = format!("01JC{:022}", RUNS_MIGRATED - 1);
    let run = store.run(&last).unwrap().expect("the last run");
    assert_eq!(run.thread_ids.len(), ((RUNS_MIGRATED - 1) % 3 + 1) as usize);
    assert_eq!(run.web, "search,fetch");
    drop(store);
    drop(registered);

    let (conn, _registered) = clax_do_vfs::open("instance.db", pages.clone())
        .expect("the database opens")
        .into_parts();
    let (version, runs, bound): (u32, u64, u64) = conn
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version), count(*),
                    count(*) FILTER (WHERE kind = 'clax.thread'
                        AND json_extract(bindings_json, '$.bindings[0].granted[1]') = 'web.fetch'
                        AND json_array_length(request_json, '$.thread_ids')
                            = json_array_length(thread_ids_json))
               FROM bot_runs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        version,
        clax_store::migrations::MIGRATIONS.len() as u32,
        "the schema version after the open, against the last migration's"
    );
    assert_eq!(
        (runs, bound),
        (RUNS_MIGRATED, RUNS_MIGRATED),
        "(runs kept, runs with kind clax.thread, web.fetch granted and as many request \
         threads as thread_ids_json), each expected to be every run seeded"
    );
}

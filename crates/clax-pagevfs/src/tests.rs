use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

fn names_by_address(seen: &mut HashMap<usize, HashSet<String>>) {
    let r = registry();
    for (addr, name) in &r.handles {
        seen.entry(*addr).or_default().insert(name.clone());
    }
}

fn keys(db: &Connection) -> Vec<i64> {
    db.prepare("SELECT k FROM t ORDER BY k")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Handles are told apart by the address of their name's buffer, and
/// SQLite's allocator hands a closed handle's address to a later file.
/// Over many opens and closes of one name in many stores, with temporary
/// files opened and deleted in between and an earlier connection always
/// alive, each connection reaches only its own store.
#[test]
fn a_reused_name_address_never_reaches_another_file() {
    let mut seen: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut stale: Option<(Database, Arc<MemPages>)> = None;
    let mut parked: Vec<Registered> = Vec::new();
    for i in 0..300u64 {
        let store = Arc::new(MemPages::new());
        let db = open("reuse.db", store.clone()).unwrap();
        names_by_address(&mut seen);
        db.execute_batch(
            "PRAGMA temp_store=FILE;
             CREATE TABLE IF NOT EXISTS t(k INTEGER PRIMARY KEY, v BLOB);
             CREATE TEMP TABLE scratch(v BLOB);
             INSERT INTO scratch SELECT randomblob(9000) FROM (SELECT 1 UNION SELECT 2);
             SELECT v FROM scratch ORDER BY v;",
        )
        .unwrap();
        names_by_address(&mut seen);
        db.execute("INSERT INTO t VALUES (?1, randomblob(100))", [i as i64])
            .unwrap();
        // The earlier open, of another store, keeps reaching its own store
        // alone: its write lands there, never in this open's.
        if let Some((old, old_store)) = stale.take() {
            let commits = old_store.commits();
            old.execute("INSERT INTO t VALUES (-1, 'stale')", [])
                .unwrap();
            assert_eq!(old_store.commits(), commits + 1, "open {i}");
        }
        let other = Arc::new(MemPages::new());
        let (conn, registered) = open(&format!("other-{}.db", i % 10), other.clone())
            .unwrap()
            .into_parts();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS t(k)")
            .unwrap();
        names_by_address(&mut seen);
        drop(conn);
        parked.push(registered);
        if parked.len() > 5 {
            parked.remove(0);
        }
        assert_eq!(keys(&db), [i as i64], "open {i}");
        stale = Some((db, store));
    }
    let reused = seen.values().filter(|names| names.len() > 1).count();
    assert!(
        reused > 0,
        "no name address was reused: the test did not exercise reuse"
    );
}

/// What one connection commits, the next open of the same store reads,
/// and a rolled-back transaction leaves nothing in the store.
#[test]
fn commits_outlive_the_connection_and_rollbacks_leave_nothing() {
    let store = Arc::new(MemPages::new());
    {
        let db = open("keep.db", store.clone()).unwrap();
        db.execute_batch(
            "CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
             INSERT INTO t VALUES (1, randomblob(20000)), (2, 'two');",
        )
        .unwrap();
        let committed = store.commits();
        db.execute_batch("BEGIN; INSERT INTO t VALUES (3, 'three'); ROLLBACK;")
            .unwrap();
        assert_eq!(store.commits(), committed, "a rollback committed");
    }
    let db = open("keep.db", store.clone()).unwrap();
    assert_eq!(keys(&db), [1, 2]);
    let check: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
}

/// A transaction past the cap fails with SQLITE_FULL and rolls back, even
/// after SQLite spilled its pages into the file; the store keeps the last
/// committed state, and the next transaction commits.
#[test]
fn a_transaction_past_the_cap_rolls_back_to_the_last_commit() {
    let store = Arc::new(MemPages::new());
    let limits = Limits {
        cache_bytes: 64 << 10,
        max_transaction_bytes: 256 << 10,
    };
    let db = open_with("cap.db", store.clone(), limits).unwrap();
    db.execute_batch(
        "CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB); INSERT INTO t VALUES (1, 'a');",
    )
    .unwrap();
    let err = db
        .execute_batch(
            "BEGIN;
             WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
             INSERT INTO t SELECT i, randomblob(4000) FROM n;
             COMMIT;",
        )
        .unwrap_err();
    assert!(err.to_string().contains("full"), "{err}");
    let _ = db.execute_batch("ROLLBACK");
    assert_eq!(keys(&db), [1]);
    db.execute("INSERT INTO t VALUES (2, 'b')", []).unwrap();
    drop(db);
    let db = open("cap.db", store).unwrap();
    assert_eq!(keys(&db), [1, 2]);
}

/// The settings the VFS relies on cannot be changed on its connection.
#[test]
fn the_connection_refuses_the_settings_the_vfs_rests_on() {
    let db = open("pragmas.db", Arc::new(MemPages::new())).unwrap();
    for p in [
        "PRAGMA journal_mode=WAL",
        "PRAGMA locking_mode=NORMAL",
        "PRAGMA synchronous=OFF",
    ] {
        assert!(db.execute_batch(p).is_err(), "{p} was allowed");
    }
    db.execute_batch("PRAGMA synchronous=FULL").unwrap();
}

/// A connection moves between threads, one at a time, as a store's jobs
/// do; several databases are used on several threads at once.
#[test]
fn connections_are_used_from_many_threads() {
    let stores: Vec<Arc<MemPages>> = (0..4).map(|_| Arc::new(MemPages::new())).collect();
    let dbs: Vec<Arc<Mutex<Database>>> = stores
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let db = open(&format!("threads-{i}.db"), s.clone()).unwrap();
            db.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB)")
                .unwrap();
            Arc::new(Mutex::new(db))
        })
        .collect();
    let failed = Arc::new(AtomicBool::new(false));
    let handles: Vec<_> = (0..16)
        .map(|t| {
            let db = dbs[t % dbs.len()].clone();
            let failed = failed.clone();
            std::thread::spawn(move || {
                for i in 0..50i64 {
                    let k = (t as i64) * 1000 + i;
                    let r = lock(&db).execute("INSERT INTO t VALUES (?1, randomblob(3000))", [k]);
                    if r.is_err() {
                        failed.store(true, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert!(!failed.load(Ordering::SeqCst));
    drop(dbs);
    for (i, s) in stores.into_iter().enumerate() {
        let db = open(&format!("threads-{i}.db"), s).unwrap();
        assert_eq!(keys(&db).len(), 200, "database {i}");
    }
}

/// The Clax store migrates and runs on a connection of the VFS, and a
/// later open of the same pages finds the migrated schema.
#[test]
fn the_clax_store_migrates_on_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let home = clax_core::Home::at(dir.path().join("home"));
    let pages = Arc::new(MemPages::new());
    let (conn, registered) = open("db", pages.clone()).unwrap().into_parts();
    let store = clax_store::Store::open_on(
        &home,
        conn,
        Arc::new(clax_core::working::SystemClock),
        Arc::new(clax_core::content::memory::MemContent::new()),
    )
    .unwrap();
    assert_eq!(store.integrity_check().unwrap(), "ok");
    drop(store);
    drop(registered);
    let db = open("db", pages).unwrap();
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(tables > 20, "{tables} tables after the migrations");
}

/// Two stores' files of one name are two files: opening the second leaves
/// the first's connection working, and each reads only its own rows.
#[test]
fn two_stores_keep_files_of_the_same_name_apart() {
    let (one, two) = (Arc::new(MemPages::new()), Arc::new(MemPages::new()));
    let a = open("same.db", one.clone()).unwrap();
    a.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);")
        .unwrap();
    let b = open("same.db", two.clone()).unwrap();
    b.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY); INSERT INTO t VALUES (2);")
        .unwrap();
    a.execute("INSERT INTO t VALUES (3)", []).unwrap();
    assert_eq!(keys(&a), [1, 3]);
    assert_eq!(keys(&b), [2]);
    let one: Arc<dyn PageStore> = one;
    assert!(stats(&one, "same.db").unwrap().commits >= 2);
}

/// Opening a store's file again replaces the earlier open: the earlier
/// connection's next write fails and reaches the store not at all.
#[test]
fn reopening_a_stores_file_replaces_the_earlier_connection() {
    let store = Arc::new(MemPages::new());
    let old = open("again.db", store.clone()).unwrap();
    old.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY)")
        .unwrap();
    let new = open("again.db", store.clone()).unwrap();
    let commits = store.commits();
    assert!(old.execute("INSERT INTO t VALUES (1)", []).is_err());
    assert_eq!(store.commits(), commits);
    new.execute("INSERT INTO t VALUES (2)", []).unwrap();
    assert_eq!(keys(&new), [2]);
}

/// Pages in memory that fail on demand: a commit that applies and then
/// reports an error (as a lost reply would), a size read that fails, and a
/// size read that tells the test it has begun and waits to go on. Each
/// commit's `stored_size` is recorded.
#[derive(Default)]
struct Faulty {
    pages: MemPages,
    fail_after_commit: AtomicBool,
    fail_size: AtomicBool,
    size_gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    stored_sizes: Mutex<Vec<Option<u64>>>,
}

impl PageStore for Faulty {
    fn size(&self, file: &str) -> Result<Option<u64>, StoreError> {
        if let Some((began, go)) = lock(&self.size_gate).take() {
            began.send(()).unwrap();
            go.recv().unwrap();
        }
        if self.fail_size.swap(false, Ordering::SeqCst) {
            return Err("the size read failed".into());
        }
        self.pages.size(file)
    }
    fn read_chunk(&self, file: &str, index: u64) -> Result<Option<Vec<u8>>, StoreError> {
        self.pages.read_chunk(file, index)
    }
    fn commit(&self, file: &str, commit: &Commit) -> Result<(), StoreError> {
        lock(&self.stored_sizes).push(commit.stored_size);
        self.pages.commit(file, commit)?;
        if self.fail_after_commit.swap(false, Ordering::SeqCst) {
            return Err("the reply to the commit was lost".into());
        }
        Ok(())
    }
    fn delete(&self, file: &str) -> Result<(), StoreError> {
        self.pages.delete(file)
    }
}

/// A commit that applied but reported an error leaves the stored size
/// unknown: the next commit (the rollback's) says so (`stored_size` is
/// `None`), so a store writes its size and drops the chunks past its end,
/// and the one after knows it again.
#[test]
fn a_commit_that_failed_leaves_the_stored_size_unknown() {
    let store = Arc::new(Faulty::default());
    let db = open("unsure.db", store.clone()).unwrap();
    db.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB)")
        .unwrap();
    store.fail_after_commit.store(true, Ordering::SeqCst);
    let failed = lock(&store.stored_sizes).len();
    assert!(
        db.execute("INSERT INTO t VALUES (1, randomblob(50000))", [])
            .is_err()
    );
    db.execute("INSERT INTO t VALUES (2, 'two')", []).unwrap();
    let sizes = lock(&store.stored_sizes).clone();
    assert!(sizes[failed].is_some(), "{sizes:?}");
    assert_eq!(sizes[failed + 1], None, "{sizes:?}");
    assert!(sizes.last().unwrap().is_some(), "{sizes:?}");
    db.execute("INSERT INTO t VALUES (3, 'three')", []).unwrap();
    assert_eq!(keys(&db), [2, 3]);
    drop(db);
    let db = open("unsure.db", store).unwrap();
    assert_eq!(keys(&db), [2, 3]);
    let check: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
}

/// A reopen cuts the earlier connection off before it reads the store's
/// size: a write the earlier connection makes while the reopen reads
/// fails, and reaches the store not at all.
#[test]
fn a_reopen_stops_the_earlier_connection_before_it_reads_the_size() {
    let store = Arc::new(Faulty::default());
    let old = open("reading.db", store.clone()).unwrap();
    old.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);")
        .unwrap();
    let (began_tx, began) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel();
    *lock(&store.size_gate) = Some((began_tx, go_rx));
    let reopen = std::thread::spawn({
        let store: Arc<dyn PageStore> = store.clone();
        move || open("reading.db", store).map(|db| db.into_parts())
    });
    began.recv().unwrap();
    let commits = store.pages.commits();
    assert!(old.execute("INSERT INTO t VALUES (2)", []).is_err());
    assert_eq!(store.pages.commits(), commits);
    go.send(()).unwrap();
    let (new, _registered) = reopen.join().unwrap().unwrap();
    assert_eq!(keys(&new), [1]);
    new.execute("INSERT INTO t VALUES (3)", []).unwrap();
    assert_eq!(keys(&new), [1, 3]);
}

/// A reopen that fails leaves the earlier connection working.
#[test]
fn a_failed_reopen_leaves_the_earlier_connection_working() {
    let store = Arc::new(Faulty::default());
    let old = open("kept.db", store.clone()).unwrap();
    old.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);")
        .unwrap();
    store.fail_size.store(true, Ordering::SeqCst);
    assert!(open("kept.db", store.clone()).is_err());
    old.execute("INSERT INTO t VALUES (2)", []).unwrap();
    assert_eq!(keys(&old), [1, 2]);
    let dyn_store: Arc<dyn PageStore> = store.clone();
    assert!(stats(&dyn_store, "kept.db").is_some());
    drop(old);
    let db = open("kept.db", store).unwrap();
    assert_eq!(keys(&db), [1, 2]);
}

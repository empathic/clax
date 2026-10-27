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
/// Over many opens, closes and replacements, with temporary files opened
/// and deleted in between and a stale connection always alive, each
/// connection reaches only its own store, and the stale one none.
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
        if let Some((old, old_store)) = stale.take() {
            let commits = old_store.commits();
            assert!(
                old.execute("INSERT INTO t VALUES (-1, 'stale')", [])
                    .is_err(),
                "open {i}: the replaced connection still writes"
            );
            assert_eq!(old_store.commits(), commits, "open {i}");
        }
        let other = Arc::new(MemPages::new());
        let (conn, registered) = open(&format!("other-{}.db", i % 10), other.clone())
            .unwrap()
            .into_parts();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS t(k)").unwrap();
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
    db.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB); INSERT INTO t VALUES (1, 'a');")
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

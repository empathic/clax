//! Adversarial checks of the VFS's durability contract against an in-memory
//! page store that can refuse chosen commits.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use clax_do_vfs::{
    CHUNK_SIZE, Chunks, Limits, PageStore, StoreError, VFS_NAME, authorize, open, open_with, stats,
};
use rand::{Rng, RngCore, SeedableRng};
use rusqlite::{Connection, ErrorCode, OpenFlags, params};

type MemFile = (u64, BTreeMap<u64, Vec<u8>>);

#[derive(Default)]
struct MemStore {
    files: RefCell<HashMap<String, MemFile>>,
    commits: Cell<u64>,
    /// Refuse the commit with this 1-based number (counting attempts).
    fail_attempt: Cell<Option<u64>>,
    attempts: Cell<u64>,
}

impl PageStore for MemStore {
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
        self.attempts.set(self.attempts.get() + 1);
        if self.fail_attempt.get() == Some(self.attempts.get()) {
            return Err("store refused the commit".into());
        }
        let mut files = self.files.borrow_mut();
        let f = files.entry(file.to_owned()).or_default();
        f.0 = size;
        for (i, c) in chunks.iter() {
            f.1.insert(*i, c.clone());
        }
        f.1.split_off(&keep_below);
        self.commits.set(self.commits.get() + 1);
        Ok(())
    }
    fn delete(&self, file: &str) -> Result<(), StoreError> {
        self.files.borrow_mut().remove(file);
        Ok(())
    }
}

/// `n(i)` for i from 0 to 9999, as a `WITH` clause.
const SERIES: &str =
    "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 9999)";

fn count(c: &Connection) -> i64 {
    c.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap()
}

/// The connection's authorizer refused the statement.
fn assert_refused(err: &rusqlite::Error) {
    assert_eq!(
        err.sqlite_error_code(),
        Some(ErrorCode::AuthorizationForStatementDenied),
        "{err}"
    );
}

fn integrity(c: &Connection) -> String {
    c.query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap()
}

/// A caller that switches the connection to WAL (as clax-core's writer does
/// in `crates/clax-store/src/exec.rs`) would commit into a WAL file the
/// VFS keeps in memory, out of the store. The VFS refuses the switch, so the
/// connection stays in MEMORY mode and its commits reach the store.
#[test]
fn wal_mode_is_refused_so_commits_reach_the_store() {
    let store = Rc::new(MemStore::default());
    let c = open("wal.db", store.clone()).unwrap();
    let r = c.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0));
    let err = r.expect_err("journal_mode=WAL is refused");
    assert_refused(&err);
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "memory");
    c.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);")
        .unwrap();
    drop(c);
    let c = open("wal.db", store.clone()).unwrap();
    assert_eq!(count(&c), 1, "the committed row survives a reopen");
}

/// Every journal mode but MEMORY and every locking mode but EXCLUSIVE is
/// refused, in any spelling; asking for the current mode is not.
#[test]
fn journal_and_locking_modes_other_than_open_s_are_refused() {
    let store = Rc::new(MemStore::default());
    let c = open("modes.db", store.clone()).unwrap();
    for mode in [
        "WAL", "wal", "'wal'", "DELETE", "TRUNCATE", "PERSIST", "OFF",
    ] {
        let r = c.execute_batch(&format!("PRAGMA journal_mode={mode}"));
        assert_refused(&r.expect_err("journal_mode={mode} is refused"));
        let r = c.execute_batch(&format!("PRAGMA main.journal_mode={mode}"));
        assert_refused(&r.expect_err("main.journal_mode={mode} is refused"));
    }
    for mode in ["NORMAL", "normal", "'normal'"] {
        let r = c.execute_batch(&format!("PRAGMA locking_mode={mode}"));
        assert_refused(&r.expect_err("locking_mode=NORMAL is refused"));
    }
    c.execute_batch("PRAGMA journal_mode=memory; PRAGMA locking_mode=exclusive;")
        .unwrap();
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "memory");
    let mode: String = c
        .query_row("PRAGMA locking_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "exclusive");
}

/// A stored database whose header says WAL (written by another SQLite, say)
/// would open its WAL file; the VFS refuses to open one.
#[test]
fn a_stored_wal_database_is_refused() {
    let store = Rc::new(MemStore::default());
    let c = open("walhdr.db", store.clone()).unwrap();
    c.execute_batch("CREATE TABLE t(k); INSERT INTO t VALUES (1);")
        .unwrap();
    drop(c);
    {
        let mut files = store.files.borrow_mut();
        let header = files.get_mut("walhdr.db").unwrap().1.get_mut(&0).unwrap();
        header[18] = 2;
        header[19] = 2;
    }
    let r = open("walhdr.db", store.clone());
    assert!(r.is_err(), "a WAL database does not open");
}

/// With `temp_store=FILE`, SQLite keeps temporary tables, statement journals
/// and sorter runs in files it opens through the VFS, several at once; each
/// handle reaches its own file, and the database's commits are unaffected.
#[test]
fn temporary_files_open_through_the_vfs_keep_their_own_contents() {
    let store = Rc::new(MemStore::default());
    let c = open("tempfiles.db", store.clone()).unwrap();
    c.execute_batch(&format!(
        "PRAGMA temp_store=FILE; PRAGMA cache_size=10;
         CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
         INSERT INTO t SELECT i, randomblob(2000) FROM ({SERIES} SELECT i FROM n WHERE i < 2000);
         CREATE TEMP TABLE a(k INTEGER PRIMARY KEY, v BLOB);
         CREATE TEMP TABLE b(k INTEGER PRIMARY KEY, v BLOB);
         INSERT INTO a SELECT k, v FROM t WHERE k % 2 = 0;
         INSERT INTO b SELECT k, zeroblob(3000) FROM t WHERE k % 2 = 1;"
    ))
    .unwrap();
    let joined: i64 = c
        .query_row(
            "SELECT count(*) FROM (SELECT a.v FROM a JOIN t USING (k) WHERE a.v = t.v
             UNION ALL SELECT b.v FROM b ORDER BY 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(joined, 2000);
    let sorted: i64 = c
        .query_row(
            "SELECT count(DISTINCT v) FROM (SELECT v FROM t ORDER BY v DESC)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sorted, 2000);
    drop(c);
    let c = open("tempfiles.db", store.clone()).unwrap();
    assert_eq!(count(&c), 2000);
    assert_eq!(integrity(&c), "ok");
}

/// SQLite opens through the VFS only the database files `open` registered,
/// one connection each, and its temporary files: a stray name, an ATTACH, a
/// VACUUM INTO and a second connection on an open file all fail.
#[test]
fn only_the_files_open_registered_can_be_opened() {
    let store = Rc::new(MemStore::default());
    let c = open("only.db", store.clone()).unwrap();
    c.execute_batch("CREATE TABLE t(k); INSERT INTO t VALUES (1);")
        .unwrap();
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    assert!(Connection::open_with_flags_and_vfs("only.db", flags, VFS_NAME).is_err());
    assert!(Connection::open_with_flags_and_vfs("stray.db", flags, VFS_NAME).is_err());
    let sqlite_name = c.sqlite_name().to_owned();
    assert!(
        Connection::open_with_flags_and_vfs(&sqlite_name, flags, VFS_NAME).is_err(),
        "a second connection on an open file"
    );
    assert!(c.execute_batch("ATTACH 'other.db' AS o").is_err());
    assert!(c.execute_batch("VACUUM INTO 'copy.db'").is_err());
    // Temporary files still work.
    c.execute_batch(
        "CREATE TEMP TABLE x(v); INSERT INTO x SELECT randomblob(5000) FROM t;
         SELECT v FROM x ORDER BY v;",
    )
    .unwrap();
    assert!(stats("only.db").is_some());
    drop(c);
    assert_eq!(stats("only.db"), None, "a closed file is forgotten");
    assert!(
        Connection::open_with_flags_and_vfs(&sqlite_name, flags, VFS_NAME).is_err(),
        "a closed open's name does not open again"
    );
    let c = open("only.db", store.clone()).unwrap();
    assert_eq!(count(&c), 1);
    assert_ne!(
        c.sqlite_name(),
        sqlite_name,
        "each open has a name of its own"
    );
}

/// A cache spill writes uncommitted pages through xWrite; nothing may reach
/// the store before COMMIT.
#[test]
fn a_spilling_transaction_stores_nothing_until_commit() {
    let store = Rc::new(MemStore::default());
    let c = open("spill.db", store.clone()).unwrap();
    c.execute_batch("PRAGMA cache_size=10; CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);")
        .unwrap();
    let before = store.commits.get();
    let tx = c.unchecked_transaction().unwrap();
    for k in 0..2000 {
        tx.execute("INSERT INTO t VALUES (?1, zeroblob(1000))", params![k])
            .unwrap();
    }
    assert_eq!(
        store.commits.get(),
        before,
        "no flush while the transaction is open"
    );
    tx.commit().unwrap();
    assert_eq!(
        store.commits.get(),
        before + 1,
        "one store commit for the transaction"
    );
}

/// Rolling back a transaction that spilled pages: SQLite plays the journal
/// back into the file and then syncs it (`pager_playback` calls
/// `sqlite3PagerSync`), so the store receives a commit of the restored
/// pages. It must hold the same bytes as before.
#[test]
fn a_spilled_rollback_stores_only_the_restored_state() {
    let store = Rc::new(MemStore::default());
    let c = open("spillrb.db", store.clone()).unwrap();
    c.execute_batch(
        "PRAGMA cache_size=10; CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
         INSERT INTO t SELECT value, randomblob(3000) FROM json_each('[1,2,3,4,5,6,7,8,9,10]');",
    )
    .unwrap();
    let before = store.files.borrow().get("spillrb.db").cloned().unwrap();
    let commits = store.commits.get();
    let tx = c.unchecked_transaction().unwrap();
    tx.execute("UPDATE t SET v = randomblob(3000)", []).unwrap();
    for k in 100..1000 {
        tx.execute("INSERT INTO t VALUES (?1, zeroblob(1000))", params![k])
            .unwrap();
    }
    tx.rollback().unwrap();
    assert_eq!(
        store.commits.get() - commits,
        1,
        "the playback's sync stores the restored pages"
    );
    assert_eq!(
        store.files.borrow().get("spillrb.db").cloned().unwrap(),
        before
    );
}

/// After a store failure at commit, the same connection keeps working and
/// what it later commits is what a reopen sees.
#[test]
fn the_connection_recovers_after_a_refused_commit() {
    let store = Rc::new(MemStore::default());
    let c = open("recover.db", store.clone()).unwrap();
    c.execute_batch(
        "PRAGMA cache_size=10; CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
         INSERT INTO t VALUES (1, zeroblob(100));",
    )
    .unwrap();
    // Refuse the next attempt: the spilled, large transaction's commit.
    store.fail_attempt.set(Some(store.attempts.get() + 1));
    let tx = c.unchecked_transaction().unwrap();
    for k in 2..500 {
        tx.execute("INSERT INTO t VALUES (?1, zeroblob(3000))", params![k])
            .unwrap();
    }
    assert!(tx.commit().is_err());
    assert_eq!(
        count(&c),
        1,
        "the refused transaction rolled back in the connection"
    );
    assert_eq!(integrity(&c), "ok");
    c.execute("INSERT INTO t VALUES (1000, zeroblob(10))", [])
        .unwrap();
    assert_eq!(count(&c), 2);
    drop(c);
    let c = open("recover.db", store.clone()).unwrap();
    assert_eq!(count(&c), 2);
    assert_eq!(integrity(&c), "ok");
}

/// A VACUUM that shrinks the file stores its pages at xSync and its
/// truncation when SQLite truncates the file after the commit, in two store
/// commits. When
/// only the second is refused, the VACUUM, already durable, succeeds; the
/// store holds the vacuumed database untruncated, and the next commit
/// stores the truncation.
#[test]
fn a_refused_truncation_is_kept_for_the_next_commit() {
    let store = Rc::new(MemStore::default());
    let c = open("vac.db", store.clone()).unwrap();
    c.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);")
        .unwrap();
    let tx = c.unchecked_transaction().unwrap();
    for k in 0..300 {
        tx.execute("INSERT INTO t VALUES (?1, zeroblob(4000))", params![k])
            .unwrap();
    }
    tx.commit().unwrap();
    c.execute("DELETE FROM t WHERE k >= 10", []).unwrap();
    let pages =
        |c: &Connection| -> i64 { c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap() };
    let before = pages(&c);
    let stored = |s: &MemStore| {
        let files = s.files.borrow();
        let f = files.get("vac.db").unwrap();
        (f.0, f.1.len() as u64)
    };
    let (size_before, _) = stored(&store);
    // The VACUUM's first store commit (xSync) succeeds, the second (the
    // truncation) is refused.
    store.fail_attempt.set(Some(store.attempts.get() + 2));
    c.execute_batch("VACUUM").unwrap();
    assert_eq!(
        store.attempts.get(),
        store.commits.get() + 1,
        "one refused commit"
    );
    let vacuumed = pages(&c);
    assert!(vacuumed < before / 10, "{vacuumed} pages of {before}");
    assert_eq!(stored(&store).0, size_before, "the truncation is pending");
    c.execute("INSERT INTO t VALUES (1000, zeroblob(10))", [])
        .unwrap();
    let pages_now = pages(&c) as u64;
    assert_eq!(
        stored(&store),
        (pages_now * CHUNK_SIZE as u64, pages_now),
        "the next commit stored the truncation"
    );
    drop(c);
    let c = open("vac.db", store.clone()).unwrap();
    assert_eq!(integrity(&c), "ok");
    assert_eq!(count(&c), 11);
}

/// A truncation refused after its commit and never stored, because the
/// connection closed first, leaves a valid database: the chunks past the
/// header's page count are never read.
#[test]
fn a_truncation_never_stored_leaves_a_valid_database() {
    let store = Rc::new(MemStore::default());
    let c = open("vac2.db", store.clone()).unwrap();
    c.execute_batch(&format!(
        "CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
         INSERT INTO t SELECT i, zeroblob(4000) FROM ({SERIES} SELECT i FROM n WHERE i < 300);
         DELETE FROM t WHERE k >= 10;"
    ))
    .unwrap();
    store.fail_attempt.set(Some(store.attempts.get() + 2));
    c.execute_batch("VACUUM").unwrap();
    drop(c);
    let c = open("vac2.db", store.clone()).unwrap();
    assert_eq!(integrity(&c), "ok");
    assert_eq!(count(&c), 10);
    c.execute("INSERT INTO t SELECT k + 1000, v FROM t", [])
        .unwrap();
    drop(c);
    let c = open("vac2.db", store.clone()).unwrap();
    assert_eq!(integrity(&c), "ok");
    assert_eq!(count(&c), 20);
}

/// Two opens of one file name (a Durable Object rebuilt in the same isolate
/// while its previous instance's connection lives on): the later open
/// replaces the earlier, whose connection then fails rather than write into
/// either store.
#[test]
fn a_stale_connection_cannot_write_into_a_later_open() {
    let old_store = Rc::new(MemStore::default());
    let old = open("alias.db", old_store.clone()).unwrap();
    old.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);")
        .unwrap();
    // The new instance reads the same committed state from its own storage.
    let new_store = Rc::new(MemStore::default());
    *new_store.files.borrow_mut() = old_store.files.borrow().clone();
    let new = open("alias.db", new_store.clone()).unwrap();
    new.execute("INSERT INTO t VALUES (2)", []).unwrap();
    // The old instance, still holding its connection, writes once more.
    let old_commits = old_store.commits.get();
    let r = old.execute("INSERT INTO t VALUES (3)", []);
    assert_eq!(
        old_store.commits.get(),
        old_commits,
        "nothing reached the old store"
    );
    let new_commits = new_store.commits.get();
    drop(old);
    drop(new);
    let check = open("alias.db", new_store.clone()).unwrap();
    let mut st = check.prepare("SELECT k FROM t ORDER BY k").unwrap();
    let keys: Vec<i64> = st
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        r.is_err() && new_commits == 1 && keys == [1, 2],
        "the old connection's write reached the new store ({r:?}, new store commits \
         {new_commits}); the new store now holds keys {keys:?}"
    );
}

/// Random transactions, savepoints, statement failures, rollbacks, vacuums
/// and reopens over several page sizes, checked against a model.
#[test]
fn random_workloads_match_a_model_across_reopens() {
    for seed in 0..8u64 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let page_size = [1024, 4096, 8192, 65536][(seed % 4) as usize];
        let name = format!("fuzz-{seed}.db");
        let store = Rc::new(MemStore::default());
        let mut c = open(&name, store.clone()).unwrap();
        c.execute_batch(&format!(
            "CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB NOT NULL);
             PRAGMA page_size={page_size}; VACUUM; PRAGMA cache_size={};",
            [5, 50, 2000][rng.random_range(0..3)]
        ))
        .unwrap();
        let ps: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        assert_eq!(ps, page_size, "seed {seed}");
        let mut model: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
        for _ in 0..40 {
            match rng.random_range(0..10) {
                0 => {
                    drop(c);
                    c = open(&name, store.clone()).unwrap();
                    c.execute_batch(&format!(
                        "PRAGMA cache_size={};",
                        [5, 50, 2000][rng.random_range(0..3)]
                    ))
                    .unwrap();
                }
                1 => {
                    c.execute_batch("VACUUM").unwrap();
                }
                _ => {
                    let mut work = model.clone();
                    let tx = c.transaction().unwrap();
                    for _ in 0..rng.random_range(1..40) {
                        match rng.random_range(0..6) {
                            0..=2 => {
                                let k = rng.random_range(0..400);
                                let mut v = vec![0u8; rng.random_range(1..12_000)];
                                rng.fill_bytes(&mut v);
                                tx.execute(
                                    "INSERT OR REPLACE INTO t VALUES (?1, ?2)",
                                    params![k, v],
                                )
                                .unwrap();
                                work.insert(k, v);
                            }
                            3 => {
                                let lo = rng.random_range(0..400);
                                let hi = lo + rng.random_range(0..80);
                                tx.execute(
                                    "DELETE FROM t WHERE k BETWEEN ?1 AND ?2",
                                    params![lo, hi],
                                )
                                .unwrap();
                                work.retain(|k, _| !(lo..=hi).contains(k));
                            }
                            4 => {
                                // A statement that fails part-way: rows
                                // inserted before the conflict are undone.
                                if let Some((&dup, _)) = work.iter().next() {
                                    let fresh = 10_000 + rng.random_range(0..1000);
                                    let r = tx.execute(
                                        "INSERT INTO t SELECT ?1 + value, zeroblob(5000)
                                         FROM json_each('[0,1,2,3,4,5,6,7,8,9]')
                                         UNION ALL SELECT ?2, zeroblob(10)",
                                        params![fresh, dup],
                                    );
                                    assert!(r.is_err());
                                }
                            }
                            _ => {
                                // A savepoint rolled back after writes.
                                tx.execute_batch("SAVEPOINT s").unwrap();
                                for _ in 0..rng.random_range(1..30) {
                                    let k = rng.random_range(0..400);
                                    tx.execute(
                                        "INSERT OR REPLACE INTO t VALUES (?1, zeroblob(?2))",
                                        params![k, rng.random_range(1..9000)],
                                    )
                                    .unwrap();
                                }
                                tx.execute_batch("ROLLBACK TO s; RELEASE s").unwrap();
                            }
                        }
                    }
                    if rng.random_bool(0.75) {
                        tx.commit().unwrap();
                        model = work;
                    } else {
                        tx.rollback().unwrap();
                    }
                }
            }
        }
        drop(c);
        let c = open(&name, store.clone()).unwrap();
        assert_eq!(integrity(&c), "ok", "seed {seed}");
        let ps: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        assert_eq!(
            ps, page_size,
            "seed {seed}: reopens and vacuums keep the page size"
        );
        let mut st = c.prepare("SELECT k, v FROM t ORDER BY k").unwrap();
        let got: BTreeMap<i64, Vec<u8>> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(got.len(), model.len(), "seed {seed}");
        assert!(got == model, "seed {seed}: contents differ");
    }
}

/// Each commit is stored, at each `synchronous` level the connection
/// accepts. `synchronous=OFF`, under which SQLite commits without syncing
/// the file, is refused in any spelling.
#[test]
fn each_commit_is_stored_and_synchronous_off_is_refused() {
    let store = Rc::new(MemStore::default());
    let c = open("sync-off.db", store.clone()).unwrap();
    for off in ["OFF", "off", "0", "'off'", "no", "false"] {
        let r = c.execute_batch(&format!("PRAGMA synchronous={off}"));
        assert_refused(&r.expect_err("synchronous=OFF is refused"));
        let r = c.execute_batch(&format!("PRAGMA main.synchronous={off}"));
        assert_refused(&r.expect_err("main.synchronous=OFF is refused"));
    }
    let level: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
    assert_eq!(level, 1, "synchronous stays NORMAL");
    drop(c);
    for sync in ["NORMAL", "FULL", "EXTRA", "1", "2", "3"] {
        let name = format!("sync-{sync}.db");
        let store = Rc::new(MemStore::default());
        let c = open(&name, store.clone()).unwrap();
        c.execute_batch(&format!(
            "PRAGMA synchronous={sync};
             CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
             INSERT INTO t VALUES (1, zeroblob(10000));"
        ))
        .unwrap();
        let commits = store.commits.get();
        c.execute("INSERT INTO t VALUES (2, zeroblob(10000))", [])
            .unwrap();
        assert!(
            store.commits.get() > commits,
            "{sync}: the commit was stored"
        );
        drop(c);
        let c = open(&name, store.clone()).unwrap();
        assert_eq!(count(&c), 2, "{sync}");
        assert_eq!(integrity(&c), "ok", "{sync}");
    }
}

/// The dirty-set cap: a transaction that writes more than
/// `max_transaction_bytes` fails with SQLITE_FULL whether its pages spill
/// before COMMIT or are written at COMMIT, rolls back, and leaves the store
/// and the connection as they were.
#[test]
fn a_transaction_past_the_cap_fails_and_rolls_back() {
    for (case, cache_bytes) in [("spilling", 64 << 10), ("at commit", 64 << 20)] {
        let name = format!("cap-{cache_bytes}.db");
        let store = Rc::new(MemStore::default());
        let limits = Limits {
            cache_bytes,
            max_transaction_bytes: 1 << 20,
        };
        let c = open_with(&name, store.clone(), limits).unwrap();
        c.execute_batch(
            "CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
             INSERT INTO t VALUES (1, zeroblob(100));",
        )
        .unwrap();
        // Under the cap: half a MiB in one transaction.
        c.execute_batch(&format!(
            "INSERT INTO t SELECT 100 + i, randomblob(1000) FROM ({SERIES} SELECT i FROM n WHERE i < 500)"
        ))
        .unwrap();
        let rows = count(&c);
        let before = store.files.borrow().get(&name).cloned().unwrap();
        c.execute_batch("BEGIN").unwrap();
        let mut refused = None;
        for k in 0..300 {
            let r = c.execute(
                "INSERT INTO t VALUES (?1, randomblob(8000))",
                params![1000 + k],
            );
            if let Err(e) = r {
                refused = Some(e);
                break;
            }
        }
        let spilled = refused.is_some();
        assert_eq!(spilled, case == "spilling", "{case}: where the cap was met");
        let err = match refused {
            Some(e) => e,
            None => c
                .execute_batch("COMMIT")
                .expect_err("the commit is refused"),
        };
        assert_eq!(
            err.sqlite_error_code(),
            Some(ErrorCode::DiskFull),
            "{case}: {err}"
        );
        assert!(c.is_autocommit(), "{case}: SQLite ended the transaction");
        assert_eq!(count(&c), rows, "{case}: the transaction rolled back");
        assert_eq!(integrity(&c), "ok", "{case}");
        assert!(
            store.files.borrow().get(&name).cloned().unwrap() == before,
            "{case}: the store holds the last commit"
        );
        c.execute("INSERT INTO t VALUES (2, zeroblob(100))", [])
            .unwrap();
        drop(c);
        let c = open_with(&name, store.clone(), limits).unwrap();
        assert_eq!(count(&c), rows + 1, "{case}");
        assert_eq!(integrity(&c), "ok", "{case}");
    }
}

/// The rows of table `t` in a copy of what `store` holds for `name`, opened
/// on its own (a Durable Object evicted now and rebuilt from its storage),
/// and the copy's `PRAGMA integrity_check`.
fn stored_state(store: &MemStore, name: &str) -> (BTreeMap<i64, Vec<u8>>, String) {
    thread_local!(static CHECKS: Cell<u64> = const { Cell::new(0) });
    let n = CHECKS.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let check = format!("check-{n}-{name}");
    let copy = Rc::new(MemStore::default());
    if let Some(f) = store.files.borrow().get(name) {
        copy.files.borrow_mut().insert(check.clone(), f.clone());
    }
    let c = open(&check, copy).unwrap();
    let ok = integrity(&c);
    let has_t: bool = c
        .query_row(
            "SELECT count(*) > 0 FROM sqlite_schema WHERE name = 't'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !has_t {
        return (BTreeMap::new(), ok);
    }
    let mut st = c.prepare("SELECT k, v FROM t ORDER BY k").unwrap();
    let rows = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    (rows, ok)
}

fn rows(c: &Connection) -> BTreeMap<i64, Vec<u8>> {
    let mut st = c.prepare("SELECT k, v FROM t ORDER BY k").unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Operations that shrink, grow back or rewrite the database file: each is
/// run with every one of its store commits refused in turn. Whatever is
/// refused, the store holds either the state before the operation or the
/// state after it (the one the connection reports), never a mix, and the
/// connection's next commit is stored whole.
#[test]
fn every_refused_store_commit_leaves_a_committed_state() {
    type Setup = &'static str;
    type Op = &'static str;
    let cases: [(&str, Setup, Op); 9] = [
        ("vacuum shrinks", "", "VACUUM"),
        (
            "vacuum to a smaller page size",
            "",
            "PRAGMA page_size=1024; VACUUM",
        ),
        (
            "vacuum to a larger page size",
            "",
            "PRAGMA page_size=16384; VACUUM",
        ),
        (
            "auto_vacuum=FULL delete",
            "PRAGMA auto_vacuum=FULL;",
            "DELETE FROM t WHERE k % 3 <> 0",
        ),
        (
            "auto_vacuum=FULL drop",
            "PRAGMA auto_vacuum=FULL;",
            "DELETE FROM t WHERE k >= 0",
        ),
        (
            "incremental vacuum",
            "PRAGMA auto_vacuum=INCREMENTAL;",
            "DELETE FROM t WHERE k % 2 = 0; PRAGMA incremental_vacuum(40)",
        ),
        (
            "incremental vacuum in a transaction",
            "PRAGMA auto_vacuum=INCREMENTAL;",
            "BEGIN; DELETE FROM t WHERE k % 2 = 1; PRAGMA incremental_vacuum;
             INSERT INTO t VALUES (5000, zeroblob(50000)); COMMIT",
        ),
        (
            "optimize",
            "",
            "CREATE INDEX IF NOT EXISTS tv ON t(length(v)); PRAGMA optimize=0x10002",
        ),
        (
            "grow then shrink in one transaction",
            "PRAGMA auto_vacuum=FULL;",
            "BEGIN; INSERT INTO t SELECT k + 1000, v FROM t;
             DELETE FROM t WHERE k >= 1000; DELETE FROM t WHERE k < 20; COMMIT",
        ),
    ];
    for (case, setup, op) in cases {
        for refuse in 1u64.. {
            let name = format!("refuse-{refuse}-{}.db", case.replace(' ', "-"));
            let store = Rc::new(MemStore::default());
            let mut c = open(&name, store.clone()).unwrap();
            c.execute_batch(&format!(
                "{setup} PRAGMA cache_size=8;
                 CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);
                 INSERT INTO t SELECT i, randomblob(3000 + i % 7 * 900)
                   FROM ({SERIES} SELECT i FROM n WHERE i < 300);
                 DELETE FROM t WHERE k BETWEEN 50 AND 250;"
            ))
            .unwrap();
            let before = rows(&c);
            assert_eq!(stored_state(&store, &name), (before.clone(), "ok".into()));
            let start = store.attempts.get();
            store.fail_attempt.set(Some(start + refuse));
            let r = c.execute_batch(op);
            let attempts = store.attempts.get() - start;
            store.fail_attempt.set(None);
            if !c.is_autocommit() {
                c.execute_batch("ROLLBACK").unwrap();
            }
            let now = rows(&c);
            assert_eq!(integrity(&c), "ok", "{case}, refusing {refuse}: {r:?}");
            let (stored, ok) = stored_state(&store, &name);
            assert_eq!(ok, "ok", "{case}, refusing {refuse}: {r:?}");
            assert!(
                stored == now,
                "{case}, refusing commit {refuse} of {attempts} ({r:?}): the store holds {} rows, \
                 the connection {} (before: {})",
                stored.len(),
                now.len(),
                before.len()
            );
            assert!(
                now == before || r.is_ok(),
                "{case}, refusing {refuse}: a failed operation changed the rows"
            );
            // The connection goes on, and its next commit is stored whole.
            let tx = c.transaction().unwrap();
            tx.execute("INSERT INTO t VALUES (9999, randomblob(7000))", [])
                .unwrap();
            tx.commit().unwrap();
            let now = rows(&c);
            assert_eq!(stored_state(&store, &name), (now.clone(), "ok".into()));
            drop(c);
            let c = open(&name, store.clone()).unwrap();
            assert_eq!(integrity(&c), "ok", "{case}, refusing {refuse}");
            assert!(rows(&c) == now, "{case}, refusing {refuse}: after a reopen");
            if refuse > attempts {
                // Every one of the operation's store commits was refused once.
                break;
            }
        }
    }
}

/// Random transactions over random store refusals, small caches (so pages
/// spill mid-transaction), every auto_vacuum mode and the maintenance
/// statements clax-core runs: no store commit is attempted while a
/// transaction is open, and after every transaction what the store holds,
/// opened on its own, is exactly the model of committed transactions.
#[test]
fn the_store_holds_exactly_the_committed_transactions() {
    for seed in 0..12u64 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1000 + seed);
        let auto_vacuum = ["NONE", "FULL", "INCREMENTAL"][(seed % 3) as usize];
        let name = format!("model-{seed}.db");
        let store = Rc::new(MemStore::default());
        let mut c = open(&name, store.clone()).unwrap();
        c.execute_batch(&format!(
            "PRAGMA auto_vacuum={auto_vacuum}; PRAGMA cache_size={};
             CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB NOT NULL);",
            [5, 20, 2000][rng.random_range(0..3)]
        ))
        .unwrap();
        let mut model: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
        for step in 0..60 {
            // Now and then the store refuses one of the next few commits.
            store.fail_attempt.set(
                rng.random_bool(0.3)
                    .then(|| store.attempts.get() + rng.random_range(1..4)),
            );
            match rng.random_range(0..12) {
                0 => {
                    drop(c);
                    c = open(&name, store.clone()).unwrap();
                    c.execute_batch(&format!(
                        "PRAGMA cache_size={};",
                        [5, 20, 2000][rng.random_range(0..3)]
                    ))
                    .unwrap();
                }
                1 => {
                    let _ = c.execute_batch("VACUUM");
                }
                2 => {
                    let _ = c.execute_batch(
                        "PRAGMA analysis_limit=400; PRAGMA optimize=0x10002; PRAGMA incremental_vacuum",
                    );
                }
                _ => {
                    let mut work = model.clone();
                    let attempts = store.attempts.get();
                    let tx = c.transaction().unwrap();
                    for _ in 0..rng.random_range(1..30) {
                        let r = match rng.random_range(0..5) {
                            0..=2 => {
                                let k = rng.random_range(0..300);
                                let mut v = vec![0u8; rng.random_range(1..9000)];
                                rng.fill_bytes(&mut v);
                                let r = tx.execute(
                                    "INSERT OR REPLACE INTO t VALUES (?1, ?2)",
                                    params![k, v],
                                );
                                if r.is_ok() {
                                    work.insert(k, v);
                                }
                                r.map(drop)
                            }
                            3 => {
                                let lo = rng.random_range(0..300);
                                let hi = lo + rng.random_range(0..60);
                                let r = tx.execute(
                                    "DELETE FROM t WHERE k BETWEEN ?1 AND ?2",
                                    params![lo, hi],
                                );
                                if r.is_ok() {
                                    work.retain(|k, _| !(lo..=hi).contains(k));
                                }
                                r.map(drop)
                            }
                            _ => tx
                                .execute_batch(
                                    "SAVEPOINT s;
                                     INSERT OR REPLACE INTO t SELECT k, zeroblob(4000) FROM t;
                                     ROLLBACK TO s; RELEASE s",
                                )
                                .map(drop),
                        };
                        assert!(r.is_ok(), "seed {seed} step {step}: {r:?}");
                        assert_eq!(
                            store.attempts.get(),
                            attempts,
                            "seed {seed} step {step}: a store commit inside a transaction"
                        );
                    }
                    if rng.random_bool(0.75) {
                        if tx.commit().is_ok() {
                            model = work;
                        }
                    } else {
                        let _ = tx.rollback();
                    }
                }
            }
            store.fail_attempt.set(None);
            if !c.is_autocommit() {
                c.execute_batch("ROLLBACK").unwrap();
            }
            assert!(rows(&c) == model, "seed {seed} step {step}: the connection");
            let (stored, ok) = stored_state(&store, &name);
            assert_eq!(ok, "ok", "seed {seed} step {step}");
            assert!(
                stored == model,
                "seed {seed} step {step}: the store holds {} rows, the model {}",
                stored.len(),
                model.len()
            );
        }
    }
}

/// Pragma table-valued functions prepare the pragma inside, through the
/// same authorizer, and the call syntax `PRAGMA name(value)` is a value
/// too: neither changes what `PRAGMA name=value` may not.
#[test]
fn pragma_functions_cannot_change_the_refused_settings() {
    let store = Rc::new(MemStore::default());
    let c = open("pragma-fn.db", store.clone()).unwrap();
    for sql in [
        "SELECT * FROM pragma_journal_mode('off')",
        "SELECT * FROM pragma_journal_mode('wal')",
        "SELECT * FROM pragma_synchronous('off')",
        "SELECT * FROM pragma_synchronous(0)",
        "SELECT * FROM pragma_locking_mode('normal')",
    ] {
        let r = c.query_row(sql, [], |r| r.get::<_, rusqlite::types::Value>(0));
        assert!(r.is_err(), "{sql}: {r:?}");
    }
    for sql in [
        "PRAGMA journal_mode(wal)",
        "PRAGMA main.journal_mode('off')",
        "PRAGMA synchronous(off)",
    ] {
        assert_refused(&c.execute_batch(sql).expect_err(sql));
    }
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "memory");
    let level: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
    assert_eq!(level, 1);
    let mode: String = c
        .query_row("PRAGMA locking_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "exclusive");
}

/// A caller's own authorizer replaces the one `open` installs; one that
/// delegates to `authorize` keeps its refusals and adds its own.
#[test]
fn a_caller_authorizer_that_delegates_keeps_the_refusals() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let store = Rc::new(MemStore::default());
    let c = open("own-auth.db", store.clone()).unwrap();
    c.authorizer(Some(|ctx: AuthContext<'_>| match authorize(ctx) {
        Authorization::Allow => match ctx.action {
            AuthAction::Pragma {
                pragma_name: "foreign_keys",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        },
        refused => refused,
    }))
    .unwrap();
    for sql in [
        "PRAGMA journal_mode=OFF",
        "PRAGMA synchronous=OFF",
        "PRAGMA locking_mode=NORMAL",
        "PRAGMA foreign_keys=ON",
    ] {
        assert_refused(&c.execute_batch(sql).expect_err(sql));
    }
    c.execute_batch("CREATE TABLE t(k); INSERT INTO t VALUES (1); PRAGMA optimize;")
        .unwrap();
    assert_eq!(count(&c), 1);
}

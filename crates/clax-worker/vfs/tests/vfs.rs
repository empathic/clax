//! The VFS against an in-memory page store, natively: what the store holds
//! after each commit, rollback and truncation, and that a connection
//! reopened on the store sees exactly the committed transactions.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use clax_do_vfs::{CHUNK_SIZE, Chunks, PageStore, StoreError, open, stats};
use rand::RngCore;
use rusqlite::params;

/// A file's size and chunks.
type MemFile = (u64, BTreeMap<u64, Vec<u8>>);

#[derive(Default)]
struct MemStore {
    files: RefCell<HashMap<String, MemFile>>,
    commits: RefCell<u64>,
    fail_commits: RefCell<bool>,
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
        if *self.fail_commits.borrow() {
            return Err("store refused the commit".into());
        }
        assert!(chunks.values().all(|c| c.len() == CHUNK_SIZE));
        let mut files = self.files.borrow_mut();
        let f = files.entry(file.to_owned()).or_default();
        f.0 = size;
        for (i, c) in chunks.iter() {
            f.1.insert(*i, c.clone());
        }
        f.1.split_off(&keep_below);
        *self.commits.borrow_mut() += 1;
        Ok(())
    }
    fn delete(&self, file: &str) -> Result<(), StoreError> {
        self.files.borrow_mut().remove(file);
        Ok(())
    }
}

fn count(c: &rusqlite::Connection) -> i64 {
    c.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn committed_rows_survive_reopen_and_uncommitted_ones_do_not() {
    let store = Rc::new(MemStore::default());
    let c = open("a.db", store.clone()).unwrap();
    c.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB)")
        .unwrap();
    c.execute("INSERT INTO t VALUES (1, 'one')", []).unwrap();
    let commits = *store.commits.borrow();
    assert!(commits >= 2, "each write transaction commits: {commits}");

    // A transaction that never commits leaves the store as it was.
    c.execute_batch("BEGIN; INSERT INTO t VALUES (2, 'two');")
        .unwrap();
    assert_eq!(*store.commits.borrow(), commits);
    drop(c);

    let c = open("a.db", store.clone()).unwrap();
    assert_eq!(count(&c), 1);
    let v: String = c
        .query_row("SELECT v FROM t WHERE k = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, "one");
}

#[test]
fn a_rolled_back_transaction_that_spilled_pages_leaves_the_store_unchanged() {
    let store = Rc::new(MemStore::default());
    let c = open("b.db", store.clone()).unwrap();
    c.execute_batch("PRAGMA cache_size=10; CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);")
        .unwrap();
    let before = store.files.borrow().get("b.db").cloned().unwrap();
    let commits = *store.commits.borrow();
    let tx = c.unchecked_transaction().unwrap();
    for k in 0..2000 {
        tx.execute("INSERT INTO t VALUES (?1, zeroblob(1000))", params![k])
            .unwrap();
    }
    assert_eq!(
        *store.commits.borrow(),
        commits,
        "the spilled pages stay out of the store while the transaction is open"
    );
    tx.rollback().unwrap();
    // The rollback writes the journaled pages back over the spilled ones and
    // syncs: one commit, of the restored state (none had nothing spilled).
    assert_eq!(*store.commits.borrow(), commits + 1);
    assert_eq!(store.files.borrow().get("b.db").cloned().unwrap(), before);
    assert_eq!(count(&c), 0);
    let ok: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn a_few_megabytes_round_trip_and_vacuum_truncates_the_store() {
    let store = Rc::new(MemStore::default());
    let c = open("c.db", store.clone()).unwrap();
    c.execute_batch("CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB)")
        .unwrap();
    let mut rng = rand::rng();
    let mut blob = vec![0u8; 64 * 1024];
    let tx = c.unchecked_transaction().unwrap();
    for k in 0..80 {
        rng.fill_bytes(&mut blob);
        tx.execute("INSERT INTO t VALUES (?1, ?2)", params![k, blob])
            .unwrap();
    }
    tx.commit().unwrap();
    let st = stats("c.db").unwrap();
    let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    assert!(pages > 1200, "{pages} pages");
    assert!(st.chunk_writes as i64 >= pages - 1, "{st:?}");
    let sum = |c: &rusqlite::Connection| -> Vec<Vec<u8>> {
        let mut s = c.prepare("SELECT v FROM t ORDER BY k").unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let before = sum(&c);
    drop(c);

    let c = open("c.db", store.clone()).unwrap();
    assert_eq!(sum(&c), before);
    c.execute("DELETE FROM t WHERE k >= 10", []).unwrap();
    c.execute_batch("VACUUM").unwrap();
    let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    let files = store.files.borrow();
    let (size, chunks) = files.get("c.db").unwrap();
    assert_eq!(*size, pages as u64 * CHUNK_SIZE as u64);
    assert_eq!(
        chunks.len() as i64,
        pages,
        "truncation deleted the chunks past the end"
    );
    drop(files);
    drop(c);

    let c = open("c.db", store.clone()).unwrap();
    assert_eq!(count(&c), 10);
    let ok: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn a_failed_commit_fails_the_transaction_and_the_store_keeps_the_last_commit() {
    let store = Rc::new(MemStore::default());
    let c = open("d.db", store.clone()).unwrap();
    c.execute_batch(
        "CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB); INSERT INTO t VALUES (1, 'a');",
    )
    .unwrap();
    *store.fail_commits.borrow_mut() = true;
    let err = c.execute("INSERT INTO t VALUES (2, 'b')", []);
    assert!(err.is_err(), "{err:?}");
    *store.fail_commits.borrow_mut() = false;
    drop(c);
    let c = open("d.db", store.clone()).unwrap();
    assert_eq!(count(&c), 1);
}

#[test]
fn databases_with_different_stores_are_independent() {
    let s1 = Rc::new(MemStore::default());
    let s2 = Rc::new(MemStore::default());
    let a = open("e1.db", s1.clone()).unwrap();
    let b = open("e2.db", s2.clone()).unwrap();
    a.execute_batch("CREATE TABLE t(k); INSERT INTO t VALUES (1);")
        .unwrap();
    b.execute_batch("CREATE TABLE t(k); INSERT INTO t VALUES (1), (2);")
        .unwrap();
    assert_eq!(count(&a), 1);
    assert_eq!(count(&b), 2);
    assert!(s1.files.borrow().contains_key("e1.db"));
    assert!(!s1.files.borrow().contains_key("e2.db"));
}

//! The calibration read of the daemon latency gate (`scripts/perf-daemon.py`):
//! a fixed SQLite read on a private database that Clax's schema and queries
//! cannot move, through the same bundled SQLite and build flags as the
//! store. The gate times it between the gallery requests and judges each
//! request as a ratio to it, which leaves out the machine's speed and load.

use crate::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{Duration, Instant};

/// Threads in the calibration database.
pub const THREADS: usize = 3000;
/// Comments per thread.
pub const PER_THREAD: usize = 4;

/// Per thread, two correlated index lookups, as the attention query makes:
/// whether one author wrote in it, and its latest comment by anyone else.
const QUERY: &str = "SELECT count(*), sum(own), max(other) FROM (
    SELECT t.id,
      EXISTS (SELECT 1 FROM c WHERE c.tid = t.id AND c.author = 'u_a') AS own,
      (SELECT max(c.created) FROM c WHERE c.tid = t.id AND (c.author IS NULL OR c.author != 'u_a')) AS other
    FROM t ORDER BY t.grp, t.created)";

/// The calibration database, open.
pub struct Calibration {
    conn: Connection,
}

impl Calibration {
    /// Creates the database at `path` (which must not exist) with
    /// [`THREADS`] threads of [`PER_THREAD`] comments, the same rows on
    /// every machine, and runs the read once to warm the page cache.
    pub fn create(path: &Path) -> Result<Calibration> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE t (id TEXT PRIMARY KEY, grp INTEGER NOT NULL, created TEXT NOT NULL);
             CREATE TABLE c (id INTEGER PRIMARY KEY, tid TEXT NOT NULL, author TEXT,
                             created TEXT NOT NULL, body TEXT NOT NULL);
             CREATE INDEX c_by_t ON c(tid, created);",
        )?;
        let tx = conn.transaction()?;
        // A fixed linear congruential sequence picks each comment's author.
        let mut seed: u64 = 1;
        let body = "x".repeat(60);
        for i in 0..THREADS {
            let tid = format!("t{i:06}");
            tx.execute(
                "INSERT INTO t VALUES (?1, ?2, ?3)",
                params![tid, (i % 300) as i64, format!("2026-01-01T{i:06}")],
            )?;
            for k in 0..PER_THREAD {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let author = match (seed >> 33) % 3 {
                    0 => Some("u_a"),
                    1 => Some("u_b"),
                    _ => None,
                };
                tx.execute(
                    "INSERT INTO c (tid, author, created, body) VALUES (?1, ?2, ?3, ?4)",
                    params![tid, author, format!("2026-01-02T{i:06}{k:02}"), body],
                )?;
            }
        }
        tx.commit()?;
        let cal = Calibration { conn };
        cal.run()?;
        Ok(cal)
    }

    /// Runs the read once; how long it took.
    pub fn run(&self) -> Result<Duration> {
        let t0 = Instant::now();
        let mut st = self.conn.prepare_cached(QUERY)?;
        let (n, own): (i64, i64) = st.query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        std::hint::black_box((n, own));
        Ok(t0.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_covers_every_thread_the_same_way_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let cal = Calibration::create(&dir.path().join("cal.db")).unwrap();
        let counts = |c: &Calibration| -> (i64, i64) {
            c.conn
                .query_row(QUERY, [], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        let (threads, own) = counts(&cal);
        assert_eq!(threads, THREADS as i64);
        // The fixed sequence gives the same authors on every machine.
        let again = Calibration::create(&dir.path().join("again.db")).unwrap();
        assert_eq!(counts(&again), (threads, own));
        assert!(own > 0 && own < threads, "{own}");
        cal.run().unwrap();
    }
}

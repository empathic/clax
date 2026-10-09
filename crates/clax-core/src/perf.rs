//! The calibration read of the daemon latency gate (`scripts/perf-daemon.py`):
//! a fixed SQLite read on a private in-memory database that Clax's schema
//! and queries cannot move, through the same bundled SQLite and build flags
//! as the store. The daemon runs it on a store worker
//! (`POST /api/admin/perf/calibrate`), and the gate times it between the
//! gallery requests and judges each request as a ratio to it, which leaves
//! out the machine's speed and load.

use crate::Result;
use rusqlite::{Connection, params};
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
    /// Creates the database in memory with [`THREADS`] threads of
    /// [`PER_THREAD`] comments, the same rows on every machine, and runs the
    /// read once. The database and temporary B-trees stay in memory, so the
    /// read costs memory and CPU only.
    ///
    /// # Errors
    /// SQLite's.
    pub fn create() -> Result<Calibration> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "PRAGMA temp_store=MEMORY;
             CREATE TABLE t (id TEXT PRIMARY KEY, grp INTEGER NOT NULL, created TEXT NOT NULL);
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

    /// The workload, pinned: a change to the rows or the read changes what
    /// the gate's `list_alone_ratio` and `attention_alone_ratio` budgets
    /// were measured against (scripts/perf-daemon-budget.json), so it means
    /// measuring them again.
    #[test]
    fn the_workload_is_the_one_the_budgets_were_measured_on() {
        let cal = Calibration::create().unwrap();
        let got: (i64, i64, String) = cal
            .conn
            .query_row(QUERY, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(got, (3000, 2440, "2026-01-02T00299903".to_string()));
        let pages: i64 = cal
            .conn
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pages, 463);
        cal.run().unwrap();
    }
}

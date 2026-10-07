//! The first start of a build with the audit log on a home that has
//! history: the daemon records the audit backfill before it serves, reports
//! its progress in `starting.json`, and the client that started it waits on
//! that progress rather than on a fixed deadline.

use assert_cmd::Command;
use clax_core::store::migrations::{AUDIT_MIGRATION, MIGRATIONS};
use rusqlite::{Connection, params};

/// A test's scratch home; dropping it stops any daemon started there.
struct Scratch(tempfile::TempDir);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = clax(self.0.path()).arg("stop").output();
    }
}

fn clax(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("clax").unwrap();
    c.env("HOME", dir)
        .env("CLAX_HOME", dir.join("ax"))
        .env_remove("CLAX_CONFIG_DIR")
        .env("CLAX_CODEX_BIN", "");
    c
}

const A: &str = "http://localhost:7702";
const B: &str = "http://localhost:7703";
/// Artifacts of one version each, with a file of [`FILE_BYTES`].
const ARTIFACTS: usize = 40;
const FILE_BYTES: usize = 1 << 20;

/// A home at the schema just before the audit (main's joined sites), as
/// the owner's is: html artifacts with a 1 MiB file each, and a joined site
/// whose page on `B` was merged away into `A`'s.
fn old_home(dir: &std::path::Path) -> clax_core::Home {
    let home = clax_core::Home::at(dir.join("ax"));
    home.ensure_dirs().unwrap();
    let mut c = Connection::open(home.db_path()).unwrap();
    let before = AUDIT_MIGRATION as usize - 1;
    let tx = c.transaction().unwrap();
    for sql in &MIGRATIONS[..before] {
        tx.execute_batch(sql).unwrap();
    }
    tx.pragma_update(None, "user_version", before as u32)
        .unwrap();
    let ts = |s: usize| format!("2026-01-01T00:{:02}:{:02}.000Z", s / 60, s % 60);
    let add = |kind: &str, at: &str, bytes: &[u8]| {
        let id = clax_core::ArtifactId::generate();
        let dir = home.version_dir(&id, 1);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), bytes).unwrap();
        tx.execute(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version, kind)
             VALUES (?1, 'T', ?2, ?2, 1, '1', ?3)",
            params![id.as_str(), at, kind],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO versions (artifact_id, n, created_at, files_json) VALUES (?1, 1, ?2, ?3)",
            params![
                id.as_str(),
                at,
                serde_json::json!({"index.html": {"content_type": "text/html", "size": bytes.len()}})
                    .to_string()
            ],
        )
        .unwrap();
        id.as_str().to_string()
    };
    for i in 0..ARTIFACTS {
        let mut bytes = vec![b'x'; FILE_BYTES];
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        add("html", &ts(i), &bytes);
    }
    let site = add("live", &ts(100), b"<p>A</p>");
    let merged = add("live", &ts(101), b"<p>B</p>");
    tx.execute_batch(&format!(
        "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES ('{site}', '{A}', '/', '{t}');
         INSERT INTO live_sites (origin, site, joined_at, last_used_at) VALUES
            ('{A}', '{A}', '{t}', '{t}'), ('{B}', '{A}', '{j}', '{j}');
         INSERT INTO live_merged_pages (artifact_id, origin, path, merged_into, merged_at)
            VALUES ('{merged}', '{B}', '/', '{site}', '{j}');",
        t = ts(100),
        j = ts(102),
    ))
    .unwrap();
    tx.commit().unwrap();
    home
}

#[test]
fn a_first_start_waits_on_the_backfills_progress_and_then_serves() {
    let dir = Scratch(tempfile::tempdir().unwrap());
    let home = old_home(dir.0.path());
    // The client reads the daemon's progress while it starts (the waiting
    // itself, past a short start window, is pinned by the client's tests
    // with stand-in daemons).
    let out = clax(dir.0.path())
        .args(["status", "--start", "--json"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("recording existing history for the audit log"),
        "the client said what the daemon was doing: {err}"
    );
    let log = std::fs::read_to_string(home.log_path()).unwrap_or_default();
    for line in log.lines().filter(|l| l.contains("audit backfill")) {
        eprintln!("{line}");
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["running"], true, "{v}");
    assert!(
        !home.starting_json().exists(),
        "starting.json is gone once it serves"
    );
    let c = Connection::open(home.db_path()).unwrap();
    let (version, marked): (u32, bool) = c
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),
                    EXISTS(SELECT 1 FROM install WHERE k = 'backfill')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(version, MIGRATIONS.len() as u32);
    assert!(marked);
    let kinds: Vec<String> = c
        .prepare("SELECT kind FROM audit_events WHERE backfilled = 1 ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let count = |k: &str| kinds.iter().filter(|x| *x == k).count();
    assert_eq!(count("version.publish"), ARTIFACTS);
    assert_eq!(count("live.snapshot"), 2);
    assert_eq!(
        count("live.page"),
        2,
        "the merged-away page is a live page too"
    );
    assert_eq!(count("live.join"), 1);
    assert_eq!(count("live.page_merge"), 1);
    let hashed: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM versions WHERE content_sha256 IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hashed as usize, ARTIFACTS + 2);
}

#[test]
fn status_names_a_starting_daemons_progress() {
    let dir = Scratch(tempfile::tempdir().unwrap());
    let home = clax_core::Home::at(dir.0.path().join("ax"));
    home.ensure_dirs().unwrap();
    // A daemon still starting: this test's own process stands in for it.
    std::fs::write(
        home.starting_json(),
        serde_json::json!({"pid": std::process::id(), "phase": "hashing files", "done": 120,
            "total": 800, "bytes": 210u64 << 20, "heartbeat_at": "2026-01-01T00:00:00.000Z"})
        .to_string(),
    )
    .unwrap();
    let out = clax(dir.0.path()).arg("status").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "starting: recording existing history for the audit log (hashing files) (pid"
        ) && text.contains("120 of 800 rows, 210 MiB hashed"),
        "{text}"
    );
    let out = clax(dir.0.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["running"], false);
    assert_eq!(v["starting"]["done"], 120);
}

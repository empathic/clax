//! Homes that installed builds of the Clax app wrote, opened through the
//! embedded core (spec 2026-10-19 §10.2): each is backed up, adopted into
//! this build's numbering and migrated to its end, and keeps every row.

use clax_embed::frame::{self, As, RequestHead, ResponseHead};
use clax_embed::{Embed, OpenEvent, OpenOptions};
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// The Mac app's home at this branch's earlier numbering, schema 43
/// (`tests/fixtures/homes/flutter-app-43.sql`, whose header says where its
/// schema and rows come from).
const FLUTTER_APP_43: &str = include_str!("fixtures/homes/flutter-app-43.sql");

/// The `-- user_version: N` line of a fixture's header.
fn user_version_of(sql: &str) -> u32 {
    sql.lines()
        .find_map(|l| l.strip_prefix("-- user_version: "))
        .expect("a user_version line")
        .trim()
        .parse()
        .unwrap()
}

/// A database at `path` made from a `sqlite3 .dump` and its header's
/// `user_version` (a dump does not carry it).
fn load_dump(path: &Path, sql: &str) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch(sql).unwrap();
    db.pragma_update(None, "user_version", user_version_of(sql))
        .unwrap();
    db
}

/// The home at `<dir>/home` holding the fixture's database, and a copy of
/// that database at `<dir>/before.db` that nothing opens but the checks.
fn fixture_home(dir: &Path, sql: &str) -> (clax_core::Home, PathBuf) {
    let home = clax_core::Home::at(dir.join("home"));
    home.ensure_dirs().unwrap();
    load_dump(&home.db_path(), sql);
    let before = dir.join("before.db");
    load_dump(&before, sql);
    (home, before)
}

/// Opens the core on `home` and waits for `Ready`, keeping each phase it
/// reported on the way.
fn open(home: &clax_core::Home) -> (Embed, mpsc::Receiver<OpenEvent>, Vec<String>) {
    let (tx, rx) = mpsc::channel();
    let embed = Embed::open(
        OpenOptions {
            home: home.root().to_path_buf(),
            origins: clax_http::state::EmbedOrigins {
                shell: "clax://app".into(),
                content: "clax://{id}.c".into(),
            },
            log: None,
        },
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .expect("the home opens");
    let mut phases = Vec::new();
    loop {
        match rx.recv().expect("an open event") {
            OpenEvent::Opening { phase, .. } => phases.push(phase),
            OpenEvent::Ready => break,
            e => panic!("the home did not open: {e:?}"),
        }
    }
    (embed, rx, phases)
}

/// Closes the core and waits until it has let go of the home.
fn close(embed: Embed, rx: &mpsc::Receiver<OpenEvent>) {
    embed.close();
    loop {
        match rx.recv().expect("an open event") {
            OpenEvent::Closed => return,
            _ => continue,
        }
    }
}

async fn get_json(embed: &Embed, path: &str) -> Value {
    let head = RequestHead {
        method: "GET".into(),
        path: path.into(),
        headers: vec![],
        as_: As::Owner,
        host: None,
    };
    let res = embed
        .call(&frame::encode(&head, b""))
        .await
        .expect("a frame");
    let (h, body) = frame::split(&res).expect("a response frame");
    let h: ResponseHead = serde_json::from_slice(h).unwrap();
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    assert_eq!(h.status, 200, "GET {path}: {v}");
    v
}

/// The copies `Store::open` made before migrating.
fn backups(home: &clax_core::Home) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(clax_store::backup::dir(home))
        .map(|d| d.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// Each table of `db` but SQLite's own, with its columns.
fn tables(db: &Connection) -> Vec<(String, Vec<String>)> {
    let names: Vec<String> = db
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             AND sql NOT LIKE 'CREATE VIRTUAL TABLE%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    names
        .into_iter()
        .map(|t| {
            let cols = db
                .prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (t, cols)
        })
        .collect()
}

fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Reads each document of the old home at `before` through the core, as
/// the owner, and checks its version. An artifact's first call moves its
/// documents to its own store, waiting out a move the core's start began,
/// so once this returns every artifact holding documents has moved.
async fn read_old_docs(embed: &Embed, before: &Path) {
    let old = Connection::open(before).unwrap();
    let rows: Vec<(String, String, i64)> = old
        .prepare("SELECT artifact_id, path, version FROM docs ORDER BY artifact_id, path")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!rows.is_empty(), "the old home holds documents");
    for (aid, path, version) in rows {
        let doc = get_json(embed, &format!("/api/artifacts/{aid}/docs/{path}")).await;
        assert_eq!(
            doc["doc"]["version"].as_i64(),
            Some(version),
            "{aid} {path}: {doc}"
        );
    }
}

/// Each row of the old home's `table` (`docs` or `doc_authors`) is in its
/// artifact's own store in `home`.
fn assert_moved(old: &Connection, home: &clax_core::Home, table: &str) {
    let sql = match table {
        "docs" => "SELECT artifact_id, path, json, version FROM docs",
        _ => "SELECT artifact_id, path, author, 0 FROM doc_authors",
    };
    let mut q = old.prepare(sql).unwrap();
    let rows: Vec<(String, String, String, i64)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (aid, path, x, version) in rows {
        let id = clax_core::ArtifactId::parse(&aid).unwrap();
        let store = Connection::open_with_flags(
            home.app_data_path(&id),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap_or_else(|e| panic!("{aid} has a store of its own: {e}"));
        let found: i64 = match table {
            "docs" => store
                .query_row(
                    "SELECT count(*) FROM docs WHERE path = ?1 AND json = ?2 AND version = ?3",
                    rusqlite::params![path, x, version],
                    |r| r.get(0),
                )
                .unwrap(),
            _ => store
                .query_row(
                    "SELECT count(*) FROM doc_authors WHERE path = ?1 AND author = ?2",
                    rusqlite::params![path, x],
                    |r| r.get(0),
                )
                .unwrap(),
        };
        assert_eq!(
            found, 1,
            "{table}: {aid} {path} is not in its artifact's store"
        );
    }
}

/// Every row of every table in `before` is in `after` unchanged, column by
/// column (the migrations since add tables and columns, and change no
/// row); returns each table's row count in `before`. With `moved`, the
/// home `after` is the database of, the app data each artifact keeps in a
/// store of its own since (`docs`, `doc_authors`, `leases`) is checked
/// there instead, each document with its data and version and each of its
/// authors; and an asset's page authors, which the asset keys migration
/// names `<artifact>/<asset>`, under that name.
fn assert_rows_kept(
    before: &Path,
    after: &Path,
    moved: Option<&clax_core::Home>,
) -> Vec<(String, i64)> {
    let db =
        Connection::open_with_flags(after, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    db.execute("ATTACH DATABASE ?1 AS before", [before.to_str().unwrap()])
        .unwrap();
    let old = Connection::open(before).unwrap();
    let mut counts = Vec::new();
    for (t, cols) in tables(&old) {
        if let Some(home) = moved {
            if ["docs", "doc_authors", "leases"].contains(&t.as_str()) {
                if t != "leases" {
                    assert_moved(&old, home, &t);
                }
                counts.push((
                    t.clone(),
                    count(&old, &format!("SELECT count(*) FROM \"{t}\"")),
                ));
                continue;
            }
            if t == "page_authors" {
                let lost = count(
                    &db,
                    "SELECT count(*) FROM before.page_authors b WHERE NOT EXISTS (
                         SELECT 1 FROM main.page_authors a WHERE a.kind = b.kind AND a.author = b.author
                           AND (a.id = b.id OR (b.kind = 'asset' AND a.id LIKE '%/' || b.id)))",
                );
                assert_eq!(
                    lost, 0,
                    "page_authors: {lost} rows of the old home are gone"
                );
                counts.push((t.clone(), count(&old, "SELECT count(*) FROM page_authors")));
                continue;
            }
        }
        let cols = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let lost = count(
            &db,
            &format!(
                "SELECT count(*) FROM (SELECT {cols} FROM before.\"{t}\" \
                 EXCEPT SELECT {cols} FROM main.\"{t}\")"
            ),
        );
        assert_eq!(
            lost, 0,
            "{t}: {lost} rows of the old home are gone or changed"
        );
        counts.push((
            t.clone(),
            count(&old, &format!("SELECT count(*) FROM \"{t}\"")),
        ));
    }
    counts
}

#[tokio::test]
async fn the_installed_flutter_app_home_backs_up_and_is_adopted() {
    let dir = tempfile::tempdir().unwrap();
    let (home, before) = fixture_home(dir.path(), FLUTTER_APP_43);
    let old = Connection::open(&before).unwrap();
    let artifact_ids: Vec<String> = old
        .prepare("SELECT id FROM artifacts ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(artifact_ids.len(), 2);

    let (embed, rx, phases) = open(&home);
    assert_eq!(
        phases.first().map(String::as_str),
        Some("backing up"),
        "{phases:?}"
    );

    // The gallery lists every artifact, with every version and thread.
    let list = get_json(&embed, "/api/artifacts").await;
    let mut listed: Vec<String> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    listed.sort();
    assert_eq!(listed, artifact_ids, "{list}");
    for id in &artifact_ids {
        let versions = get_json(&embed, &format!("/api/artifacts/{id}/versions")).await;
        let want = count(
            &old,
            &format!("SELECT count(*) FROM versions WHERE artifact_id = '{id}'"),
        );
        assert_eq!(
            versions["versions"].as_array().map(Vec::len),
            Some(want as usize),
            "{versions}"
        );
        let threads = get_json(&embed, &format!("/api/artifacts/{id}/threads")).await;
        let want = count(
            &old,
            &format!("SELECT count(*) FROM threads WHERE artifact_id = '{id}'"),
        );
        assert_eq!(
            threads["threads"].as_array().map(Vec::len),
            Some(want as usize),
            "{threads}"
        );
    }
    read_old_docs(&embed, &before).await;
    close(embed, &rx);

    // One copy made before anything was written, at the old version, with
    // the old home's rows.
    let copies = backups(&home);
    assert_eq!(copies.len(), 1, "{copies:?}");
    let name = copies[0].file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("clax-43-") && name.ends_with(".db"),
        "{name}"
    );
    assert_eq!(
        count(
            &Connection::open(&copies[0]).unwrap(),
            "PRAGMA user_version"
        ),
        43
    );
    assert_rows_kept(&before, &copies[0], None);

    // Adopted and migrated to this build's last migration, the tailnet
    // login's.
    let version = clax_store::schema_version(&home).unwrap();
    assert_eq!(
        version,
        Some(clax_store::migrations::TAILSCALE_LOGIN_MIGRATION)
    );
    assert_eq!(
        version,
        Some(clax_store::migrations::MIGRATIONS.len() as u32)
    );
    let counts = assert_rows_kept(&before, &home.db_path(), Some(&home));
    for t in [
        "artifacts",
        "versions",
        "threads",
        "comments",
        "docs",
        "db_read_grants",
        "sessions",
    ] {
        let n = counts.iter().find(|(name, _)| name == t).map(|(_, n)| *n);
        assert!(
            n.unwrap_or(0) > 0,
            "the fixture holds no {t} row: {counts:?}"
        );
    }
    let after = Connection::open(home.db_path()).unwrap();
    // Its artifacts declare nothing (`{}`), so none has an approval to keep.
    for t in [
        "legal_agreements",
        "briefs",
        "jobs",
        "job_steps",
        "capability_approvals",
    ] {
        assert_eq!(
            count(&after, &format!("SELECT count(*) FROM {t}")),
            0,
            "{t}"
        );
    }

    // A second open runs nothing and copies nothing.
    let (embed, rx, phases) = open(&home);
    assert!(!phases.iter().any(|p| p == "backing up"), "{phases:?}");
    close(embed, &rx);
    assert_eq!(backups(&home).len(), 1);
    assert_eq!(clax_store::schema_version(&home).unwrap(), version);
}

/// Adds `n` copies of the one row of `table` that `filter` picks, the
/// columns in `set` given by SQL over `k` (0 to n - 1) and the row's own
/// columns (`src.<column>`).
fn fan_out(db: &Connection, table: &str, filter: &str, n: u32, set: &[(&str, &str)]) {
    let cols: Vec<String> = db
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let exprs = cols
        .iter()
        .map(|c| {
            set.iter()
                .find(|(name, _)| name == c)
                .map_or_else(|| format!("src.\"{c}\""), |(_, e)| e.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let names = cols
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let added = db
        .execute(
            &format!(
                "WITH RECURSIVE seq(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM seq WHERE k < {last}) \
                 INSERT INTO \"{table}\" ({names}) SELECT {exprs} FROM seq, \"{table}\" AS src WHERE {filter}",
                last = n - 1
            ),
            [],
        )
        .unwrap();
    assert_eq!(added, n as usize, "{table}");
}

/// The fixture grown to `scripts/perf-daemon.sh`'s seed
/// (`scripts/perf-daemon-budget.json`): 300 artifacts more, each of one
/// version and eight threads of three comments, copied from the fixture's
/// own rows at its schema.
fn grow_to_perf_seed(path: &Path) {
    let db = Connection::open(path).unwrap();
    let (artifact, thread, comment): (String, String, String) = db
        .query_row(
            "SELECT t.artifact_id, t.id, c.id FROM threads t JOIN comments c ON c.thread_id = t.id \
             ORDER BY t.id, c.id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let tx = db.unchecked_transaction().unwrap();
    // Artifact IDs of digits after a `p` are in ArtifactId's alphabet; the
    // thread and comment IDs keep a ULID's length and alphabet.
    let aid = "'p' || printf('%011d', k)";
    fan_out(
        &tx,
        "artifacts",
        &format!("src.id = '{artifact}'"),
        300,
        &[
            ("id", aid),
            ("title", "'Perf ' || k"),
            ("current_version", "1"),
        ],
    );
    fan_out(
        &tx,
        "versions",
        &format!("src.artifact_id = '{artifact}' AND src.n = 1"),
        300,
        &[("artifact_id", aid)],
    );
    fan_out(
        &tx,
        "threads",
        &format!("src.id = '{thread}'"),
        2400,
        &[
            ("id", "'01PERFTHREAD' || printf('%014d', k)"),
            ("artifact_id", "'p' || printf('%011d', k / 8)"),
            ("version_n", "1"),
        ],
    );
    fan_out(
        &tx,
        "comments",
        &format!("src.id = '{comment}'"),
        7200,
        &[
            ("id", "'01PERFCOMMNT' || printf('%014d', k)"),
            ("thread_id", "'01PERFTHREAD' || printf('%014d', k / 3)"),
        ],
    );
    tx.commit().unwrap();
}

#[tokio::test]
async fn the_perf_home_opens_through_the_core() {
    let dir = tempfile::tempdir().unwrap();
    let (home, before) = fixture_home(dir.path(), FLUTTER_APP_43);
    grow_to_perf_seed(&home.db_path());
    grow_to_perf_seed(&before);

    // No time is asserted: the open of a large home at the old numbering
    // is the case, and its migrations only add tables.
    let (embed, rx, _) = open(&home);
    let list = get_json(&embed, "/api/artifacts").await;
    assert_eq!(list["artifacts"].as_array().map(Vec::len), Some(302));
    let threads = get_json(&embed, "/api/artifacts/p00000000299/threads").await;
    assert_eq!(
        threads["threads"].as_array().map(Vec::len),
        Some(8),
        "{threads}"
    );
    read_old_docs(&embed, &before).await;
    close(embed, &rx);

    assert_eq!(
        clax_store::schema_version(&home).unwrap(),
        Some(clax_store::migrations::MIGRATIONS.len() as u32)
    );
    let counts = assert_rows_kept(&before, &home.db_path(), Some(&home));
    let of = |t: &str| counts.iter().find(|(n, _)| n == t).unwrap().1;
    assert_eq!(
        (of("artifacts"), of("threads"), of("comments")),
        (302, 2402, 7204)
    );
}

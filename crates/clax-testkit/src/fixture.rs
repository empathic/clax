//! The fixtures a test daemon loads: whole databases with their files.

use clax_core::Home;

/// The directory of the fixtures [`TestServer::from_fixture_with_clock`]
/// loads: this crate's `fixtures/`.
pub fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Writes fixture `sql` into `home`: runs it on a new database at the home's
/// database path, then writes each row of its `fixture_files (path,
/// content)` table to that path under the home root and drops the table.
/// A fixture is a whole database (schema, rows and `user_version`) with the
/// files its rows name (version files, clips, assets).
///
/// # Panics
/// When the home already has a database, the SQL fails, or a file path is
/// not a plain relative path.
pub fn load_fixture(home: &Home, sql: &str) {
    home.ensure_dirs().unwrap();
    assert!(
        !home.db_path().exists(),
        "{} exists",
        home.db_path().display()
    );
    let conn = rusqlite::Connection::open(home.db_path()).unwrap();
    conn.execute_batch(sql).unwrap();
    let files: Vec<(String, Vec<u8>)> = {
        let mut st = conn
            .prepare("SELECT path, content FROM fixture_files ORDER BY path")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    for (rel, bytes) in files {
        let rel = std::path::Path::new(&rel);
        assert!(
            rel.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "fixture file {} is not a plain relative path",
            rel.display()
        );
        let path = home.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    conn.execute_batch("DROP TABLE fixture_files").unwrap();
}

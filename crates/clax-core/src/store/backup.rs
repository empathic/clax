//! Copies of the database taken before its schema is upgraded.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::{CoreError, Home, Result};

/// How many copies `<home>/backups` keeps: the newest.
pub const KEEP: usize = 3;

/// The directory the copies are kept in.
pub fn dir(home: &Home) -> PathBuf {
    home.root().join("backups")
}

/// Before a migration changes an existing database (one whose
/// `user_version` is above 0 and below `known`), copies it whole, with
/// what its write-ahead log holds, to
/// `<home>/backups/clax-<version>-<UTC time>.db` (`VACUUM INTO` a
/// temporary name, then renamed, so a copy that exists is complete), then
/// removes all but the newest [`KEEP`] copies. Returns the copy's path, or
/// `None` when nothing is to be migrated.
///
/// # Errors
/// [`CoreError::MigrationFailed`] when the copy cannot be made: the caller
/// must not migrate then. Removing old copies never fails it.
pub fn before_migrate(
    conn: &Connection,
    home: &Home,
    known: u32,
    now: DateTime<Utc>,
) -> Result<Option<PathBuf>> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 0 || version >= known {
        return Ok(None);
    }
    let fail = |e: &dyn std::fmt::Display| CoreError::MigrationFailed {
        at: version,
        detail: format!("could not copy it to {} first: {e}", dir(home).display()),
    };
    let dir = dir(home);
    std::fs::create_dir_all(&dir).map_err(|e| fail(&e))?;
    let name = format!("clax-{version}-{}.db", now.format("%Y%m%dT%H%M%S%3fZ"));
    let path = dir.join(&name);
    let tmp = dir.join(format!(".{name}.{:016x}.tmp", rand::random::<u64>()));
    let _ = std::fs::remove_file(&tmp);
    let tmp_str = tmp.to_str().ok_or_else(|| fail(&"the path is not UTF-8"))?;
    if let Err(e) = conn.execute("VACUUM INTO ?1", [tmp_str]) {
        let _ = std::fs::remove_file(&tmp);
        return Err(fail(&e));
    }
    std::fs::rename(&tmp, &path).map_err(|e| fail(&e))?;
    tracing::warn!(
        from = version,
        to = known,
        copy = %path.display(),
        "upgrading the database; copied it first"
    );
    prune(&dir);
    Ok(Some(path))
}

/// The copies in `dir`, oldest first (by the time in their names).
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut copies: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_prefix("clax-")?.strip_suffix(".db")?;
            let (_, at) = stem.rsplit_once('-')?;
            Some((at.to_string(), e.path()))
        })
        .collect();
    copies.sort();
    copies.into_iter().map(|(_, p)| p).collect()
}

fn prune(dir: &Path) {
    let copies = list(dir);
    for old in copies.iter().take(copies.len().saturating_sub(KEEP)) {
        if let Err(e) = std::fs::remove_file(old) {
            tracing::warn!(copy = %old.display(), error = %e, "could not remove an old database copy");
        }
    }
}

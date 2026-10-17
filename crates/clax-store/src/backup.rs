//! Copies of the database taken before its schema is upgraded.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use clax_core::{CoreError, Home, Result};

/// How many of the newest copies `<home>/backups` keeps (one older copy
/// may be kept besides them; see `prune`).
pub const KEEP: usize = 3;

/// The directory the copies are kept in.
pub fn dir(home: &Home) -> PathBuf {
    home.root().join("backups")
}

/// Before a migration changes an existing database (one whose
/// `user_version` is above 0 and below `known`, or any existing database
/// when `repair` says a repair will change it), copies it whole, with
/// what its write-ahead log holds, to
/// `<home>/backups/clax-<version>-<UTC time>.db` (`VACUUM INTO` a
/// temporary name, then renamed, so a copy that exists is complete), then
/// prunes the older copies (see `prune`). Returns the copy's path, or
/// `None` when nothing is to be migrated.
///
/// # Errors
/// [`CoreError::MigrationFailed`] when the copy cannot be made: the caller
/// must not migrate then. Removing old copies never fails it.
pub fn before_migrate(
    conn: &Connection,
    home: &Home,
    known: u32,
    repair: bool,
    now: DateTime<Utc>,
) -> Result<Option<PathBuf>> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 0 || (version >= known && !repair) {
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

/// The schema version in a copy's name (`clax-<version>-<time>.db`).
fn version_of(copy: &Path) -> Option<u32> {
    let name = copy.file_name()?.to_str()?;
    name.strip_prefix("clax-")?.split_once('-')?.0.parse().ok()
}

/// Removes all but the newest [`KEEP`] copies, and keeps besides them the
/// newest copy at a version below the newest copy's: the database as it
/// was before the upgrade now under way, which repeated starts that fail
/// partway through it (each copying the database at the version it
/// reached) would otherwise rotate out.
fn prune(dir: &Path) {
    let copies = list(dir);
    let newest = copies.last().and_then(|c| version_of(c));
    let pinned = copies
        .iter()
        .rev()
        .find(|c| matches!((version_of(c), newest), (Some(v), Some(n)) if v < n));
    for old in copies.iter().take(copies.len().saturating_sub(KEEP)) {
        if Some(old) == pinned {
            continue;
        }
        if let Err(e) = std::fs::remove_file(old) {
            tracing::warn!(copy = %old.display(), error = %e, "could not remove an old database copy");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        list(dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn failed_retries_keep_the_copy_from_before_the_upgrade() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(tmp.path().join("ax"));
        let backups = dir(&home);
        std::fs::create_dir_all(&backups).unwrap();
        // A start at version 9 copied, migrated to 10, and failed the next step.
        std::fs::write(backups.join("clax-9-20261001T000000000Z.db"), b"v9").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 10).unwrap();
        for day in 2..=6 {
            let now = format!("2026-10-{day:02}T00:00:00Z").parse().unwrap();
            assert!(
                before_migrate(&conn, &home, 12, false, now)
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(
            names(&backups),
            [
                "clax-9-20261001T000000000Z.db",
                "clax-10-20261004T000000000Z.db",
                "clax-10-20261005T000000000Z.db",
                "clax-10-20261006T000000000Z.db",
            ]
        );
        // Once a later upgrade starts from a newer version, the pin moves on.
        conn.pragma_update(None, "user_version", 11).unwrap();
        let now = "2026-10-07T00:00:00Z".parse().unwrap();
        before_migrate(&conn, &home, 12, false, now).unwrap();
        assert_eq!(
            names(&backups),
            [
                "clax-10-20261005T000000000Z.db",
                "clax-10-20261006T000000000Z.db",
                "clax-11-20261007T000000000Z.db",
            ]
        );
    }
}

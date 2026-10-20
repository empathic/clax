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
/// `user_version` is above 0 and below `known`), copies it whole, with
/// what its write-ahead log holds, to
/// `<home>/backups/clax-<version>-<UTC time>.db`, then prunes the older
/// copies (see `prune`). The copy is written under a temporary name
/// (`VACUUM INTO`), synced to stable storage, renamed, and the directory
/// synced, as is the home that holds the directory, so a copy that
/// exists under its name is complete, and it survives a power loss after
/// the migrations that follow commit. Where the file system cannot sync
/// through the drive's cache it is synced with `fsync`, and where it
/// cannot sync a directory at all the directory is left to it. A sync a
/// signal interrupts is made again.
/// Returns the copy's path, or `None` when nothing is to be migrated.
///
/// # Errors
/// [`CoreError::MigrationFailed`] when the copy cannot be made or synced
/// (a real I/O error, not a sync the file system does not support): the
/// caller must not migrate then. Removing old copies never fails it.
pub fn before_migrate(
    conn: &Connection,
    home: &Home,
    known: u32,
    now: DateTime<Utc>,
) -> Result<Option<PathBuf>> {
    before_migrate_on(&mut RealDisk, conn, home, known, now)
}

/// How far a sync pushes what was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flush {
    /// Through the drive's own cache to its stable storage: `F_FULLFSYNC`
    /// on Apple platforms, `fsync` elsewhere.
    Full,
    /// `fsync`: to the drive, which on Apple platforms may still hold it in
    /// its cache.
    Plain,
}

/// The file-system steps that put a finished copy in place: the real
/// disk's, or a test's.
trait Disk {
    /// Flushes `file`'s content to stable storage, as far as `how` says.
    fn sync_file(&mut self, file: &Path, how: Flush) -> std::io::Result<()>;
    /// Renames `from` to `to`, replacing it.
    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()>;
    /// Flushes `dir`'s entries (a rename into it, a directory made in it)
    /// to stable storage, as far as `how` says.
    fn sync_dir(&mut self, dir: &Path, how: Flush) -> std::io::Result<()>;
}

struct RealDisk;

impl RealDisk {
    /// Syncs the open `file` (a file or a directory) as `how` says.
    fn sync(file: &std::fs::File, how: Flush) -> std::io::Result<()> {
        match how {
            // `sync_all` is `F_FULLFSYNC` on Apple platforms and `fsync`
            // elsewhere.
            Flush::Full => file.sync_all(),
            #[cfg(unix)]
            Flush::Plain => Ok(nix::unistd::fsync(file)?),
            #[cfg(not(unix))]
            Flush::Plain => file.sync_all(),
        }
    }
}

impl Disk for RealDisk {
    fn sync_file(&mut self, file: &Path, how: Flush) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new().write(true).open(file)?;
        Self::sync(&file, how)
    }

    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    #[cfg(unix)]
    fn sync_dir(&mut self, dir: &Path, how: Flush) -> std::io::Result<()> {
        Self::sync(&std::fs::File::open(dir)?, how)
    }

    /// A directory cannot be opened for syncing here; NTFS journals the
    /// rename itself.
    #[cfg(not(unix))]
    fn sync_dir(&mut self, _dir: &Path, _how: Flush) -> std::io::Result<()> {
        Ok(())
    }
}

/// [`before_migrate`] on `disk`.
fn before_migrate_on(
    disk: &mut dyn Disk,
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
    // The directory's entry is in the home: it reaches the disk before a
    // copy in it is counted on. Every copy syncs it, not only the one that
    // makes the directory, which may have failed or stopped before its sync.
    sync_dir(disk, home.root()).map_err(|e| fail(&e))?;
    let name = format!("clax-{version}-{}.db", now.format("%Y%m%dT%H%M%S%3fZ"));
    let path = dir.join(&name);
    let tmp = dir.join(format!(".{name}.{:016x}.tmp", rand::random::<u64>()));
    let _ = std::fs::remove_file(&tmp);
    let tmp_str = tmp.to_str().ok_or_else(|| fail(&"the path is not UTF-8"))?;
    if let Err(e) = conn.execute("VACUUM INTO ?1", [tmp_str]) {
        let _ = std::fs::remove_file(&tmp);
        return Err(fail(&e));
    }
    // VACUUM INTO does not sync what it writes: the copy reaches the disk
    // before its name does, and its name before any migration commits.
    if let Err(e) = sync_file(disk, &tmp).and_then(|()| disk.rename(&tmp, &path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(fail(&e));
    }
    sync_dir(disk, &dir).map_err(|e| fail(&e))?;
    tracing::warn!(
        from = version,
        to = known,
        copy = %path.display(),
        "upgrading the database; copied it first"
    );
    prune(&dir);
    Ok(Some(path))
}

/// Syncs `file` as SQLite's `fullfsync` does: through the drive's cache,
/// or with `fsync` where the file system does not support that (SMB, AFP,
/// exFAT and FUSE volumes answer `F_FULLFSYNC` so). Any other error, or
/// `fsync` failing too, is returned: the database beside the copy could
/// not commit there either.
fn sync_file(disk: &mut dyn Disk, file: &Path) -> std::io::Result<()> {
    match uninterrupted(|| disk.sync_file(file, Flush::Full)) {
        Err(e) if unsupported(&e) => uninterrupted(|| disk.sync_file(file, Flush::Plain)),
        done => done,
    }
}

/// Syncs `dir`'s entries as [`sync_file`] syncs a file, and leaves them to
/// the file system where it supports neither sync of a directory, as
/// SQLite does (some FUSE and network file systems refuse it). A real I/O
/// error is returned.
fn sync_dir(disk: &mut dyn Disk, dir: &Path) -> std::io::Result<()> {
    let synced = match uninterrupted(|| disk.sync_dir(dir, Flush::Full)) {
        Err(e) if unsupported(&e) => uninterrupted(|| disk.sync_dir(dir, Flush::Plain)),
        done => done,
    };
    match synced {
        Err(e) if unsupported(&e) => {
            tracing::debug!(dir = %dir.display(), error = %e, "the file system does not sync directories");
            Ok(())
        }
        done => done,
    }
}

/// Makes `sync` again for as long as a signal interrupts it (`EINTR`), as
/// `File::sync_all` does: `fsync` is not restarted after a signal, and an
/// interrupted sync is no I/O error.
fn uninterrupted(mut sync: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    loop {
        match sync() {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            done => return done,
        }
    }
}

/// Whether `e` says the file system does not support a sync, rather than
/// that the sync failed: `ENOTSUP`, `EOPNOTSUPP`, `EINVAL` (a descriptor
/// that does not support synchronisation) or `ENOSYS`.
fn unsupported(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        e.raw_os_error().is_some_and(|code| {
            [
                Errno::ENOTSUP,
                Errno::EOPNOTSUPP,
                Errno::EINVAL,
                Errno::ENOSYS,
            ]
            .contains(&Errno::from_raw(code))
        })
    }
    #[cfg(not(unix))]
    {
        e.kind() == std::io::ErrorKind::InvalidInput
    }
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

    /// A step (as [`Recording`] names it) that fails, and its error.
    type Failure = (&'static str, fn() -> std::io::Error);

    /// A [`Disk`] that records each step (`"<step> <how> <name>"`, the
    /// temporary copy's name as `tmp`), does it on the real disk, and fails
    /// a step named in `failing` with its error, once for each time it is
    /// named there.
    #[derive(Default)]
    struct Recording {
        steps: Vec<String>,
        failing: Vec<Failure>,
    }

    impl Recording {
        fn failing(failing: &[Failure]) -> Self {
            Recording {
                failing: failing.to_vec(),
                ..Recording::default()
            }
        }

        fn step(&mut self, step: &str, how: Option<Flush>, path: &Path) -> std::io::Result<()> {
            let name = path.file_name().unwrap().to_str().unwrap();
            let kind = if name.ends_with(".tmp") { "tmp" } else { name };
            let how = match how {
                Some(Flush::Full) => " full",
                Some(Flush::Plain) => " plain",
                None => "",
            };
            let step = format!("{step}{how} {kind}");
            let failure = self
                .failing
                .iter()
                .position(|(s, _)| *s == step)
                .map(|i| self.failing.remove(i).1);
            self.steps.push(step);
            failure.map_or(Ok(()), |e| Err(e()))
        }
    }

    impl Disk for Recording {
        fn sync_file(&mut self, file: &Path, how: Flush) -> std::io::Result<()> {
            self.step("sync_file", Some(how), file)?;
            RealDisk.sync_file(file, how)
        }

        fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
            self.step("rename", None, from)?;
            RealDisk.rename(from, to)
        }

        fn sync_dir(&mut self, dir: &Path, how: Flush) -> std::io::Result<()> {
            self.step("sync_dir", Some(how), dir)?;
            RealDisk.sync_dir(dir, how)
        }
    }

    fn gone() -> std::io::Error {
        std::io::Error::other("the disk is gone")
    }

    #[cfg(unix)]
    fn eio() -> std::io::Error {
        nix::errno::Errno::EIO.into()
    }

    #[cfg(unix)]
    fn eintr() -> std::io::Error {
        nix::errno::Errno::EINTR.into()
    }

    /// The errors a file system gives for a sync it does not support.
    #[cfg(unix)]
    const UNSUPPORTED: [fn() -> std::io::Error; 5] = [
        || nix::errno::Errno::ENOTSUP.into(),
        || nix::errno::Errno::EOPNOTSUPP.into(),
        || nix::errno::Errno::EINVAL.into(),
        || nix::errno::Errno::ENOSYS.into(),
        || std::io::ErrorKind::Unsupported.into(),
    ];

    fn at_version(v: u32) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);")
            .unwrap();
        conn.pragma_update(None, "user_version", v).unwrap();
        conn
    }

    /// Copies a database at version 9 into `home` on `disk`.
    fn copy_on(disk: &mut Recording, home: &Home) -> Result<Option<PathBuf>> {
        let now = "2026-10-02T00:00:00Z".parse().unwrap();
        before_migrate_on(disk, &at_version(9), home, 12, now)
    }

    /// The names of the files in `home`'s backups directory.
    fn left_in(home: &Home) -> Vec<String> {
        std::fs::read_dir(dir(home))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect()
    }

    #[test]
    fn the_copy_is_synced_then_renamed_then_its_directory_synced() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(tmp.path().join("ax"));
        let mut disk = Recording::default();
        let copy = copy_on(&mut disk, &home).unwrap().unwrap();
        assert_eq!(
            disk.steps,
            [
                "sync_dir full ax",
                "sync_file full tmp",
                "rename tmp",
                "sync_dir full backups"
            ],
            "the home is synced once it holds the backups directory"
        );
        let copied = Connection::open(&copy).unwrap();
        let v: u32 = copied
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 9);

        // The directory is there now, and the home is synced again: an
        // earlier open may have made it and failed or stopped before its
        // sync.
        let mut disk = Recording::default();
        let now = "2026-10-03T00:00:00Z".parse().unwrap();
        before_migrate_on(&mut disk, &at_version(9), &home, 12, now)
            .unwrap()
            .unwrap();
        assert_eq!(
            disk.steps,
            [
                "sync_dir full ax",
                "sync_file full tmp",
                "rename tmp",
                "sync_dir full backups"
            ]
        );
    }

    /// An open that made the backups directory and could not sync the home
    /// leaves the next open to sync it before the copy it makes there.
    #[cfg(unix)]
    #[test]
    fn a_backups_directory_whose_home_sync_failed_is_synced_by_the_next_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(tmp.path().join("ax"));
        let mut disk = Recording::failing(&[("sync_dir full ax", eio)]);
        copy_on(&mut disk, &home).unwrap_err();
        assert!(dir(&home).is_dir());
        let mut disk = Recording::failing(&[("sync_dir full ax", eio)]);
        copy_on(&mut disk, &home).unwrap_err();
        assert_eq!(
            disk.steps,
            ["sync_dir full ax"],
            "no copy before the home is synced"
        );
        assert!(left_in(&home).is_empty());
    }

    #[test]
    fn a_copy_that_cannot_be_synced_is_not_put_in_place() {
        for failing in [
            "sync_dir full ax",
            "sync_file full tmp",
            "rename tmp",
            "sync_dir full backups",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let home = Home::at(tmp.path().join("ax"));
            let mut disk = Recording::failing(&[(failing, gone)]);
            let err = copy_on(&mut disk, &home).unwrap_err();
            assert!(
                matches!(&err, CoreError::MigrationFailed { at: 9, detail }
                    if detail.contains("the disk is gone")),
                "{failing}: {err:?}"
            );
            let left = left_in(&home);
            assert!(
                left.iter().all(|n| !n.ends_with(".tmp")),
                "{failing}: no temporary file is left: {left:?}"
            );
        }
    }

    /// Where `F_FULLFSYNC` is not supported (SMB, AFP, exFAT, FUSE), the
    /// copy and its directories are synced with `fsync`, as SQLite's
    /// `fullfsync` does.
    #[cfg(unix)]
    #[test]
    fn without_a_full_sync_the_copy_is_synced_with_fsync() {
        for unsupported in UNSUPPORTED {
            let tmp = tempfile::tempdir().unwrap();
            let home = Home::at(tmp.path().join("ax"));
            let mut disk = Recording::failing(&[
                ("sync_dir full ax", unsupported),
                ("sync_file full tmp", unsupported),
                ("sync_dir full backups", unsupported),
            ]);
            let copy = copy_on(&mut disk, &home).unwrap().unwrap();
            assert!(copy.is_file(), "{:?}", unsupported());
            assert_eq!(
                disk.steps,
                [
                    "sync_dir full ax",
                    "sync_dir plain ax",
                    "sync_file full tmp",
                    "sync_file plain tmp",
                    "rename tmp",
                    "sync_dir full backups",
                    "sync_dir plain backups",
                ],
                "{:?}",
                unsupported()
            );
        }
    }

    /// A file system that cannot sync a directory at all (some FUSE and
    /// network ones) still gets its copy: the directory is left to it.
    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_synced_is_left_to_its_file_system() {
        for unsupported in UNSUPPORTED {
            let tmp = tempfile::tempdir().unwrap();
            let home = Home::at(tmp.path().join("ax"));
            let mut disk = Recording::failing(&[
                ("sync_dir full ax", unsupported),
                ("sync_dir plain ax", unsupported),
                ("sync_dir full backups", unsupported),
                ("sync_dir plain backups", unsupported),
            ]);
            let copy = copy_on(&mut disk, &home).unwrap().unwrap();
            assert!(copy.is_file(), "{:?}", unsupported());
            assert_eq!(left_in(&home).len(), 1, "{:?}", unsupported());
        }
    }

    /// A sync a signal interrupts (`EINTR`) is made again, as `sync_all`
    /// makes its own, the `fsync` fallback's included: an interrupted sync
    /// is no I/O error and never refuses the open.
    #[cfg(unix)]
    #[test]
    fn a_sync_a_signal_interrupts_is_made_again() {
        let unsupported = UNSUPPORTED[0];
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(tmp.path().join("ax"));
        let mut disk = Recording::failing(&[
            ("sync_dir full ax", eintr),
            ("sync_file full tmp", unsupported),
            ("sync_file plain tmp", eintr),
            ("sync_file plain tmp", eintr),
            ("sync_dir full backups", unsupported),
            ("sync_dir plain backups", eintr),
        ]);
        let copy = copy_on(&mut disk, &home).unwrap().unwrap();
        assert!(copy.is_file());
        assert_eq!(
            disk.steps,
            [
                "sync_dir full ax",
                "sync_dir full ax",
                "sync_file full tmp",
                "sync_file plain tmp",
                "sync_file plain tmp",
                "sync_file plain tmp",
                "rename tmp",
                "sync_dir full backups",
                "sync_dir plain backups",
                "sync_dir plain backups",
            ]
        );
    }

    /// A real I/O error refuses the open wherever it comes, as does a file
    /// that cannot be synced even with `fsync`: the database beside it
    /// could not commit either.
    #[cfg(unix)]
    #[test]
    fn a_real_io_error_refuses_the_open() {
        let unsupported = UNSUPPORTED[0];
        let cases: [&[Failure]; 7] = [
            &[("sync_dir full ax", eio)],
            &[
                ("sync_dir full ax", unsupported),
                ("sync_dir plain ax", eio),
            ],
            &[("sync_file full tmp", eio)],
            &[
                ("sync_file full tmp", unsupported),
                ("sync_file plain tmp", eio),
            ],
            &[
                ("sync_file full tmp", unsupported),
                ("sync_file plain tmp", unsupported),
            ],
            &[("sync_dir full backups", eio)],
            &[
                ("sync_dir full backups", unsupported),
                ("sync_dir plain backups", eio),
            ],
        ];
        for failing in cases {
            let tmp = tempfile::tempdir().unwrap();
            let home = Home::at(tmp.path().join("ax"));
            let mut disk = Recording::failing(failing);
            let err = copy_on(&mut disk, &home).unwrap_err();
            let names: Vec<_> = failing.iter().map(|f| f.0).collect();
            assert!(
                matches!(&err, CoreError::MigrationFailed { at: 9, .. }),
                "{names:?}: {err:?}"
            );
            let left = left_in(&home);
            assert!(
                left.iter().all(|n| !n.ends_with(".tmp")),
                "{names:?}: no temporary file is left: {left:?}"
            );
            if names.contains(&"sync_file plain tmp") || names.contains(&"sync_dir full ax") {
                assert!(
                    left.is_empty(),
                    "{names:?}: no copy is put in place: {left:?}"
                );
            }
        }
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
            assert!(before_migrate(&conn, &home, 12, now).unwrap().is_some());
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
        before_migrate(&conn, &home, 12, now).unwrap();
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

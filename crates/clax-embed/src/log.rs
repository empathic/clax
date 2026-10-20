//! The core's log: a file in the app's caches, cut at [`CUT_AT`] to one
//! older file beside it.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// The size past which the log is cut: the file becomes `<name>.1`
/// (replacing the one before) and a new file starts.
pub const CUT_AT: u64 = 1024 * 1024;

/// Installs the process's tracing subscriber, writing to `path` (a
/// `RUST_LOG` filter, else `warn,clax=info`, without colours). Only the
/// first call in a process installs one; later calls change nothing.
pub fn init(path: PathBuf) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,clax=info"));
    let log: &'static RollingLog = Box::leak(Box::new(RollingLog::new(path, CUT_AT)));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || log)
        .try_init();
}

/// A log file that is cut at a size: before a write would take it past
/// `cut_at`, the file is renamed to `<name>.1` and a new one started, so a
/// line is never split across the two.
pub struct RollingLog {
    path: PathBuf,
    cut_at: u64,
    open: Mutex<Option<(File, u64)>>,
}

impl RollingLog {
    pub fn new(path: PathBuf, cut_at: u64) -> RollingLog {
        RollingLog {
            path,
            cut_at,
            open: Mutex::new(None),
        }
    }
}

impl RollingLog {
    /// The file appended to, at the end of what it holds.
    fn open(&self) -> io::Result<(File, u64)> {
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let len = f.metadata()?.len();
        Ok((f, len))
    }

    /// Renames the file to `<name>.1`, replacing the one before.
    fn cut(&self) -> io::Result<()> {
        let mut older = self.path.clone().into_os_string();
        older.push(".1");
        std::fs::rename(&self.path, older)
    }
}

impl Write for &RollingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if open.is_none() {
            *open = Some(self.open()?);
        }
        let len = open.as_ref().map_or(0, |(_, len)| *len);
        if len > 0 && len + buf.len() as u64 > self.cut_at {
            *open = None;
            self.cut()?;
            *open = Some(self.open()?);
        }
        let (f, len) = open.as_mut().expect("opened above");
        let n = f.write(buf)?;
        *len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_past_the_cut_starts_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        let log = RollingLog::new(path.clone(), 64);
        let first = format!("{}\n", "a".repeat(49));
        let second = format!("{}\n", "b".repeat(49));
        (&log).write_all(first.as_bytes()).unwrap();
        (&log).write_all(second.as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("daemon.log.1")).unwrap(),
            first
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), second);
    }

    #[test]
    fn the_cut_replaces_the_older_file_and_appends_across_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        std::fs::write(dir.path().join("daemon.log.1"), "old\n").unwrap();
        std::fs::write(&path, "kept\n").unwrap();
        let log = RollingLog::new(path.clone(), 64);
        (&log).write_all(b"one\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept\none\n");
        (&log).write_all("c".repeat(60).as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("daemon.log.1")).unwrap(),
            "kept\none\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "c".repeat(60));
    }
}

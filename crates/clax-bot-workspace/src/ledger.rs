//! What the host remembers of each run across a restart, a file per run: the
//! container, what the run pushed, and the proposals it has answered. A
//! container's own word is worth nothing, and its labels cannot change after
//! creation, so all of this is kept here.

use anyhow::{Context, Result};
use clax_bot::workspace::Offer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How long a push counts against [`Ledger::take_push`]'s limit, in seconds.
pub const DAY: u64 = 24 * 60 * 60;

/// A push under way: its proposal's key and its commit, written before the
/// push so that a crash during it can be told from someone else's commit. A
/// commit a forge made also has its tree and parent, so that the same change
/// on the same parent, under any key, takes that commit rather than making
/// another.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Pending {
    pub key: String,
    pub commit: String,
    #[serde(default)]
    pub tree: String,
    #[serde(default)]
    pub parent: String,
}

/// A preview being waited for, which a restart takes up again.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Awaiting {
    pub pr: u64,
    pub commit: String,
    /// When to give up, in seconds since the epoch.
    pub until: u64,
}

/// Where a run's change goes. A target's name can come to mean another
/// repository while a run holds this one, so it is the addresses that are kept.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Repository {
    /// [`crate::forge::Forge::repository`].
    pub forge: String,
    pub repo_url: String,
    pub container_repo_url: String,
}

/// One run.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Its container's ID.
    #[serde(default)]
    pub container: Option<String>,
    /// That container's labels.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// The commits it pushed.
    #[serde(default)]
    pub pushed: Vec<String>,
    #[serde(default)]
    pub pending: Option<Pending>,
    /// What each proposal answered, by its key.
    #[serde(default)]
    pub proposals: BTreeMap<String, Value>,
    /// The latest proposal's preview.
    #[serde(default)]
    pub awaiting: Option<Awaiting>,
    /// What the run was offered when it began: it keeps that until it ends,
    /// whatever becomes of its target.
    #[serde(default)]
    pub offer: Option<Offer>,
    /// The branch its work stands on, when it has a repository.
    #[serde(default)]
    pub branch: Option<String>,
    /// The repository its checkout came from. Only that one takes its change.
    #[serde(default)]
    pub repository: Option<Repository>,
    /// Its container's checkout finished. Until then a container with no
    /// checkout holds nothing of the run's.
    #[serde(default)]
    pub checked_out: bool,
    /// The run is over: nothing is made for it again.
    #[serde(default)]
    pub finished: bool,
}

/// The files, in a directory that must outlive the daemon for any of this to
/// hold, or in a temporary one that goes with this value.
#[derive(Debug)]
pub struct Ledger {
    dir: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

impl Ledger {
    pub fn at(dir: PathBuf) -> Ledger {
        Ledger {
            dir,
            _temporary: None,
        }
    }

    /// One that lasts as long as this process's workspace.
    pub fn temporary() -> Result<Ledger> {
        let dir = tempfile::tempdir().context("making a temporary run ledger")?;
        Ok(Ledger {
            dir: dir.path().to_path_buf(),
            _temporary: Some(dir),
        })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Run `id`'s entry; the default for a run with none.
    pub fn read(&self, id: &str) -> Result<Entry> {
        match std::fs::read(self.path(id)) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading the run ledger"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Entry::default()),
            Err(e) => Err(e).context("reading the run ledger"),
        }
    }

    /// Replaces run `id`'s entry whole: a crash leaves the old one or the new.
    pub fn write(&self, id: &str, entry: &Entry) -> Result<()> {
        std::fs::create_dir_all(&self.dir).context("making the run ledger")?;
        let path = self.path(id);
        let next = path.with_extension("next");
        std::fs::write(&next, serde_json::to_vec(entry)?).context("writing the run ledger")?;
        std::fs::rename(&next, &path).context("writing the run ledger")
    }

    pub fn remove(&self, id: &str) {
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(error = %e, run = id, "clearing the run ledger failed"),
        }
    }

    /// Counts a push of `target` at `now`, in seconds since the epoch, unless
    /// `limit` were pushed in the 24 hours before it; answers whether it was
    /// counted. A push that then fails has spent its place.
    pub fn take_push(&self, target: &str, limit: usize, now: u64) -> Result<bool> {
        let dir = self.dir.join("pushes");
        std::fs::create_dir_all(&dir).context("making the push count")?;
        // Held until it is dropped: two runs, or two processes, count in turn.
        let held = std::fs::File::create(dir.join("lock")).context("locking the push count")?;
        held.lock().context("locking the push count")?;
        let name: String = target.bytes().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("{name}.json"));
        let mut times: Vec<u64> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading the push count")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).context("reading the push count"),
        };
        times.retain(|t| t.saturating_add(DAY) > now);
        if times.len() >= limit {
            return Ok(false);
        }
        times.push(now);
        let next = path.with_extension("next");
        std::fs::write(&next, serde_json::to_vec(&times)?).context("writing the push count")?;
        std::fs::rename(&next, &path).context("writing the push count")?;
        Ok(true)
    }

    /// Every run with an entry.
    pub fn runs(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let path: &Path = &e.path();
                let is_entry = path.extension().is_some_and(|x| x == "json");
                let stem = path.file_stem()?.to_str()?.to_string();
                is_entry.then_some(stem)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_takes_its_limit_of_pushes_in_a_day_and_no_more() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(dir.path().into());
        assert!(ledger.take_push("t", 2, 1000).unwrap());
        assert!(ledger.take_push("t", 2, 2000).unwrap());
        assert!(!ledger.take_push("t", 2, 3000).unwrap());
        assert!(ledger.take_push("another", 2, 3000).unwrap());
    }

    #[test]
    fn a_push_a_day_old_counts_no_more() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(dir.path().into());
        assert!(ledger.take_push("t", 1, 1000).unwrap());
        assert!(!ledger.take_push("t", 1, 1000 + DAY - 1).unwrap());
        assert!(ledger.take_push("t", 1, 1000 + DAY).unwrap());
    }

    #[test]
    fn pushes_counted_survive_a_restart_and_are_no_run() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Ledger::at(dir.path().into())
                .take_push("a/b", 1, 5)
                .unwrap()
        );
        let again = Ledger::at(dir.path().into());
        assert!(!again.take_push("a/b", 1, 6).unwrap());
        assert!(again.runs().is_empty());
    }

    #[test]
    fn an_entry_survives_the_ledger_being_opened_again() {
        let dir = tempfile::tempdir().unwrap();
        let entry = Entry {
            container: Some("c1".into()),
            pushed: vec!["abc".into()],
            ..Entry::default()
        };
        Ledger::at(dir.path().into()).write("r1", &entry).unwrap();
        let again = Ledger::at(dir.path().into());
        assert_eq!(again.read("r1").unwrap(), entry);
        assert_eq!(again.runs(), vec!["r1".to_string()]);
        again.remove("r1");
        again.remove("r1");
        assert_eq!(again.read("r1").unwrap(), Entry::default());
    }
}

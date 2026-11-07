//! What became of fixes' previews, kept until the host says it told them. A
//! preview's word can come after its run has ended and while nobody is
//! polling, so it goes to disk before anything is answered: a restart loses
//! none, and the host hears each until it acknowledges. A disk that will not
//! take it does not lose it: it stays in memory, the host hears it all the
//! same, and each poll saves again.

use anyhow::{Context, Result};
use async_trait::async_trait;
use clax_bot::workspace::PreviewSink;
use clax_bot_remote::wire::{PreviewEvent, PreviewTold};
use clax_bot_sandbox::HostError;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const FILE: &str = "workspace-previews.json";

#[derive(Default, Deserialize, Serialize)]
struct Kept {
    /// The number of the next event, which outlives every event so that an
    /// ID is never used twice.
    next: u64,
    events: Vec<PreviewEvent>,
    /// What is here is not all on disk.
    #[serde(skip)]
    unsaved: bool,
}

/// The file of events not yet acknowledged.
pub struct PreviewLog {
    path: PathBuf,
    kept: Mutex<Kept>,
}

/// Replaces `path` whole and durably: a crash leaves the old file or the new.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let next = path.with_extension("next");
    let writing = || format!("writing {}", path.display());
    let mut file = File::create(&next).with_context(writing)?;
    file.write_all(bytes).with_context(writing)?;
    file.sync_all().with_context(writing)?;
    std::fs::rename(&next, path).with_context(writing)?;
    let dir = path.parent().with_context(writing)?;
    File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(writing)
}

impl PreviewLog {
    /// The log kept in `dir`. A file that cannot be read is an `Err`: starting
    /// without it would drop what it holds.
    ///
    /// # Errors
    /// When `dir` cannot be made or its file read.
    pub fn open(dir: &Path) -> Result<PreviewLog> {
        std::fs::create_dir_all(dir).context("making the told file's directory")?;
        let path = dir.join(FILE);
        let kept = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("reading {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Kept::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(PreviewLog {
            path,
            kept: Mutex::new(kept),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, kept: &Kept) -> Result<()> {
        replace(&self.path, &serde_json::to_vec(kept)?)
    }

    /// Saves what the disk would not take before, if it takes it now.
    fn save_late(&self, kept: &mut Kept) {
        if kept.unsaved && self.save(kept).is_ok() {
            kept.unsaved = false;
        }
    }

    /// Keeps the word that `told` of `run`. The same word not yet acknowledged
    /// adds nothing: a restart takes up a run's wait and may say it again.
    fn add(&self, run: &str, told: PreviewTold) {
        let mut kept = self.lock();
        if kept.events.iter().any(|e| e.run == run && e.told == told) {
            return;
        }
        let id = format!("p{}", kept.next);
        kept.events.push(PreviewEvent {
            id,
            run: run.to_string(),
            told,
        });
        kept.next += 1;
        if let Err(e) = self.save(&kept) {
            tracing::warn!(error = %e, run, "a preview's word is in memory alone");
            kept.unsaved = true;
        }
    }

    /// Every event not yet acknowledged, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<PreviewEvent> {
        let mut kept = self.lock();
        self.save_late(&mut kept);
        kept.events.clone()
    }

    /// Whether every event has been acknowledged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().events.is_empty()
    }

    /// Forgets `ids`. An ID it does not hold is one acknowledged before.
    ///
    /// # Errors
    /// When the file cannot be written: every event stays where it was, to be
    /// told again.
    pub fn acknowledge(&self, ids: &[String]) -> Result<()> {
        let mut kept = self.lock();
        let known = |e: &&PreviewEvent| !ids.contains(&e.id);
        let left: Vec<_> = kept.events.iter().filter(known).cloned().collect();
        if left.len() == kept.events.len() {
            return Ok(());
        }
        let next = Kept {
            next: kept.next,
            events: left,
            unsaved: false,
        };
        self.save(&next)?;
        *kept = next;
        Ok(())
    }
}

#[async_trait]
impl PreviewSink for PreviewLog {
    async fn preview_ready(
        &self,
        run: &str,
        verify_url: &str,
        reviewed: bool,
    ) -> Result<(), HostError> {
        let verify_url = verify_url.to_string();
        let told = PreviewTold::Ready {
            verify_url,
            reviewed,
        };
        self.add(run, told);
        Ok(())
    }

    async fn preview_missed(&self, run: &str) -> Result<(), HostError> {
        self.add(run, PreviewTold::Missed);
        Ok(())
    }
}

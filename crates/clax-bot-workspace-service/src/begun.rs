//! The commands begun, on disk before they start. The job table is memory, so
//! a restart forgets a command that ran; a send of its POST arriving after
//! that must find the key here and not run it again. A proposal needs none of
//! this: its key is the ledger's, and it pushes nothing twice.
//!
//! An append, synced, per command, each line after a newline of its own. A
//! line a crash tore off is skipped and loses only itself: its command had not
//! been started, as a key is on disk first.

use crate::previews::replace;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE: &str = "workspace-jobs.log";

#[derive(Deserialize, Serialize)]
struct Line {
    key: String,
    run: String,
}

/// The file of keys begun, and what it holds.
pub struct Begun {
    path: PathBuf,
    file: File,
    /// The run of each key.
    keys: HashMap<String, String>,
}

fn append(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

impl Begun {
    /// The keys kept in `dir`.
    ///
    /// # Errors
    /// When the file cannot be read or opened.
    pub fn open(dir: &Path) -> Result<Begun> {
        std::fs::create_dir_all(dir).context("making the begun keys' directory")?;
        let path = dir.join(FILE);
        let text = match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let lines = text.lines().filter_map(|l| serde_json::from_str(l).ok());
        let keys = lines.map(|l: Line| (l.key, l.run)).collect();
        let file = append(&path)?;
        Ok(Begun { path, file, keys })
    }

    /// The run that began `key`.
    #[must_use]
    pub fn run_of(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(String::as_str)
    }

    /// Every run with a key here.
    pub fn runs(&self) -> impl Iterator<Item = &String> {
        self.keys.values()
    }

    /// Keeps `key`, durably, before its command starts.
    ///
    /// # Errors
    /// When it cannot be kept: the command must then not start.
    pub fn note(&mut self, key: &str, run: &str) -> Result<()> {
        let line = Line {
            key: key.to_string(),
            run: run.to_string(),
        };
        let mut text = vec![b'\n'];
        serde_json::to_writer(&mut text, &line)?;
        text.push(b'\n');
        self.file.write_all(&text).context("keeping a job's key")?;
        self.file.sync_data().context("keeping a job's key")?;
        self.keys.insert(line.key, line.run);
        Ok(())
    }

    /// Forgets run `id`'s keys, as its run has ended. What cannot be written
    /// stays on disk until the next time, and refuses only keys of a run over.
    pub fn forget(&mut self, id: &str) {
        let before = self.keys.len();
        self.keys.retain(|_, run| run != id);
        if self.keys.len() == before {
            return;
        }
        let rewritten = self.rewrite();
        if let Err(e) = rewritten {
            tracing::warn!(error = %e, run = id, "forgetting a run's keys failed");
        }
    }

    fn rewrite(&mut self) -> Result<()> {
        let mut text = Vec::new();
        for (key, run) in &self.keys {
            let (key, run) = (key.clone(), run.clone());
            serde_json::to_writer(&mut text, &Line { key, run })?;
            text.push(b'\n');
        }
        replace(&self.path, &text)?;
        self.file = append(&self.path)?;
        Ok(())
    }
}

//! A run's draft (spec §6.3): the files of the version it is based on,
//! with the paths the run wrote or deleted laid over them. A script's
//! `files.*` calls change a [`Draft`] in memory; what they changed is
//! committed with the script's step ([`Draft::changes`]), so a script
//! that does not commit (a crash, a cancel) leaves the stored draft as it
//! was.

use crate::host::PublishFile;
use clax_bot_sandbox::HostError;
use clax_core::{ArtifactId, CoreError};
use clax_store::Store;
use clax_store::bot_runs::{DraftChange, Run};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The most bytes one draft file may hold.
pub const MAX_FILE_BYTES: u64 = clax_core::publish::MAX_FILE_BYTES;
/// The most bytes a whole draft may hold.
pub const MAX_DRAFT_BYTES: u64 = clax_core::publish::MAX_BODY_BYTES;

/// One file of a draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftEntry {
    pub content: Vec<u8>,
    pub content_type: String,
}

/// A run's draft as a script sees it.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    /// The version publishing it replaces (`if_version`).
    pub base_version: u32,
    files: BTreeMap<String, DraftEntry>,
    /// The files of the base version.
    base: BTreeMap<String, DraftEntry>,
    /// Every path the run's overlay names (written or deleted), this step's
    /// changes included.
    overlay: std::collections::BTreeSet<String>,
    /// What changed since it was loaded: `None` for a deleted path.
    changed: BTreeMap<String, Option<DraftEntry>>,
}

/// What [`Draft::rebase`] did.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Rebased {
    /// The version the draft is now based on.
    pub version: u32,
    /// The paths the draft changes over it.
    pub changed: Vec<String>,
    /// Paths both the draft and the new version changed: the draft keeps
    /// its own ([`Draft::theirs`] reads the other).
    pub conflicts: Vec<String>,
}

/// The files of version `n` of `run`'s artifact.
async fn version_files(
    store: &Store,
    run: &Run,
    n: u32,
) -> Result<BTreeMap<String, DraftEntry>, CoreError> {
    let aid = ArtifactId::parse(&run.artifact_id)?;
    let version = store.get_version(&aid, n)?.ok_or(CoreError::NotFound)?;
    let mut files = BTreeMap::new();
    for (path, meta) in &version.files {
        if let Some((bytes, _)) = store.read_file_async(&aid, n, path).await? {
            files.insert(
                path.clone(),
                DraftEntry {
                    content: bytes,
                    content_type: meta.content_type.clone(),
                },
            );
        }
    }
    Ok(files)
}

fn host_err(code: &str, message: impl Into<String>) -> HostError {
    HostError::new(code, message)
}

impl Draft {
    /// A draft of exactly `files`, based on `base_version`.
    #[must_use]
    pub fn of(base_version: u32, files: BTreeMap<String, DraftEntry>) -> Draft {
        Draft {
            base_version,
            base: files.clone(),
            files,
            overlay: Default::default(),
            changed: BTreeMap::new(),
        }
    }

    /// Run `run`'s draft: the files of version `base_version` of its
    /// artifact with the run's stored overlay laid over them.
    ///
    /// # Errors
    /// `NotFound` when the version is gone; the store's.
    pub async fn load(store: &Store, run: &Run, base_version: u32) -> Result<Draft, CoreError> {
        let base = version_files(store, run, base_version).await?;
        let mut d = Draft::of(base_version, base);
        for f in store.draft_files(&run.id)? {
            d.overlay.insert(f.path.clone());
            match f.content {
                Some(content) => {
                    let content_type = f
                        .content_type
                        .unwrap_or_else(|| clax_core::publish::content_type_for(&f.path));
                    d.files.insert(
                        f.path,
                        DraftEntry {
                            content,
                            content_type,
                        },
                    );
                }
                None => {
                    d.files.remove(&f.path);
                }
            }
        }
        Ok(d)
    }

    /// `draft.rebase()`: bases the draft on version `version` of its
    /// artifact, the overlay's paths laid over that version's files.
    ///
    /// # Errors
    /// `NotFound` when the version is gone; the store's.
    pub async fn rebase(
        &mut self,
        store: &Store,
        run: &Run,
        version: u32,
    ) -> Result<Rebased, CoreError> {
        let theirs = version_files(store, run, version).await?;
        let mut files = theirs.clone();
        let mut conflicts = Vec::new();
        for path in &self.overlay {
            if theirs.get(path) != self.base.get(path) {
                conflicts.push(path.clone());
            }
            match self.files.get(path) {
                Some(e) => {
                    files.insert(path.clone(), e.clone());
                }
                None => {
                    files.remove(path);
                }
            }
        }
        self.files = files;
        self.base = theirs;
        self.base_version = version;
        Ok(Rebased {
            version,
            changed: self.overlay.iter().cloned().collect(),
            conflicts,
        })
    }

    /// `draft.theirs(path)`: `path`'s text in the base version, `None` when
    /// it has no such file.
    ///
    /// # Errors
    /// `invalid_path`, `not_text`.
    pub fn theirs(&self, path: &str) -> Result<Option<String>, HostError> {
        Draft::checked(path)?;
        self.base
            .get(path)
            .map(|e| {
                String::from_utf8(e.content.clone())
                    .map_err(|_| host_err("not_text", format!("{path} is binary")))
            })
            .transpose()
    }

    /// Every file, by path.
    #[must_use]
    pub fn files(&self) -> &BTreeMap<String, DraftEntry> {
        &self.files
    }

    /// The paths changed since the draft was loaded.
    #[must_use]
    pub fn changed_paths(&self) -> Vec<String> {
        self.changed.keys().cloned().collect()
    }

    /// What changed since the draft was loaded, for the step's commit.
    #[must_use]
    pub fn changes(&self) -> Vec<DraftChange> {
        self.changed
            .iter()
            .map(|(path, e)| match e {
                Some(e) => DraftChange::Write {
                    path: path.clone(),
                    content: e.content.clone(),
                    content_type: Some(e.content_type.clone()),
                },
                None => DraftChange::Delete { path: path.clone() },
            })
            .collect()
    }

    /// The whole draft, to publish.
    #[must_use]
    pub fn publish_files(&self) -> Vec<PublishFile> {
        self.files
            .iter()
            .map(|(path, e)| PublishFile {
                path: path.clone(),
                content: e.content.clone(),
                content_type: e.content_type.clone(),
            })
            .collect()
    }

    fn info(path: &str, e: &DraftEntry) -> Value {
        json!({"path": path, "bytes": e.content.len(), "content_type": e.content_type})
    }

    /// `files.list()`.
    #[must_use]
    pub fn list(&self) -> Value {
        Value::Array(self.files.iter().map(|(p, e)| Draft::info(p, e)).collect())
    }

    fn checked(path: &str) -> Result<(), HostError> {
        clax_core::publish::check_path(path).map_err(|e| host_err("invalid_path", e.to_string()))
    }

    fn get(&self, path: &str) -> Result<&DraftEntry, HostError> {
        Draft::checked(path)?;
        self.files
            .get(path)
            .ok_or_else(|| host_err("not_found", format!("the draft has no file {path:?}")))
    }

    /// `files.read(path)`.
    ///
    /// # Errors
    /// `invalid_path`, `not_found`, `not_text` (not UTF-8).
    pub fn read(&self, path: &str) -> Result<String, HostError> {
        let e = self.get(path)?;
        String::from_utf8(e.content.clone()).map_err(|_| {
            host_err(
                "not_text",
                format!(
                    "{path} is binary ({}); it cannot be read as text",
                    e.content_type
                ),
            )
        })
    }

    fn put(&mut self, path: &str, content: Vec<u8>) -> Result<Value, HostError> {
        Draft::checked(path)?;
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(host_err(
                "too_large",
                format!(
                    "{path} would be {} bytes; a file holds at most {MAX_FILE_BYTES}",
                    content.len()
                ),
            ));
        }
        let others: u64 = self
            .files
            .iter()
            .filter(|(p, _)| p.as_str() != path)
            .map(|(_, e)| e.content.len() as u64)
            .sum();
        if others + content.len() as u64 > MAX_DRAFT_BYTES {
            return Err(host_err(
                "too_large",
                format!("the draft would be over {MAX_DRAFT_BYTES} bytes"),
            ));
        }
        let content_type = self
            .files
            .get(path)
            .map(|e| e.content_type.clone())
            .unwrap_or_else(|| clax_core::publish::content_type_for(path));
        let e = DraftEntry {
            content,
            content_type,
        };
        let info = Draft::info(path, &e);
        self.overlay.insert(path.to_string());
        self.changed.insert(path.to_string(), Some(e.clone()));
        self.files.insert(path.to_string(), e);
        Ok(info)
    }

    /// `files.write(path, text)`.
    ///
    /// # Errors
    /// `invalid_path`, `too_large`.
    pub fn write(&mut self, path: &str, text: &str) -> Result<Value, HostError> {
        self.put(path, text.as_bytes().to_vec())
    }

    /// `files.replace(path, old, new)`: `old` must occur exactly once.
    ///
    /// # Errors
    /// `invalid_path`, `not_found`, `not_text`, `no_match` (no occurrence,
    /// or more than one), `too_large`.
    pub fn replace(&mut self, path: &str, old: &str, new: &str) -> Result<Value, HostError> {
        let text = self.read(path)?;
        if old.is_empty() {
            return Err(host_err("no_match", "old is empty"));
        }
        match text.matches(old).count() {
            1 => self.put(path, text.replacen(old, new, 1).into_bytes()),
            0 => Err(host_err(
                "no_match",
                format!("{path} does not contain the text to replace; read it again"),
            )),
            n => Err(host_err(
                "no_match",
                format!(
                    "the text to replace occurs {n} times in {path}; include more of it so it occurs once"
                ),
            )),
        }
    }

    /// `files.delete(path)`.
    ///
    /// # Errors
    /// `invalid_path`, `not_found`.
    pub fn delete(&mut self, path: &str) -> Result<Value, HostError> {
        self.get(path)?;
        self.files.remove(path);
        self.overlay.insert(path.to_string());
        self.changed.insert(path.to_string(), None);
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        let mut files = BTreeMap::new();
        files.insert(
            "index.html".to_string(),
            DraftEntry {
                content: b"<p>hello hello</p>".to_vec(),
                content_type: "text/html".into(),
            },
        );
        files.insert(
            "logo.png".to_string(),
            DraftEntry {
                content: vec![0x89, 0xff, 0xfe],
                content_type: "image/png".into(),
            },
        );
        Draft::of(3, files)
    }

    #[test]
    fn files_change_the_draft_and_are_listed_as_changes() {
        let mut d = draft();
        assert_eq!(d.list().as_array().unwrap().len(), 2);
        assert_eq!(d.read("logo.png").unwrap_err().code, "not_text");
        assert_eq!(d.read("nope.html").unwrap_err().code, "not_found");
        assert_eq!(d.read("../x").unwrap_err().code, "invalid_path");
        assert_eq!(
            d.replace("index.html", "hello", "x").unwrap_err().code,
            "no_match"
        );
        d.replace("index.html", "hello hello", "bye").unwrap();
        assert_eq!(d.read("index.html").unwrap(), "<p>bye</p>");
        let info = d.write("style.css", "body{}").unwrap();
        assert_eq!(info["content_type"], "text/css");
        d.delete("logo.png").unwrap();
        assert_eq!(
            d.changes(),
            vec![
                DraftChange::Write {
                    path: "index.html".into(),
                    content: b"<p>bye</p>".to_vec(),
                    content_type: Some("text/html".into())
                },
                DraftChange::Delete {
                    path: "logo.png".into()
                },
                DraftChange::Write {
                    path: "style.css".into(),
                    content: b"body{}".to_vec(),
                    content_type: Some("text/css".into())
                },
            ]
        );
        assert_eq!(d.publish_files().len(), 2);
    }
}

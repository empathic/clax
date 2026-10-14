//! Artifact versions kept in git: one repository per artifact, named
//! `a-<artifact ID>`; one commit per version on `main`, tagged `v<n>`
//! (lightweight); author and committer both the publishing person
//! (`<name> <<person ID>@users.clax.dev>`) at the publish time in UTC; the
//! message the publish note followed by the trailers `Clax-Version: <n>`,
//! `Clax-Publisher: <person ID>`, `Clax-Agent: <agent name>` when an agent
//! published, and `Clax-Forked-From: <artifact ID>@v<n>` on a fork's first
//! version, whose note is by default `Forked from <artifact ID>@v<n>`.
//!
//! [`GitVersions`] is the git side of Clax's content store over any
//! [`RepoHost`]: read a file at a version, write version n+1 from version n
//! plus changes, fork with history, list versions and files.

use std::collections::BTreeMap;

use crate::client::{is_atomic_collateral, is_conflict_reason};
use crate::error::{GitError, Result};
use crate::id::ObjectId;
use crate::object::{
    Commit, Mode, ObjectKind, Person, Signature, parse_trailers, tag_target, with_trailers,
};
use crate::repo::{MAIN, RefUpdate, Repo, RepoHost};
use crate::tree::{Changes, build_tree, resolve_path, walk_tree};

/// The repository of artifact `artifact_id`: `a-<artifact ID>`.
pub fn repo_name(artifact_id: &str) -> String {
    format!("a-{artifact_id}")
}

/// The email a person's commits carry: `<person ID>@users.clax.dev`.
pub fn person_email(person_id: &str) -> String {
    format!("{person_id}@users.clax.dev")
}

/// The name of version `n`'s tag: `v<n>`.
pub fn version_tag(n: u64) -> String {
    format!("v{n}")
}

/// How a fork names the version it was forked from: `<artifact ID>@v<n>`.
pub fn fork_ref(artifact_id: &str, n: u64) -> String {
    format!("{artifact_id}@v{n}")
}

/// `refs/tags/<tag>`, refusing a name that is not letters, digits, `.`,
/// `_` and `-` (starting with a letter or digit), holds `..` or ends in
/// `.lock`.
pub fn tag_ref(tag: &str) -> Result<String> {
    let b = tag.as_bytes();
    let ok = !b.is_empty()
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(c))
        && !tag.ends_with(".lock")
        && !tag.contains("..");
    if ok {
        Ok(format!("refs/tags/{tag}"))
    } else {
        Err(GitError::invalid(format!("Not a valid tag name: {tag:?}")))
    }
}

/// The version number a ref names when it is a version tag
/// `refs/tags/v<n>` (n ≥ 1, no leading zero).
pub fn version_of_tag_ref(name: &str) -> Option<u64> {
    let n = name.strip_prefix("refs/tags/v")?;
    if n.is_empty() || n.starts_with('0') || !n.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// Who published a version and how, for its commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionMeta {
    /// The commit author and committer.
    pub author: Person,
    /// Unix seconds, written in UTC.
    pub time: i64,
    /// The publish note; may be empty.
    pub note: String,
    /// The publishing person's ID (`Clax-Publisher`).
    pub publisher: String,
    /// The publishing agent's name (`Clax-Agent`), when an agent published.
    pub agent: Option<String>,
}

/// The trailers of version `n`'s commit, `extra` last.
pub fn version_trailers(
    n: u64,
    meta: &VersionMeta,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let mut t = vec![
        ("Clax-Version".to_string(), n.to_string()),
        ("Clax-Publisher".to_string(), meta.publisher.clone()),
    ];
    if let Some(agent) = &meta.agent {
        t.push(("Clax-Agent".to_string(), agent.clone()));
    }
    t.extend(extra.iter().cloned());
    t
}

/// A publish: version `version` on top of `parent` (the commit `main` must
/// be at; `None` for the first) with `changes`.
#[derive(Clone, Debug)]
pub struct Publish {
    pub version: u64,
    pub parent: Option<ObjectId>,
    pub changes: Changes,
    pub meta: VersionMeta,
}

/// What a publish or fork made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    /// The new commit, which `main` now points at.
    pub commit: ObjectId,
    pub tree: ObjectId,
    /// `None`, or why the tag could not be written although `main` moved (a
    /// remote without atomic pushes refused the tag after taking `main`):
    /// the version exists, untagged.
    pub tag_error: Option<String>,
}

/// A version or commit to read at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rev {
    /// The commit the tag `v<n>` names.
    Version(u64),
    Commit(ObjectId),
}

/// One file of a version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub size: u64,
    /// The git blob ID.
    pub id: ObjectId,
    pub mode: Mode,
}

/// A commit as [`GitVersions::log`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMeta {
    pub commit: ObjectId,
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    /// The whole message, trailers included, with one trailing newline removed.
    pub message: String,
    /// The message's trailers, in order.
    pub trailers: Vec<(String, String)>,
}

impl CommitMeta {
    pub fn from_commit(id: ObjectId, c: Commit) -> CommitMeta {
        let message = c
            .message
            .strip_suffix('\n')
            .unwrap_or(&c.message)
            .to_string();
        let trailers = parse_trailers(&message);
        CommitMeta {
            commit: id,
            tree: c.tree,
            parents: c.parents,
            author: c.author,
            committer: c.committer,
            message,
            trailers,
        }
    }

    /// The value of the first trailer `key`.
    pub fn trailer(&self, key: &str) -> Option<&str> {
        self.trailers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Artifact versions over the repositories of `H`.
#[derive(Clone, Debug, Default)]
pub struct GitVersions<H> {
    host: H,
}

async fn commit_of_repo(repo: &impl Repo, at: &Rev) -> Result<Option<ObjectId>> {
    let mut id = match at {
        Rev::Commit(id) => *id,
        Rev::Version(n) => match repo.refs().await?.get(&format!("refs/tags/v{n}")) {
            Some(id) => *id,
            None => return Ok(None),
        },
    };
    for _ in 0..16 {
        match repo.read_object(&id).await? {
            Some(o) if o.kind == ObjectKind::Commit => return Ok(Some(id)),
            Some(o) if o.kind == ObjectKind::Tag => match tag_target(&o.data) {
                Some((target, _)) => id = target,
                None => return Ok(None),
            },
            _ => return Ok(None),
        }
    }
    Ok(None)
}

async fn read_commit(repo: &impl Repo, id: &ObjectId) -> Result<Option<Commit>> {
    match repo.read_object(id).await? {
        Some(o) if o.kind == ObjectKind::Commit => Ok(Some(Commit::parse(&o.data)?)),
        _ => Ok(None),
    }
}

impl<H: RepoHost> GitVersions<H> {
    pub fn new(host: H) -> GitVersions<H> {
        GitVersions { host }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    async fn repo(&self, artifact: &str) -> Result<H::Repo> {
        let name = repo_name(artifact);
        self.host
            .open(&name)
            .await?
            .ok_or_else(|| GitError::not_found(format!("No repository {name}")))
    }

    /// Creates the artifact's empty repository.
    pub async fn create(&self, artifact: &str) -> Result<()> {
        self.host.create(&repo_name(artifact)).await
    }

    /// Deletes the artifact's repository.
    pub async fn delete(&self, artifact: &str) -> Result<()> {
        self.host.delete(&repo_name(artifact)).await
    }

    /// The commit `main` points at, or `None` before the first version.
    pub async fn head(&self, artifact: &str) -> Result<Option<ObjectId>> {
        Ok(self.repo(artifact).await?.refs().await?.get(MAIN).copied())
    }

    /// The commit of `at`, or `None` when the repository has no such version
    /// or commit.
    pub async fn commit_of(&self, artifact: &str, at: &Rev) -> Result<Option<ObjectId>> {
        commit_of_repo(&self.repo(artifact).await?, at).await
    }

    /// Commits `p.changes` on top of `p.parent` as version `p.version` and
    /// moves `main` and the tag `v<version>` to it in one atomic update.
    ///
    /// - `main` not at `p.parent` (before, or raced during the update):
    ///   `version_conflict` with the head; nothing moved.
    /// - The tag already exists while `main` is current: `invalid_argument`;
    ///   nothing moved.
    /// - A repository that took `main` but refused the tag (a remote without
    ///   atomic updates): the result carries `tag_error`.
    pub async fn publish(&self, artifact: &str, p: &Publish) -> Result<Published> {
        let repo = self.repo(artifact).await?;
        let tag = tag_ref(&version_tag(p.version))?;
        let message = with_trailers(&p.meta.note, &version_trailers(p.version, &p.meta, &[]))?;
        let parent_tree = match &p.parent {
            None => None,
            Some(parent) => Some(
                read_commit(&repo, parent)
                    .await?
                    .ok_or_else(|| GitError::VersionConflict {
                        message: format!("Commit {parent} is not in {}", repo_name(artifact)),
                        head: None,
                    })?
                    .tree,
            ),
        };
        let (tree, mut objects) = build_tree(&repo, parent_tree, &p.changes).await?;
        let sig = Signature::utc(&p.meta.author, p.meta.time);
        let commit = Commit {
            tree,
            parents: p.parent.into_iter().collect(),
            author: sig.clone(),
            committer: sig,
            message,
        }
        .to_object()?;
        let id = commit.id();
        objects.push(commit);
        let updates = [
            RefUpdate::new(MAIN, p.parent, Some(id)),
            RefUpdate::new(tag.clone(), None, Some(id)),
        ];
        let tag_error = commit_refs(&repo, objects, &updates).await?;
        Ok(Published {
            commit: id,
            tree,
            tag_error,
        })
    }

    /// Creates `to` as a fork of `from` at version `from_version` (whose
    /// commit is `at`, normally `Rev::Version(from_version)`): copies the
    /// whole history, drops the copied version tags (the fork numbers its
    /// own versions), and commits the fork's v1, with the tree of `at`, on
    /// top of the copied `main`, tagged `v1`. Its note is `meta.note`, or
    /// `Forked from <from>@v<from_version>` when that is blank.
    pub async fn fork(
        &self,
        from: &str,
        to: &str,
        at: &Rev,
        from_version: u64,
        meta: &VersionMeta,
    ) -> Result<Published> {
        let source = self.repo(from).await?;
        let source_commit = commit_of_repo(&source, at).await?.ok_or_else(|| {
            GitError::not_found(format!("No such version in {}", repo_name(from)))
        })?;
        self.host.fork(&repo_name(from), &repo_name(to)).await?;
        let repo = self.repo(to).await?;
        let refs = repo.refs().await?;
        let drop: Vec<RefUpdate> = refs
            .iter()
            .filter(|(name, _)| version_of_tag_ref(name).is_some())
            .map(|(name, id)| RefUpdate::new(name.clone(), Some(*id), None))
            .collect();
        if !drop.is_empty() {
            for (u, s) in drop.iter().zip(repo.apply(Vec::new(), &drop, false).await?) {
                if let Err(reason) = s {
                    return Err(GitError::unavailable(format!(
                        "Could not delete {}: {reason}",
                        u.name
                    )));
                }
            }
        }
        let tree = read_commit(&repo, &source_commit)
            .await?
            .ok_or_else(|| GitError::unavailable(format!("Commit {source_commit} is missing")))?
            .tree;
        let head = refs.get(MAIN).copied();
        let reference = fork_ref(from, from_version);
        let note = if meta.note.trim().is_empty() {
            format!("Forked from {reference}")
        } else {
            meta.note.clone()
        };
        let extra = [("Clax-Forked-From".to_string(), reference)];
        let message = with_trailers(&note, &version_trailers(1, meta, &extra))?;
        let sig = Signature::utc(&meta.author, meta.time);
        let commit = Commit {
            tree,
            parents: head.into_iter().collect(),
            author: sig.clone(),
            committer: sig,
            message,
        }
        .to_object()?;
        let id = commit.id();
        let updates = [
            RefUpdate::new(MAIN, head, Some(id)),
            RefUpdate::new(tag_ref("v1")?, None, Some(id)),
        ];
        let tag_error = commit_refs(&repo, vec![commit], &updates).await?;
        Ok(Published {
            commit: id,
            tree,
            tag_error,
        })
    }

    /// The file at `path` in `at`, or `None` when the version, commit or
    /// path is not there or names no file.
    pub async fn read_file(&self, artifact: &str, at: &Rev, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(repo) = self.host.open(&repo_name(artifact)).await? else {
            return Ok(None);
        };
        let Some(commit) = commit_of_repo(&repo, at).await? else {
            return Ok(None);
        };
        let Some(c) = read_commit(&repo, &commit).await? else {
            return Ok(None);
        };
        let Some(entry) = resolve_path(&repo, &c.tree, path).await? else {
            return Ok(None);
        };
        if !entry.mode.is_blob() {
            return Ok(None);
        }
        match repo.read_object(&entry.id).await? {
            Some(o) if o.kind == ObjectKind::Blob => Ok(Some(o.data)),
            _ => Ok(None),
        }
    }

    /// Every file of `at`, recursively, by path (submodules left out).
    /// `not_found` when there is no such version or commit.
    pub async fn list_files(
        &self,
        artifact: &str,
        at: &Rev,
    ) -> Result<BTreeMap<String, FileEntry>> {
        let repo = self.repo(artifact).await?;
        let commit = commit_of_repo(&repo, at).await?.ok_or_else(|| {
            GitError::not_found(format!("No such version in {}", repo_name(artifact)))
        })?;
        let c = read_commit(&repo, &commit)
            .await?
            .ok_or_else(|| GitError::unavailable(format!("Commit {commit} is missing")))?;
        let mut out = BTreeMap::new();
        for e in walk_tree(&repo, &c.tree).await? {
            if e.mode == Mode::GITLINK {
                continue;
            }
            let Some(o) = repo.read_object(&e.id).await? else {
                return Err(GitError::unavailable(format!("Blob {} is missing", e.id)));
            };
            out.insert(
                e.path,
                FileEntry {
                    size: o.data.len() as u64,
                    id: e.id,
                    mode: e.mode,
                },
            );
        }
        Ok(out)
    }

    /// The version tags, by version number, to the commit each names.
    pub async fn versions(&self, artifact: &str) -> Result<BTreeMap<u64, ObjectId>> {
        let refs = self.repo(artifact).await?.refs().await?;
        Ok(refs
            .iter()
            .filter_map(|(name, id)| version_of_tag_ref(name).map(|n| (n, *id)))
            .collect())
    }

    /// The first-parent chain of `main`, newest first, stopping before
    /// `since` (exclusive) or after `limit` commits. When `since` is not on
    /// the chain (rewritten history) the whole chain up to `limit` is
    /// answered.
    pub async fn log(
        &self,
        artifact: &str,
        since: Option<ObjectId>,
        limit: Option<usize>,
    ) -> Result<Vec<CommitMeta>> {
        let repo = self.repo(artifact).await?;
        let limit = limit.unwrap_or(usize::MAX);
        let mut out = Vec::new();
        let mut at = repo.refs().await?.get(MAIN).copied();
        while let Some(id) = at {
            if Some(id) == since || out.len() >= limit {
                break;
            }
            let c = read_commit(&repo, &id)
                .await?
                .ok_or_else(|| GitError::unavailable(format!("Commit {id} is missing")))?;
            at = c.parents.first().copied();
            out.push(CommitMeta::from_commit(id, c));
        }
        Ok(out)
    }

    /// Points each named tag at its commit, creating or moving it, or deletes
    /// it for `None` (a missing tag is skipped). `invalid_argument` for a
    /// bad tag name or a commit the repository does not hold.
    pub async fn set_tags(
        &self,
        artifact: &str,
        tags: &BTreeMap<String, Option<ObjectId>>,
    ) -> Result<()> {
        let repo = self.repo(artifact).await?;
        let refs = repo.refs().await?;
        let mut updates = Vec::new();
        for (name, want) in tags {
            let r = tag_ref(name)?;
            if let Some(c) = want
                && read_commit(&repo, c).await?.is_none()
            {
                return Err(GitError::invalid(format!(
                    "No commit {c} in {}",
                    repo_name(artifact)
                )));
            }
            let at = refs.get(&r).copied();
            if at != *want {
                updates.push(RefUpdate::new(r, at, *want));
            }
        }
        for (u, s) in updates
            .iter()
            .zip(repo.apply(Vec::new(), &updates, true).await?)
        {
            if let Err(reason) = s {
                return Err(GitError::unavailable(format!(
                    "Could not update {}: {reason}",
                    u.name
                )));
            }
        }
        Ok(())
    }
}

fn conflict(head: Option<ObjectId>, parent: Option<ObjectId>) -> GitError {
    let show = |c: Option<ObjectId>| c.map_or("no commit".to_string(), |c| c.to_string());
    GitError::VersionConflict {
        message: format!("main is at {}, not {}", show(head), show(parent)),
        head,
    }
}

/// Writes `objects` and moves `[main, tag]` in one atomic update, as a
/// publish does, mapping a refusal: `main` moved meanwhile is
/// `version_conflict` with the head now; the tag created meanwhile is
/// `invalid_argument`; a tag refused for a reason of its own is
/// `repo_unavailable` naming it; else `main`'s refusal is
/// `version_conflict` (with the head now) when
/// its reason means a race, `repo_unavailable` otherwise. Answers the tag
/// error when a repository took `main` but refused the tag.
pub async fn commit_refs(
    repo: &impl Repo,
    objects: Vec<crate::object::Object>,
    updates: &[RefUpdate; 2],
) -> Result<Option<String>> {
    let statuses = repo.apply(objects, updates, true).await?;
    let [main_status, tag_status] = <[_; 2]>::try_from(statuses)
        .map_err(|_| GitError::unavailable("The repository did not report on every ref"))?;
    let (main, tag) = (&updates[0], &updates[1]);
    match (main_status, tag_status) {
        (Ok(()), Ok(())) => Ok(None),
        (Ok(()), Err(reason)) => Ok(Some(format!(
            "The git remote refused {}: {reason}",
            tag.name
        ))),
        (Err(main_reason), tag_status) => {
            // When a ref races, every ref gets the same reason, so the refs
            // as they stand now tell which one was taken.
            let now = repo.refs().await?;
            let now_head = now.get(MAIN).copied();
            let expected = (!main.old.is_zero()).then_some(main.old);
            if now_head != expected {
                return Err(conflict(now_head, expected));
            }
            if now.contains_key(&tag.name) {
                return Err(GitError::invalid(format!("{} already exists", tag.name)));
            }
            if let Err(reason) = tag_status
                && !is_atomic_collateral(&reason)
            {
                return Err(GitError::unavailable(format!(
                    "The git remote refused {}: {reason}",
                    tag.name
                )));
            }
            if is_conflict_reason(&main_reason) {
                Err(GitError::VersionConflict {
                    message: format!("The git remote refused {}: {main_reason}", main.name),
                    head: now_head,
                })
            } else {
                Err(GitError::unavailable(format!(
                    "The git remote refused {}: {main_reason}",
                    main.name
                )))
            }
        }
    }
}

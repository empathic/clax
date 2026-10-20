//! [`GitContent`]: a [`ContentStore`] that keeps artifact versions in git,
//! one repository per artifact, over any [`RepoHost`]: clax-git's
//! in-memory host in tests, a Durable Object stand-in under `wrangler dev`,
//! Cloudflare Artifacts in production. The same code runs natively and on
//! `wasm32-unknown-unknown`; it does no I/O of its own.
//!
//! # Layout
//!
//! - Artifact `<id>` is the repository `a-<id>`, created on its first write.
//! - Version `n` is one commit on `main`, tagged `v<n>` (lightweight), whose
//!   tree holds exactly the version's files and whose parent is the commit
//!   of its predecessor, the newest version below `n` (none for the first).
//!   Its message is the publish note ([`VersionWrite::meta`]; `Version <n>`
//!   without one) followed by the trailers `Clax-Version: <n>`,
//!   `Clax-Publisher: <person ID>` and, when an agent published,
//!   `Clax-Agent: <agent name>`. Author and committer are the publishing
//!   person, `<name> <<person ID>@users.clax.dev>` (`someone` for a person
//!   without a name; `<`, `>` and control characters dropped, as git drops
//!   them), at the clock's time in UTC; a write whose metadata names no
//!   person is the store's committer's, without `Clax-Publisher`. So
//!   `git log` shows who made each version. A fork's version 1 is `Forked
//!   from <artifact ID>@v<n>` with `Clax-Forked-From: <artifact ID>@v<n>`
//!   after the trailers of its
//!   publisher (the forking person and agent, [`ContentStore::fork`]'s
//!   metadata), and is signed as a version they published.
//! - A carried file keeps its blob: the new tree is built from version
//!   `n - 1`'s tree, so neither its bytes nor its unchanged directories are
//!   read or written again.
//! - Assets are not in git: they go to an [`AssetStore`] the host provides
//!   (R2 or Durable Object storage in a Worker, the home's `assets/`
//!   directory natively). Assets are unversioned and deletable, and a
//!   deleted asset must be gone: in git its blob would stay in the packs
//!   (neither the stand-in nor Artifacts collects garbage on request) and in
//!   every fork; large binaries would also weigh on every clone and fork of
//!   the repository, whose history they are not part of.
//!
//! # Conditional ref updates
//!
//! Every ref change is one atomic [`Repo::apply`] whose old values are the
//! refs as read (compare and swap; over smart HTTP, receive-pack's
//! old-value check). So:
//!
//! - **Write** of `n` creates `v<n>` (it must not exist) and moves `main`
//!   forward from the predecessor's commit, or from the untagged commits a
//!   remove left above it (below). A second writer of `n`, or one that
//!   finds `main` moved (another process published, or a push landed), is
//!   refused and changes nothing. A repository without atomic updates that
//!   takes `main` and refuses the tag leaves the new commit on `main`
//!   untagged, as a remove would, and the write fails.
//! - **Remove** of `n` deletes `v<n>` only while it names the commit read,
//!   and, while `main` is at that commit, moves `main` back to its parent
//!   in the same update. A remote may refuse that move: it is not a fast
//!   forward (git's `receive.denyNonFastForwards`, clax-git's stand-in
//!   server by default), and for a first version, which has no parent, it
//!   would delete the current branch (always refused). Then the tag alone
//!   is deleted, while `main` is still at the commit, and `main` keeps it.
//!   Reads go by tags, so an untagged commit on `main` is never read; the
//!   next write of `n` builds on it (a fast forward; its tree still comes
//!   from version `n - 1`), so every update a write makes is a fast
//!   forward. An untagged commit counts as left by a remove when it
//!   carries `Clax-Version` and its first parents lead, through such
//!   commits only, to the predecessor (or to no commit, for a first
//!   version). When `main` is already back at the parent (a remove a
//!   repository without atomic updates left half done), the tag alone is
//!   deleted. When `main` has moved past the commit the remove fails and
//!   leaves both: it never discards a commit another writer put on top.
//!   This is the compensating step of a publish whose recording
//!   transaction failed (clax-core's content
//!   [Ordering](clax_core::content#ordering)); the alternative of rows
//!   naming content addresses, with refs advanced after the transaction,
//!   is not taken because the filesystem store has no content address.
//! - **Pushes** (clax-core's content [Pushes](clax_core::content#pushes)):
//!   a push to `main` puts commits above the newest version that no tag
//!   names. [`ContentStore::pushed_commits`] lists them, oldest first, by
//!   `main`'s first parents down to the commit of the version pushed past,
//!   leaving out the publish attempts a remove left directly above it;
//!   more than 64 of them, or a `main` that no longer leads to that
//!   version, are its head alone. [`ContentStore::tag_pushed`] creates
//!   `v<n>` on one (create-only) and leaves `main` where it is, so the next
//!   write finds `main` at version `n`'s commit; [`ContentStore::untag_pushed`]
//!   deletes the tag only while it names that commit. Who pushed and the
//!   repositories' tokens come from the [`PushLedger`] given with
//!   [`GitContent::with_ledger`] (what serves the pushes keeps them).
//! - **Fork** makes the fork's repository as far as the forked version's
//!   commit ([`fork_at`]: a copy cut back and pruned, or a new repository
//!   given what that commit reaches), so the fork's history ends at the
//!   forked version and holds nothing the source gained after it; then in
//!   one update it commits the fork's version 1 on that commit and points
//!   `main` and `v1` at it. When any step after the repository exists
//!   fails, it is deleted, so a retry starts clean.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use clax_core::content::{
    AssetKey, ContentStore, MAX_PUSHED_ENTRIES, PushLedger, PushedCommit, PushedContent,
    PushedFile, Pusher, RepoToken, VersionMeta, VersionWrite, missing, present, superseded,
};
use clax_core::working::Clock;
use clax_core::{ArtifactId, CoreError, Result};
use clax_git::object::with_trailers;
use clax_git::tree::{Changes, build_tree, walk_tree, walk_tree_within};
use clax_git::versions::{
    commit_refs, fork_at, fork_ref, person_email, repo_name, tag_ref, version_of_tag_ref,
    version_tag,
};
use clax_git::{
    Commit, GitError, GitVersions, MAIN, Mode, ObjectId, Person, RefUpdate, Repo, RepoHost, Rev,
    Signature,
};

/// Where a [`GitContent`] keeps assets: put, read and delete by key, and
/// delete all of an artifact's. Every [`ContentStore`] is one (through its
/// asset methods and [`ContentStore::delete_artifact`]), so natively the
/// home's [`clax_core::content::fs::FsContent`] serves, and in tests
/// [`clax_core::content::memory::MemContent`]. `Send + Sync` on every
/// target, as a [`ContentStore`] is (clax-core's content
/// [Futures](clax_core::content#futures)).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait AssetStore: Send + Sync {
    /// Stores `bytes` as the asset `key`, visible only once all are written.
    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()>;
    /// The bytes of asset `key`, or `None` when it is not there.
    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>>;
    /// Removes asset `key`; succeeds when it is not there.
    async fn delete_asset(&self, key: &AssetKey) -> Result<()>;
    /// Removes every asset of `id`; succeeds when there are none.
    async fn delete_assets(&self, id: &ArtifactId) -> Result<()>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<C: ContentStore> AssetStore for C {
    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        ContentStore::put_asset(self, key, bytes).await
    }
    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        ContentStore::read_asset(self, key).await
    }
    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        ContentStore::delete_asset(self, key).await
    }
    async fn delete_assets(&self, id: &ArtifactId) -> Result<()> {
        ContentStore::delete_artifact(self, id).await
    }
}

/// Artifact versions in the git repositories of `H`, assets in `A`. See the
/// [crate docs](crate).
pub struct GitContent<H, A> {
    versions: GitVersions<H>,
    assets: A,
    committer: Person,
    clock: Arc<dyn Clock>,
    needs_executor: bool,
    /// Who pushed, and the repositories' tokens ([`GitContent::with_ledger`]).
    ledger: Option<Arc<dyn PushLedger>>,
}

/// A git error as clax-core's: an I/O error of the kind its code names, so
/// a refused write reads as the store's other write failures do.
pub fn core_error(e: GitError) -> CoreError {
    use std::io::ErrorKind;
    let kind = match &e {
        GitError::InvalidArgument(_) => ErrorKind::InvalidInput,
        GitError::NotFound(_) => ErrorKind::NotFound,
        GitError::VersionConflict { .. } => ErrorKind::AlreadyExists,
        GitError::Auth { .. } | GitError::RepoUnavailable(_) => ErrorKind::Other,
    };
    CoreError::Io(std::io::Error::new(kind, e.to_string()))
}

fn unavailable(message: String) -> CoreError {
    CoreError::Io(std::io::Error::other(message))
}

/// The version tags among `refs`, by version number, to their commits.
fn versions_of(refs: &BTreeMap<String, ObjectId>) -> BTreeMap<u32, ObjectId> {
    refs.iter()
        .filter_map(|(name, id)| {
            let n = u32::try_from(version_of_tag_ref(name)?).ok()?;
            Some((n, *id))
        })
        .collect()
}

/// `s` as one line git takes in an ident or a trailer: without `<`, `>`
/// and control characters (as git's own ident cleaning drops them), and
/// trimmed, so no display name or agent name can make a publish fail.
fn ident_part(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .collect::<String>()
        .trim()
        .to_string()
}

/// The most untagged commits a remove may leave between `main` and the
/// newest version; a write finding more is refused.
const MAX_LEFT: usize = 64;

/// The most pushed commits one reconcile records one by one; past it,
/// `main`'s head stands for them all.
const MAX_PUSHED: usize = 64;

/// The most bytes of changed files one pushed commit's files carry
/// ([`ContentStore::pushed_files`]): what a request body may carry on the
/// Worker, whose isolate holds them.
const PUSHED_BYTES: usize = 16 << 20;

/// Whether an entry of `mode` is a regular file (executable or not).
fn is_regular(mode: Mode) -> bool {
    mode == Mode::FILE || mode == Mode::EXEC
}

/// Commit `hex`.
fn parse_commit(hex: &str) -> Result<ObjectId> {
    ObjectId::from_hex(hex).ok_or_else(|| {
        CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{hex:?} is not a commit ID"),
        ))
    })
}

fn version_ref(n: u32) -> String {
    tag_ref(&version_tag(u64::from(n))).expect("v<n> is a valid tag name")
}

/// The changes that make a tree holding exactly `keep` and `put` from a
/// base tree holding `base`: every other base path deleted, every put
/// written. A deletion a put already makes (a file under a path put as a
/// file, or a file where a put needs a directory) is left out, as the tree
/// builder refuses both in one set of changes.
fn changes_from(base: &[String], keep: &BTreeSet<&str>, put: &[(&str, &[u8])]) -> Changes {
    let put_paths: BTreeSet<&str> = put.iter().map(|(p, _)| *p).collect();
    let put_dirs: BTreeSet<&str> = put
        .iter()
        .flat_map(|(p, _)| p.match_indices('/').map(move |(i, _)| &p[..i]))
        .collect();
    let under_put = |path: &str| {
        path.match_indices('/')
            .any(|(i, _)| put_paths.contains(&path[..i]))
    };
    let mut changes = Changes::new();
    for path in base {
        let path = path.as_str();
        if keep.contains(path) || put_paths.contains(path) {
            continue;
        }
        if put_dirs.contains(path) || under_put(path) {
            continue;
        }
        changes.insert(path.to_string(), None);
    }
    for (path, bytes) in put {
        changes.insert((*path).to_string(), Some(bytes.to_vec()));
    }
    changes
}

impl<H: RepoHost, A: AssetStore> GitContent<H, A> {
    /// Versions in `host`'s repositories, committed as `committer` at
    /// `clock`'s time; assets in `assets`.
    pub fn new(host: H, assets: A, committer: Person, clock: Arc<dyn Clock>) -> GitContent<H, A> {
        GitContent {
            versions: GitVersions::new(host),
            assets,
            committer,
            clock,
            needs_executor: false,
            ledger: None,
        }
    }

    /// This store answering who pushed and the repositories' tokens from
    /// `ledger` (clax-core's content [Pushes](clax_core::content#pushes));
    /// without one it knows no pusher and no token.
    pub fn with_ledger(mut self, ledger: Arc<dyn PushLedger>) -> GitContent<H, A> {
        self.ledger = Some(ledger);
        self
    }

    /// This store saying its I/O completes only when an executor polls it
    /// ([`ContentStore::needs_executor`]): a host or asset store over
    /// JavaScript promises (a Worker's bindings), so the store's synchronous
    /// methods refuse it at once instead of failing part way.
    pub fn needing_executor(mut self) -> GitContent<H, A> {
        self.needs_executor = true;
        self
    }

    pub fn host(&self) -> &H {
        self.versions.host()
    }

    pub fn assets(&self) -> &A {
        &self.assets
    }

    fn signature(&self) -> Signature {
        Signature::utc(&self.committer, self.clock.now().timestamp())
    }

    /// The message and signature of version `n`'s commit, published as
    /// `meta` says: the note (else `Version <n>`), then `Clax-Version`,
    /// `Clax-Publisher` and `Clax-Agent`; signed by the publishing person
    /// (`<name> <<person ID>@users.clax.dev>`), or by the store's committer
    /// when `meta` names no person.
    fn version_commit(&self, n: u32, meta: &VersionMeta) -> Result<(String, Signature)> {
        let note = meta.note.as_deref().map(str::trim).unwrap_or("");
        let subject = if note.is_empty() {
            format!("Version {n}")
        } else {
            note.to_string()
        };
        self.commit_as(n, &subject, meta, Vec::new())
    }

    /// The message and signature of version `n`'s commit with `subject`,
    /// published as `meta` says (its note aside): `Clax-Version`,
    /// `Clax-Publisher`, `Clax-Agent`, then `extra`; signed as
    /// [`GitContent::version_commit`] signs.
    fn commit_as(
        &self,
        n: u32,
        subject: &str,
        meta: &VersionMeta,
        extra: Vec<(String, String)>,
    ) -> Result<(String, Signature)> {
        let mut trailers = vec![("Clax-Version".to_string(), n.to_string())];
        let sig = match meta.person.as_deref().map(ident_part) {
            Some(person) if !person.is_empty() => {
                trailers.push(("Clax-Publisher".to_string(), person.clone()));
                let name = meta
                    .person_name
                    .as_deref()
                    .map(ident_part)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "someone".to_string());
                let author = Person {
                    name,
                    email: person_email(&person),
                };
                Signature::utc(&author, self.clock.now().timestamp())
            }
            _ => self.signature(),
        };
        if let Some(agent) = meta.agent.as_deref().map(ident_part)
            && !agent.is_empty()
        {
            trailers.push(("Clax-Agent".to_string(), agent));
        }
        trailers.extend(extra);
        let message = with_trailers(subject, &trailers).map_err(core_error)?;
        Ok((message, sig))
    }

    async fn open(&self, id: &ArtifactId) -> Result<Option<H::Repo>> {
        self.host()
            .open(&repo_name(id.as_str()))
            .await
            .map_err(core_error)
    }

    /// The repository of `id`, created when there is none (a creation that
    /// races another is the other's repository).
    async fn open_or_create(&self, id: &ArtifactId) -> Result<H::Repo> {
        if let Some(repo) = self.open(id).await? {
            return Ok(repo);
        }
        match self.host().create(&repo_name(id.as_str())).await {
            Ok(()) | Err(GitError::InvalidArgument(_)) => {}
            Err(e) => return Err(core_error(e)),
        }
        self.open(id)
            .await?
            .ok_or_else(|| unavailable(format!("{} was not created", repo_name(id.as_str()))))
    }

    /// Whether `head` leads to `base` (`None`: to no commit) through
    /// commits a remove left: untagged (not in `tagged`), carrying
    /// `Clax-Version`, followed by their first parents. At most
    /// [`MAX_LEFT`] of them.
    async fn left_above(
        repo: &H::Repo,
        head: Option<ObjectId>,
        base: Option<ObjectId>,
        tagged: &BTreeSet<ObjectId>,
    ) -> Result<bool> {
        let mut at = head;
        for _ in 0..=MAX_LEFT {
            if at == base {
                return Ok(true);
            }
            let Some(c) = at else {
                return Ok(false);
            };
            if tagged.contains(&c) {
                return Ok(false);
            }
            let Some(meta) = repo.read_commit_meta(&c).await.map_err(core_error)? else {
                return Ok(false);
            };
            if meta.trailer("Clax-Version").is_none() {
                return Ok(false);
            }
            at = meta.parents.first().copied();
        }
        Ok(false)
    }

    /// The tree of commit `commit`.
    async fn tree_of(repo: &H::Repo, commit: &ObjectId) -> Result<ObjectId> {
        Ok(repo
            .read_commit_meta(commit)
            .await
            .map_err(core_error)?
            .ok_or_else(|| unavailable(format!("Commit {commit} is missing")))?
            .tree)
    }

    /// Every blob path of `tree` to its entry.
    async fn files_of(repo: &H::Repo, tree: &ObjectId) -> Result<BTreeMap<String, bool>> {
        Ok(walk_tree(repo, tree)
            .await
            .map_err(core_error)?
            .into_iter()
            .map(|e| (e.path, e.mode.is_blob()))
            .collect())
    }

    /// Writes `objects` and makes `updates` in one atomic update; any
    /// refusal is an error naming the refused ref.
    async fn apply_all(
        repo: &H::Repo,
        objects: Vec<clax_git::Object>,
        updates: &[RefUpdate],
    ) -> Result<()> {
        let statuses = repo
            .apply(objects, updates, true)
            .await
            .map_err(core_error)?;
        for (u, s) in updates.iter().zip(&statuses) {
            if let Err(reason) = s
                && !clax_git::client::is_atomic_collateral(reason)
            {
                return Err(unavailable(format!(
                    "The repository refused {}: {reason}",
                    u.name
                )));
            }
        }
        if statuses.len() != updates.len() || statuses.iter().any(|s| s.is_err()) {
            return Err(unavailable("The repository refused the update".to_string()));
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<H: RepoHost + Send + Sync, A: AssetStore> ContentStore for GitContent<H, A> {
    fn needs_executor(&self) -> bool {
        self.needs_executor
    }

    async fn read_file(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<Vec<u8>>> {
        self.versions
            .read_file(id.as_str(), &Rev::Version(u64::from(n)), path)
            .await
            .map_err(core_error)
    }

    async fn write_version(&self, id: &ArtifactId, n: u32, write: &VersionWrite<'_>) -> Result<()> {
        let repo = self.open_or_create(id).await?;
        let refs = repo.refs().await.map_err(core_error)?;
        let tags = versions_of(&refs);
        if tags.contains_key(&n) {
            return Err(present(id, n));
        }
        if let Some((&m, _)) = tags.range(n.saturating_add(1)..).next_back() {
            return Err(superseded(id, n, m));
        }
        let prev = tags.range(..n).next_back().map(|(k, c)| (*k, *c));
        let base = match prev {
            Some((k, commit)) if k + 1 == n => Some(Self::tree_of(&repo, &commit).await?),
            _ => None,
        };
        let base_files = match &base {
            Some(tree) => Self::files_of(&repo, tree).await?,
            None => BTreeMap::new(),
        };
        let keep: BTreeSet<&str> = write.carry.iter().copied().collect();
        for path in &keep {
            if base_files.get(*path) != Some(&true) {
                return Err(missing(id, n.saturating_sub(1), path));
            }
        }
        let put: Vec<(&str, &[u8])> = write.put.iter().map(|f| (f.path, f.bytes)).collect();
        let base_paths: Vec<String> = base_files.into_keys().collect();
        let changes = changes_from(&base_paths, &keep, &put);
        let (tree, mut objects) = build_tree(&repo, base, &changes)
            .await
            .map_err(core_error)?;
        // The new commit goes on main as it must be (compare and swap): the
        // predecessor's commit, or commits a remove left above it.
        let prev_commit = prev.map(|(_, c)| c);
        let head = refs.get(MAIN).copied();
        let tagged: BTreeSet<ObjectId> = tags.values().copied().collect();
        let parent =
            if head != prev_commit && Self::left_above(&repo, head, prev_commit, &tagged).await? {
                head
            } else {
                prev_commit
            };
        let (message, sig) = self.version_commit(n, &write.meta)?;
        let commit = Commit {
            tree,
            parents: parent.into_iter().collect(),
            author: sig.clone(),
            committer: sig,
            message,
        }
        .to_object()
        .map_err(core_error)?;
        let new = commit.id();
        objects.push(commit);
        let updates = [
            RefUpdate::new(MAIN, parent, Some(new)),
            RefUpdate::new(version_ref(n), None, Some(new)),
        ];
        match commit_refs(&repo, objects, &updates).await {
            Ok(None) => Ok(()),
            // A repository without atomic updates took main but not the
            // tag: the new commit stays on main untagged, as a remove
            // leaves one, and the next write builds on it.
            Ok(Some(tag_error)) => Err(unavailable(tag_error)),
            Err(e) => Err(core_error(e)),
        }
    }

    async fn fork(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        meta: &VersionMeta,
    ) -> Result<()> {
        let first = paths.first().copied().unwrap_or(clax_core::publish::INDEX);
        let source = self
            .open(from)
            .await?
            .ok_or_else(|| missing(from, from_n, first))?;
        let source_commit = *versions_of(&source.refs().await.map_err(core_error)?)
            .get(&from_n)
            .ok_or_else(|| missing(from, from_n, first))?;
        let source_tree = Self::tree_of(&source, &source_commit).await?;
        let source_files = Self::files_of(&source, &source_tree).await?;
        let keep: BTreeSet<&str> = paths.iter().copied().collect();
        for path in &keep {
            if source_files.get(*path) != Some(&true) {
                return Err(missing(from, from_n, path));
            }
        }
        if let Some(existing) = self.open(to).await? {
            let tags = versions_of(&existing.refs().await.map_err(core_error)?);
            if tags.contains_key(&1) {
                return Err(present(to, 1));
            }
            if let Some((&m, _)) = tags.iter().next_back() {
                return Err(superseded(to, 1, m));
            }
            // No version: what a failed fork or a first write left.
            self.host()
                .delete(&repo_name(to.as_str()))
                .await
                .map_err(core_error)?;
        }
        let name = repo_name(to.as_str());
        let made = async {
            // The fork's history ends at the forked version: its repository
            // holds only what that commit reaches, and v1's parent is it.
            let repo = fork_at(self.host(), &repo_name(from.as_str()), &name, source_commit)
                .await
                .map_err(core_error)?;
            let base_paths: Vec<String> = source_files.into_keys().collect();
            let changes = changes_from(&base_paths, &keep, &[]);
            let (tree, mut objects) = build_tree(&repo, Some(source_tree), &changes)
                .await
                .map_err(core_error)?;
            let reference = fork_ref(from.as_str(), u64::from(from_n));
            let (message, sig) = self.commit_as(
                1,
                &format!("Forked from {reference}"),
                meta,
                vec![("Clax-Forked-From".to_string(), reference)],
            )?;
            let commit = Commit {
                tree,
                parents: vec![source_commit],
                author: sig.clone(),
                committer: sig,
                message,
            }
            .to_object()
            .map_err(core_error)?;
            let new = commit.id();
            objects.push(commit);
            let updates = [
                RefUpdate::new(MAIN, Some(source_commit), Some(new)),
                RefUpdate::new(version_ref(1), None, Some(new)),
            ];
            Self::apply_all(&repo, objects, &updates).await
        }
        .await;
        if made.is_err() {
            let _ = self.host().delete(&name).await;
        }
        made
    }

    async fn remove_version(&self, id: &ArtifactId, n: u32) -> Result<()> {
        let Some(repo) = self.open(id).await? else {
            return Ok(());
        };
        let refs = repo.refs().await.map_err(core_error)?;
        let tags = versions_of(&refs);
        let Some(&commit) = tags.get(&n) else {
            return Ok(());
        };
        if let Some((&m, _)) = tags.range(n.saturating_add(1)..).next_back() {
            return Err(superseded(id, n, m));
        }
        let parent = repo
            .read_commit_meta(&commit)
            .await
            .map_err(core_error)?
            .ok_or_else(|| unavailable(format!("Commit {commit} is missing")))?
            .parents
            .first()
            .copied();
        let tag = RefUpdate::new(version_ref(n), Some(commit), None);
        let main = refs.get(MAIN).copied();
        if main == Some(commit)
            && let Some(parent) = parent
        {
            // Main first: a repository without atomic updates that stops
            // after it leaves main moved back and the tag, which a retry
            // deletes by the branch below.
            let back = [
                RefUpdate::new(MAIN, Some(commit), Some(parent)),
                tag.clone(),
            ];
            let Err(refused) = Self::apply_all(&repo, Vec::new(), &back).await else {
                return Ok(());
            };
            // A remote that refuses moving main back (not a fast forward)
            // keeps it; the tag goes alone while main is as read or back.
            let now = repo.refs().await.map_err(core_error)?;
            let now_main = now.get(MAIN).copied();
            if now.get(&tag.name) != Some(&commit)
                || (now_main != Some(commit) && now_main != Some(parent))
            {
                return Err(refused);
            }
            return Self::apply_all(&repo, Vec::new(), &[tag]).await;
        }
        let tagged: BTreeSet<ObjectId> = tags.values().copied().collect();
        if main == Some(commit)
            || main == parent
            || Self::left_above(&repo, main, Some(commit), &tagged).await?
        {
            // A first version, whose main stays (deleting the current
            // branch is refused); main already back below the version; or
            // main on commits a remove left above it: nothing is built on
            // the version.
            return Self::apply_all(&repo, Vec::new(), &[tag]).await;
        }
        Err(unavailable(format!(
            "main of {} is not at version {n}'s commit {commit}; version {n} is left",
            repo_name(id.as_str())
        )))
    }

    async fn pushed_commits(&self, id: &ArtifactId, after: u32) -> Result<Vec<PushedCommit>> {
        let Some(repo) = self.open(id).await? else {
            return Ok(Vec::new());
        };
        let refs = repo.refs().await.map_err(core_error)?;
        let tags = versions_of(&refs);
        let Some(head) = refs.get(MAIN).copied() else {
            return Ok(Vec::new());
        };
        let base = match tags.get(&after) {
            Some(c) => Some(*c),
            // Its content is lost: nothing is known to follow it.
            None if after > 0 => return Ok(Vec::new()),
            None => None,
        };
        if Some(head) == base {
            return Ok(Vec::new());
        }
        // main's first parents, newest first, down to the base.
        let mut chain: Vec<clax_git::CommitMeta> = Vec::new();
        let mut at = Some(head);
        let mut reached = false;
        for _ in 0..MAX_PUSHED + MAX_LEFT + 1 {
            if at == base {
                reached = true;
                break;
            }
            let Some(c) = at else { break };
            let meta = repo
                .read_commit_meta(&c)
                .await
                .map_err(core_error)?
                .ok_or_else(|| unavailable(format!("Commit {c} is missing")))?;
            at = meta.parents.first().copied();
            chain.push(meta);
        }
        let tagged: BTreeSet<ObjectId> = tags.values().copied().collect();
        if reached {
            chain.reverse();
            // Publish attempts a remove left directly above the base.
            let left = chain
                .iter()
                .take_while(|c| !tagged.contains(&c.commit) && c.trailer("Clax-Version").is_some())
                .count();
            chain.drain(..left);
            if chain.is_empty() {
                return Ok(Vec::new());
            }
            if chain.len() <= MAX_PUSHED {
                // Each one's first parent is the one below it (or the
                // base, through the attempts a remove left).
                return Ok(chain
                    .into_iter()
                    .map(|c| PushedCommit {
                        commit: c.commit.to_hex(),
                        message: c.message,
                        follows: true,
                        collapsed: false,
                    })
                    .collect());
            }
        }
        // Too many commits, or main no longer leads to the base: its head
        // stands for them all.
        let meta = repo
            .read_commit_meta(&head)
            .await
            .map_err(core_error)?
            .ok_or_else(|| unavailable(format!("Commit {head} is missing")))?;
        if !reached && meta.trailer("Clax-Version").is_some() && !tagged.contains(&head) {
            // An untagged publish attempt on a history that lost the base.
            return Ok(Vec::new());
        }
        Ok(vec![PushedCommit {
            commit: head.to_hex(),
            message: meta.message,
            follows: false,
            collapsed: true,
        }])
    }

    async fn pushed_files(
        &self,
        id: &ArtifactId,
        commit: &str,
        base: u32,
        published: &BTreeMap<String, clax_core::model::FileMeta>,
    ) -> Result<Vec<PushedFile>> {
        let repo = self
            .open(id)
            .await?
            .ok_or_else(|| unavailable(format!("{} is missing", repo_name(id.as_str()))))?;
        let commit = parse_commit(commit)?;
        let tree = Self::tree_of(&repo, &commit).await?;
        let refs = repo.refs().await.map_err(core_error)?;
        let base_entries: BTreeMap<String, ObjectId> = match versions_of(&refs).get(&base) {
            Some(c) => {
                let base_tree = Self::tree_of(&repo, c).await?;
                walk_tree(&repo, &base_tree)
                    .await
                    .map_err(core_error)?
                    .into_iter()
                    // A path of the commit the version left out is not
                    // the version's: read it again.
                    .filter(|e| is_regular(e.mode) && published.contains_key(&e.path))
                    .map(|e| (e.path, e.id))
                    .collect()
            }
            None => BTreeMap::new(),
        };
        let Some(entries) = walk_tree_within(&repo, &tree, MAX_PUSHED_ENTRIES)
            .await
            .map_err(core_error)?
        else {
            return Err(CoreError::invalid(
                "push_too_large",
                format!("the commit's trees hold more than {MAX_PUSHED_ENTRIES} entries"),
            ));
        };
        let mut out = Vec::new();
        // Each blob is read once (`None`: it did not fit), and the files
        // answered carry at most PUSHED_BYTES in all.
        let mut read: BTreeMap<ObjectId, Option<Vec<u8>>> = BTreeMap::new();
        let mut total = 0usize;
        for e in entries {
            let content = if !is_regular(e.mode) {
                PushedContent::NotAFile
            } else if base_entries.get(&e.path) == Some(&e.id) {
                PushedContent::Same
            } else {
                if let std::collections::btree_map::Entry::Vacant(slot) = read.entry(e.id) {
                    let blob = if total >= PUSHED_BYTES {
                        None
                    } else {
                        repo.read_object(&e.id)
                            .await
                            .map_err(core_error)?
                            .ok_or_else(|| unavailable(format!("Blob {} is missing", e.id)))
                            .map(Some)?
                    };
                    slot.insert(
                        blob.map(|b| b.data)
                            .filter(|d| total + d.len() <= PUSHED_BYTES),
                    );
                }
                match &read[&e.id] {
                    Some(bytes) if total + bytes.len() <= PUSHED_BYTES => {
                        total += bytes.len();
                        PushedContent::Bytes(bytes.clone())
                    }
                    _ => PushedContent::TooLarge,
                }
            };
            out.push(PushedFile {
                path: e.path,
                content,
            });
        }
        Ok(out)
    }

    async fn tag_pushed(&self, id: &ArtifactId, n: u32, commit: &str) -> Result<bool> {
        let repo = self
            .open(id)
            .await?
            .ok_or_else(|| unavailable(format!("{} is missing", repo_name(id.as_str()))))?;
        let commit = parse_commit(commit)?;
        let tags = versions_of(&repo.refs().await.map_err(core_error)?);
        match tags.get(&n) {
            Some(c) if *c == commit => return Ok(false),
            Some(_) => return Err(present(id, n)),
            None => {}
        }
        if let Some((&m, _)) = tags.range(n.saturating_add(1)..).next_back() {
            return Err(superseded(id, n, m));
        }
        Self::apply_all(
            &repo,
            Vec::new(),
            &[RefUpdate::new(version_ref(n), None, Some(commit))],
        )
        .await
        .map(|()| true)
    }

    async fn untag_pushed(&self, id: &ArtifactId, n: u32, commit: &str) -> Result<()> {
        let Some(repo) = self.open(id).await? else {
            return Ok(());
        };
        let commit = parse_commit(commit)?;
        let tags = versions_of(&repo.refs().await.map_err(core_error)?);
        match tags.get(&n) {
            None => Ok(()),
            Some(c) if *c == commit => {
                Self::apply_all(
                    &repo,
                    Vec::new(),
                    &[RefUpdate::new(version_ref(n), Some(commit), None)],
                )
                .await
            }
            Some(_) => Err(present(id, n)),
        }
    }

    async fn pushers(&self, id: &ArtifactId, commits: &[String]) -> Result<Vec<Option<Pusher>>> {
        match &self.ledger {
            Some(l) => {
                let named = l.pushers(id, commits).await?;
                if named.len() != commits.len() {
                    return Err(unavailable(format!(
                        "The push ledger named {} pushers for {} commits",
                        named.len(),
                        commits.len()
                    )));
                }
                Ok(named)
            }
            None => Ok(vec![None; commits.len()]),
        }
    }

    async fn repo_tokens(&self, id: &ArtifactId) -> Result<Vec<RepoToken>> {
        match &self.ledger {
            Some(l) => l.tokens(id).await,
            None => Ok(Vec::new()),
        }
    }

    async fn revoke_repo_token(&self, id: &ArtifactId, token: &str) -> Result<()> {
        match &self.ledger {
            Some(l) => l.revoke(id, token).await,
            None => Err(unavailable(format!(
                "{} keeps no token {token}",
                repo_name(id.as_str())
            ))),
        }
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        self.host()
            .delete(&repo_name(id.as_str()))
            .await
            .map_err(core_error)?;
        self.assets.delete_assets(id).await
    }

    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> Result<()> {
        self.assets.put_asset(key, bytes).await
    }

    async fn read_asset(&self, key: &AssetKey) -> Result<Option<Vec<u8>>> {
        self.assets.read_asset(key).await
    }

    async fn delete_asset(&self, key: &AssetKey) -> Result<()> {
        self.assets.delete_asset(key).await
    }
}

/// Versions in clax-git's in-memory host and assets in memory: for tests,
/// and for hosts that keep nothing.
pub type MemGitContent =
    GitContent<clax_git::memory::MemoryHost, clax_core::content::memory::MemContent>;

impl MemGitContent {
    /// An empty in-memory store, committing as `committer` at `clock`'s time.
    pub fn in_memory(committer: Person, clock: Arc<dyn Clock>) -> MemGitContent {
        GitContent::new(
            clax_git::memory::MemoryHost::new(),
            clax_core::content::memory::MemContent::new(),
            committer,
            clock,
        )
    }
}

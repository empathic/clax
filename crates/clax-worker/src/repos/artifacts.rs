//! A [`RepoHost`] over Cloudflare Artifacts: repositories are created,
//! forked, deleted and read through the `ARTIFACTS` binding
//! ([`ArtifactsApi`], whose Worker implementation is `js::JsArtifacts`),
//! and written over git smart HTTP to each repository's remote with
//! clax-git's receive-pack client, since the binding has no write call.
//!
//! - **Reads**: commits through `readCommit` (as [`CommitMeta`]), trees
//!   through `readTree`, blobs through `readBlob`. A tree is rebuilt from
//!   its entries and checked against its ID; the type of every entry a
//!   tree read names is remembered for the repository handle's life, so a
//!   blob is read with one call. Refs come from the remote's receive-pack
//!   advertisement.
//! - **Writes**: [`clax_git::client::apply`] with the store's own write
//!   token, cached per repository (`create`'s initial token when its
//!   expiry can be read from it, else one minted for an hour) and replaced
//!   five minutes before it expires. When the remote refuses a token (401
//!   or 403) the request is sent once more with a newly minted one.
//! - **Tokens for people and agents**: [`ArtifactsTokens`], a
//!   [`TokenHost`] over `createToken` and `revokeToken`.
//! - **Readiness**: `get` throws `FORK_IN_PROGRESS` (or
//!   `IMPORT_IN_PROGRESS`) for a repository Artifacts is still making. A
//!   fork waits for its copy, and every `get` waits for such a repository,
//!   pausing [`READY_WAITS_MS`] between tries (about 9 s in all) before it
//!   fails `repo_unavailable`; a fork whose copy does not open in that time
//!   deletes it.
//!
//! Version tags are protected only where Clax serves receive-pack (record
//! 6ff810dc on `crates/clax-git/src/server.rs`): a push straight to an
//! Artifacts remote is checked by Artifacts' own rules.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex};

use clax_core::working::Clock;
use clax_git::client::{self, Remote};
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::object::{encode_tree, hash_object};
use clax_git::{
    Commit, CommitMeta, GitError, MaybeSend, MaybeSendSync, Mode, Object, ObjectId, ObjectKind,
    RefStatus, RefUpdate, Repo, RepoHost, TreeEntry,
};

use super::tokens::{Minted, Scope, TokenHost};

/// Lifetime of the store's own write tokens, in seconds.
pub const WRITE_TOKEN_TTL_S: u32 = 3600;
/// A cached write token is replaced this long before it expires.
pub const WRITE_TOKEN_MARGIN_MS: i64 = 5 * 60 * 1000;

/// The pauses between tries of a `get` that answers a repository still
/// being forked or imported.
pub const READY_WAITS_MS: &[u32] = &[100, 200, 400, 800, 1600, 2000, 2000, 2000];

/// What `create` and `fork` answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedRepo {
    pub remote: String,
    /// `create`'s initial write token, `art_v1_…?expires=<unix seconds>`.
    pub token: Option<String>,
}

/// A token as `createToken` answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiToken {
    pub id: String,
    pub plaintext: String,
    /// Milliseconds since the epoch.
    pub expires_ms: i64,
}

/// The binding's calls Clax makes on a namespace. Errors carry the
/// binding's codes as [`error_for_code`] maps them.
pub trait ArtifactsApi: MaybeSendSync {
    type Handle: ArtifactsRepoApi;

    /// `create(name, { setDefaultBranch: "main" })`.
    fn create(&self, name: &str)
    -> impl Future<Output = clax_git::Result<CreatedRepo>> + MaybeSend;
    /// `get(name)`; `None` for `NOT_FOUND`.
    fn get(
        &self,
        name: &str,
    ) -> impl Future<Output = clax_git::Result<Option<Self::Handle>>> + MaybeSend;
    /// `delete(name)`; whether there was one.
    fn delete(&self, name: &str) -> impl Future<Output = clax_git::Result<bool>> + MaybeSend;
    /// Waits `ms` milliseconds (a timer in a Worker).
    fn pause(&self, ms: u32) -> impl Future<Output = ()> + MaybeSend;
}

/// The calls Clax makes on a repository handle (released when dropped).
pub trait ArtifactsRepoApi: MaybeSendSync {
    /// `info().remote`.
    fn remote(&self) -> impl Future<Output = clax_git::Result<String>> + MaybeSend;
    fn create_token(
        &self,
        scope: Scope,
        ttl_s: u32,
    ) -> impl Future<Output = clax_git::Result<ApiToken>> + MaybeSend;
    /// Whether a live token was revoked.
    fn revoke_token(&self, id: &str) -> impl Future<Output = clax_git::Result<bool>> + MaybeSend;
    fn fork(&self, to: &str) -> impl Future<Output = clax_git::Result<CreatedRepo>> + MaybeSend;
    /// `readCommit(hash)`; `None` when there is no such commit.
    fn read_commit(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = clax_git::Result<Option<CommitMeta>>> + MaybeSend;
    /// `readTree(hash)`; `None` when there is no such tree.
    fn read_tree(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = clax_git::Result<Option<Vec<TreeEntry>>>> + MaybeSend;
    /// `readBlob(hash)`; `None` when missing or not a blob.
    fn read_blob(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = clax_git::Result<Option<Vec<u8>>>> + MaybeSend;
    /// `readFile({ ref, path })`; `None` when the path is missing or a
    /// directory.
    fn read_file(
        &self,
        rev: &str,
        path: &str,
    ) -> impl Future<Output = clax_git::Result<Option<Vec<u8>>>> + MaybeSend;
}

/// The error for an Artifacts error `code` (`NOT_FOUND`, `ALREADY_EXISTS`,
/// …) and its message.
pub fn error_for_code(code: Option<&str>, message: String) -> GitError {
    match code {
        Some("NOT_FOUND") => GitError::NotFound(message),
        Some(c @ (FORK_IN_PROGRESS | IMPORT_IN_PROGRESS)) => {
            GitError::RepoUnavailable(format!("{c}: {message}"))
        }
        Some(
            "ALREADY_EXISTS" | "INVALID_INPUT" | "INVALID_REPO_NAME" | "INVALID_TTL"
            | "INVALID_URL",
        ) => GitError::InvalidArgument(message),
        _ => GitError::RepoUnavailable(message),
    }
}

const FORK_IN_PROGRESS: &str = "FORK_IN_PROGRESS";
const IMPORT_IN_PROGRESS: &str = "IMPORT_IN_PROGRESS";

/// Whether `e` is [`error_for_code`]'s for a repository still being forked
/// or imported.
pub fn in_progress(e: &GitError) -> bool {
    matches!(e, GitError::RepoUnavailable(m)
        if m.starts_with(FORK_IN_PROGRESS) || m.starts_with(IMPORT_IN_PROGRESS))
}

/// `api.get(name)`, tried again after each of [`READY_WAITS_MS`] while the
/// repository is still being forked or imported.
pub async fn get_ready<B: ArtifactsApi>(
    api: &B,
    name: &str,
) -> clax_git::Result<Option<B::Handle>> {
    let mut waits = READY_WAITS_MS.iter();
    loop {
        match api.get(name).await {
            Err(e) if in_progress(&e) => match waits.next() {
                Some(&ms) => api.pause(ms).await,
                None => {
                    let total: u32 = READY_WAITS_MS.iter().sum();
                    return Err(GitError::RepoUnavailable(format!(
                        "Repository {name} is still being made after {total} ms ({e})"
                    )));
                }
            },
            r => return r,
        }
    }
}

/// The expiry, in milliseconds, of a token `art_v1_…?expires=<unix s>`.
pub fn token_expiry_ms(token: &str) -> Option<i64> {
    let (_, q) = token.split_once('?')?;
    q.split('&')
        .find_map(|kv| kv.strip_prefix("expires="))
        .and_then(|s| s.parse::<i64>().ok())
        .map(|s| s * 1000)
}

#[derive(Default)]
struct Caches {
    /// Repository to its write token and when it expires.
    tokens: HashMap<String, (String, i64)>,
    remotes: HashMap<String, String>,
}

struct Inner<B, T> {
    api: B,
    transport: T,
    clock: Arc<dyn Clock>,
    caches: Mutex<Caches>,
}

/// Repositories in Cloudflare Artifacts. Cloning shares the binding, the
/// transport and the caches; keep one per Durable Object instance, not one
/// per request, so write tokens and remotes are reused.
pub struct ArtifactsHost<B, T> {
    inner: Arc<Inner<B, T>>,
}

impl<B, T> Clone for ArtifactsHost<B, T> {
    fn clone(&self) -> Self {
        ArtifactsHost {
            inner: self.inner.clone(),
        }
    }
}

impl<B: ArtifactsApi, T: Transport> ArtifactsHost<B, T> {
    /// Repositories of `api`, written over `transport`; `clock` times the
    /// write tokens.
    pub fn new(api: B, transport: T, clock: Arc<dyn Clock>) -> ArtifactsHost<B, T> {
        ArtifactsHost {
            inner: Arc::new(Inner {
                api,
                transport,
                clock,
                caches: Mutex::default(),
            }),
        }
    }

    pub fn api(&self) -> &B {
        &self.inner.api
    }

    fn caches(&self) -> std::sync::MutexGuard<'_, Caches> {
        self.inner.caches.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now_ms(&self) -> i64 {
        self.inner.clock.now().timestamp_millis()
    }

    fn forget(&self, name: &str) {
        let mut c = self.caches();
        c.tokens.remove(name);
        c.remotes.remove(name);
    }

    fn learn(&self, name: &str, created: &CreatedRepo) {
        let mut c = self.caches();
        c.remotes.insert(name.to_string(), created.remote.clone());
        if let Some((t, exp)) = created
            .token
            .as_ref()
            .and_then(|t| Some((t, token_expiry_ms(t)?)))
        {
            c.tokens.insert(name.to_string(), (t.clone(), exp));
        }
    }
}

/// A repository of an [`ArtifactsHost`]: an open handle on it.
pub struct ArtifactsRepo<B: ArtifactsApi, T> {
    host: ArtifactsHost<B, T>,
    name: String,
    handle: B::Handle,
    /// Types learned from the trees read through this handle.
    kinds: Mutex<HashMap<ObjectId, ObjectKind>>,
}

impl<B: ArtifactsApi, T: Transport> ArtifactsRepo<B, T> {
    pub fn handle(&self) -> &B::Handle {
        &self.handle
    }

    /// The repository's remote, cached.
    pub async fn remote(&self) -> clax_git::Result<String> {
        if let Some(r) = self.host.caches().remotes.get(&self.name) {
            return Ok(r.clone());
        }
        let r = self.handle.remote().await?;
        self.host
            .caches()
            .remotes
            .insert(self.name.clone(), r.clone());
        Ok(r)
    }

    /// The store's write token: the cached one while it has more than
    /// [`WRITE_TOKEN_MARGIN_MS`] to live, else a new one.
    async fn write_token(&self, fresh: bool) -> clax_git::Result<String> {
        let now = self.host.now_ms();
        let cached = self.host.caches().tokens.get(&self.name).cloned();
        if let Some((t, _)) = cached.filter(|(_, exp)| !fresh && exp - WRITE_TOKEN_MARGIN_MS > now)
        {
            return Ok(t);
        }
        let t = self
            .handle
            .create_token(Scope::Write, WRITE_TOKEN_TTL_S)
            .await?;
        self.host
            .caches()
            .tokens
            .insert(self.name.clone(), (t.plaintext.clone(), t.expires_ms));
        Ok(t.plaintext)
    }

    /// A file of `rev` (a commit, or a ref such as `refs/tags/v3`), read
    /// with one `readFile` call.
    pub async fn read_file_at(&self, rev: &str, path: &str) -> clax_git::Result<Option<Vec<u8>>> {
        match self.handle.read_file(rev, path).await {
            Err(GitError::NotFound(_)) => Ok(None),
            r => r,
        }
    }

    fn kind_of(&self, id: &ObjectId) -> Option<ObjectKind> {
        self.kinds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .copied()
    }

    async fn tree(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        let Some(entries) = self.handle.read_tree(id).await? else {
            return Ok(None);
        };
        {
            let mut kinds = self.kinds.lock().unwrap_or_else(|e| e.into_inner());
            for e in &entries {
                if e.mode.is_tree() {
                    kinds.insert(e.id, ObjectKind::Tree);
                } else if e.mode.is_blob() {
                    kinds.insert(e.id, ObjectKind::Blob);
                }
            }
        }
        let data = encode_tree(&entries)?;
        if hash_object(ObjectKind::Tree, &data) != *id {
            return Err(GitError::RepoUnavailable(format!(
                "Tree {id} read through the binding does not hash back to its ID"
            )));
        }
        Ok(Some(Object::new(ObjectKind::Tree, data)))
    }

    async fn blob(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        Ok(self
            .handle
            .read_blob(id)
            .await?
            .map(|b| Object::new(ObjectKind::Blob, b)))
    }

    async fn commit(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        let Some(m) = self.handle.read_commit(id).await? else {
            return Ok(None);
        };
        let c = Commit {
            tree: m.tree,
            parents: m.parents,
            author: m.author,
            committer: m.committer,
            message: m.message,
        };
        let o = c.to_object()?;
        if o.id() != *id {
            return Err(GitError::RepoUnavailable(format!(
                "Commit {id} cannot be rebuilt from the binding's metadata"
            )));
        }
        Ok(Some(o))
    }
}

/// [`ArtifactsRepo`]'s transport: the host's, with the store's write token
/// as the bearer, sent once more with a new token when it is refused.
struct Authed<'a, B: ArtifactsApi, T> {
    repo: &'a ArtifactsRepo<B, T>,
}

fn with_bearer(mut r: HttpRequest, token: &str) -> HttpRequest {
    r.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
    r.headers.push(("authorization", format!("Bearer {token}")));
    r
}

impl<B: ArtifactsApi, T: Transport> Transport for Authed<'_, B, T> {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let t = &self.repo.host.inner.transport;
        let token = self.repo.write_token(false).await?;
        let r = t.send(with_bearer(request.clone(), &token)).await?;
        if r.status != 401 && r.status != 403 {
            return Ok(r);
        }
        self.repo.host.caches().tokens.remove(&self.repo.name);
        let token = self.repo.write_token(true).await?;
        t.send(with_bearer(request, &token)).await
    }
}

impl<B: ArtifactsApi, T: Transport> Repo for ArtifactsRepo<B, T> {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        match self.kind_of(id) {
            Some(ObjectKind::Blob) => return self.blob(id).await,
            Some(ObjectKind::Tree) => return self.tree(id).await,
            Some(ObjectKind::Commit) => return self.commit(id).await,
            _ => {}
        }
        if let Some(o) = self.tree(id).await? {
            return Ok(Some(o));
        }
        if let Some(o) = self.blob(id).await? {
            return Ok(Some(o));
        }
        self.commit(id).await
    }

    async fn read_commit_meta(&self, id: &ObjectId) -> clax_git::Result<Option<CommitMeta>> {
        self.handle.read_commit(id).await
    }

    async fn refs(&self) -> clax_git::Result<BTreeMap<String, ObjectId>> {
        let remote = Remote::new(self.remote().await?, "");
        let adv = client::advertised_refs(&remote, &Authed { repo: self }).await?;
        Ok(adv
            .refs
            .into_iter()
            .filter(|(name, _)| !name.ends_with("^{}"))
            .collect())
    }

    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> clax_git::Result<Vec<RefStatus>> {
        let remote = Remote::new(self.remote().await?, "");
        client::apply(&remote, &Authed { repo: self }, &objects, updates, atomic).await
    }
}

impl<B: ArtifactsApi, T: Transport> RepoHost for ArtifactsHost<B, T> {
    type Repo = ArtifactsRepo<B, T>;

    async fn open(&self, name: &str) -> clax_git::Result<Option<ArtifactsRepo<B, T>>> {
        Ok(get_ready(&self.inner.api, name)
            .await?
            .map(|handle| ArtifactsRepo {
                host: self.clone(),
                name: name.to_string(),
                handle,
                kinds: Mutex::default(),
            }))
    }

    async fn create(&self, name: &str) -> clax_git::Result<()> {
        let created = self.inner.api.create(name).await?;
        self.learn(name, &created);
        Ok(())
    }

    async fn fork(&self, from: &str, to: &str) -> clax_git::Result<()> {
        let Some(h) = get_ready(&self.inner.api, from).await? else {
            return Err(GitError::NotFound(format!("No repository {from}")));
        };
        let created = h.fork(to).await?;
        drop(h);
        self.learn(to, &created);
        // Artifacts makes the copy in the background: wait until it opens.
        // A copy left behind would carry the source's version tags, which
        // the next fork to the name would take for the artifact's own, so
        // one that does not open is deleted.
        let ready = match get_ready(&self.inner.api, to).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                GitError::RepoUnavailable(format!("The fork {to} was not found after it was made"))
            }
            Err(e) => e,
        };
        self.forget(to);
        match self.inner.api.delete(to).await {
            Ok(_) | Err(GitError::NotFound(_)) => Err(ready),
            Err(e) => Err(GitError::RepoUnavailable(format!(
                "{ready}; deleting the unfinished copy {to} failed too: {e}"
            ))),
        }
    }

    /// False: Artifacts has no call that drops a repository's objects, so
    /// a fork is made as a new repository holding only what it reaches
    /// ([`clax_git::versions::fork_at`]), never as an Artifacts fork, which
    /// would keep every object the source held.
    fn can_prune(&self) -> bool {
        false
    }

    async fn prune(&self, name: &str) -> clax_git::Result<()> {
        Err(GitError::RepoUnavailable(format!(
            "Artifacts cannot drop the objects of {name}"
        )))
    }

    async fn delete(&self, name: &str) -> clax_git::Result<()> {
        self.forget(name);
        match self.inner.api.delete(name).await {
            Ok(_) | Err(GitError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Tokens for people and agents, minted and revoked by Artifacts.
pub struct ArtifactsTokens<B> {
    pub api: B,
}

impl<B: ArtifactsApi> TokenHost for ArtifactsTokens<B> {
    async fn mint(
        &self,
        repo: &str,
        scope: Scope,
        ttl_s: u32,
        _now_ms: i64,
    ) -> clax_git::Result<Minted> {
        let Some(h) = get_ready(&self.api, repo).await? else {
            return Err(GitError::NotFound(format!("No repository {repo}")));
        };
        let t = h.create_token(scope, ttl_s).await?;
        Ok(Minted {
            id: t.id,
            token: t.plaintext,
            expires_ms: t.expires_ms,
            hash: None,
        })
    }

    async fn revoke(&self, repo: &str, id: &str, _now_ms: i64) -> clax_git::Result<bool> {
        // A deleted repository's tokens went with it, and a token the
        // repository does not know (one of an earlier repository of the
        // name) is none of its own: neither is left to revoke.
        match get_ready(&self.api, repo).await? {
            None => Ok(false),
            Some(h) => match h.revoke_token(id).await {
                Err(GitError::NotFound(_)) => Ok(false),
                r => r,
            },
        }
    }

    async fn remote(&self, repo: &str, _origin: Option<&str>) -> clax_git::Result<String> {
        match get_ready(&self.api, repo).await? {
            None => Err(GitError::NotFound(format!("No repository {repo}"))),
            Some(h) => h.remote().await,
        }
    }
}

/// A tree entry from the binding's `mode` (octal text), `name` and `hash`.
pub fn tree_entry(mode: &str, name: String, hash: &str) -> clax_git::Result<TreeEntry> {
    let mode = u32::from_str_radix(mode, 8)
        .map_err(|_| GitError::RepoUnavailable(format!("A tree entry mode {mode:?}")))?;
    let id = ObjectId::from_hex(hash)
        .ok_or_else(|| GitError::RepoUnavailable(format!("A tree entry hash {hash:?}")))?;
    Ok(TreeEntry {
        mode: Mode(mode),
        name,
        id,
    })
}

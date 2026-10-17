//! The host's git remotes as the daemon holds them in memory (spec
//! 2026-10-11-clax-dev-hosted-design §9 "Tokens"): the [`GitRemotes`] a host
//! installs in a [`Slot`], what it mints and lists, and the ended sessions
//! whose tokens wait to be revoked. Who may have a token, and the routes
//! and layer that hand them out and settle revocations, are the HTTP
//! layer's `git_remotes`.

use clax_core::ArtifactId;
use clax_core::rights::RightType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// What a token allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Clone and fetch.
    Read,
    /// Push as well.
    Write,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "read" => Some(Scope::Read),
            "write" => Some(Scope::Write),
            _ => None,
        }
    }

    /// The right a token of this scope needs at its artifact.
    pub fn right(self) -> (RightType, &'static str) {
        match self {
            Scope::Read => (RightType::Artifacts, "read"),
            Scope::Write => (RightType::Artifacts, "update"),
        }
    }
}

/// Whom a token is handed to: a person's public ID (`None` for the owner)
/// and, for an agent's token, its session; or (`link`) whoever reads a
/// page through its link, a read token naming no one ([`Asker::Link`]). A
/// push with the token is theirs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub person: Option<String>,
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub link: bool,
}

/// A token as the host mints it, with the commands that clone with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Minted {
    /// Names the token to [`GitRemotes::revoke`].
    pub id: String,
    pub remote: String,
    /// The secret, shown once.
    pub token: String,
    /// RFC 3339, UTC.
    pub expires_at: String,
    /// `git -c http.extraHeader="Authorization: Bearer <token>" clone <remote>`
    pub clone_command: String,
    /// The same through `jj git clone`, the header in the environment's
    /// git configuration.
    pub jj_clone_command: String,
}

/// A live token the host lists ([`GitRemotes::held`]): never its secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub id: String,
    pub artifact: ArtifactId,
    pub scope: Scope,
    pub holder: Holder,
}

/// Why the host could not do what it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteError {
    /// The artifact has no repository.
    NotFound(String),
    /// The host failed or is unreachable.
    Unavailable(String),
}

/// The futures a [`GitRemotes`] answers with: `Send` natively; on wasm32
/// not, as a Worker's JavaScript futures are not (one thread).
#[cfg(not(target_arch = "wasm32"))]
pub type RemoteFuture<'a, T> = futures::future::BoxFuture<'a, Result<T, RemoteError>>;
/// The futures a [`GitRemotes`] answers with: `Send` natively; on wasm32
/// not, as a Worker's JavaScript futures are not (one thread).
#[cfg(target_arch = "wasm32")]
pub type RemoteFuture<'a, T> = futures::future::LocalBoxFuture<'a, Result<T, RemoteError>>;

/// Where artifacts' repositories are served and their tokens minted: the
/// host's (see the module docs). Rights are checked before it is asked.
pub trait GitRemotes: Send + Sync {
    /// A token for `artifact`'s repository living `ttl_s` seconds, handed
    /// to `holder`. `origin` is the app origin the asking request came to,
    /// for a host that serves its remotes there.
    fn mint<'a>(
        &'a self,
        artifact: &'a ArtifactId,
        scope: Scope,
        ttl_s: u32,
        origin: Option<&'a str>,
        holder: &'a Holder,
    ) -> RemoteFuture<'a, Minted>;

    /// Whether a push made with a token this host minted is credited to
    /// the token's holder. A host that cannot say who pushed (Cloudflare
    /// Artifacts: its pushed events name no pusher, so its commits are
    /// recorded as the owner's) answers `false`, and then only the owner
    /// and the owner's agents get write tokens.
    fn names_pushers(&self) -> bool {
        true
    }

    /// Every live token handed to a person or an agent session.
    fn held(&self) -> RemoteFuture<'_, Vec<Held>>;

    /// Revokes token `id` of `artifact`'s repository; answers whether a
    /// live token was revoked. A failure is retried by the host.
    fn revoke<'a>(&'a self, artifact: &'a ArtifactId, id: &'a str) -> RemoteFuture<'a, bool>;

    /// Revokes every live token handed to agent session `session`, or to
    /// person `person` (their own and their agents'); answers how many.
    fn revoke_held<'a>(
        &'a self,
        person: Option<&'a str>,
        session: Option<&'a str>,
    ) -> RemoteFuture<'a, usize>;
}

/// The host's [`GitRemotes`], once it installs one, and the revocations
/// waiting for [`settle`].
#[derive(Default)]
pub struct Slot {
    host: RwLock<Option<Arc<dyn GitRemotes>>>,
    /// Sessions that ended whose tokens are not yet revoked.
    ended: Mutex<BTreeSet<String>>,
    /// Whether every held token is to be rechecked ([`recheck`]).
    recheck_due: AtomicBool,
    /// One [`settle`] at a time: a settle returns only once what was due
    /// when it began is done, whoever began it.
    settling: tokio::sync::Mutex<()>,
}

impl Slot {
    /// Installs `remotes`, replacing any before.
    pub fn set(&self, remotes: Arc<dyn GitRemotes>) {
        *self.host.write().unwrap_or_else(|e| e.into_inner()) = Some(remotes);
    }

    /// The installed host; `None` on a daemon that serves no git.
    pub fn get(&self) -> Option<Arc<dyn GitRemotes>> {
        self.host.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Notes that session `sid` ended: its tokens are revoked at the next
    /// [`settle`] or [`Slot::revoke_ended`]. Nothing on a daemon that
    /// serves no git.
    pub fn session_ended(&self, sid: &str) {
        if self.get().is_some() {
            self.ended
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(sid.to_string());
        }
    }

    /// Asks the next [`settle`] to recheck every held token.
    pub fn recheck_soon(&self) {
        if self.get().is_some() {
            self.recheck_due.store(true, Ordering::Release);
        }
    }

    /// Whether a revocation or a recheck waits for [`settle`].
    pub fn due(&self) -> bool {
        self.get().is_some()
            && (self.recheck_due.load(Ordering::Acquire)
                || !self
                    .ended
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty())
    }

    /// Revokes the tokens of every ended session noted
    /// ([`Slot::session_ended`]); one whose revocation fails stays noted
    /// for the next try.
    pub async fn revoke_ended(&self) {
        let Some(host) = self.get() else {
            return;
        };
        let _turn = self.settling.lock().await;
        self.revoke_ended_now(&host).await;
    }

    /// Waits for this slot's turn to settle: one settle at a time, so one
    /// returns only once what was due when it began is done.
    pub async fn settling(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.settling.lock().await
    }

    /// Takes the request to recheck every held token
    /// ([`Slot::recheck_soon`]): whether one was made since the last take.
    pub fn take_recheck(&self) -> bool {
        self.recheck_due.swap(false, Ordering::AcqRel)
    }

    /// Revokes the tokens of every ended session noted, through `host`;
    /// the caller holds [`Slot::settling`]. One whose revocation fails
    /// stays noted for the next try.
    pub async fn revoke_ended_now(&self, host: &Arc<dyn GitRemotes>) {
        let sids: Vec<String> = self
            .ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        for sid in sids {
            match clax_rt::sendable(host.revoke_held(None, Some(&sid))).await {
                Ok(_) => {
                    self.ended
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&sid);
                }
                Err(e) => tracing::warn!(
                    session = sid.as_str(),
                    error = ?e,
                    "revoking an ended session's git tokens failed; it is tried again"
                ),
            }
        }
    }
}

/// Notes that agent session `sid` ended, so its tokens are revoked before
/// the request that ended it answers ([`settle_layer`]), and revokes them
/// on `handle` as well, for a path that is no request's (the reaper, the
/// grants' expiry): what ending a session does
/// ([`crate::feedback::sessions_stop`]), from whichever path ended it.
/// Nothing when `remotes` holds no host.
pub fn revoke_session_soon(remotes: &Arc<Slot>, handle: &clax_rt::Handle, sid: &str) {
    if remotes.get().is_none() {
        return;
    }
    remotes.session_ended(sid);
    let slot = remotes.clone();
    handle.spawn(async move { slot.revoke_ended().await });
}

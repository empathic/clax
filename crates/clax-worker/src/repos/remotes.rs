//! The Worker's [`GitRemotes`]: repository tokens from `RepoDO`'s ledger
//! ([`DoTokens`] over its internal API), for the server's git token routes
//! and the recheck that revokes them (`clax_server::git_remotes`).
//! `RepoDO` mints, lists and revokes for either repository host (the
//! stand-in, Cloudflare Artifacts), so one implementation serves both.
//!
//! Artifact `<id>`'s repository is `a-<id>`
//! (`clax_git::versions::repo_name`); a ledger row of any other repository
//! is not an artifact's and is left out of [`GitRemotes::held`].

use clax_core::ArtifactId;
use clax_git::GitError;
use clax_git::http::Transport;
use clax_git::versions::repo_name;
use clax_server::git_remotes::{
    GitRemotes, Held, Holder, Minted, RemoteError, RemoteFuture, Scope,
};

use super::api::DoTokens;
use super::tokens;

/// [`GitRemotes`] over `RepoDO`'s ledger, reached through `tokens`'
/// transport (a stub's `fetch` in a Worker).
pub struct WorkerRemotes<T> {
    pub tokens: DoTokens<T>,
    /// Whether the repositories' host credits a push to its token's
    /// holder ([`GitRemotes::names_pushers`]).
    pub names_pushers: bool,
}

fn remote_error(e: GitError) -> RemoteError {
    match e {
        GitError::NotFound(m) => RemoteError::NotFound(m),
        e => RemoteError::Unavailable(e.to_string()),
    }
}

fn scope_of(s: Scope) -> tokens::Scope {
    match s {
        Scope::Read => tokens::Scope::Read,
        Scope::Write => tokens::Scope::Write,
    }
}

/// The artifact repository `repo` is, when it is one.
fn artifact_of(repo: &str) -> Option<ArtifactId> {
    repo.strip_prefix("a-")
        .and_then(|id| ArtifactId::parse(id).ok())
}

/// A ledger row as the server lists it; `None` for a repository that is no
/// artifact's or a scope the server does not know.
fn held_of(t: tokens::HeldToken) -> Option<Held> {
    Some(Held {
        artifact: artifact_of(&t.repo)?,
        scope: Scope::parse(&t.scope)?,
        id: t.id,
        holder: Holder {
            person: t.person,
            session: t.session,
        },
    })
}

impl<T> GitRemotes for WorkerRemotes<T>
where
    T: Transport + Send + Sync + 'static,
{
    fn mint<'a>(
        &'a self,
        artifact: &'a ArtifactId,
        scope: Scope,
        ttl_s: u32,
        origin: Option<&'a str>,
        holder: &'a Holder,
    ) -> RemoteFuture<'a, Minted> {
        Box::pin(async move {
            let holder = tokens::Holder {
                person: holder.person.clone(),
                session: holder.session.clone(),
            };
            let t = self
                .tokens
                .mint(
                    &repo_name(artifact.as_str()),
                    scope_of(scope),
                    ttl_s,
                    origin,
                    &holder,
                )
                .await
                .map_err(remote_error)?;
            Ok(Minted {
                id: t.id,
                remote: t.remote,
                token: t.token,
                expires_at: t.expires_at,
                clone_command: t.clone_command,
                jj_clone_command: t.jj_clone_command,
            })
        })
    }

    fn names_pushers(&self) -> bool {
        self.names_pushers
    }

    fn held(&self) -> RemoteFuture<'_, Vec<Held>> {
        Box::pin(async move {
            let rows = self.tokens.held().await.map_err(remote_error)?;
            Ok(rows.into_iter().filter_map(held_of).collect())
        })
    }

    fn revoke<'a>(&'a self, artifact: &'a ArtifactId, id: &'a str) -> RemoteFuture<'a, bool> {
        Box::pin(async move {
            self.tokens
                .revoke(&repo_name(artifact.as_str()), id)
                .await
                .map_err(remote_error)
        })
    }

    fn revoke_held<'a>(
        &'a self,
        person: Option<&'a str>,
        session: Option<&'a str>,
    ) -> RemoteFuture<'a, usize> {
        Box::pin(async move {
            self.tokens
                .revoke_held(person, session)
                .await
                .map_err(remote_error)
        })
    }
}

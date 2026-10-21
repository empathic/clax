//! Git remotes for artifacts (spec 2026-10-11-clax-dev-hosted-design §9
//! "Tokens"): a short-lived repository token, handed with the remote and
//! the commands that clone, fetch and push with it, to a signed-in person
//! (`POST /api/artifacts/{aid}/git-token`) or to an agent session
//! (`POST /api/sessions/{id}/git-token/{aid}`, the MCP `git_remote` tool).
//!
//! The host serves the repositories and mints the tokens ([`GitRemotes`],
//! installed with [`Slot::set`]): the Worker does, over `RepoDO`'s ledger
//! (the stand-in) or Cloudflare Artifacts. The native daemon keeps
//! versions as files, not in git, and installs none: every route answers
//! 501 `not_available` there.
//!
//! # Rights
//!
//! The prototype's roles map onto multi-person's rights
//! ([`clax_core::rights::Rights::allows`]) at the artifact:
//!
//! - a `read` token (clone and fetch) needs `clax-artifacts` `read`, as the
//!   viewer role's clone does: a person's rights, or, on a hosted daemon,
//!   the link of a page shared by link, which anyone who reads the page
//!   through it holds (a guest, or a visitor who gave no name), as anyone
//!   who can view a repository may clone it;
//! - a `write` token (push as well) needs `clax-artifacts` `update`, the
//!   right a publish needs: a commit pushed to `main` becomes a version
//!   published by whom the token was handed to ([`Holder`]), so pushing is
//!   publishing. Under `clax:collaborator` that is the person's own
//!   artifacts, as the owner role's push is in the prototype. A host that
//!   cannot say who pushed ([`GitRemotes::names_pushers`]: Cloudflare
//!   Artifacts) records every push as the owner's, so there only the owner
//!   and the owner's agents get one (403 `push_unattributed`), and the
//!   Clone dialog offers no other a write token.
//!
//! A fork's repository carries its source's history up to the forked
//! version (and so on up the lineage), so a token of a fork also needs
//! `read` at every artifact up its lineage, deleted ones included.
//!
//! Tokens go to people and their agents: the owner (browser or CLI), a
//! connected person or a hosted daemon's member (their rights, and the
//! artifact's link's), and agent sessions (the owner's, or a person's
//! within their grant's rights); and, on a hosted daemon, read tokens
//! living [`LINK_TTL_S`] to whoever reads a page through its link
//! ([`Asker::Link`]: a guest, or a request with no session), for that
//! artifact alone. A link never gets a write token, and a link to a fork
//! opens its source only where the source is shared by link too. A LAN
//! viewer who is not a person gets none, and neither does a spectator (a
//! person or guest beyond the live seats, read-only there).
//!
//! An agent's token also needs its person to see every author of the
//! repository's history at the artifact (the visibility filter): a clone
//! carries every version, and a token cannot filter it. The history is the
//! artifact's versions, a fork's source's versions up to the forked one
//! (and so on up its lineage, deleted artifacts included), and the pushers
//! of commits left out of a version, which stay on `main`
//! ([`Store::repo_history`]). Otherwise the route answers 403 `withheld`
//! naming the withheld authors (none named when an artifact up the
//! lineage is gone, so the authors cannot all be), and while the
//! repository holds pushed commits not yet recorded as versions (whose
//! authors are not known yet) 503 `repo_unavailable`.
//!
//! # Lifetimes and revocation
//!
//! People's tokens live [`PERSON_TTL_S`], agents' [`AGENT_TTL_S`]. The
//! host keeps whom each token was handed to, and revokes them by holder
//! ([`GitRemotes::revoke_held`], over the same revocation, and must-revoke
//! list, as every other):
//!
//! - when a session ends, by whichever path ([`revoke_session_soon`],
//!   from [`clax_realtime::feedback::sessions_stop`]);
//! - when a person is revoked or their rights narrowed, their tokens and
//!   their agents' ([`rights_changed`], from the people routes), and when
//!   a grant is narrowed, its sessions' (the narrowing route).
//!
//! Each of these is done before the request that caused it answers: the
//! router's [`settle_layer`] revokes the tokens of the sessions the
//! request ended, and when the request changed what agents may see (an
//! approval, rights, a grant, a person) rechecks every held token, before
//! the response leaves; a hosted page's link narrowed rechecks that
//! artifact's tokens in its route. On a Durable Object an eviction cannot
//! come between a request and its answer, so none of this is lost to one.
//! A revocation the host fails stays due for the next settle (the sweep's,
//! from the alarm on the Worker, which [`AppState::next_sweep_in`] sets
//! while one is due), and a server built anew rechecks every held token
//! first.
//!
//! Before a pushed commit is recorded, and before a publish writes its
//! commit, the store revokes the agents' tokens that may not see it
//! (`clax_store::pushes`); a publish whose revocation fails is
//! refused, 503 `repo_unavailable`, with nothing written. Beside these,
//! [`listen`] rechecks every live held token whenever a version lands,
//! people, rights or grants change, a session starts or ends, an artifact
//! is deleted, or what agents may see changes ([`recheck`]), revoking what
//! the paths above did not reach: a published version an agent's person
//! may not see, an approval withdrawn, rights at one artifact dropped.

use crate::error::ApiError;
use crate::hosted::MemberRights;
use crate::state::AppState;
use axum::http::StatusCode;
use clax_core::rights::Rights;
use clax_core::visibility::{self, Author, Reader};
use clax_core::{ArtifactId, CoreError};
use clax_store::Store;
use clax_store::visibility::Visibility;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Lifetime of a person's token, in seconds.
pub const PERSON_TTL_S: u32 = 3600;
/// Lifetime of an agent's token, in seconds: shorter than a person's, as
/// it bounds what a missed revocation exposes.
pub const AGENT_TTL_S: u32 = 900;
/// Lifetime of a read token handed to a page's link ([`Asker::Link`]), in
/// seconds: as short as an agent's, as it names no one.
pub const LINK_TTL_S: u32 = 900;
/// How soon a sweep tries again a revocation or recheck the host failed.
pub const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
/// The name the commands for an existing checkout give the remote.
pub const REMOTE_NAME: &str = "clax";

pub use clax_realtime::git_remotes::{
    GitRemotes, Held, Holder, Minted, RemoteError, RemoteFuture, Scope, Slot, revoke_session_soon,
};

impl From<RemoteError> for ApiError {
    fn from(e: RemoteError) -> ApiError {
        match e {
            RemoteError::NotFound(_) => ApiError::not_found(),
            RemoteError::Unavailable(m) => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "repo_unavailable", m)
            }
        }
    }
}

/// Does what [`Slot`] holds due: revokes the ended sessions' tokens, then,
/// when asked, rechecks every held token ([`recheck`]). What fails stays
/// due for the next settle (the sweep's, the next request's, the
/// listener's). Nothing on a daemon that serves no git.
pub async fn settle(s: &AppState) {
    let slot = &s.git_remotes;
    let Some(host) = slot.get() else {
        return;
    };
    let _turn = slot.settling().await;
    slot.revoke_ended_now(&host).await;
    if slot.take_recheck()
        && let Err(e) = recheck(s, None).await
    {
        slot.recheck_soon();
        tracing::warn!(code = e.code, error = %e.message, "rechecking git tokens failed; it is tried again");
    }
}

/// The router's layer that has every request's revocations done before it
/// answers: an ended session's tokens ([`Slot::session_ended`]), and, when
/// the request changed what agents may see (the visibility generation
/// moved: an approval, a person's rights, a grant narrowed or ended, a
/// person made or revoked), a recheck of every held token. So a route that
/// narrows rights or ends a session answers only once the tokens it took
/// away are revoked, or noted for the next try when the host failed.
pub async fn settle_layer(
    axum::extract::State(s): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let seen = s.vis_gen.current();
    let res = next.run(req).await;
    if s.git_remotes.get().is_some() {
        if s.vis_gen.current() != seen {
            s.git_remotes.recheck_soon();
        }
        if s.git_remotes.due() {
            settle(&s).await;
        }
    }
    res
}

/// 501 `not_available`: this daemon serves no git remotes.
pub fn not_available() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "not_available",
        "this Clax serves no git remotes: artifacts are kept as files here, not in git",
    )
}

fn host_of(s: &AppState) -> Result<Arc<dyn GitRemotes>, ApiError> {
    s.git_remotes.get().ok_or_else(not_available)
}

// ── Who may have a token ────────────────────────────────────────────────────

/// Who asks for a token, as a route resolved them.
#[derive(Clone, Debug)]
pub enum Asker {
    /// The owner: every right.
    Owner,
    /// A connected person or a hosted daemon's member, by public ID.
    Person(String),
    /// The owner's agent from session `session`.
    OwnerAgent { session: String },
    /// A person's agent (`person` `None`: a grant of the owner's row) from
    /// session `session`, within `rights`.
    PersonAgent {
        person: Option<String>,
        session: String,
        rights: Arc<Rights>,
    },
    /// Whoever reads the artifact through the link of a page shared by
    /// link, on a hosted daemon: a guest, or a request with no session.
    /// Read alone, at the artifact and up its lineage, while each is
    /// shared by link.
    Link,
}

impl Asker {
    pub fn holder(&self) -> Holder {
        match self {
            Asker::Owner => Holder::default(),
            Asker::Person(p) => Holder {
                person: Some(p.clone()),
                ..Holder::default()
            },
            Asker::OwnerAgent { session } => Holder {
                session: Some(session.clone()),
                ..Holder::default()
            },
            Asker::PersonAgent {
                person, session, ..
            } => Holder {
                person: person.clone(),
                session: Some(session.clone()),
                ..Holder::default()
            },
            Asker::Link => Holder {
                link: true,
                ..Holder::default()
            },
        }
    }

    pub fn reader(&self) -> Option<Reader> {
        match self {
            Asker::Owner | Asker::Person(_) | Asker::Link => None,
            Asker::OwnerAgent { .. } | Asker::PersonAgent { person: None, .. } => {
                Some(Reader::Owner)
            }
            Asker::PersonAgent {
                person: Some(p), ..
            } => Some(Reader::Person(p.clone())),
        }
    }

    pub fn ttl_s(&self) -> u32 {
        match self {
            Asker::Owner | Asker::Person(_) => PERSON_TTL_S,
            Asker::OwnerAgent { .. } | Asker::PersonAgent { .. } => AGENT_TTL_S,
            Asker::Link => LINK_TTL_S,
        }
    }
}

/// Whether `asker`'s rights allow a token of `scope` at `aid`, and read
/// at every artifact up its lineage, read on `st` now.
fn allows(st: &Store, asker: &Asker, scope: Scope, aid: &str) -> clax_core::Result<bool> {
    if matches!(asker, Asker::Owner | Asker::OwnerAgent { .. }) {
        return Ok(true);
    }
    if !allows_one(st, asker, scope, aid)? {
        return Ok(false);
    }
    // A clone carries a fork's lineage's history up to each forked version:
    // reading it needs read at every artifact up the lineage, deleted ones
    // included. A lineage that cannot be followed allows a person nothing;
    // an agent's history check refuses it, `withheld`.
    let Some(history) = st.repo_history(&ArtifactId::parse(aid)?)? else {
        return Ok(matches!(asker, Asker::PersonAgent { .. }));
    };
    let up: BTreeSet<&str> = history
        .versions
        .iter()
        .map(|v| v.artifact_id.as_str())
        .filter(|a| *a != aid)
        .collect();
    for a in up {
        if !allows_one(st, asker, Scope::Read, a)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// [`allows`] at `aid` alone.
fn allows_one(st: &Store, asker: &Asker, scope: Scope, aid: &str) -> clax_core::Result<bool> {
    let (kind, action) = scope.right();
    match asker {
        Asker::Owner | Asker::OwnerAgent { .. } => Ok(true),
        Asker::Person(p) => person_allows(st, p, scope, aid),
        Asker::PersonAgent { person, rights, .. } => {
            let own = |x: &str| {
                st.artifact_created_by(x, person.as_deref())
                    .unwrap_or(false)
            };
            Ok(rights.allows(kind, action, Some(aid), &own))
        }
        Asker::Link => link_allows(st, scope, aid),
    }
}

/// Whether the link of `aid` (shared by link) allows `scope`: read alone.
fn link_allows(st: &Store, scope: Scope, aid: &str) -> clax_core::Result<bool> {
    if scope != Scope::Read {
        return Ok(false);
    }
    let (kind, action) = scope.right();
    let none = |_: &str| false;
    Ok(st
        .link_access(aid)?
        .is_some_and(|a| a.rights(aid).allows(kind, action, Some(aid), &none)))
}

/// Whether person `p`'s rights, or the artifact's link's, allow `scope` at
/// `aid`; a person who is no longer one (revoked) is allowed nothing.
fn person_allows(st: &Store, p: &str, scope: Scope, aid: &str) -> clax_core::Result<bool> {
    // A revoked person keeps their row, and their rights in it: they are a
    // plain viewer from then on.
    if st
        .viewer_by_public_id(p)?
        .is_none_or(|v| v.person_revoked_at.is_some())
    {
        return Ok(false);
    }
    let Ok(rights) = st.person_rights(p) else {
        return Ok(false);
    };
    MemberRights {
        person: p.to_string(),
        rights: Arc::new(rights),
    }
    .allows_at(st, scope.right(), aid)
}

/// The authors of `aid`'s repository history that agents of `reader` may
/// not see there, named as views name them, each once: every version's
/// label, note and files the history carries, a fork's lineage's up to
/// each forked version included ([`Store::repo_history`]), and the pushers
/// of commits left out of a version, which stay on `main`. `None` when the
/// history's authors cannot all be named (an artifact up the lineage is
/// gone from the store).
fn withheld_history(
    st: &Store,
    aid: &ArtifactId,
    reader: &Reader,
    generation: u64,
) -> clax_core::Result<Option<Vec<visibility::placeholder::WithheldAuthor>>> {
    let Some(history) = st.repo_history(aid)? else {
        return Ok(None);
    };
    let vis = Visibility::for_reader(st, reader, generation)?;
    let mut fields: Vec<Vec<Author>> = Vec::new();
    for v in &history.versions {
        fields.push(visibility::authors_of_version(v));
        fields.extend(v.files.keys().map(|p| visibility::authors_of_file(v, p)));
    }
    fields.extend(
        history
            .refused_pushers
            .iter()
            .map(|p| vec![Author::of_token(p.as_deref())]),
    );
    let mut seen: Vec<Author> = Vec::new();
    for f in fields {
        for a in vis.withheld_authors(&f, aid.as_str(), None) {
            if !seen.contains(&a) {
                seen.push(a);
            }
        }
    }
    seen.iter()
        .map(|a| named(st, &vis, a))
        .collect::<clax_core::Result<_>>()
        .map(Some)
}

/// `a` as views list a withheld author: public ID (none for anonymous
/// viewers) and current display name.
fn named(
    st: &Store,
    vis: &Visibility,
    a: &Author,
) -> clax_core::Result<visibility::placeholder::WithheldAuthor> {
    let viewer = match a {
        Author::Owner => st.owner()?,
        Author::Person(p) => st.viewer_by_public_id(p)?,
        Author::Unconnected | Author::System => None,
    };
    Ok(visibility::placeholder::WithheldAuthor {
        public_id: match a {
            Author::Person(p) => Some(p.clone()),
            _ => viewer.as_ref().map(|v| v.public_id.clone()),
        },
        name: vis.label(
            &clax_core::feedback::display_name(
                viewer
                    .as_ref()
                    .and_then(|v| v.display_name.as_deref())
                    .unwrap_or(""),
            ),
            a,
        ),
    })
}

/// 403 `withheld` with no authors named: the history holds work whose
/// authors can no longer all be named (an artifact it was forked from is
/// gone from the store), so a clone could carry work the agent's person
/// may not see.
fn withheld_unknown() -> ApiError {
    let mut e = ApiError::forbidden(
        "withheld",
        "the repository's history holds work whose authors can no longer all be named: a clone could carry work your person has not approved",
    );
    e.extra.insert("withheld_authors".into(), json!([]));
    e
}

/// 403 `withheld`: the history holds work by `authors`, whom the agent's
/// person may not see here.
fn withheld(authors: &[visibility::placeholder::WithheldAuthor]) -> ApiError {
    let names: Vec<&str> = authors.iter().map(|a| a.name.as_str()).collect();
    let mut e = ApiError::forbidden(
        "withheld",
        format!(
            "the repository's history holds work by {}, which your person has not approved on this artifact: a clone would carry it",
            names.join(", ")
        ),
    );
    e.extra.insert("withheld_authors".into(), json!(authors));
    e
}

/// Refuses an agent's token while its person may not see every author of
/// `aid`'s history, or while the repository holds pushed commits not yet
/// recorded (whose authors are not known yet).
async fn check_history(s: &AppState, asker: &Asker, aid: &ArtifactId) -> Result<(), ApiError> {
    let Some(reader) = asker.reader() else {
        return Ok(());
    };
    let generation = s.vis_gen.current();
    let id = aid.clone();
    s.store_call_async(move |st| async move {
            let Some(a) = st.get_artifact(&id)? else {
                return Err(CoreError::NotFound);
            };
            if !st
                .content()
                .pushed_commits(&id, a.current_version)
                .await?
                .is_empty()
            {
                return Ok(Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "repo_unavailable",
                    "the repository holds pushed commits not yet recorded as versions: try again shortly",
                )));
            }
            Ok(match withheld_history(&st, &id, &reader, generation)? {
                None => Err(withheld_unknown()),
                Some(w) if w.is_empty() => Ok(()),
                Some(w) => Err(withheld(&w)),
            })
        })
        .await?
}

// ── Answers ─────────────────────────────────────────────────────────────────

fn remote_safe(r: &str) -> bool {
    let rest = r
        .strip_prefix("https://")
        .or_else(|| r.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._~:/@%+=-".contains(&b))
    })
}

fn token_safe(t: &str) -> bool {
    !t.is_empty()
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~:/?@%+=&,-".contains(&b))
}

/// The commands for a checkout that already exists: add the remote
/// ([`REMOTE_NAME`]) and fetch from it, with git and with jj, and for a
/// write token push to its `main`. The token travels as git's
/// `http.extraHeader` on each command (`-c`, or the environment for jj),
/// never in the remote's URL or a configuration file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Checkout {
    pub remote_name: &'static str,
    pub add_remote_command: String,
    pub fetch_command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_command: Option<String>,
    pub jj_add_remote_command: String,
    pub jj_fetch_command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jj_push_command: Option<String>,
}

/// [`Checkout`] for `remote` and `token`; `None` when either holds what a
/// shell word cannot carry.
pub fn checkout(remote: &str, token: &str, scope: Scope) -> Option<Checkout> {
    if !remote_safe(remote) || !token_safe(token) {
        return None;
    }
    let header = format!("Authorization: Bearer {token}");
    let git = format!("git -c http.extraHeader=\"{header}\"");
    let jj = format!(
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0=\"{header}\" jj"
    );
    let write = scope == Scope::Write;
    Some(Checkout {
        remote_name: REMOTE_NAME,
        add_remote_command: format!("git remote add {REMOTE_NAME} {remote}"),
        fetch_command: format!("{git} fetch {REMOTE_NAME}"),
        push_command: write.then(|| format!("{git} push {REMOTE_NAME} HEAD:main")),
        jj_add_remote_command: format!("jj git remote add {REMOTE_NAME} {remote}"),
        jj_fetch_command: format!("{jj} git fetch --remote {REMOTE_NAME}"),
        jj_push_command: write
            .then(|| format!("{jj} git push --remote {REMOTE_NAME} --bookmark main")),
    })
}

/// What a token request answers.
fn answer(aid: &ArtifactId, scope: Scope, m: Minted) -> Result<Value, ApiError> {
    let existing = checkout(&m.remote, &m.token, scope).ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "repo_unavailable",
            "the repository's remote or token holds characters a shell command cannot carry",
        )
    })?;
    Ok(json!({
        "artifact_id": aid.as_str(),
        "scope": scope.as_str(),
        "remote": m.remote,
        "token": m.token,
        "expires_at": m.expires_at,
        "clone_command": m.clone_command,
        "jj_clone_command": m.jj_clone_command,
        "existing_checkout": existing,
    }))
}

/// 403 `not_permitted`: `scope`'s right is not the asker's here.
pub fn not_permitted(scope: Scope) -> ApiError {
    let (kind, action) = scope.right();
    let mut e = ApiError::forbidden(
        "not_permitted",
        format!(
            "your rights here do not let you {} ({} {action})",
            match scope {
                Scope::Read => "clone this artifact",
                Scope::Write => "push to this artifact",
            },
            kind.as_str()
        ),
    );
    e.extra.insert(
        "right".into(),
        json!({"type": kind.as_str(), "action": action}),
    );
    e
}

/// Checks `asker` may have a token of `scope` on `aid`, mints it and checks
/// again once minted, revoking it when it is now refused: the holder's
/// rights as the store holds them then ([`still_allowed`]: rights may
/// have narrowed, a session ended, meanwhile) and, for an agent, the
/// history (a version may have landed). Answers
/// `{artifact_id, scope, remote, token, expires_at, clone_command,
/// jj_clone_command, existing_checkout}` ([`Checkout`]).
///
/// # Errors
/// 501 `not_available` on a daemon that serves no git; 404 for an
/// artifact that does not exist; 400 `live_page`; 403 `not_permitted`
/// without the scope's right; for an agent 403 `withheld` and 503
/// `repo_unavailable` (see the module docs); the host's.
pub async fn mint_for(
    s: &AppState,
    asker: Asker,
    aid: ArtifactId,
    scope: Scope,
    origin: Option<String>,
) -> Result<Value, ApiError> {
    let host = host_of(s)?;
    if s.live_ids.contains(aid.as_str()) {
        return Err(ApiError::bad_request(
            "live_page",
            "a live page's snapshots are not kept in git: it has no remote",
        ));
    }
    let (a, id) = (asker.clone(), aid.clone());
    let allowed = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            allows(st, &a, scope, id.as_str())
        })
        .await?;
    if !allowed {
        return Err(not_permitted(scope));
    }
    if !credited(host.as_ref(), &asker, scope) {
        return Err(push_unattributed());
    }
    check_history(s, &asker, &aid).await?;
    let holder = asker.holder();
    let minted =
        clax_rt::sendable(host.mint(&aid, scope, asker.ttl_s(), origin.as_deref(), &holder))
            .await?;
    let refused = match still_allowed(s, holder, scope, &aid).await {
        Ok(()) => check_history(s, &asker, &aid).await,
        Err(e) => Err(e),
    };
    if let Err(e) = refused {
        let _ = clax_rt::sendable(host.revoke(&aid, &minted.id)).await;
        return Err(e);
    }
    answer(&aid, scope, minted)
}

/// Whether `holder` may still have a token of `scope` at `aid`, read on the
/// store once the host minted it, as [`recheck`] would judge it: the
/// artifact still exists, the holder still may hold a token (the session
/// live, the grant not ended, the person not revoked) and their rights now
/// allow the scope. A request that narrowed rights or ended a session
/// while the host minted rechecked or revoked only the tokens the host
/// listed then, which this one was not yet among; its write is in the
/// store before this read, so the token is refused here.
async fn still_allowed(
    s: &AppState,
    holder: Holder,
    scope: Scope,
    aid: &ArtifactId,
) -> Result<(), ApiError> {
    let id = aid.clone();
    s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        Ok(match asker_now(st, &holder)? {
            Some(now) if allows(st, &now, scope, id.as_str())? => Ok(()),
            _ => Err(not_permitted(scope)),
        })
    })
    .await?
}

/// The scopes `asker`'s rights allow at `aid`, read first: what the Clone
/// dialog offers.
///
/// # Errors
/// The store's.
pub async fn scopes_for(
    s: &AppState,
    asker: &Asker,
    aid: &ArtifactId,
) -> Result<Vec<Scope>, ApiError> {
    let (a, id) = (asker.clone(), aid.clone());
    let host = s.git_remotes.get();
    s.store_call(move |st| {
        let mut out = Vec::new();
        for scope in [Scope::Read, Scope::Write] {
            let credited = host.as_deref().is_none_or(|h| credited(h, &a, scope));
            if credited && allows(st, &a, scope, id.as_str())? {
                out.push(scope);
            }
        }
        Ok(out)
    })
    .await
}

/// Whether a push with a token of `scope` handed to `asker` would be
/// credited to them: always but for a write token from a host that names
/// no pusher ([`GitRemotes::names_pushers`]), whose pushes are the
/// owner's, which only the owner and the owner's agents get.
fn credited(host: &dyn GitRemotes, asker: &Asker, scope: Scope) -> bool {
    scope == Scope::Read
        || host.names_pushers()
        || matches!(asker, Asker::Owner | Asker::OwnerAgent { .. })
}

/// 403 `push_unattributed`: see [`credited`].
fn push_unattributed() -> ApiError {
    ApiError::forbidden(
        "push_unattributed",
        "this instance's repositories cannot say who pushed, so only the owner gets write tokens; clone with a read token and publish your changes",
    )
}

// ── Revocation ──────────────────────────────────────────────────────────────

/// Whom a held token's holder is now, read on `st`: `None` once they may
/// hold no token (the session ended or is gone, the grant ended).
fn asker_now(st: &Store, h: &Holder) -> clax_core::Result<Option<Asker>> {
    if h.link {
        return Ok(Some(Asker::Link));
    }
    match (&h.session, &h.person) {
        (Some(sid), _) => {
            let Some(sess) = st.get_session(sid)? else {
                return Ok(None);
            };
            if sess.ended_at.is_some() {
                return Ok(None);
            }
            match &sess.grant_id {
                None => Ok(Some(Asker::OwnerAgent {
                    session: sid.clone(),
                })),
                Some(g) => match st.grant_caller(g, Some(sid)) {
                    Ok(gc) => Ok(Some(Asker::PersonAgent {
                        person: gc.person,
                        session: sid.clone(),
                        rights: Arc::new(gc.rights),
                    })),
                    Err(_) => Ok(None),
                },
            }
        }
        (None, Some(p)) => Ok(Some(Asker::Person(p.clone()))),
        (None, None) => Ok(Some(Asker::Owner)),
    }
}

/// Whether the held token `t` should be revoked now (see the module docs).
fn stale(st: &Store, t: &Held, generation: u64) -> clax_core::Result<bool> {
    let Some(asker) = asker_now(st, &t.holder)? else {
        return Ok(true);
    };
    if st.get_artifact(&t.artifact)?.is_none() {
        return Ok(true);
    }
    if !allows(st, &asker, t.scope, t.artifact.as_str())? {
        return Ok(true);
    }
    Ok(match asker.reader() {
        Some(reader) => {
            withheld_history(st, &t.artifact, &reader, generation)?.is_none_or(|w| !w.is_empty())
        }
        None => false,
    })
}

/// Rechecks every live token the host lists as handed to a person or an
/// agent (`only`: those of that artifact) and revokes each that its
/// holder may no longer have (see the module docs). Answers how many were
/// revoked; nothing on a daemon that serves no git.
///
/// # Errors
/// The host's, when it cannot list its tokens; the store's.
pub async fn recheck(s: &AppState, only: Option<&ArtifactId>) -> Result<usize, ApiError> {
    let Some(host) = s.git_remotes.get() else {
        return Ok(0);
    };
    let held: Vec<Held> = clax_rt::sendable(host.held())
        .await?
        .into_iter()
        .filter(|t| only.is_none_or(|a| &t.artifact == a))
        .collect();
    if held.is_empty() {
        return Ok(0);
    }
    let generation = s.vis_gen.current();
    let stale_ids: Vec<(ArtifactId, String)> = s
        .store_call(move |st| {
            let mut out = Vec::new();
            for t in held {
                if stale(st, &t, generation)? {
                    out.push((t.artifact, t.id));
                }
            }
            Ok(out)
        })
        .await?;
    let mut revoked = 0;
    for (aid, id) in stale_ids {
        if clax_rt::sendable(host.revoke(&aid, &id)).await? {
            revoked += 1;
        }
    }
    Ok(revoked)
}

/// Revokes every live token handed to agent session `session`, or to
/// person `person` (their own and their agents'); nothing on a daemon that
/// serves no git. Answers how many.
///
/// # Errors
/// The host's.
pub async fn revoke_holder(
    s: &AppState,
    person: Option<&str>,
    session: Option<&str>,
) -> Result<usize, ApiError> {
    match s.git_remotes.get() {
        None => Ok(0),
        Some(host) => Ok(clax_rt::sendable(host.revoke_held(person, session)).await?),
    }
}

/// Revokes person `person`'s tokens and their agents' once their rights
/// went from `before` to `after`, when `after` no longer allows all that
/// `before` did (a narrowing, or a revocation: `after` `None`). The owner
/// may hand them new ones.
///
/// # Errors
/// The host's.
pub async fn rights_changed(
    s: &AppState,
    person: &str,
    before: Option<&Rights>,
    after: Option<&Rights>,
) -> Result<usize, ApiError> {
    let narrowed = match (before, after) {
        (Some(b), Some(a)) => !b.is_narrower_or_equal(a),
        (_, None) => true,
        (None, Some(_)) => false,
    };
    if !narrowed {
        return Ok(0);
    }
    revoke_holder(s, Some(person), None).await
}

/// Whether `e` may change who may hold a token: a version landing,
/// people, rights or grants changing, a session starting or ending, an
/// artifact deleted.
fn relevant(e: &clax_core::Event) -> bool {
    use clax_core::Event;
    matches!(
        e,
        Event::Version { .. }
            | Event::ArtifactDeleted { .. }
            | Event::People { .. }
            | Event::Agents { .. }
    )
}

/// Rechecks the live tokens ([`recheck`]) after every event that may
/// change who may hold one and every change to what agents may see, one
/// recheck for each burst. Does nothing on a daemon that serves no git.
/// Returns the rechecking task's handle.
pub fn listen(s: &AppState) -> clax_rt::JoinHandle<()> {
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};
    let mut rx = s.events.subscribe();
    let st = s.clone();
    clax_rt::spawn(async move {
        let mut seen = st.vis_gen.current();
        // A server built anew (a Durable Object after an eviction) first
        // revokes what a lost revocation left live: tokens of sessions that
        // ended, rights that narrowed, while it was away.
        st.git_remotes.recheck_soon();
        settle(&st).await;
        loop {
            tokio::select! {
                r = rx.recv() => match r {
                    Ok(e) if !relevant(&e.event) => continue,
                    Err(RecvError::Closed) => return,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                },
                _ = st.vis_gen.wait_after(seen) => {}
            }
            // One recheck covers what queued meanwhile.
            loop {
                match rx.try_recv() {
                    Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Closed) => return,
                }
            }
            seen = st.vis_gen.current();
            st.git_remotes.recheck_soon();
            settle(&st).await;
        }
    })
}

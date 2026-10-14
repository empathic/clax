//! Repository tokens: short-lived, scoped to one repository, `read`
//! (clone and fetch) or `write` (push as well), handed to a person or an
//! agent with the remote and the commands that clone it.
//!
//! `RepoDO` mints, revokes and keeps the ledger of every token
//! ([`super::db`]'s `git_tokens`), whichever host the repositories are in:
//!
//! - **Stand-in** (`REPO_STORE=memory`): a token is `cxm_<scope>_<43
//!   base64url characters>` (32 random bytes), named by an ID
//!   `cxt_<16 base64url characters>`; the ledger keeps its SHA-256, never
//!   the plaintext, and the `/git/` route takes it as `Authorization:
//!   Bearer <token>` while it is live (not revoked, not expired) for the
//!   repository it names, and the repository exists.
//! - **Cloudflare Artifacts**: the repository handle's `createToken` and
//!   `revokeToken`; Artifacts checks the token on its own git remote.
//!
//! A revocation enters the token in the must-revoke list
//! (`git_tokens_due`) before the host is asked and leaves it once revoked,
//! so one that fails is tried again ([`retry_due`], from `RepoDO`'s alarm)
//! until it is revoked or expires.
//!
//! Who may have a token is the server's to decide, before it asks for one:
//! a `read` token needs the clone right, a `write` token the push right,
//! and an agent's read token also that its person sees every author of the
//! repository's history (a token cannot filter what a clone carries).

use std::future::Future;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clax_git::server::Access;
use clax_git::{GitError, MaybeSend, MaybeSendSync};
use sha2::{Digest, Sha256};

use super::db;
use super::local::{AsConn, LocalHost};

/// Lifetime of a person's token, in seconds.
pub const PERSON_TOKEN_TTL_S: u32 = 3600;
/// Lifetime of an agent's token, in seconds: shorter than a person's, as
/// it bounds what a missed revocation exposes.
pub const AGENT_TOKEN_TTL_S: u32 = 900;
/// The shortest and longest lifetimes a token may be asked for (Cloudflare
/// Artifacts' bounds).
pub const MIN_TTL_S: u32 = 60;
pub const MAX_TTL_S: u32 = 31_536_000;

/// What a token allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Read,
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

    pub fn access(self) -> Access {
        match self {
            Scope::Read => Access::Read,
            Scope::Write => Access::Write,
        }
    }
}

/// Whom a token is handed to, as the server knows them when it asks for
/// one: a connected person's public ID (`None` for the owner) and, for an
/// agent's token, its session. A push with the token is theirs: its
/// commits become versions they published.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Holder {
    pub person: Option<String>,
    pub session: Option<String>,
}

/// A token as a host mints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Minted {
    /// Names the token to [`TokenHost::revoke`].
    pub id: String,
    /// The secret, shown once.
    pub token: String,
    /// Milliseconds since the epoch.
    pub expires_ms: i64,
    /// The SHA-256 of `token`, when this store checks it itself.
    pub hash: Option<[u8; 32]>,
}

/// What a person or agent is handed: the remote, the token, when it
/// expires, and the commands that clone with it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitToken {
    pub id: String,
    pub remote: String,
    pub token: String,
    /// RFC 3339, UTC.
    pub expires_at: String,
    /// `git -c http.extraHeader="Authorization: Bearer <token>" clone <remote>`
    pub clone_command: String,
    /// The same through `jj git clone`, the header in the environment's git
    /// configuration.
    pub jj_clone_command: String,
}

/// Where tokens are minted and revoked: the stand-in's own ledger, or
/// Cloudflare Artifacts.
pub trait TokenHost: MaybeSendSync {
    /// A token for `repo`, valid for `ttl_s` seconds from `now_ms`.
    fn mint(
        &self,
        repo: &str,
        scope: Scope,
        ttl_s: u32,
        now_ms: i64,
    ) -> impl Future<Output = clax_git::Result<Minted>> + MaybeSend;

    /// Revokes token `id` of `repo` at the host as of `now_ms`; answers
    /// whether a live token was revoked.
    fn revoke(
        &self,
        repo: &str,
        id: &str,
        now_ms: i64,
    ) -> impl Future<Output = clax_git::Result<bool>> + MaybeSend;

    /// The git remote of `repo`; `origin` is the app origin a request came
    /// to, for a host served by this Worker.
    fn remote(
        &self,
        repo: &str,
        origin: Option<&str>,
    ) -> impl Future<Output = clax_git::Result<String>> + MaybeSend;
}

/// SHA-256 of a token's plaintext.
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn random_b64(n: usize) -> clax_git::Result<String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf)
        .map_err(|e| GitError::RepoUnavailable(format!("No random source: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// A new stand-in token for `scope`, expiring `ttl_s` after `now_ms`.
pub fn new_local_token(scope: Scope, ttl_s: u32, now_ms: i64) -> clax_git::Result<Minted> {
    let token = format!("cxm_{}_{}", scope.as_str(), random_b64(32)?);
    Ok(Minted {
        id: format!("cxt_{}", random_b64(12)?),
        hash: Some(token_hash(&token)),
        token,
        expires_ms: now_ms + i64::from(ttl_s) * 1000,
    })
}

/// `<origin>/git/<repo>.git`: the stand-in's remote at an app origin.
pub fn local_remote(origin: &str, repo: &str) -> String {
    format!("{}/git/{repo}.git", origin.trim_end_matches('/'))
}

/// The stand-in's tokens: minted into, and checked against, its ledger.
pub struct LocalTokens<C> {
    pub host: LocalHost<C>,
    /// `PUBLIC_ORIGIN`; empty for the origin each request came to.
    pub public_origin: String,
}

impl<C: AsConn + MaybeSend> TokenHost for LocalTokens<C> {
    async fn mint(
        &self,
        repo: &str,
        scope: Scope,
        ttl_s: u32,
        now_ms: i64,
    ) -> clax_git::Result<Minted> {
        if !self.host.with(|c| db::exists(c, repo))? {
            return Err(GitError::NotFound(format!("No repository {repo}")));
        }
        new_local_token(scope, ttl_s, now_ms)
    }

    async fn revoke(&self, repo: &str, id: &str, now_ms: i64) -> clax_git::Result<bool> {
        // The ledger is the stand-in's record of live tokens: revoking there
        // is revoking.
        self.host.with(|c| db::revoke_token(c, repo, id, now_ms))
    }

    async fn remote(&self, repo: &str, origin: Option<&str>) -> clax_git::Result<String> {
        let at = Some(self.public_origin.as_str())
            .filter(|o| !o.is_empty())
            .or(origin)
            .ok_or_else(|| {
                GitError::InvalidArgument(
                    "The stand-in's remote needs PUBLIC_ORIGIN or the request's origin".into(),
                )
            })?;
        Ok(local_remote(at, repo))
    }
}

/// Mints a token for `repo` at `tokens`, handed to `holder`, and enters it
/// in the ledger of `ledger`. `ttl_s` must lie within
/// [`MIN_TTL_S`]..=[`MAX_TTL_S`].
#[allow(clippy::too_many_arguments)]
pub async fn mint<C: AsConn, T: TokenHost>(
    ledger: &LocalHost<C>,
    tokens: &T,
    repo: &str,
    scope: Scope,
    ttl_s: u32,
    origin: Option<&str>,
    holder: &Holder,
    now_ms: i64,
) -> clax_git::Result<GitToken> {
    if !(MIN_TTL_S..=MAX_TTL_S).contains(&ttl_s) {
        return Err(GitError::InvalidArgument(format!(
            "A token lives {MIN_TTL_S} to {MAX_TTL_S} seconds"
        )));
    }
    let minted = tokens.mint(repo, scope, ttl_s, now_ms).await?;
    let remote = tokens.remote(repo, origin).await?;
    let row = db::TokenRow {
        id: minted.id.clone(),
        repo: repo.to_string(),
        scope: scope.as_str().to_string(),
        expires_ms: minted.expires_ms,
        revoked_ms: None,
        holder: holder.clone(),
    };
    ledger.with(|c| db::insert_token(c, &row, minted.hash.as_ref().map(|h| h.as_slice())))?;
    Ok(GitToken {
        clone_command: clone_command(&remote, &minted.token)?,
        jj_clone_command: jj_clone_command(&remote, &minted.token)?,
        id: minted.id,
        remote,
        token: minted.token,
        expires_at: rfc3339(minted.expires_ms),
    })
}

/// Revokes token `id` of `repo`: enters it in the must-revoke list, asks
/// the host, and on success marks it revoked and takes it off the list.
/// A failure leaves it listed for [`retry_due`] and is answered. Answers
/// whether a live token was revoked.
pub async fn revoke<C: AsConn, T: TokenHost>(
    ledger: &LocalHost<C>,
    tokens: &T,
    repo: &str,
    id: &str,
    now_ms: i64,
) -> clax_git::Result<bool> {
    let row = ledger.with(|c| db::token(c, id))?;
    if let Some(r) = &row {
        if r.repo != repo {
            return Ok(false);
        }
        if r.revoked_ms.is_some() || r.expires_ms <= now_ms {
            return Ok(false);
        }
    }
    // A token minted before the ledger knew it: kept listed for the longest
    // a token lives.
    let expires_ms = row
        .as_ref()
        .map_or(now_ms + i64::from(MAX_TTL_S) * 1000, |r| r.expires_ms);
    ledger.with(|c| db::add_due(c, repo, id, expires_ms))?;
    let revoked = tokens.revoke(repo, id, now_ms).await?;
    ledger.with(|c| -> clax_git::Result<()> {
        db::revoke_token(c, repo, id, now_ms)?;
        db::remove_due(c, id)
    })?;
    Ok(revoked)
}

/// A live token as [`held`] lists it: never its secret.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeldToken {
    pub id: String,
    pub repo: String,
    /// `read` or `write`.
    pub scope: String,
    /// Milliseconds since the epoch.
    pub expires_ms: i64,
    pub person: Option<String>,
    pub session: Option<String>,
}

/// Every token live at `now_ms` that was handed to a person or an agent
/// session ([`db::live_held_tokens`]).
pub fn held<C: AsConn>(ledger: &LocalHost<C>, now_ms: i64) -> clax_git::Result<Vec<HeldToken>> {
    let rows = ledger.with(|c| db::live_held_tokens(c, now_ms))?;
    Ok(rows
        .into_iter()
        .map(|r| HeldToken {
            id: r.id,
            repo: r.repo,
            scope: r.scope,
            expires_ms: r.expires_ms,
            person: r.holder.person,
            session: r.holder.session,
        })
        .collect())
}

/// Revokes, as [`revoke`] does, every live token handed to agent session
/// `session` (when given) or to person `person` (when given; their own
/// tokens and their agents'). Naming neither revokes nothing. Answers how
/// many were revoked; the first failure at the host is answered after
/// every other token was tried, each failed one left in the must-revoke
/// list.
pub async fn revoke_held<C: AsConn, T: TokenHost>(
    ledger: &LocalHost<C>,
    tokens: &T,
    person: Option<&str>,
    session: Option<&str>,
    now_ms: i64,
) -> clax_git::Result<usize> {
    if person.is_none() && session.is_none() {
        return Ok(0);
    }
    let rows = ledger.with(|c| db::live_held_tokens(c, now_ms))?;
    let mut revoked = 0;
    let mut failed = None;
    for r in rows.iter().filter(|r| {
        session.is_some_and(|s| r.holder.session.as_deref() == Some(s))
            || person.is_some_and(|p| r.holder.person.as_deref() == Some(p))
    }) {
        match revoke(ledger, tokens, &r.repo, &r.id, now_ms).await {
            Ok(true) => revoked += 1,
            Ok(false) => {}
            Err(e) => {
                failed.get_or_insert(e);
            }
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(revoked),
    }
}

/// Tries again to revoke every token in the must-revoke list; expired ones
/// leave it. Answers how many are still listed.
pub async fn retry_due<C: AsConn, T: TokenHost>(
    ledger: &LocalHost<C>,
    tokens: &T,
    now_ms: i64,
) -> clax_git::Result<usize> {
    let due = ledger.with(|c| db::due(c, now_ms))?;
    let mut left = 0;
    for (repo, id) in due {
        match tokens.revoke(&repo, &id, now_ms).await {
            Ok(_) => ledger.with(|c| -> clax_git::Result<()> {
                db::revoke_token(c, &repo, &id, now_ms)?;
                db::remove_due(c, &id)
            })?,
            Err(_) => left += 1,
        }
    }
    Ok(left)
}

/// What a stand-in token grants on `repo` at `now_ms`, or `None` when it
/// is not a live token of that repository (or the repository is gone).
pub fn check<C: AsConn>(
    ledger: &LocalHost<C>,
    repo: &str,
    token: &str,
    now_ms: i64,
) -> clax_git::Result<Option<Access>> {
    let hash = token_hash(token);
    ledger.with(|c| {
        let Some(row) = db::live_token_by_hash(c, &hash, now_ms)? else {
            return Ok(None);
        };
        if row.repo != repo || !db::exists(c, repo)? {
            return Ok(None);
        }
        Ok(Scope::parse(&row.scope).map(Scope::access))
    })
}

/// Whom the live stand-in token `token` of `repo` was handed to at
/// `now_ms`, or `None` when it is not one.
pub fn holder<C: AsConn>(
    ledger: &LocalHost<C>,
    repo: &str,
    token: &str,
    now_ms: i64,
) -> clax_git::Result<Option<Holder>> {
    let hash = token_hash(token);
    ledger.with(|c| {
        Ok(db::live_token_by_hash(c, &hash, now_ms)?
            .filter(|row| row.repo == repo)
            .map(|row| row.holder))
    })
}

/// Records in `ledger` who made the push to `path` that moved `pushed`:
/// when it left `main` at a commit, the holder of the push's token (from
/// `authorization`; the owner for none the ledger knows), against that
/// commit. Answers the artifact and the holder, or `None` when `path` is
/// no artifact's repository or the push did not move `main` to a commit.
/// It does not wait on anything, so `RepoDO` records the push before any
/// other request reaches it.
pub fn note_push<C: AsConn>(
    ledger: &LocalHost<C>,
    path: &str,
    authorization: Option<&str>,
    pushed: &[clax_git::RefUpdate],
    now_ms: i64,
) -> clax_git::Result<Option<(String, Holder)>> {
    let Some((repo, _)) = clax_git::server::split_git_path(path) else {
        return Ok(None);
    };
    let Some(artifact) = repo
        .strip_prefix("a-")
        .filter(|id| clax_git::events::is_artifact_id(id))
    else {
        return Ok(None);
    };
    let Some(head) = pushed
        .iter()
        .find(|u| u.name == clax_git::MAIN && !u.new.is_zero())
        .map(|u| u.new)
    else {
        return Ok(None);
    };
    let holder = match authorization.and_then(clax_git::server::bearer_token) {
        Some(t) => holder(ledger, repo, t, now_ms)?.unwrap_or_default(),
        None => Holder::default(),
    };
    ledger.with(|c| db::record_push(c, repo, &head, &holder, now_ms))?;
    Ok(Some((artifact.to_string(), holder)))
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Milliseconds since the epoch of an RFC 3339 time.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp_millis())
}

// ── Commands ────────────────────────────────────────────────────────────────

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

fn checked<'a>(what: &str, ok: bool, v: &'a str) -> clax_git::Result<&'a str> {
    if ok {
        Ok(v)
    } else {
        Err(GitError::RepoUnavailable(format!(
            "The repository's {what} holds characters a shell command cannot carry"
        )))
    }
}

/// `Authorization: Bearer <token>`, as git's `http.extraHeader`.
pub fn auth_header(token: &str) -> clax_git::Result<String> {
    Ok(format!(
        "Authorization: Bearer {}",
        checked("token", token_safe(token), token)?
    ))
}

/// `git -c http.extraHeader="Authorization: Bearer <token>" clone <remote>`.
pub fn clone_command(remote: &str, token: &str) -> clax_git::Result<String> {
    Ok(format!(
        "git -c http.extraHeader=\"{}\" clone {}",
        auth_header(token)?,
        checked("remote", remote_safe(remote), remote)?
    ))
}

/// `jj git clone` with the header set through `GIT_CONFIG_COUNT`,
/// `GIT_CONFIG_KEY_0` and `GIT_CONFIG_VALUE_0`, which the git jj runs for
/// fetches and pushes reads; the same prefix serves `jj git fetch` and
/// `jj git push` in the clone.
pub fn jj_clone_command(remote: &str, token: &str) -> clax_git::Result<String> {
    Ok(format!(
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0=\"{}\" jj git clone {}",
        auth_header(token)?,
        checked("remote", remote_safe(remote), remote)?
    ))
}

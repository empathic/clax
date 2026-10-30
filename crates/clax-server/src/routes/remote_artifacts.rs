//! Artifacts on a connected Clax instance (a remote), read and commented
//! on through this daemon, which holds the remote's agent token (spec
//! 2026-10-29-remote-instances §4): the MCP tools' `read` and `comments_*`
//! and the CLI's `clax comments` on a URL that `GET /api/resolve` answers
//! `remote`.
//!
//! Under `/api/remotes/<rid>/artifacts/<raid>`:
//!
//! - `GET` → the remote's `GET /api/artifacts/<raid>`: `{artifact,
//!   versions}`.
//! - `GET versions/<n>/files/<path>` → that file's bytes, as the remote's
//!   `GET /api/artifacts/<raid>/versions/<n>/files/<path>` answers them,
//!   at most a file's 16 MiB.
//! - `GET threads` (its query, such as `include_resolved` and `cursor`,
//!   passed on) and `GET threads/<tid>` → the remote's thread routes:
//!   `{threads, next_cursor}` and `{thread}`, as the remote filters them
//!   for its person's agents. A thread whose comments the remote withholds
//!   carries `withheld: {count, authors, approval}`.
//! - `POST threads` (multipart, as the remote takes it), `POST
//!   threads/<tid>/comments`, `POST threads/<tid>/resolve` and `POST
//!   threads/<tid>/reopen` → the remote's routes of the same paths, the
//!   body and its `content-type` passed on as sent, and the caller's
//!   `Idempotency-Key` with them: the remote answers a key it has seen
//!   from this token with what the first request made.
//! - `POST threads/<tid>/handoff {to?}` → the remote's `POST
//!   /api/sessions/<sid>/threads/<tid>/handoff`, `sid` the session the
//!   write is made in, once the remote has the thread on `raid`.
//!
//! A write is made in a session of its own there, ended once it is
//! answered (or its request dropped): named as the calling agent's session
//! here (its name, else its harness, and its harness), or the person's
//! by-hand one for the owner. An agent with no session here writes nothing
//! there (403 `no_session`): a by-hand write is the person's own.
//!
//! The owner and the owner's agents alone reach these routes, as they
//! alone may push (`super::remotes::asker`), on a local daemon (404 on a
//! hosted one). `rid` is a remote's ID, name or origin (404 `no_remote`
//! otherwise; 400 `not_connected` while its sign-in waits); `raid` an
//! artifact ID, `tid` a thread ID, a file path's segments neither empty,
//! `.` nor `..` (400 `invalid_args` otherwise). The remote's answers come
//! back with their status and body, sandboxed and unsniffed
//! (`super::remote_apps`). A remote not reached is 502 `remote_unreachable`
//! `{origin, name}`, and its discovery answer is forgotten
//! ([`clax_http::discovery_cache::DiscoveryCache::forget`]). A remote that
//! refuses the token (401 `invalid_token`, `grant_revoked` or
//! `token_expired`: revoked or gone there) is marked `signed_out` and
//! answers 502 `remote_sign_in_required` `{origin, name, request: {tool,
//! command}}`, the connection request an agent makes to have the person
//! connect it again ([`crate::resolve::SignInRequest`]); a signed-out
//! remote answers it at once. Any other 401 (`invalid_session_proof`,
//! `replayed_request`: one request refused) keeps the token and is 502
//! `remote_refused` naming the remote's code. No answer carries the token.

use super::artifacts::path;
use super::remotes::Asker;
use crate::public::{self, Client, Forwarded, RemoteFail};
use crate::resolve::SignInRequest;
use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use clax_core::Secret;
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::remotes::Remote;
use reqwest::Method;
use serde_json::json;

/// What only the owner and the owner's agents do here.
const WHAT: &str = "reaches a connected Clax instance";

/// The headers of a write passed on to the remote as the caller sent them.
const WRITE_HEADERS: [&str; 2] = ["content-type", "idempotency-key"];

/// A remote a route reaches: the remote, its token, and who asks.
struct Reach {
    remote: Remote,
    token: Secret,
    asker: Asker,
}

/// 502 `remote_sign_in_required`: remote `r` refused this daemon's token
/// (or did before), so the person must connect it again; `request` is how
/// an agent asks them.
pub fn sign_in_required(r: &Remote) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::BAD_GATEWAY,
        "remote_sign_in_required",
        format!(
            "{} ({}) no longer takes this Clax's token: the person connects it again",
            r.name, r.origin
        ),
    );
    e.extra.insert("origin".into(), json!(r.origin));
    e.extra.insert("name".into(), json!(r.name));
    if let Ok(u) = url::Url::parse(&r.origin) {
        e.extra
            .insert("request".into(), json!(SignInRequest::for_url(&u)));
    }
    e
}

/// 502 `remote_unreachable`: remote `r` was not reached (`why`).
fn unreachable(r: &Remote, why: &str) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::BAD_GATEWAY,
        "remote_unreachable",
        format!("cannot reach {} ({}): {why}", r.name, r.origin),
    );
    e.extra.insert("origin".into(), json!(r.origin));
    e.extra.insert("name".into(), json!(r.name));
    e
}

/// Remote `r` refused its token: it is marked signed out, its discovery
/// answer forgotten, and its linked threads' sync stops.
async fn signed_out(s: &AppState, r: &Remote) -> ApiError {
    let id = r.id.clone();
    if let Err(e) = s.store_call(move |st| st.sign_out_remote(&id)).await {
        tracing::warn!(error = %e.message, "marking a remote signed out failed");
    }
    s.discovery.forget(&r.origin);
    s.linked.links_changed();
    sign_in_required(r)
}

/// Whether a remote's 401 with error `code` says the token itself is no
/// longer good there (revoked, expired or unknown), rather than refusing
/// one request (a session proof or a replay).
fn token_refused(code: &str) -> bool {
    matches!(code, "invalid_token" | "grant_revoked" | "token_expired")
}

/// 502 `remote_refused`: remote `r` refused one request with a 401 whose
/// `code` does not say the token is gone; the token is kept.
fn refused_once(r: &Remote, code: &str, message: &str) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::BAD_GATEWAY,
        "remote_refused",
        format!("{} refused: {code}: {message}", r.origin),
    );
    e.extra.insert("remote_code".into(), json!(code));
    e.extra.insert("remote_status".into(), json!(401));
    e
}

/// A request to remote `r` that failed as [`RemoteFail`] `f`, answered as
/// the module comment says.
async fn failed(s: &AppState, r: &Remote, f: RemoteFail) -> ApiError {
    match f.status {
        None => {
            s.discovery.forget(&r.origin);
            unreachable(r, &f.message)
        }
        Some(401) if token_refused(&f.code) => signed_out(s, r).await,
        Some(401) => refused_once(r, &f.code, &f.message),
        Some(_) if f.code == "too_large" => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "remote_refused",
            "the connected Clax's answer is too large",
        ),
        Some(_) => ApiError::from(f),
    }
}

/// Remote `r`'s answer `f`, passed on (`super::remote_apps::answer`); a
/// refused token marks it signed out, and any other 401 is
/// `remote_refused` naming the remote's code.
async fn passed(s: &AppState, r: &Remote, f: Forwarded) -> Response {
    if f.status == 401 {
        let v = serde_json::from_slice::<serde_json::Value>(&f.body).unwrap_or_default();
        let code = v["error"]["code"].as_str().unwrap_or("error");
        if token_refused(code) {
            return signed_out(s, r).await.into_response();
        }
        let message = v["error"]["message"].as_str().unwrap_or("HTTP 401");
        return refused_once(r, code, message).into_response();
    }
    super::remote_apps::answer(&r.origin, f)
}

/// The connected remote `key` names (its ID, name or origin) and its
/// token, for the owner or one of the owner's agents (each handler has
/// refused anyone else, `who.require_owner`, and a hosted daemon,
/// `local_only`).
async fn reach(s: &AppState, d: &DeniedToGrants, key: &str) -> Result<Reach, ApiError> {
    let asker = super::remotes::asker(s, d).await?;
    let k = key.trim().trim_end_matches('/').to_string();
    let remote = s
        .store_call(move |st| st.find_remote(&k))
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "no_remote",
                format!("no remote is named '{key}': `clax remote list` names them"),
            )
        })?;
    match remote.status.as_str() {
        "connected" => {}
        "signed_out" => return Err(sign_in_required(&remote)),
        _ => {
            return Err(ApiError::bad_request(
                "not_connected",
                format!("{} is not connected yet: its sign-in waits", remote.name),
            ));
        }
    }
    let id = remote.id.clone();
    let token = s
        .store_call(move |st| st.remote_token(&id))
        .await?
        .ok_or_else(|| sign_in_required(&remote))?;
    Ok(Reach {
        remote,
        token,
        asker,
    })
}

/// A thread ID as a path names it.
fn thread_id(raw: &str) -> Result<String, ApiError> {
    if clax_core::is_ulid(raw) {
        Ok(raw.to_string())
    } else {
        Err(ApiError::bad_request("invalid_args", "not a thread ID"))
    }
}

/// The query pairs of `raw`, as sent.
fn query_pairs(raw: Option<String>) -> Vec<(String, String)> {
    raw.map(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .into_owned()
            .collect()
    })
    .unwrap_or_default()
}

/// `GET segs` (with `query`) at `r`'s remote, at most `max` bytes back,
/// answered as the module comment says.
async fn read(
    s: &AppState,
    r: &Reach,
    segs: &[&str],
    query: &[(String, String)],
    max: usize,
) -> Response {
    let c = Client::new(&r.remote.origin, Some(r.token.clone()));
    let q: Vec<(&str, String)> = query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    match clax_rt::sendable(c.forward(Method::GET, segs, &q, Vec::new(), None, max)).await {
        Ok(f) => passed(s, &r.remote, f).await,
        Err(f) => failed(s, &r.remote, f).await.into_response(),
    }
}

/// Makes `c` speak in a session of its own there for whoever asks
/// ([`Asker`]): named as the calling agent's session here, or the owner's
/// by-hand session for the owner. (An agent with no session is refused
/// before, [`write`].)
async fn speak_for(s: &AppState, c: &mut Client, asker: &Asker) -> Result<(), RemoteFail> {
    let sid = match asker {
        Asker::Agent(Some(sid)) => sid.clone(),
        _ => return c.by_hand().await,
    };
    let sess = s
        .store_call(move |st| st.get_session(&sid))
        .await
        .ok()
        .flatten();
    match sess {
        Some(x) => {
            let name = x.name.clone().unwrap_or_else(|| x.harness.clone());
            c.session_for(&x.harness, &name).await
        }
        None => c.by_hand().await,
    }
}

/// `POST` `body` to `segs` at `r`'s remote, with the caller's `headers`
/// of [`WRITE_HEADERS`], in a session of its own there
/// ([`speak_for`]); with `first`, a `GET` of it first, whose refusal is the
/// answer. `segs` may name the session (`{session}`).
async fn write(
    s: &AppState,
    r: &Reach,
    first: Option<&[&str]>,
    segs: &[&str],
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    // A write there is the person's own only when the owner makes it: an
    // agent with no session here has none to be named for (as a push).
    if let Asker::Agent(None) = r.asker {
        return ApiError::forbidden(
            "no_session",
            "an agent writes on a connected Clax from a harness session (X-Clax-Session)",
        )
        .into_response();
    }
    let mut c = Client::new(&r.remote.origin, Some(r.token.clone()));
    if let Some(check) = first {
        match clax_rt::sendable(c.forward(
            Method::GET,
            check,
            &[],
            Vec::new(),
            None,
            public::MAX_ANSWER_BYTES,
        ))
        .await
        {
            Ok(f) if (200..300).contains(&f.status) => {}
            Ok(f) => return passed(s, &r.remote, f).await,
            Err(f) => return failed(s, &r.remote, f).await.into_response(),
        }
    }
    let sent: Vec<(&'static str, String)> = WRITE_HEADERS
        .iter()
        .filter_map(|k| {
            headers
                .get(*k)
                .and_then(|v| v.to_str().ok())
                .map(|v| (*k, v.to_string()))
        })
        .collect();
    let out = clax_rt::sendable(async {
        speak_for(s, &mut c, &r.asker).await?;
        // Ended there however this future ends: dropped mid-write, the
        // guard ends it in a task of its own.
        let mut guard = EndSession(Some(c.clone()));
        let sid = c.session().map(|x| x.0.clone()).unwrap_or_default();
        let segs: Vec<&str> = segs
            .iter()
            .map(|x| if *x == "{session}" { sid.as_str() } else { *x })
            .collect();
        let body = (!body.is_empty()).then(|| body.to_vec());
        let f = c
            .forward(
                Method::POST,
                &segs,
                &[],
                sent,
                body,
                public::MAX_ANSWER_BYTES,
            )
            .await;
        guard.0 = None;
        c.end_session().await;
        f
    })
    .await;
    match out {
        Ok(f) => passed(s, &r.remote, f).await,
        Err(f) => failed(s, &r.remote, f).await.into_response(),
    }
}

/// Ends the remote session its client speaks for when dropped while still
/// holding it: a write whose future is dropped mid-request.
struct EndSession(Option<Client>);

impl Drop for EndSession {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            clax_rt::spawn(clax_rt::sendable(async move { c.end_session().await }));
        }
    }
}

/// `GET /api/remotes/<rid>/artifacts/<raid>` → the remote's `GET
/// /api/artifacts/<raid>`.
pub async fn get(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (rid, raid) = path(p)?;
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    let r = reach(&s, &d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    Ok(read(
        &s,
        &r,
        &["api", "artifacts", &raid],
        &[],
        public::MAX_ANSWER_BYTES,
    )
    .await)
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/versions/<n>/files/<path>` →
/// the bytes of that file of version `n`, as the remote's `GET
/// /api/artifacts/<raid>/versions/<n>/files/<path>` answers them, at most
/// a file's 16 MiB.
pub async fn file(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (rid, raid, n, file) = path(p)?;
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    let r = reach(&s, &d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    let file = super::remote_apps::file_segments(&file)?;
    let n = n.to_string();
    let mut segs = vec!["api", "artifacts", raid.as_str(), "versions", &n, "files"];
    segs.extend(file.iter().map(String::as_str));
    Ok(read(
        &s,
        &r,
        &segs,
        &[],
        clax_core::publish::MAX_FILE_BYTES as usize,
    )
    .await)
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/threads` → the remote's `GET
/// /api/artifacts/<raid>/threads`, its query passed on.
pub async fn threads(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String)>, PathRejection>,
    RawQuery(q): RawQuery,
) -> Result<Response, ApiError> {
    let (rid, raid) = path(p)?;
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    let r = reach(&s, &d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    Ok(read(
        &s,
        &r,
        &["api", "artifacts", &raid, "threads"],
        &query_pairs(q),
        public::MAX_ANSWER_BYTES,
    )
    .await)
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/threads/<tid>` → the remote's
/// `GET /api/artifacts/<raid>/threads/<tid>`.
pub async fn thread(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (rid, raid, tid) = path(p)?;
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    let r = reach(&s, &d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    let tid = thread_id(&tid)?;
    Ok(read(
        &s,
        &r,
        &["api", "artifacts", &raid, "threads", &tid],
        &[],
        public::MAX_ANSWER_BYTES,
    )
    .await)
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/threads` (multipart, as the
/// remote's `POST /api/artifacts/<raid>/threads` takes it) → its answer.
pub async fn create_thread(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let (rid, raid) = path(p)?;
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    let r = reach(&s, &d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    Ok(write(
        &s,
        &r,
        None,
        &["api", "artifacts", &raid, "threads"],
        &headers,
        body,
    )
    .await)
}

/// The thread action `verb` (`comments`, `resolve` or `reopen`) of thread
/// `tid` of `raid` at the remote `rid` names, as [`write`] makes it.
async fn act(
    s: &AppState,
    d: &DeniedToGrants,
    p: Result<Path<(String, String, String)>, PathRejection>,
    verb: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let (rid, raid, tid) = path(p)?;
    let r = reach(s, d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    let tid = thread_id(&tid)?;
    Ok(write(
        s,
        &r,
        None,
        &["api", "artifacts", &raid, "threads", &tid, verb],
        headers,
        body,
    )
    .await)
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/threads/<tid>/comments` →
/// the remote's `POST /api/artifacts/<raid>/threads/<tid>/comments`: an
/// agent's reply (`author_kind: "agent"`), or the owner's comment.
pub async fn comment(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String, String)>, PathRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    act(&s, &d, p, "comments", &headers, body).await
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/threads/<tid>/resolve` → the
/// remote's `POST /api/artifacts/<raid>/threads/<tid>/resolve`.
pub async fn resolve(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String, String)>, PathRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    act(&s, &d, p, "resolve", &headers, body).await
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/threads/<tid>/reopen` → the
/// remote's `POST /api/artifacts/<raid>/threads/<tid>/reopen`.
pub async fn reopen(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String, String)>, PathRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    act(&s, &d, p, "reopen", &headers, body).await
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/threads/<tid>/handoff {to?}`
/// → the remote's `POST /api/sessions/<sid>/threads/<tid>/handoff`, once
/// its `GET /api/artifacts/<raid>/threads/<tid>` finds the thread there:
/// hands it to the agent `to` names there, or releases it.
pub async fn handoff(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String, String)>, PathRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let (rid, raid, tid) = path(p)?;
    super::remotes::local_only(&s)?;
    who.require_owner(WHAT)?;
    let r = reach(&s, &d, &rid).await?;
    let raid = super::remote_apps::remote_artifact(&raid)?;
    let tid = thread_id(&tid)?;
    Ok(write(
        &s,
        &r,
        Some(&["api", "artifacts", &raid, "threads", &tid]),
        &["api", "sessions", "{session}", "threads", &tid, "handoff"],
        &headers,
        body,
    )
    .await)
}

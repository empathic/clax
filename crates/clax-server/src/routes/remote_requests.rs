//! Connection requests (spec 2026-10-29-remote-instances §3): an agent of
//! the owner's asks that this home connect to a Clax instance; the owner
//! answers in their browser, and a Connect runs the owner's own device
//! sign-in ([`super::remotes::start_add`]), approved on the remote. The
//! store keeps the rules ([`clax_store::remote_requests`]).
//!
//! - `POST /api/remotes/requests {url, reason?}` (an agent of the owner's
//!   with a session registered here) → 201 `{status: "open", request}`
//!   for a new request (the owner is told, once: the gallery's inbox, in
//!   the asking agent's thread when it was working in one), 200 `{status:
//!   "open", request}` for one it joined, 200 `{status: "connected",
//!   remote}` (the remote's ID), 200 `{status: "muted"}`, 200 `{status:
//!   "not_now", until}`. 403 `session_required` without a live session,
//!   422 `not_a_clax` for an origin whose discovery names no Clax, 429
//!   `too_many_requests` past three new requests of the session in a day.
//! - `GET /api/remotes/requests/<id>?wait=<s>` (the owner or the owner's
//!   agents) → `{request_id, status, origin, name, remote?, waited_s,
//!   call_again}` as soon as the request is not open, or after `wait`
//!   seconds (at most [`MAX_WAIT_SECS`]), or when the daemon stops. A poll
//!   that finds as many polls waiting on the request as may answers at
//!   once, with `retry_after_s`.
//! - `POST /api/remotes/requests/<id>/answer {answer}` (the owner's
//!   browser alone, M20): `connect` → `{request, verification_url,
//!   user_code, expires_at}`, the sign-in for the shell to open, which the
//!   daemon then polls itself ([`drive`]) until it ends; `not_now` and
//!   `never` → `{request}`. 409 `request_closed` with `request` for one no
//!   longer open. While an agent of the owner's is live and passkeys are
//!   required, the answer waits for a confirmation made with a passkey
//!   (202 `confirmation_pending`, [`clax_http::confirm`]), as every
//!   decision that matters does.
//! - `GET /api/remotes/mutes` (the owner) → `{mutes: [{origin, muted_at}]}`;
//!   `DELETE /api/remotes/mutes/<origin>` (the owner's browser) →
//!   `{origin, unmuted}`.
//!
//! A hosted daemon has no remotes: every route answers 404 there.

use super::artifacts::{body, path};
use super::remotes::{local_only, owner_only, sign_in_view, start_add};
use crate::public::{self, SignIn};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::Caller;
use clax_http::confirm::{Confirming, Decision, require_confirmation};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_rt::time::Instant;
use clax_store::remote_requests::{Answer, Ask, Asked, Request};
use clax_store::remotes::Remote;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Longest `wait` of a request poll, in seconds.
pub const MAX_WAIT_SECS: u64 = clax_core::remote::REQUEST_MAX_WAIT_SECS;

/// When a poll that found every wait on its request taken should call
/// again, in seconds (its answer's `retry_after_s`).
pub const RETRY_AFTER_S: u64 = clax_core::remote::REQUEST_RETRY_AFTER_S;

/// The longest reason kept, in characters.
const MAX_REASON_CHARS: usize = 500;

/// A request as an agent sees it: its instance, its state, and how many
/// sessions asked; never another session's URL or reason.
fn for_agent(r: &Request) -> Value {
    json!({
        "id": r.id,
        "origin": r.origin,
        "name": r.name,
        "status": r.status,
        "created_at": r.created_at,
        "expires_at": r.expires_at,
        "answered_at": r.answered_at,
        "askers": r.askers.len(),
    })
}

/// A request as the owner's browser sees it.
fn owner_view(r: &Request) -> Value {
    json!(r)
}

/// 403 `browser_required` unless a browser of the owner's makes the
/// request (M20; [`clax_http::auth::require_owner_browser`]).
fn require_browser(caller: &Caller, what: &str) -> Result<(), ApiError> {
    clax_http::auth::require_owner_browser(caller, what)
}

fn session_required(why: &str) -> ApiError {
    ApiError::forbidden(
        "session_required",
        format!("asking to connect a Clax instance needs an agent session registered here: {why}"),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskBody {
    /// A URL on the instance (the page the agent wants to read), or its
    /// bare origin.
    url: String,
    /// Why, in the agent's words, shown to the person.
    #[serde(default)]
    reason: Option<String>,
}

/// The instance `raw` names: its origin, normalized as remotes keep it,
/// and the URL to show (none for a bare origin).
fn instance_of(raw: &str) -> Result<(String, Option<String>), ApiError> {
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let u = url::Url::parse(&with_scheme).map_err(|_| {
        ApiError::bad_request("invalid_url", format!("'{raw}' is not a URL or an origin"))
    })?;
    let origin = public::origin_of(&u.origin().ascii_serialization()).map_err(|_| {
        not_a_clax(&format!(
            "{raw} is not on an https origin (or http on this machine)"
        ))
    })?;
    let shown = (u.path() != "/" || u.query().is_some()).then(|| with_scheme.clone());
    Ok((origin, shown))
}

fn not_a_clax(message: &str) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "not_a_clax", message)
}

/// `POST /api/remotes/requests`, as the module comment says.
pub async fn ask(
    State(s): State<AppState>,
    d: DeniedToGrants,
    req: Result<Json<AskBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    local_only(&s)?;
    let sid = match d.caller(&s).await? {
        Caller::OwnerAgent { session: Some(sid) } => sid,
        Caller::OwnerAgent { session: None } | Caller::Owner { browser: false } => {
            return Err(session_required("send X-Clax-Session"));
        }
        Caller::Anonymous => return Err(ApiError::unauthorized()),
        _ => {
            return Err(ApiError::forbidden(
                "forbidden",
                "only the owner's agents ask to connect a Clax instance; the owner connects one with Connect in Clax or `clax remote add`",
            ));
        }
    };
    let b = body(req)?;
    let reason = b
        .reason
        .map(|r| r.trim().chars().take(MAX_REASON_CHARS).collect::<String>())
        .filter(|r| !r.is_empty());
    let (origin, url) = instance_of(&b.url)?;
    let id = sid.clone();
    let live = s
        .store_call(move |st| Ok(st.get_session(&id)?.is_some_and(|x| x.ended_at.is_none())))
        .await?;
    if !live {
        return Err(session_required("no live session has this ID"));
    }
    // A connected, muted, held-off or capped ask is answered before any
    // request leaves for the origin.
    let (o, id) = (origin.clone(), sid.clone());
    if let Some(early) = s
        .store_call(move |st| st.remote_ask_answer(&o, &id))
        .await?
    {
        return asked(early);
    }
    // The gate before the person hears anything: a Clax, by its discovery.
    let Some(instance) = clax_rt::sendable(crate::resolve::discover(&s, &origin)).await else {
        return Err(not_a_clax(&format!(
            "{origin} does not describe itself as a Clax instance (GET /.well-known/clax)"
        )));
    };
    let page = working_page(&s, &sid).await?;
    let st = s.clone();
    let asked_now = s
        .store_call(move |db| {
            let place = clax_http::surface::suggestion(&st, db, &sid, page.as_deref())?.r#where;
            db.ask_remote(&Ask {
                origin,
                name: instance.name,
                session: sid,
                url,
                reason,
                artifact_id: place.artifact_id,
                thread_id: place.thread_id,
            })
        })
        .await?;
    asked(asked_now)
}

/// The answer to an ask that the store answered `asked`.
fn asked(asked: Asked) -> Result<Response, ApiError> {
    Ok(match asked {
        Asked::Opened(r) => (
            StatusCode::CREATED,
            Json(json!({"status": "open", "request": for_agent(&r)})),
        )
            .into_response(),
        Asked::Joined(r) => {
            Json(json!({"status": "open", "request": for_agent(&r)})).into_response()
        }
        Asked::Connected(remote) => {
            Json(json!({"status": "connected", "remote": remote})).into_response()
        }
        Asked::Muted => Json(json!({"status": "muted"})).into_response(),
        Asked::Backoff { until } => {
            Json(json!({"status": "not_now", "until": until})).into_response()
        }
        Asked::Capped => {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                format!(
                    "this session opened {} connection requests in the last day: wait for the person's answers",
                    clax_store::remote_requests::MAX_NEW_PER_SESSION
                ),
            ));
        }
    })
}

/// The page session `sid` works on (its newest working record's, else
/// its artifact), which the request's place in Clax is weighed with.
async fn working_page(s: &AppState, sid: &str) -> Result<Option<String>, ApiError> {
    let raw = match s.working.newest_artifact_of(sid) {
        Some(aid) => Some(aid),
        None => {
            let id = sid.to_string();
            s.store_call(move |st| st.session_artifact(&id)).await?
        }
    };
    Ok(raw.filter(|a| clax_core::ArtifactId::parse(a).is_ok()))
}

#[derive(Deserialize)]
pub struct WaitQuery {
    #[serde(default)]
    wait: u64,
}

/// What a waiting agent is told of request `r`.
fn answer_view(r: &Request) -> Value {
    let mut v = json!({
        "request_id": r.id,
        "status": r.status,
        "origin": r.origin,
        "name": r.name,
        "call_again": r.status == "open",
    });
    if r.status == "connected" {
        v["remote"] = json!(r.remote_id);
    }
    v
}

/// `GET /api/remotes/requests/<id>?wait=<s>`, as the module comment says.
pub async fn poll(
    State(s): State<AppState>,
    d: DeniedToGrants,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<WaitQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    match d.caller(&s).await? {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => {}
        Caller::Anonymous => return Err(ApiError::unauthorized()),
        _ => return Err(ApiError::not_found()),
    }
    let id = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let started = Instant::now();
    let wait = Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let id1 = id.clone();
    s.store_call(move |st| st.remote_request(&id1)).await?;
    let Some((notify, _hold)) = s.questions.hold(&id, None) else {
        // As many polls as may wait on one request wait on it already:
        // this one answers how it stands now, and when to call again.
        let id1 = id.clone();
        let r = s.store_call(move |st| st.remote_request(&id1)).await?;
        let mut v = answer_view(&r);
        v["waited_s"] = json!(0);
        if r.status == "open" {
            v["retry_after_s"] = json!(RETRY_AFTER_S);
        }
        return Ok(Json(v));
    };
    let mut shutdown = s.shutdown.clone();
    let stopping = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopping);
    let expiry = s.question_sleeper.sleep(wait);
    tokio::pin!(expiry);
    let mut done = wait.is_zero();
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let id1 = id.clone();
        let r = s.store_call(move |st| st.remote_request(&id1)).await?;
        if r.status != "open" || done {
            let mut v = answer_view(&r);
            v["waited_s"] = json!(started.elapsed().as_secs());
            return Ok(Json(v));
        }
        tokio::select! {
            () = &mut notified => {}
            () = &mut expiry, if !done => done = true,
            () = &mut stopping, if !done => done = true,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBody {
    /// `connect`, `not_now` or `never`.
    answer: String,
}

/// `POST /api/remotes/requests/<id>/answer`, as the module comment says.
pub async fn answer(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    by: Confirming,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<AnswerBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    local_only(&s)?;
    let caller = d.caller(&s).await?;
    require_browser(&caller, "answering a connection request")?;
    let id = path(p)?;
    let b = body(req)?;
    decide(&s, &caller, &by, id, &b.answer).await
}

/// The owner's answer `answer` to request `id`, its caller checked: the
/// one place every answer is decided. First, the answer goes through the
/// browser confirmation of the decisions that matter
/// ([`Decision::AnswerConnection`]).
async fn decide(
    s: &AppState,
    caller: &Caller,
    by: &Confirming,
    id: String,
    answer: &str,
) -> Result<Response, ApiError> {
    let decision = Decision::AnswerConnection {
        request_id: id.clone(),
        answer: answer.to_string(),
    };
    require_confirmation(s, caller, by, decision).await?;
    let id1 = id.clone();
    let r = s.store_call(move |st| st.remote_request(&id1)).await?;
    if r.status != "open" {
        return Err(closed(&r));
    }
    let out = match answer {
        "connect" => return connect(s, r).await,
        "not_now" => Answer::NotNow,
        "never" => Answer::Never,
        _ => {
            return Err(ApiError::bad_request(
                "invalid_args",
                "answer is connect, not_now or never",
            ));
        }
    };
    let id1 = id.clone();
    let r = s
        .store_call(move |st| st.answer_remote_request(&id1, out))
        .await?;
    s.questions.wake(&id);
    Ok(Json(json!({"request": owner_view(&r)})).into_response())
}

/// 409 `request_closed` with the request's view.
fn closed(r: &Request) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::CONFLICT,
        "request_closed",
        format!("this request is {} already", r.status),
    );
    e.extra.insert("request".into(), owner_view(r));
    e
}

/// Connect: the owner's add for the request's origin, the daemon then
/// polling its sign-in ([`drive`]). Connects take the origin's Connect
/// lock ([`clax_http::remote_drivers::RemoteDrivers::connect_lock`]), so
/// a second one, at once or later while the first's sign-in waits, answers that sign-in again (driving it when no task of this
/// daemon does, as after a restart) rather than starting another.
async fn connect(s: &AppState, r: Request) -> Result<Response, ApiError> {
    let lock = s.remote_drivers.connect_lock(&r.origin);
    let _one = lock.lock().await;
    // Read again under the lock: another Connect may have started one.
    let id = r.id.clone();
    let r = s.store_call(move |st| st.remote_request(&id)).await?;
    if r.status != "open" {
        return Err(closed(&r));
    }
    if let Some(p) = pending_of(s, &r).await? {
        drive(s.clone(), r.id.clone(), p.clone());
        return Ok(Json(connect_view(&r, &p)).into_response());
    }
    let (remote, _) = start_add(s, r.origin.clone(), None, None, None, None).await?;
    let (id, rid) = (r.id.clone(), remote.id.clone());
    let r = s
        .store_call(move |st| st.answer_remote_request(&id, Answer::Connecting(rid)))
        .await?;
    // The store keeps a waiting sign-in another daemon recorded first.
    let p = pending_of(s, &r).await?.unwrap_or(remote);
    drive(s.clone(), r.id.clone(), p.clone());
    Ok(Json(connect_view(&r, &p)).into_response())
}

/// The pending remote whose sign-in request `r`'s Connect started, while
/// it waits.
async fn pending_of(s: &AppState, r: &Request) -> Result<Option<Remote>, ApiError> {
    let Some(rid) = r.remote_id.clone() else {
        return Ok(None);
    };
    let found = s.store_call(move |st| st.find_remote(&rid)).await?;
    Ok(found.filter(|p| p.status == "pending" && Some(&p.id) == r.remote_id.as_ref()))
}

/// Drives again the sign-ins a Connect left waiting when this daemon last
/// stopped: each open request whose remote still waits for its sign-in.
/// Nothing on a hosted daemon.
pub fn resume(s: &AppState) {
    if s.hosted.is_some() {
        return;
    }
    let s = s.clone();
    clax_rt::spawn(async move {
        let waiting = s
            .store_call(|st| {
                let mut out = Vec::new();
                for r in st.open_remote_requests()? {
                    let Some(rid) = r.remote_id.clone() else {
                        continue;
                    };
                    if let Some(p) = st.find_remote(&rid)?.filter(|p| p.status == "pending") {
                        out.push((r.id, p));
                    }
                }
                Ok(out)
            })
            .await;
        match waiting {
            Ok(w) => {
                for (id, p) in w {
                    drive(s.clone(), id, p);
                }
            }
            Err(e) => tracing::warn!(code = e.code, "cannot resume connection requests' sign-ins"),
        }
    });
}

fn connect_view(r: &Request, remote: &Remote) -> Value {
    let mut v = sign_in_view(remote, false);
    v["request"] = owner_view(r);
    v
}

/// Polls pending remote `remote`'s sign-in, started by request `id`'s
/// Connect, every interval the remote asked for (on the question clock),
/// until it ends: connected answers the request `connected`; denied there
/// `dismissed`, holding new requests off as a "Not now"; expired, or the
/// remote forgotten by another poll, `dismissed`. Each end wakes the
/// waiting agents. It stops when the daemon does, or when the request is
/// no longer open.
///
/// At most one task of this daemon drives a request
/// ([`clax_http::remote_drivers::RemoteDrivers`]): a second call while one
/// runs does nothing.
fn drive(s: AppState, id: String, remote: Remote) {
    let Some(claim) = s.remote_drivers.claim(&id) else {
        return;
    };
    let every = Duration::from_secs(remote.poll_interval_s.unwrap_or(5).clamp(1, 60) as u64);
    clax_rt::spawn(async move {
        let _claim = claim;
        let mut shutdown = s.shutdown.clone();
        loop {
            let stop = async {
                if shutdown.wait_for(|v| *v).await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = s.question_sleeper.sleep(every) => {}
                () = stop => return,
            }
            match step(&s, &id, &remote.id).await {
                Ok(None) => continue,
                Ok(Some(ans)) => {
                    let id1 = id.clone();
                    let _ = s
                        .store_call(move |st| st.answer_remote_request(&id1, ans))
                        .await;
                    s.questions.wake(&id);
                    return;
                }
                Err(e) => {
                    tracing::warn!(request = id.as_str(), code = e.code, "sign-in poll failed");
                    if e.code == "request_settled" {
                        s.questions.wake(&id);
                        return;
                    }
                }
            }
        }
    });
}

/// One poll of request `id`'s sign-in on remote `rid`: the answer it ends
/// with, or `None` while it waits.
async fn step(s: &AppState, id: &str, rid: &str) -> Result<Option<Answer>, ApiError> {
    // One poll at a time, under the origin's Connect lock: a second driver
    // of the same sign-in (a stranded one beside its replacement) reads
    // what the first's poll left, rather than spending the used code again.
    let rid0 = rid.to_string();
    let origin = s
        .store_call(move |st| Ok(st.find_remote(&rid0)?.map(|r| r.origin)))
        .await?;
    let lock = origin.map(|o| s.remote_drivers.connect_lock(&o));
    let _one = match &lock {
        Some(l) => Some(l.lock().await),
        None => None,
    };
    let (id1, rid1) = (id.to_string(), rid.to_string());
    let (req, remote) = s
        .store_call(move |st| Ok((st.remote_request(&id1)?, st.find_remote(&rid1)?)))
        .await?;
    if req.status != "open" || req.remote_id.as_deref() != Some(rid) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "request_settled",
            "the request is no longer waiting on this sign-in",
        ));
    }
    let Some(remote) = remote else {
        return Ok(Some(Answer::Dismissed { hold_off: false }));
    };
    if remote.status == "connected" {
        return Ok(Some(Answer::Connected(remote.id)));
    }
    Ok(
        match clax_rt::sendable(public::poll_sign_in(s, &remote)).await {
            Ok(SignIn::Waiting) => None,
            Ok(SignIn::Connected(c)) => {
                owner_only(&s.home);
                Some(Answer::Connected(c.id))
            }
            Ok(SignIn::Refused("access_denied")) => Some(Answer::Dismissed { hold_off: true }),
            Ok(SignIn::Refused(_)) => Some(Answer::Dismissed { hold_off: false }),
            // Connected, or forgotten, by another poll between the read and
            // this one (two drivers of a stranded sign-in poll at once).
            Err(e) if e.code == "not_pending" => {
                let rid = remote.id.clone();
                match s.store_call(move |st| st.find_remote(&rid)).await? {
                    Some(r) if r.status == "connected" => Some(Answer::Connected(r.id)),
                    _ => Some(Answer::Dismissed { hold_off: false }),
                }
            }
            Err(e) => {
                // Past its expiry by this daemon's clock, a sign-in the remote
                // cannot be asked about is over.
                let now = s.store.now();
                if remote
                    .sign_in_expires_at
                    .as_deref()
                    .is_some_and(|t| !t.is_empty() && t <= now.as_str())
                {
                    let rid = remote.id.clone();
                    s.store_call(move |st| st.abandon_sign_in(&rid).map(|_| ()))
                        .await?;
                    Some(Answer::Dismissed { hold_off: false })
                } else {
                    return Err(e);
                }
            }
        },
    )
}

/// `GET /api/remotes/mutes`, as the module comment says.
pub async fn mutes(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("lists the muted Clax instances")?;
    let ms = s.store_call(|st| st.remote_mutes()).await?;
    Ok(Json(json!({
        "mutes": ms.iter().map(|(o, at)| json!({"origin": o, "muted_at": at})).collect::<Vec<_>>()
    })))
}

/// `DELETE /api/remotes/mutes/<origin>`, as the module comment says.
pub async fn unmute(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    require_browser(&d.caller(&s).await?, "unmuting a Clax instance")?;
    let origin = path(p)?;
    let o = origin.clone();
    let unmuted = s.store_call(move |st| st.unmute_origin(&o)).await?;
    Ok(Json(json!({"origin": origin, "unmuted": unmuted})))
}

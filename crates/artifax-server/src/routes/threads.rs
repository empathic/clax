//! Comment threads (spec §6 "Comments"): create with anchor and clip, comment,
//! send to the agent, resolve, list, and serve clips.

use super::artifacts::{body, parse_id, path, publishing_session, session_header};
use super::assets::multipart_error;
use crate::auth::has_token;
use crate::error::ApiError;
use crate::feedback::{apply, thread_view};
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie, author_name};
use artifax_core::feedback::Touched;
use artifax_core::model::{Session, Thread};
use artifax_core::store::threads::{
    AUTHOR_AGENT, AUTHOR_VIEWER, DEFAULT_THREAD_PAGE, NewComment, NewThread, clip_problem,
};
use artifax_core::{Anchor, ArtifactId, CoreError, Event, Store};
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

/// Request cap for thread creation; a clip over `MAX_CLIP_BYTES` but under this
/// is dropped with `clip_error`, a larger request is refused with 413.
pub const THREAD_BODY_LIMIT: usize = 16 * 1024 * 1024;
pub const GUIDANCE_REPLY: &str = "This thread was not sent to you. Only threads the person sends to the agent accept agent replies; leave plain threads to people. Nothing was written.";
pub const GUIDANCE_RESOLVE: &str = "This thread was not sent to you. Only threads the person sends to the agent can be resolved by the agent; leave plain threads to people. Nothing was changed.";
pub const GUIDANCE_REOPEN: &str = "This thread was not sent to you. Only threads the person sends to the agent can be reopened by the agent; leave plain threads to people. Nothing was changed.";
pub const GUIDANCE_DELETE: &str = "This thread was not sent to you. Only threads the person sends to the agent can be deleted by the agent; leave plain threads to people. Nothing was deleted.";
pub const NAME_REQUIRED: &str =
    "set a name in the viewer (the \"Your name\" field) before reopening or deleting threads";

/// True when `body` mentions `@agent` as a word: not inside an address
/// (`me@agent.dev`) or a longer word (`@agents`).
pub fn mentions_agent(body: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    body.match_indices("@agent").any(|(i, m)| {
        let before_ok = body[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !word(c) && c != '.');
        let mut rest = body[i + m.len()..].chars();
        let after_ok = match rest.next() {
            None => true,
            Some('.') => rest.next().is_none_or(|c| !word(c)),
            Some(c) => !word(c),
        };
        before_ok && after_ok
    })
}

/// Broadcasts the `thread` event. `/api/events` needs no token, so the view is
/// always built without `clip_path`, whoever made the change.
fn publish_thread(
    ctx: &crate::feedback::FeedbackCtx,
    st: &Store,
    t: &Thread,
) -> artifax_core::Result<()> {
    let view = thread_view(st, t, ctx.codex_push(), false)?;
    ctx.events.publish(Event::Thread {
        artifact_id: t.artifact_id.clone(),
        thread: view,
    });
    Ok(())
}

/// The live session an agent reply or resolve comes from: `X-Artifax-Session`
/// is required, and must name a session that exists and has not ended.
///
/// # Errors
/// `unknown_session` otherwise.
fn agent_session(st: &Store, header: &Option<String>) -> artifax_core::Result<Session> {
    let sid = publishing_session(st, header)?.ok_or_else(|| {
        CoreError::invalid(
            "unknown_session",
            "agent replies, resolves, reopens, and deletes need X-Artifax-Session naming a live session",
        )
    })?;
    st.get_session(&sid)?.ok_or(CoreError::NotFound)
}

/// The thread `tid` if it belongs to the live artifact `id`.
fn thread_of(st: &Store, id: &ArtifactId, tid: &str) -> artifax_core::Result<Thread> {
    match st.get_thread(tid)? {
        Some(t) if t.artifact_id == id.as_str() => Ok(t),
        _ => Err(CoreError::NotFound),
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    include_resolved: bool,
    cursor: Option<String>,
    limit: Option<usize>,
}

pub async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let with_path = has_token(&headers, &s.token);
    let codex = s.feedback_ctx().codex_push();
    let limit = q.limit.unwrap_or(DEFAULT_THREAD_PAGE).clamp(1, 200);
    let (threads, next) = s
        .store_call(move |st| {
            let (ts, next) =
                st.list_threads(&id, q.include_resolved, q.cursor.as_deref(), limit)?;
            let views = ts
                .iter()
                .map(|t| thread_view(st, t, codex, with_path))
                .collect::<artifax_core::Result<Vec<_>>>()?;
            Ok((views, next))
        })
        .await?;
    Ok(Json(json!({"threads": threads, "next_cursor": next})))
}

pub async fn get(
    State(s): State<AppState>,
    headers: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let with_path = has_token(&headers, &s.token);
    let codex = s.feedback_ctx().codex_push();
    let view = s
        .store_call(move |st| thread_view(st, &thread_of(st, &id, &tid)?, codex, with_path))
        .await?;
    Ok(Json(json!({"thread": view})))
}

/// The clip PNG, sandboxed like `/_blob`.
pub async fn clip(
    State(s): State<AppState>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let bytes = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            if !t.has_clip {
                return Err(CoreError::NotFound);
            }
            Ok(std::fs::read(st.home().clip_path(&id, &t.id))?)
        })
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// Multipart fields: `anchor` (JSON), `body`, `version`, optional `clip` (PNG),
/// optional `via_page` (`true` when the page wrote the comment through the
/// `comments` capability: the comment is marked so, and an `@agent` mention in
/// it sends nothing). A clip that fails `clip_problem` is dropped and reported
/// as `clip_error`.
/// A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    aid: Result<Path<String>, PathRejection>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&path(aid)?)?;
    let mut mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let (mut anchor, mut text, mut version, mut clip) = (None, None, None, None);
    let mut via_page = false;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "anchor" => {
                anchor = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| multipart_error(e.status(), e.body_text()))?,
                )
            }
            "body" => {
                text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| multipart_error(e.status(), e.body_text()))?,
                )
            }
            "version" => {
                version = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| multipart_error(e.status(), e.body_text()))?,
                )
            }
            "via_page" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| multipart_error(e.status(), e.body_text()))?;
                via_page = match v.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(ApiError::bad_request(
                            "invalid_via_page",
                            "multipart field 'via_page' is true or false",
                        ));
                    }
                };
            }
            "clip" => {
                clip = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| multipart_error(e.status(), e.body_text()))?
                        .to_vec(),
                )
            }
            _ => {}
        }
    }
    let anchor: Anchor = serde_json::from_str(anchor.as_deref().ok_or_else(|| {
        ApiError::bad_request("invalid_anchor", "multipart field 'anchor' is required")
    })?)
    .map_err(|e| ApiError::bad_request("invalid_anchor", e.to_string()))?;
    let version_n: u32 = version
        .as_deref()
        .unwrap_or("")
        .trim()
        .parse()
        .map_err(|_| {
            ApiError::bad_request(
                "invalid_version",
                "multipart field 'version' must be a version number",
            )
        })?;
    let clip_error = clip.as_deref().and_then(clip_problem);
    let clip = if clip_error.is_some() { None } else { clip };
    let ctx = s.feedback_ctx();
    let with_path = has_token(&headers, &s.token);
    let view = s
        .store_call(move |st| {
            let author = author_name(st, viewer.0.as_deref())?;
            let body_text = text.unwrap_or_default();
            let mention = !via_page && mentions_agent(&body_text);
            let mut t = st.create_thread(
                &id,
                NewThread {
                    version_n,
                    anchor,
                    author_name: author,
                    body: body_text,
                    clip,
                    via_page,
                },
            )?;
            if mention {
                let (sent, touched) = st.send_to_agent(&t.id)?;
                t = sent;
                apply(&ctx, st, &touched);
            }
            publish_thread(&ctx, st, &t)?;
            thread_view(st, &t, ctx.codex_push(), with_path)
        })
        .await?;
    let mut out = json!({"thread": view});
    if let Some(e) = clip_error {
        out["clip_error"] = json!(e);
    }
    Ok((StatusCode::CREATED, Json(out)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentBody {
    body: String,
    #[serde(default)]
    author_kind: Option<String>,
    /// The page wrote the comment through the `comments` capability.
    #[serde(default)]
    via_page: bool,
}

enum Outcome {
    Guidance(&'static str),
    Done(Value),
}

fn respond(o: Outcome, created: StatusCode) -> Response {
    match o {
        Outcome::Guidance(g) => (StatusCode::OK, Json(json!({"guidance": g}))).into_response(),
        Outcome::Done(v) => (created, Json(v)).into_response(),
    }
}

/// A viewer comment (no token) or, with `author_kind: "agent"`, an agent reply
/// (token and `X-Artifax-Session` naming a live session, else 400
/// `unknown_session`; only on sent threads, otherwise guidance). A viewer
/// comment on a sent thread, or one mentioning `@agent`, is forwarded to the
/// agent; `via_page: true` marks a viewer comment the page wrote through the
/// `comments` capability, whose `@agent` mention sends nothing (400
/// `invalid_via_page` on an agent reply). A request with a foreign `Origin` is
/// refused ([`SameOrigin`]).
pub async fn comment(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<CommentBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let agent = match b.author_kind.as_deref() {
        None | Some("viewer") => false,
        Some("agent") => true,
        Some(k) => {
            return Err(ApiError::bad_request(
                "invalid_author_kind",
                format!("author_kind '{k}' is not viewer or agent"),
            ));
        }
    };
    if agent && b.via_page {
        return Err(ApiError::bad_request(
            "invalid_via_page",
            "via_page marks viewer comments only",
        ));
    }
    let authed = has_token(&headers, &s.token);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    let session = session_header(&headers)?;
    let ctx = s.feedback_ctx();
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            let mut touched = Touched::default();
            let c = if agent {
                let sess = agent_session(st, &session)?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_REPLY));
                }
                let c = st.add_comment(
                    &tid,
                    NewComment {
                        author_kind: AUTHOR_AGENT,
                        author_name: sess.harness,
                        via_session_id: Some(sess.id.clone()),
                        body: b.body,
                        via_page: false,
                    },
                )?;
                touched.merge(st.acknowledge(&sess.id, std::slice::from_ref(&tid))?);
                c
            } else {
                let name = author_name(st, viewer.0.as_deref())?;
                let c = st.add_comment(
                    &tid,
                    NewComment {
                        author_kind: AUTHOR_VIEWER,
                        author_name: name,
                        via_session_id: None,
                        body: b.body,
                        via_page: b.via_page,
                    },
                )?;
                if t.sent_to_agent || (!c.via_page && mentions_agent(&c.body)) {
                    touched.merge(st.send_to_agent(&tid)?.1);
                }
                c
            };
            ctx.events.publish(Event::Comment {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                comment: json!(c),
            });
            apply(&ctx, st, &touched);
            let t = thread_of(st, &id, &tid)?;
            publish_thread(&ctx, st, &t)?;
            let view = thread_view(st, &t, ctx.codex_push(), authed)?;
            Ok(Outcome::Done(json!({"comment": c, "thread": view})))
        })
        .await?;
    Ok(respond(o, StatusCode::CREATED))
}

/// Sets `sent_to_agent` and creates feedback rows; idempotent. A request with a
/// foreign `Origin` is refused ([`SameOrigin`]).
pub async fn send(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let ctx = s.feedback_ctx();
    let with_path = has_token(&headers, &s.token);
    let view = s
        .store_call(move |st| {
            thread_of(st, &id, &tid)?;
            let (t, touched) = st.send_to_agent(&tid)?;
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            thread_view(st, &t, ctx.codex_push(), with_path)
        })
        .await?;
    Ok(Json(json!({"thread": view})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    #[serde(rename = "as", default)]
    as_: Option<String>,
}

/// Resolves as the viewer (no token) or, with `{"as": "agent"}`, as the agent
/// (token and `X-Artifax-Session` naming a live session, else 400
/// `unknown_session`; only on sent threads, otherwise guidance). An empty body
/// resolves as the viewer. `resolved_by` is `viewer:<public ID>` (the viewer
/// row is created for a cookie seen for the first time), `viewer:anonymous`
/// without a cookie, or `agent:<harness>`: never the cookie or a session ID. Undelivered feedback on the thread is withdrawn; a
/// `feedback_state` event follows when delivered rows remain, and when none
/// remain the `thread` event carries `feedback_state: null`. A request with a
/// foreign `Origin` is refused ([`SameOrigin`]).
pub async fn resolve(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let agent = acting_as(&raw)?;
    let authed = has_token(&headers, &s.token);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    let session = session_header(&headers)?;
    let ctx = s.feedback_ctx();
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            let mut touched = Touched::default();
            let by = if agent {
                let sess = agent_session(st, &session)?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_RESOLVE));
                }
                touched.merge(st.acknowledge(&sess.id, std::slice::from_ref(&tid))?);
                format!("agent:{}", sess.harness)
            } else {
                match viewer.0.as_deref() {
                    Some(cookie) => format!("viewer:{}", st.upsert_viewer(cookie, None)?.public_id),
                    None => "viewer:anonymous".to_string(),
                }
            };
            let (t, withdrawn) = st.resolve_thread_touched(&tid, &by)?;
            touched.merge(withdrawn);
            ctx.events.publish(Event::ThreadResolved {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                resolved_by: t.resolved_by.clone().unwrap_or_default(),
                resolved_at: t.resolved_at.clone().unwrap_or_default(),
            });
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            let view = thread_view(st, &t, ctx.codex_push(), authed)?;
            Ok(Outcome::Done(json!({"thread": view})))
        })
        .await?;
    Ok(respond(o, StatusCode::OK))
}

/// `as` of a resolve or reopen body (or a delete query): `viewer` (the
/// default, also for an empty body) or `agent`, as `true`.
fn agent_as(as_: Option<&str>) -> Result<bool, ApiError> {
    match as_ {
        None | Some("viewer") => Ok(false),
        Some("agent") => Ok(true),
        Some(k) => Err(ApiError::bad_request(
            "invalid_resolver",
            format!("'as' is viewer or agent, not '{k}'"),
        )),
    }
}

/// `as` from a resolve or reopen body; see [`agent_as`].
fn acting_as(raw: &[u8]) -> Result<bool, ApiError> {
    let b: ResolveBody = if raw.is_empty() {
        ResolveBody::default()
    } else {
        serde_json::from_slice(raw)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    agent_as(b.as_.as_deref())
}

/// Reopening and deleting need caller level `interact` or above: the token
/// (the owner shell, or an agent, whose session is checked separately), or a
/// viewer cookie naming a viewer with a display name. Anyone else gets 403
/// `forbidden` asking them to set a name.
async fn require_interact(
    s: &AppState,
    authed: bool,
    cookie: Option<String>,
) -> Result<(), ApiError> {
    if authed {
        return Ok(());
    }
    let named = s
        .store_call(move |st| {
            Ok(match cookie {
                Some(c) => st.get_viewer(&c)?.is_some_and(|v| v.display_name.is_some()),
                None => false,
            })
        })
        .await?;
    if named {
        Ok(())
    } else {
        Err(ApiError::forbidden("forbidden", NAME_REQUIRED))
    }
}

/// Reopens a thread (status `open`, resolution cleared) as the viewer (a named
/// viewer, or the owner shell with the token) or, with `{"as": "agent"}`, as
/// the agent (token and `X-Artifax-Session` naming a live session, else 400
/// `unknown_session`; only on sent threads, otherwise guidance). An unnamed
/// viewer gets 403 `forbidden`. Answers `{thread}` and publishes the `thread`
/// event. A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn reopen(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let agent = acting_as(&raw)?;
    let authed = has_token(&headers, &s.token);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    require_interact(&s, authed, viewer.0).await?;
    let session = session_header(&headers)?;
    let ctx = s.feedback_ctx();
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            if agent {
                agent_session(st, &session)?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_REOPEN));
                }
            }
            let t = st.reopen_thread(&tid)?;
            publish_thread(&ctx, st, &t)?;
            Ok(Outcome::Done(
                json!({"thread": thread_view(st, &t, ctx.codex_push(), authed)?}),
            ))
        })
        .await?;
    Ok(respond(o, StatusCode::OK))
}

#[derive(Deserialize, Default)]
pub struct DeleteQuery {
    #[serde(rename = "as")]
    as_: Option<String>,
}

/// Deletes a thread with its comments, feedback rows, and clip, as the viewer
/// or, with `?as=agent`, as the agent (the same rules as [`reopen`], the level
/// check included). Answers `{deleted: true, thread_id}` and publishes
/// `thread_deleted`. A request with a foreign `Origin` is refused
/// ([`SameOrigin`]).
pub async fn delete(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let agent = agent_as(q.as_.as_deref())?;
    let authed = has_token(&headers, &s.token);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    require_interact(&s, authed, viewer.0).await?;
    let session = session_header(&headers)?;
    let ctx = s.feedback_ctx();
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            if agent {
                agent_session(st, &session)?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_DELETE));
                }
            }
            let (_, touched) = st.delete_thread_touched(&tid)?;
            ctx.events.publish(Event::ThreadDeleted {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
            });
            apply(&ctx, st, &touched);
            Ok(Outcome::Done(json!({"deleted": true, "thread_id": tid})))
        })
        .await?;
    Ok(respond(o, StatusCode::OK))
}

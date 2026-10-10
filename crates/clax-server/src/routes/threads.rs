//! Comment threads (spec §6 "Comments"): create with anchor and clip, comment,
//! send to the agent, resolve, list, and serve clips.

use super::artifacts::{body, parse_id, path, publishing_session};
use super::assets::multipart_error;
use crate::agent::{Agent, TokenOrAgent};
use crate::audit::DeferredAudit;
use crate::auth::Caller;
use crate::error::ApiError;
use crate::feedback::{apply, thread_view, thread_views};
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::{SameOrigin, author};
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::feedback::Touched;
use clax_core::model::{Session, Thread};
use clax_core::store::approvals::{OWNER, UNCONNECTED};
use clax_core::store::batches::SendBatch;
use clax_core::store::threads::{
    AUTHOR_AGENT, AUTHOR_VIEWER, DEFAULT_THREAD_PAGE, NewComment, NewThread, clip_problem,
};
use clax_core::working::StopReason;
use clax_core::{Anchor, ArtifactId, CoreError, Event, SendTarget, Store};
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
pub(crate) fn publish_thread(
    ctx: &crate::feedback::FeedbackCtx,
    st: &Store,
    t: &Thread,
) -> clax_core::Result<()> {
    let view = thread_view(st, t, ctx.codex_push(), false)?;
    ctx.events.publish(Event::Thread {
        artifact_id: t.artifact_id.clone(),
        thread: view,
    });
    Ok(())
}

/// The live session an agent reply or resolve comes from: the caller's
/// session (`X-Clax-Session`, or the path's) is required, and must exist,
/// not have ended, and be the caller's credential's (`grant`).
///
/// # Errors
/// `unknown_session` otherwise; `foreign_session` for another credential's.
fn agent_session(
    st: &Store,
    session: &Option<String>,
    grant: Option<&str>,
) -> clax_core::Result<Session> {
    let sid = publishing_session(st, session, grant)?.ok_or_else(|| {
        CoreError::invalid(
            "unknown_session",
            "agent replies, resolves, reopens, and deletes need X-Clax-Session naming a live session",
        )
    })?;
    st.get_session(&sid)?.ok_or(CoreError::NotFound)
}

/// Whether the caller of `a` may act as an agent (reply, resolve, reopen
/// or delete with `as: agent`), and skips the name a viewer needs to
/// reopen or delete: the daemon token or an agent of another machine. Its
/// session decides which agent ([`agent_session`]).
fn may_act_as_agent<N: crate::agent::Need>(a: &Agent<N>) -> bool {
    a.holds_token() || a.caller.is_agent()
}

/// The session and grant an agent action of `c` names.
fn acting(c: &Caller) -> (Option<String>, Option<String>) {
    let grant = match c {
        Caller::PersonAgent { grant, .. } => Some(grant.clone()),
        _ => None,
    };
    (c.session().map(str::to_string), grant)
}

/// The thread `tid` if it belongs to the live artifact `id`.
fn thread_of(st: &Store, id: &ArtifactId, tid: &str) -> clax_core::Result<Thread> {
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
    a: Agent<crate::agent::ThreadsList>,
    aid: Result<Path<String>, PathRejection>,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let with_path = a.holds_token();
    let codex = s.feedback_ctx().codex_push();
    let limit = q.limit.unwrap_or(DEFAULT_THREAD_PAGE).clamp(1, 200);
    let (threads, next) = s
        .store_call(move |st| {
            let (ts, next) =
                st.list_threads(&id, q.include_resolved, q.cursor.as_deref(), limit)?;
            Ok((thread_views(st, &ts, codex, with_path)?, next))
        })
        .await?;
    Ok(Json(json!({"threads": threads, "next_cursor": next})))
}

#[derive(Deserialize)]
pub struct AllQuery {
    #[serde(default)]
    include_resolved: bool,
}

/// `GET /api/threads[?include_resolved=true]` (token): every thread of every
/// live artifact, as `{artifacts: [{artifact_id, title, current_version,
/// files, threads}]}`. `files` names the current version's files; each
/// artifact's `threads` are thread views with `clip_path`, oldest first, and
/// an artifact with none is left out. Artifacts come in the order of
/// `GET /api/artifacts`.
pub async fn list_all(
    State(s): State<AppState>,
    _a: TokenOrAgent<crate::agent::ThreadsAll>,
    q: Result<Query<AllQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let codex = s.feedback_ctx().codex_push();
    // Every thread of the home: the bulk lane.
    let artifacts = s
        .store_call_bulk(move |st| {
            let mut out = Vec::new();
            for a in st.list_artifacts()? {
                let id = ArtifactId::parse(&a.id)?;
                let mut threads = Vec::new();
                let mut cursor: Option<String> = None;
                loop {
                    let (page, next) =
                        st.list_threads(&id, q.include_resolved, cursor.as_deref(), 200)?;
                    threads.extend(page);
                    match next {
                        Some(n) => cursor = Some(n),
                        None => break,
                    }
                }
                if threads.is_empty() {
                    continue;
                }
                let files: Vec<String> = st
                    .get_version(&id, a.current_version)?
                    .map(|v| v.files.into_keys().collect())
                    .unwrap_or_default();
                out.push(json!({
                    "artifact_id": a.id,
                    "title": a.title,
                    "current_version": a.current_version,
                    "files": files,
                    "threads": thread_views(st, &threads, codex, true)?,
                }));
            }
            Ok(out)
        })
        .await?;
    Ok(Json(json!({"artifacts": artifacts})))
}

pub async fn get(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadGet>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let with_path = a.holds_token();
    let codex = s.feedback_ctx().codex_push();
    let view = s
        .store_call(move |st| thread_view(st, &thread_of(st, &id, &tid)?, codex, with_path))
        .await?;
    Ok(Json(json!({"thread": view})))
}

/// The clip PNG, sandboxed like `/_blob`.
pub async fn clip(
    State(s): State<AppState>,
    _a: Agent<crate::agent::ThreadClip>,
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

/// Whether `@name` may send to and give a watch to live session `sid` on
/// artifact `id` (spec §11): always for the owner's agents; for a person's
/// agent only off live pages (`live_page`) and where its rights allow
/// reading and watching the artifact. §10's routing request for another
/// person's agent is not made here: such a name is reported unresolved.
fn named_agent_reaches(
    st: &Store,
    sid: &str,
    id: &ArtifactId,
    live_page: bool,
) -> clax_core::Result<bool> {
    use clax_core::rights::RightType;
    let Some(s) = st.get_session(sid)? else {
        return Ok(false);
    };
    let Some(grant) = s.grant_id.as_deref() else {
        return Ok(true);
    };
    if live_page {
        return Ok(false);
    }
    let gc = match st.grant_caller(grant, Some(sid)) {
        Ok(gc) => gc,
        Err(CoreError::Invalid { .. }) => return Ok(false),
        Err(e) => return Err(e),
    };
    let own = |x: &str| {
        st.artifact_created_by(x, gc.person.as_deref())
            .unwrap_or(false)
    };
    Ok(gc
        .rights
        .allows(RightType::Artifacts, "read", Some(id.as_str()), &own)
        && gc
            .rights
            .allows(RightType::Comments, "watch", Some(id.as_str()), &own))
}

/// The live agents a viewer comment's `body` names with `@name` on
/// artifact `id` (spec §11), each given a direct watch when it has none,
/// and the `@name`s that reached no live agent (`unresolved_mentions`):
/// those naming none, and those of an agent [`named_agent_reaches`]
/// refuses. A watch it makes is recorded under `audit`, the commenter's.
pub(crate) fn resolve_named(
    st: &Store,
    audit: &AuditCtx,
    id: &ArtifactId,
    body: &str,
    live_page: bool,
) -> clax_core::Result<(Vec<String>, Vec<String>)> {
    if !body.contains('@') {
        return Ok((Vec::new(), Vec::new()));
    }
    let live = st.live_agent_names()?;
    let names: Vec<String> = live.iter().map(|(_, n)| n.clone()).collect();
    let (mut sids, mut unresolved) = (Vec::new(), Vec::new());
    for name in clax_core::mentions::agent_mentions(body, &names) {
        let Some((sid, _)) = live.iter().find(|(_, n)| *n == name) else {
            continue;
        };
        if !named_agent_reaches(st, sid, id, live_page)? {
            unresolved.push(name);
            continue;
        }
        st.ensure_watch(audit, sid, id)?;
        sids.push(sid.clone());
    }
    if !clax_core::mentions::unresolved_mentions(body, &names, &[]).is_empty() {
        let viewers = st.viewer_display_names()?;
        unresolved.extend(clax_core::mentions::unresolved_mentions(
            body, &names, &viewers,
        ));
    }
    Ok((sids, unresolved))
}

/// Creates the thread `t` on `id`, sends it when its body mentions
/// `@agent` or a live agent's `@name` (unless the page wrote it), publishes
/// the `thread` event, and returns the thread view (with `clip_path` when
/// `with_path`) and the `@name`s that reached no agent. Both are recorded under `audit`.
pub(crate) fn create_thread_now(
    st: &Store,
    ctx: &crate::feedback::FeedbackCtx,
    audit: &AuditCtx,
    id: &ArtifactId,
    t: NewThread,
    live_page: bool,
    with_path: bool,
) -> clax_core::Result<(Value, Vec<String>)> {
    let mention = !t.via_page && mentions_agent(&t.body);
    let named = (!t.via_page).then(|| t.body.clone());
    let thread = st.create_thread(audit, id, t)?;
    announce_new_thread(
        st,
        ctx,
        audit,
        thread,
        mention,
        named.as_deref(),
        live_page,
        with_path,
    )
}

/// What follows writing a new thread: sends it to the agent when its first
/// comment `mention`s one, and to each live agent `named` (the comment's
/// body, `None` for a page's) names with `@name` ([`resolve_named`]),
/// announces it, and answers its view and the `@name`s that reached no
/// agent. Sends are recorded under `audit`.
pub(crate) fn announce_new_thread(
    st: &Store,
    ctx: &crate::feedback::FeedbackCtx,
    audit: &AuditCtx,
    mut thread: Thread,
    mention: bool,
    named: Option<&str>,
    live_page: bool,
    with_path: bool,
) -> clax_core::Result<(Value, Vec<String>)> {
    if mention {
        let (sent, touched) = st.send_to_agent(audit, &thread.id)?;
        thread = sent;
        apply(ctx, st, &touched);
    }
    let mut unresolved = Vec::new();
    if let Some(body) = named {
        let id = ArtifactId::parse(&thread.artifact_id)?;
        let (sids, u) = resolve_named(st, audit, &id, body, live_page)?;
        unresolved = u;
        if !sids.is_empty() {
            let (sent, touched) = st.send_to_named(&thread.id, &sids)?;
            thread = sent;
            apply(ctx, st, &touched);
        }
    }
    publish_thread(ctx, st, &thread)?;
    Ok((
        thread_view(st, &thread, ctx.codex_push(), with_path)?,
        unresolved,
    ))
}

/// Multipart fields: `anchor` (JSON), `body`, `version`, optional `clip` (PNG),
/// optional `via_page` (`true` when the page wrote the comment through the
/// `comments` capability: the comment is marked so, and an `@agent` mention in
/// it sends nothing). A clip that fails `clip_problem` is dropped and reported
/// as `clip_error`. An `artifact` anchor (the whole artifact) with `via_page`
/// is refused (`invalid_anchor`): a page anchors on itself.
/// A request with a foreign `Origin` is refused ([`SameOrigin`]).
/// 409 `merged_away` for a live page a join merged into another (spec
/// §7.2): its threads are the other page's now, so a new one goes there.
/// The error names it: `merged_into` (its artifact ID) and `page_url`.
async fn refuse_merged_away(s: &AppState, aid: &str) -> Result<(), ApiError> {
    let a = aid.to_string();
    let into: Option<(String, Option<String>)> = s
        .store_call(move |st| {
            let Some((_, _, Some(into))) = st.merged_live_page(&a)? else {
                return Ok(None);
            };
            let page = st.live_page_of(&clax_core::ArtifactId::parse(&into)?)?;
            Ok(Some((
                into,
                page.map(|p| format!("{}{}", p.origin, p.path)),
            )))
        })
        .await?;
    let Some((into, page_url)) = into else {
        return Ok(());
    };
    let mut e = ApiError::new(
        StatusCode::CONFLICT,
        "merged_away",
        format!(
            "this page was merged into {}; comment there",
            page_url.as_deref().unwrap_or(&into)
        ),
    );
    e.extra.insert("merged_into".into(), json!(into));
    e.extra.insert("page_url".into(), json!(page_url));
    Err(e)
}

pub async fn create(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadsCreate>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&path(aid)?)?;
    refuse_merged_away(&s, id.as_str()).await?;
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
    if via_page && anchor.is_whole() {
        return Err(ApiError::bad_request(
            "invalid_anchor",
            "a page's comment points at the page; a comment on the whole artifact is the viewer's own",
        ));
    }
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
    let with_path = a.holds_token();
    let live_page = s.live_ids.contains(id.as_str());
    let (view, unresolved) = s
        .store_call(move |st| {
            let audit = audit.resolve(st)?;
            let (author_name, author_public_id) = author(st, &who)?;
            create_thread_now(
                st,
                &ctx,
                &audit,
                &id,
                NewThread {
                    author_public_id,
                    version_n,
                    anchor,
                    author_name,
                    body: text.unwrap_or_default(),
                    clip,
                    via_page,
                },
                live_page,
                with_path,
            )
        })
        .await?;
    let mut out = json!({"thread": view});
    if let Some(e) = clip_error {
        out["clip_error"] = json!(e);
    }
    if !unresolved.is_empty() {
        out["unresolved_mentions"] = json!(unresolved);
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
    /// An agent reply on a live page says the page now shows the fix: the
    /// thread is linked to the page's next snapshot (spec 2026-10-05 L11).
    #[serde(default)]
    addressed: bool,
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
/// (token and `X-Clax-Session` naming a live session, else 400
/// `unknown_session`; only on sent threads, otherwise guidance). A viewer
/// comment on a sent thread, or one mentioning `@agent`, is forwarded to the
/// agent; one naming live agents with `@name` goes to them (spec §11), and
/// the answer's `unresolved_mentions` lists the `@name`s that reached none; `via_page: true` marks a viewer comment the page wrote through the
/// `comments` capability, whose `@agent` mention sends nothing (400
/// `invalid_via_page` on an agent reply). `addressed: true` on an agent reply
/// to a live page's thread records a pending address, linked to the page's
/// next snapshot, and the answer carries `addressed: "pending"`; on a viewer
/// comment or another artifact it is 400 `invalid_args`, before anything is
/// written. A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn comment(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadComment>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
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
    let addressed = b.addressed;
    if addressed && !agent {
        return Err(ApiError::bad_request(
            "invalid_args",
            "addressed is only for agent replies",
        ));
    }
    if addressed && !s.live_ids.contains(id.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_args",
            "addressed is for live pages; publish with addresses instead",
        ));
    }
    if agent && !may_act_as_agent(&a) {
        return Err(ApiError::unauthorized());
    }
    if !agent {
        // Not an agent reply: writing a comment.
        a.also_needs(&s, (clax_core::rights::RightType::Comments, "write"), &id)
            .await?;
    }
    let with_path = a.holds_token();
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let live_page = s.live_ids.contains(id.as_str());
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            let mut touched = Touched::default();
            let mut unresolved = Vec::new();
            let c = if agent {
                let sess = agent_session(st, &session, grant.as_deref())?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_REPLY));
                }
                let audit = audit.resolve(st)?;
                let reply = NewComment {
                    author_public_id: None,
                    author_kind: AUTHOR_AGENT,
                    author_name: sess.harness.clone(),
                    via_session_id: Some(sess.id.clone()),
                    body: b.body,
                    via_page: false,
                };
                let c = if addressed {
                    st.add_addressed_reply(&audit, &id, &tid, reply, &sess.harness)?
                } else {
                    st.add_comment(&audit, &tid, reply)?
                };
                touched.merge(st.acknowledge(&audit, &sess.id, std::slice::from_ref(&tid))?);
                let changed =
                    ctx.working
                        .thread_done(&sess.id, id.as_str(), &tid, StopReason::Explicit);
                crate::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
                c
            } else {
                let audit = audit.resolve(st)?;
                let (name, author_public_id) = author(st, &who)?;
                let c = st.add_comment(
                    &audit,
                    &tid,
                    NewComment {
                        author_public_id,
                        author_kind: AUTHOR_VIEWER,
                        author_name: name,
                        via_session_id: None,
                        body: b.body,
                        via_page: b.via_page,
                    },
                )?;
                let (named, u) = if c.via_page {
                    (Vec::new(), Vec::new())
                } else {
                    resolve_named(st, &audit, &id, &c.body, live_page)?
                };
                unresolved = u;
                // A comment naming agents goes to them, not to the thread's
                // target; `@agent` keeps its meaning beside them.
                let at_agent = !c.via_page && mentions_agent(&c.body);
                if at_agent || (t.sent_to_agent && named.is_empty()) {
                    touched.merge(st.send_to_agent(&audit, &tid)?.1);
                }
                if !named.is_empty() {
                    touched.merge(st.send_to_named(&tid, &named)?.1);
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
            let view = thread_view(st, &t, ctx.codex_push(), with_path)?;
            let mut out = json!({"comment": c, "thread": view});
            if addressed {
                out["addressed"] = json!("pending");
            }
            if !unresolved.is_empty() {
                out["unresolved_mentions"] = json!(unresolved);
            }
            Ok(Outcome::Done(out))
        })
        .await?;
    Ok(respond(o, StatusCode::CREATED))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SendBody {
    #[serde(default)]
    to: Option<String>,
}

/// The live owner or watcher session of artifact `id` that agent handle `to`
/// names; 400 `unknown_agent` when it names none.
fn send_target(st: &Store, id: &ArtifactId, to: Option<&str>) -> clax_core::Result<Option<String>> {
    match to {
        Some(h) if clax_core::is_agent_handle(h) => {
            Ok(Some(st.live_agent(id, h)?.ok_or_else(|| {
                CoreError::invalid(
                    "unknown_agent",
                    "no live agent on this artifact has that handle",
                )
            })?))
        }
        Some(_) => Err(CoreError::invalid("unknown_agent", "not an agent handle")),
        None => Ok(None),
    }
}

/// Sets `sent_to_agent` and creates feedback rows; idempotent. An optional
/// body `{"to": <agent handle>}` sends to that live agent only, and it becomes
/// the thread's target (400 `unknown_agent` when it names no live owner or
/// watcher). Without `to` the send goes to every live owner and watcher and
/// clears the thread's target. A request with a foreign `Origin` is refused
/// ([`SameOrigin`]).
pub async fn send(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadSend>,
    _o: SameOrigin,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let b: SendBody = if raw.is_empty() {
        SendBody::default()
    } else {
        serde_json::from_slice(&raw)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    let ctx = s.feedback_ctx();
    let with_path = a.holds_token();
    let view = s
        .store_call(move |st| {
            thread_of(st, &id, &tid)?;
            let to = send_target(st, &id, b.to.as_deref())?;
            let audit = audit.resolve(st)?;
            let (t, touched) = st.send_to(
                &audit,
                &tid,
                to.as_deref()
                    .map_or(SendTarget::Everyone, SendTarget::Agent),
            )?;
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            thread_view(st, &t, ctx.codex_push(), with_path)
        })
        .await?;
    Ok(Json(json!({"thread": view})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    thread_ids: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

/// `POST /api/artifacts/<aid>/threads:send`: sends several threads to the
/// agent as one batch ([`Store::send_batch`]), all or nothing, to the agent
/// `to` names or, without it, to every live owner and watcher. The same
/// access as the single send (no token; a foreign `Origin` is refused); the
/// sender is the viewer the request speaks for ([`Identity`]: the owner for
/// an owner credential, else the viewer cookie's). One fan-out for the whole batch, so every
/// tier hands its rows over together.
pub async fn send_batch(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadsSendBatch>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<BatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let ctx = s.feedback_ctx();
    let with_path = a.holds_token();
    let agent_person = match &a.caller {
        crate::auth::Caller::PersonAgent { person, .. } => Some(person.clone()),
        _ => None,
    };
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let to = send_target(st, &id, b.to.as_deref())?;
            let audit = audit.resolve(st)?;
            let (sent_by, viewer) = author(st, &who)?;
            // The sender as an author of the note (spec §8.2).
            let sender = match agent_person {
                Some(person) => person.unwrap_or_else(|| OWNER.to_string()),
                None if who.is_owner() => OWNER.to_string(),
                None => viewer.unwrap_or_else(|| UNCONNECTED.to_string()),
            };
            let r = st.send_batch(&audit, &id, SendBatch { thread_ids: b.thread_ids, note: b.note, sent_by, sender, to })?;
            apply(&ctx, st, &r.touched);
            let sent = r
                .sent
                .iter()
                .map(|tid| thread_of(st, &id, tid))
                .collect::<clax_core::Result<Vec<_>>>()?;
            let public = thread_views(st, &sent, ctx.codex_push(), false)?;
            for (t, view) in sent.iter().zip(&public) {
                ctx.events.publish(Event::Thread {
                    artifact_id: t.artifact_id.clone(),
                    thread: view.clone(),
                });
            }
            let views = if with_path {
                thread_views(st, &sent, ctx.codex_push(), true)?
            } else {
                public
            };
            Ok(json!({"batch": r.batch, "sent": r.sent, "unchanged": r.unchanged, "threads": views}))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    #[serde(rename = "as", default)]
    as_: Option<String>,
}

/// Resolves as the viewer (no token) or, with `{"as": "agent"}`, as the agent
/// (token and `X-Clax-Session` naming a live session, else 400
/// `unknown_session`; only on sent threads, otherwise guidance). An empty body
/// resolves as the viewer. `resolved_by` is `viewer:<public ID>` (the
/// owner's for an owner credential; else the cookie's viewer, its row created
/// for a cookie seen for the first time), `viewer:anonymous` without either,
/// or `agent:<harness>`: never the cookie or a session ID. Undelivered feedback on the thread is withdrawn; a
/// `feedback_state` event follows when delivered rows remain, and when none
/// remain the `thread` event carries `feedback_state: null`. An agent resolve
/// lists a thread no version lists yet as addressed: in the artifact's current
/// version, or, on a live page, in the page's next snapshot (a pending
/// address). A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn resolve(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadResolve>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let agent = acting_as(&raw)?;
    if agent && !may_act_as_agent(&a) {
        return Err(ApiError::unauthorized());
    }
    let with_path = a.holds_token();
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let live = s.live_ids.contains(id.as_str());
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            let mut touched = Touched::default();
            let mut resolver: Option<Session> = None;
            let by = if agent {
                let sess = agent_session(st, &session, grant.as_deref())?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_RESOLVE));
                }
                let by = format!("agent:{}", sess.harness);
                resolver = Some(sess);
                by
            } else {
                match who.ensure_viewer(st)? {
                    Some(v) => format!("viewer:{}", v.public_id),
                    None => "viewer:anonymous".to_string(),
                }
            };
            let audit = audit.resolve(st)?;
            if let Some(sess) = &resolver {
                touched.merge(st.acknowledge(&audit, &sess.id, std::slice::from_ref(&tid))?);
            }
            let (t, withdrawn) = match &resolver {
                Some(sess) => {
                    // Whose agent resolved it: its session's person.
                    let person = sess.person.as_deref().unwrap_or(OWNER);
                    st.resolve_thread_addressed(
                        &audit,
                        &id,
                        &tid,
                        &by,
                        &sess.harness,
                        live,
                        Some(person),
                    )?
                }
                None => st.resolve_thread_touched(&audit, &tid, &by)?,
            };
            touched.merge(withdrawn);
            let changed = match &resolver {
                Some(sess) => {
                    ctx.working
                        .thread_done(&sess.id, id.as_str(), &tid, StopReason::Resolved)
                }
                None => ctx
                    .working
                    .thread_gone(id.as_str(), &tid, StopReason::Resolved),
            };
            crate::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
            ctx.events.publish(Event::ThreadResolved {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                resolved_by: t.resolved_by.clone().unwrap_or_default(),
                resolved_at: t.resolved_at.clone().unwrap_or_default(),
            });
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            let view = thread_view(st, &t, ctx.codex_push(), with_path)?;
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
/// (the owner shell, the CLI, or an agent, whose session is checked
/// separately), or a request whose viewer ([`Identity::viewer`]) has a
/// display name. Anyone else gets 403 `forbidden` asking them to set a name.
/// Reads the viewer row without making one.
async fn require_interact(s: &AppState, authed: bool, who: Identity) -> Result<(), ApiError> {
    if authed {
        return Ok(());
    }
    let named = s
        .store_call(move |st| Ok(who.viewer(st)?.is_some_and(|v| v.display_name.is_some())))
        .await?;
    if named {
        Ok(())
    } else {
        Err(ApiError::forbidden("forbidden", NAME_REQUIRED))
    }
}

/// Reopens a thread (status `open`, resolution cleared) as the viewer (a named
/// viewer, or the owner shell with the token) or, with `{"as": "agent"}`, as
/// the agent (token and `X-Clax-Session` naming a live session, else 400
/// `unknown_session`; only on sent threads, otherwise guidance). An unnamed
/// viewer gets 403 `forbidden`. Answers `{thread}` and publishes the `thread`
/// event. A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn reopen(
    State(s): State<AppState>,
    a: Agent<crate::agent::ThreadReopen>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let agent = acting_as(&raw)?;
    let authed = may_act_as_agent(&a);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    require_interact(&s, authed, who).await?;
    let with_path = a.holds_token();
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            if agent {
                agent_session(st, &session, grant.as_deref())?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_REOPEN));
                }
            }
            let audit = audit.resolve(st)?;
            let t = st.reopen_thread(&audit, &tid)?;
            publish_thread(&ctx, st, &t)?;
            Ok(Outcome::Done(
                json!({"thread": thread_view(st, &t, ctx.codex_push(), with_path)?}),
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
    a: Agent<crate::agent::ThreadDelete>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let agent = agent_as(q.as_.as_deref())?;
    let authed = may_act_as_agent(&a);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    require_interact(&s, authed, who).await?;
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let o = s
        .store_call(move |st| {
            let t = thread_of(st, &id, &tid)?;
            if agent {
                agent_session(st, &session, grant.as_deref())?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_DELETE));
                }
            }
            let audit = audit.resolve(st)?;
            let (_, touched) = st.delete_thread_touched(&audit, &tid)?;
            let changed = ctx
                .working
                .thread_gone(id.as_str(), &tid, StopReason::Deleted);
            crate::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
            ctx.events.publish(Event::ThreadDeleted {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                moved: false,
            });
            apply(&ctx, st, &touched);
            Ok(Outcome::Done(json!({"deleted": true, "thread_id": tid})))
        })
        .await?;
    Ok(respond(o, StatusCode::OK))
}

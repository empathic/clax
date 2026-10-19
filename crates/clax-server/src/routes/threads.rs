//! Comment threads (spec §6 "Comments"): create with anchor and clip, comment,
//! send to the agent, resolve, list, and serve clips.
//!
//! Every write (create, comment, send, batch send, resolve, reopen, delete)
//! by a person beyond the artifact's live seats is 403 `spectator`
//! ([`clax_http::seats::require_seat`]; spec 2026-10-11-clax-dev-hosted-design
//! §7, §16.1); agents and the owner are never refused it.

use super::artifacts::{body, path, publishing_session};
use super::assets::multipart_error;
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::feedback::Touched;
use clax_core::model::{Comment, Session, Thread};
use clax_core::visibility::{OWNER, UNCONNECTED};
use clax_core::working::StopReason;
use clax_core::{Anchor, ArtifactId, CoreError, Event};
use clax_http::agent::{Agent, TokenOrAgent, person_agent_session};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::{SameOrigin, acting_viewer, author};
use clax_http::views::thread::{VisFor, thread_event_views};
use clax_realtime::feedback::apply;
use clax_store::batches::SendBatch;
use clax_store::linked::{CommentKey, Keyed};
use clax_store::threads::{
    AUTHOR_AGENT, AUTHOR_VIEWER, DEFAULT_THREAD_PAGE, NewComment, NewThread, clip_problem,
};
use clax_store::{SendTarget, Store};
use serde::Deserialize;
use serde_json::{Value, json};

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

/// Broadcasts the `thread` event. It reaches every subscriber, so its view
/// ([`clax_http::views::thread::thread_event_views`]) has no `clip_path` and no
/// delivery state, whoever made the change.
pub(crate) fn publish_thread(
    ctx: &clax_realtime::feedback::FeedbackCtx,
    st: &Store,
    t: &Thread,
) -> clax_core::Result<()> {
    let view = thread_event_views(st, std::slice::from_ref(t), ctx.codex_push())?
        .pop()
        .expect("one view per thread");
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
fn may_act_as_agent<N: clax_http::agent::Need>(a: &Agent<N>) -> bool {
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

/// An agent token's grant and the idempotency key its comment or new
/// thread names (`Idempotency-Key`, as a publish's): `None` without one,
/// and for every other caller, whose writes are as before.
///
/// # Errors
/// 400 `invalid_args` for a key that is not one.
fn comment_key(
    c: &Caller,
    headers: &axum::http::HeaderMap,
) -> Result<Option<(String, String)>, ApiError> {
    let Caller::PersonAgent { grant, .. } = c else {
        return Ok(None);
    };
    Ok(super::artifacts::publish_key(headers, Some(grant))?.map(|k| (grant.clone(), k)))
}

/// What a person's agent's comment needs (spec §6.7): a session, since a
/// person's own comments are written in their browser and agent replies
/// come from agent sessions (400 `no_session`); and no `via_page`, which
/// only a page's browser sets (400 `invalid_via_page`). Other callers pass.
fn person_agent_comments(c: &Caller, via_page: bool) -> Result<(), ApiError> {
    person_agent_session(c)?;
    if via_page && matches!(c, Caller::PersonAgent { .. }) {
        return Err(clax_http::agent::invalid_via_page());
    }
    Ok(())
}

/// For a person's agent, that it may read the threads of `id` it writes
/// to: an agent replies to, resolves, reopens or sends only threads it
/// can list (403 `not_permitted`).
async fn sees_threads<N: clax_http::agent::Need>(
    a: &Agent<N>,
    s: &AppState,
    id: &ArtifactId,
) -> Result<(), ApiError> {
    a.also_needs(s, (clax_core::rights::RightType::Comments, "read"), id)
        .await
}

/// The thread `tid` if it belongs to the live artifact `id`.
fn thread_of(st: &Store, id: &ArtifactId, tid: &str) -> clax_core::Result<Thread> {
    match st.get_thread(tid)? {
        Some(t) if t.artifact_id == id.as_str() => Ok(t),
        _ => Err(CoreError::NotFound),
    }
}

/// Most threads one `GET /api/artifacts/{aid}/approves-first` names.
pub const MAX_FIRSTS_THREADS: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstsQuery {
    /// Thread IDs, comma-separated.
    #[serde(default)]
    threads: Option<String>,
}

/// `GET /api/artifacts/{aid}/approves-first?threads=<tid,…>` → `{approves_first:
/// [<public ID> | null]}`: the people a send of those threads naming one of
/// their agents asks first (spec 2026-10-08 §10, "Sam approves first";
/// `null` is the owner), among the people with a live agent on the
/// artifact ([`Store::approves_first`]), for the Send menu as it opens.
/// The owner learns every such person; a connected person, only their own
/// approvals and others' of content they authored (§18, "Only about their
/// content"). A LAN viewer, an anonymous caller and agents get 403
/// `forbidden`. A thread
/// of another artifact or none is 404; more than [`MAX_FIRSTS_THREADS`]
/// is 400 `invalid_query`. A request with a foreign `Origin` is refused.
pub async fn approves_first(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<FirstsQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let asker = clax_http::views::inbox::recipient_of(&s, &who)
        .await
        .map_err(|_| {
            ApiError::forbidden(
                "forbidden",
                "only the owner or a connected person sees who approves first",
            )
        })?;
    let id = parse_id(&path(p)?)?;
    let q = q
        .map(|Query(q)| q)
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let ids: Vec<String> = q
        .threads
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    if ids.len() > MAX_FIRSTS_THREADS {
        return Err(ApiError::bad_request(
            "invalid_query",
            format!("at most {MAX_FIRSTS_THREADS} threads"),
        ));
    }
    let v = s
        .store_call(move |st| {
            let threads = ids
                .iter()
                .map(|tid| thread_of(st, &id, tid))
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({"approves_first": st.approves_first(&id, &threads, asker.as_deref())?}))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    include_resolved: bool,
    cursor: Option<String>,
    limit: Option<usize>,
    /// An agent token's: only threads with activity (made, commented on or
    /// resolved) at or after this time, as the daemon writes times.
    since: Option<String>,
}

/// The newest time thread `t` was made, commented on or resolved.
pub fn activity_of(t: &Thread) -> &str {
    let mut at = t.created_at.as_str();
    for c in &t.comments {
        if c.created_at.as_str() > at {
            at = &c.created_at;
        }
    }
    if let Some(r) = t.resolved_at.as_deref()
        && r > at
    {
        at = r;
    }
    at
}

pub async fn list(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadsList>,
    aid: Result<Path<String>, PathRejection>,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    if q.since.is_some() && !a.caller.is_agent() {
        return Err(ApiError::bad_request(
            "invalid_query",
            "since is for an agent token's reads",
        ));
    }
    let with_path = a.holds_token();
    let codex = s.feedback_ctx().codex_push();
    let limit = q.limit.unwrap_or(DEFAULT_THREAD_PAGE).clamp(1, 200);
    let vf = VisFor::of(&a.caller, &s);
    let (threads, next) = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            let (mut ts, next) =
                st.list_threads(&id, q.include_resolved, q.cursor.as_deref(), limit)?;
            if let Some(since) = q.since.as_deref() {
                ts.retain(|t| activity_of(t) >= since);
            }
            Ok((vis.filter_threads(st, &ts, codex, with_path)?, next))
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
/// `GET /api/artifacts`. For the owner's agents each field goes through
/// their visibility ([`clax_http::views::thread::Vis`]): a withheld title is null
/// with `withheld: {authors}`, withheld file names are left out and counted
/// in `files_withheld`, and the threads are filtered as everywhere.
pub async fn list_all(
    State(s): State<AppState>,
    TokenOrAgent(agent): TokenOrAgent<clax_http::agent::ThreadsAll>,
    q: Result<Query<AllQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let codex = s.feedback_ctx().codex_push();
    let vf = VisFor::of(&agent.caller, &s);
    // Every thread of the home: the bulk lane.
    let artifacts = s
        .store_call_bulk(move |st| {
            let vis = vf.build(st)?;
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
                let version = st.get_version(&id, a.current_version)?;
                let files: Vec<&String> = version
                    .as_ref()
                    .map(|v| v.files.keys().collect())
                    .unwrap_or_default();
                let mut entry = json!({
                    "artifact_id": a.id,
                    "title": a.title,
                    "current_version": a.current_version,
                    "files": files,
                    "threads": vis.filter_threads(st, &threads, codex, true)?,
                });
                vis.filter_listing_entry(st, &a, version.as_ref(), &mut entry)?;
                out.push(entry);
            }
            Ok(out)
        })
        .await?;
    Ok(Json(json!({"artifacts": artifacts})))
}

pub async fn get(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadGet>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let with_path = a.holds_token();
    let codex = s.feedback_ctx().codex_push();
    let vf = VisFor::of(&a.caller, &s);
    let view = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            vis.filter_thread(st, &thread_of(st, &id, &tid)?, codex, with_path)
        })
        .await?;
    Ok(Json(json!({"thread": view})))
}

/// `GET /_clip/<aid>/<tid>?k=<key>` on a hosted daemon: thread `tid`'s
/// clip PNG at the address agents of other machines read it at
/// ([`clax_http::hosted::clip_link`]). The hosted gate has checked the key,
/// which only a thread view that shows the clip carries, so no credential
/// is asked for (a request carrying a grant's is refused, as on `/_blob`),
/// and a live page's clip is 404 to a caller who does not see live pages;
/// the PNG never changes, so it is cached as immutable, but privately, so
/// no shared cache keeps it once its thread is deleted or restricted. A
/// local daemon serves no such address: 404.
pub async fn keyed_clip(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    sees: clax_http::live_access::SeesLive,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    if s.hosted.is_none() {
        return Err(ApiError::not_found());
    }
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let live_ids = s.live_ids.clone();
    let bytes = s
        .store_call_async(move |st| async move {
            let st = &*st;
            if sees.check(&live_ids, id.as_str()).is_err() {
                return Err(CoreError::NotFound);
            }
            let t = thread_of(st, &id, &tid)?;
            if !t.has_clip {
                return Err(CoreError::NotFound);
            }
            st.read_clip_async(&id, &t.id)
                .await?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// The clip PNG, sandboxed like `/_blob`. A clip an agent's visibility
/// withholds (spec §8.3) is 403 `withheld`, naming its `authors`: none of
/// it is read.
pub async fn clip(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadClip>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    let vf = VisFor::of(&a.caller, &s);
    let bytes = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let vis = vf.build(st)?;
            let t = thread_of(st, &id, &tid)?;
            if !t.has_clip {
                return Err(CoreError::NotFound);
            }
            if let Some(authors) = vis.filter_clip(st, &t)? {
                return Ok(Err(authors));
            }
            Ok(Ok(st
                .read_clip_async(&id, &t.id)
                .await?
                .ok_or(CoreError::NotFound)?))
        })
        .await?;
    let bytes = match bytes {
        Ok(b) => b,
        Err(authors) => {
            let mut e = ApiError::forbidden(
                "withheld",
                "this clip is withheld until your person approves its authors",
            );
            e.extra.insert("authors".into(), json!(authors));
            return Err(e);
        }
    };
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

/// Whether a send or `@name` reaches live session `sid` on artifact `id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    /// It may read and watch the artifact.
    Yes,
    /// Its rights do not let it read the artifact (spec §10: 400
    /// `agent_cannot_read`).
    CannotRead,
    /// It may read it but not watch it, or it is a person's agent and the
    /// artifact is a live page (which no person's agent sees, L10), or its
    /// grant has ended: an `@name` is reported unresolved.
    No,
}

/// Whether a send or `@name` reaches live session `sid` on artifact `id`
/// (spec §10, §11): always for the owner's agents; for a person's agent
/// only off live pages (`live_page`) and where its rights allow reading
/// and watching the artifact. Reaching another person's agent makes a
/// routing request (M18), which the store's send does.
fn named_agent_reaches(
    st: &Store,
    sid: &str,
    id: &ArtifactId,
    live_page: bool,
) -> clax_core::Result<Reach> {
    use clax_core::rights::RightType;
    let Some(s) = st.get_session(sid)? else {
        return Ok(Reach::No);
    };
    let Some(grant) = s.grant_id.as_deref() else {
        return Ok(Reach::Yes);
    };
    if live_page {
        return Ok(Reach::No);
    }
    let gc = match st.grant_caller(grant, Some(sid)) {
        Ok(gc) => gc,
        Err(CoreError::Invalid { .. }) => return Ok(Reach::No),
        Err(e) => return Err(e),
    };
    let own = |x: &str| {
        st.artifact_created_by(x, gc.person.as_deref())
            .unwrap_or(false)
    };
    if !gc
        .rights
        .allows(RightType::Artifacts, "read", Some(id.as_str()), &own)
    {
        return Ok(Reach::CannotRead);
    }
    Ok(
        if gc
            .rights
            .allows(RightType::Comments, "watch", Some(id.as_str()), &own)
        {
            Reach::Yes
        } else {
            Reach::No
        },
    )
}

/// 400 `agent_cannot_read`: a send to an agent that may not read the
/// artifact (spec §10).
fn cannot_read(name: &str) -> CoreError {
    CoreError::invalid(
        "agent_cannot_read",
        format!(
            "{} may not read this artifact; nothing was sent",
            clax_core::feedback::display_name(name)
        ),
    )
}

/// 400 `agent_cannot_read` when a viewer comment's `body` names with
/// `@name` a live agent that may not read artifact `id`: checked before
/// the comment is written, so a refused send writes nothing.
pub(crate) fn check_named_reads(
    st: &Store,
    id: &ArtifactId,
    body: &str,
    live_page: bool,
) -> clax_core::Result<()> {
    if !body.contains('@') {
        return Ok(());
    }
    let live = st.live_agent_names()?;
    let names: Vec<String> = live.iter().map(|(_, n)| n.clone()).collect();
    for name in clax_core::mentions::agent_mentions(body, &names) {
        if let Some((sid, _)) = live.iter().find(|(_, n)| *n == name)
            && named_agent_reaches(st, sid, id, live_page)? == Reach::CannotRead
        {
            return Err(cannot_read(&name));
        }
    }
    Ok(())
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
        if named_agent_reaches(st, sid, id, live_page)? != Reach::Yes {
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
/// the `thread` event, and returns the thread and the `@name`s that reached
/// no agent. Both are recorded under `audit`.
///
/// An agent token's write keyed `key`
/// ([`clax_store::Store::create_thread_keyed_async`]) whose key is recorded
/// already writes nothing and answers the thread it made. Its sends and
/// announcement come after the thread's transaction, so the first write
/// may have ended before them: a thread still open and sent to no agent is
/// sent and announced now, and any other announced again.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_thread_keyed_now(
    st: &Store,
    ctx: &clax_realtime::feedback::FeedbackCtx,
    audit: &AuditCtx,
    id: &ArtifactId,
    t: NewThread,
    live_page: bool,
    hidden: &[String],
    key: Option<CommentKey<'_>>,
) -> clax_core::Result<(Thread, Vec<String>)> {
    let mention = !t.via_page && mentions_agent(&t.body);
    let named = (!t.via_page).then(|| t.body.clone());
    let thread = match st
        .create_thread_keyed_async(audit, id, t, hidden, key)
        .await?
    {
        Keyed::Made(t) => t,
        Keyed::Before { thread_id, .. } => {
            let t = thread_of(st, id, &thread_id)?;
            if t.status != "open" || t.sent_to_agent {
                publish_thread(ctx, st, &t)?;
                return Ok((t, Vec::new()));
            }
            t
        }
    };
    announce_new_thread(st, ctx, audit, thread, mention, named.as_deref(), live_page)
}

/// What follows writing a new thread: sends it to the agent when its first
/// comment `mention`s one, and to each live agent `named` (the comment's
/// body, `None` for a page's) names with `@name` ([`resolve_named`]),
/// announces it, and answers it as it is then and the `@name`s that
/// reached no agent. Sends are recorded under `audit`. The thread's `thread`
/// event goes out before its delivery states ([`apply`]), so every client
/// holds the thread when its state arrives.
pub(crate) fn announce_new_thread(
    st: &Store,
    ctx: &clax_realtime::feedback::FeedbackCtx,
    audit: &AuditCtx,
    mut thread: Thread,
    mention: bool,
    named: Option<&str>,
    live_page: bool,
) -> clax_core::Result<(Thread, Vec<String>)> {
    let mut all = clax_core::feedback::Touched::default();
    if mention {
        // `@agent` routes to every agent it reaches (M18).
        let (sent, touched, _) = st.send_routed(
            audit,
            &thread.id,
            SendTarget::Thread,
            clax_store::approvals::Route::Send,
            false,
        )?;
        thread = sent;
        all.merge(touched);
    }
    let mut unresolved = Vec::new();
    if let Some(body) = named {
        let id = ArtifactId::parse(&thread.artifact_id)?;
        let (sids, u) = resolve_named(st, audit, &id, body, live_page)?;
        unresolved = u;
        if !sids.is_empty() {
            let (sent, touched) = st.send_to_named(audit, &thread.id, &sids)?;
            thread = sent;
            all.merge(touched);
        }
    }
    publish_thread(ctx, st, &thread)?;
    apply(ctx, st, &all);
    Ok((thread, unresolved))
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
    a: Agent<clax_http::agent::ThreadsCreate>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    headers: axum::http::HeaderMap,
    aid: Result<Path<String>, PathRejection>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&path(aid)?)?;
    refuse_merged_away(&s, id.as_str()).await?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let key = comment_key(&a.caller, &headers)?;
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
    person_agent_comments(&a.caller, via_page)?;
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
    let vf = VisFor::of(&a.caller, &s);
    let (view, unresolved) = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let vis = vf.build(st)?;
            let audit = audit.resolve(st)?;
            let (author_name, author_public_id) = author(st, &who)?;
            // A file the reader may not see is, to it, a file the version
            // lacks (spec §8.3).
            let version = match vis.filters() {
                true => st.get_version(&id, version_n)?,
                false => None,
            };
            let hidden: Vec<String> = match version {
                Some(v) => v
                    .files
                    .keys()
                    .filter(|f| vis.filter_anchor_file(&v, f))
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            if !via_page {
                check_named_reads(st, &id, text.as_deref().unwrap_or_default(), live_page)?;
            }
            // The key is checked, and recorded, in the thread's own
            // transaction: a resend answers the thread the first write made
            // and does the sends that write may not have reached.
            let (thread, unresolved) = create_thread_keyed_now(
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
                &hidden,
                key.as_ref().map(|(g, k)| (g.as_str(), k.as_str())),
            )
            .await?;
            let view = vis.filter_thread(st, &thread, ctx.codex_push(), with_path)?;
            Ok((view, unresolved))
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

/// 409 `thread_unlinked` for a thread mirrored from a remote the artifact
/// is no longer linked to: it is read-only history there
/// ([`clax_store::linked`]).
async fn refuse_unlinked(s: &AppState, tid: &str) -> Result<(), ApiError> {
    if s.hosted.is_some() {
        return Ok(());
    }
    let t = tid.to_string();
    let lt = s.store_call(move |st| st.linked_thread(&t)).await?;
    match lt {
        Some(lt) if lt.unlinked_at.is_some() => Err(ApiError::new(
            StatusCode::CONFLICT,
            "thread_unlinked",
            format!(
                "this thread is history of {}, which this artifact is no longer linked to: it is read-only",
                lt.remote_name
            ),
        )),
        _ => Ok(()),
    }
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
    a: Agent<clax_http::agent::ThreadComment>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    headers: axum::http::HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<CommentBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let reads = a.read_rights();
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    refuse_unlinked(&s, &tid).await?;
    let b = body(req)?;
    let key = comment_key(&a.caller, &headers)?;
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
    person_agent_comments(&a.caller, b.via_page)?;
    if !agent {
        // Not an agent reply: writing a comment.
        a.also_needs(&s, (clax_core::rights::RightType::Comments, "write"), &id)
            .await?;
    }
    sees_threads(&a, &s, &id).await?;
    let with_path = a.holds_token();
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let live_page = s.live_ids.contains(id.as_str());
    let vf = VisFor::of(&a.caller, &s);
    let app = s.clone();
    let o = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            let t = thread_of(st, &id, &tid)?;
            let was_resolved = t.status == "resolved";
            let mut touched = Touched::default();
            let mut unresolved = Vec::new();
            // The key is checked, and recorded, in the comment's own
            // transaction. A resend finds the comment the first write made
            // and does again what follows it, which that write may not have
            // reached: an agent reply's acknowledgement, a viewer comment's
            // sends while it reached no agent.
            let key = key.as_ref().map(|(g, k)| (g.as_str(), k.as_str()));
            let replayed = |ktid: String, cid: String| -> clax_core::Result<Comment> {
                if ktid != tid {
                    return Err(CoreError::invalid(
                        "invalid_args",
                        "this Idempotency-Key made a comment on another thread",
                    ));
                }
                thread_of(st, &id, &tid)?
                    .comments
                    .into_iter()
                    .find(|c| c.id == cid)
                    .ok_or(CoreError::NotFound)
            };
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
                // Another live agent's thread refuses the reply (409
                // `thread_taken`); an unowned one becomes this agent's.
                let (c, took) = st.add_agent_reply_keyed(
                    &audit,
                    &tid,
                    reply,
                    addressed.then_some((&id, sess.harness.as_str())),
                    key,
                )?;
                let c = match c {
                    Keyed::Made(c) => c,
                    Keyed::Before {
                        thread_id,
                        comment_id,
                    } => replayed(thread_id, comment_id)?,
                };
                touched.merge(took);
                let seen = clax_store::visibility::Visibility::for_session(st, &sess.id)?;
                let may = |x: &clax_core::model::Artifact| reads.allows(x);
                let readable: Option<&(dyn Fn(&clax_core::model::Artifact) -> bool + Sync)> =
                    reads.is_person_agent().then_some(&may);
                touched.merge(st.acknowledge(
                    &audit,
                    &sess.id,
                    std::slice::from_ref(&tid),
                    &seen,
                    readable,
                )?);
                let changed =
                    ctx.working
                        .thread_done(&sess.id, id.as_str(), &tid, StopReason::Explicit);
                clax_realtime::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
                c
            } else {
                if !b.via_page {
                    check_named_reads(st, &id, &b.body, live_page)?;
                }
                let audit = audit.resolve(st)?;
                let (name, author_public_id) = author(st, &who)?;
                let (c, follow) = match st.add_comment_keyed(
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
                    key,
                )? {
                    Keyed::Made(c) => (c, true),
                    Keyed::Before {
                        thread_id,
                        comment_id,
                    } => {
                        let c = replayed(thread_id, comment_id)?;
                        let unsent = !st.comment_has_feedback(&c.id)?;
                        (c, unsent)
                    }
                };
                if follow {
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
                        // `@agent` routes (M18); a later comment on a sent
                        // thread counts into the requests open there.
                        let route = if at_agent {
                            clax_store::approvals::Route::Send
                        } else {
                            clax_store::approvals::Route::Join
                        };
                        let (_, t, _) =
                            st.send_routed(&audit, &tid, SendTarget::Thread, route, false)?;
                        touched.merge(t);
                    }
                    if !named.is_empty() {
                        touched.merge(st.send_to_named(&audit, &tid, &named)?.1);
                    }
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
            // A person's reply reopened it: its open questions move back in.
            if was_resolved && t.status == "open" {
                clax_http::views::question::announce_thread(&app, st, &tid);
            }
            let view = vis.filter_thread(st, &t, ctx.codex_push(), with_path)?;
            let mut out = json!({"comment": vis.filter_comment(&aid, &c), "thread": view});
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

/// The live session a send's `to` names on artifact `id`, if any: a live
/// owner or watcher by its handle; 400 `unknown_agent` when it names none,
/// and 400 `agent_cannot_read` when it names an agent that may not read the
/// artifact (spec §10). For a person (`person`: the owner, or a connected
/// person, in a browser or by hand) `to` may also name, by handle or name,
/// any live agent that may read and watch the artifact, as an `@name` in
/// their comment may ([`handoff_target`]); that agent is given a watch
/// (recorded under `audit`, the sender's) when it has none.
fn send_target(
    st: &Store,
    audit: &AuditCtx,
    id: &ArtifactId,
    to: Option<&str>,
    live_page: bool,
    person: bool,
) -> clax_core::Result<Option<String>> {
    let Some(h) = to else {
        return Ok(None);
    };
    let watching = if clax_core::is_agent_handle(h) {
        st.live_agent(id, h)?
    } else {
        None
    };
    if let Some(sid) = watching {
        if named_agent_reaches(st, &sid, id, live_page)? == Reach::CannotRead {
            let name = st
                .get_session(&sid)?
                .map(|s| s.name.unwrap_or_else(|| h.to_string()))
                .unwrap_or_default();
            return Err(cannot_read(&name));
        }
        return Ok(Some(sid));
    }
    if person {
        let sid = handoff_target(st, id, h, live_page)?;
        st.ensure_watch(audit, &sid, id)?;
        return Ok(Some(sid));
    }
    Err(if clax_core::is_agent_handle(h) {
        CoreError::invalid(
            "unknown_agent",
            "no live agent on this artifact has that handle",
        )
    } else {
        CoreError::invalid("unknown_agent", "not an agent handle")
    })
}

/// Sets `sent_to_agent` and creates feedback rows; idempotent. An optional
/// body `{"to": <agent handle>}` sends to that live agent only, and it becomes
/// the thread's target (400 `unknown_agent` when it names no live owner or
/// watcher, 400 `agent_cannot_read` when it names one that may not read the
/// artifact). From a person (the owner, or a connected person; not a guest
/// and not an agent) `to` may name any live agent, by handle or name, that
/// may read and watch the artifact: it is given a watch, as an `@name` in
/// their comment gives one ([`send_target`]). Without `to` the send goes to
/// every live owner and watcher and clears the thread's target. The send routes (spec §9.2, M18): each
/// agent it reaches whose person has not approved an author of the
/// thread's comments makes or joins a request to that person; a page's
/// send (`x-clax-via: page`, a page's `sendToClaude`) names the page and
/// its viewer on the request. A request with a foreign `Origin` is refused
/// ([`SameOrigin`]).
pub async fn send(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadSend>,
    _o: SameOrigin,
    audit: DeferredAudit,
    headers: axum::http::HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let page = clax_http::agent::page_marker(&a.caller, &headers)?;
    let live_page = s.live_ids.contains(id.as_str());
    let b: SendBody = if raw.is_empty() {
        SendBody::default()
    } else {
        serde_json::from_slice(&raw)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    sees_threads(&a, &s, &id).await?;
    let ctx = s.feedback_ctx();
    let with_path = a.holds_token();
    let vf = VisFor::of(&a.caller, &s);
    let person = matches!(a.caller, Caller::Owner { .. } | Caller::Person { .. });
    let view = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            let t = thread_of(st, &id, &tid)?;
            let audit = audit.resolve(st)?;
            // A resolved thread is refused by the send below; it gives no watch.
            let person = person && t.status != "resolved";
            let to = send_target(st, &audit, &id, b.to.as_deref(), live_page, person)?;
            if person && to.is_some() {
                // The Send menu and roster show a watcher it may have gained.
                ctx.events.publish(Event::Agents {
                    artifacts: vec![id.as_str().to_string()],
                });
            }
            let (t, touched, _) = st.send_routed(
                &audit,
                &tid,
                to.as_deref()
                    .map_or(SendTarget::Everyone, SendTarget::Agent),
                clax_store::approvals::Route::Send,
                page,
            )?;
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            vis.filter_thread(st, &t, ctx.codex_push(), with_path)
        })
        .await?;
    Ok(Json(json!({"thread": view})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct HandoffBody {
    #[serde(default)]
    to: Option<String>,
}

/// The live session a handoff's `to` names on artifact `id`: an agent's
/// name (ignoring case) or handle. 400 `unknown_agent` when it names no
/// live agent, or one that may not watch the artifact; 400
/// `agent_cannot_read` when it names one that may not read it (spec §10).
/// [`Store::handoff`] gives it its watch, once nothing refuses the handoff.
fn handoff_target(
    st: &Store,
    id: &ArtifactId,
    to: &str,
    live_page: bool,
) -> clax_core::Result<String> {
    let unknown = || {
        CoreError::invalid(
            "unknown_agent",
            "no live agent that may watch this artifact has that name or handle",
        )
    };
    let found = if clax_core::is_agent_handle(to) {
        st.live_session_by_handle(to)?
            .map(|s| (s.id.clone(), s.name.unwrap_or_else(|| to.to_string())))
    } else {
        let want = to.to_lowercase();
        st.live_agent_names()?
            .into_iter()
            .find(|(_, n)| n.to_lowercase() == want)
    };
    let (sid, name) = found.ok_or_else(unknown)?;
    match named_agent_reaches(st, &sid, id, live_page)? {
        Reach::Yes => {}
        Reach::CannotRead => return Err(cannot_read(&name)),
        Reach::No => return Err(unknown()),
    }
    Ok(sid)
}

/// `POST /api/sessions/<sid>/threads/<tid>/handoff {to?}` (spec §12,
/// `comments_handoff`): session `sid` hands thread `tid` to the live agent
/// `to` names (its name or handle), or, without `to`, releases it. `sid`
/// must own the thread; or, while it has no live owner, be an agent of its
/// ended owner's person or one whose person sent a comment of the thread
/// it sees (O8); 409 `thread_taken` otherwise. A handoff needs `comments
/// send` as well as `comments reply`, and sends the thread to `to`
/// ([`Store::handoff`]), giving it a watch when it has none and making a
/// routing request when `to`'s person has not approved an author of its
/// comments (M18); `to` must be an agent that may read and watch the
/// artifact (400 `agent_cannot_read`, `unknown_agent`). A release by the
/// owner clears the owner and untargets the thread's undelivered rows; a
/// release of a thread with no live owner changes nothing. Answers
/// `{thread}`, filtered for the caller, with its new `owner`.
pub async fn handoff(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::ThreadHandoff>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    let (sid, tid) = path(p)?;
    let b: HandoffBody = if raw.is_empty() {
        HandoffBody::default()
    } else {
        serde_json::from_slice(&raw)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    let t1 = tid.clone();
    let aid = s
        .store_call(move |st| Ok(st.get_thread(&t1)?.map(|t| t.artifact_id)))
        .await?
        .ok_or_else(ApiError::not_found)?;
    let id = a.named_artifact(&s, &aid).await?;
    a.also_needs(&s, (clax_core::rights::RightType::Comments, "reply"), &id)
        .await?;
    if b.to.is_some() {
        a.also_needs(&s, (clax_core::rights::RightType::Comments, "send"), &id)
            .await?;
    }
    sees_threads(&a, &s, &id).await?;
    let ctx = s.feedback_ctx();
    let with_path = a.holds_token();
    let live_page = s.live_ids.contains(id.as_str());
    let vf = VisFor::of(&a.caller, &s);
    let view = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            match st.get_session(&sid)? {
                Some(x) if x.ended_at.is_none() => {}
                Some(_) => {
                    return Err(CoreError::invalid(
                        "unknown_session",
                        "no live session has this ID",
                    ));
                }
                None => return Err(CoreError::NotFound),
            }
            thread_of(st, &id, &tid)?;
            st.check_may_hand_off(&tid, &sid)?;
            let audit = audit.for_session(st, &sid)?;
            let to = match b.to.as_deref() {
                Some(to) => Some(handoff_target(st, &id, to, live_page)?),
                None => None,
            };
            let (t, touched, _) = st.handoff(&audit, &tid, &sid, to.as_deref())?;
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            vis.filter_thread(st, &t, ctx.codex_push(), with_path)
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
    a: Agent<clax_http::agent::ThreadsSendBatch>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<BatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let b = body(req)?;
    sees_threads(&a, &s, &id).await?;
    let ctx = s.feedback_ctx();
    let with_path = a.holds_token();
    let vf = VisFor::of(&a.caller, &s);
    let live_page = s.live_ids.contains(id.as_str());
    let agent_person = match &a.caller {
        clax_http::auth::Caller::PersonAgent { person, .. } => Some(person.clone()),
        _ => None,
    };
    let v = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let audit = audit.resolve(st)?;
            // A batch names only an agent that owns or watches the artifact.
            let to = send_target(st, &audit, &id, b.to.as_deref(), live_page, false)?;
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
            let public = thread_event_views(st, &sent, ctx.codex_push())?;
            for (t, view) in sent.iter().zip(&public) {
                ctx.events.publish(Event::Thread {
                    artifact_id: t.artifact_id.clone(),
                    thread: view.clone(),
                });
            }
            let views = vis.filter_threads(st, &sent, ctx.codex_push(), with_path)?;
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
/// `feedback_state` event follows: each reader's state of the delivered rows
/// that remain, or, when none remain, `state: null` to the streams of
/// those who read a row before (the owner's and each such person's; spec
/// 2026-10-08 §10). An agent resolve
/// lists a thread no version lists yet as addressed: in the artifact's current
/// version, or, on a live page, in the page's next snapshot (a pending
/// address). Each open question asked in the thread is published again as
/// a `question` event, its `thread.open` now false. A request with a
/// foreign `Origin` is refused ([`SameOrigin`]).
pub async fn resolve(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadResolve>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let reads = a.read_rights();
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    refuse_unlinked(&s, &tid).await?;
    let agent = acting_as(&raw)?;
    if agent && !may_act_as_agent(&a) {
        return Err(ApiError::unauthorized());
    }
    sees_threads(&a, &s, &id).await?;
    let with_path = a.holds_token();
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let live = s.live_ids.contains(id.as_str());
    let vf = VisFor::of(&a.caller, &s);
    let app = s.clone();
    let o = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
            let t = thread_of(st, &id, &tid)?;
            let mut touched = Touched::default();
            let mut resolver: Option<Session> = None;
            let by = if agent {
                let sess = agent_session(st, &session, grant.as_deref())?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_RESOLVE));
                }
                st.check_not_taken(&tid, &sess.id)?;
                let by = format!("agent:{}", sess.harness);
                resolver = Some(sess);
                by
            } else {
                match acting_viewer(st, &who)? {
                    Some(v) => format!("viewer:{}", v.public_id),
                    None => "viewer:anonymous".to_string(),
                }
            };
            let audit = audit.resolve(st)?;
            if let Some(sess) = &resolver {
                let seen = clax_store::visibility::Visibility::for_session(st, &sess.id)?;
                let may = |x: &clax_core::model::Artifact| reads.allows(x);
                let readable: Option<&(dyn Fn(&clax_core::model::Artifact) -> bool + Sync)> =
                    reads.is_person_agent().then_some(&may);
                touched.merge(st.acknowledge(
                    &audit,
                    &sess.id,
                    std::slice::from_ref(&tid),
                    &seen,
                    readable,
                )?);
            }
            let (t, withdrawn) = match &resolver {
                // Whose agent resolved it: its session's person; refused
                // when another live agent owns the thread.
                Some(sess) => st.resolve_thread_as(&audit, &id, &tid, sess, live)?,
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
            clax_realtime::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
            ctx.events.publish(Event::ThreadResolved {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                resolved_by: t.resolved_by.clone().unwrap_or_default(),
                resolved_at: t.resolved_at.clone().unwrap_or_default(),
            });
            apply(&ctx, st, &touched);
            publish_thread(&ctx, st, &t)?;
            clax_http::views::question::announce_thread(&app, st, &tid);
            let view = vis.filter_thread(st, &t, ctx.codex_push(), with_path)?;
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
/// event, and each open question asked in the thread again as a `question`
/// event. A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn reopen(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadReopen>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    refuse_unlinked(&s, &tid).await?;
    let agent = acting_as(&raw)?;
    let authed = may_act_as_agent(&a);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    require_interact(&s, authed, who).await?;
    sees_threads(&a, &s, &id).await?;
    let with_path = a.holds_token();
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let vf = VisFor::of(&a.caller, &s);
    let app = s.clone();
    let o = s
        .store_call(move |st| {
            let vis = vf.build(st)?;
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
            clax_http::views::question::announce_thread(&app, st, &tid);
            Ok(Outcome::Done(
                json!({"thread": vis.filter_thread(st, &t, ctx.codex_push(), with_path)?}),
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
/// `thread_deleted`, and each open question asked in the thread again as a
/// `question` event, its `thread` now null. A person's agent deletes from a session (400
/// `no_session`) and only where it may read threads (403 `not_permitted`).
/// A request with a foreign `Origin` is refused ([`SameOrigin`]).
pub async fn delete(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ThreadDelete>,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let (aid, tid) = path(p)?;
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let agent = agent_as(q.as_.as_deref())?;
    let authed = may_act_as_agent(&a);
    if agent && !authed {
        return Err(ApiError::unauthorized());
    }
    person_agent_session(&a.caller)?;
    require_interact(&s, authed, who).await?;
    sees_threads(&a, &s, &id).await?;
    let (session, grant) = acting(&a.caller);
    let ctx = s.feedback_ctx();
    let app = s.clone();
    let o = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let t = thread_of(st, &id, &tid)?;
            if agent {
                agent_session(st, &session, grant.as_deref())?;
                if !t.sent_to_agent {
                    return Ok(Outcome::Guidance(GUIDANCE_DELETE));
                }
            }
            let audit = audit.resolve(st)?;
            let (_, touched) = st.delete_thread_touched_async(&audit, &tid).await?;
            let changed = ctx
                .working
                .thread_gone(id.as_str(), &tid, StopReason::Deleted);
            clax_realtime::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
            ctx.events.publish(Event::ThreadDeleted {
                artifact_id: aid.clone(),
                thread_id: tid.clone(),
                moved: false,
            });
            apply(&ctx, st, &touched);
            clax_http::views::question::announce_thread(&app, st, &tid);
            Ok(Outcome::Done(json!({"deleted": true, "thread_id": tid})))
        })
        .await?;
    Ok(respond(o, StatusCode::OK))
}

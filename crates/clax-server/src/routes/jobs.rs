//! The app's jobs, owner credential only: the job rows are the truth about
//! every job, and one queue owner per client, claimed with its live event
//! stream, may write them. Every write answers the job's row after it
//! (`{"job": …}`), which the queue applies; every committed change of a
//! job, by any path, is published as `Event::Job` on the `jobs` topic
//! ([`listen`]).
//!
//! Routes:
//! - `POST /api/jobs/owner` `{client, stream}` and `DELETE /api/jobs/owner`
//! - `POST /api/jobs`, `GET /api/jobs?open=1&client=…`, `GET /api/jobs/{id}`
//! - `PATCH /api/jobs/{id}`
//! - `PUT`/`DELETE /api/jobs/{id}/threads/{tid}`
//! - `PUT /api/jobs/{id}/questions/{qid}`
//! - `POST /api/jobs/{id}/steps/{key}`
//! - `GET /api/agent-read-consents`, `PUT`/`DELETE
//!   /api/agent-read-consents/{reader}/{source}`

use super::artifacts::{
    JsonBytes, body, parse_body, path, published_from, publishing_session, working_threads_of,
};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use clax_core::audit::AuditCtx;
use clax_core::publish::{PublishRequest, require_title, validate};
use clax_core::{ArtifactId, CoreError, Event};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::RequireToken;
use clax_http::error::ApiError;
use clax_http::hosted::HostedRequest;
use clax_http::jobs_owner::{Claimed, OWNER_HEADER, WriteGuard};
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::docs::{BatchOp, BatchWrite};
use clax_store::jobs::{
    DraftRef, JobError, JobFull, JobPatch, JobRow, JobState, NewJob, PublishStep, ReplyStep,
    StepError,
};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

/// Installs the store's job listener: each committed write of a job, by
/// whatever path (a job route, a question's close, a delete that cancels
/// it), publishes `Event::Job` with the row it left.
pub fn listen(s: &AppState) {
    let events = s.events.clone();
    s.store.set_job_listener(move |changes| {
        for c in changes {
            events.publish(Event::Job {
                id: c.id,
                client: c.client,
                state: c.state.as_str().to_string(),
                step: c.step,
                rev: c.rev,
            });
        }
    });
}

/// The queue owner's token the request carries.
fn owner_token(headers: &HeaderMap) -> Option<&str> {
    headers.get(OWNER_HEADER).and_then(|v| v.to_str().ok())
}

/// The write admitted under the claim of `client`'s queue the request's
/// token holds, to move into the store job that does the write
/// ([`JobsOwners::enter`](clax_http::jobs_owner::JobsOwners::enter)); 409
/// `not_owner` unless the token holds it, and while its claim drains.
pub(crate) fn require_owner(
    s: &AppState,
    headers: &HeaderMap,
    client: &str,
) -> Result<WriteGuard, ApiError> {
    if let Some(guard) = owner_token(headers).and_then(|t| s.jobs_owners.enter(client, t)) {
        return Ok(guard);
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "not_owner",
        format!("this request does not hold {client}'s queue: claim it with POST /api/jobs/owner"),
    ))
}

/// Job `id` with everything, or 404.
fn full(st: &Store, id: &str) -> clax_core::Result<JobFull> {
    st.job(id)?.ok_or(CoreError::NotFound)
}

/// A job's write answered as its route answers it: `{"job": JobFull}`.
fn answer(st: &Store, row: &JobRow) -> clax_core::Result<Value> {
    Ok(json!({"job": full(st, &row.id)?}))
}

/// What a job write's refusal answers.
fn refused(st: &Store, e: JobError) -> clax_core::Result<ApiError> {
    Ok(match e {
        JobError::Moved(row) => {
            let mut err = ApiError::new(
                StatusCode::CONFLICT,
                "job_moved",
                format!(
                    "job {} is at rev {} in {}",
                    row.id,
                    row.rev,
                    row.state.as_str()
                ),
            );
            err.extra.insert("job".into(), json!(full(st, &row.id)?));
            err
        }
        JobError::Conflict => ApiError::new(
            StatusCode::CONFLICT,
            "job_exists",
            "a job of that ID exists with another body",
        ),
        JobError::NotFound => ApiError::not_found(),
        JobError::Store(e) => e.into(),
    })
}

/// What a step's refusal answers.
fn step_refused(st: &Store, e: StepError) -> clax_core::Result<ApiError> {
    Ok(match e {
        StepError::VersionMoved { current } => {
            let mut err = ApiError::new(
                StatusCode::CONFLICT,
                "version_moved",
                format!("the artifact is at version {current}: plan the step again on it"),
            );
            err.extra.insert("current".into(), json!(current));
            err
        }
        StepError::DocMoved { path } => {
            let mut err = ApiError::new(
                StatusCode::CONFLICT,
                "doc_moved",
                format!("document {path} moved since the step was planned"),
            );
            err.extra.insert("path".into(), json!(path));
            err
        }
        StepError::NotRenderedClean => ApiError::new(
            StatusCode::CONFLICT,
            "not_rendered_clean",
            "the draft has no clean render of these files",
        ),
        StepError::Job(e) => refused(st, e)?,
        StepError::Store(e) => e.into(),
    })
}

/// Runs job write `f` on the store, admitted under `guard`, and answers
/// it: the job after it, or its refusal. The guard moves into the store
/// job, so it ends when the write does, even when this request is dropped
/// while the write runs.
async fn write(
    s: &AppState,
    guard: WriteGuard,
    f: impl FnOnce(&Store) -> Result<JobRow, JobError> + Send + 'static,
) -> Result<Json<Value>, ApiError> {
    let out = s
        .store_call(move |st| {
            let _guard = guard;
            match f(st) {
                Ok(row) => Ok(Ok(answer(st, &row)?)),
                Err(e) => Ok(Err(refused(st, e)?)),
            }
        })
        .await??;
    Ok(Json(out))
}

/// The client of job `id`, which a write of it must hold the queue of;
/// 404 when there is none.
async fn client_of(s: &AppState, id: &str) -> Result<String, ApiError> {
    let id = id.to_string();
    s.store_call(move |st| Ok(full(st, &id)?.job.client)).await
}

// ------------------------------------------------------------- the owner

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimBody {
    client: String,
    stream: String,
}

/// `POST /api/jobs/owner` `{client, stream}`: claims `client`'s queue for
/// the holder of event stream `stream` (its `ready` block names it).
/// 200 `{owner}` (the token every write of the client's jobs carries in
/// `x-clax-jobs-owner`); 409 `owner_held` while another claim's stream
/// lives; 400 `no_such_stream` when the hub holds no connection of
/// `stream`, or none of the caller's; 409 `owner_draining` at once when
/// the claim is the caller's but writes admitted under an older claim are
/// still running: `jobs_owner` with `held: true` is published when they
/// end, and the stream's next claim answers its token. That event may
/// reach the stream before this answer does, so one heard after the
/// claim was sent counts. The claim is
/// released when that connection ends. Before anything else the
/// claim subscribes `stream` to the `jobs` topic, so a claimant refused
/// `owner_held` hears the `jobs_owner` release it waits for, however soon
/// it comes.
pub async fn claim(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    who: clax_http::identity::Identity,
    req: Result<Json<ClaimBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let b = body(req)?;
    let w = who.clone();
    let caller = s
        .store_call(move |st| clax_http::db_caller::caller_of(st, w.token || w.events_cookie, &w))
        .await?;
    match s.stream.update(
        &b.stream,
        &caller,
        &[clax_realtime::stream::Topic::Jobs],
        &[],
    ) {
        Ok(_) => {}
        Err(clax_realtime::stream::SubError::UnknownStream) => {
            return Err(no_such_stream(&b.stream));
        }
        Err(clax_realtime::stream::SubError::TooMany) => {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "limit_reached",
                format!(
                    "a stream holds at most {} topics",
                    clax_realtime::stream::MAX_TOPICS
                ),
            ));
        }
        Err(_) => {
            return Err(ApiError::forbidden(
                "forbidden",
                "this stream may not follow the app's jobs",
            ));
        }
    }
    match s
        .jobs_owners
        .claim(&s.stream, &s.events, &b.client, &b.stream)
        .await
    {
        Claimed::Owner(token) => Ok(Json(json!({"owner": token}))),
        Claimed::Held => Err(ApiError::new(
            StatusCode::CONFLICT,
            "owner_held",
            format!(
                "{}'s queue is held by another live stream: wait for jobs_owner with held false",
                b.client
            ),
        )),
        Claimed::Draining => Err(ApiError::new(
            StatusCode::CONFLICT,
            "owner_draining",
            format!(
                "{}'s queue is this stream's once the old owner's writes end: wait for jobs_owner with held true, then claim again",
                b.client
            ),
        )),
        Claimed::NoSuchStream => Err(no_such_stream(&b.stream)),
    }
}

/// 400 `no_such_stream`: `stream` is not a live stream of the caller's.
fn no_such_stream(stream: &str) -> ApiError {
    ApiError::bad_request("no_such_stream", format!("{stream} is not a live stream"))
}

/// `DELETE /api/jobs/owner`: releases the claim the request's
/// `x-clax-jobs-owner` token holds. 204, whether or not it held one.
pub async fn release(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    if let Some(client) = owner_token(&headers).and_then(|t| s.jobs_owners.release_token(t)) {
        s.events.publish(Event::JobsOwner {
            client,
            held: false,
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------- jobs

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    id: String,
    client: String,
    kind: String,
    #[serde(default)]
    artifact_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    input: Value,
    #[serde(default)]
    plan: Option<Value>,
}

/// `POST /api/jobs` `{id, client, kind, artifact_id?, title?, input,
/// plan?}`: creates a job in `queued` (201 `{job}`); an existing `id`
/// with the same body answers it (200), another body 409 `job_exists`. On
/// a hosted daemon an `artifact_id` out of the owner's reach is 404
/// ([`clax_http::hosted::require_reach`]).
pub async fn create(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<HostedRequest>>,
    headers: HeaderMap,
    req: Result<Json<CreateBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let b = body(req)?;
    let guard = require_owner(&s, &headers, &b.client)?;
    if let Some(a) = &b.artifact_id {
        clax_http::hosted::require_reach(&s, hosted.as_deref(), a).await?;
    }
    let id = b.id.clone();
    let existed = s.store_call(move |st| Ok(st.job(&id)?.is_some())).await?;
    let out = write(&s, guard, move |st| {
        st.create_job(NewJob {
            id: b.id,
            client: b.client,
            kind: b.kind,
            artifact_id: b.artifact_id,
            title: b.title,
            input: b.input,
            plan: b.plan,
        })
    })
    .await?;
    let status = if existed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, out))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    client: Option<String>,
}

/// `GET /api/jobs?open=1&client=<client>`: `{jobs: [JobFull]}`, the
/// client's jobs that are neither `cancelled` nor `done`, oldest first.
/// Only open jobs are listed: 400 `invalid_query` without `open=1` and a
/// client.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let q = q
        .map(|Query(q)| q)
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let (Some("1"), Some(client)) = (q.open.as_deref(), q.client) else {
        return Err(ApiError::bad_request(
            "invalid_query",
            "list a client's open jobs: ?open=1&client=<client>",
        ));
    };
    let jobs = s.store_call(move |st| st.open_jobs(&client)).await?;
    Ok(Json(json!({ "jobs": jobs })))
}

/// `GET /api/jobs/{id}`: `{job: JobFull}`; 404 when there is none.
pub async fn get_one(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let id = path(p)?;
    let job = s.store_call(move |st| full(st, &id)).await?;
    Ok(Json(json!({ "job": job })))
}

/// A field present (`null` or a value) as `Some`, absent as `None` (by
/// `#[serde(default)]`).
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    if_rev: i64,
    #[serde(default)]
    state: Option<JobState>,
    #[serde(default, deserialize_with = "present")]
    step: Option<Option<String>>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    plan: Option<Option<Value>>,
    #[serde(default, deserialize_with = "present")]
    carry: Option<Option<Value>>,
    #[serde(default, deserialize_with = "present")]
    error: Option<Option<Value>>,
}

/// `PATCH /api/jobs/{id}` `{if_rev, state?, step?, title?, plan?, carry?,
/// error?}`: compare-and-set on `rev` (409 `job_moved` with the current
/// job); a field given as `null` is cleared. The state moves by the
/// store's table (`jobs::may_move`): 400 `invalid_move` for a move to
/// `queued`, `waiting` or `done`.
pub async fn patch(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    headers: HeaderMap,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let id = path(p)?;
    let b = body(req)?;
    let guard = require_owner(&s, &headers, &client_of(&s, &id).await?)?;
    write(&s, guard, move |st| {
        st.patch_job(
            &id,
            JobPatch {
                if_rev: b.if_rev,
                state: b.state,
                step: b.step,
                title: b.title,
                plan: b.plan,
                carry: b.carry,
                error: b.error,
            },
        )
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadBody {
    comment_id: String,
    #[serde(default)]
    stopped: bool,
}

/// `PUT /api/jobs/{id}/threads/{tid}` `{comment_id, stopped?}`: the job
/// answers thread `tid` up to comment `comment_id`, or stopped at it. On a
/// hosted daemon a thread of an artifact out of the owner's reach is 400
/// `invalid_thread`, as one that does not exist.
pub async fn put_thread(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<HostedRequest>>,
    headers: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<ThreadBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (id, tid) = path(p)?;
    let b = body(req)?;
    let guard = require_owner(&s, &headers, &client_of(&s, &id).await?)?;
    if s.hosted.is_some() {
        let t = tid.clone();
        let of = s
            .store_call(move |st| Ok(st.get_thread(&t)?.map(|t| t.artifact_id)))
            .await?;
        if let Some(aid) = of {
            match clax_http::hosted::require_reach(&s, hosted.as_deref(), &aid).await {
                Err(e) if e.code == "not_found" => {
                    return Err(ApiError::bad_request(
                        "invalid_thread",
                        format!("no thread {tid}"),
                    ));
                }
                other => other?,
            }
        }
    }
    write(&s, guard, move |st| {
        st.set_job_thread(&id, &tid, &b.comment_id, b.stopped)
    })
    .await
}

/// `DELETE /api/jobs/{id}/threads/{tid}`: the job forgets thread `tid`.
pub async fn delete_thread(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    headers: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (id, tid) = path(p)?;
    let guard = require_owner(&s, &headers, &client_of(&s, &id).await?)?;
    write(&s, guard, move |st| st.remove_job_thread(&id, &tid)).await
}

/// `PUT /api/jobs/{id}/questions/{qid}`: records question `qid` (one of
/// the `ask` tool's, which no other job asked; 400 `invalid_question`
/// otherwise) as the job's, moving a `running` job to `waiting` while it
/// is open, in one transaction. On a hosted daemon the question must be
/// one the owner's agents asked, about no artifact or one in the owner's
/// reach: any other is 400 `invalid_question`, as one that does not exist.
pub async fn put_question(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<HostedRequest>>,
    headers: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (id, qid) = path(p)?;
    let guard = require_owner(&s, &headers, &client_of(&s, &id).await?)?;
    if s.hosted.is_some() {
        let q = qid.clone();
        // The owner's agents' question: its artifact, if any.
        let of = s
            .store_call(move |st| match st.question_of(&q, None) {
                Ok(row) => Ok(Some(row.artifact_id)),
                Err(CoreError::NotFound) => Ok(None),
                Err(e) => Err(e),
            })
            .await?;
        let mine = match of {
            None => false,
            Some(None) => true,
            Some(Some(aid)) => {
                match clax_http::hosted::require_reach(&s, hosted.as_deref(), &aid).await {
                    Ok(()) => true,
                    Err(e) if e.code == "not_found" => false,
                    Err(e) => return Err(e),
                }
            }
        };
        if !mine {
            return Err(ApiError::bad_request(
                "invalid_question",
                format!("no question {qid} of the ask tool"),
            ));
        }
    }
    write(&s, guard, move |st| st.ask_job_question(&id, &qid)).await
}

// ------------------------------------------------------------- steps

/// One document write of a publish step, as `docs:batch` takes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocWriteBody {
    op: String,
    path: String,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    if_version: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftBody {
    id: String,
    digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishBody {
    artifact_id: Option<String>,
    base: u32,
    request: PublishRequest,
    #[serde(default)]
    docs: Vec<DocWriteBody>,
    #[serde(default)]
    draft: Option<DraftBody>,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyBody {
    threads: Vec<String>,
    body: String,
    #[serde(default)]
    resolve: bool,
    #[serde(default)]
    ack: Vec<String>,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepBody {
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    publish: Option<PublishBody>,
    #[serde(default)]
    reply: Option<ReplyBody>,
    #[serde(default)]
    state: Option<JobState>,
}

/// A step, checked.
enum Step {
    /// A step with only a result, ending the job when `done`.
    Plain(Value, bool),
    Publish(Box<PublishStep>),
    Reply(ReplyStep),
}

fn parse_step(b: StepBody) -> Result<Step, ApiError> {
    let done = match b.state {
        None => false,
        Some(JobState::Done) => true,
        Some(other) => {
            return Err(ApiError::bad_request(
                "invalid_move",
                format!("a step ends its job done, never {}", other.as_str()),
            ));
        }
    };
    match (b.result, b.publish, b.reply) {
        (result, None, None) => Ok(Step::Plain(result.unwrap_or(Value::Null), done)),
        (None, Some(p), None) => {
            let docs = p
                .docs
                .into_iter()
                .map(|w| {
                    let op = match (w.op.as_str(), w.data) {
                        ("set", Some(d)) => BatchOp::Set(d),
                        ("update", Some(d)) => BatchOp::Update(d),
                        ("delete", None) => BatchOp::Delete,
                        (op, _) => {
                            return Err(ApiError::bad_request(
                                "invalid_argument",
                                format!(
                                    "'{op}' on {}: op is set or update (with data) or delete (without)",
                                    w.path
                                ),
                            ));
                        }
                    };
                    Ok(BatchWrite {
                        path: w.path,
                        op,
                        if_version: w.if_version,
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            if let Some(a) = &p.artifact_id {
                ArtifactId::parse(a)?;
            }
            if p.request.comments.is_some() {
                return Err(ApiError::bad_request(
                    "invalid_args",
                    "comments is set when an artifact is made; PUT /api/hosted/artifacts/<id>/comments changes it",
                ));
            }
            if p.request.forked_from.is_some() {
                return Err(ApiError::bad_request(
                    "invalid_args",
                    "forked_from is set by POST /api/artifacts alone",
                ));
            }
            Ok(Step::Publish(Box::new(PublishStep {
                artifact_id: p.artifact_id,
                base: p.base,
                request: validate(p.request)?,
                docs,
                draft: p.draft.map(|d| DraftRef {
                    id: d.id,
                    digest: d.digest,
                }),
                session_id: p.session_id,
                done,
            })))
        }
        (None, None, Some(r)) => Ok(Step::Reply(ReplyStep {
            threads: r.threads,
            body: r.body,
            resolve: r.resolve,
            ack: r.ack,
            done,
            session_id: r.session_id,
        })),
        _ => Err(ApiError::bad_request(
            "invalid_step",
            "a step carries a result, a publish or a reply, one of them",
        )),
    }
}

/// `POST /api/jobs/{id}/steps/{key}` `{result?, publish?, reply?,
/// state?}`: commits step `key`, with its effects in one transaction (a
/// publish's version and pinned document writes, a reply's comments,
/// resolves and acknowledgements, made from the session that published
/// the job's version while it is live, else from the live session of the
/// owner's that the reply names in `session_id`); `state: "done"` ends
/// the job in it.
/// `{job, step, replayed, version?}`: a key the job already has answers
/// its step as recorded with `replayed: true` and changes nothing. 409
/// `version_moved` with `current`, `doc_moved` with `path`,
/// `not_rendered_clean` for a publish naming a draft with no clean render
/// of its files, `job_moved` with the job; nothing is recorded then. 400
/// `invalid_args` for a publish carrying `comments`, which only making an
/// artifact sets. On a hosted daemon the step holds to the owner's reach
/// ([`clax_http::hosted::require_right`]: 404 out of it, 403
/// `not_permitted` within it): a publish to an artifact needs
/// `clax-artifacts` `update` there, and its document writes, made at the
/// owner's level, an artifact the owner made, and a change of its
/// capabilities the owner's administering it (403 `admin_only`); a reply
/// needs `clax-comments` `reply` at the job's artifact, and `resolve` too
/// when it resolves.
pub async fn commit(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<HostedRequest>>,
    audit: AuditCtx,
    headers: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (id, key) = path(p)?;
    let step = parse_body(req, "the publish limit", parse_step).await?;
    let guard = require_owner(&s, &headers, &client_of(&s, &id).await?)?;
    if s.hosted.is_some() {
        step_in_reach(&s, hosted.as_deref(), &d, &id, &step).await?;
    }
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let out = match step {
        Step::Plain(result, done) => {
            s.store_call(move |st| {
                let _guard = guard;
                match st.commit_step(&id, &key, result, done) {
                    Ok((step, row, replayed)) => Ok(Ok(json!({
                        "job": full(st, &row.id)?, "step": step, "replayed": replayed,
                    }))),
                    Err(e) => Ok(Err(refused(st, e)?)),
                }
            })
            .await??
        }
        Step::Publish(p) => {
            let mut p = *p;
            let drafts = s.drafts.clone();
            let step_docs = clax_http::app_data_host::HostStepDocs::of(&s, &audit);
            let out = s
                .store_call_async(move |st| async move {
                    // Ends with the step's work, wherever it runs.
                    let _guard = guard;
                    let st = &*st;
                    // A key already committed answers as recorded; a new one
                    // is published as POST /api/artifacts or its versions
                    // publish for the session it names.
                    let committed = st
                        .job(&id)?
                        .is_some_and(|j| j.steps.iter().any(|x| x.key == key));
                    if !committed {
                        if p.artifact_id.is_none() {
                            require_title(p.request.title.as_deref())?;
                        }
                        p.session_id = publishing_session(st, &p.session_id, None)?;
                        if let (Some(sid), Some(aid)) = (&p.session_id, &p.artifact_id) {
                            p.request.working_threads = working_threads_of(&ctx, sid, aid);
                        }
                    }
                    let sid = p.session_id.clone();
                    // The approval before the step, to tell open views when
                    // the version's declaration narrowed it.
                    let approved = p
                        .artifact_id
                        .as_deref()
                        .and_then(|a| ArtifactId::parse(a).ok())
                        .map(|a| {
                            let before = st.capability_approval(&a).ok().flatten();
                            (a, before)
                        });
                    match st
                        .commit_publish_step_async(&audit, &id, &key, p, &step_docs)
                        .await
                    {
                        Ok(c) => {
                            if let Some(made) = &c.made {
                                after_publish_step(
                                    &events,
                                    made,
                                    || match &sid {
                                        Some(sid) => published_from(
                                            st,
                                            &audit,
                                            &ctx,
                                            &drafts,
                                            sid,
                                            &made.artifact,
                                            true,
                                        ),
                                        None => Ok(()),
                                    },
                                    |tid| {
                                        if let Some(t) = st.get_thread(tid)? {
                                            crate::routes::threads::publish_thread(&ctx, st, &t);
                                        }
                                        Ok(())
                                    },
                                );
                                if let Some((aid, before)) = &approved
                                    && let Err(e) = super::permissions::tell_if_narrowed(
                                        st,
                                        &events,
                                        aid,
                                        before.as_ref(),
                                    )
                                {
                                    tracing::warn!(artifact = aid.as_str(), error = %e,
                                        "a committed publish step's permissions event failed");
                                }
                            }
                            Ok(Ok((
                                json!({
                                    "job": full(st, &c.job.id)?, "step": c.step,
                                    "replayed": c.replayed, "version": c.step.version_n,
                                }),
                                c.made.map(|m| m.artifact.id),
                            )))
                        }
                        Err(e) => Ok(Err(step_refused(st, e)?)),
                    }
                })
                .await??;
            let (v, made) = out;
            if let Some(aid) = made.and_then(|a| ArtifactId::parse(&a).ok()) {
                crate::public::after_publish(&s, &aid);
            }
            v
        }
        Step::Reply(mut r) => {
            s.store_call(move |st| {
                let _guard = guard;
                // A key already committed answers as recorded. A new one
                // may name a session of the owner's, which is the store's
                // fallback for a publishing session no longer live: one
                // that ended is passed on for the store to refuse only
                // when the publishing session ended too.
                let committed = st
                    .job(&id)?
                    .is_some_and(|j| j.steps.iter().any(|x| x.key == key));
                if !committed && r.session_id.is_some() {
                    r.session_id = match publishing_session(st, &r.session_id, None) {
                        Err(CoreError::Invalid {
                            code: "unknown_session",
                            ..
                        }) => r.session_id,
                        other => other?,
                    };
                }
                match st.commit_reply_step(&audit, &id, &key, r) {
                    Ok(c) => {
                        if let Some(made) = &c.made {
                            let sid = &made.session_id;
                            for cm in &made.comments {
                                let Some(t) = st.get_thread(&cm.thread_id)? else {
                                    continue;
                                };
                                events.publish(Event::Comment {
                                    artifact_id: t.artifact_id.clone(),
                                    thread_id: t.id.clone(),
                                    comment: json!(cm),
                                });
                                let changed = ctx.working.thread_done(
                                    sid,
                                    &t.artifact_id,
                                    &t.id,
                                    clax_core::working::StopReason::Explicit,
                                );
                                clax_realtime::working::settle(
                                    st,
                                    &audit,
                                    &ctx.events,
                                    &ctx.working,
                                    &changed,
                                );
                                crate::routes::threads::publish_thread(&ctx, st, &t);
                            }
                            clax_realtime::feedback::apply(&ctx, st, &made.touched);
                        }
                        Ok(Ok(json!({
                            "job": full(st, &c.job.id)?, "step": c.step, "replayed": c.replayed,
                        })))
                    }
                    Err(e) => Ok(Err(step_refused(st, e)?)),
                }
            })
            .await??
        }
    };
    Ok(Json(out))
}

/// On a hosted daemon, that step `step` of job `id` stays within the
/// reach of the request's caller (`h`, who is `d`'s), as [`commit`]
/// describes.
async fn step_in_reach(
    s: &AppState,
    h: Option<&HostedRequest>,
    d: &DeniedToGrants,
    id: &str,
    step: &Step,
) -> Result<(), ApiError> {
    use clax_core::rights::RightType;
    match step {
        Step::Plain(..) => Ok(()),
        Step::Publish(p) => {
            let Some(aid) = p.artifact_id.clone() else {
                return Ok(());
            };
            let right = (RightType::Artifacts, "update");
            clax_http::hosted::require_right(s, h, right, &aid).await?;
            // A change of its capabilities is its maker's, as a publish's.
            if p.request.capabilities.is_some() {
                let caller = d.caller(s).await?;
                let id = ArtifactId::parse(&aid)?;
                let caps = p.request.capabilities.clone();
                super::artifacts::admin_change(s, &caller, &id, None, caps).await?;
            }
            let hosted = s.hosted.is_some();
            let owners = s
                .store_call(move |st| clax_http::db_caller::token_at(st, hosted, true, &aid))
                .await?;
            if !p.docs.is_empty() && !owners {
                return Err(ApiError::forbidden(
                    "not_permitted",
                    "a step's document writes are made at the owner's level: only at a page the owner made",
                ));
            }
            Ok(())
        }
        Step::Reply(r) => {
            let job = id.to_string();
            let of = s
                .store_call(move |st| Ok(st.job(&job)?.and_then(|j| j.job.artifact_id)))
                .await?;
            match of {
                Some(aid) => {
                    let right = (RightType::Comments, "reply");
                    clax_http::hosted::require_right(s, h, right, &aid).await?;
                    if r.resolve {
                        let right = (RightType::Comments, "resolve");
                        clax_http::hosted::require_right(s, h, right, &aid).await?;
                    }
                    Ok(())
                }
                // The store refuses a reply on a job of no artifact.
                None => Ok(()),
            }
        }
    }
}

/// What follows a publish step's commit, none of which turns the
/// committed step into an error: the version's event and its documents'
/// first, then `from` (what a publish from the step's session does for
/// it), then `thread` for each thread the version addresses, a failure of
/// either logged. A replay made nothing, and nothing follows it.
fn after_publish_step(
    events: &clax_core::events::EventBus,
    made: &clax_store::jobs::Published,
    from: impl FnOnce() -> clax_core::Result<()>,
    mut thread: impl FnMut(&str) -> clax_core::Result<()>,
) {
    let aid = &made.artifact.id;
    events.publish(Event::Version {
        artifact_id: aid.clone(),
        n: made.version.n,
        by_page: false,
        title: Some(made.artifact.title.clone()),
        at: Some(made.version.created_at.clone()),
    });
    for w in &made.docs {
        if let Some(c) = w.change.clone() {
            events.publish(Event::Doc {
                artifact_id: aid.clone(),
                path: c.path,
                version: c.version,
                private_to: c.private_to,
                read_level: c.read_level,
                self_read: c.self_read,
            });
        }
    }
    if let Err(e) = from() {
        tracing::warn!(artifact = aid.as_str(), n = made.version.n, error = %e,
            "a committed publish step's session effects failed");
    }
    for tid in &made.version.addresses {
        if let Err(e) = thread(tid) {
            tracing::warn!(artifact = aid.as_str(), thread = tid.as_str(), error = %e,
                "a committed publish step's thread event failed");
        }
    }
}

// ------------------------------------------------------------- consents

/// `GET /api/agent-read-consents`: `{consents: [{reader, source}]}`, the
/// agent's remembered consents to read `source`'s documents for `reader`;
/// on a hosted daemon those whose two artifacts are both in the caller's
/// reach ([`clax_http::hosted::MemberRights::lists`]) alone.
pub async fn consents(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<clax_http::hosted::HostedRequest>>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let scope = s.hosted.as_ref().map(|_| hosted.and_then(|h| h.0.member));
    let all = s
        .store_call(move |st| {
            let mut all = st.agent_read_consents()?;
            if let Some(scope) = scope {
                all.retain(|(reader, source)| {
                    scope
                        .as_ref()
                        .is_some_and(|m| m.lists(st, reader) && m.lists(st, source))
                });
            }
            Ok(all)
        })
        .await?;
    let all: Vec<Value> = all
        .into_iter()
        .map(|(reader, source)| json!({"reader": reader, "source": source}))
        .collect();
    Ok(Json(json!({ "consents": all })))
}

/// `PUT /api/agent-read-consents/{reader}/{source}`: remembers the
/// consent (204); 400 `invalid_id` for an ID that is not an artifact's,
/// 404 when either artifact does not exist, or on a hosted daemon is out of
/// the caller's reach.
pub async fn put_consent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<clax_http::hosted::HostedRequest>>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let (reader, source) = path(p)?;
    let (r, so) = (ArtifactId::parse(&reader)?, ArtifactId::parse(&source)?);
    for a in [&r, &so] {
        clax_http::hosted::require_reach(&s, hosted.as_deref(), a.as_str()).await?;
    }
    s.store_call(move |st| {
        if st.get_artifact(&r)?.is_none() || st.get_artifact(&so)?.is_none() {
            return Err(CoreError::NotFound);
        }
        st.put_agent_read_consent(r.as_str(), so.as_str())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/agent-read-consents/{reader}/{source}`: forgets the
/// consent; `{deleted}`, whether there was one. On a hosted daemon 404 when
/// either artifact is out of the caller's reach.
pub async fn delete_consent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    hosted: Option<axum::Extension<clax_http::hosted::HostedRequest>>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (reader, source) = path(p)?;
    if s.hosted.is_some() {
        let (r, so) = (ArtifactId::parse(&reader)?, ArtifactId::parse(&source)?);
        for a in [&r, &so] {
            clax_http::hosted::require_reach(&s, hosted.as_deref(), a.as_str()).await?;
        }
    }
    let deleted = s
        .store_call(move |st| st.delete_agent_read_consent(&reader, &source))
        .await?;
    Ok(Json(json!({ "deleted": deleted })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::events::EventBus;

    const DAEMON: &AuditCtx = &AuditCtx::DAEMON;

    /// A publish step's documents in their artifact's file in the home,
    /// written directly (`clax_store::app_data::shim`).
    struct FileDocs<'a>(&'a Store);

    #[async_trait::async_trait]
    impl clax_store::jobs::StepDocs for FileDocs<'_> {
        async fn write(
            &self,
            aid: &ArtifactId,
            op: clax_store::app_data::OpId,
            writes: Vec<BatchWrite>,
        ) -> clax_core::Result<Vec<clax_store::docs::Written>> {
            let rules = clax_core::db::Rules::from_capabilities(&self.0.doc_capabilities(aid)?)?;
            clax_store::app_data::shim::Docs::of(self.0)
                .data(aid)?
                .doc_batch(
                    &clax_store::app_data::WriteCtx::of(DAEMON).with_op(op),
                    &rules,
                    writes,
                    false,
                    &clax_core::db::Caller::new(clax_core::db::Level::Owner, None),
                )
        }
    }

    fn publish(html: &str) -> clax_core::publish::ValidatedPublish {
        let req: PublishRequest = serde_json::from_value(json!({
            "title": "Budget",
            "capabilities": {"db": {}},
            "files": {"index.html": {"content": html}},
        }))
        .unwrap();
        validate(req).unwrap()
    }

    /// A publish step committed on a store of its own: what it made.
    fn made() -> (tempfile::TempDir, clax_store::jobs::Published) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let (a, _) = st.create_artifact(DAEMON, publish("<p>one"), None).unwrap();
        st.create_job(NewJob {
            id: "j1".into(),
            client: "app".into(),
            kind: "change".into(),
            artifact_id: Some(a.id.clone()),
            title: None,
            input: json!({"request": "make it green"}),
            plan: None,
        })
        .unwrap();
        st.patch_job(
            "j1",
            JobPatch {
                if_rev: 1,
                state: Some(JobState::Running),
                ..JobPatch::default()
            },
        )
        .unwrap();
        let step = PublishStep {
            artifact_id: Some(a.id.clone()),
            base: 1,
            request: publish("<p>two"),
            docs: vec![BatchWrite {
                path: "notes/n1".into(),
                op: BatchOp::Set(json!({"t": 1})),
                if_version: None,
            }],
            draft: None,
            session_id: None,
            done: false,
        };
        let c = st
            .commit_publish_step(DAEMON, "j1", "publish:1", step, &FileDocs(&st))
            .unwrap();
        (dir, c.made.unwrap())
    }

    /// The session's effects failing after the commit (its session gone,
    /// a write of its watch refused) still leaves the version's and its
    /// documents' events published.
    #[test]
    fn a_committed_publish_steps_events_go_out_when_its_session_effects_fail() {
        let (_d, made) = made();
        let events = EventBus::new();
        let mut rx = events.subscribe();
        let mut from = false;
        after_publish_step(
            &events,
            &made,
            || {
                from = true;
                Err(CoreError::invalid("unknown_session", "gone"))
            },
            |_| Err(CoreError::NotFound),
        );
        assert!(from);
        let mut got = Vec::new();
        while let Ok(s) = rx.try_recv() {
            got.push(s.event);
        }
        assert!(
            got.iter().any(|e| matches!(e, Event::Version { n: 2, .. })),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|e| matches!(e, Event::Doc { path, .. } if path == "notes/n1")),
            "{got:?}"
        );
    }
}

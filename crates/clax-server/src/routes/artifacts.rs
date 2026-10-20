//! REST routes for artifacts and versions.

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::rejection::{JsonRejection, MissingJsonContentType, QueryRejection};
use axum::extract::{FromRequest, Request};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::live::KIND_LIVE;
use clax_core::model::{Artifact, Session};
use clax_core::publish::{PublishRequest, require_title, validate};
use clax_core::rights::RightType;
use clax_core::{ArtifactId, CoreError, Event};
use clax_http::agent::{Agent, ArtifactsList, TokenOrAgent, page_marker, person_agent_session};
use clax_http::error::ApiError;
use clax_http::headers::SESSION_HEADER;
use clax_http::headers::parse_id;
use clax_http::state::AppState;
use clax_http::views::artifact::LivePart;
use clax_http::views::artifact::live_part;
use clax_http::views::artifact::strip_sessions;
use clax_http::views::artifact::with_owner;
use clax_http::views::thread::VisFor;
use clax_store::live::LivePage;
use clax_store::{MetaPatch, Participants, Store};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// The JSON body of a route with axum's default body limit; see [`body_within`].
pub(crate) fn body<T>(r: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body_within(r, "the request body limit")
}

/// The JSON body, or 413 `body_too_large` naming `limit` (the route's own
/// cap, such as "the publish limit"), or 400 `invalid_json`.
pub(crate) fn body_within<T>(
    r: Result<Json<T>, JsonRejection>,
    limit: &str,
) -> Result<T, ApiError> {
    r.map(|Json(v)| v).map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                format!("request body exceeds {limit}"),
            )
        } else {
            ApiError::bad_request("invalid_json", e.body_text())
        }
    })
}

/// Bodies up to this size are parsed on the runtime worker; larger ones on
/// the blocking pool (see [`parse_body`]).
pub(crate) const INLINE_PARSE_BYTES: usize = 64 * 1024;

/// The unparsed body of a JSON request: the `Content-Type` and the body
/// limit are checked as [`Json`] checks them, with the same rejections.
/// [`parse_body`] parses it.
pub struct JsonBytes(pub Bytes);

impl<S: Send + Sync> FromRequest<S> for JsonBytes {
    type Rejection = JsonRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, JsonRejection> {
        if !json_content_type(req.headers()) {
            return Err(MissingJsonContentType::default().into());
        }
        Ok(JsonBytes(Bytes::from_request(req, state).await?))
    }
}

/// Whether `headers` declare a JSON body (`application/json` or any
/// `application/*+json`), as [`Json`] requires.
fn json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<mime_guess::mime::Mime>().ok())
        .is_some_and(|m| {
            m.type_() == "application"
                && (m.subtype() == "json" || m.suffix().is_some_and(|s| s == "json"))
        })
}

/// Parses the body as `T`, with [`body_within`]'s errors, and applies `then`
/// to it. A body over [`INLINE_PARSE_BYTES`] is parsed (and `then` run) on
/// the blocking pool, so a large body never occupies a runtime worker.
pub(crate) async fn parse_body<T, R>(
    req: Result<JsonBytes, JsonRejection>,
    limit: &'static str,
    then: impl FnOnce(T) -> Result<R, ApiError> + Send + 'static,
) -> Result<R, ApiError>
where
    T: DeserializeOwned + Send + 'static,
    R: Send + 'static,
{
    let bytes = body_within(req.map(|JsonBytes(b)| Json(b)), limit)?;
    let large = bytes.len() > INLINE_PARSE_BYTES;
    let work = move || then(body_within(Json::<T>::from_bytes(&bytes), limit)?);
    if !large {
        return work();
    }
    clax_rt::spawn_blocking(work).await.map_err(|e| {
        tracing::error!(error = %e, "parsing a request body failed");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "parsing the request body failed",
        )
    })?
}

pub(crate) fn path<T>(r: Result<Path<T>, PathRejection>) -> Result<T, ApiError> {
    r.map(|Path(v)| v)
        .map_err(|e| ApiError::bad_request("invalid_path_param", e.body_text()))
}

/// The session named by `X-Clax-Session`, checked to exist, be live, and
/// be one of the caller's credential's: `grant` (`None` for the daemon
/// token, M4). A session of [`BY_HAND_HARNESS`](super::sessions::BY_HAND_HARNESS)
/// (another Clax publishing its person's own versions) is `None`: what it
/// writes is the person's by hand.
///
/// # Errors
/// `unknown_session` when the header names a session that does not exist
/// or has ended; `foreign_session` when it names another credential's.
pub(crate) fn publishing_session(
    st: &Store,
    header: &Option<String>,
    grant: Option<&str>,
) -> Result<Option<String>, CoreError> {
    let Some(id) = header else {
        return Ok(None);
    };
    match st.get_session(id)? {
        Some(sess) if sess.grant_id.as_deref() != grant => Err(CoreError::invalid(
            "foreign_session",
            "that session is not one of this credential's",
        )),
        Some(sess) if sess.ended_at.is_none() => {
            Ok((sess.harness != super::sessions::BY_HAND_HARNESS).then_some(sess.id))
        }
        _ => Err(CoreError::invalid(
            "unknown_session",
            "X-Clax-Session names no live session",
        )),
    }
}

/// Header an agent of another machine names a publish's idempotency key in
/// (another Clax's push: `crate::public`). A publish whose key the token
/// named before answers the version that publish made, whatever the
/// public copy holds now, and publishes nothing.
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";

/// The idempotency key of a publish by agent token `grant`: `None` without
/// one, and always for the daemon token (a local daemon's own publishes
/// are as before).
///
/// # Errors
/// 400 `invalid_args` for a key over
/// [`MAX_PUBLISH_KEY`](clax_store::remotes::MAX_PUBLISH_KEY) bytes or
/// with anything but printable ASCII.
pub(crate) fn publish_key(
    headers: &HeaderMap,
    grant: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let (Some(_), Some(v)) = (grant, headers.get(IDEMPOTENCY_HEADER)) else {
        return Ok(None);
    };
    match v.to_str() {
        Ok(k)
            if !k.is_empty()
                && k.len() <= clax_store::remotes::MAX_PUBLISH_KEY
                && k.bytes().all(|b| b.is_ascii_graphic()) =>
        {
            Ok(Some(k.to_string()))
        }
        _ => Err(ApiError::bad_request(
            "invalid_args",
            format!(
                "Idempotency-Key is at most {} printable ASCII characters",
                clax_store::remotes::MAX_PUBLISH_KEY
            ),
        )),
    }
}

/// Holds the lock of agent token `grant`'s idempotency key `key`, when the
/// publish names one, until the publish's version is recorded with it: the
/// same publish sent again meanwhile waits, then replays.
async fn lock_key<'a>(
    st: &'a Store,
    grant: &Option<String>,
    key: &Option<String>,
) -> Option<clax_core::content::ArtifactLock<'a>> {
    match (grant, key) {
        (Some(g), Some(k)) => Some(st.lock_publish_key(g, k).await),
        _ => None,
    }
}

/// The idempotency key agent token `grant` named, for the store to record
/// with the version.
fn publish_key_of(
    grant: &Option<String>,
    key: &Option<String>,
) -> Option<clax_core::publish::PublishKey> {
    match (grant, key) {
        (Some(g), Some(k)) => Some(clax_core::publish::PublishKey {
            grant_id: g.clone(),
            key: k.clone(),
        }),
        _ => None,
    }
}

/// The version agent token `grant`'s publish keyed `key` made, as `vis`
/// shows it, when it made one (of artifact `of`, when given).
///
/// # Errors
/// 400 `invalid_args` for a key that made another artifact's version.
fn replayed(
    st: &Store,
    grant: Option<&str>,
    key: Option<&str>,
    of: Option<&ArtifactId>,
    vis: &clax_http::views::thread::Vis,
) -> clax_core::Result<Option<(Value, Value)>> {
    let (Some(g), Some(k)) = (grant, key) else {
        return Ok(None);
    };
    let Some((aid, n)) = st.publish_key(g, k)? else {
        return Ok(None);
    };
    if of.is_some_and(|o| o.as_str() != aid) {
        return Err(CoreError::invalid(
            "invalid_args",
            "this Idempotency-Key named a publish of another artifact",
        ));
    }
    let id = ArtifactId::parse(&aid)?;
    match (st.get_artifact(&id)?, st.get_version(&id, n)?) {
        (Some(a), Some(v)) => Ok(Some(vis.filter_published(st, &a, &v)?)),
        _ => Ok(None),
    }
}

/// The raw `X-Clax-Session` value; a value that is not UTF-8 is
/// `unknown_session`.
pub(crate) fn session_header(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    headers
        .get(SESSION_HEADER)
        .map(|v| {
            v.to_str().map(str::to_string).map_err(|_| {
                ApiError::bad_request("unknown_session", "X-Clax-Session is not valid text")
            })
        })
        .transpose()
}

#[derive(Deserialize)]
pub struct ListQuery {
    artifact: Option<String>,
}

/// Each live artifact, with the owner fields of [`with_owner`]. Without the
/// token, no session ID is included. With `?artifact=<aid>`, that artifact
/// alone, or none when it is not live (400 for a malformed ID). Live pages
/// are left out for a caller that may not see them
/// ([`clax_http::live_access::sees_live_pages`]), and for a person's agent every
/// artifact its rights do not let it read, answered as if it did not
/// exist. An agent channel's fields are filtered for its reader (spec
/// §8.3, O9). With `?tag=<id or name>` (repeatable), only the artifacts
/// carrying every tag named (match all); a name no tag has matches none.
pub async fn list(
    State(s): State<AppState>,
    a: Agent<ArtifactsList>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let tags = super::tags::tag_params(raw.as_deref());
    let local = clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let reads = a.read_rights();
    let vf = VisFor::of(&a.caller, &s);
    let artifacts = match q.artifact.as_deref().map(parse_id).transpose()? {
        Some(id) => {
            let working = s.working.for_artifact(id.as_str());
            let wakes = super::sessions::Wakes::for_caller(&s, &a.caller);
            s.store_call(move |st| {
                let Some(a) = st.get_artifact(&id)? else {
                    return Ok(vec![]);
                };
                if (!local && a.kind == KIND_LIVE) || !reads.allows(&a) {
                    return Ok(vec![]);
                }
                let owner = match &a.owner_session_id {
                    Some(sid) => st.get_session(sid)?,
                    None => None,
                };
                let mut participants = st.participants(&id)?;
                super::sessions::Wakes::mark_with(wakes.as_ref(), &mut participants);
                let mut view = with_owner(
                    &a,
                    owner.as_ref(),
                    &working,
                    &participants,
                    live_part(st, &a)?.as_ref(),
                );
                let vis = vf.build(st)?;
                vis.filter_artifact(st, &a, &working, &mut view)?;
                Ok(vec![view])
            })
            .await?
        }
        None => {
            let all = s.working.all();
            let wakes = super::sessions::Wakes::for_caller(&s, &a.caller);
            // The whole listing costs what the home does: the bulk lane.
            s.store_call_bulk(move |st| {
                let artifacts = st.list_artifacts()?;
                let owners: std::collections::HashMap<String, Session> = st
                    .list_sessions(false)?
                    .into_iter()
                    .map(|s| (s.id.clone(), s))
                    .collect();
                let mut participants = st.participants_all()?;
                for p in participants.values_mut() {
                    super::sessions::Wakes::mark_with(wakes.as_ref(), p);
                }
                // Pages a join merged away are kept, out of the listing
                // while the page they were merged into exists; released
                // ones are listed again (spec §7.2).
                let mut merged = std::collections::HashSet::new();
                let mut released = Vec::new();
                for m in st.merged_live_pages()? {
                    if m.merged_into.is_some() {
                        merged.insert(m.artifact_id);
                    } else {
                        released.push(m);
                    }
                }
                let mut sites: std::collections::HashMap<String, Vec<(String, String)>> =
                    std::collections::HashMap::new();
                for (origin, key, used) in st.site_memberships()? {
                    sites.entry(key).or_default().push((used, origin));
                }
                let pages: std::collections::HashMap<String, LivePart> = st
                    .live_pages()?
                    .into_iter()
                    .map(|p| {
                        let mut list = sites.get(&p.origin).cloned().unwrap_or_default();
                        list.sort_by(|a, b| b.cmp(a));
                        let mut origins: Vec<String> = list.into_iter().map(|x| x.1).collect();
                        if origins.is_empty() {
                            origins.push(p.origin.clone());
                        }
                        (
                            p.artifact_id.clone(),
                            LivePart {
                                page: p,
                                origins,
                                merged_into: None,
                            },
                        )
                    })
                    .chain(released.into_iter().map(|m| {
                        let page = LivePage {
                            artifact_id: m.artifact_id.clone(),
                            origin: m.origin.clone(),
                            path: m.path,
                        };
                        (
                            m.artifact_id,
                            LivePart {
                                page,
                                origins: vec![m.origin],
                                merged_into: None,
                            },
                        )
                    }))
                    .collect();
                let none = Participants::default();
                let vis = vf.build(st)?;
                artifacts
                    .iter()
                    .filter(|a| local || a.kind != KIND_LIVE)
                    .filter(|a| !merged.contains(&a.id))
                    .filter(|a| reads.allows(a))
                    .map(|a| {
                        let owner = a.owner_session_id.as_ref().and_then(|sid| owners.get(sid));
                        let working = all.get(&a.id).map(Vec::as_slice).unwrap_or(&[]);
                        let people = participants.get(&a.id).unwrap_or(&none);
                        let mut view = with_owner(a, owner, working, people, pages.get(&a.id));
                        vis.filter_artifact(st, a, working, &mut view)?;
                        Ok(view)
                    })
                    .collect::<clax_core::Result<Vec<_>>>()
            })
            .await?
        }
    };
    let mut artifacts = artifacts;
    if !tags.is_empty() {
        artifacts.retain(|v| {
            let carried: Vec<clax_core::tags::TagRef> =
                serde_json::from_value(v["tags"].clone()).unwrap_or_default();
            clax_store::tags::carries_all(&carried, &tags)
        });
    }
    if !a.caller.is_agent() {
        mark_brief(&s, &mut artifacts).await?;
    }
    let mut v = json!({"artifacts": artifacts});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    Ok(Json(v))
}

/// Creates an artifact. The body must carry a non-blank `title`. A
/// person's agent creates from a session, as its person (spec §6.7, §8.2);
/// the answer is filtered for an agent channel's reader.
pub async fn create(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::ArtifactsCreate>,
    audit: AuditCtx,
    headers: HeaderMap,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    person_agent_session(&a.caller)?;
    let mut p = parse_body(req, "the publish limit", |r: PublishRequest| {
        Ok(validate(r)?)
    })
    .await?;
    require_title(p.title.as_deref())?;
    p.person = publisher(&a.caller);
    let truncated = p.note_truncated;
    let session = session_header(&headers)?;
    let grant = a.grant().map(str::to_string);
    let key = publish_key(&headers, grant.as_deref())?;
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let vf = VisFor::of(&a.caller, &s);
    let (artifact, version) = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let session = publishing_session(st, &session, grant.as_deref())?;
            let _keyed = lock_key(st, &grant, &key).await;
            if let Some(done) =
                replayed(st, grant.as_deref(), key.as_deref(), None, &vf.build(st)?)?
            {
                return Ok(done);
            }
            p.publish_key = publish_key_of(&grant, &key);
            let (artifact, version) = st
                .create_artifact_async(&audit, p, session.as_deref())
                .await?;
            if let Some(sid) = &session {
                let aid = ArtifactId::parse(&artifact.id)?;
                st.ensure_watch(&audit, sid, &aid)?;
                let touched = st.retarget_untargeted(&aid, sid)?;
                clax_realtime::feedback::apply(&ctx, st, &touched);
            }
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page: false,
                title: Some(artifact.title.clone()),
                at: Some(version.created_at.clone()),
            });
            let vis = vf.build(st)?;
            vis.filter_published(st, &artifact, &version)
        })
        .await?;
    let url = format!("/a/{}", artifact["id"].as_str().unwrap_or_default());
    let mut v =
        json!({"artifact": artifact, "version": version, "url": url, "note_truncated": truncated});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    Ok((StatusCode::CREATED, Json(v)))
}

/// `POST /api/artifacts:new` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BriefBody {
    title: String,
    /// What the person wants built: the artifact's first thread.
    brief: String,
}

/// Why `caller` may not start an artifact from a brief, or `None` when it
/// may: the owner (in a browser or by hand) always; a connected person
/// (`Caller::Person`, on a local daemon or signed in to a hosted one) when
/// their rights create artifacts and would let them read the new one and
/// comment on it (an artifact they made is their own, `own`); a guest (a
/// viewer who is no connected person) never (403 `not_permitted`), and no
/// credential 401. Agents are refused before this (they publish).
pub(crate) fn brief_refusal(
    st: &Store,
    caller: &clax_http::auth::Caller,
) -> clax_core::Result<Option<ApiError>> {
    use clax_http::auth::Caller;
    let no_create = || {
        Some(ApiError::forbidden(
            "not_permitted",
            "your rights here do not let you create artifacts (clax-artifacts)",
        ))
    };
    Ok(match caller {
        Caller::Owner { .. } => None,
        Caller::Person { viewer } => {
            let rights = st.person_rights(&viewer.public_id)?;
            // The new artifact matches no artifact a right names by ID, and
            // is the person's own.
            const NEW: &str = "new";
            let own = |_: &str| true;
            let ok = rights.allows(RightType::Artifacts, "create", None, &|_| false)
                && rights.allows(RightType::Artifacts, "read", Some(NEW), &own)
                && rights.allows(RightType::Comments, "write", Some(NEW), &own);
            if ok { None } else { no_create() }
        }
        Caller::Viewer { .. } => Some(ApiError::forbidden(
            "not_permitted",
            "a guest cannot create artifacts: sign in as a person of this Clax first",
        )),
        Caller::Anonymous => Some(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            Some(clax_http::auth::not_for_agents())
        }
    })
}

/// `POST /api/artifacts:new {title, brief}` → 201 `{artifact, version, url,
/// thread}`: a person starts an artifact from a brief in their browser
/// (the gallery's New). Version 1 is a placeholder page naming the title
/// ([`clax_core::brief::placeholder_html`]); the brief is its first thread,
/// on the whole artifact, authored by the person and sent to no agent (a
/// send naming an agent, `POST …/threads/<tid>/send {to}`, gives that agent
/// its watch). The artifact is the person's (`person`; the owner's has
/// none) and no session's. Who may ([`brief_refusal`]): the owner, and a
/// connected person whose rights create artifacts; a guest is 403
/// `not_permitted`, no credential 401, agents 403 (`forbidden` for a
/// person's, `not_for_agents` for the owner's: agents publish). A foreign
/// `Origin` is refused. 400 `invalid_title` (blank, over
/// [`clax_core::brief::MAX_TITLE_CHARS`] characters, or holding a control
/// or format character) and `invalid_brief` (blank, or longer than a
/// comment may be). An `Idempotency-Key` (1 to
/// [`clax_store::briefs::MAX_BRIEF_KEY`] printable ASCII characters, else
/// 400 `invalid_args`) makes a repeat by the same person answer 200 with
/// the artifact, version 1 and brief thread the first request made, while
/// that artifact exists ([`clax_store::briefs`]); the same request sent
/// while the first is running waits for it. A member's artifact cap
/// (`quota_exceeded`) is counted in the transaction that inserts the
/// artifact, as for a publish.
pub async fn create_from_brief(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    _o: clax_http::viewer::SameOrigin,
    who: clax_http::identity::Identity,
    audit: clax_http::audit::DeferredAudit,
    headers: HeaderMap,
    req: Result<Json<BriefBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    use clax_http::auth::Caller;
    d.refuse_agents()?;
    let caller = d.caller(&s).await?;
    let c = caller.clone();
    s.store_call(move |st| brief_refusal(st, &c))
        .await?
        .map_or(Ok(()), Err)?;
    let key = match headers.get(IDEMPOTENCY_HEADER) {
        None => None,
        Some(v) => match v.to_str() {
            Ok(k)
                if !k.is_empty()
                    && k.len() <= clax_store::briefs::MAX_BRIEF_KEY
                    && k.bytes().all(|b| b.is_ascii_graphic()) =>
            {
                Some(k.to_string())
            }
            _ => {
                return Err(ApiError::bad_request(
                    "invalid_args",
                    format!(
                        "Idempotency-Key is at most {} printable ASCII characters",
                        clax_store::briefs::MAX_BRIEF_KEY
                    ),
                ));
            }
        },
    };
    let b = body(req)?;
    let title = clax_core::brief::check_title(&b.title)?;
    let brief = clax_core::brief::check_brief(&b.brief)?;
    if brief.chars().count() > clax_store::threads::MAX_BODY_CHARS {
        return Err(ApiError::bad_request(
            "invalid_brief",
            format!(
                "a brief is at most {} characters",
                clax_store::threads::MAX_BODY_CHARS
            ),
        ));
    }
    let person = match &caller {
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        _ => None,
    };
    let starter = person
        .clone()
        .unwrap_or_else(|| clax_store::briefs::OWNER_STARTER.to_string());
    let html = clax_core::brief::placeholder_html(&title);
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        clax_core::publish::INDEX.to_string(),
        Some(clax_core::publish::FileInput {
            content: html,
            encoding: clax_core::publish::Encoding::Utf8,
            content_type: None,
        }),
    );
    let mut p = validate(PublishRequest {
        title: Some(title),
        files,
        ..Default::default()
    })?;
    p.person = person;
    let ctx = s.feedback_ctx();
    let with_path = d.holds_token();
    let vf = VisFor::of(&caller, &s);
    let (artifact, version, thread, again) = s
        .store_call_async(move |st| async move {
            let st = &*st;
            // Held until the key is recorded: the same request sent again
            // meanwhile waits, then answers what this one made.
            let _keyed = match &key {
                Some(k) => Some(st.lock_brief_key(&starter, k).await),
                None => None,
            };
            if let Some(k) = &key
                && let Some(aid) = st.brief_by_key(&starter, k)?
            {
                let id = ArtifactId::parse(&aid)?;
                // A key whose artifact is gone makes a new one
                // (`record_brief` takes it over); a live one is answered.
                if let Some(a) = st.get_artifact(&id)? {
                    let v = st.get_version(&id, 1)?;
                    let threads = st.list_threads(&id, true, None, 1)?.0;
                    let (Some(v), Some(t)) = (v, threads.first()) else {
                        return Err(CoreError::invalid(
                            "invalid_args",
                            "this Idempotency-Key named an artifact whose brief is gone",
                        ));
                    };
                    let vis = vf.build(st)?;
                    let (a, v) = vis.filter_published(st, &a, &v)?;
                    let t = vis.filter_thread(st, t, ctx.codex_push(), with_path)?;
                    return Ok((a, v, t, true));
                }
            }
            let audit = audit.resolve(st)?;
            let (author_name, author_public_id) = clax_http::viewer::author(st, &who)?;
            let (artifact, version) = st.create_artifact_async(&audit, p, None).await?;
            let aid = ArtifactId::parse(&artifact.id)?;
            ctx.events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page: false,
                title: Some(artifact.title.clone()),
                at: Some(version.created_at.clone()),
            });
            let anchor = clax_core::Anchor {
                kind: clax_core::anchor::AnchorKind::Artifact,
                selector: None,
                quote: None,
                prefix: None,
                suffix: None,
                html_hash: None,
                rect: None,
                custom_name: None,
                area: None,
                file: clax_core::anchor::WHOLE_FILE.to_string(),
                route: None,
            };
            let made = st
                .create_thread_hiding_async(
                    &audit,
                    &aid,
                    clax_store::threads::NewThread {
                        version_n: version.n,
                        anchor,
                        author_name,
                        author_public_id,
                        body: brief,
                        clip: None,
                        via_page: false,
                    },
                    &[],
                )
                .await;
            // A brief that cannot be written leaves no artifact behind.
            let thread = match made {
                Ok(t) => t,
                Err(e) => {
                    let _ = st.delete_artifact_async(&audit, &aid).await;
                    return Err(e);
                }
            };
            // The key's lock serialises this process's requests; one key
            // recorded by another process first leaves no artifact here.
            let recorded = match st.record_brief(&artifact.id, &starter, key.as_deref()) {
                Ok(true) => Ok(()),
                Ok(false) => Err(CoreError::invalid(
                    "invalid_args",
                    "this Idempotency-Key named another request, which made an artifact first",
                )),
                Err(e) => Err(e),
            };
            if let Err(e) = recorded {
                let _ = st.delete_artifact_async(&audit, &aid).await;
                return Err(e);
            }
            // The brief goes to no agent: `@` in it sends nothing.
            let (thread, _) =
                super::threads::announce_new_thread(st, &ctx, &audit, thread, false, None, false)?;
            let vis = vf.build(st)?;
            let (a, v) = vis.filter_published(st, &artifact, &version)?;
            let t = vis.filter_thread(st, &thread, ctx.codex_push(), with_path)?;
            Ok((a, v, t, false))
        })
        .await?;
    let url = format!("/a/{}", artifact["id"].as_str().unwrap_or_default());
    let mut v = json!({"artifact": artifact, "version": version, "url": url, "thread": thread});
    if let Some(a) = v.get_mut("artifact") {
        mark_brief(&s, std::slice::from_mut(a)).await?;
    }
    if !d.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    let status = if again {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(v)))
}

/// Adds `brief: {by}` to each of `views` (artifact views) started from a
/// brief in a person's browser: who started it, by their display name now
/// (`Viewer` without one, as their comments are signed). For people's views only: agents' leave it out.
///
/// # Errors
/// The store's.
pub(crate) async fn mark_brief(s: &AppState, views: &mut [Value]) -> Result<(), ApiError> {
    let ids: Vec<String> = views
        .iter()
        .filter_map(|v| v["id"].as_str().map(str::to_string))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let starters = s
        .store_call(move |st| {
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            st.brief_starters(&refs)
        })
        .await?;
    for v in views.iter_mut() {
        if let Some(by) = v["id"].as_str().and_then(|id| starters.get(id)) {
            v["brief"] =
                json!({"by": clax_core::feedback::display_name(by.as_deref().unwrap_or(""))});
        }
    }
    Ok(())
}

/// Who publishes for `caller` (spec §8.2): a person's agent's person (none
/// for a grant of the owner's row); `None` otherwise, which takes the
/// publishing session's person.
fn publisher(caller: &clax_http::auth::Caller) -> Option<String> {
    match caller {
        clax_http::auth::Caller::PersonAgent { person, .. } => person.clone(),
        _ => None,
    }
}

/// The artifact (with the owner fields of [`with_owner`]) and its versions,
/// filtered for an agent channel's reader. Without the token, no session ID
/// is included. From a browser with a viewer
/// (the owner for the owner cookie, else the viewer cookie's or a hosted
/// session's; see [`clax_http::identity`]), also that viewer's `attention`,
/// and the response is private to the cookie.
pub async fn get(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ArtifactsGet>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let who = clax_http::identity::Identity::from_parts(&headers, &extensions, &s.token);
    let working = s.working.for_artifact(id.as_str());
    let has_viewer = who.owner_browser() || who.names_viewer();
    let vf = VisFor::of(&a.caller, &s);
    let reader = SourceReader::new(a.caller.clone(), a.member().cloned(), s.hosted.is_some());
    let (caller, member) = (a.caller.clone(), a.member_rights());
    let wakes = super::sessions::Wakes::for_caller(&s, &a.caller);
    let mut v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let versions = st.list_versions(&id)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            let vis = vf.build(st)?;
            // Whether each live agent is woken by a comment while idle,
            // which the page shows beside it.
            let mut participants = st.participants(&id)?;
            super::sessions::Wakes::mark_with(wakes.as_ref(), &mut participants);
            let mut artifact = with_owner(
                &a,
                owner.as_ref(),
                &working,
                &participants,
                live_part(st, &a)?.as_ref(),
            );
            vis.filter_artifact(st, &a, &working, &mut artifact)?;
            if let Some(lineage) = forked_from_view(st, &a, &reader, Some(&vis))? {
                artifact["forked_from"] = lineage;
            }
            // Whether the caller may fork it (the shell's Fork button).
            artifact["may_fork"] = json!(
                a.kind != KIND_LIVE
                    && fork_refusal(st, &caller, member.as_deref(), &a.id)?.is_none()
            );
            let mut v = json!({
                "artifact": artifact,
                "versions": vis.filter_versions(&versions),
            });
            if let Some(viewer) = who.browser_viewer(st)? {
                v["attention"] = json!(st.attention(&viewer.id, &id)?);
            }
            Ok(v)
        })
        .await?;
    if !a.caller.is_agent()
        && let Some(art) = v.get_mut("artifact")
    {
        mark_brief(&s, std::slice::from_mut(art)).await?;
    }
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    if has_viewer {
        Ok((
            [
                (header::CACHE_CONTROL, "private, no-cache"),
                (header::VARY, "Cookie"),
            ],
            Json(v),
        )
            .into_response())
    } else {
        Ok(Json(v).into_response())
    }
}

/// 403 `admin_only` when `pinned` or `capabilities` would change artifact
/// `id`'s and `caller` does not administer it
/// ([`clax_http::agent::administers`]): an editor changes contents alone. A
/// value equal to the stored one is no change, so an editor's publish that
/// sends the page's capabilities as they are passes.
async fn admin_change(
    s: &AppState,
    caller: &clax_http::auth::Caller,
    id: &ArtifactId,
    pinned: Option<bool>,
    capabilities: Option<Value>,
) -> Result<(), ApiError> {
    if pinned.is_none() && capabilities.is_none() {
        return Ok(());
    }
    let (caller, id) = (caller.clone(), id.clone());
    let allowed = s
        .store_call(move |st| {
            // No such artifact is the edit's own answer.
            let Some(cur) = st.get_artifact(&id)? else {
                return Ok(true);
            };
            let same = pinned.is_none_or(|p| p == cur.pinned)
                && capabilities.as_ref().is_none_or(|c| *c == cur.capabilities);
            Ok(same || clax_http::agent::administers(st, &caller, id.as_str())?)
        })
        .await?;
    if allowed {
        Ok(())
    } else {
        Err(clax_http::agent::admin_only())
    }
}

/// Metadata edits. `capabilities` replaces the whole declaration (omitted
/// keeps, `{}` clears); it and the text fields are validated like a
/// publish's.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    title: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    pinned: Option<bool>,
    capabilities: Option<Value>,
}

/// `GET /api/artifacts/<aid>/presence` (no token): `{people, viewing}`, the
/// viewers here, away or recently gone (spec §10, "Presence"), never a
/// cookie, and how many are viewing: the people seated and the spectators
/// seen in the last 15 s (spec 2026-10-11-clax-dev-hosted-design §16.3). An
/// agent reads a viewer who is not connected as `<name> (not connected)`
/// (spec 2026-10-08 §9.6).
pub async fn presence(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::Presence>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let aid = id.as_str().to_string();
    let reg = s.presence.clone();
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let people = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let mut people = json!(reg.for_artifact(id.as_str()));
            vf.build(st)?.mark_viewers(&mut people, "public_id");
            Ok(people)
        })
        .await?;
    let viewing = s.seats.counts(&aid).viewing;
    Ok(Json(json!({"people": people, "viewing": viewing})))
}

/// Pins or edits an artifact's metadata. A person's agent edits from a
/// session, never as a page (spec §6.7); the answer is filtered for an
/// agent channel's reader.
pub async fn patch(
    State(s): State<AppState>,
    a: TokenOrAgent<clax_http::agent::ArtifactsPatch>,
    headers: HeaderMap,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    person_agent_session(&a.0.caller)?;
    // Who sets the text fields (spec §8.2): a person's agent's person, else
    // the owner; a page's patch also carries its page's authors.
    let by = match &a.0.caller {
        clax_http::auth::Caller::PersonAgent { person, .. } => {
            clax_core::visibility::Author::of_token(person.as_deref())
        }
        _ => clax_core::visibility::Author::Owner,
    };
    let via_page = page_marker(&a.0.caller, &headers)?;
    if let Some(c) = &b.capabilities {
        clax_core::capabilities::validate(c)?;
    }
    clax_core::publish::check_update_title(b.title.as_deref())?;
    clax_core::publish::check_meta_text(
        b.title.as_deref(),
        b.description.as_deref(),
        b.icon.as_deref(),
    )?;
    admin_change(&s, &a.0.caller, &id, b.pinned, b.capabilities.clone()).await?;
    let vf = VisFor::of(&a.0.caller, &s);
    let artifact = s
        .store_call(move |st| {
            let a = st.update_meta(
                &audit,
                &id,
                MetaPatch {
                    title: b.title,
                    description: b.description,
                    icon: b.icon,
                    pinned: b.pinned,
                    capabilities: b.capabilities,
                    by: Some(by),
                    via_page,
                },
            )?;
            let mut view = json!(a);
            let vis = vf.build(st)?;
            vis.filter_artifact(st, &a, &[], &mut view)?;
            Ok(view)
        })
        .await?;
    let mut v = json!({"artifact": artifact});
    if !a.0.holds_token() {
        strip_sessions(&mut v);
    }
    Ok(Json(v))
}

/// Deletes an artifact. A person's agent deletes from a session (400
/// `no_session`), as it writes.
pub async fn delete(
    State(s): State<AppState>,
    a: TokenOrAgent<clax_http::agent::ArtifactsDelete>,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    person_agent_session(&a.0.caller)?;
    let id = parse_id(&path(aid)?)?;
    let events = s.events.clone();
    let cache = s.wrap_cache.clone();
    let working = s.working.clone();
    let live_ids = s.live_ids.clone();
    let drafts = s.drafts.clone();
    s.store_call_async(move |st| async move {
        let st = &*st;
        st.delete_artifact_async(&audit, &id).await?;
        // Its drafts go with it; `artifact_deleted` tells the people.
        drafts.end_artifact(id.as_str());
        let gone = working.artifact_gone(id.as_str());
        clax_realtime::working::settle(st, &audit, &events, &working, &gone);
        cache.remove_artifact(id.as_str());
        events.publish(Event::ArtifactDeleted {
            artifact_id: id.as_str().to_string(),
        });
        // After the event, so a live page's deletion reaches only the
        // streams that saw the page.
        live_ids.remove(id.as_str());
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every version of the artifact, filtered for an agent channel's reader;
/// without the token, no session ID.
pub async fn list_versions(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::VersionsList>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let vf = VisFor::of(&a.caller, &s);
    let versions = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            Ok(vis.filter_versions(&st.list_versions(&id)?))
        })
        .await?;
    let mut v = json!({"versions": versions});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    Ok(Json(v))
}

/// Publishes the next version. A person's agent publishes from a session,
/// as its person and never as a page (spec §6.7, §8.2); the answer is
/// filtered for an agent channel's reader, so a file carried forward from
/// someone it has not approved stays withheld.
pub async fn publish(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::Publish>,
    audit: AuditCtx,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&path(aid)?)?;
    if s.live_ids.contains(id.as_str()) {
        return Err(ApiError::bad_request(
            "live_page",
            "a live page takes snapshots from the Clax extension; it cannot be published",
        ));
    }
    person_agent_session(&a.caller)?;
    let by_page = page_marker(&a.caller, &headers)?;
    let mut p = parse_body(req, "the publish limit", |r: PublishRequest| {
        Ok(validate(r)?)
    })
    .await?;
    clax_core::publish::check_update_title(p.title.as_deref())?;
    let truncated = p.note_truncated;
    let session = session_header(&headers)?;
    let grant = a.grant().map(str::to_string);
    let key = publish_key(&headers, grant.as_deref())?;
    p.by_page = by_page;
    p.person = publisher(&a.caller);
    admin_change(&s, &a.caller, &id, None, p.capabilities.clone()).await?;
    // A person's agent's publish leaves the owner's agents' untargeted
    // feedback where it is: it is not the person's agent's to take.
    let retarget = grant.is_none();
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let vf = VisFor::of(&a.caller, &s);
    let drafts = s.drafts.clone();
    let (artifact, version) = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let session = publishing_session(st, &session, grant.as_deref())?;
            let _keyed = lock_key(st, &grant, &key).await;
            // A publish sent again whose first answer was lost: what it made.
            if let Some(done) = replayed(
                st,
                grant.as_deref(),
                key.as_deref(),
                Some(&id),
                &vf.build(st)?,
            )? {
                return Ok(done);
            }
            // Read before the clear below: the threads this session was
            // working on are linked to the new version.
            if let Some(sid) = &session {
                p.working_threads = ctx.working.threads_of(sid, id.as_str());
            }
            // The previous version's files the reader may not see never
            // constrain the publish (spec §21.7).
            let vis = vf.build(st)?;
            if vis.filters()
                && let Some(n) = p.if_version
                && let Some(prev) = st.get_version(&id, n)?
            {
                p.hidden = vis.filter_hidden_files(&prev);
            }
            p.publish_key = publish_key_of(&grant, &key);
            let (artifact, version) = st
                .publish_version_async(&audit, &id, p, session.as_deref())
                .await?;
            if let Some(sid) = &session {
                let aid = ArtifactId::parse(&artifact.id)?;
                st.ensure_watch(&audit, sid, &aid)?;
                let (changed, _) = ctx.working.clear(
                    sid,
                    artifact.id.as_str(),
                    None,
                    clax_core::working::End::Publish,
                );
                clax_realtime::working::settle(st, &audit, &ctx.events, &ctx.working, &changed);
                // The version lands: the session's draft on it ends.
                clax_realtime::drafts::end(&events, &drafts, sid, artifact.id.as_str());
                if retarget {
                    let touched = st.retarget_untargeted(&aid, sid)?;
                    clax_realtime::feedback::apply(&ctx, st, &touched);
                }
            }
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page,
                title: Some(artifact.title.clone()),
                at: Some(version.created_at.clone()),
            });
            for tid in &version.addresses {
                if let Some(t) = st.get_thread(tid)? {
                    crate::routes::threads::publish_thread(&ctx, st, &t)?;
                }
            }
            vis.filter_published(st, &artifact, &version)
        })
        .await?;
    // A public twin that follows new versions gets this one.
    if let Ok(id) = clax_core::ArtifactId::parse(artifact["id"].as_str().unwrap_or_default()) {
        crate::public::after_publish(&s, &id);
    }
    let url = format!("/a/{}", artifact["id"].as_str().unwrap_or_default());
    let mut v =
        json!({"artifact": artifact, "version": version, "url": url, "note_truncated": truncated});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    Ok((StatusCode::CREATED, Json(v)))
}

/// Version `n`, filtered for an agent channel's reader; without the token,
/// no session ID.
pub async fn get_version(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::VersionGet>,
    params: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, n) = path(params)?;
    let id = parse_id(&aid)?;
    let vf = VisFor::of(&a.caller, &s);
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = st.get_version(&id, n)?.ok_or(CoreError::NotFound)?;
            let mut view = json!(v);
            let vis = vf.build(st)?;
            vis.filter_version(&v, &mut view);
            Ok(view)
        })
        .await?;
    let mut v = json!({"version": v});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    Ok(Json(v))
}

/// The current version's files: `{files, version}`, each file an agent
/// channel's reader may not see left out (`files_withheld`).
pub async fn files(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::FilesList>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let vf = VisFor::of(&a.caller, &s);
    let v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = st
                .get_version(&id, a.current_version)?
                .ok_or(CoreError::NotFound)?;
            let mut view = json!({"files": v.files, "version": v.n});
            let vis = vf.build(st)?;
            vis.filter_files(&v, &mut view);
            Ok(view)
        })
        .await?;
    Ok(Json(v))
}

/// Who reads a fork's lineage ([`forked_from_view`]): whether they may
/// read its source decides whether its title is named.
#[derive(Clone)]
pub(crate) struct SourceReader {
    caller: clax_http::auth::Caller,
    member: Option<clax_http::hosted::MemberRights>,
    hosted: bool,
}

impl SourceReader {
    /// `caller`, a hosted daemon's signed-in person `member` (`None`: no
    /// member), on a hosted daemon when `hosted`.
    pub(crate) fn new(
        caller: clax_http::auth::Caller,
        member: Option<clax_http::hosted::MemberRights>,
        hosted: bool,
    ) -> SourceReader {
        SourceReader {
            caller,
            member,
            hosted,
        }
    }

    /// Whether they may read artifact `src`: the owner and the owner's
    /// agents always; a person's agent and a hosted daemon's signed-in
    /// person by their rights (the person's also by `src`'s link); a local
    /// daemon's connected person by their person rights; on a hosted daemon
    /// a guest or no session only when `src` is shared by link, and on a
    /// local daemon, whose browsers read every artifact, always.
    fn may_read(&self, st: &Store, src: &Artifact) -> clax_core::Result<bool> {
        use clax_http::auth::Caller;
        let read = (RightType::Artifacts, "read");
        if let Some(m) = &self.member {
            return m.allows_at(st, read, &src.id);
        }
        Ok(match &self.caller {
            Caller::Owner { .. } | Caller::OwnerAgent { .. } => true,
            Caller::PersonAgent { person, rights, .. } => {
                rights.allows(RightType::Artifacts, "read", Some(&src.id), &|x| {
                    x == src.id && src.authors.creator == *person
                })
            }
            Caller::Person { viewer } => {
                let pid = viewer.public_id.as_str();
                let own = |x: &str| st.artifact_created_by(x, Some(pid)).unwrap_or(false);
                st.person_rights(pid)?
                    .allows(RightType::Artifacts, "read", Some(&src.id), &own)
            }
            Caller::Viewer { .. } | Caller::Anonymous if self.hosted => {
                st.link_access(&src.id)?.is_some()
            }
            Caller::Viewer { .. } | Caller::Anonymous => true,
        })
    }
}

/// `a`'s lineage as `reader` sees it: `{artifact_id, version, title}`, the
/// title the source's (as `vis` filters it for an agent) when it still
/// exists and `reader` may read it, else null; `None` when `a` is no fork.
///
/// # Errors
/// The store's.
pub(crate) fn forked_from_view(
    st: &Store,
    a: &Artifact,
    reader: &SourceReader,
    vis: Option<&clax_http::views::thread::Vis>,
) -> clax_core::Result<Option<Value>> {
    let Some(f) = &a.forked_from else {
        return Ok(None);
    };
    let source = match ArtifactId::parse(&f.artifact_id) {
        Ok(id) => st.get_artifact(&id)?,
        Err(_) => None,
    };
    let title = match source {
        Some(s) if reader.may_read(st, &s)? => {
            let mut view = json!(s);
            if let Some(vis) = vis {
                vis.filter_artifact(st, &s, &[], &mut view)?;
            }
            view["title"].clone()
        }
        _ => Value::Null,
    };
    Ok(Some(
        json!({"artifact_id": f.artifact_id, "version": f.version, "title": title}),
    ))
}

/// Why `caller` may not fork artifact `aid` (one they may read, as the
/// route's extractor checked for an agent and a hosted daemon's signed-in
/// person, `member`), or `None` when they may: a person's agent and
/// `member` need `create` in their rights; a local daemon's connected
/// person needs read on `aid` (404 without) and `create` in their own
/// person rights; a guest is refused 403 `not_permitted`, no credential
/// 401; the owner and the owner's agents may. A live page is never forked.
///
/// # Errors
/// The store's.
pub(crate) fn fork_refusal(
    st: &Store,
    caller: &clax_http::auth::Caller,
    member: Option<&clax_core::rights::Rights>,
    aid: &str,
) -> clax_core::Result<Option<ApiError>> {
    use clax_http::auth::Caller;
    let create =
        |r: &clax_core::rights::Rights| r.allows(RightType::Artifacts, "create", None, &|_| false);
    let no_create = || {
        Some(ApiError::forbidden(
            "not_permitted",
            "your rights here do not let you create (clax-artifacts)",
        ))
    };
    if member.is_some_and(|m| !create(m)) {
        return Ok(no_create());
    }
    Ok(match caller {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => None,
        Caller::PersonAgent { rights, .. } => (!create(rights)).then(no_create).flatten(),
        Caller::Person { .. } if member.is_some() => None,
        Caller::Person { viewer } => {
            let pid = viewer.public_id.as_str();
            let rights = st.person_rights(pid)?;
            let own = |x: &str| st.artifact_created_by(x, Some(pid)).unwrap_or(false);
            if !rights.allows(RightType::Artifacts, "read", Some(aid), &own) {
                Some(ApiError::not_found())
            } else if !create(&rights) {
                no_create()
            } else {
                None
            }
        }
        Caller::Viewer { .. } => Some(ApiError::forbidden(
            "not_permitted",
            "a guest cannot fork: sign in as a person of this Clax first",
        )),
        Caller::Anonymous => Some(ApiError::unauthorized()),
    })
}

/// `POST /api/artifacts/<aid>/fork` body.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ForkBody {
    /// The version to fork; the current version when absent.
    version: Option<u32>,
}

/// Forks version `version` (default the current one) of the artifact into
/// a new artifact of the caller's (spec 2026-10-11-clax-dev-hosted-design
/// H11; [`clax_store::forks`]): `{artifact, version, url}` with
/// `artifact.forked_from`, 201. It needs read on the source and `create`
/// (multi-person rights): a person's agent's, a hosted daemon's signed-in
/// person's, or a local daemon's connected person's own rights; the owner
/// and the owner's agents hold both. A guest (a named viewer who is no
/// connected person) is refused 403 `not_permitted`, no credential 401;
/// a person beyond the live seats forks as one within them. A person's agent
/// forks from a session, as its person; the fork is the forking person's,
/// and the session, when one is named, watches it.
pub async fn fork(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ArtifactsFork>,
    _o: clax_http::viewer::SameOrigin,
    audit: AuditCtx,
    req: Result<Json<ForkBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    use clax_http::auth::Caller;
    let id = a.artifact()?;
    let b = body(req)?;
    if s.live_ids.contains(id.as_str()) {
        return Err(ApiError::bad_request(
            "live_page",
            "a live page cannot be forked: its versions are snapshots of a page elsewhere",
        ));
    }
    person_agent_session(&a.caller)?;
    let (c, m, aid) = (a.caller.clone(), a.member_rights(), id.as_str().to_string());
    s.store_call(move |st| fork_refusal(st, &c, m.as_deref(), &aid))
        .await?
        .map_or(Ok(()), Err)?;
    // Who forks: the owner (no person), a person's agent's person, or a
    // connected person in their browser.
    let person: Option<String> = match &a.caller {
        Caller::PersonAgent { person, .. } => person.clone(),
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        _ => None,
    };
    // A spectator forks too: a fork writes nothing of the source's (spec
    // 2026-10-11-clax-dev-hosted-design §7: beyond the seats one reads and
    // forks).
    // The session the verified caller speaks for: an agent's alone. A
    // browser's X-Clax-Session names no session of its own.
    let session = a.caller.session().map(str::to_string);
    let grant = a.grant().map(str::to_string);
    let events = s.events.clone();
    let vf = VisFor::of(&a.caller, &s);
    let reader = SourceReader::new(a.caller.clone(), a.member().cloned(), s.hosted.is_some());
    let (mut artifact, version, lineage) = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let session = publishing_session(st, &session, grant.as_deref())?;
            let (artifact, version) = st
                .fork_artifact_async(
                    &audit,
                    &id,
                    b.version,
                    person.as_deref(),
                    session.as_deref(),
                )
                .await?;
            if let Some(sid) = &session {
                st.ensure_watch(&audit, sid, &ArtifactId::parse(&artifact.id)?)?;
            }
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page: false,
                title: Some(artifact.title.clone()),
                at: Some(version.created_at.clone()),
            });
            let vis = vf.build(st)?;
            let lineage = forked_from_view(st, &artifact, &reader, Some(&vis))?;
            let (a, v) = vis.filter_published(st, &artifact, &version)?;
            Ok((a, v, lineage))
        })
        .await?;
    artifact["forked_from"] = json!(lineage);
    let url = format!("/a/{}", artifact["id"].as_str().unwrap_or_default());
    let mut v = json!({"artifact": artifact, "version": version, "url": url});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
    clax_http::hosted::stamp_content_keys(&s, &mut v);
    Ok((StatusCode::CREATED, Json(v)))
}

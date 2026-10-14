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
use clax_core::model::Session;
use clax_core::publish::{PublishRequest, require_title, validate};
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
    tokio::task::spawn_blocking(work).await.map_err(|e| {
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
/// token, M4).
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
        Some(sess) if sess.ended_at.is_none() => Ok(Some(sess.id)),
        _ => Err(CoreError::invalid(
            "unknown_session",
            "X-Clax-Session names no live session",
        )),
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
/// §8.3, O9).
pub async fn list(
    State(s): State<AppState>,
    a: Agent<ArtifactsList>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let local = clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let reads = a.read_rights();
    let vf = VisFor::of(&a.caller, &s);
    let artifacts = match q.artifact.as_deref().map(parse_id).transpose()? {
        Some(id) => {
            let working = s.working.for_artifact(id.as_str());
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
                let mut view = with_owner(
                    &a,
                    owner.as_ref(),
                    &working,
                    &st.participants(&id)?,
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
            // The whole listing costs what the home does: the bulk lane.
            s.store_call_bulk(move |st| {
                let artifacts = st.list_artifacts()?;
                let owners: std::collections::HashMap<String, Session> = st
                    .list_sessions(false)?
                    .into_iter()
                    .map(|s| (s.id.clone(), s))
                    .collect();
                let participants = st.participants_all()?;
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
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let vf = VisFor::of(&a.caller, &s);
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session, grant.as_deref())?;
            let (artifact, version) = st.create_artifact(&audit, p, session.as_deref())?;
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
    Ok((StatusCode::CREATED, Json(v)))
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
/// (the owner for the owner cookie, else the viewer cookie's; see
/// [`clax_http::identity`]), also that viewer's `attention`, and the response is
/// private to the cookie.
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
    let has_viewer = who.owner_browser() || who.cookie.is_some();
    let vf = VisFor::of(&a.caller, &s);
    let mut v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let versions = st.list_versions(&id)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            let vis = vf.build(st)?;
            let mut artifact = with_owner(
                &a,
                owner.as_ref(),
                &working,
                &st.participants(&id)?,
                live_part(st, &a)?.as_ref(),
            );
            vis.filter_artifact(st, &a, &working, &mut artifact)?;
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
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
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

/// Metadata edits. `capabilities` replaces the whole declaration (omitted
/// keeps, `{}` clears) and is validated like a publish's.
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
    s.store_call(move |st| {
        st.delete_artifact(&audit, &id)?;
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
    let truncated = p.note_truncated;
    let session = session_header(&headers)?;
    let grant = a.grant().map(str::to_string);
    p.by_page = by_page;
    p.person = publisher(&a.caller);
    // A person's agent's publish leaves the owner's agents' untargeted
    // feedback where it is: it is not the person's agent's to take.
    let retarget = grant.is_none();
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let vf = VisFor::of(&a.caller, &s);
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session, grant.as_deref())?;
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
            let (artifact, version) = st.publish_version(&audit, &id, p, session.as_deref())?;
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
    let url = format!("/a/{}", artifact["id"].as_str().unwrap_or_default());
    let mut v =
        json!({"artifact": artifact, "version": version, "url": url, "note_truncated": truncated});
    if !a.holds_token() {
        strip_sessions(&mut v);
    }
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

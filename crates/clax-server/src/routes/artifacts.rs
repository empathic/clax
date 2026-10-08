//! REST routes for artifacts and versions.

use crate::auth::RequireToken;
use crate::auth::has_token;
use crate::error::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::rejection::{JsonRejection, MissingJsonContentType, QueryRejection};
use axum::extract::{FromRequest, Request};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::live::KIND_LIVE;
use clax_core::model::{Artifact, Session};
use clax_core::publish::{PublishRequest, require_title, validate};
use clax_core::store::live::LivePage;
use clax_core::{ArtifactId, CoreError, Event, MetaPatch, Participants, Store};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Request body cap for the publish routes. A fully base64-encoded 64 MiB
/// payload is about 85 MiB on the wire, so the cap sits above that;
/// `validate` still enforces the 64 MiB decoded limit.
pub const PUBLISH_BODY_LIMIT: usize = 96 * 1024 * 1024;

pub fn parse_id(raw: &str) -> Result<ArtifactId, ApiError> {
    ArtifactId::parse(raw).map_err(ApiError::from)
}

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

/// Header a shell sets when the page itself publishes (`artifact.publish`):
/// the version event then carries `by_page`.
pub const VIA_HEADER: &str = "x-clax-via";

/// Header naming the session a publish is attributed to.
pub const SESSION_HEADER: &str = "x-clax-session";

/// The session named by `X-Clax-Session`, checked to exist and be live.
///
/// # Errors
/// `unknown_session` when the header is not valid text, or names a session that
/// does not exist or has ended.
pub(crate) fn publishing_session(
    st: &Store,
    header: &Option<String>,
) -> Result<Option<String>, CoreError> {
    let Some(id) = header else {
        return Ok(None);
    };
    match st.get_session(id)? {
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

/// `a` as JSON with `owner_live` (its owner session exists and has not ended),
/// `owner_harness` (the owner's harness, when it exists) and `participants`
/// (people by public ID, agents by handle; see [`Store::participants`]).
/// A live page (`kind` `live`) also carries `live: {origin, path, page_url,
/// origins}` from `live`: `origins` are its site's (spec §7.2), the most
/// recently used first, and `page_url` is the page's path on the first.
/// `working` is the artifact's working list (spec §10 "Working"), which
/// never names a session. The
/// artifact's own `owner_session_id` stays; token-less routes drop it with
/// [`strip_sessions`].
pub(crate) fn with_owner(
    a: &Artifact,
    owner: Option<&Session>,
    working: &[clax_core::working::WorkingView],
    participants: &Participants,
    live: Option<&LivePart>,
) -> Value {
    let mut v = serde_json::to_value(a).expect("serialisable artifact");
    v["owner_live"] = json!(owner.is_some_and(|o| o.ended_at.is_none()));
    v["owner_harness"] = json!(owner.map(|o| &o.harness));
    v["working"] = json!(working);
    v["participants"] = json!(participants);
    if let Some(LivePart {
        page: p,
        origins,
        merged_into,
    }) = live
    {
        let newest = origins.first().unwrap_or(&p.origin);
        v["live"] = json!({
            "origin": p.origin,
            "path": p.path,
            "page_url": format!("{newest}{}", p.path),
            "origins": origins,
        });
        if let Some((m, url)) = merged_into {
            v["live"]["merged_into"] = json!(m);
            v["live"]["merged_into_url"] = json!(url);
        }
    }
    v
}

/// A live page with its site's origins, the most recently used first.
pub(crate) struct LivePart {
    pub page: LivePage,
    pub origins: Vec<String>,
    /// A page a join merged away (spec §7.2): the page it was merged into,
    /// and that page's URL.
    pub merged_into: Option<(String, Option<String>)>,
}

/// The live page `a` is, when it is one.
pub(crate) fn live_part(st: &Store, a: &Artifact) -> clax_core::Result<Option<LivePart>> {
    if a.kind != KIND_LIVE {
        return Ok(None);
    }
    let Some(page) = st.live_page_of(&ArtifactId::parse(&a.id)?)? else {
        let Some((origin, path, into)) = st.merged_live_page(&a.id)? else {
            return Ok(None);
        };
        let page = LivePage {
            artifact_id: a.id.clone(),
            origin: origin.clone(),
            path,
        };
        return Ok(Some(LivePart {
            page,
            origins: vec![origin],
            merged_into: match into {
                Some(i) => {
                    let url = st
                        .live_page_of(&ArtifactId::parse(&i)?)?
                        .map(|p| format!("{}{}", p.origin, p.path));
                    Some((i, url))
                }
                None => None,
            },
        }));
    };
    let origins = st.joined_site(&page.origin)?.origin_names();
    Ok(Some(LivePart {
        page,
        origins,
        merged_into: None,
    }))
}

/// Leaves out the session IDs a token-less caller must not see (spec §14):
/// the artifact's `owner_session_id` and each version's `session_id`.
pub(crate) fn strip_sessions(v: &mut Value) {
    if let Some(a) = v.get_mut("artifact").and_then(Value::as_object_mut) {
        a.remove("owner_session_id");
    }
    if let Some(x) = v.get_mut("version").and_then(Value::as_object_mut) {
        x.remove("session_id");
    }
    for x in v
        .get_mut("versions")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(x) = x.as_object_mut() {
            x.remove("session_id");
        }
    }
    for x in v
        .get_mut("artifacts")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(x) = x.as_object_mut() {
            x.remove("owner_session_id");
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    artifact: Option<String>,
}

/// Each live artifact, with the owner fields of [`with_owner`]. Without the
/// token, no session ID is included. With `?artifact=<aid>`, that artifact
/// alone, or none when it is not live (400 for a malformed ID). Live pages
/// are left out for a caller that may not see them
/// ([`crate::live::sees_live_pages`]).
pub async fn list(
    State(s): State<AppState>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let local = crate::live::sees_live_pages(&headers, &extensions, &s.token);
    let artifacts = match q.artifact.as_deref().map(parse_id).transpose()? {
        Some(id) => {
            let working = s.working.for_artifact(id.as_str());
            s.store_call(move |st| {
                let Some(a) = st.get_artifact(&id)? else {
                    return Ok(vec![]);
                };
                if !local && a.kind == KIND_LIVE {
                    return Ok(vec![]);
                }
                let owner = match &a.owner_session_id {
                    Some(sid) => st.get_session(sid)?,
                    None => None,
                };
                Ok(vec![with_owner(
                    &a,
                    owner.as_ref(),
                    &working,
                    &st.participants(&id)?,
                    live_part(st, &a)?.as_ref(),
                )])
            })
            .await?
        }
        None => {
            let all = s.working.all();
            s.store_call(move |st| {
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
                Ok(artifacts
                    .iter()
                    .filter(|a| local || a.kind != KIND_LIVE)
                    .filter(|a| !merged.contains(&a.id))
                    .map(|a| {
                        let owner = a.owner_session_id.as_ref().and_then(|sid| owners.get(sid));
                        let working = all.get(&a.id).map(Vec::as_slice).unwrap_or(&[]);
                        let people = participants.get(&a.id).unwrap_or(&none);
                        with_owner(a, owner, working, people, pages.get(&a.id))
                    })
                    .collect::<Vec<_>>())
            })
            .await?
        }
    };
    let mut v = json!({"artifacts": artifacts});
    if !has_token(&headers, &s.token) {
        strip_sessions(&mut v);
    }
    Ok(Json(v))
}

/// Creates an artifact. The body must carry a non-blank `title`.
pub async fn create(
    State(s): State<AppState>,
    _t: RequireToken,
    headers: HeaderMap,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let p = parse_body(req, "the publish limit", |r: PublishRequest| {
        Ok(validate(r)?)
    })
    .await?;
    require_title(p.title.as_deref())?;
    let truncated = p.note_truncated;
    let session = session_header(&headers)?;
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session)?;
            let (artifact, version) = st.create_artifact(p, session.as_deref())?;
            if let Some(sid) = &session {
                let aid = ArtifactId::parse(&artifact.id)?;
                st.ensure_watch(sid, &aid)?;
                let touched = st.retarget_untargeted(&aid, sid)?;
                crate::feedback::apply(&ctx, st, &touched);
            }
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page: false,
                title: Some(artifact.title.clone()),
                at: Some(version.created_at.clone()),
            });
            Ok((artifact, version))
        })
        .await?;
    let url = format!("/a/{}", artifact.id);
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"artifact": artifact, "version": version, "url": url, "note_truncated": truncated}),
        ),
    ))
}

/// The artifact (with the owner fields of [`with_owner`]) and its versions.
/// Without the token, no session ID is included. From a browser with a viewer
/// (the owner for the owner cookie, else the viewer cookie's; see
/// [`crate::identity`]), also that viewer's `attention`, and the response is
/// private to the cookie.
pub async fn get(
    State(s): State<AppState>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let who = crate::identity::Identity::from_parts(&headers, &extensions, &s.token);
    let working = s.working.for_artifact(id.as_str());
    let has_viewer = who.owner_browser() || who.cookie.is_some();
    let mut v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let versions = st.list_versions(&id)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            let mut v = json!({
                "artifact": with_owner(
                    &a,
                    owner.as_ref(),
                    &working,
                    &st.participants(&id)?,
                    live_part(st, &a)?.as_ref(),
                ),
                "versions": versions,
            });
            if let Some(viewer) = who.browser_viewer(st)? {
                v["attention"] = json!(st.attention(&viewer.id, &id)?);
            }
            Ok(v)
        })
        .await?;
    if !has_token(&headers, &s.token) {
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

/// `GET /api/artifacts/<aid>/presence` (no token): `{people}`, the viewers
/// here, away or recently gone (spec §10, "Presence"), never a cookie.
pub async fn presence(
    State(s): State<AppState>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let reg = s.presence.clone();
    let people = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            Ok(reg.for_artifact(id.as_str()))
        })
        .await?;
    Ok(Json(json!({"people": people})))
}

pub async fn patch(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    if let Some(c) = &b.capabilities {
        clax_core::capabilities::validate(c)?;
    }
    let artifact = s
        .store_call(move |st| {
            st.update_meta(
                &id,
                MetaPatch {
                    title: b.title,
                    description: b.description,
                    icon: b.icon,
                    pinned: b.pinned,
                    capabilities: b.capabilities,
                },
            )
        })
        .await?;
    Ok(Json(json!({"artifact": artifact})))
}

pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let events = s.events.clone();
    let cache = s.wrap_cache.clone();
    let working = s.working.clone();
    let live_ids = s.live_ids.clone();
    s.store_call(move |st| {
        st.delete_artifact(&id)?;
        crate::working::announce(&events, &working, &working.artifact_gone(id.as_str()));
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

/// Every version of the artifact; without the token, no session ID.
pub async fn list_versions(
    State(s): State<AppState>,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let versions = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.list_versions(&id)
        })
        .await?;
    let mut v = json!({"versions": versions});
    if !has_token(&headers, &s.token) {
        strip_sessions(&mut v);
    }
    Ok(Json(v))
}

pub async fn publish(
    State(s): State<AppState>,
    _t: RequireToken,
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
    let mut p = parse_body(req, "the publish limit", |r: PublishRequest| {
        Ok(validate(r)?)
    })
    .await?;
    let truncated = p.note_truncated;
    let session = session_header(&headers)?;
    let by_page = headers.get(VIA_HEADER).and_then(|v| v.to_str().ok()) == Some("page");
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session)?;
            // Read before the clear below: the threads this session was
            // working on are linked to the new version.
            if let Some(sid) = &session {
                p.working_threads = ctx.working.threads_of(sid, id.as_str());
            }
            let (artifact, version) = st.publish_version(&id, p, session.as_deref())?;
            if let Some(sid) = &session {
                let aid = ArtifactId::parse(&artifact.id)?;
                st.ensure_watch(sid, &aid)?;
                let (changed, _) = ctx.working.clear(
                    sid,
                    artifact.id.as_str(),
                    None,
                    clax_core::working::End::Publish,
                );
                crate::working::announce(&ctx.events, &ctx.working, &changed);
                let touched = st.retarget_untargeted(&aid, sid)?;
                crate::feedback::apply(&ctx, st, &touched);
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
            Ok((artifact, version))
        })
        .await?;
    let url = format!("/a/{}", artifact.id);
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"artifact": artifact, "version": version, "url": url, "note_truncated": truncated}),
        ),
    ))
}

/// Version `n`; without the token, no session ID.
pub async fn get_version(
    State(s): State<AppState>,
    headers: HeaderMap,
    params: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, n) = path(params)?;
    let id = parse_id(&aid)?;
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.get_version(&id, n)?.ok_or(CoreError::NotFound)
        })
        .await?;
    let mut v = json!({"version": v});
    if !has_token(&headers, &s.token) {
        strip_sessions(&mut v);
    }
    Ok(Json(v))
}

pub async fn files(
    State(s): State<AppState>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.get_version(&id, a.current_version)?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(Json(json!({"files": v.files, "version": v.n})))
}

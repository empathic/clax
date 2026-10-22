//! The page runtime's backend on an artifact's own site host:
//! `/_clax/api/…`, scoped by the host to its artifact
//! ([`clax_http::host::SiteRequest`]). No path, query or body field names an
//! artifact: each route is the public host's `/api/artifacts/<id>/…` route
//! with the host's artifact as `<id>`, with the same request and response
//! bodies.
//!
//! | `/_clax/api/…` | Methods | The route it is |
//! |---|---|---|
//! | `docs/<path>` | `GET` `PUT` `PATCH` `DELETE` | `/api/artifacts/<id>/docs/<path>` |
//! | `docs` | `GET` (a query) | `/api/artifacts/<id>/docs` |
//! | `docs:batch`, `docs:acquire`, `docs:str_replace` | `POST` | `/api/artifacts/<id>/docs:…` |
//! | `assets` | `POST` (an upload), `GET` (the list) | `/api/artifacts/<id>/assets` |
//! | `assets/<asset ID>` | `DELETE` | `/api/artifacts/<id>/assets/<asset ID>` (an asset of `<id>`'s alone) |
//! | `room` | `GET` (a WebSocket) | `/api/artifacts/<id>/room` |
//! | `artifact` | `GET` | `/api/artifacts/<id>` |
//! | `whoami` | `GET` | `{kind, id?, name?, level, can_edit}` ([`whoami`]) |
//!
//! Anything else under `/_clax/api/` is 404: comments and threads are the
//! review UI's, served on the public host alone.
//!
//! - **Who calls** is who the site's access check found
//!   ([`clax_http::site_access::SiteCaller`]), and no one else: the request's
//!   own `Authorization` is refused (401 `unauthorized`), its cookies and
//!   every `x-clax-…` header are dropped (so a page forges no tool call or
//!   git context into the audit), and the route sees the holder's identity as
//!   the public host would for their browser (the owner's with the token,
//!   as the owner's shell; a person's with their rights; a guest's; a link
//!   viewer's or a visitor's, which is no one's). Its `db` level is
//!   [`clax_http::db_caller::site_caller`]'s; every write is a page's.
//! - **Origin:** a request whose `Origin` is not the site's own is 403
//!   `forbidden_origin`, and a write (any method but `GET` and `HEAD`)
//!   without one is too. No answer carries `Access-Control-Allow-Origin`.
//! - **A visitor** (a site open to the world) reads only `artifact`'s
//!   `{artifact: {id, title, current_version, capabilities}}`. On a hosted daemon each
//!   write of a visitor, a link viewer or a guest is counted per client
//!   address and per artifact ([`clax_http::hosted::Hosted::site_write`],
//!   429 `rate_limited`). Closing the site to the world refuses a visitor
//!   on their next request (`site_access_layer`) and closes their room
//!   socket (`super::room`, on `Event::SiteAccess`).

use super::{artifacts, assets, docs, room};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request};
use axum::handler::Handler;
use axum::http::{HeaderValue, Method, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clax_core::ArtifactId;
use clax_core::rights::RightType;
use clax_http::agent;
use clax_http::error::ApiError;
use clax_http::host::SiteRequest;
use clax_http::hosted::{HostedRequest, MemberRights};
use clax_http::site_access::SiteCaller;
use clax_http::state::AppState;
use clax_store::hosted::SessionViewer;
use clax_store::site_sessions::Holder;
use serde_json::{Value, json};

/// Where the site API is mounted on a site host.
pub const PREFIX: &str = "/_clax/api/";

/// Whether a request on the site API never passes header `name` on: its
/// cookies, and every Clax header (`x-clax-…`: a session, a channel, a
/// tool call, a git context), which would make it speak for someone else
/// or forge what its audit records.
fn dropped(name: &str) -> bool {
    name.eq_ignore_ascii_case("cookie") || name.to_ascii_lowercase().starts_with("x-clax-")
}

/// The public host's routes the site API reaches, each at its
/// `/api/artifacts/{aid}/…` path and with its body limit there
/// (`crate::router`).
fn inner(state: &AppState) -> axum::Router {
    let r = Router::new()
        .route("/api/artifacts/{aid}", get(artifacts::get))
        .route("/api/artifacts/{aid}/docs", get(docs::list))
        .route(
            "/api/artifacts/{aid}/docs/{*path}",
            get(docs::get)
                .put(docs::put.layer(DefaultBodyLimit::max(
                    <agent::DocsPut as agent::Need>::LIMIT,
                )))
                .patch(docs::patch)
                .delete(docs::delete),
        )
        .route(
            "/api/artifacts/{aid}/docs:batch",
            post(docs::batch.layer(DefaultBodyLimit::max(clax_http::limits::DOCS_BATCH_LIMIT))),
        )
        .route(
            "/api/artifacts/{aid}/docs:str_replace",
            post(docs::str_replace),
        )
        .route("/api/artifacts/{aid}/docs:acquire", post(docs::acquire))
        .route(
            "/api/artifacts/{aid}/assets",
            get(assets::list).post(
                assets::upload.layer(DefaultBodyLimit::max(clax_http::limits::UPLOAD_BODY_LIMIT)),
            ),
        )
        .route(
            "/api/artifacts/{aid}/assets/{asset_id}",
            axum::routing::delete(assets::delete),
        );
    #[cfg(not(target_arch = "wasm32"))]
    let r = r.route("/api/artifacts/{aid}/room", get(room::room));
    #[cfg(target_arch = "wasm32")]
    let r = r.route("/api/artifacts/{aid}/room", get(room::room_socket));
    r.fallback(|| async { ApiError::not_found() })
        .with_state(state.clone())
}

/// `/_clax/api/…` for the site routes (`super::site`), under the site's
/// access check. Nested as a service, so the public host's routes see
/// their own path parameters alone.
pub fn routes(state: &AppState) -> Router<AppState> {
    let api = inner(state);
    let s = state.clone();
    Router::new().nest_service(
        "/_clax/api",
        tower::service_fn(move |req: Request| {
            let (s, api) = (s.clone(), api.clone());
            async move { Ok::<_, std::convert::Infallible>(dispatch(s, api, req).await) }
        }),
    )
}

/// The public host's path of site API path `rest` on artifact `aid`, when
/// the site API serves it.
fn inner_path(aid: &ArtifactId, rest: &str) -> Option<String> {
    let base = format!("/api/artifacts/{}", aid.as_str());
    match rest {
        "artifact" => Some(base),
        "docs" | "docs:batch" | "docs:acquire" | "docs:str_replace" | "assets" | "room" => {
            Some(format!("{base}/{rest}"))
        }
        r if r.starts_with("docs/") => Some(format!("{base}/{r}")),
        // One segment: the asset's ID, which the route checks is this
        // artifact's.
        r if r
            .strip_prefix("assets/")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())) =>
        {
            Some(format!("{base}/{r}"))
        }
        _ => None,
    }
}

/// 403 `forbidden_origin`: a site API request from another origin.
fn forbidden_origin(site: &SiteRequest) -> ApiError {
    ApiError::forbidden(
        "forbidden_origin",
        format!(
            "this site's API answers its own pages alone: send Origin {}",
            site.origin
        ),
    )
}

/// The origin check (see the module docs).
fn origin_ok(req: &Request, site: &SiteRequest) -> bool {
    let write = !matches!(*req.method(), Method::GET | Method::HEAD);
    match req.headers().get(header::ORIGIN).map(|o| o.to_str()) {
        Some(Ok(o)) => o == site.origin,
        Some(Err(_)) => false,
        None => !write && !clax_http::viewer::fetched_from_elsewhere(req.headers()),
    }
}

async fn dispatch(s: AppState, api: axum::Router, req: Request) -> Response {
    match answer(s, api, req).await {
        Ok(r) | Err(r) => r,
    }
}

async fn answer(s: AppState, api: axum::Router, mut req: Request) -> Result<Response, Response> {
    let refuse = |e: ApiError| e.into_response();
    let (Some(site), Some(caller)) = (
        req.extensions().get::<SiteRequest>().cloned(),
        req.extensions().get::<SiteCaller>().cloned(),
    ) else {
        return Err(refuse(ApiError::not_found()));
    };
    if !origin_ok(&req, &site) {
        return Err(refuse(forbidden_origin(&site)));
    }
    if req.headers().contains_key(header::AUTHORIZATION) {
        return Err(refuse(ApiError::unauthorized()));
    }
    // Nested under `/_clax/api`, the path is what follows it.
    let rest = req
        .uri()
        .path()
        .strip_prefix('/')
        .unwrap_or_default()
        .to_string();
    let get = matches!(*req.method(), Method::GET | Method::HEAD);
    if rest == "whoami" {
        if !get {
            return Err(refuse(method_not_allowed()));
        }
        return whoami(&s, &site.artifact, &caller).await.map_err(refuse);
    }
    let Some(path) = inner_path(&site.artifact, &rest) else {
        return Err(refuse(ApiError::not_found()));
    };
    let visitor = matches!(caller, SiteCaller::Visitor(_));
    if visitor && rest == "artifact" {
        if !get {
            return Err(refuse(method_not_allowed()));
        }
        return visitor_artifact(&s, &site.artifact).await.map_err(refuse);
    }
    // Who signed in as no one (a visitor, a link viewer, a guest) is
    // counted: their writes are anyone's.
    let anyone = matches!(
        caller,
        SiteCaller::Visitor(_) | SiteCaller::Holder(Holder::Link | Holder::Guest(_))
    );
    if anyone
        && !get
        && let Some(h) = &s.hosted
    {
        h.site_write(req.headers(), req.extensions(), site.artifact.as_str())
            .map_err(refuse)?;
    }
    let hosted = holder_request(&s, &caller).await.map_err(refuse)?;
    let headers = req.headers_mut();
    let gone: Vec<_> = headers
        .keys()
        .filter(|n| dropped(n.as_str()))
        .cloned()
        .collect();
    for name in gone {
        headers.remove(name);
    }
    headers.insert(
        clax_http::headers::VIA_HEADER,
        HeaderValue::from_static("page"),
    );
    // The owner's site acts as the owner's shell does: with the token.
    if matches!(caller, SiteCaller::Holder(Holder::Owner)) {
        let bearer = HeaderValue::from_str(&format!("Bearer {}", s.token))
            .map_err(|_| refuse(ApiError::not_found()))?;
        headers.insert(header::AUTHORIZATION, bearer);
    }
    let ext = req.extensions_mut();
    ext.remove::<clax_http::extension::ViaExtension>();
    ext.remove::<agent::VerifiedGrant>();
    ext.insert(hosted);
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    *req.uri_mut() = format!("{path}{query}")
        .parse()
        .map_err(|_| refuse(ApiError::not_found()))?;
    use tower::ServiceExt as _;
    let Ok(mut res) = api.oneshot(req).await;
    res.headers_mut()
        .remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    Ok(res)
}

fn method_not_allowed() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "this route takes GET",
    )
}

/// The identity a site caller's request carries to the public host's
/// routes: the holder's browser session as the hosted gate would find it
/// (with a person's rights), its `Origin` checked; a link viewer and a
/// visitor have none.
async fn holder_request(s: &AppState, caller: &SiteCaller) -> Result<HostedRequest, ApiError> {
    let caller = caller.clone();
    s.store_call(move |st| {
        let (session, member) = match &caller {
            SiteCaller::Holder(Holder::Owner) => (
                Some(SessionViewer {
                    viewer: st.owner_viewer(true)?,
                    owner: true,
                    guest: false,
                }),
                None,
            ),
            SiteCaller::Holder(Holder::Person(pid)) => match st.viewer_by_public_id(pid)? {
                Some(viewer) => (
                    Some(SessionViewer {
                        viewer,
                        owner: false,
                        guest: false,
                    }),
                    Some(MemberRights {
                        person: pid.clone(),
                        rights: std::sync::Arc::new(st.person_rights(pid)?),
                    }),
                ),
                None => (None, None),
            },
            SiteCaller::Holder(Holder::Guest(pid)) => (
                st.viewer_by_public_id(pid)?.map(|viewer| SessionViewer {
                    viewer,
                    owner: false,
                    guest: true,
                }),
                None,
            ),
            SiteCaller::Holder(Holder::Link) | SiteCaller::Visitor(_) => (None, None),
        };
        Ok(HostedRequest {
            session,
            origin_checked: true,
            member,
            cookie_sha256: None,
        })
    })
    .await
}

/// `whoami`: who the site's caller is, computed by the server.
///
/// `{kind, id?, name?, level, can_edit}`: `kind` is `owner`, `person`,
/// `guest`, `link` (a link viewer) or `visitor` (a site open to the world);
/// `id` the viewer's public ID, or a visitor's visitor ID (a link viewer has
/// none); `name` the viewer's display name, when they have one; `level` the
/// caller's `db` level (`public` for a guest, a link viewer and a visitor);
/// `can_edit` whether they may publish the artifact's versions (the owner,
/// and a person whose rights `update` it).
async fn whoami(s: &AppState, aid: &ArtifactId, caller: &SiteCaller) -> Result<Response, ApiError> {
    let (caller, aid) = (caller.clone(), aid.clone());
    let v = s
        .store_call(move |st| {
            let level = clax_http::db_caller::site_caller(st, &caller)?.level;
            let viewer = |pid: &str| -> clax_core::Result<Option<String>> {
                Ok(st.viewer_by_public_id(pid)?.and_then(|v| v.display_name))
            };
            let (kind, id, name, can_edit) = match &caller {
                SiteCaller::Holder(Holder::Owner) => {
                    let v = st.owner_viewer(true)?;
                    ("owner", Some(v.public_id), v.display_name, true)
                }
                SiteCaller::Holder(Holder::Person(pid)) => {
                    let m = MemberRights {
                        person: pid.clone(),
                        rights: std::sync::Arc::new(st.person_rights(pid)?),
                    };
                    let edits = m.allows_at(st, (RightType::Artifacts, "update"), aid.as_str())?;
                    ("person", Some(pid.clone()), viewer(pid)?, edits)
                }
                SiteCaller::Holder(Holder::Guest(pid)) => {
                    ("guest", Some(pid.clone()), viewer(pid)?, false)
                }
                SiteCaller::Holder(Holder::Link) => ("link", None, None, false),
                SiteCaller::Visitor(vid) => ("visitor", Some(vid.clone()), None, false),
            };
            let mut v = json!({"kind": kind, "level": level.as_str(), "can_edit": can_edit});
            if let Some(id) = id {
                v["id"] = json!(id);
            }
            if let Some(name) = name {
                v["name"] = json!(name);
            }
            Ok(v)
        })
        .await?;
    Ok(no_store(Json(v).into_response()))
}

/// `artifact` for a visitor: `{artifact: {id, title, current_version,
/// capabilities}}`, what a page reads of itself (its declaration, from which
/// the bridge decides what `use()` resolves), and nothing of who works on it.
async fn visitor_artifact(s: &AppState, aid: &ArtifactId) -> Result<Response, ApiError> {
    let aid = aid.clone();
    let v: Value = s
        .store_call(move |st| {
            let a = st
                .get_artifact(&aid)?
                .ok_or(clax_core::CoreError::NotFound)?;
            Ok(json!({"artifact": {
                "id": a.id.as_str(),
                "title": a.title,
                "current_version": a.current_version,
                "capabilities": a.capabilities,
            }}))
        })
        .await?;
    Ok(no_store(Json(v).into_response()))
}

fn no_store(mut res: Response<Body>) -> Response<Body> {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_is_the_hosts_artifacts() {
        let a = ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        for (rest, want) in [
            ("artifact", Some("/api/artifacts/7q3k9mzx2b4t")),
            ("docs", Some("/api/artifacts/7q3k9mzx2b4t/docs")),
            ("docs/a/b", Some("/api/artifacts/7q3k9mzx2b4t/docs/a/b")),
            ("docs:batch", Some("/api/artifacts/7q3k9mzx2b4t/docs:batch")),
            ("assets", Some("/api/artifacts/7q3k9mzx2b4t/assets")),
            (
                "assets/01J0000000000000000000000A",
                Some("/api/artifacts/7q3k9mzx2b4t/assets/01J0000000000000000000000A"),
            ),
            ("assets/", None),
            ("assets/a/b", None),
            ("assets/..", None),
            ("room", Some("/api/artifacts/7q3k9mzx2b4t/room")),
            ("threads", None),
            ("artifact/8q3k9mzx2b4t", None),
            ("../artifacts/8q3k9mzx2b4t", None),
            ("", None),
        ] {
            assert_eq!(inner_path(&a, rest).as_deref(), want, "{rest}");
        }
    }
}

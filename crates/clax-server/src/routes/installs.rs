//! A queue's installs (`clax_core::install`): the sites that embed the
//! overlay and file into the queue, edited by the queue's admins in Clax,
//! and the read-only config each site's overlay fetches.
//!
//! - `GET /api/queues/<qid>/installs` → `{installs}`: a member's read.
//! - `POST /api/queues/<qid>/installs {name, origins, config?}` → 201
//!   `{install}`; `PATCH /api/queues/<qid>/installs/<iid> {name?,
//!   origins?, config?}` → `{install}`; `DELETE` the same → 204: an
//!   admin's, from the daemon's own origin, as the queue's routes are
//!   (`routes/queues.rs`).
//! - `GET /api/installs/<iid>/config` → the install's public config, for
//!   anyone: no credential, mounted with the reporter routes in front of
//!   the credential layers. It carries an `ETag` (a quoted SHA-256 of the
//!   answer) and answers `If-None-Match` with 304; it names
//!   `Access-Control-Allow-Origin` for the install's own origins alone,
//!   so another site's page cannot read it. 404 `not_found` for no such
//!   live install, or one whose queue is deleted.

use super::artifacts::{body, path};
use super::queues::{admin_person, require_admin};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_core::install::{Install, InstallConfig};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::queues::{InstallPatch, NewInstall, QueueRow};
use serde::Deserialize;
use serde_json::{Value, json};

/// `GET /api/queues/<qid>/installs` → `{installs: [{id, queue_id, name,
/// origins, config, created_at, updated_at}]}`, oldest first: any member's
/// read. 404 `not_a_member` for a non-member or no such queue.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let installs = s
        .store_call(move |st| {
            if st.queue_role(&qid, person.as_deref())?.is_none() {
                return Err(CoreError::NotAMember);
            }
            st.installs_of_queue(&qid)
        })
        .await?;
    Ok(Json(json!({ "installs": installs })))
}

/// `POST /api/queues/<qid>/installs` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    name: String,
    #[serde(default)]
    origins: Vec<String>,
    #[serde(default)]
    config: InstallConfig,
}

/// `POST /api/queues/<qid>/installs {name, origins?, config?}` → 201
/// `{install}`: an admin's. 400 `invalid_install` (a blank or long name,
/// bad wording or colour, or a queue at its most installs) or
/// `invalid_origin`.
pub async fn create(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<CreateBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    let install = s
        .store_call(move |st| {
            if let Err(e) = require_admin(st, &qid, person.as_deref()) {
                return Ok(Err(e));
            }
            let i = st.create_install(
                &ctx,
                &qid,
                NewInstall {
                    name: b.name,
                    origins: b.origins,
                    config: b.config,
                },
            )?;
            Ok(Ok(i))
        })
        .await??;
    Ok((StatusCode::CREATED, Json(json!({ "install": install }))))
}

/// `PATCH /api/queues/<qid>/installs/<iid>` body: each field given is set,
/// `config` whole.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    name: Option<String>,
    origins: Option<Vec<String>>,
    config: Option<InstallConfig>,
}

/// `PATCH /api/queues/<qid>/installs/<iid> {name?, origins?, config?}` →
/// `{install}`: an admin's; the errors of the `POST`, and 404 `not_found`
/// for no such install of the queue.
pub async fn patch(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let (qid, iid) = path(p)?;
    let b = body(req)?;
    let install = s
        .store_call(move |st| {
            if let Err(e) = require_admin(st, &qid, person.as_deref()) {
                return Ok(Err(e));
            }
            let i = st.update_install(
                &ctx,
                &qid,
                &iid,
                InstallPatch {
                    name: b.name,
                    origins: b.origins,
                    config: b.config,
                },
            )?;
            Ok(Ok(i))
        })
        .await??;
    Ok(Json(json!({ "install": install })))
}

/// `DELETE /api/queues/<qid>/installs/<iid>` → 204: an admin's; its config
/// answers 404 from then on. 404 `not_found` for no such install.
pub async fn delete(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let person = admin_person(&s, &d).await?;
    let (qid, iid) = path(p)?;
    s.store_call(move |st| {
        if let Err(e) = require_admin(st, &qid, person.as_deref()) {
            return Ok(Err(e));
        }
        st.delete_install(&ctx, &qid, &iid)?;
        Ok(Ok(()))
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

/// The public config's route, which `queue_reporter::router` mounts in
/// front of the credential layers.
pub fn public_router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/api/installs/{iid}/config",
        axum::routing::get(config).options(config_preflight),
    )
}

/// The config an install's overlay reads: its queue's name and pages (the
/// queue page for its members, the reporter page for reporters, both
/// paths on this Clax) and the install's settings.
pub fn public_config(i: &Install, q: &QueueRow) -> Value {
    json!({
        "install_id": i.id,
        "name": i.name,
        "queue": {
            "id": q.id,
            "name": q.name,
            "reporters_see": q.reporters_see,
            "page": format!("/queues/{}", q.id),
            "reporter_page": format!("/reporter/{}", q.id),
        },
        "config": i.config,
    })
}

/// The one `Origin` a request sent, when it sent exactly one.
fn origin_of(h: &HeaderMap) -> Option<String> {
    let mut sent = h.get_all(header::ORIGIN).iter();
    match (sent.next(), sent.next()) {
        (Some(o), None) => o.to_str().ok().map(str::to_string),
        _ => None,
    }
}

/// Names `origin` as allowed on `res` when the install lists it.
fn allow(res: &mut Response, origins: &[String], origin: Option<&str>, preflight: bool) {
    let h = res.headers_mut();
    h.append(header::VARY, HeaderValue::from_static("Origin"));
    let Some(o) = origin.filter(|o| origins.iter().any(|l| l == o)) else {
        return;
    };
    let Ok(v) = HeaderValue::from_str(o) else {
        return;
    };
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("ETag"),
    );
    if preflight {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("If-None-Match"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
    }
}

/// Install `iid` with its queue, or 404.
async fn live(s: &AppState, iid: String) -> Result<(Install, QueueRow), ApiError> {
    s.store_call(move |st| st.install_with_queue(&iid))
        .await?
        .ok_or_else(|| CoreError::NotFound.into())
}

/// `GET /api/installs/<iid>/config`: see the module's documentation.
pub async fn config(
    State(s): State<AppState>,
    headers: HeaderMap,
    iid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let iid = path(iid)?;
    let origin = origin_of(&headers);
    let (i, q) = match live(&s, iid).await {
        Ok(found) => found,
        Err(e) => {
            let mut res = e.into_response();
            res.headers_mut()
                .append(header::VARY, HeaderValue::from_static("Origin"));
            return Ok(res);
        }
    };
    let text = public_config(&i, &q).to_string();
    let etag = format!("\"{}\"", clax_core::audit::sha256_hex(text.as_bytes()));
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));
    let mut res = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(header::CONTENT_TYPE, "application/json")],
            text,
        )
            .into_response()
    };
    let h = res.headers_mut();
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("hex is a header value"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    allow(&mut res, &i.origins, origin.as_deref(), false);
    Ok(res)
}

/// `OPTIONS /api/installs/<iid>/config` → 204: the preflight a fetch with
/// `If-None-Match` sends, allowed for the install's own origins.
pub async fn config_preflight(
    State(s): State<AppState>,
    headers: HeaderMap,
    iid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let iid = path(iid)?;
    let origin = origin_of(&headers);
    let origins = live(&s, iid)
        .await
        .map(|(i, _)| i.origins)
        .unwrap_or_default();
    let mut res = StatusCode::NO_CONTENT.into_response();
    allow(&mut res, &origins, origin.as_deref(), true);
    Ok(res)
}

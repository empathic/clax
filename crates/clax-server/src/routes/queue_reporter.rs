//! A queue's routes for a deployer's app (spec 2026-10-27-clax-sdk-design
//! §5.3, §7.5), on their own router ([`router`]), which the daemon mounts
//! in front of its own credential layers: they carry their own
//! credentials ([`clax_http::reporter`]) and answer cross-origin requests.
//!
//! - The app's backend (`Authorization: Clax-Queue <token>` and a
//!   `Clax-Signature` by its service grant's key, no `Origin`): `POST
//!   /api/queues/<qid>/reporter-tokens {reporter_id, origin, public_key}`
//!   → `{token, expires_at}`.
//! - A reporter in the app's page (`Authorization: Clax-Reporter <token>`,
//!   a `Clax-Signature` by the reporter's key, and the token's `Origin`):
//!   `POST /api/queues/<qid>/reporter/reports` (multipart) → `201
//!   {report}`; `GET /api/queues/<qid>/reporter/reports?cursor=` →
//!   `{reports, next}`; `POST …/reporter/reports/<tid>/comments {body}` →
//!   `{report}`; `GET …/reporter/reports/<tid>/clip` → the PNG; `OPTIONS`
//!   on each, the CORS preflight ([`clax_http::reporter::cors`]).
//!
//! The reporter is always the token's, never the body's. A reporter reads
//! the reports its queue's `reporters_see` shows it; any other is 404
//! `not_found`, as one that does not exist.

use super::assets::multipart_error;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clax_core::audit::{Actor, AuditCtx, Via};
use clax_core::gitctx::GitField;
use clax_core::{Anchor, CoreError};
use clax_http::error::ApiError;
use clax_http::reporter::{self, REPORT_BODY_LIMIT, ReporterCaller, ServiceCaller};
use clax_http::state::AppState;
use clax_store::queues::{NewReport, Visibility};
use clax_store::threads::clip_problem;
use serde::Deserialize;
use serde_json::{Value, json};

/// The service and reporter routes, with `state`.
pub fn router(state: AppState) -> Router {
    let reporter = Router::new()
        .route(
            "/api/queues/{qid}/reporter/reports",
            get(my_reports)
                .post(file_report)
                // The extractor bounds the body; `Multipart` reads it again.
                .layer(DefaultBodyLimit::disable())
                .options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/comments",
            post(comment).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/clip",
            get(clip).options(preflight),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            reporter::cors,
        ));
    Router::new()
        .route("/api/queues/{qid}/reporter-tokens", post(mint_token))
        .merge(reporter)
        .with_state(state)
}

/// Who a reporter's change is recorded as: no person of this instance.
fn reporter_ctx() -> AuditCtx {
    AuditCtx {
        actor: Actor::Anonymous,
        via: Via::Lan,
        git: GitField::Absent,
        call: None,
    }
}

fn json_body<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|e| ApiError::bad_request("invalid_args", e.to_string()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MintBody {
    reporter_id: String,
    origin: String,
    public_key: String,
}

/// `POST /api/queues/<qid>/reporter-tokens {reporter_id, origin,
/// public_key}`: a reporter token of the caller's service grant for
/// `reporter_id`, bound to `origin` (one of the queue's) and the Ed25519
/// `public_key` (unpadded base64url). Answers `{token, expires_at}`.
///
/// # Errors
/// [`ServiceCaller`]'s; 400 `invalid_args`, `invalid_origin`,
/// `invalid_reporter_id` or `invalid_key`; 403 `wrong_origin` for an
/// origin the queue does not list.
pub async fn mint_token(
    State(s): State<AppState>,
    c: ServiceCaller,
) -> Result<Json<Value>, ApiError> {
    let b: MintBody = json_body(&c.body)?;
    let grant = c.grant;
    let (token, expires_at) = s
        .store_call(move |st| {
            st.mint_reporter_token(&grant, &b.reporter_id, &b.origin, &b.public_key)
        })
        .await?;
    Ok(Json(
        json!({"token": token.expose(), "expires_at": expires_at}),
    ))
}

/// A report's multipart fields, each at most once.
#[derive(Default)]
struct Fields {
    external_id: Option<String>,
    url: Option<String>,
    title: Option<String>,
    anchor: Option<String>,
    body: Option<String>,
    metadata: Option<String>,
    revision: Option<String>,
    clip: Option<Vec<u8>>,
}

/// Reads a report's fields; an unknown field, or one given twice, is 400
/// `invalid_args`.
async fn read_fields(mut mp: Multipart) -> Result<Fields, ApiError> {
    let mut f = Fields::default();
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        let name = field.name().unwrap_or("").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e.status(), e.body_text()))?;
        let text = || {
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                ApiError::bad_request("invalid_args", format!("field '{name}' is not UTF-8"))
            })
        };
        let twice = match name.as_str() {
            "external_id" => f.external_id.replace(text()?).is_some(),
            "url" => f.url.replace(text()?).is_some(),
            "title" => f.title.replace(text()?).is_some(),
            "anchor" => f.anchor.replace(text()?).is_some(),
            "body" => f.body.replace(text()?).is_some(),
            "metadata" => {
                if bytes.len() > clax_core::queue::MAX_THREAD_METADATA_BYTES {
                    return Err(ApiError::bad_request(
                        "metadata_too_large",
                        format!(
                            "a report's metadata is at most {} bytes",
                            clax_core::queue::MAX_THREAD_METADATA_BYTES
                        ),
                    ));
                }
                f.metadata.replace(text()?).is_some()
            }
            "revision" => f.revision.replace(text()?).is_some(),
            "clip" => f.clip.replace(bytes.to_vec()).is_some(),
            _ => {
                return Err(ApiError::bad_request(
                    "invalid_args",
                    format!(
                        "unknown field '{name}': a report takes external_id, url, title, anchor, body, metadata, revision and clip"
                    ),
                ));
            }
        };
        if twice {
            return Err(ApiError::bad_request(
                "invalid_args",
                format!("field '{name}' is given twice"),
            ));
        }
    }
    Ok(f)
}

fn required(v: Option<String>, name: &str) -> Result<String, ApiError> {
    v.ok_or_else(|| ApiError::bad_request("invalid_args", format!("field '{name}' is required")))
}

/// The report `f` describes.
fn new_report(f: Fields, reporter_id: &str) -> Result<NewReport, ApiError> {
    let anchor: Anchor = serde_json::from_str(&required(f.anchor, "anchor")?)
        .map_err(|e| ApiError::bad_request("invalid_anchor", e.to_string()))?;
    let metadata = match f.metadata {
        None => Value::Null,
        Some(m) => serde_json::from_str(&m)
            .map_err(|e| ApiError::bad_request("invalid_metadata", e.to_string()))?,
    };
    if let Some(problem) = f.clip.as_deref().and_then(clip_problem) {
        return Err(ApiError::bad_request("invalid_clip", problem));
    }
    Ok(NewReport {
        external_id: f.external_id,
        url: required(f.url, "url")?,
        title: required(f.title, "title")?,
        anchor,
        body: required(f.body, "body")?,
        author_name: reporter_id.to_string(),
        metadata,
        revision: f.revision,
        clip: f.clip,
    })
}

/// Broadcasts report `tid`'s `thread` event, as a thread's routes do; a
/// failure is logged, the change being made.
async fn announce(s: &AppState, tid: String) {
    let ctx = s.feedback_ctx();
    let r = s
        .store_call(move |st| {
            if let Some(t) = st.get_thread(&tid)? {
                super::threads::publish_thread(&ctx, st, &t)?;
            }
            Ok(())
        })
        .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, "announcing a report failed");
    }
}

/// `POST /api/queues/<qid>/reporter/reports` (multipart `external_id?`,
/// `url`, `title`, `anchor` (JSON), `body`, `metadata?` (JSON, at most
/// [`clax_core::queue::MAX_THREAD_METADATA_BYTES`] bytes), `revision?`,
/// `clip?` (PNG)): files a report as the token's reporter. Answers `201
/// {report}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 400 `invalid_args`, `invalid_multipart`,
/// `invalid_anchor`, `invalid_metadata`, `metadata_too_large`,
/// `invalid_clip` and the store's refusals of bad input.
pub async fn file_report(
    State(s): State<AppState>,
    Path(qid): Path<String>,
    r: ReporterCaller<REPORT_BODY_LIMIT>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mp = Multipart::from_request(r.request(), &())
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let rid = r.token.reporter_id.clone();
    let new = new_report(read_fields(mp).await?, &rid)?;
    let report = s
        .store_call_async(move |st| async move {
            st.file_report_async(&reporter_ctx(), &qid, Some(&rid), new)
                .await
        })
        .await?;
    announce(&s, report.id.clone()).await;
    Ok((StatusCode::CREATED, Json(json!({ "report": report }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    cursor: Option<String>,
}

/// `GET /api/queues/<qid>/reporter/reports?cursor=`: a page of the reports
/// the token's reporter sees, in order of arrival. Answers `{reports,
/// next}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 400 `invalid_args` for another query.
pub async fn my_reports(
    State(s): State<AppState>,
    Path(qid): Path<String>,
    q: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
    r: ReporterCaller,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))?;
    let rid = r.token.reporter_id;
    let (reports, next) = s
        .store_call(move |st| {
            st.reports(&qid, Visibility::Reporter(&rid), None, q.cursor.as_deref())
        })
        .await?;
    Ok(Json(json!({ "reports": reports, "next": next })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentBody {
    body: String,
}

/// `POST /api/queues/<qid>/reporter/reports/<tid>/comments {body}`: the
/// token's reporter comments on a report it sees (a resolved one reopens).
/// Answers `{report}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for a report the reporter does
/// not see; 400 `invalid_args` or `invalid_comment`.
pub async fn comment(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller,
) -> Result<Json<Value>, ApiError> {
    let b: CommentBody = json_body(&r.body)?;
    let rid = r.token.reporter_id;
    let t = tid.clone();
    let report = s
        .store_call(move |st| st.reporter_comment(&reporter_ctx(), &qid, &t, &rid, &b.body))
        .await?;
    announce(&s, tid).await;
    Ok(Json(json!({ "report": report })))
}

/// `GET /api/queues/<qid>/reporter/reports/<tid>/clip`: the clip of a
/// report the token's reporter sees, as `image/png`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for no clip, or a report the
/// reporter does not see.
pub async fn clip(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller,
) -> Result<Response, ApiError> {
    let rid = r.token.reporter_id;
    let bytes = s
        .store_call_async(move |st| async move {
            st.report_clip_async(&qid, &tid, Visibility::Reporter(&rid))
                .await?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// `OPTIONS` on each reporter route: the CORS preflight, `204`, whose
/// headers [`reporter::cors`] adds for the queue's origins.
pub async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    /// These routes run before every credential layer of the daemon, so
    /// each handler but the preflight takes the credential extractor that
    /// authenticates it, and every reporter route has its CORS layer.
    #[test]
    fn every_handler_takes_its_credential() {
        let src = include_str!("queue_reporter.rs");
        let src = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let mut seen = 0;
        for part in src.split("\npub async fn ").skip(1) {
            let name = part.split('(').next().unwrap();
            let sig = &part[..part.find('{').unwrap()];
            seen += 1;
            match name {
                "preflight" => assert!(sig.contains("()"), "{name}"),
                "mint_token" => assert!(sig.contains("c: ServiceCaller"), "{name}"),
                _ => assert!(sig.contains("r: ReporterCaller"), "{name}"),
            }
        }
        assert_eq!(seen, 6);
        let routes = &src[src.find("pub fn router").unwrap()..];
        let routes = &routes[..routes.find("\n}\n").unwrap()];
        assert_eq!(routes.matches(".route(").count(), 4);
        assert_eq!(routes.matches(".route_layer(").count(), 1);
        assert!(routes.find("reporter::cors").unwrap() < routes.find("reporter-tokens").unwrap());
    }
}

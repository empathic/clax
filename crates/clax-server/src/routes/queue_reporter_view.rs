//! A reporter's view of its queue in Clax: the pages a reporter lands on
//! from its app, and the routes they read, under `/reporter/<qid>`.
//!
//! - `GET /reporter/<qid>/enter?code=` trades a view link's single-use
//!   code (`POST /api/queues/<qid>/reporter/view-link`,
//!   [`super::queue_reporter::view_link`]) for a view session, kept by the
//!   browser as the `HttpOnly`, `SameSite=Lax` cookie [`COOKIE`] on
//!   `Path=/reporter/<qid>`, and answers `303` to the list
//!   (`/reporter/<qid>`) or the link's report (`/reporter/<qid>/<tid>`). A
//!   code that is spent, unknown or expired sets no cookie and lands on
//!   the list, which then says to open it from the app again.
//! - `GET /reporter/<qid>` and `/reporter/<qid>/<tid>`: the shell's page,
//!   which shows the reporter's view (`web/shell/src/queues/ReporterRoute.svelte`).
//! - With the cookie: `GET /reporter/<qid>/api/queue` → `{queue: {id,
//!   name, reporters_see}}`; `GET …/api/reports?cursor=` → `{reports,
//!   next}`; `GET …/api/reports/<tid>` → `{report}`; `POST
//!   …/api/reports/<tid>/comments {body}` (JSON) → `{report}`; `GET
//!   …/api/reports/<tid>/clip` → the PNG.
//!
//! These routes run in front of every credential layer of the daemon, as
//! the reporter routes do: each handler but the pages and the trade takes
//! [`ViewCaller`], which is the cookie's live session of the path's
//! queue. They read through the same store calls as the reporter routes
//! (`Store::reporter_reports`, `Store::reporter_report`), so the queue's
//! `reporters_see` and what a reporter sees of another are decided in one
//! place. 401 `unauthorized` without a live session.

use super::queue_reporter::reporter_ctx;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clax_core::CoreError;
use clax_core::queue::VIEW_SESSION_SECS;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::queues::{ViewSession, Visibility};
use serde::Deserialize;
use serde_json::json;

/// The view session's cookie.
pub const COOKIE: &str = "clax_reporter";

/// The routes, for [`super::queue_reporter::router`].
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/reporter/{qid}", get(page))
        .route("/reporter/{qid}/enter", get(enter))
        .route("/reporter/{qid}/{tid}", get(report_page))
        .route("/reporter/{qid}/api/queue", get(queue))
        .route("/reporter/{qid}/api/reports", get(reports))
        .route("/reporter/{qid}/api/reports/{tid}", get(report))
        .route("/reporter/{qid}/api/reports/{tid}/comments", post(comment))
        .route("/reporter/{qid}/api/reports/{tid}/clip", get(clip))
}

/// `s` as one path segment or query value.
fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// The path a view link's `code` of queue `qid` opens.
pub fn enter_path(qid: &str, code: &str) -> String {
    format!("/reporter/{}/enter?code={}", enc(qid), enc(code))
}

/// A request with a live view session of the path's queue.
pub struct ViewCaller(pub ViewSession);

impl FromRequestParts<AppState> for ViewCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        let Path(params) = Path::<Vec<(String, String)>>::from_request_parts(parts, &())
            .await
            .map_err(|_| ApiError::unauthorized())?;
        let qid = params
            .into_iter()
            .find_map(|(k, v)| (k == "qid").then_some(v))
            .ok_or_else(ApiError::unauthorized)?;
        let token = clax_http::hosted::cookie_value(&parts.headers, COOKIE)
            .ok_or_else(ApiError::unauthorized)?;
        let sha = clax_core::secret::sha256_hex(&token);
        let found = s.store_call(move |st| st.view_session(&qid, &sha)).await?;
        found.map(ViewCaller).ok_or_else(ApiError::unauthorized)
    }
}

fn no_store(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}

/// `GET /reporter/<qid>`: the shell's page.
///
/// # Errors
/// 503 `ui_not_built` without a web build.
pub async fn page(State(s): State<AppState>, req: HeaderMap) -> Result<Response, ApiError> {
    Ok(no_store(crate::shell::reporter_page(&s, &req)?))
}

/// `GET /reporter/<qid>/<tid>`: the shell's page.
///
/// # Errors
/// 503 `ui_not_built` without a web build.
pub async fn report_page(State(s): State<AppState>, req: HeaderMap) -> Result<Response, ApiError> {
    Ok(no_store(crate::shell::reporter_page(&s, &req)?))
}

#[derive(Deserialize)]
pub struct EnterQuery {
    code: Option<String>,
}

/// `GET /reporter/<qid>/enter?code=`: trades the code for a view session
/// and its cookie, then `303` to the reporter's page.
///
/// # Errors
/// None: a code that does not trade lands on the list without a cookie.
pub async fn enter(
    State(s): State<AppState>,
    Path(qid): Path<String>,
    Query(q): Query<EnterQuery>,
) -> Response {
    let list = format!("/reporter/{}", enc(&qid));
    let Some(code) = q.code else {
        return no_store(redirect(&list, None));
    };
    let id = qid.clone();
    let traded = s
        .store_call(move |st| st.redeem_view_code(&id, &code))
        .await;
    match traded {
        Ok((token, _, report)) => {
            let to = match report {
                Some(tid) => format!("{list}/{}", enc(&tid)),
                None => list.clone(),
            };
            let secure = if s.hosted.as_ref().is_some_and(|h| h.secure) {
                "; Secure"
            } else {
                ""
            };
            let cookie = format!(
                "{COOKIE}={}; Path={list}; Max-Age={VIEW_SESSION_SECS}; HttpOnly{secure}; SameSite=Lax",
                token.expose()
            );
            no_store(redirect(&to, HeaderValue::from_str(&cookie).ok()))
        }
        Err(e) => {
            tracing::debug!(error = ?e, "a reporter's view code did not trade");
            no_store(redirect(&list, None))
        }
    }
}

fn redirect(to: &str, cookie: Option<HeaderValue>) -> Response {
    let mut r = (StatusCode::SEE_OTHER, [(header::LOCATION, to.to_string())]).into_response();
    if let Some(c) = cookie {
        r.headers_mut().insert(header::SET_COOKIE, c);
    }
    r
}

/// `GET /reporter/<qid>/api/queue`: the queue as its reporter sees it,
/// `{queue: {id, name, reporters_see}}`.
///
/// # Errors
/// 401 without a live session.
pub async fn queue(State(s): State<AppState>, c: ViewCaller) -> Result<Response, ApiError> {
    let qid = c.0.queue_id;
    let q = s
        .store_call(move |st| st.queue(&qid)?.ok_or(CoreError::NotFound))
        .await?;
    Ok(no_store(
        Json(json!({"queue": {"id": q.id, "name": q.name, "reporters_see": q.reporters_see}}))
            .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    cursor: Option<String>,
}

/// `GET /reporter/<qid>/api/reports?cursor=`: a page of the reports the
/// session's reporter sees, `{reports, next}`.
///
/// # Errors
/// 401 without a live session; 400 `invalid_args` for another query.
pub async fn reports(
    State(s): State<AppState>,
    q: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
    c: ViewCaller,
) -> Result<Response, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))?;
    let ViewSession {
        queue_id,
        reporter_id,
        ..
    } = c.0;
    let (reports, next) = s
        .store_call(move |st| st.reporter_reports(&queue_id, &reporter_id, q.cursor.as_deref()))
        .await?;
    Ok(no_store(
        Json(json!({ "reports": reports, "next": next })).into_response(),
    ))
}

/// `GET /reporter/<qid>/api/reports/<tid>`: a report the session's
/// reporter sees, `{report}`.
///
/// # Errors
/// 401 without a live session; 404 `not_found` for a report it does not
/// see.
pub async fn report(
    State(s): State<AppState>,
    Path((_, tid)): Path<(String, String)>,
    c: ViewCaller,
) -> Result<Response, ApiError> {
    let ViewSession {
        queue_id,
        reporter_id,
        ..
    } = c.0;
    let r = s
        .store_call(move |st| {
            st.reporter_report(&queue_id, &tid, &reporter_id)?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(no_store(Json(json!({ "report": r })).into_response()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentBody {
    body: String,
}

/// `POST /reporter/<qid>/api/reports/<tid>/comments {body}`: the
/// session's reporter comments on a report it sees, as its reporter
/// token would. The body is JSON (`Content-Type: application/json`), which
/// no cross-site form can send. Answers `{report}`.
///
/// # Errors
/// 401 without a live session; 404 `not_found` for a report it does not
/// see; 415 `unsupported_media_type` for another body; 400 `invalid_args`
/// or `invalid_comment`.
pub async fn comment(
    State(s): State<AppState>,
    Path((_, tid)): Path<(String, String)>,
    c: ViewCaller,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let json_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    if !json_type {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "a comment is a JSON body",
        ));
    }
    let b: CommentBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request("invalid_args", e.to_string()))?;
    let ViewSession {
        queue_id,
        reporter_id,
        ..
    } = c.0;
    let t = tid.clone();
    let r = s
        .store_call(move |st| {
            let r = st.reporter_comment(
                &reporter_ctx(&queue_id, &reporter_id),
                &queue_id,
                &t,
                &reporter_id,
                &b.body,
            )?;
            st.as_reporter(&queue_id, &reporter_id, r)
        })
        .await?;
    super::queue_reporter::announce(&s, tid).await;
    Ok(no_store(Json(json!({ "report": r })).into_response()))
}

/// `GET /reporter/<qid>/api/reports/<tid>/clip`: the clip of a report the
/// session's reporter sees, as `image/png`.
///
/// # Errors
/// 401 without a live session; 404 `not_found` for no clip, or a report it
/// does not see.
pub async fn clip(
    State(s): State<AppState>,
    Path((_, tid)): Path<(String, String)>,
    c: ViewCaller,
) -> Result<Response, ApiError> {
    let ViewSession {
        queue_id,
        reporter_id,
        ..
    } = c.0;
    let bytes = s
        .store_call_async(move |st| async move {
            st.report_clip_async(&queue_id, &tid, Visibility::Reporter(&reporter_id))
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

#[cfg(test)]
mod tests {
    /// These routes run before every credential layer of the daemon, so
    /// each handler but the pages and the code's trade takes the view
    /// session's extractor.
    #[test]
    fn every_handler_takes_its_credential() {
        let src = include_str!("queue_reporter_view.rs");
        let src = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let mut seen = 0;
        for part in src.split("\npub async fn ").skip(1) {
            let name = part.split('(').next().unwrap();
            let sig = &part[..part.find('{').unwrap()];
            seen += 1;
            match name {
                "page" | "report_page" | "enter" => assert!(!sig.contains("ViewCaller"), "{name}"),
                _ => assert!(sig.contains("c: ViewCaller"), "{name}"),
            }
        }
        assert_eq!(seen, 8);
    }
}

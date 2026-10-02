//! The `sample` capability's routes (spec §6 "Sample"; `docs/contract.md`
//! "Sample protocol"). All three refuse a foreign `Origin`: pages reach them
//! only through the shell, which asks the viewer's consent first. Only the
//! owner's browser spends the key: the call and tool-result routes need the
//! bearer token (401 `unauthorized`) and a viewer cookie (403 `forbidden`:
//! an agent or a script holding the token is not a browser), and the status
//! route answers `available: false` to a caller without the token. The call
//! route streams `start`, `text`, `tool_call`, and one `done` or `error` over
//! SSE; closing the stream cancels the call.

use crate::auth::{RequireToken, has_token};
use crate::error::ApiError;
use crate::routes::artifacts::parse_id;
use crate::sample::flight::{self, Flight, Out};
use crate::sample::request::{self, MAX_TOOL_RESULT_BYTES, SampleBody};
use crate::sample::{Deliver, ToolOutput};
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;

/// Body limit of the call route: five downsized images, base64, with room to spare.
pub const SAMPLE_BODY_LIMIT: usize = 48 * 1024 * 1024;

async fn declared(s: &AppState, aid: &str) -> Result<String, ApiError> {
    let id = parse_id(aid)?;
    let canonical = id.as_str().to_string();
    let artifact = s
        .store_call(move |st| st.get_artifact(&id))
        .await?
        .ok_or_else(ApiError::not_found)?;
    if artifact.capabilities.get("sample").is_none() {
        return Err(ApiError::forbidden(
            "not_declared",
            "this artifact does not declare sample",
        ));
    }
    Ok(canonical)
}

/// The viewer cookie of the owner's browser making a call; 403 `forbidden`
/// without one.
fn owner_browser(cookie: Option<String>) -> Result<String, ApiError> {
    cookie.ok_or_else(|| {
        ApiError::forbidden(
            "forbidden",
            "sample() is spent from the owner's browser only",
        )
    })
}

/// `GET /api/artifacts/{aid}/sample`: whether this daemon can sample for the
/// caller, and the view's limits. Without the token: never available.
pub async fn status(
    State(s): State<AppState>,
    Path(aid): Path<String>,
    _o: SameOrigin,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    s.store_call(move |st| st.get_artifact(&id))
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !has_token(&headers, &s.token) {
        return Ok(Json(
            json!({"available": false, "provider": null, "limits": request::limits_json(false), "calls_today": 0, "daily_call_cap": null}),
        ));
    }
    let images = s.sample.provider().is_some_and(|p| p.supports_images());
    Ok(Json(json!({
        "available": s.sample.available(),
        "provider": s.sample.provider_name(),
        "limits": request::limits_json(images),
        "calls_today": 0,
        "daily_call_cap": s.sample.settings.daily_call_cap,
    })))
}

/// The SSE response: `start`, then the flight's frames.
pub(crate) fn stream_response(
    s: &AppState,
    start: Value,
    frames: impl Stream<Item = Out> + Send + 'static,
) -> Response {
    let first = futures::stream::once(async move {
        Ok::<_, Infallible>(SseEvent::default().event("start").data(start.to_string()))
    });
    let rest = frames.map(|o| {
        let (name, data) = o.sse();
        Ok::<_, Infallible>(SseEvent::default().event(name).data(data.to_string()))
    });
    Sse::new(first.chain(rest))
        .keep_alive(
            KeepAlive::new()
                .interval(s.sse_keep_alive)
                .text("keep-alive"),
        )
        .into_response()
}

/// `POST /api/artifacts/{aid}/sample`.
pub async fn sample(
    State(s): State<AppState>,
    Path(aid): Path<String>,
    _o: SameOrigin,
    _t: RequireToken,
    ViewerCookie(cookie): ViewerCookie,
    body: Result<Json<SampleBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    // `Some` from here on: the per-viewer APIs below take the cookie as an option.
    let cookie = Some(owner_browser(cookie)?);
    let _aid = declared(&s, &aid).await?;
    let Some(provider) = s.sample.provider().cloned() else {
        return Err(ApiError::forbidden(
            "sampling_disabled",
            "no sample provider is configured on this machine",
        ));
    };
    let Json(body) = body.map_err(|e| ApiError::bad_request("invalid_request", e.body_text()))?;
    let prepared = request::prepare(body, &s.sample.settings, provider.supports_images())?;
    let call_id = clax_core::new_ulid();
    let f = Flight::new();
    s.sample.open_call(&call_id, cookie);
    let task = tokio::spawn(flight::drive(
        s.sample.clone(),
        call_id.clone(),
        prepared,
        f.clone(),
        |_| {},
    ));
    f.set_abort(task.abort_handle());
    let start = json!({"call_id": call_id, "cached": false, "calls_today": 0, "daily_call_cap": s.sample.settings.daily_call_cap});
    Ok(stream_response(&s, start, f.reader()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultBody {
    id: String,
    content: String,
    #[serde(default)]
    is_error: bool,
}

/// `POST /api/artifacts/{aid}/sample/{call}/tool_result`: a page tool's answer.
pub async fn tool_result(
    State(s): State<AppState>,
    Path((aid, call)): Path<(String, String)>,
    _o: SameOrigin,
    _t: RequireToken,
    ViewerCookie(cookie): ViewerCookie,
    body: Result<Json<ToolResultBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let cookie = Some(owner_browser(cookie)?);
    parse_id(&aid)?;
    let Json(b) = body.map_err(|e| ApiError::bad_request("invalid_request", e.body_text()))?;
    if b.content.len() > MAX_TOOL_RESULT_BYTES {
        return Err(ApiError::bad_request(
            "invalid_request",
            format!("a tool result is at most {MAX_TOOL_RESULT_BYTES} bytes"),
        ));
    }
    match s.sample.deliver(
        &call,
        cookie.as_deref(),
        &b.id,
        ToolOutput {
            content: b.content,
            is_error: b.is_error,
        },
    ) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(Deliver::NotFound) => Err(ApiError::not_found()),
        Err(Deliver::Forbidden) => Err(ApiError::forbidden(
            "forbidden",
            "this call belongs to another viewer",
        )),
    }
}

/// `GET /api/sample` (token): whether this daemon samples, and why not.
/// Names the key variable, never its value. `clax doctor` reads it.
pub async fn daemon(State(s): State<AppState>, _t: RequireToken) -> Json<Value> {
    use crate::sample::OffReason;
    let (reason, detail) = match s.sample.reason() {
        None => (None, None),
        Some(OffReason::NoKey) => (Some("no_key"), None),
        Some(OffReason::BadConfig(m)) => (Some("bad_config"), Some(m)),
        Some(OffReason::Disabled) => (Some("disabled"), None),
    };
    Json(json!({
        "available": s.sample.available(),
        "provider": s.sample.provider_name(),
        "reason": reason,
        "detail": detail,
        "key_env": s.sample.key_env(),
        "daily_call_cap": s.sample.settings.daily_call_cap,
    }))
}

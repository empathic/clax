//! The `sample` capability's routes (spec §6 "Sample"; `docs/contract.md`
//! "Sample protocol"). All three refuse a foreign `Origin`: pages reach them
//! only through the shell, which asks the viewer's consent first. Only the
//! owner's browser spends the key: the call and tool-result routes need the
//! bearer token (401 `unauthorized`) and a browser: the owner cookie, or a
//! viewer cookie not yet claimed for the owner (403 `forbidden`:
//! an agent or a script holding the token is not a browser), and the status
//! route answers `available: false` to a caller without the token. The call
//! route streams `start`, `text`, `tool_call`, and one `done` or `error` over
//! SSE; closing the stream cancels the call.

use crate::auth::{RequireToken, has_token};
use crate::error::ApiError;
use crate::identity::Identity;
use crate::routes::artifacts::parse_id;
use crate::sample::cache::AnswerCache;
use crate::sample::flight::Done;
use crate::sample::flight::{self, Flight, Out};
use crate::sample::request::CachePolicy;
use crate::sample::request::{self, MAX_TOOL_RESULT_BYTES, SampleBody};
use crate::sample::{Deliver, ToolOutput};
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::NaiveDate;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::time::Instant;

/// Body limit of the call route: five downsized images, base64, with room to spare.
pub const SAMPLE_BODY_LIMIT: usize = 48 * 1024 * 1024;

/// What a call does with its answer when it ends (`None`: it failed or was aborted).
type OnFinish = Box<dyn FnOnce(Option<&Done>) + Send>;

/// The daemon's local date, for the daily cap.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn rate_limited(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
}

fn start_json(call_id: &str, cached: bool, calls_today: u32, cap: Option<u32>) -> Value {
    json!({"call_id": call_id, "cached": cached, "calls_today": calls_today, "daily_call_cap": cap})
}

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

/// The key of the owner's browser making a call (the owner's browsers are one
/// person, so they share one key; a browser not yet claimed for the owner is
/// its viewer cookie); 403 `forbidden` from no browser.
fn owner_browser(who: Identity) -> Result<String, ApiError> {
    if who.owner_browser() {
        return Ok("owner".to_string());
    }
    who.cookie.ok_or_else(|| {
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
    let canonical = id.as_str().to_string();
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
        "calls_today": s.sample.counts.today(&canonical, today()),
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
    who: Identity,
    body: Result<Json<SampleBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    // `Some` from here on: the per-viewer APIs below take the cookie as an option.
    let cookie = Some(owner_browser(who)?);
    let aid = declared(&s, &aid).await?;
    let Some(provider) = s.sample.provider().cloned() else {
        return Err(ApiError::forbidden(
            "sampling_disabled",
            "no sample provider is configured on this machine",
        ));
    };
    let Json(body) = body.map_err(|e| ApiError::bad_request("invalid_request", e.body_text()))?;
    let prepared = request::prepare(body, &s.sample.settings, provider.supports_images())?;
    let cap = s.sample.settings.daily_call_cap;
    let key = AnswerCache::key(&aid, cookie.as_deref(), &prepared.input_key);
    if let CachePolicy::Window { gc, refresh: false } = prepared.cache {
        if let Some(done) = s.sample.cache.get(&key, gc, Instant::now()) {
            let start = start_json(
                &clax_core::new_ulid(),
                true,
                s.sample.counts.today(&aid, today()),
                cap,
            );
            return Ok(stream_response(
                &s,
                start,
                futures::stream::iter([Out::Done(done)]),
            ));
        }
        if let Some(f) = s.sample.cache.flight(&key) {
            let start = start_json(
                &clax_core::new_ulid(),
                true,
                s.sample.counts.today(&aid, today()),
                cap,
            );
            return Ok(stream_response(&s, start, f.reader()));
        }
    }
    let permit = s
        .sample
        .queues
        .enter(cookie.as_deref().unwrap_or("anonymous"))
        .await
        .ok_or_else(|| {
            rate_limited("too many calls from this viewer at once; try again when one has finished")
        })?;
    let calls_today =
        s.sample.counts.try_take(&aid, today(), cap).map_err(|n| {
            rate_limited(format!("this artifact reached its daily cap of {n} calls"))
        })?;
    let call_id = clax_core::new_ulid();
    let f = Flight::new();
    s.sample.open_call(&call_id, cookie);
    let on_finish: OnFinish = match prepared.cache {
        CachePolicy::Window { gc, .. } => {
            s.sample.cache.begin(key.clone(), f.clone());
            let (sampler, flight) = (s.sample.clone(), f.clone());
            Box::new(move |done: Option<&Done>| {
                if let Some(d) = done {
                    sampler
                        .cache
                        .put(key.clone(), d.clone(), gc, Instant::now());
                }
                sampler.cache.end(&key, &flight);
            })
        }
        CachePolicy::Off => Box::new(|_: Option<&Done>| {}),
    };
    let (sampler, id, flight) = (s.sample.clone(), call_id.clone(), f.clone());
    let task = tokio::spawn(async move {
        let _running = permit;
        flight::drive(sampler, id, prepared, flight, on_finish).await;
    });
    f.set_abort(task.abort_handle());
    Ok(stream_response(
        &s,
        start_json(&call_id, false, calls_today, cap),
        f.reader(),
    ))
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
    who: Identity,
    body: Result<Json<ToolResultBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let cookie = Some(owner_browser(who)?);
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

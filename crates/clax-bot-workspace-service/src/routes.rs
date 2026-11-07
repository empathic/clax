//! The wire's routes ([`clax_bot_remote::wire`]). Every request, on a route or
//! not, shows the bearer secret first, its version second and the instance
//! third, and no body is read until all three hold.

use crate::jobs::Service;
use crate::settings::BearerToken;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use clax_bot_remote::wire::{
    CODE_BAD_ID, CODE_BAD_REQUEST, CODE_KEY_TAKEN, CODE_NO_SUCH_JOB, CODE_TOO_LARGE,
    CODE_UNAUTHORIZED, CODE_WRONG_INSTANCE, JOBS_PATH, JobRequest, MAX_PREVIEW_EVENTS,
    MAX_WAIT_SECS, PREVIEWS_ACK_PATH, PREVIEWS_PATH, PreviewsAck, PreviewsAnswer, REAP_PATH,
    ReapRequest, STATUS_WIRE_SKEW, Snapshot, WIRE_HEADER, WIRE_INSTANCE_HEADER, WIRE_VERSION,
    WireFailure,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// The most bytes of a request's body.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// How long ending a run keeps its host waiting, under the 30 seconds a host
/// waits for any answer. The ending goes on past it, and the host's next word
/// from the service says how it stands.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(25);
const MAX_ID_BYTES: usize = 128;
/// A job's key is a run's ID and two numbers.
const MAX_KEY_BYTES: usize = MAX_ID_BYTES + 64;

/// Whether `id` is `limit` bytes at most, of letters, digits, `_`, `-` and
/// `extra`, starting with a letter or digit. A run's ID names a file and a
/// container, and a thread's a branch, so what passes here cannot leave a
/// directory or begin an option.
fn well_formed(id: &str, limit: usize, extra: &[u8]) -> bool {
    let allowed = |b: &u8| b.is_ascii_alphanumeric() || b"_-".contains(b) || extra.contains(b);
    id.len() <= limit
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.as_bytes().iter().all(allowed)
}

pub fn is_id(id: &str) -> bool {
    well_formed(id, MAX_ID_BYTES, b"")
}

pub fn is_key(key: &str) -> bool {
    well_formed(key, MAX_KEY_BYTES, b":")
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    let body = WireFailure {
        code: code.to_string(),
        message: message.to_string(),
        wire: None,
    };
    (status, Json(body)).into_response()
}

fn bad_id() -> Response {
    let message = "an ID holds letters, digits, underscores and hyphens";
    failure(StatusCode::BAD_REQUEST, CODE_BAD_ID, message)
}

/// `rejection` in the wire's words: over the limit stays 413.
fn unreadable(rejection: &JsonRejection) -> Response {
    let status = rejection.status();
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return failure(
            status,
            CODE_TOO_LARGE,
            "this request's body is over the limit",
        );
    }
    let message = "this request's body could not be read";
    failure(StatusCode::BAD_REQUEST, CODE_BAD_REQUEST, message)
}

/// Whether `headers` carry `token` as a bearer secret.
fn bears(headers: &HeaderMap, token: &BearerToken) -> bool {
    let given = headers.get(header::AUTHORIZATION);
    let given = given.and_then(|v| v.to_str().ok());
    let given = given.and_then(|v| v.strip_prefix("Bearer "));
    given.is_some_and(|g| token.matches(g))
}

/// Who may come in.
struct Door {
    token: BearerToken,
    instance: String,
}

/// The door: the secret, then the version, then the instance.
async fn guard(State(door): State<Arc<Door>>, request: Request, next: Next) -> Response {
    if !bears(request.headers(), &door.token) {
        let refused = failure(
            StatusCode::UNAUTHORIZED,
            CODE_UNAUTHORIZED,
            "this request does not carry the service's secret",
        );
        return ([(header::WWW_AUTHENTICATE, "Bearer")], refused).into_response();
    }
    let version = request.headers().get(WIRE_HEADER);
    let version = version.and_then(|v| v.to_str().ok());
    if version != Some(WIRE_VERSION.to_string().as_str()) {
        let status = StatusCode::from_u16(STATUS_WIRE_SKEW).expect("a status in range");
        return (status, Json(WireFailure::skew())).into_response();
    }
    let instance = request.headers().get(WIRE_INSTANCE_HEADER);
    let instance = instance.and_then(|v| v.to_str().ok());
    if instance != Some(door.instance.as_str()) {
        let message = "this service serves another instance";
        return failure(StatusCode::FORBIDDEN, CODE_WRONG_INSTANCE, message);
    }
    next.run(request).await
}

async fn start(
    State(service): State<Arc<Service>>,
    request: Result<Json<JobRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(r)) => r,
        Err(e) => return unreadable(&e),
    };
    let ids = request.run.thread_ids.iter().all(|t| is_id(t));
    if !ids || !is_id(&request.run.id) || !is_key(&request.key) {
        return bad_id();
    }
    match service.start(request) {
        Ok(answer) => Json(answer).into_response(),
        Err(_) => failure(
            StatusCode::CONFLICT,
            CODE_KEY_TAKEN,
            "this key belongs to a job of another run",
        ),
    }
}

#[derive(Deserialize)]
struct Wait {
    wait: Option<u64>,
}

async fn poll(
    State(service): State<Arc<Service>>,
    Path(key): Path<String>,
    wait: Result<Query<Wait>, QueryRejection>,
) -> Response {
    if !is_key(&key) {
        return bad_id();
    }
    let Ok(Query(Wait { wait })) = wait else {
        let message = "wait is a number of seconds";
        return failure(StatusCode::BAD_REQUEST, CODE_BAD_REQUEST, message);
    };
    let wait = Duration::from_secs(wait.unwrap_or(0).min(MAX_WAIT_SECS));
    match service.poll(&key, wait).await {
        Some(answer) => Json(answer).into_response(),
        None => failure(
            StatusCode::NOT_FOUND,
            CODE_NO_SUCH_JOB,
            "no such job, begun or ended",
        ),
    }
}

/// `work`'s answer, or within [`ANSWER_WITHIN`] how things stand. `work` is its
/// own task, so it goes on if the host gives up on the answer, or this does.
async fn within(service: &Service, work: JoinHandle<Snapshot>) -> Response {
    match tokio::time::timeout(ANSWER_WITHIN, work).await {
        Ok(Ok(snapshot)) => Json(snapshot).into_response(),
        Ok(Err(_)) => internal(),
        Err(_) => Json(service.snapshot()).into_response(),
    }
}

async fn end(State(service): State<Arc<Service>>, Path(id): Path<String>) -> Response {
    if !is_id(&id) {
        return bad_id();
    }
    let ending = service.clone();
    let ended = tokio::spawn(async move { ending.end(&id).await });
    within(&service, ended).await
}

async fn reap(
    State(service): State<Arc<Service>>,
    request: Result<Json<ReapRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(r)) => r,
        Err(e) => return unreadable(&e),
    };
    let reaping = service.clone();
    let reaped = tokio::spawn(async move { reaping.reap(&request.active).await });
    within(&service, reaped).await
}

async fn previews(
    State(service): State<Arc<Service>>,
    Extension(door): Extension<Arc<Door>>,
) -> Json<PreviewsAnswer> {
    let mut oldest = service.previews().events();
    oldest.truncate(MAX_PREVIEW_EVENTS);
    Json(PreviewsAnswer {
        instance: door.instance.clone(),
        events: oldest,
        snapshot: service.snapshot(),
    })
}

async fn acknowledge(
    State(service): State<Arc<Service>>,
    request: Result<Json<PreviewsAck>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(r)) => r,
        Err(e) => return unreadable(&e),
    };
    match service.previews().acknowledge(&request.ids) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "an acknowledgement was not kept");
            internal()
        }
    }
}

fn internal() -> Response {
    let message = "the workspace service failed: ask again";
    failure(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
}

async fn no_route() -> Response {
    failure(StatusCode::NOT_FOUND, "no_such_route", "no such route")
}

/// The service's routes, open to whoever bears `token` for `instance`.
pub fn router(service: Arc<Service>, token: BearerToken, instance: String) -> Router {
    let door = Arc::new(Door { token, instance });
    Router::new()
        .route(JOBS_PATH, post(start))
        .route(&format!("{JOBS_PATH}/{{key}}"), get(poll))
        .route("/v1/runs/{id}", delete(end))
        .route(REAP_PATH, post(reap))
        .route(PREVIEWS_PATH, get(previews))
        .route(PREVIEWS_ACK_PATH, post(acknowledge))
        .fallback(no_route)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(service)
        .layer(Extension(door.clone()))
        .layer(middleware::from_fn_with_state(door, guard))
}

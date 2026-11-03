//! Every request's id, and what it caused.
//!
//! [`layer`] gives each request an id: the caller's `x-request-id` when it
//! sent a well-formed one ([`valid`]), so a browser can name its request
//! before sending it, or else a new one ([`mint`]). The handler then runs
//! inside [`clax_core::cause::scope`] with that id, so every event it
//! publishes carries it as its `cause` (`/api/events` frames show it beside
//! the event's fields), and inside a `request` tracing span with the id, so
//! `RUST_LOG` lines name it too. The answer carries the id back as
//! `x-request-id`, and a `Server-Timing` entry for the id and the time the
//! daemon spent before answering (`req;desc="<id>", app;dur=<ms>`), which a
//! browser's Network panel shows under Timing with no collector.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use tracing::Instrument as _;

/// The header that carries a request's id, both ways.
pub static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Whether `s` may be taken as a request id: 1 to 64 ASCII letters, digits,
/// `-`, `_` or `.` (nothing a log line or a `Server-Timing` `desc` would need
/// quoting for).
pub fn valid(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A new request id: 12 hex digits of the milliseconds since the epoch, then
/// 16 random, so ids sort by when they were minted.
pub fn mint() -> String {
    let ms = clax_rt::wall::SystemTime::now()
        .duration_since(clax_rt::wall::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    format!("{:012x}{:016x}", ms & 0xffff_ffff_ffff, rand::random::<u64>())
}

/// Names the request, runs it under its id as cause, and stamps the answer
/// (see the module docs).
pub async fn layer(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .filter(|s| valid(s))
        .map(str::to_owned)
        .unwrap_or_else(mint);
    let value = HeaderValue::from_str(&id).expect("a valid id is a header value");
    req.headers_mut().insert(X_REQUEST_ID.clone(), value.clone());
    let span = tracing::info_span!(
        "request",
        id = %id,
        method = %req.method(),
        path = %req.uri().path(),
    );
    let started = clax_rt::wall::Instant::now();
    let cause: Arc<str> = id.as_str().into();
    let mut res = clax_core::cause::scope(cause, next.run(req))
        .instrument(span)
        .await;
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let h = res.headers_mut();
    h.insert(X_REQUEST_ID.clone(), value);
    let timing = format!("req;desc=\"{id}\", app;dur={ms:.1}");
    if let Ok(v) = HeaderValue::from_str(&timing) {
        h.append(axum::http::header::HeaderName::from_static("server-timing"), v);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt as _;

    #[test]
    fn ids_are_checked_and_minted_well_formed() {
        assert!(valid("01HZX-abc_1.2"));
        assert!(!valid(""));
        assert!(!valid("a b"));
        assert!(!valid("a\"b"));
        assert!(!valid(&"a".repeat(65)));
        let m = mint();
        assert!(valid(&m) && m.len() == 28, "{m}");
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/",
                get(|| async {
                    clax_core::cause::current()
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                }),
            )
            .layer(axum::middleware::from_fn(layer))
    }

    async fn body(res: Response) -> String {
        let b = axum::body::to_bytes(res.into_body(), 1 << 16).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_callers_id_is_the_handlers_cause_and_comes_back() {
        let req = Request::get("/")
            .header("x-request-id", "page-42")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(res.headers()["x-request-id"], "page-42");
        let timing = res.headers()["server-timing"].to_str().unwrap().to_owned();
        assert!(timing.starts_with("req;desc=\"page-42\", app;dur="), "{timing}");
        assert_eq!(body(res).await, "page-42");
    }

    #[tokio::test]
    async fn a_malformed_id_is_replaced() {
        let req = Request::get("/")
            .header("x-request-id", "no spaces")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let id = res.headers()["x-request-id"].to_str().unwrap().to_owned();
        assert!(valid(&id) && id != "no spaces");
        assert_eq!(body(res).await, id);
    }
}

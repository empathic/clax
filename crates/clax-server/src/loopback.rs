//! The daemon's own router, called in-process: the transport of a tool
//! set whose daemon is this one ([`clax_mcp::client::InProcess`]).
//!
//! A request goes through the whole router built by [`crate::router::build_router`]
//! (host rewrite, `/api` host check, agent authentication, the route's
//! own layers and handler), as one from the network does, with exactly
//! the credentials its sender put on it: it arrives over no connection, so
//! nothing a loopback peer is trusted with (live pages without the token,
//! the token itself) is granted, on any target. Its `Host` is [`HOST`],
//! which the `/api` check admits without a connection, whatever
//! `self_base` names (a Durable Object's public origin included). Only the
//! routes the tool set calls are reached ([`reaches`]): never an MCP
//! endpoint, so a tool call cannot re-enter one. A response is read whole
//! up to [`RESPONSE_LIMIT`]. The router is built on the first call, from
//! the state the loopback was made with: a router does not hold itself.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use clax_http::state::AppState;
use clax_mcp::client::{InProcess, InProcessFuture};
use std::sync::OnceLock;
use tower::ServiceExt;

/// The `Host` of every in-process request.
pub const HOST: &str = "localhost";

/// The most of a response an in-process request reads: above a hosted
/// file's 10 MiB and a hosted publish's 25 MiB, and a fraction of a
/// Durable Object's 128 MB isolate.
pub const RESPONSE_LIMIT: usize = 32 << 20;

/// Whether an in-process request may go to `path`: the API and the health
/// check, which are what the tool set's client calls.
pub fn reaches(path: &str) -> bool {
    path.starts_with("/api/") || path == "/healthz"
}

/// The credential an in-process request carries for the request `headers`
/// came on: its `Authorization`, as sent. It is forwarded, not judged
/// here: the router the in-process request goes through decides who it
/// speaks for, as it decided for the request itself.
pub fn caller_credential(headers: &axum::http::HeaderMap) -> Option<HeaderValue> {
    headers.get(header::AUTHORIZATION).cloned()
}

pub struct Loopback {
    state: AppState,
    router: OnceLock<axum::Router>,
    limit: usize,
}

impl Loopback {
    pub fn new(state: AppState) -> Loopback {
        assert!(
            clax_http::auth::api_host_allowed(HOST, None),
            "the /api check admits in-process requests' Host"
        );
        Loopback {
            state,
            router: OnceLock::new(),
            limit: RESPONSE_LIMIT,
        }
    }

    /// This loopback reading at most `limit` bytes of a response.
    pub fn with_response_limit(mut self, limit: usize) -> Loopback {
        self.limit = limit;
        self
    }
}

/// A JSON error answer, in the API's shape.
fn refusal(
    status: StatusCode,
    code: &str,
    message: &str,
) -> axum::http::Response<axum::body::Bytes> {
    let body = serde_json::json!({"error": {"code": code, "message": message}});
    axum::http::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Bytes::from(body.to_string()))
        .expect("static response parts")
}

impl InProcess for Loopback {
    fn call(&self, req: axum::http::Request<axum::body::Bytes>) -> InProcessFuture {
        if !reaches(req.uri().path()) {
            let res = refusal(
                StatusCode::NOT_FOUND,
                "not_in_process",
                "an in-process request reaches only /api and /healthz",
            );
            return Box::pin(async move { res });
        }
        let router = self
            .router
            .get_or_init(|| crate::router::build_router(self.state.clone()))
            .clone();
        let limit = self.limit;
        let mut req = req.map(Body::from);
        req.headers_mut()
            .insert(header::HOST, HeaderValue::from_static(HOST));
        Box::pin(async move {
            let res = match router.oneshot(req).await {
                Ok(res) => res,
                Err(never) => match never {},
            };
            let (parts, body) = res.into_parts();
            match axum::body::to_bytes(body, limit).await {
                Ok(bytes) => axum::http::Response::from_parts(parts, bytes),
                Err(e) => {
                    tracing::warn!(error = %e, limit, "an in-process response was not read");
                    refusal(
                        StatusCode::BAD_GATEWAY,
                        "response_too_large",
                        &format!("the daemon's answer was not read whole within {limit} bytes"),
                    )
                }
            }
        })
    }
}

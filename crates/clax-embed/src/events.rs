//! A subscription to the core's events (spec §2.4): `GET /api/stream`
//! sent through the router as the owner, its body's chunks (one SSE block
//! each, keep-alives included) handed to the subscriber as they come, and
//! an [`end`] block last.

use crate::core::Embed;
use crate::frame::{As, RequestHead};
use bytes::Bytes;
use futures::StreamExt;
use tower::ServiceExt;

/// The SSE block a subscription ends with, `event: end` with
/// `{"reason":…}`: `core_failed` (the core did not open), `core_closed`
/// (it closed), `stream_refused` (the router refused the stream) or
/// `stream_ended` (the stream ended or failed by itself).
pub fn end(reason: &str) -> Bytes {
    Bytes::from(format!(
        "event: end\ndata: {}\n\n",
        serde_json::json!({ "reason": reason })
    ))
}

impl Embed {
    /// Sends `GET /api/stream` as the owner, once the core is ready, and
    /// calls `on_block` with each chunk of its body until `on_block`
    /// returns false, the stream ends or the core closes; unless `on_block`
    /// said false, its last call is an [`end`] block. The subscription then
    /// ends and `on_block` is dropped. A core that does not open ends it at
    /// once, with its end block.
    pub fn subscribe(&self, on_block: Box<dyn FnMut(Bytes) -> bool + Send>) {
        let opened = self.ready();
        self.spawn(async move {
            let mut on_block = on_block;
            let reason = match opened.await {
                Ok((router, token, phase)) => {
                    match pump(router, token, phase, &mut on_block).await {
                        Some(reason) => reason,
                        None => return,
                    }
                }
                Err(reason) => reason,
            };
            on_block(end(reason));
        });
    }
}

/// Hands the stream's blocks to `on_block`; why the stream ended, or
/// `None` when `on_block` said false.
async fn pump(
    router: axum::Router,
    token: String,
    mut phase: crate::core::Readiness,
    on_block: &mut Box<dyn FnMut(Bytes) -> bool + Send>,
) -> Option<&'static str> {
    let head = RequestHead {
        method: "GET".into(),
        path: "/api/stream".into(),
        headers: vec![],
        as_: As::Owner,
        host: None,
    };
    let req = match crate::core::request(&head, Bytes::new(), &token) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "could not build the event stream's request");
            return Some("stream_refused");
        }
    };
    let res = router
        .oneshot(req)
        .await
        .unwrap_or_else(|never| match never {});
    if !res.status().is_success() {
        tracing::warn!(status = %res.status(), "the event stream was refused");
        return Some("stream_refused");
    }
    let mut body = res.into_body().into_data_stream();
    loop {
        tokio::select! {
            chunk = body.next() => match chunk {
                Some(Ok(b)) => {
                    if !on_block(b) {
                        return None;
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "the event stream failed");
                    return Some("stream_ended");
                }
                None => return Some("stream_ended"),
            },
            () = phase.closed() => return Some("core_closed"),
        }
    }
}

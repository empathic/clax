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
/// (it closed, whether the stream then ended or was refused) or
/// `stream_ended` (the stream ended or failed by itself); see [`refused`]
/// for `stream_refused`.
pub fn end(reason: &str) -> Bytes {
    block(&serde_json::json!({ "reason": reason }))
}

/// The end block of a stream the router of a ready core refused:
/// `{"reason":"stream_refused","status":…}`, with the refusal's status,
/// so the subscriber can tell a refusal that asking again cannot change
/// (400, 403, 404) from one that may pass.
pub fn refused(status: u16) -> Bytes {
    block(&serde_json::json!({ "reason": "stream_refused", "status": status }))
}

fn block(data: &serde_json::Value) -> Bytes {
    Bytes::from(format!("event: end\ndata: {data}\n\n"))
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
            let ended = match opened.await {
                Ok((router, token, phase)) => {
                    match pump(router, token, phase, &mut on_block).await {
                        Some(ended) => ended,
                        None => return,
                    }
                }
                Err(reason) => end(reason),
            };
            on_block(ended);
        });
    }
}

/// Hands the stream's blocks to `on_block`; the end block that says why
/// the stream ended, or `None` when `on_block` said false.
async fn pump(
    router: axum::Router,
    token: String,
    mut phase: crate::core::Readiness,
    on_block: &mut Box<dyn FnMut(Bytes) -> bool + Send>,
) -> Option<Bytes> {
    let head = RequestHead {
        method: "GET".into(),
        path: "/api/stream".into(),
        headers: vec![],
        as_: As::Owner,
        host: None,
    };
    // A closing core refuses or ends the stream too; either way the reason
    // is then the core's, so the phase is looked at whenever it stops.
    let why = |phase: &crate::core::Readiness, own: Bytes| {
        Some(if phase.ready() { own } else { end("core_closed") })
    };
    let req = match crate::core::request(&head, Bytes::new(), &token) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "could not build the event stream's request");
            return why(&phase, refused(500));
        }
    };
    let res = router
        .oneshot(req)
        .await
        .unwrap_or_else(|never| match never {});
    if !res.status().is_success() {
        tracing::warn!(status = %res.status(), "the event stream was refused");
        return why(&phase, refused(res.status().as_u16()));
    }
    let mut body = res.into_body().into_data_stream();
    loop {
        tokio::select! {
            biased;
            () = phase.closed() => return Some(end("core_closed")),
            chunk = body.next() => match chunk {
                Some(Ok(b)) => {
                    if !on_block(b) {
                        return None;
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "the event stream failed");
                    return why(&phase, end("stream_ended"));
                }
                None => return why(&phase, end("stream_ended")),
            },
        }
    }
}

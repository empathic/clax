//! A subscription to the core's events (spec §2.4): `GET /api/stream`
//! sent through the router as the owner, its body's chunks (one SSE block
//! each, keep-alives included) handed to the subscriber as they come.

use crate::core::Embed;
use crate::frame::{As, RequestHead};
use bytes::Bytes;
use futures::StreamExt;
use tower::ServiceExt;

impl Embed {
    /// Sends `GET /api/stream` as the owner, once the core is ready, and
    /// calls `on_block` with each chunk of its body until `on_block`
    /// returns false, the stream ends or the core closes; the subscription
    /// then ends and `on_block` is dropped. A core that does not open ends
    /// it at once.
    pub fn subscribe(&self, on_block: Box<dyn FnMut(Bytes) -> bool + Send>) {
        let opened = self.ready();
        self.spawn(async move {
            let mut on_block = on_block;
            let Some((router, token, mut phase)) = opened.await else {
                return;
            };
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
                    return;
                }
            };
            let res = router
                .oneshot(req)
                .await
                .unwrap_or_else(|never| match never {});
            if !res.status().is_success() {
                tracing::warn!(status = %res.status(), "the event stream was refused");
                return;
            }
            let mut body = res.into_body().into_data_stream();
            loop {
                tokio::select! {
                    chunk = body.next() => match chunk {
                        Some(Ok(b)) => {
                            if !on_block(b) {
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "the event stream failed");
                            break;
                        }
                        None => break,
                    },
                    () = phase.closed() => break,
                }
            }
        });
    }
}

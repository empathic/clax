//! SSE fan-out of the event bus.

use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Deserialize)]
pub struct EventsQuery {
    artifact: Option<String>,
}

pub async fn events(
    State(s): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = s.events.subscribe();
    let filter = q.artifact;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let live = BroadcastStream::new(rx).filter_map(move |item| {
        let ev = item.ok()?;
        if let Some(f) = &filter
            && ev.artifact_id() != f
        {
            return None;
        }
        let name = match &ev {
            artifax_core::Event::Version { .. } => "version",
            artifax_core::Event::ArtifactDeleted { .. } => "artifact_deleted",
        };
        Some(Ok(SseEvent::default()
            .event(name)
            .data(serde_json::to_string(&ev).unwrap())))
    });
    let mut shutdown = s.shutdown.clone();
    // A dropped sender means the state has no shutdown source; never end the stream then.
    let stop = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    Sse::new(futures::StreamExt::take_until(ready.chain(live), stop)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

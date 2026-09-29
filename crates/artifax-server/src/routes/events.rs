//! SSE fan-out of the event bus.

use crate::state::AppState;
use artifax_core::Event;
use artifax_core::db::{Caller, Level};
use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// Query string of `GET /api/events`.
#[derive(Deserialize)]
pub struct EventsQuery {
    /// When set, only events whose `artifact_id` equals this value are sent.
    /// `resync` is sent regardless, since dropped events cannot be filtered.
    artifact: Option<String>,
}

/// `GET /api/events`: a Server-Sent Events stream of the event bus.
///
/// The stream opens with `event: ready` (`data: {}`), then carries `version`,
/// `artifact_deleted`, `thread`, `comment`, `thread_resolved`,
/// `feedback_state`, and `doc` events whose data is the JSON-serialised
/// [`artifax_core::Event`]. A `doc` event carries the path and version only;
/// one for a private `data/users/<id>/` path reaches only that viewer, and
/// any other reaches only subscribers whose level may read the path. The
/// subscriber's level and viewer ([`crate::db_caller::Subscriber`]: the token
/// from `Authorization` or `?token=`, and the viewer cookie) are fixed when
/// the stream opens.
/// A subscriber that falls more than the bus capacity behind receives
/// `event: resync` with `data: {"dropped": <n>}` and then continues from the oldest
/// retained event; clients should refetch state on `resync`. A `: keep-alive`
/// comment is sent every `AppState::sse_keep_alive` while idle. The stream ends
/// when the daemon begins shutting down.
pub async fn events(
    State(s): State<AppState>,
    Query(q): Query<EventsQuery>,
    who: crate::db_caller::Subscriber,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Resolved when the stream opens: a name set later takes effect on reconnect.
    let me = s
        .store_call(move |st| who.resolve(st))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = ?e, "resolving an event subscriber failed; streaming at view");
            Caller {
                level: Level::View,
                viewer: None,
            }
        });
    let rx = s.events.subscribe();
    let filter = q.artifact;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let live = BroadcastStream::new(rx).filter_map(move |item| {
        let ev = match item {
            Ok(ev) => ev,
            Err(BroadcastStreamRecvError::Lagged(dropped)) => {
                return Some(Ok(SseEvent::default()
                    .event("resync")
                    .data(serde_json::json!({ "dropped": dropped }).to_string())));
            }
        };
        if let Some(f) = &filter
            && ev.artifact_id() != f
        {
            return None;
        }
        if let Event::Doc {
            private_to,
            read_level,
            ..
        } = &ev
        {
            let visible = match private_to {
                Some(owner) => me.viewer.as_deref() == Some(owner.as_str()),
                None => me.level >= *read_level,
            };
            if !visible {
                return None;
            }
        }
        let name = ev.name();
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
            .interval(s.sse_keep_alive)
            .text("keep-alive"),
    )
}

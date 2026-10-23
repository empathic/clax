//! SSE fan-out of the event bus.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use clax_core::db::{Caller, Level};
use clax_core::{ArtifactId, Event, Stamped};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use futures::stream::Stream;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// Query string of `GET /api/events`.
#[derive(Deserialize)]
pub struct EventsQuery {
    /// When set, only events whose `artifact_id` is in this comma list are
    /// sent. `resync` is sent regardless, since dropped events cannot be filtered.
    artifact: Option<String>,
    /// When set, only events whose name is in this comma list are sent; `ready` and `resync` always are.
    types: Option<String>,
    /// The resume point, as the `Last-Event-ID` header carries it; the header
    /// wins when both are present.
    last_event_id: Option<String>,
    /// The app the subscriber reads for: `doc` events are judged at `view` with no viewer, and each
    /// reaches the stream only while this app may read its artifact
    /// ([`clax_http::db_caller::reader_caller`]), checked as it is sent.
    reader: Option<String>,
}

/// What the stream sends after `ready`: a bus event, or `resync` after
/// `dropped` events were lost.
enum Out {
    Event(Stamped),
    Resync(u64),
}

/// How many `/api/events` streams are open in this process.
static OPEN_STREAMS: AtomicUsize = AtomicUsize::new(0);

/// Counts one open stream until dropped (the client went away, or the
/// daemon is shutting down).
struct OpenStream;

impl OpenStream {
    fn new() -> Self {
        OPEN_STREAMS.fetch_add(1, Ordering::Relaxed);
        OpenStream
    }
}

impl Drop for OpenStream {
    fn drop(&mut self) {
        OPEN_STREAMS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Debug builds only: how many event streams are open, so browser tests can
/// check that a page holds no more than one. `{open}`.
#[cfg(debug_assertions)]
pub async fn open_streams(
    _d: clax_http::agent::DeniedToGrants,
    _t: clax_http::auth::RequireToken,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "open": OPEN_STREAMS.load(Ordering::Relaxed) }))
}

/// The bus position an event ID (`<epoch>-<n>`) names, when it is this bus's.
fn resume_point(id: &str, epoch: &str) -> Option<u64> {
    let (e, n) = id.trim().split_once('-')?;
    if e != epoch || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// A comma list as a set; `None` when absent.
fn list(v: Option<String>) -> Option<BTreeSet<String>> {
    v.map(|t| {
        t.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    })
}

/// `GET /api/events`: a Server-Sent Events stream of the event bus.
///
/// The stream opens with `event: ready`, then carries `version`,
/// `artifact_deleted`, `thread`, `comment`, `thread_resolved`,
/// `thread_deleted`, `working`, `presence`, and `doc`
/// events whose data is the JSON-serialised [`clax_core::Event`]. A `doc`
/// event carries the path and version only; one for a private
/// `data/users/<id>/` path reaches only that viewer, and any other reaches
/// only subscribers whose level may read the path (for a path in an opened
/// `{self}` subtree, the level its own viewer needs). The subscriber's level
/// and viewer ([`clax_http::db_caller::Subscriber`]: the token from
/// `Authorization`, `?token=` or the events cookie, and the viewer cookie)
/// are fixed when the stream opens. A stream opened by a request that may
/// not see live pages ([`clax_http::live_access::sees_live_pages`]) carries no event
/// of a live page, and a stream without the token or without `?artifact=`
/// no event of a draft.
///
/// Every event, `ready` included, carries an SSE `id` (`<epoch>-<n>`). A
/// client reconnecting with that ID (the `Last-Event-ID` header, or
/// `?last_event_id=`) first receives the retained events it missed that its
/// filters pass, and `ready` says `{"resumed": true}`; otherwise (no ID, an
/// ID from another daemon run, or one older than every retained event)
/// `ready` says `{"resumed": false}` and the client should refetch state.
///
/// With `?reader=<A>` (an artifact ID, else 400 `invalid_args`), the
/// subscriber reads for app A: a `doc` event reaches it only when `view`
/// with no viewer may read its path, and only while A may read the event's
/// artifact, checked for each event as it is sent, so a revoked grant, a
/// withdrawn share or a dropped declaration stops the next one. Other
/// events are unchanged.
/// A subscriber that falls more than the bus capacity behind receives
/// `event: resync` with `data: {"dropped": <n>}` and then continues from the oldest
/// retained event; clients should refetch state on `resync`. A `: keep-alive`
/// comment is sent every `AppState::sse_keep_alive` while idle. The stream ends
/// when the daemon begins shutting down.
pub async fn events(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    Query(q): Query<EventsQuery>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    who: clax_http::db_caller::Subscriber,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, clax_http::error::ApiError> {
    // The event stream serves people (spec §7.5): an agent reads through
    // the filtered routes.
    d.refuse_agents()?;
    let reader =
        match &q.reader {
            None => None,
            Some(r) => Some(ArtifactId::parse(r).map_err(|_| {
                ApiError::bad_request("invalid_args", "reader is not an artifact ID")
            })?),
        };
    let hidden = (!clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token))
        .then(|| s.live_ids.clone());
    let who = who.or_events_cookie();
    let token = who.holds_token();
    // Resolved when the stream opens: a name set later takes effect on reconnect.
    let me = s
        .store_call(move |st| who.resolve(st))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = ?e, "resolving an event subscriber failed; streaming at view");
            Caller::new(Level::View, None)
        });
    let me = if reader.is_some() {
        Caller::new(me.level.min(Level::View), None)
    } else {
        me
    };
    let epoch: Arc<str> = s.events.epoch().into();
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or(q.last_event_id)
        .and_then(|id| resume_point(&id, &epoch));
    let resume = s.events.resume(after);
    let filter = list(q.artifact);
    let types = list(q.types);
    let store = s.store.clone();
    let passes = move |ev: &Event| -> bool {
        // A site's change names no artifact: only `site:` topics carry it.
        // Questions, the inbox, people and jobs are the owner's; only their
        // stream topics carry them. A delivery state is its reader's alone (spec
        // 2026-10-08 §10): only `/api/stream` carries it, to its reader.
        // Drafts go to people's streams alone (their tokens are
        // capabilities, never an agent's). Tag changes reach the gallery's
        // and artifacts' stream topics alone, a Comments switch change the
        // artifact's alone. A site's access change is its room sockets'
        // alone.
        if matches!(
            ev,
            Event::Site { .. }
                | Event::Drafts { .. }
                | Event::Question { .. }
                | Event::InboxItem { .. }
                | Event::InboxRead { .. }
                | Event::ApprovalRequest { .. }
                | Event::People { .. }
                | Event::Agents { .. }
                | Event::Job { .. }
                | Event::JobsOwner { .. }
                | Event::Tags { .. }
                | Event::Comments { .. }
                | Event::FeedbackState { .. }
                | Event::SiteAccess { .. }
                | Event::Access { .. }
        ) {
            return false;
        }
        if hidden
            .as_ref()
            .is_some_and(|live| live.contains(ev.artifact_id()))
        {
            return false;
        }
        if let Some(f) = &filter
            && !f.contains(ev.artifact_id())
        {
            return false;
        }
        // A draft's events reach only a stream that names it and holds the token.
        if (filter.is_none() || !token) && store.is_hidden(ev.artifact_id()) {
            return false;
        }
        if let Some(t) = &types
            && !t.contains(ev.name())
        {
            return false;
        }
        if let Event::Doc {
            private_to,
            read_level,
            self_read,
            ..
        } = ev
        {
            let visible = match private_to {
                Some(owner) => me.viewer.as_deref() == Some(owner.as_str()),
                None => {
                    me.level >= *read_level
                        || self_read.as_ref().is_some_and(|(owner, level)| {
                            me.viewer.as_deref() == Some(owner.as_str()) && me.level >= *level
                        })
                }
            };
            if !visible {
                return false;
            }
        }
        true
    };
    let sse = {
        let epoch = epoch.clone();
        move |st: &Stamped| {
            SseEvent::default()
                .id(format!("{epoch}-{}", st.id))
                .event(st.event.name())
                .data(serde_json::to_string(&st.event).unwrap())
        }
    };
    let at = if resume.resumed {
        after.unwrap_or(resume.last)
    } else {
        resume.last
    };
    let ready = tokio_stream::once(Ok(SseEvent::default()
        .id(format!("{epoch}-{at}"))
        .event("ready")
        .data(serde_json::json!({ "resumed": resume.resumed }).to_string())));
    let replay: Vec<Out> = resume
        .replay
        .iter()
        .filter(|st| passes(&st.event))
        .map(|st| Out::Event(st.clone()))
        .collect();
    let open = OpenStream::new();
    let live = BroadcastStream::new(resume.rx).filter_map(move |item| match item {
        Ok(st) => {
            let _held = &open;
            passes(&st.event).then_some(Out::Event(st))
        }
        Err(BroadcastStreamRecvError::Lagged(dropped)) => Some(Out::Resync(dropped)),
    });
    // Only a reader's stream asks the store about its events.
    let check = reader.map(|r| (s.clone(), r));
    let sent = futures::StreamExt::filter_map(tokio_stream::iter(replay).chain(live), move |out| {
        let (check, sse) = (check.clone(), sse.clone());
        async move {
            match out {
                Out::Resync(dropped) => Some(Ok(SseEvent::default()
                    .event("resync")
                    .data(serde_json::json!({ "dropped": dropped }).to_string()))),
                Out::Event(st) => {
                    if let (Some((state, r)), Event::Doc { artifact_id, .. }) = (check, &st.event)
                        && !reads(&state, r, artifact_id).await
                    {
                        return None;
                    }
                    Some(Ok(sse(&st)))
                }
            }
        }
    });
    let mut shutdown = s.shutdown.clone();
    // A dropped sender means the state has no shutdown source; never end the stream then.
    let stop = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    Ok(
        Sse::new(futures::StreamExt::take_until(ready.chain(sent), stop)).keep_alive(
            KeepAlive::new()
                .interval(s.sse_keep_alive)
                .text("keep-alive"),
        ),
    )
}

/// Whether `reader` may read `source`'s documents now; an error (an unknown
/// or deleted artifact, a failed read) is no.
async fn reads(s: &AppState, reader: ArtifactId, source: &str) -> bool {
    let Ok(source) = ArtifactId::parse(source) else {
        return false;
    };
    s.store_call(move |st| clax_http::db_caller::reader_caller(st, &reader, &source, Level::View))
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::resume_point;

    #[test]
    fn resume_points_name_this_bus_only() {
        assert_eq!(
            resume_point("00ff00ff00ff00ff-42", "00ff00ff00ff00ff"),
            Some(42)
        );
        assert_eq!(
            resume_point("00ff00ff00ff00ff-0", "00ff00ff00ff00ff"),
            Some(0)
        );
        for bad in [
            "1111111111111111-42",
            "00ff00ff00ff00ff-",
            "00ff00ff00ff00ff-+4",
            "42",
            "",
        ] {
            assert_eq!(resume_point(bad, "00ff00ff00ff00ff"), None, "{bad}");
        }
    }
}

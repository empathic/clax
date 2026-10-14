//! `GET /api/artifacts/<aid>/live`: a person's live socket (spec
//! 2026-10-11-clax-dev-hosted-design §16.3, §16.5), the WebSocket the shell
//! opens per artifact view to send and receive the live layers: pointers,
//! selections, typing and comment drafts ([`crate::live_layers`]).
//!
//! People only: the live layers are people-to-people, so an agent (the
//! owner's, with the token and a session, or a person's, with a grant) is
//! refused, as on every route that streams others' activity (§7.5), and a
//! request with no viewer (the token alone, no cookie) is 400 `no_viewer`.
//! Agents see posted comments, through the visibility filter, and never a
//! draft, a typing mark or a cursor. A foreign `Origin` is refused before
//! the upgrade ([`SameOrigin`]), and a live page is 404 to a caller that may
//! not see it.
//!
//! Frames are JSON text. The daemon sends `{"t":"welcome","person_id","color"}`
//! once, then batches `{"t":"live","senders","drop","others","present"}` (see
//! [`clax_core::live_layers::Relay::batches`]); the page sends the messages
//! [`clax_core::live_layers::parse_message`] reads (`open`, `typing`,
//! `draft`, `cursor`) and `ping`, answered `pong`. Malformed messages are
//! dropped. Each socket may send about
//! [`clax_core::live_layers::LIVE_RATE`] messages a second, pings included
//! ([`TokenBucket`]): the excess is dropped (a ping is still answered), and a socket over the rate for
//! [`clax_core::live_layers::LIVE_OVERRUN_SECONDS`] in a row is closed.
//!
//! A socket holds its person's live seat (spec §16.1) while it is open,
//! under a connection key of its own ([`LiveLayers::seat_conn`]): it takes
//! it with [`crate::seats::LiveSeats::take`] before it joins the hub, so a
//! spectator's socket never joins and nothing of theirs is relayed. A
//! viewer with no name takes no seat, as on the stream.
//!
//! Close codes: 4429 when no live seat is free (reason `no_live_seat`) or
//! the seat was released for want of input (reason `seat_idle`), 1008
//! after a sustained overrun, 4410 when the artifact is deleted, 1001 when
//! the daemon shuts down.

use crate::auth::Caller;
use crate::error::ApiError;
use crate::identity::Identity;
use crate::live_layers::LiveLayers;
use crate::routes::artifacts::parse_id;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use clax_core::live_layers::{MAX_LIVE_MESSAGE_BYTES, Take, TokenBucket, parse_message};
use clax_core::{CoreError, Event};
use serde_json::json;

/// The close code of a socket without a live seat.
pub const CLOSE_NO_SEAT: u16 = 4429;
/// The close code of a socket that kept sending over the rate.
pub const CLOSE_OVERRUN: u16 = 1008;
/// The close code of every socket of a deleted artifact.
pub const CLOSE_DELETED: u16 = 4410;

/// `GET /api/artifacts/{aid}/live`.
pub async fn live(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    Path(aid): Path<String>,
    _o: SameOrigin,
    who: Identity,
    caller: Caller,
    sees: crate::live::SeesLive,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // People-to-people (spec §16.3): never an agent.
    d.refuse_agents()?;
    let id = parse_id(&aid)?;
    sees.check(&s.live_ids, id.as_str())?;
    let aid = id.as_str().to_string();
    let a = aid.clone();
    let (viewer, claim) = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let viewer = who
                .browser_viewer(st)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
            Ok((viewer, crate::seats::claim_for(st, &caller, &a)?))
        })
        .await?;
    Ok(ws
        .max_message_size(MAX_LIVE_MESSAGE_BYTES)
        .max_frame_size(MAX_LIVE_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(s, aid, viewer.public_id, claim, socket)))
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

/// The seat a socket holds while it is open: left when it closes.
struct Seat {
    s: AppState,
    aid: String,
    conn: String,
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.s.live_layers.unwatch_seat(&self.conn);
        self.s.seats.leave(&self.aid, &self.conn);
    }
}

async fn session(
    s: AppState,
    aid: String,
    person: String,
    claim: Option<clax_core::seats::Claim>,
    mut socket: WebSocket,
) {
    let hub: std::sync::Arc<LiveLayers> = s.live_layers.clone();
    // Taken before the socket joins: a spectator's socket never does.
    let (_seat, mut released) = match claim {
        Some(c) => {
            let conn = hub.seat_conn();
            let released = hub.watch_seat(&conn);
            let seat = Seat {
                s: s.clone(),
                aid: aid.clone(),
                conn: conn.clone(),
            };
            if !s.seats.take(&aid, &c, &conn) {
                drop(seat);
                let _ = socket
                    .send(close(CLOSE_NO_SEAT, clax_core::seats::NO_SEAT_CODE))
                    .await;
                return;
            }
            (Some(seat), Some(released))
        }
        None => (None, None),
    };
    let mut joined = hub.join(&aid, &person);
    let key = joined.key;
    let welcome = json!({"t": "welcome", "person_id": person, "color": joined.color});
    let mut bucket = TokenBucket::new(hub.now());
    let mut events = s.events.subscribe();
    let mut shutdown = s.shutdown.clone();
    let end = 'run: {
        if socket
            .send(Message::Text(welcome.to_string().into()))
            .await
            .is_err()
        {
            break 'run None;
        }
        loop {
            tokio::select! {
                Some(batch) = joined.rx.recv() => {
                    if socket.send(Message::Text(batch.into())).await.is_err() {
                        break 'run None;
                    }
                }
                msg = socket.recv() => match msg {
                    Some(Ok(Message::Text(t))) => {
                        let t = t.as_str();
                        // Every message counts against the rate, pings too.
                        let take = bucket.take(hub.now());
                        if take == Take::Close {
                            break 'run Some(close(CLOSE_OVERRUN, "too many live messages"));
                        }
                        if t == "ping" {
                            // Answered over the rate too: the page learns the socket is open.
                            if socket.send(Message::Text("pong".into())).await.is_err() {
                                break 'run None;
                            }
                            continue;
                        }
                        if take == Take::Drop {
                            continue;
                        }
                        if let Some(m) = parse_message(t) {
                            hub.apply(&aid, key, m);
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break 'run None,
                    Some(Ok(_)) => {}
                },
                Ok(stamped) = events.recv() => {
                    if matches!(&stamped.event, Event::ArtifactDeleted { artifact_id } if *artifact_id == aid) {
                        break 'run Some(close(CLOSE_DELETED, "artifact deleted"));
                    }
                }
                r = async { released.as_mut().expect("guarded").await }, if released.is_some() => {
                    if r.is_ok() {
                        break 'run Some(close(CLOSE_NO_SEAT, clax_core::seats::IDLE_CODE));
                    }
                    released = None;
                }
                Ok(()) = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break 'run Some(close(1001, "shutting down"));
                    }
                }
            }
        }
    };
    hub.leave(&aid, key);
    if let Some(m) = end {
        let _ = socket.send(m).await;
    }
}

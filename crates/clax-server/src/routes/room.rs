//! `GET /api/artifacts/<aid>/room?peer=<label>`: the `room` capability's
//! WebSocket (spec §6, §9; contract `room.d.ts`). The shell opens one per open
//! document of the artifact in both frame modes and reuses the document's
//! peer label when it reconnects; a page never reaches it (a foreign `Origin`
//! is refused before the upgrade). The caller's level is fixed when the
//! socket opens ([`Subscriber::resolve`]: the `?token=` query parameter, never
//! logged, with or without the viewer cookie; see `docs/contract.md` "Room
//! protocol") and decides which topics it may send on: a topic declared `"interact"` in `capabilities.room.topics` admits
//! `interact` and above, every other topic `admin` and above; presence admits
//! everyone. Frames are the JSON objects of `docs/contract.md` "Room protocol".
//!
//! Rooms are keyed by artifact, not by version ([`crate::room`]): a socket
//! stays open across new versions that keep declaring `room`.
//!
//! Close codes: 4403 `not_granted` (the artifact is missing or does not declare
//! `room`), 4403 `revoked` (the artifact was deleted while the socket was
//! open, or a new version stopped declaring `room`), 4409 `replaced` (a newer socket took this peer label), 1001 when the
//! daemon shuts down.

use crate::db_caller::Subscriber;
use crate::error::ApiError;
use crate::room::{Frame, Membership, Rooms, Sender, Who};
use crate::routes::artifacts::parse_id;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use clax_core::db::{Caller, Level};
use clax_core::room::{
    MAX_JOINED, Topics, check_json, check_presence, peer_label_ok, room_name_ok, topic_ok,
};
use clax_core::{CoreError, Event};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;
use tokio_stream::StreamMap;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// Most bytes the daemon reads of one WebSocket message (and frame). A frame
/// carries at most [`clax_core::room::MAX_JSON_BYTES`] of `data` or presence
/// plus its envelope; a larger message closes the socket (1009) before it is
/// held in memory whole.
pub const MAX_ROOM_MESSAGE_BYTES: usize = 64 * 1024;

/// Sends per second that emits and presence share, and the burst (`room.d.ts` `emit`).
pub const SEND_RATE: f64 = 40.0;
pub const SEND_BURST: f64 = 80.0;

/// A token bucket of [`SEND_RATE`] per second holding at most [`SEND_BURST`].
pub struct Budget {
    tokens: f64,
    at: Instant,
}

impl Budget {
    pub fn new(now: Instant) -> Budget {
        Budget {
            tokens: SEND_BURST,
            at: now,
        }
    }

    /// Takes one send if the budget allows it at `now`.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * SEND_RATE).min(SEND_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The route's own query field; `token` is read by [`Subscriber`].
#[derive(Deserialize)]
pub struct RoomQuery {
    peer: String,
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ClientMsg {
    Presence {
        room: Option<String>,
        state: Value,
    },
    Emit {
        id: u64,
        room: Option<String>,
        topic: String,
        #[serde(default)]
        data: Option<Value>,
    },
    Join {
        id: u64,
        room: String,
    },
    Leave {
        room: String,
    },
}

type Streams = StreamMap<Option<String>, BroadcastStream<Arc<Frame>>>;

/// `GET /api/artifacts/{aid}/room`.
pub async fn room(
    State(s): State<AppState>,
    Path(aid): Path<String>,
    Query(q): Query<RoomQuery>,
    _o: SameOrigin,
    subscriber: Subscriber,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let id = parse_id(&aid)?;
    if !peer_label_ok(&q.peer) {
        return Err(ApiError::bad_request(
            "invalid_argument",
            "peer is 16 characters of [0-9a-z]",
        ));
    }
    let aid = id.as_str().to_string();
    let (artifact, caller) = s
        .store_call(move |st| Ok((st.get_artifact(&id)?, subscriber.resolve(st)?)))
        .await?;
    let declared = artifact
        .as_ref()
        .filter(|a| a.capabilities.get("room").is_some());
    let topics = declared.map(|a| Topics::from_capabilities(&a.capabilities).unwrap_or_default());
    let declares_user = declared.is_some_and(|a| a.capabilities.get("user").is_some());
    let Caller { level, viewer } = caller;
    let who = Who {
        peer: q.peer,
        by: if declares_user { viewer.clone() } else { None },
        viewer,
    };
    Ok(ws
        .max_message_size(MAX_ROOM_MESSAGE_BYTES)
        .max_frame_size(MAX_ROOM_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(s, aid, who, level, topics, socket)))
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

fn wire_peer(who: &Who, presence: &Map<String, Value>, me: &Who) -> Value {
    let mut v = serde_json::to_value(Sender::render(who, me)).expect("sender serialises");
    v["presence"] = Value::Object(presence.clone());
    v
}

fn peers_frame(room: &Option<String>, m: &Membership, me: &Who) -> Value {
    let peers: Vec<Value> = m
        .snapshot()
        .iter()
        .map(|(w, p)| wire_peer(w, p, me))
        .collect();
    json!({"t": "peers", "room": room, "peers": peers})
}

fn render(room: &Option<String>, f: &Frame, me: &Who) -> Value {
    match f {
        Frame::Peer { who, presence } => {
            json!({"t": "peer", "room": room, "peer": wire_peer(who, presence, me)})
        }
        Frame::Left { peer } => json!({"t": "left", "room": room, "peer": peer}),
        Frame::Msg { who, topic, data } => {
            let mut msg = serde_json::to_value(Sender::render(who, me)).expect("sender serialises");
            msg["topic"] = json!(topic);
            if let Some(d) = data {
                msg["data"] = d.clone();
            }
            json!({"t": "msg", "room": room, "msg": msg})
        }
    }
}

fn message_of(e: CoreError) -> String {
    match e {
        CoreError::Invalid { message, .. } => message,
        other => other.to_string(),
    }
}

/// One socket's rooms and send budget.
struct Conn {
    rooms: Arc<Rooms>,
    aid: String,
    who: Who,
    level: Level,
    topics: Topics,
    lobby: Membership,
    named: BTreeMap<String, Membership>,
    budget: Budget,
    /// Presence that arrived past the budget, latest per room.
    pending: BTreeMap<Option<String>, Map<String, Value>>,
}

impl Conn {
    fn member(&self, room: &Option<String>) -> Option<&Membership> {
        match room {
            None => Some(&self.lobby),
            Some(n) => self.named.get(n),
        }
    }

    fn handle(&mut self, text: &str, streams: &mut Streams) -> Vec<Value> {
        let Ok(msg) = serde_json::from_str::<ClientMsg>(text) else {
            tracing::debug!("room: ignored a malformed frame");
            return Vec::new();
        };
        match msg {
            ClientMsg::Presence { room, state } => {
                if check_presence(&state).is_err() || self.member(&room).is_none() {
                    return Vec::new();
                }
                let Value::Object(map) = state else {
                    return Vec::new();
                };
                if self.budget.take(Instant::now()) {
                    self.pending.remove(&room);
                    self.member(&room).expect("checked above").set_presence(map);
                } else {
                    self.pending.insert(room, map);
                }
                Vec::new()
            }
            ClientMsg::Emit {
                id,
                room,
                topic,
                data,
            } => {
                let nack = |code: &str, message: String| json!({"t": "nack", "id": id, "code": code, "message": message});
                if !topic_ok(&topic) {
                    return vec![nack(
                        "invalid_argument",
                        format!("'{topic}' is not a topic (^[a-z][a-z0-9_.-]{{0,47}}$)"),
                    )];
                }
                if let Some(d) = &data
                    && let Err(e) = check_json("data", d)
                {
                    return vec![nack("invalid_argument", message_of(e))];
                }
                if self.level < self.topics.send_level(&topic) {
                    return vec![nack(
                        "not_permitted",
                        format!("this viewer may not send on '{topic}'"),
                    )];
                }
                if self.member(&room).is_none() {
                    return vec![nack(
                        "invalid_argument",
                        "this page is not in that room".into(),
                    )];
                }
                if !self.budget.take(Instant::now()) {
                    return vec![json!({"t": "ack", "id": id, "dropped": true})];
                }
                self.member(&room).expect("checked above").emit(topic, data);
                vec![json!({"t": "ack", "id": id})]
            }
            ClientMsg::Join { id, room } => {
                if !room_name_ok(&room) {
                    return vec![json!({"t": "nack", "id": id, "code": "invalid_argument",
                        "message": format!("'{room}' is not a room name (^[a-z0-9][a-z0-9_.-]{{0,47}}$)")})];
                }
                if self.named.contains_key(&room) {
                    return vec![json!({"t": "ack", "id": id})];
                }
                if self.named.len() >= MAX_JOINED {
                    return vec![json!({"t": "nack", "id": id, "code": "limit_reached",
                        "message": format!("a page may be in at most {MAX_JOINED} named rooms")})];
                }
                let (m, rx) = self.rooms.enter(&self.aid, Some(&room), self.who.clone());
                let key = Some(room.clone());
                let snapshot = peers_frame(&key, &m, &self.who);
                streams.insert(key, BroadcastStream::new(rx));
                self.named.insert(room, m);
                vec![snapshot, json!({"t": "ack", "id": id})]
            }
            ClientMsg::Leave { room } => {
                self.pending.remove(&Some(room.clone()));
                streams.remove(&Some(room.clone()));
                self.named.remove(&room);
                Vec::new()
            }
        }
    }

    /// Sends presence that waited for the budget, while the budget allows.
    fn flush(&mut self) {
        while let Some(room) = self.pending.keys().next().cloned() {
            if !self.budget.take(Instant::now()) {
                return;
            }
            let map = self.pending.remove(&room).expect("key just read");
            if let Some(m) = self.member(&room) {
                m.set_presence(map);
            }
        }
    }
}

async fn session(
    s: AppState,
    aid: String,
    who: Who,
    level: Level,
    topics: Option<Topics>,
    mut socket: WebSocket,
) {
    let Some(topics) = topics else {
        let _ = socket.send(close(4403, "not_granted")).await;
        return;
    };
    let claim = s.rooms.claim(&aid, &who.peer);
    let (lobby, rx) = s.rooms.enter(&aid, None, who.clone());
    let mut streams: Streams = StreamMap::new();
    streams.insert(None, BroadcastStream::new(rx));
    let mut conn = Conn {
        rooms: s.rooms.clone(),
        aid: aid.clone(),
        who: who.clone(),
        level,
        topics,
        lobby,
        named: BTreeMap::new(),
        budget: Budget::new(Instant::now()),
        pending: BTreeMap::new(),
    };
    let mut events = s.events.subscribe();
    let mut shutdown = s.shutdown.clone();
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let hello = [
        json!({"t": "welcome", "peer": who.peer}),
        peers_frame(&None, &conn.lobby, &who),
    ];
    for v in hello {
        if socket
            .send(Message::Text(v.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    loop {
        let out: Vec<Value> = tokio::select! {
            () = claim.replaced.notified() => {
                drop(conn);
                let _ = socket.send(close(4409, "replaced")).await;
                return;
            }
            Ok(stamped) = events.recv() => {
                let gone = match &stamped.event {
                    Event::ArtifactDeleted { artifact_id } => *artifact_id == aid,
                    // A new version may drop `room` (a metadata edit emits no
                    // event: it takes effect at the next connection).
                    Event::Version { artifact_id, .. } if *artifact_id == aid => {
                        let id = parse_id(&aid).expect("checked at the upgrade");
                        !matches!(
                            s.store_call(move |st| st.get_artifact(&id)).await,
                            Ok(Some(a)) if a.capabilities.get("room").is_some()
                        )
                    }
                    _ => false,
                };
                if gone {
                    drop(conn);
                    let _ = socket.send(close(4403, "revoked")).await;
                    return;
                }
                continue;
            }
            Ok(()) = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = socket.send(close(1001, "shutting down")).await;
                    return;
                }
                continue;
            }
            Some((room, item)) = streams.next() => match item {
                Ok(frame) => vec![render(&room, &frame, &who)],
                Err(BroadcastStreamRecvError::Lagged(_)) => {
                    conn.member(&room).map(|m| vec![peers_frame(&room, m, &who)]).unwrap_or_default()
                }
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => conn.handle(t.as_str(), &mut streams),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => continue,
            },
            _ = tick.tick() => {
                conn.flush();
                continue;
            }
        };
        for v in out {
            if socket
                .send(Message::Text(v.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_allows_a_burst_of_80_then_40_a_second() {
        let t0 = Instant::now();
        let mut b = Budget::new(t0);
        assert!((0..80).all(|_| b.take(t0)));
        assert!(!b.take(t0));
        assert!(b.take(t0 + Duration::from_millis(25)));
        assert!(!b.take(t0 + Duration::from_millis(25)));
    }
}

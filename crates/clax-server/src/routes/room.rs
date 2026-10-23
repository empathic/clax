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
//! A person beyond the artifact's live seats is a spectator (spec
//! 2026-10-11-clax-dev-hosted-design §7, §16.1): while they are, each emit
//! is nacked `spectator` and their presence is dropped; the seat is asked
//! at each one ([`clax_http::seats::is_spectator`]), so a viewer who joins live
//! sends again on the same socket.
//!
//! Rooms are keyed by artifact, not by version ([`clax_realtime::room`]): a socket
//! stays open across new versions that keep declaring `room`.
//!
//! A socket opened on an artifact's site host (`/_clax/api/room`) keeps its
//! caller's [`SiteCredentials`] and is checked again by them: a holder's
//! whenever who may read changes (`Event::Access`, `Event::SiteAccess`),
//! before the next message it sends after such a change, and when a host
//! reopens it ([`reopen`]); a visitor's when the site is closed to the
//! world.
//!
//! Close codes: 4403 `not_granted` (the artifact is missing or does not declare
//! `room`), 4403 `revoked` (the artifact was deleted while the socket was
//! open, a new version stopped declaring `room`, or its site caller no
//! longer reads it), 4409 `replaced` (a newer socket took this peer label), 1001 when the
//! daemon shuts down.

#[cfg(not(target_arch = "wasm32"))]
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use chrono::{DateTime, Utc};
use clax_core::db::{Caller, Level};
use clax_core::room::{
    MAX_JOINED, Topics, check_json, check_presence, peer_label_ok, room_name_ok, topic_ok,
};
use clax_core::{CoreError, Event};
use clax_http::db_caller::Subscriber;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::site_access::{SiteCaller, SiteCredentials};
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_realtime::room::{Frame, Membership, Rooms, Sender, Who};
use clax_rt::time::Instant;
use clax_store::site_sessions::SITE_SESSION_RENEW_SECS;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tokio_stream::StreamExt;
#[cfg(not(target_arch = "wasm32"))]
use tokio_stream::StreamMap;
#[cfg(not(target_arch = "wasm32"))]
use tokio_stream::wrappers::BroadcastStream;
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
type Streams = StreamMap<Option<String>, BroadcastStream<Arc<Frame>>>;

/// Where a socket's rooms' frames arrive: a task's [`Streams`], or the
/// receivers a host drains itself ([`RoomSocket`]).
trait Subs {
    fn add(&mut self, room: Option<String>, rx: tokio::sync::broadcast::Receiver<Arc<Frame>>);
    fn remove(&mut self, room: &Option<String>);
}

#[cfg(not(target_arch = "wasm32"))]
impl Subs for Streams {
    fn add(&mut self, room: Option<String>, rx: tokio::sync::broadcast::Receiver<Arc<Frame>>) {
        self.insert(room, BroadcastStream::new(rx));
    }
    fn remove(&mut self, room: &Option<String>) {
        StreamMap::remove(self, room);
    }
}

type Receivers = BTreeMap<Option<String>, tokio::sync::broadcast::Receiver<Arc<Frame>>>;

impl Subs for Receivers {
    fn add(&mut self, room: Option<String>, rx: tokio::sync::broadcast::Receiver<Arc<Frame>>) {
        self.insert(room, rx);
    }
    fn remove(&mut self, room: &Option<String>) {
        BTreeMap::remove(self, room);
    }
}

/// `GET /api/artifacts/{aid}/room`.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub async fn room(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    Path(aid): Path<String>,
    Query(q): Query<RoomQuery>,
    _o: SameOrigin,
    subscriber: Subscriber,
    headers: axum::http::HeaderMap,
    extensions: axum::http::Extensions,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // The room streams presence and documents to people (spec §7.5).
    d.refuse_agents()?;
    let id = parse_id(&aid)?;
    if !peer_label_ok(&q.peer) {
        return Err(ApiError::bad_request(
            "invalid_argument",
            "peer is 16 characters of [0-9a-z]",
        ));
    }
    let slot = s
        .sockets
        .admit(&clax_http::socket_caps::SocketClient::of_request(
            &s,
            &headers,
            &extensions,
        ))?;
    let aid = id.as_str().to_string();
    // The seat its emits and presence are checked against (a spectator's
    // are refused, spec 2026-10-11-clax-dev-hosted-design §7, §16.1).
    let seat = match d.caller(&s).await {
        Ok(c) => clax_http::seats::write_claim(&s, &c, &id).await?,
        Err(_) => None,
    };
    let hosted = s.hosted.is_some();
    let (artifact, caller) = s
        .store_call(move |st| {
            let caller = subscriber.resolve_at(st, hosted, id.as_str())?;
            Ok((st.get_artifact(&id)?, caller))
        })
        .await?;
    let declared = artifact
        .as_ref()
        .filter(|a| a.capabilities.get("room").is_some());
    let topics = declared.map(|a| Topics::from_capabilities(&a.capabilities).unwrap_or_default());
    let declares_user = declared.is_some_and(|a| a.capabilities.get("user").is_some());
    let Caller { level, viewer, .. } = caller;
    let site = extensions
        .get::<SiteCaller>()
        .and_then(|c| SiteCredentials::of(c, &extensions));
    let who = Who {
        peer: q.peer,
        by: if declares_user { viewer.clone() } else { None },
        viewer,
    };
    Ok(ws
        .max_message_size(MAX_ROOM_MESSAGE_BYTES)
        .max_frame_size(MAX_ROOM_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(s, aid, who, level, topics, seat, site, slot, socket)))
}

#[cfg(not(target_arch = "wasm32"))]
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
    seats: Arc<clax_http::seats::LiveSeats>,
    /// The seat this socket's writes are checked against
    /// ([`clax_http::seats::write_claim`]).
    claim: Option<clax_core::seats::Claim>,
}

impl Conn {
    fn member(&self, room: &Option<String>) -> Option<&Membership> {
        match room {
            None => Some(&self.lobby),
            Some(n) => self.named.get(n),
        }
    }

    /// Whether the socket's viewer is a spectator now: their emits are
    /// refused and their presence is dropped.
    fn spectator(&self) -> bool {
        clax_http::seats::is_spectator(&self.seats, &self.aid, self.claim.as_ref())
    }

    fn handle(&mut self, text: &str, streams: &mut impl Subs) -> Vec<Value> {
        let Ok(msg) = serde_json::from_str::<ClientMsg>(text) else {
            tracing::debug!("room: ignored a malformed frame");
            return Vec::new();
        };
        match msg {
            ClientMsg::Presence { room, state } => {
                if check_presence(&state).is_err()
                    || self.member(&room).is_none()
                    || self.spectator()
                {
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
                if self.spectator() {
                    return vec![nack(
                        clax_core::seats::SPECTATOR_CODE,
                        clax_core::seats::SPECTATOR_MESSAGE.into(),
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
                streams.add(key, rx);
                self.named.insert(room, m);
                vec![snapshot, json!({"t": "ack", "id": id})]
            }
            ClientMsg::Leave { room } => {
                self.pending.remove(&Some(room.clone()));
                Subs::remove(streams, &Some(room.clone()));
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

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
async fn session(
    s: AppState,
    aid: String,
    who: Who,
    level: Level,
    topics: Option<Topics>,
    seat: Option<clax_core::seats::Claim>,
    site: Option<SiteCredentials>,
    _slot: clax_http::socket_caps::SocketSlot,
    mut socket: WebSocket,
) {
    use tokio::sync::broadcast::error::RecvError;
    let Some(topics) = topics else {
        let _ = socket.send(close(4403, "not_granted")).await;
        return;
    };
    // A label another viewer holds is theirs (4403 `peer_taken`).
    let Some(claim) = s.rooms.claim(&aid, &who.peer, who.viewer.as_deref()) else {
        let _ = socket.send(close(4403, "peer_taken")).await;
        return;
    };
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
        seats: s.seats.clone(),
        claim: seat,
    };
    let mut events = s.events.subscribe();
    let check = RoomCheck {
        aid: aid.clone(),
        site,
    };
    let mut renewed = s.store.now_utc();
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
            ev = events.recv() => {
                let gone = match ev {
                    Ok(stamped) => check.ended_by(&s, &stamped.event).await,
                    // Whatever was missed may have ended it.
                    Err(RecvError::Lagged(_)) => !check.stays(&s).await,
                    Err(RecvError::Closed) => return,
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
            msg = socket.recv() => {
                // The events published before this message are acted on
                // first: a right lost is lost for the very next message.
                if check.ended_by_pending(&s, &mut events).await {
                    drop(conn);
                    let _ = socket.send(close(4403, "revoked")).await;
                    return;
                }
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if check.renewal_due(&mut renewed, s.store.now_utc()) {
                            check.renew(&s).await;
                        }
                        conn.handle(t.as_str(), &mut streams)
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                    Some(Ok(_)) => continue,
                }
            }
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

/// What a room socket is checked again by ([`RoomStep::Recheck`]): its
/// artifact, and on a site host the credentials its caller was let in by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomCheck {
    pub aid: String,
    pub site: Option<SiteCredentials>,
}

impl RoomCheck {
    /// Whether the socket may stay: its artifact still declares `room`
    /// ([`still_declared`]) and, on a site host, its credentials still let
    /// its caller in ([`SiteCredentials::check`]).
    pub async fn stays(&self, s: &AppState) -> bool {
        still_declared(s, &self.aid).await && self.site_stays(s).await
    }

    /// Whether the socket's site credentials still let its caller in (a
    /// socket opened on the public host has none and always does).
    async fn site_stays(&self, s: &AppState) -> bool {
        let Some(site) = self.site.clone() else {
            return true;
        };
        let Ok(id) = parse_id(&self.aid) else {
            return false;
        };
        matches!(
            s.store_call(move |st| site.check(st, &id)).await,
            Ok(Some(_))
        )
    }

    /// Whether a message at `now` renews the socket's site session (a
    /// holder's: a visitor's has none): its last renewal, `last` (the
    /// socket's opening, before any), is [`SITE_SESSION_RENEW_SECS`] old or
    /// older. When it is, `last` moves to `now`.
    pub fn renewal_due(&self, last: &mut DateTime<Utc>, now: DateTime<Utc>) -> bool {
        if !matches!(self.site, Some(SiteCredentials::Session(_))) {
            return false;
        }
        if now - *last < chrono::Duration::seconds(SITE_SESSION_RENEW_SECS) {
            return false;
        }
        *last = now;
        true
    }

    /// Renews the socket's site session ([`clax_store::Store::site_session_renew`]):
    /// a message on a holder's socket is a use of it, as a request on the
    /// site is, so a socket in use outlives the session's TTL counted from
    /// its opening. One that has expired is not renewed.
    pub async fn renew(&self, s: &AppState) {
        let Some(SiteCredentials::Session(hash)) = self.site.clone() else {
            return;
        };
        let Ok(id) = parse_id(&self.aid) else {
            return;
        };
        let _ = s
            .store_call(move |st| st.site_session_renew(&hash, &id))
            .await;
    }

    /// What event `e` asks of the socket: [`Ask::Close`] when the artifact
    /// was deleted, or its site closed to the world and the socket is a
    /// visitor's; [`Ask::Recheck`] after a new version (it may drop `room`;
    /// a metadata edit emits no event and takes effect at the next
    /// connection), and, for a holder's socket on a site host, after its
    /// site's access or who may read the artifact changed.
    fn ask(&self, e: &Event) -> Ask {
        let holder = self.site.as_ref().is_some_and(|c| !c.visitor());
        match e {
            Event::ArtifactDeleted { artifact_id } if *artifact_id == self.aid => Ask::Close,
            Event::Version { artifact_id, .. } if *artifact_id == self.aid => Ask::Recheck,
            Event::SiteAccess { artifact_id, world } if *artifact_id == self.aid => {
                match &self.site {
                    Some(c) if c.visitor() && !world => Ask::Close,
                    Some(_) if holder => Ask::Recheck,
                    _ => Ask::Nothing,
                }
            }
            Event::Access { artifact_id }
                if holder && artifact_id.as_deref().is_none_or(|a| a == self.aid) =>
            {
                Ask::Recheck
            }
            _ => Ask::Nothing,
        }
    }

    /// Whether event `e` ends the socket ([`RoomCheck::ask`], a recheck
    /// answered by [`RoomCheck::stays`]).
    #[cfg(not(target_arch = "wasm32"))]
    async fn ended_by(&self, s: &AppState, e: &Event) -> bool {
        match self.ask(e) {
            Ask::Close => true,
            Ask::Recheck => !self.stays(s).await,
            Ask::Nothing => false,
        }
    }

    /// Whether an event already waiting in `events` ends the socket; each
    /// one read is consumed.
    #[cfg(not(target_arch = "wasm32"))]
    async fn ended_by_pending(
        &self,
        s: &AppState,
        events: &mut tokio::sync::broadcast::Receiver<clax_core::events::Stamped>,
    ) -> bool {
        use tokio::sync::broadcast::error::TryRecvError;
        let mut recheck = false;
        loop {
            match events.try_recv() {
                Ok(stamped) => match self.ask(&stamped.event) {
                    Ask::Close => return true,
                    Ask::Recheck => recheck = true,
                    Ask::Nothing => {}
                },
                Err(TryRecvError::Lagged(_)) => recheck = true,
                Err(_) => break,
            }
        }
        recheck && !self.stays(s).await
    }
}

/// What an event asks of a room socket ([`RoomCheck::ask`]).
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    Nothing,
    Close,
    Recheck,
}

/// What a host that holds a room socket itself (a Durable Object) does next.
#[derive(Debug, PartialEq, Eq)]
pub enum RoomStep {
    /// Send this text frame.
    Send(String),
    /// Close the socket with this code and reason; the [`RoomSocket`] is
    /// done.
    Close(u16, &'static str),
    /// A new version of the artifact, or (for a site holder's socket) a
    /// change to who may read it: close `revoked` unless the socket still
    /// [`RoomCheck::stays`].
    Recheck(RoomCheck),
}

/// A room socket driven by its host, step by step, where [`session`]'s
/// task cannot run: the host sends what [`RoomSocket::open`] gives first,
/// hands it every text message ([`RoomSocket::message`]), and after each
/// request, message and alarm sends what [`RoomSocket::drain`] gives. The
/// rules are [`session`]'s; presence held back by the send budget goes out
/// at the next drain, not on a 25 ms tick. Dropping it leaves its rooms.
pub struct RoomSocket {
    conn: Conn,
    receivers: Receivers,
    claim: clax_realtime::room::Claim,
    events: tokio::sync::broadcast::Receiver<clax_core::events::Stamped>,
    /// What it is checked again by.
    check: RoomCheck,
    /// When its site session was last renewed ([`RoomCheck::renewal_due`]).
    renewed: DateTime<Utc>,
    /// Its place in its client's cap ([`clax_http::socket_caps`]).
    _slot: Option<clax_http::socket_caps::SocketSlot>,
}

/// What a room socket is opened with: the artifact, the peer label, the
/// viewer and level, the topics its artifact declares (`None`: it declares
/// no `room`), and the seat its writes are checked against.
pub struct RoomAdmission {
    pub aid: String,
    pub who: Who,
    pub level: Level,
    pub topics: Option<Topics>,
    pub seat: Option<clax_core::seats::Claim>,
    /// What its site host's access let its caller in by, for a socket
    /// opened there.
    pub site: Option<SiteCredentials>,
}

impl RoomSocket {
    /// Enters the lobby and each of `rooms` (named rooms the socket was in
    /// before its host lost them): the socket and its first frames (the
    /// welcome and the lobby's peers, then each named room's peers); for
    /// an artifact that declares no `room`, the close to send.
    ///
    /// # Errors
    /// As above.
    pub fn open(
        s: &AppState,
        a: RoomAdmission,
        rooms: &[String],
    ) -> Result<(RoomSocket, Vec<String>), RoomStep> {
        let Some(topics) = a.topics else {
            return Err(RoomStep::Close(4403, "not_granted"));
        };
        let Some(claim) = s.rooms.claim(&a.aid, &a.who.peer, a.who.viewer.as_deref()) else {
            return Err(RoomStep::Close(4403, "peer_taken"));
        };
        let (lobby, rx) = s.rooms.enter(&a.aid, None, a.who.clone());
        let mut receivers = Receivers::new();
        receivers.insert(None, rx);
        let check = RoomCheck {
            aid: a.aid.clone(),
            site: a.site,
        };
        let mut conn = Conn {
            rooms: s.rooms.clone(),
            aid: a.aid,
            who: a.who.clone(),
            level: a.level,
            topics,
            lobby,
            named: BTreeMap::new(),
            budget: Budget::new(Instant::now()),
            pending: BTreeMap::new(),
            seats: s.seats.clone(),
            claim: a.seat,
        };
        let mut first = vec![
            json!({"t": "welcome", "peer": a.who.peer}).to_string(),
            peers_frame(&None, &conn.lobby, &a.who).to_string(),
        ];
        for r in rooms.iter().filter(|r| room_name_ok(r)).take(MAX_JOINED) {
            let (m, rx) = s.rooms.enter(&conn.aid, Some(r), a.who.clone());
            let key = Some(r.clone());
            first.push(peers_frame(&key, &m, &a.who).to_string());
            receivers.insert(key, rx);
            conn.named.insert(r.clone(), m);
        }
        Ok((
            RoomSocket {
                conn,
                receivers,
                claim,
                events: s.events.subscribe(),
                check,
                renewed: s.store.now_utc(),
                _slot: None,
            },
            first,
        ))
    }

    /// This socket, holding `slot` (its client's place under the cap) until
    /// it drops.
    pub fn holding(mut self, slot: clax_http::socket_caps::SocketSlot) -> RoomSocket {
        self._slot = Some(slot);
        self
    }

    /// The artifact the socket is on.
    pub fn artifact(&self) -> &str {
        &self.conn.aid
    }

    /// What its site host's access let its caller in by, for a socket
    /// opened there: what its host keeps to reopen it ([`reopen`]).
    pub fn site(&self) -> Option<&SiteCredentials> {
        self.check.site.as_ref()
    }

    /// The named rooms the socket is in, for its host to keep.
    pub fn joined(&self) -> Vec<String> {
        self.conn.named.keys().cloned().collect()
    }

    /// What the socket's message at `now` asks its host to renew, when its
    /// site session is due ([`RoomCheck::renewal_due`]): the host answers
    /// it with [`RoomCheck::renew`].
    pub fn renewal(&mut self, now: DateTime<Utc>) -> Option<RoomCheck> {
        self.check
            .renewal_due(&mut self.renewed, now)
            .then(|| self.check.clone())
    }

    /// The socket's text message `t`: the frames that answer it.
    pub fn message(&mut self, t: &str) -> Vec<String> {
        self.conn
            .handle(t, &mut self.receivers)
            .into_iter()
            .map(|v| v.to_string())
            .collect()
    }

    /// The frames its rooms sent since the last drain, then the close when
    /// a newer socket took its peer label, the artifact was deleted or (a
    /// visitor's) its site closed to the world, or [`RoomStep::Recheck`]
    /// after a new version and (a site holder's) after a change to who may
    /// read the artifact ([`RoomCheck::ask`]); the host answers it with
    /// [`RoomCheck::stays`].
    pub fn drain(&mut self) -> Vec<RoomStep> {
        use futures::FutureExt;
        let mut out = Vec::new();
        if self.claim.replaced.notified().now_or_never().is_some() {
            out.push(RoomStep::Close(4409, "replaced"));
            return out;
        }
        let mut recheck = false;
        loop {
            match self.events.try_recv() {
                Ok(stamped) => match self.check.ask(&stamped.event) {
                    Ask::Close => {
                        out.push(RoomStep::Close(4403, "revoked"));
                        return out;
                    }
                    Ask::Recheck => recheck = true,
                    Ask::Nothing => {}
                },
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => recheck = true,
                Err(_) => break,
            }
        }
        self.conn.flush();
        let who = self.conn.who.clone();
        for (room, rx) in &mut self.receivers {
            loop {
                match rx.try_recv() {
                    Ok(frame) => out.push(RoomStep::Send(render(room, &frame, &who).to_string())),
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                        let snap = match room {
                            None => Some(&self.conn.lobby),
                            Some(n) => self.conn.named.get(n),
                        };
                        if let Some(m) = snap {
                            out.push(RoomStep::Send(peers_frame(room, m, &who).to_string()));
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        if recheck {
            out.push(RoomStep::Recheck(self.check.clone()));
        }
        out
    }
}

/// Whether artifact `aid` still declares `room` (part of
/// [`RoomCheck::stays`]).
pub async fn still_declared(s: &AppState, aid: &str) -> bool {
    let Ok(id) = parse_id(aid) else {
        return false;
    };
    matches!(
        s.store_call(move |st| st.get_artifact(&id)).await,
        Ok(Some(a)) if a.capabilities.get("room").is_some()
    )
}

/// How the room route admits a socket of `aid` for `peer`, from the store:
/// the artifact's topics, the viewer and level `level_of` gives, and the
/// seat `caller`'s writes are checked against.
fn admission(
    st: &clax_store::Store,
    aid: &str,
    peer: String,
    caller: Option<&clax_http::auth::Caller>,
    resolved: Caller,
    site: Option<SiteCredentials>,
) -> clax_core::Result<RoomAdmission> {
    let id = clax_core::ArtifactId::parse(aid).map_err(|_| CoreError::NotFound)?;
    let artifact = st.get_artifact(&id)?;
    let declared = artifact
        .as_ref()
        .filter(|a| a.capabilities.get("room").is_some());
    let topics = declared.map(|a| Topics::from_capabilities(&a.capabilities).unwrap_or_default());
    let declares_user = declared.is_some_and(|a| a.capabilities.get("user").is_some());
    let seat = match caller {
        Some(
            c @ (clax_http::auth::Caller::Person { .. } | clax_http::auth::Caller::Viewer { .. }),
        ) => clax_http::seats::claim_for(st, c, aid)?,
        _ => None,
    };
    let Caller { level, viewer, .. } = resolved;
    Ok(RoomAdmission {
        aid: aid.to_string(),
        who: Who {
            peer,
            by: if declares_user { viewer.clone() } else { None },
            viewer,
        },
        level,
        topics,
        seat,
        site,
    })
}

/// What [`RoomOpened`] carries: the socket and its first frames (or the
/// close to send at once), its peer label and the credentials to reopen
/// it with.
pub type RoomOpen = (
    Result<(RoomSocket, Vec<String>), RoomStep>,
    String,
    clax_http::identity::SocketCredentials,
);

/// A room socket a host opened for its own WebSocket: the response of `GET
/// /api/artifacts/<aid>/room` on wasm32 carries it as an extension.
#[derive(Clone)]
pub struct RoomOpened(pub std::sync::Arc<std::sync::Mutex<Option<RoomOpen>>>);

/// `GET /api/artifacts/{aid}/room` on wasm32, where the socket is the
/// host's (a Durable Object's): admitted as [`room`] admits it, and
/// answered `200` with a [`RoomOpened`] extension the host replaces with
/// its socket; any other request is 426 `websocket_required`.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
pub async fn room_socket(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    Path(aid): Path<String>,
    Query(q): Query<RoomQuery>,
    _o: SameOrigin,
    subscriber: Subscriber,
    extensions: axum::http::Extensions,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    use axum::response::IntoResponse;
    d.refuse_agents()?;
    let upgrade = headers
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if !upgrade {
        return Err(ApiError::new(
            axum::http::StatusCode::UPGRADE_REQUIRED,
            "websocket_required",
            "the room rides a WebSocket",
        ));
    }
    let id = parse_id(&aid)?;
    if !peer_label_ok(&q.peer) {
        return Err(ApiError::bad_request(
            "invalid_argument",
            "peer is 16 characters of [0-9a-z]",
        ));
    }
    let aid = id.as_str().to_string();
    let client = clax_http::socket_caps::SocketClient::of_request(&s, &headers, &extensions);
    let slot = s.sockets.admit(&client)?;
    let caller = d.caller(&s).await.ok();
    let mut creds = subscriber.credentials(&s.token, &extensions);
    creds.address = client.address();
    let (a, peer) = (aid.clone(), q.peer.clone());
    let site = extensions
        .get::<SiteCaller>()
        .and_then(|c| SiteCredentials::of(c, &extensions));
    let hosted = s.hosted.is_some();
    let admitted = s
        .store_call(move |st| {
            let resolved = subscriber.resolve_at(st, hosted, &a)?;
            admission(st, &a, peer, caller.as_ref(), resolved, site)
        })
        .await?;
    let opened = RoomSocket::open(&s, admitted, &[]).map(|(r, f)| (r.holding(slot), f));
    let mut res = axum::http::StatusCode::OK.into_response();
    res.extensions_mut()
        .insert(RoomOpened(std::sync::Arc::new(std::sync::Mutex::new(
            Some((opened, q.peer, creds)),
        ))));
    Ok(res)
}

/// Reopens a room socket of `aid` under `peer`, in `rooms`, kept with
/// `creds` (and `site`, for one opened on the artifact's site host) by a
/// host that lost its server (a Durable Object woken from hibernation):
/// admitted again as [`room`] admits it, as whoever the credentials are
/// now, a hosted session still reading the page. On a site host its caller
/// is found again from `site` ([`SiteCredentials::check`]: a holder's site
/// session still live and still reading the artifact, a visitor's site
/// still open to the world) and its identity rebuilt as the site API
/// builds it. `None` when it may no longer open: the host closes it. On a
/// hosted daemon that is also a socket opened on the public host with
/// neither the token nor a session, or whose session has ended.
pub async fn reopen(
    s: &AppState,
    aid: &str,
    peer: &str,
    rooms: &[String],
    creds: &clax_http::identity::SocketCredentials,
    site: Option<&SiteCredentials>,
) -> Option<Result<(RoomSocket, Vec<String>), RoomStep>> {
    let client = clax_http::socket_caps::SocketClient::of_credentials(creds);
    let (creds, a, token, peer, site) = (
        creds.clone(),
        aid.to_string(),
        s.token.clone(),
        peer.to_string(),
        site.cloned(),
    );
    let hosted = s.hosted.is_some();
    let admitted = s
        .store_call(move |st| {
            let (who, session) = creds.reopen(st, &token)?;
            if let Some(sc) = site {
                let id = clax_core::ArtifactId::parse(&a).map_err(|_| CoreError::NotFound)?;
                let found = sc.check(st, &id)?.ok_or(CoreError::NotFound)?;
                let who = super::site_api::site_hosted_request(st, &found)?.identity(who);
                let caller = who.caller(st, None).ok();
                if caller
                    .as_ref()
                    .is_some_and(clax_http::auth::Caller::is_agent)
                {
                    return Err(CoreError::NotFound);
                }
                let resolved = clax_http::db_caller::site_caller(st, &found)?;
                return admission(st, &a, peer, caller.as_ref(), resolved, Some(sc));
            }
            // On a hosted daemon only a session or the token opened it.
            if hosted && session.is_none() && !who.token {
                return Err(CoreError::NotFound);
            }
            if let Some(sv) = &session
                && !clax_http::hosted::session_reads(st, sv, &a)?
            {
                return Err(CoreError::NotFound);
            }
            // The token on a hosted daemon is the owner's, scoped as the
            // owner's session is.
            if hosted
                && session.is_none()
                && !clax_http::hosted::MemberRights::owner(st)?.allows_at(
                    st,
                    (clax_core::rights::RightType::Artifacts, "read"),
                    &a,
                )?
            {
                return Err(CoreError::NotFound);
            }
            let caller = who.caller(st, None).ok();
            if caller
                .as_ref()
                .is_some_and(clax_http::auth::Caller::is_agent)
            {
                return Err(CoreError::NotFound);
            }
            let resolved = clax_http::db_caller::base_caller_at(st, hosted, who.token, &who, &a)?;
            admission(st, &a, peer, caller.as_ref(), resolved, None)
        })
        .await
        .ok()?;
    let slot = s.sockets.hold(&client);
    Some(RoomSocket::open(s, admitted, rooms).map(|(r, f)| (r.holding(slot), f)))
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

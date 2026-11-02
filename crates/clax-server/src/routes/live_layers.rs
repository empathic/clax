//! `GET /api/artifacts/<aid>/live`: a person's live socket (spec
//! 2026-10-11-clax-dev-hosted-design §16.3, §16.5), the WebSocket the shell
//! opens per artifact view to send and receive the live layers: pointers,
//! selections, typing and comment drafts ([`clax_realtime::live_layers`]).
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
//! it with [`clax_http::seats::LiveSeats::take`] before it joins the hub, so a
//! spectator's socket never joins and nothing of theirs is relayed. A
//! viewer with no name takes no seat, as on the stream.
//!
//! Close codes: 4429 when no live seat is free (reason `no_live_seat`) or
//! the seat was released for want of input (reason `seat_idle`), 1008
//! after a sustained overrun, 4410 when the artifact is deleted, 1001 when
//! the daemon shuts down.

#[cfg(not(target_arch = "wasm32"))]
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use clax_core::live_layers::{MAX_LIVE_MESSAGE_BYTES, Take, TokenBucket, parse_message};
use clax_core::{CoreError, Event};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_realtime::live_layers::LiveLayers;
use serde_json::json;

/// The close code of a socket without a live seat.
pub const CLOSE_NO_SEAT: u16 = 4429;
/// The close code of a socket that kept sending over the rate.
pub const CLOSE_OVERRUN: u16 = 1008;
/// The close code of every socket of a deleted artifact.
pub const CLOSE_DELETED: u16 = 4410;

/// `GET /api/artifacts/{aid}/live`.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub async fn live(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    Path(aid): Path<String>,
    _o: SameOrigin,
    who: Identity,
    caller: Caller,
    sees: clax_http::live_access::SeesLive,
    headers: axum::http::HeaderMap,
    extensions: axum::http::Extensions,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // People-to-people (spec §16.3): never an agent.
    d.refuse_agents()?;
    let id = parse_id(&aid)?;
    sees.check(&s.live_ids, id.as_str())?;
    let slot = s
        .sockets
        .admit(&clax_http::socket_caps::SocketClient::of_request(
            &s,
            &headers,
            &extensions,
        ))?;
    let aid = id.as_str().to_string();
    let a = aid.clone();
    let member = member_of(&extensions);
    let ended = crate::routes::events::owner_session_ended(&s, &extensions);
    let (viewer, claim, text) = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let viewer = who
                .browser_viewer(st)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
            let text = clax_http::hosted::reads_comments(st, member.as_ref(), &a)?;
            Ok((viewer, clax_http::seats::claim_for(st, &caller, &a)?, text))
        })
        .await?;
    Ok(ws
        .max_message_size(MAX_LIVE_MESSAGE_BYTES)
        .max_frame_size(MAX_LIVE_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            session(s, aid, viewer.public_id, claim, text, slot, ended, socket)
        }))
}

/// A hosted daemon's signed-in person's rights, as the gate set them.
fn member_of(extensions: &axum::http::Extensions) -> Option<clax_http::hosted::MemberRights> {
    extensions
        .get::<clax_http::hosted::HostedRequest>()
        .and_then(|h| h.member.clone())
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
async fn session(
    s: AppState,
    aid: String,
    person: String,
    claim: Option<clax_core::seats::Claim>,
    text: bool,
    _slot: clax_http::socket_caps::SocketSlot,
    mut ended: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
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
    let mut joined = hub.join(&aid, &person, text);
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
                // The owner browser session it was opened with ended.
                () = &mut ended => break 'run Some(close(4403, "revoked")),
            }
        }
    };
    hub.leave(&aid, key);
    if let Some(m) = end {
        let _ = socket.send(m).await;
    }
}

/// What a host that holds a live socket itself (a Durable Object, whose
/// hibernatable sockets no task of the server's reads) does next.
#[derive(Debug, PartialEq, Eq)]
pub enum LiveStep {
    /// Send this text frame.
    Send(String),
    /// Close the socket with this code and reason; the [`LiveSocket`] is
    /// done.
    Close(u16, &'static str),
}

/// A live socket driven by its host, step by step, where [`session`]'s
/// task cannot run: the host sends what [`LiveSocket::open`] gives first,
/// hands it every text message ([`LiveSocket::message`]), and after each
/// request, message and alarm, and whenever the hub says batches are
/// queued (`LiveLayers::on_queue`), sends what [`LiveSocket::drain`]
/// gives. The rules are [`session`]'s: the seat taken before joining, the
/// message rate, the close codes. Dropping it leaves the hub and the seat.
pub struct LiveSocket {
    hub: std::sync::Arc<LiveLayers>,
    aid: String,
    key: u64,
    rx: tokio::sync::mpsc::Receiver<String>,
    bucket: TokenBucket,
    events: tokio::sync::broadcast::Receiver<clax_core::events::Stamped>,
    released: Option<tokio::sync::oneshot::Receiver<()>>,
    _seat: Option<Seat>,
    /// Its place in its client's cap ([`clax_http::socket_caps`]).
    _slot: Option<clax_http::socket_caps::SocketSlot>,
}

impl Drop for LiveSocket {
    fn drop(&mut self) {
        self.hub.leave(&self.aid, self.key);
    }
}

impl LiveSocket {
    /// The artifact the socket is on.
    pub fn artifact(&self) -> &str {
        &self.aid
    }

    /// Takes `claim`'s seat (when it has one) and joins `person`'s socket
    /// to `aid`'s live layers (`text`: whether they may read comments
    /// there, [`LiveLayers::join`]): the socket and its welcome frame; with
    /// no seat free, the close to send ([`CLOSE_NO_SEAT`]).
    ///
    /// # Errors
    /// As above.
    pub fn open(
        s: &AppState,
        aid: &str,
        person: &str,
        claim: Option<clax_core::seats::Claim>,
        text: bool,
    ) -> Result<(LiveSocket, String), LiveStep> {
        let hub = s.live_layers.clone();
        let (seat, released) = match claim {
            Some(c) => {
                let conn = hub.seat_conn();
                let released = hub.watch_seat(&conn);
                let seat = Seat {
                    s: s.clone(),
                    aid: aid.to_string(),
                    conn: conn.clone(),
                };
                if !s.seats.take(aid, &c, &conn) {
                    return Err(LiveStep::Close(
                        CLOSE_NO_SEAT,
                        clax_core::seats::NO_SEAT_CODE,
                    ));
                }
                (Some(seat), Some(released))
            }
            None => (None, None),
        };
        let joined = hub.join(aid, person, text);
        let welcome = json!({"t": "welcome", "person_id": person, "color": joined.color});
        Ok((
            LiveSocket {
                bucket: TokenBucket::new(hub.now()),
                hub,
                aid: aid.to_string(),
                key: joined.key,
                rx: joined.rx,
                events: s.events.subscribe(),
                released,
                _seat: seat,
                _slot: None,
            },
            welcome.to_string(),
        ))
    }

    /// This socket, holding `slot` (its client's place under the cap) until
    /// it drops.
    pub fn holding(mut self, slot: clax_http::socket_caps::SocketSlot) -> LiveSocket {
        self._slot = Some(slot);
        self
    }

    /// The socket's text message `t`: applied, or answered (`ping` is
    /// `pong`, over the rate too), or the close for a sustained overrun.
    pub fn message(&mut self, t: &str) -> Option<LiveStep> {
        let take = self.bucket.take(self.hub.now());
        if take == Take::Close {
            return Some(LiveStep::Close(CLOSE_OVERRUN, "too many live messages"));
        }
        if t == "ping" {
            return Some(LiveStep::Send("pong".into()));
        }
        if take == Take::Drop || t.len() > MAX_LIVE_MESSAGE_BYTES {
            return None;
        }
        if let Some(m) = parse_message(t) {
            self.hub.apply(&self.aid, self.key, m);
        }
        None
    }

    /// The batches queued for the socket, then the close when its seat was
    /// released for want of input or its artifact was deleted.
    pub fn drain(&mut self) -> Vec<LiveStep> {
        let mut out = Vec::new();
        while let Ok(b) = self.rx.try_recv() {
            out.push(LiveStep::Send(b));
        }
        if let Some(r) = self.released.as_mut() {
            match r.try_recv() {
                Ok(()) => {
                    out.push(LiveStep::Close(CLOSE_NO_SEAT, clax_core::seats::IDLE_CODE));
                    return out;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => self.released = None,
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        loop {
            match self.events.try_recv() {
                Ok(stamped) => {
                    if matches!(&stamped.event, Event::ArtifactDeleted { artifact_id } if *artifact_id == self.aid)
                    {
                        out.push(LiveStep::Close(CLOSE_DELETED, "artifact deleted"));
                        return out;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        out
    }
}

/// Who may open `aid`'s live socket, as [`live`] admits them: the artifact
/// exists, and the caller is a browser with a viewer; their person's
/// public ID, seat claim, and whether they may read comments there
/// (`member`: a hosted daemon's signed-in person's rights).
fn admit(
    st: &clax_store::Store,
    who: &Identity,
    caller: &Caller,
    member: Option<&clax_http::hosted::MemberRights>,
    aid: &str,
) -> clax_core::Result<(String, Option<clax_core::seats::Claim>, bool)> {
    let id = clax_core::ArtifactId::parse(aid).map_err(|_| CoreError::NotFound)?;
    st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
    let viewer = who
        .browser_viewer(st)?
        .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
    Ok((
        viewer.public_id,
        clax_http::seats::claim_for(st, caller, aid)?,
        clax_http::hosted::reads_comments(st, member, aid)?,
    ))
}

/// What [`LiveOpened`] carries: the socket (or the close to send at once)
/// and the credentials to reopen it with.
pub type LiveOpen = (
    Result<(LiveSocket, String), LiveStep>,
    clax_http::identity::SocketCredentials,
);

/// A live socket a host opened for its own WebSocket: the response of `GET
/// /api/artifacts/<aid>/live` on wasm32 carries it as an extension, with
/// the credentials to reopen it with ([`reopen`]).
#[derive(Clone)]
pub struct LiveOpened(pub std::sync::Arc<std::sync::Mutex<Option<LiveOpen>>>);

/// `GET /api/artifacts/{aid}/live` on wasm32, where the socket is the
/// host's (a Durable Object's): admitted as [`live`] admits it, and
/// answered `200` with a [`LiveOpened`] extension the host replaces with
/// its socket; any other request is 426 `websocket_required`.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
pub async fn live_socket(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    Path(aid): Path<String>,
    _o: SameOrigin,
    who: Identity,
    caller: Caller,
    sees: clax_http::live_access::SeesLive,
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
            "the live layers ride a WebSocket",
        ));
    }
    let id = parse_id(&aid)?;
    sees.check(&s.live_ids, id.as_str())?;
    let aid = id.as_str().to_string();
    let client = clax_http::socket_caps::SocketClient::of_request(&s, &headers, &extensions);
    let slot = s.sockets.admit(&client)?;
    let mut creds = clax_http::identity::SocketCredentials::of_request(&who, &s.token, &extensions);
    creds.address = client.address();
    let a = aid.clone();
    let member = member_of(&extensions);
    let (person, claim, text) = s
        .store_call(move |st| admit(st, &who, &caller, member.as_ref(), &a))
        .await?;
    let opened =
        LiveSocket::open(&s, &aid, &person, claim, text).map(|(l, w)| (l.holding(slot), w));
    let mut res = axum::http::StatusCode::OK.into_response();
    res.extensions_mut()
        .insert(LiveOpened(std::sync::Arc::new(std::sync::Mutex::new(
            Some((opened, creds)),
        ))));
    Ok(res)
}

/// Reopens a live socket of `aid` kept with `creds` by a host that lost
/// its server (a Durable Object woken from hibernation): admitted again as
/// [`live`] admits it, as whoever the credentials are now
/// ([`clax_http::identity::SocketCredentials::reopen`]), a hosted session
/// still reading the page ([`clax_http::hosted::session_reads`]); its seat
/// taken again. `None` when it may no longer open: the host closes it. On
/// a hosted daemon that is also a socket opened with neither the token nor
/// a session, or whose session has ended.
pub async fn reopen(
    s: &AppState,
    aid: &str,
    creds: &clax_http::identity::SocketCredentials,
) -> Option<Result<(LiveSocket, String), LiveStep>> {
    let client = clax_http::socket_caps::SocketClient::of_credentials(creds);
    let (creds, a, token) = (creds.clone(), aid.to_string(), s.token.clone());
    let hosted = s.hosted.is_some();
    let (person, claim, text) = s
        .store_call(move |st| {
            let (who, session) = creds.reopen(st, &token)?;
            // On a hosted daemon only a session or the token opened it.
            if hosted && session.is_none() && !who.token {
                return Err(CoreError::NotFound);
            }
            if let Some(sv) = &session
                && !clax_http::hosted::session_reads(st, sv, &a)?
            {
                return Err(CoreError::NotFound);
            }
            let caller = who.caller(st, None).map_err(|_| CoreError::NotFound)?;
            if caller.is_agent() {
                return Err(CoreError::NotFound);
            }
            // The token on a hosted daemon is the owner's, scoped as the
            // owner's session is.
            let member = match &session {
                Some(sv) => clax_http::hosted::MemberRights::of_session(st, sv)?,
                None if hosted => Some(clax_http::hosted::MemberRights::owner(st)?),
                None => None,
            };
            if session.is_none()
                && let Some(m) = &member
                && !m.allows_at(st, (clax_core::rights::RightType::Artifacts, "read"), &a)?
            {
                return Err(CoreError::NotFound);
            }
            admit(st, &who, &caller, member.as_ref(), &a)
        })
        .await
        .ok()?;
    let slot = s.sockets.hold(&client);
    Some(LiveSocket::open(s, aid, &person, claim, text).map(|(l, w)| (l.holding(slot), w)))
}

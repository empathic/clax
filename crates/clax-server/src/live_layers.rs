//! The daemon's hub of the live layers (spec 2026-10-11-clax-dev-hosted-design
//! §16.3, §16.5): one [`Relay`] per artifact with live sockets, each socket's
//! outbound queue, and the flush that sends every receiver its batch at most
//! once per [`FLUSH_MS`]. Nothing here is stored, and nothing is carried on
//! `/api/events` or `/api/stream`.
//!
//! The socket route (`routes/live_layers.rs`) joins a socket, applies what it
//! sends, and leaves when it closes. Applying arms the artifact's flush when
//! none is armed: a task that waits [`FLUSH_MS`] and then sends the batches,
//! so a burst of updates from many senders costs one flush. A receiver whose
//! queue is full loses that batch; the relay then sends it every sender whole
//! at the next flush ([`Relay::resend`]).

use clax_core::live_layers::{ClientMessage, FLUSH_MS, Receiver, Relay, pick_color};
use clax_core::working::Clock;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Batches one socket may have queued before it loses one.
pub const SOCKET_QUEUE: usize = 16;

/// What a live socket's seat connections are keyed by: this, then a number
/// unique in the daemon ([`LiveLayers::seat_conn`]).
pub const SEAT_CONN_PREFIX: &str = "live:";

struct Socket {
    person: String,
    /// The thread its page has open, once it said.
    open: Option<Option<String>>,
    tx: mpsc::Sender<String>,
}

#[derive(Default)]
struct Room {
    relay: Relay,
    sockets: HashMap<u64, Socket>,
    /// Each present person's colour, kept while any of their sockets is open.
    colors: HashMap<String, u8>,
    armed: bool,
}

/// Every artifact's live layers.
pub struct LiveLayers {
    rooms: Mutex<HashMap<String, Room>>,
    next: Mutex<u64>,
    clock: Arc<dyn Clock>,
    flush_after: Duration,
    /// The open sockets that hold a live seat, by seat connection key: each
    /// is told when its seat is released for want of input.
    seated: Mutex<HashMap<String, oneshot::Sender<()>>>,
    /// Called after a flush queued batches, or a seat was released
    /// ([`LiveLayers::on_queue`]).
    on_queue: Mutex<Option<QueueListener>>,
}

/// Hears that live sockets have batches queued.
pub type QueueListener = Arc<dyn Fn() + Send + Sync>;

/// A socket's place in an artifact's live layers.
pub struct Joined {
    pub key: u64,
    /// The person's live colour.
    pub color: u8,
    /// The socket's batches.
    pub rx: mpsc::Receiver<String>,
}

impl LiveLayers {
    /// A hub reading time from `clock`, flushing [`FLUSH_MS`] after the
    /// first update.
    pub fn new(clock: Arc<dyn Clock>) -> LiveLayers {
        LiveLayers {
            rooms: Mutex::new(HashMap::new()),
            next: Mutex::new(0),
            clock,
            flush_after: Duration::from_millis(FLUSH_MS),
            seated: Mutex::new(HashMap::new()),
            on_queue: Mutex::new(None),
        }
    }

    /// Calls `f`, outside the hub's lock, after each flush that queued a
    /// batch and each seat released: what a host that sends sockets their
    /// batches itself (a Durable Object, through
    /// `crate::routes::live_layers::LiveSocket::drain`) listens to, as the
    /// stream hub's `on_queue`.
    pub fn on_queue(&self, f: QueueListener) {
        *self.on_queue.lock().unwrap_or_else(PoisonError::into_inner) = Some(f);
    }

    fn queued(&self) {
        let f = self
            .on_queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(f) = f {
            f();
        }
    }

    /// A new seat connection key for a live socket (`live:<n>`), under which
    /// it takes its person's seat with [`crate::seats::LiveSeats::take`].
    pub fn seat_conn(&self) -> String {
        let mut n = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        *n += 1;
        format!("{SEAT_CONN_PREFIX}{n}")
    }

    /// Socket `conn` holds a seat: the receiver hears when the seat is
    /// released ([`LiveLayers::unseat`]).
    pub fn watch_seat(&self, conn: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.seated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(conn.to_string(), tx);
        rx
    }

    /// Socket `conn` closed: it no longer hears of its seat.
    pub fn unwatch_seat(&self, conn: &str) {
        self.seated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(conn);
    }

    /// The seat socket `conn` held was released (for want of input): the
    /// socket is told, and closes 4429.
    pub fn unseat(&self, conn: &str) {
        let tx = self
            .seated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(conn);
        if let Some(tx) = tx {
            let _ = tx.send(());
            self.queued();
        }
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.now().timestamp_millis()).unwrap_or(0)
    }

    /// The hub's clock, in ms, for a socket's [`clax_core::live_layers::TokenBucket`].
    pub fn now(&self) -> u64 {
        self.now_ms()
    }

    /// Joins a socket of `person` to artifact `aid`'s live layers; it is
    /// sent what everyone else shows at the next flush, which is armed.
    pub fn join(self: &Arc<Self>, aid: &str, person: &str) -> Joined {
        let key = {
            let mut n = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            *n += 1;
            *n
        };
        let (tx, rx) = mpsc::channel(SOCKET_QUEUE);
        let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
        let room = rooms.entry(aid.to_string()).or_default();
        let color = match room.colors.get(person) {
            Some(&c) => c,
            None => {
                let taken: Vec<u8> = room.colors.values().copied().collect();
                let c = pick_color(person, &taken);
                room.colors.insert(person.to_string(), c);
                c
            }
        };
        room.sockets.insert(
            key,
            Socket {
                person: person.to_string(),
                open: None,
                tx,
            },
        );
        room.relay.resend(key);
        let arm = Self::arm(room);
        drop(rooms);
        if arm {
            self.schedule(aid);
        }
        Joined { key, color, rx }
    }

    /// Applies `m` from socket `key` of artifact `aid` and arms the flush.
    pub fn apply(self: &Arc<Self>, aid: &str, key: u64, m: ClientMessage) {
        let now = self.now_ms();
        let arm = {
            let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(room) = rooms.get_mut(aid) else {
                return;
            };
            let Some(sock) = room.sockets.get_mut(&key) else {
                return;
            };
            if let ClientMessage::Open(t) = &m {
                sock.open = Some(t.clone());
            }
            let person = sock.person.clone();
            room.relay.apply(&person, key, m, now);
            Self::arm(room)
        };
        if arm {
            self.schedule(aid);
        }
    }

    /// Socket `key` of artifact `aid` closed.
    pub fn leave(self: &Arc<Self>, aid: &str, key: u64) {
        let arm = {
            let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(room) = rooms.get_mut(aid) else {
                return;
            };
            let Some(sock) = room.sockets.remove(&key) else {
                return;
            };
            room.relay.leave(&sock.person, key);
            if !room.sockets.values().any(|s| s.person == sock.person) {
                room.colors.remove(&sock.person);
            }
            if room.sockets.is_empty() {
                rooms.remove(aid);
                return;
            }
            Self::arm(room)
        };
        if arm {
            self.schedule(aid);
        }
    }

    /// Whether the room's flush is to be armed now (it was not, and the relay has news).
    fn arm(room: &mut Room) -> bool {
        if room.armed || !room.relay.pending() {
            return false;
        }
        room.armed = true;
        true
    }

    fn schedule(self: &Arc<Self>, aid: &str) {
        let hub = self.clone();
        let aid = aid.to_string();
        clax_rt::spawn(async move {
            clax_rt::time::sleep(hub.flush_after).await;
            hub.flush(&aid);
        });
    }

    /// Sends each socket of artifact `aid` its batch (see
    /// [`Relay::batches`]); a socket whose queue is full gets everything
    /// whole at the next flush, which is then armed.
    pub fn flush(self: &Arc<Self>, aid: &str) {
        let mut sent = false;
        let again = {
            let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(room) = rooms.get_mut(aid) else {
                return;
            };
            room.armed = false;
            let receivers: Vec<Receiver> = room
                .sockets
                .iter()
                .map(|(k, s)| Receiver {
                    key: *k,
                    person: s.person.clone(),
                    open: s.open.clone(),
                })
                .collect();
            let colors = &room.colors;
            let out = room
                .relay
                .batches(&receivers, &|p| colors.get(p).copied().unwrap_or(0));
            for (key, b) in out {
                let Some(sock) = room.sockets.get(&key) else {
                    continue;
                };
                match sock.tx.try_send(b) {
                    Ok(()) => sent = true,
                    Err(mpsc::error::TrySendError::Full(_)) => room.relay.resend(key),
                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                }
            }
            Self::arm(room)
        };
        if sent {
            self.queued();
        }
        if again {
            self.schedule(aid);
        }
    }

    /// The live sockets open on `aid`.
    pub fn sockets(&self, aid: &str) -> usize {
        self.rooms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(aid)
            .map_or(0, |r| r.sockets.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use clax_core::live_layers::{LiveCursor, LiveDraft, LivePoint};

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> DateTime<Utc> {
            DateTime::from_timestamp_millis(1_000).unwrap()
        }
    }

    fn hub() -> Arc<LiveLayers> {
        Arc::new(LiveLayers::new(Arc::new(Fixed)))
    }

    fn cursor() -> ClientMessage {
        ClientMessage::Cursor(LiveCursor {
            version: 1,
            file: "index.html".into(),
            anchor: Some(LivePoint {
                path: vec![0],
                offset: -1,
                x: 0.5,
                y: 0.5,
            }),
            selection: None,
        })
    }

    #[tokio::test]
    async fn one_persons_sockets_share_a_colour_and_others_get_another() {
        let h = hub();
        let a1 = h.join("x", "u_a");
        let a2 = h.join("x", "u_a");
        let b = h.join("x", "u_b");
        assert_eq!(a1.color, a2.color);
        assert_ne!(a1.color, b.color);
        assert_eq!(h.sockets("x"), 3);
        h.leave("x", a1.key);
        h.leave("x", a2.key);
        h.leave("x", b.key);
        assert_eq!(h.sockets("x"), 0);
    }

    #[tokio::test]
    async fn a_flush_sends_each_receiver_its_batch() {
        let h = hub();
        let a = h.join("x", "u_a");
        let mut b = h.join("x", "u_b");
        h.apply("x", a.key, cursor());
        h.apply(
            "x",
            a.key,
            ClientMessage::Draft(LiveDraft {
                thread_id: None,
                text: "hi".into(),
            }),
        );
        h.flush("x");
        let got: serde_json::Value = serde_json::from_str(&b.rx.try_recv().unwrap()).unwrap();
        assert_eq!(got["senders"][0]["person_id"], "u_a");
        assert_eq!(got["senders"][0]["draft"]["text"], "hi");
        assert!(b.rx.try_recv().is_err(), "one batch per flush");
    }

    #[tokio::test]
    async fn a_full_queue_gets_everything_whole_next_time() {
        let h = hub();
        let a = h.join("x", "u_a");
        let mut b = h.join("x", "u_b");
        h.apply("x", a.key, cursor());
        h.flush("x");
        // Fill b's queue with changes it never reads.
        for i in 0..SOCKET_QUEUE + 2 {
            h.apply(
                "x",
                a.key,
                ClientMessage::Draft(LiveDraft {
                    thread_id: None,
                    text: format!("t{i}"),
                }),
            );
            h.flush("x");
        }
        while b.rx.try_recv().is_ok() {}
        h.flush("x");
        let got: serde_json::Value = serde_json::from_str(&b.rx.try_recv().unwrap()).unwrap();
        let s = &got["senders"][0];
        assert!(s.get("cursor").is_some(), "whole: {s}");
        assert_eq!(s["draft"]["text"], format!("t{}", SOCKET_QUEUE + 1));
    }
}

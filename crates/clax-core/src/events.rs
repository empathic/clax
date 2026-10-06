//! In-process broadcast of storage changes, fanned out to SSE clients by the server.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use crate::feedback::{FeedbackPhase, FeedbackState, Tier};

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A new version; `by_page` when the page published it through the
    /// `artifact` capability (open views then reload at once instead of
    /// offering a banner). Serialised only when true.
    Version {
        artifact_id: String,
        n: u32,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        by_page: bool,
        /// The artifact's title after the publish, for `/api/stream` deltas;
        /// never serialised on `/api/events`.
        #[serde(skip)]
        title: Option<String>,
        /// When the version was created, for `/api/stream` deltas; never
        /// serialised on `/api/events`.
        #[serde(skip)]
        at: Option<String>,
    },
    ArtifactDeleted {
        artifact_id: String,
    },
    /// A thread was created or changed; `thread` is its full view (upsert).
    Thread {
        artifact_id: String,
        thread: serde_json::Value,
    },
    Comment {
        artifact_id: String,
        thread_id: String,
        comment: serde_json::Value,
    },
    ThreadResolved {
        artifact_id: String,
        thread_id: String,
        resolved_by: String,
        resolved_at: String,
    },
    /// A thread and its comments were deleted. `moved` when the thread
    /// moved to another live page instead: sent to the page's topics (not
    /// `site:`) beside `thread_moved`, for clients that know only this
    /// event; never serialised.
    ThreadDeleted {
        artifact_id: String,
        thread_id: String,
        #[serde(skip)]
        moved: bool,
    },
    /// A live page's thread left `artifact_id` for the live page
    /// `to_artifact_id` (a move or a merge); a `thread` event of that page
    /// carries it there.
    ThreadMoved {
        artifact_id: String,
        thread_id: String,
        to_artifact_id: String,
    },
    FeedbackState {
        artifact_id: String,
        thread_id: String,
        state: FeedbackPhase,
        tier: Option<Tier>,
        since: String,
        resends: u32,
        exhausted: bool,
    },
    /// The artifact's working list changed; `working` is the whole list
    /// (spec §10 "Working"), never naming a session.
    Working {
        artifact_id: String,
        working: Vec<crate::working::WorkingView>,
    },
    /// The artifact's presence changed; `people` is the whole list (spec §10
    /// "Presence"), never carrying a cookie.
    Presence {
        artifact_id: String,
        people: Vec<crate::presence::PresenceView>,
    },
    /// A `db` document changed; `version` is `None` after a delete. The body
    /// is never carried (SSE needs no token). `private_to` names the viewer
    /// whose private subtree holds `path` (the event goes only to that viewer);
    /// otherwise it goes only to subscribers at `read_level` or above, and to
    /// the viewer named in `self_read` (the path is in their opened `{self}`
    /// subtree) at its level or above.
    Doc {
        artifact_id: String,
        path: String,
        version: Option<u64>,
        #[serde(skip)]
        private_to: Option<String>,
        #[serde(skip)]
        read_level: crate::db::Level,
        #[serde(skip)]
        self_read: Option<(String, crate::db::Level)>,
    },
}

impl Event {
    pub fn artifact_id(&self) -> &str {
        match self {
            Event::Version { artifact_id, .. }
            | Event::ArtifactDeleted { artifact_id }
            | Event::Thread { artifact_id, .. }
            | Event::Comment { artifact_id, .. }
            | Event::ThreadResolved { artifact_id, .. }
            | Event::ThreadDeleted { artifact_id, .. }
            | Event::ThreadMoved { artifact_id, .. }
            | Event::FeedbackState { artifact_id, .. }
            | Event::Working { artifact_id, .. }
            | Event::Presence { artifact_id, .. }
            | Event::Doc { artifact_id, .. } => artifact_id,
        }
    }

    /// The SSE event name, equal to the serialised `type`.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Version { .. } => "version",
            Event::ArtifactDeleted { .. } => "artifact_deleted",
            Event::Thread { .. } => "thread",
            Event::Comment { .. } => "comment",
            Event::ThreadResolved { .. } => "thread_resolved",
            Event::ThreadDeleted { .. } => "thread_deleted",
            Event::ThreadMoved { .. } => "thread_moved",
            Event::FeedbackState { .. } => "feedback_state",
            Event::Working { .. } => "working",
            Event::Presence { .. } => "presence",
            Event::Doc { .. } => "doc",
        }
    }

    pub fn feedback_state(artifact_id: String, s: FeedbackState) -> Event {
        Event::FeedbackState {
            artifact_id,
            thread_id: s.thread_id,
            state: s.state,
            tier: s.tier,
            since: s.since,
            resends: s.resends,
            exhausted: s.exhausted,
        }
    }
}

/// Events a subscriber may fall behind by before it lags and older events are
/// dropped; also how many recent events the bus keeps for resuming streams.
pub const EVENT_BUS_CAPACITY: usize = 256;

/// A function every published event is handed to, synchronously, before the
/// broadcast (see [`EventBus::set_tap`]).
pub type Tap = Box<dyn Fn(&Event) + Send + Sync>;

/// An event with its place on the bus: `id` grows by one per event published
/// on this bus, starting at 1.
#[derive(Clone, Debug)]
pub struct Stamped {
    pub id: u64,
    pub event: Event,
}

/// What a subscriber resuming after event `after` gets: the retained events
/// published since then (`replay`, oldest first) and a receiver for the ones
/// after those. `resumed` is false when `after` is not a point this bus can
/// resume from (it is older than every retained event, or ahead of the bus),
/// and `replay` is then empty.
pub struct Resume {
    /// The ID of the latest event published when the receiver subscribed.
    pub last: u64,
    pub replay: Vec<Stamped>,
    pub rx: broadcast::Receiver<Stamped>,
    pub resumed: bool,
}

struct Ring {
    /// The ID of the latest event published; 0 before the first.
    last: u64,
    /// The latest `EVENT_BUS_CAPACITY` events, oldest first.
    recent: VecDeque<Stamped>,
}

/// The daemon's event bus. Each bus has a random `epoch`, so a resume point
/// taken from another bus (another daemon run) is never mistaken for one of
/// its own.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Stamped>,
    ring: Arc<Mutex<Ring>>,
    epoch: Arc<str>,
    tap: Arc<std::sync::OnceLock<Tap>>,
}

impl EventBus {
    pub fn new() -> Self {
        EventBus {
            tx: broadcast::channel(EVENT_BUS_CAPACITY).0,
            ring: Arc::new(Mutex::new(Ring {
                last: 0,
                recent: VecDeque::with_capacity(EVENT_BUS_CAPACITY),
            })),
            epoch: format!("{:016x}", rand::random::<u64>()).into(),
            tap: Default::default(),
        }
    }

    /// This bus's epoch: 16 lowercase hex digits.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    fn ring(&self) -> std::sync::MutexGuard<'_, Ring> {
        // The ring is only ever left consistent: a poisoned lock still holds it.
        self.ring.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn publish(&self, event: Event) {
        if let Some(tap) = self.tap.get() {
            tap(&event);
        }
        let mut ring = self.ring();
        ring.last += 1;
        let stamped = Stamped {
            id: ring.last,
            event,
        };
        if ring.recent.len() == EVENT_BUS_CAPACITY {
            ring.recent.pop_front();
        }
        ring.recent.push_back(stamped.clone());
        // Sent under the lock, so a `resume` sees each event either in its
        // replay or on its receiver, never both and never neither.
        let _ = self.tx.send(stamped);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Stamped> {
        self.tx.subscribe()
    }

    /// The ID of the latest event published (0 before the first).
    pub fn last_id(&self) -> u64 {
        self.ring().last
    }

    /// Subscribes from just after event `after` (see [`Resume`]); `None`
    /// subscribes from now, with `resumed` false.
    pub fn resume(&self, after: Option<u64>) -> Resume {
        let ring = self.ring();
        let rx = self.tx.subscribe();
        let Some(after) = after else {
            return Resume {
                last: ring.last,
                replay: Vec::new(),
                rx,
                resumed: false,
            };
        };
        let oldest = ring.recent.front().map_or(ring.last + 1, |s| s.id);
        if after > ring.last || after + 1 < oldest {
            return Resume {
                last: ring.last,
                replay: Vec::new(),
                rx,
                resumed: false,
            };
        }
        Resume {
            last: ring.last,
            replay: ring
                .recent
                .iter()
                .filter(|s| s.id > after)
                .cloned()
                .collect(),
            rx,
            resumed: true,
        }
    }

    /// Hands every event published from now on, on this bus and its clones,
    /// to `tap` on the publishing thread, before it is numbered and
    /// broadcast. The tap must not block. A bus has at most one tap: returns
    /// `false`, leaving the first in place, when one is already set.
    pub fn set_tap(&self, tap: Tap) -> bool {
        self.tap.set(tap).is_ok()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_events_as_json() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(Event::Version {
            artifact_id: "7q3k9mzx2b4t".into(),
            n: 2,
            by_page: false,
            title: Some("T".into()),
            at: None,
        });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.id, 1);
        let ev = ev.event;
        assert_eq!(ev.artifact_id(), "7q3k9mzx2b4t");
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "version", "artifact_id": "7q3k9mzx2b4t", "n": 2})
        );
    }

    #[test]
    fn the_tap_sees_each_event_before_subscribers_and_is_set_once() {
        let bus = EventBus::new();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        assert!(bus.clone().set_tap(Box::new(move |e| {
            s2.lock().unwrap().push(e.artifact_id().to_string())
        })));
        assert!(!bus.set_tap(Box::new(|_| {})));
        bus.publish(Event::ArtifactDeleted {
            artifact_id: "a".into(),
        });
        assert_eq!(*seen.lock().unwrap(), vec!["a".to_string()]);
    }

    #[test]
    fn names_match_the_serialised_type() {
        let s = crate::feedback::FeedbackState {
            thread_id: "t".into(),
            state: FeedbackPhase::Sent,
            tier: Some(Tier::Wait),
            since: "s".into(),
            resends: 0,
            exhausted: false,
        };
        for ev in [
            Event::Thread {
                artifact_id: "a".into(),
                thread: serde_json::json!({}),
            },
            Event::Comment {
                artifact_id: "a".into(),
                thread_id: "t".into(),
                comment: serde_json::json!({}),
            },
            Event::ThreadResolved {
                artifact_id: "a".into(),
                thread_id: "t".into(),
                resolved_by: "viewer:x".into(),
                resolved_at: "r".into(),
            },
            Event::ThreadDeleted {
                artifact_id: "a".into(),
                thread_id: "t".into(),
                moved: false,
            },
            Event::feedback_state("a".into(), s),
            Event::Working {
                artifact_id: "a".into(),
                working: vec![],
            },
            Event::Presence {
                artifact_id: "a".into(),
                people: vec![],
            },
            Event::Doc {
                artifact_id: "a".into(),
                path: "t/1".into(),
                version: Some(1),
                private_to: Some("u_x".into()),
                read_level: crate::db::Level::View,
                self_read: None,
            },
        ] {
            assert_eq!(serde_json::to_value(&ev).unwrap()["type"], ev.name());
        }
    }

    #[test]
    fn doc_events_never_serialise_their_private_owner() {
        let ev = Event::Doc {
            artifact_id: "a".into(),
            path: "data/users/u_x/p".into(),
            version: None,
            private_to: Some("u_x".into()),
            read_level: crate::db::Level::Admin,
            self_read: Some(("u_x".into(), crate::db::Level::View)),
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "doc", "artifact_id": "a", "path": "data/users/u_x/p", "version": null})
        );
    }

    #[test]
    fn publish_without_subscribers_does_not_panic() {
        EventBus::new().publish(Event::ArtifactDeleted {
            artifact_id: "x".into(),
        });
    }

    fn deleted(n: u32) -> Event {
        Event::ArtifactDeleted {
            artifact_id: format!("a{n}"),
        }
    }

    #[tokio::test]
    async fn resume_replays_what_was_missed_then_continues_live() {
        let bus = EventBus::new();
        for n in 1..=3 {
            bus.publish(deleted(n));
        }
        assert_eq!(bus.last_id(), 3);
        let mut r = bus.resume(Some(1));
        assert!(r.resumed);
        assert_eq!(r.replay.iter().map(|s| s.id).collect::<Vec<_>>(), [2, 3]);
        bus.publish(deleted(4));
        assert_eq!(r.rx.recv().await.unwrap().id, 4);
        // Caught up: nothing to replay.
        let up = bus.resume(Some(4));
        assert!(up.resumed && up.replay.is_empty());
        assert!(!bus.resume(None).resumed);
    }

    #[test]
    fn resume_refuses_points_it_no_longer_holds_or_never_had() {
        let bus = EventBus::new();
        for n in 0..(EVENT_BUS_CAPACITY as u32 + 10) {
            bus.publish(deleted(n));
        }
        // The oldest retained event is 11: resuming after 10 is complete, after 9 is not.
        assert!(bus.resume(Some(10)).resumed);
        assert_eq!(bus.resume(Some(10)).replay.len(), EVENT_BUS_CAPACITY);
        let gap = bus.resume(Some(9));
        assert!(!gap.resumed && gap.replay.is_empty());
        assert!(!bus.resume(Some(bus.last_id() + 1)).resumed);
        assert_eq!(bus.epoch().len(), 16);
        assert_ne!(bus.epoch(), EventBus::new().epoch());
    }
}

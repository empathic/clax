//! In-process broadcast of storage changes, fanned out to SSE clients by the server.

use serde::Serialize;
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
    /// A thread and its comments were deleted.
    ThreadDeleted {
        artifact_id: String,
        thread_id: String,
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

/// Events a subscriber may fall behind by before it lags and older events are dropped.
pub const EVENT_BUS_CAPACITY: usize = 256;

/// A function every published event is handed to, synchronously, before the
/// broadcast (see [`EventBus::set_tap`]).
pub type Tap = Box<dyn Fn(&Event) + Send + Sync>;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
    tap: std::sync::Arc<std::sync::OnceLock<Tap>>,
}

impl EventBus {
    pub fn new() -> Self {
        EventBus {
            tx: broadcast::channel(EVENT_BUS_CAPACITY).0,
            tap: Default::default(),
        }
    }
    pub fn publish(&self, event: Event) {
        if let Some(tap) = self.tap.get() {
            tap(&event);
        }
        let _ = self.tx.send(event);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
    /// Hands every event published from now on, on this bus and its clones,
    /// to `tap` on the publishing thread, before the broadcast. The tap must
    /// not block. A bus has at most one tap: returns `false`, leaving the
    /// first in place, when one is already set.
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
}

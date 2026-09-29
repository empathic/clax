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
    FeedbackState {
        artifact_id: String,
        thread_id: String,
        state: FeedbackPhase,
        tier: Option<Tier>,
        since: String,
        resends: u32,
        exhausted: bool,
    },
    /// A `db` document changed; `version` is `None` after a delete. The body
    /// is never carried (SSE needs no token). `private_to` names the viewer
    /// whose private subtree holds `path` (the event goes only to that viewer);
    /// otherwise it goes only to subscribers at `read_level` or above.
    Doc {
        artifact_id: String,
        path: String,
        version: Option<u64>,
        #[serde(skip)]
        private_to: Option<String>,
        #[serde(skip)]
        read_level: crate::db::Level,
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
            | Event::FeedbackState { artifact_id, .. }
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
            Event::FeedbackState { .. } => "feedback_state",
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

#[derive(Clone)]
pub struct EventBus(broadcast::Sender<Event>);

impl EventBus {
    pub fn new() -> Self {
        EventBus(broadcast::channel(EVENT_BUS_CAPACITY).0)
    }
    pub fn publish(&self, event: Event) {
        let _ = self.0.send(event);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
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
        });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.artifact_id(), "7q3k9mzx2b4t");
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "version", "artifact_id": "7q3k9mzx2b4t", "n": 2})
        );
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
            Event::feedback_state("a".into(), s),
            Event::Doc {
                artifact_id: "a".into(),
                path: "t/1".into(),
                version: Some(1),
                private_to: Some("u_x".into()),
                read_level: crate::db::Level::View,
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

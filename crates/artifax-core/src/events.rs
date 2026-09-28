//! In-process broadcast of storage changes, fanned out to SSE clients by the server.

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Version { artifact_id: String, n: u32 },
    ArtifactDeleted { artifact_id: String },
}

impl Event {
    pub fn artifact_id(&self) -> &str {
        match self {
            Event::Version { artifact_id, .. } | Event::ArtifactDeleted { artifact_id } => {
                artifact_id
            }
        }
    }
}

#[derive(Clone)]
pub struct EventBus(broadcast::Sender<Event>);

impl EventBus {
    pub fn new() -> Self {
        EventBus(broadcast::channel(256).0)
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
        });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.artifact_id(), "7q3k9mzx2b4t");
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "version", "artifact_id": "7q3k9mzx2b4t", "n": 2})
        );
    }

    #[test]
    fn publish_without_subscribers_does_not_panic() {
        EventBus::new().publish(Event::ArtifactDeleted {
            artifact_id: "x".into(),
        });
    }
}

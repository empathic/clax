//! Serialisable records returned by the store and the HTTP API.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub current_version: u32,
    pub pinned: bool,
    pub capabilities: serde_json::Value,
    pub contract_version: String,
    pub owner_session_id: Option<String>,
    /// `html` (published by an agent) or `live` (a live page, spec 2026-10-05 §5.1).
    #[serde(default = "html_kind")]
    pub kind: String,
}

fn html_kind() -> String {
    crate::live::KIND_HTML.to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub content_type: String,
    pub size: u64,
    /// The file's SHA-256 in lowercase hex. Absent on versions written
    /// before version content hashes; a carried-forward file keeps its
    /// previous version's hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub artifact_id: String,
    pub n: u32,
    pub label: Option<String>,
    pub created_at: String,
    pub session_id: Option<String>,
    pub files: BTreeMap<String, FileMeta>,
    /// The publisher's change note.
    pub note: Option<String>,
    /// Thread IDs this version addressed, in link order.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// The publishing session's agent handle.
    #[serde(default)]
    pub agent: Option<String>,
    /// The publishing session's harness.
    #[serde(default)]
    pub agent_harness: Option<String>,
    /// `sha256:<hex>` of the version's file manifest
    /// ([`content_manifest_sha256`](crate::audit::content_manifest_sha256));
    /// `None` only on versions written before content hashes, until backfill.
    #[serde(default)]
    pub content_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub artifact_id: String,
    pub content_type: String,
    pub size: u64,
    pub ext: String,
    pub created_at: String,
}

/// One harness conversation (Claude Code, Codex, Pi) that publishes artifacts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub harness: String,
    pub harness_session_id: Option<String>,
    pub cwd: String,
    pub pid: Option<u32>,
    pub parent_pid: Option<u32>,
    pub started_at: String,
    pub last_seen_at: String,
    pub ended_at: Option<String>,
    /// The opaque name viewers see for this session; never its ID.
    pub agent_handle: String,
}

pub const CONTRACT_VERSION: &str = "0.2.61";

/// A comment thread anchored to one version of an artifact. `status` is `open`
/// or `resolved`; `comments` are oldest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub artifact_id: String,
    pub version_n: u32,
    pub anchor: crate::anchor::Anchor,
    pub status: String,
    pub sent_to_agent: bool,
    pub has_clip: bool,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub resolved_by: Option<String>,
    pub comments: Vec<Comment>,
}

/// `author_kind` is `viewer` or `agent`; an agent comment names the harness in
/// `author_name` and in `via_harness` (`null` on viewer comments). The
/// replying session's ID is stored but never part of a comment: it would be
/// broadcast on the unauthenticated `/api/events`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub thread_id: String,
    pub author_kind: String,
    pub author_name: String,
    /// The authoring viewer's public ID; `None` for agents and anonymous viewers.
    #[serde(default)]
    pub author_public_id: Option<String>,
    pub via_harness: Option<String>,
    /// The page wrote it through the `comments` capability, as the viewer.
    #[serde(default)]
    pub via_page: bool,
    pub body: String,
    pub created_at: String,
}

/// A session's watch on an artifact; `replies_armed` gates the Stop-hook and
/// native-push delivery tiers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub session_id: String,
    pub artifact_id: String,
    pub replies_armed: bool,
    pub created_at: String,
}

/// A browser viewer, keyed by the `clax_viewer` cookie. `id` is the cookie
/// value, the viewer's credential: it is never serialised, so no response,
/// event, or log built from a `Viewer` carries it. Others see the viewer as
/// `public_id` (`u_` and 22 lowercase hex digits).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewer {
    #[serde(skip_serializing, default)]
    pub id: String,
    pub public_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
}

/// One feedback row: a viewer comment addressed to one target session (or to
/// none, until a session publishes or watches the artifact).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub id: String,
    pub thread_id: String,
    pub comment_id: String,
    pub target_session_id: Option<String>,
    pub created_at: String,
    pub delivered_at: Option<String>,
    pub delivery_tier: Option<String>,
    pub acknowledged_at: Option<String>,
    pub resend_count: u32,
    pub last_sent_at: Option<String>,
    /// When the row became untargeted (created with no live target session, or
    /// released by its target); `None` while it has a target.
    pub untargeted_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::Feedback;

    #[test]
    fn feedback_carries_untargeted_at() {
        let f = Feedback {
            id: "f".into(),
            thread_id: "t".into(),
            comment_id: "c".into(),
            target_session_id: None,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            delivered_at: None,
            delivery_tier: None,
            acknowledged_at: None,
            resend_count: 0,
            last_sent_at: None,
            untargeted_at: Some("2026-01-02T00:00:00.000Z".into()),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["untargeted_at"], "2026-01-02T00:00:00.000Z");
    }
}

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
    /// Who wrote its title, description, icon and capabilities, for the
    /// visibility filter (spec 2026-10-08-multi-agent-multi-person §8.2).
    /// Never served.
    #[serde(skip)]
    pub authors: ArtifactAuthors,
    /// The artifact and version its version 1 was forked from; `None` for
    /// an artifact that is not a fork. Not serialised with the artifact:
    /// the routes that show it add the source's title for a reader who may
    /// read the source.
    #[serde(skip)]
    pub forked_from: Option<ForkedFrom>,
}

/// Where a fork came from: version `version` of artifact `artifact_id`,
/// which may since have been deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedFrom {
    pub artifact_id: String,
    pub version: u32,
}

/// Who wrote an artifact's own text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtifactAuthors {
    /// `artifacts.person`: the creator's person; `None`: the owner.
    pub creator: Option<String>,
    /// `artifacts.text_authors_json`: the authors of each of `title`,
    /// `description`, `icon` and `capabilities`, set by the publish or
    /// metadata change that last set it; `None` (and a field it does not
    /// list): the creator's.
    pub text: Option<BTreeMap<String, Vec<Option<String>>>>,
}

/// Who wrote a version (spec 2026-10-08-multi-agent-multi-person §5.1,
/// §8.2). Person values are public IDs, `*owner`, or `None` for the owner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionAuthors {
    /// `versions.person`: the publisher's person.
    pub person: Option<String>,
    /// A page published it: the files it wrote also hold the previous
    /// version's page authors (every author of its files).
    pub by_page: bool,
    /// `versions.file_authors_json`: each file's authors, fixed at
    /// publish; `None` (and a file it does not list): the publisher's.
    pub files: Option<BTreeMap<String, Vec<Option<String>>>>,
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
    /// Who wrote it, for the visibility filter. Never served.
    #[serde(skip)]
    pub authors: VersionAuthors,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub artifact_id: String,
    pub content_type: String,
    pub size: u64,
    pub ext: String,
    pub created_at: String,
    /// The uploader's person (`assets.person`); `None`: the owner. For the
    /// visibility filter; never served.
    #[serde(skip)]
    pub person: Option<String>,
    /// For a page's upload, the page's authors at the upload
    /// (`page_authors`); empty otherwise. Never served.
    #[serde(skip)]
    pub page_authors: Vec<String>,
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
    /// The grant it registered under; `None` for the daemon token's sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    /// Its person's public ID; `None` for the owner's agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Its agent name (spec §11), unique among live sessions ignoring case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Who gave the name: `derived`, `agent` or `person`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_source: Option<String>,
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
    /// For an agent's resolve (`resolved_by` `agent:<harness>`), whose
    /// agent it was: `*owner` or a person's public ID (`None` for a
    /// viewer's resolve, and for agents' resolves from before it was
    /// recorded). For the visibility filter; never served.
    #[serde(skip)]
    pub resolved_by_person: Option<String>,
}

/// `Comment::author_kind` of a comment a viewer wrote in the browser.
pub const AUTHOR_VIEWER: &str = "viewer";
/// `Comment::author_kind` of a comment an agent wrote.
pub const AUTHOR_AGENT: &str = "agent";

/// The text fields of an artifact that carry authors of their own (spec
/// 2026-10-08 §8.2): `artifacts.text_authors_json`'s keys.
pub const ARTIFACT_TEXT_FIELDS: [&str; 4] = ["title", "description", "icon", "capabilities"];

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
    /// For `via_page`, the page's authors at the write (`page_authors`:
    /// public IDs or `*owner`; none recorded: the owner). For the
    /// visibility filter; never served.
    #[serde(skip)]
    pub page_authors: Vec<String>,
    /// For an agent comment, its session's person (`None`: the owner's
    /// agent). For the visibility filter; never served.
    #[serde(skip)]
    pub agent_person: Option<String>,
    /// For an agent comment from a session with an agent name (spec §11
    /// "Display"), the agent: its name and person, from which each reader's
    /// author line is made. Absent otherwise (viewers' comments, and
    /// replies from before agent names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<CommentAgent>,
}

/// An agent comment's agent (spec 2026-10-08 §11 "Display"): its current
/// name, its person's public ID (`None`: the owner's agent, as
/// `thread.owner.person` names it) and that person's display name as
/// stored, as browsers read names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommentAgent {
    pub name: String,
    pub person: Option<String>,
    pub person_name: String,
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

/// A browser viewer, found by its `clax_viewer` cookies (random secrets the
/// store keeps as SHA-256 hashes; spec 2026-10-08 §21.4). `id` is its row
/// ID, which is no credential (no cookie is derived from it) and is never
/// serialised, so no response, event, or log built from a `Viewer` carries
/// it. Others see the viewer as `public_id` (`u_` and 22 lowercase hex
/// digits).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewer {
    #[serde(skip_serializing, default)]
    pub id: String,
    pub public_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
    /// A connected person (spec M1): a viewer with, or that had, a grant.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub person: bool,
    /// When the person was revoked; they are a plain viewer from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_revoked_at: Option<String>,
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

/// What a harness shim or hook reports when it starts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegisterSession {
    pub harness: String,
    #[serde(default)]
    pub harness_session_id: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub parent_pid: Option<u32>,
    /// The harness's transcript file, where the harness's extension API
    /// names one (Pi's session file). Not sent when absent, so an older
    /// daemon is not asked to read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// The grant the caller authenticated with; `None` for the daemon token.
    /// Set by the daemon from the credential, never read from a body.
    #[serde(skip)]
    pub grant_id: Option<String>,
    /// The repository's directory name, for the agent's name (spec §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The repository's branch, for the agent's name (spec §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

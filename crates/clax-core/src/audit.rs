//! Audit records (spec 2026-10-06-toolpath-audit-design §5, §6): what Clax
//! records about each change to its history, who made it, through which
//! channel, under which tool call, and from which git state.
//!
//! A record's `body` is a Clax-native JSON object, versioned `v: 1`; the
//! Toolpath rendering happens later, from the stored row.

use crate::gitctx::{self, GitField};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Declares [`AuditKind`] with the spec's name for each kind.
macro_rules! kinds {
    ($($v:ident = $s:literal,)*) => {
        /// The kind of a recorded event (spec §6). The serialized form is the
        /// spec's dotted name, e.g. `version.publish`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum AuditKind {
            $(#[serde(rename = $s)] $v,)*
        }

        impl AuditKind {
            /// Every kind, in spec order.
            pub const ALL: &'static [AuditKind] = &[$(AuditKind::$v,)*];

            /// The spec's dotted name.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(AuditKind::$v => $s,)*
                }
            }

            /// The kind with the dotted name `s`.
            pub fn parse(s: &str) -> Option<AuditKind> {
                match s {
                    $($s => Some(AuditKind::$v),)*
                    _ => None,
                }
            }
        }
    };
}

kinds! {
    ArtifactCreate = "artifact.create",
    VersionPublish = "version.publish",
    ArtifactUpdate = "artifact.update",
    ArtifactDelete = "artifact.delete",
    AssetUpload = "asset.upload",
    AssetDelete = "asset.delete",
    DocWrite = "doc.write",
    DocMove = "doc.move",
    ViewerClaim = "viewer.claim",
    ThreadOpen = "thread.open",
    CommentAdd = "comment.add",
    ThreadResolve = "thread.resolve",
    ThreadReopen = "thread.reopen",
    ThreadDelete = "thread.delete",
    ThreadSend = "thread.send",
    FeedbackDelivered = "feedback.delivered",
    FeedbackRelease = "feedback.release",
    ThreadAddressed = "thread.addressed",
    LivePage = "live.page",
    LiveSnapshot = "live.snapshot",
    ThreadMove = "thread.move",
    LiveRule = "live.rule",
    LiveJoin = "live.join",
    LiveSplit = "live.split",
    LivePageRekey = "live.page_rekey",
    LivePageMerge = "live.page_merge",
    LiveJoinAnswer = "live.join_answer",
    WatchStart = "watch.start",
    WatchStop = "watch.stop",
    WatchUpdate = "watch.update",
    WorkingStart = "working.start",
    WorkingStop = "working.stop",
    QuestionAsk = "question.ask",
    QuestionAnswer = "question.answer",
    QuestionDecline = "question.decline",
    QuestionRelease = "question.release",
    QuestionWithdraw = "question.withdraw",
    ToolCall = "tool.call",
    ToolCallId = "tool.call_id",
    SessionStart = "session.start",
    SessionJoin = "session.join",
    SessionEnd = "session.end",
    BackfillSkip = "backfill.skip",
}

impl std::fmt::Display for AuditKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An agent actor: a registered harness session, or the sessionless `/mcp`
/// route, which has no `session_id` (serialized as `null`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentActor {
    /// The Clax session ULID; `None` only for the sessionless `/mcp` route.
    pub session_id: Option<String>,
    /// The session's harness (`claude`, `codex`, `pi`, `grok`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// The ID the harness gives its own session, as the harness reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    /// The session's public agent handle (`a_…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_handle: Option<String>,
    /// The harness's transcript file, where the harness reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

/// Why Clax itself made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemReason {
    /// A working record or similar expired.
    Ttl,
    /// A live-page rule applied.
    Rule,
    /// The audit backfill of earlier history.
    Backfill,
    /// Any other internal change.
    Daemon,
}

/// Who made a change (spec §5.2). Serialized as an object tagged by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    Agent(AgentActor),
    /// The owner (token, owner cookie or extension credential).
    Owner {
        public_id: String,
    },
    /// A LAN viewer, by public ID and the display name they chose.
    Viewer {
        public_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    Anonymous,
    System {
        reason: SystemReason,
    },
}

/// The channel a change came through (spec §6.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The stdio MCP shim.
    Mcp,
    /// `clax hook`.
    Hook,
    /// The Pi extension.
    Pi,
    /// A `clax` command.
    Cli,
    /// The owner's browser shell.
    Shell,
    /// The Chrome extension.
    Extension,
    /// A LAN viewer.
    Lan,
    /// Internal: TTL sweeps, rules, backfill.
    Daemon,
}

impl Via {
    /// Every channel, in spec order.
    pub const ALL: &'static [Via] = &[
        Via::Mcp,
        Via::Hook,
        Via::Pi,
        Via::Cli,
        Via::Shell,
        Via::Extension,
        Via::Lan,
        Via::Daemon,
    ];

    /// The serialized name, e.g. `mcp`.
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Mcp => "mcp",
            Via::Hook => "hook",
            Via::Pi => "pi",
            Via::Cli => "cli",
            Via::Shell => "shell",
            Via::Extension => "extension",
            Via::Lan => "lan",
            Via::Daemon => "daemon",
        }
    }

    /// The channel named `s`, as an `x-clax-via` header gives it.
    pub fn parse(s: &str) -> Option<Via> {
        Via::ALL.iter().copied().find(|v| v.as_str() == s)
    }
}

/// The identity of the agent tool call a request is made under (spec §6.7),
/// as the agent side sends it in the `x-clax-call` header: base64url JSON of
/// at most [`MAX_HEADER_BYTES`](gitctx::MAX_HEADER_BYTES).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallHeader {
    /// A ULID the agent side minted for the call.
    pub call_id: String,
    /// The bare Clax tool name, e.g. `publish`.
    pub tool: String,
    /// The name the harness used for the tool, when the agent side knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_tool: Option<String>,
    /// `sha256:<hex>` of the canonical arguments (spec §12.2).
    pub args_sha256: String,
    /// RFC 3339 time the call began, from the agent side's clock.
    pub started_at: String,
    /// The harness's own ID for the call, where the agent side has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_call_id: Option<String>,
}

impl CallHeader {
    /// Checks the shape the daemon accepts: a ULID `call_id`; a bare tool
    /// name of lowercase letters, digits and `_`; a well-formed
    /// `args_sha256` and `started_at`; and harness names and IDs that are
    /// non-empty and free of control characters.
    pub fn validate(&self) -> Result<(), String> {
        if !crate::ids::is_ulid(&self.call_id) {
            return Err("call_id is not a ULID".into());
        }
        if self.tool.is_empty()
            || self.tool.len() > 64
            || !self
                .tool
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err("tool is not a bare Clax tool name".into());
        }
        if self.tool.contains("__") {
            return Err("tool is a harness-qualified name".into());
        }
        if !gitctx::is_sha256_ref(&self.args_sha256) {
            return Err("args_sha256 is not sha256:<64 lowercase hex>".into());
        }
        if !gitctx::is_rfc3339(&self.started_at) {
            return Err("started_at is not RFC 3339".into());
        }
        if let Some(t) = &self.harness_tool {
            gitctx::text("harness_tool", t)?;
        }
        if let Some(i) = &self.harness_call_id {
            gitctx::text("harness_call_id", i)?;
        }
        Ok(())
    }
}

/// The `x-clax-call` header value for `call`; an error when it would pass
/// [`MAX_HEADER_BYTES`](gitctx::MAX_HEADER_BYTES).
pub fn encode_call_header(call: &CallHeader) -> Result<String, String> {
    gitctx::encode_json(call)
}

/// Reads an `x-clax-call` header value: base64url JSON of at most
/// [`MAX_HEADER_BYTES`](gitctx::MAX_HEADER_BYTES) that passes [`CallHeader::validate`].
pub fn decode_call_header(value: &str) -> Result<CallHeader, String> {
    let call: CallHeader = gitctx::decode_json(value)?;
    call.validate()?;
    Ok(call)
}

/// Everything a request contributes to the records it makes: the actor, the
/// channel, the agent's git state and the tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditCtx {
    pub actor: Actor,
    pub via: Via,
    pub git: GitField,
    pub call: Option<CallHeader>,
}

impl AuditCtx {
    /// Clax itself, through the `daemon` channel, for no more specific
    /// reason: maintenance, and changes made outside any request.
    pub const DAEMON: AuditCtx = AuditCtx::system(SystemReason::Daemon);

    /// Clax itself, for `reason`, through the `daemon` channel.
    pub const fn system(reason: SystemReason) -> AuditCtx {
        AuditCtx {
            actor: Actor::System { reason },
            via: Via::Daemon,
            git: GitField::Absent,
            call: None,
        }
    }
}

/// The object IDs an event concerns, stored in their own indexed columns and
/// never repeated in the body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuditIds {
    pub artifact: Option<String>,
    /// The second artifact, the target of a `thread.move`.
    pub artifact2: Option<String>,
    pub thread: Option<String>,
    /// The Clax session of an agent actor.
    pub session: Option<String>,
    pub question: Option<String>,
    /// The tool call the event was made under.
    pub call: Option<String>,
    /// The live-page origin.
    pub origin: Option<String>,
}

/// One event to record: its kind, its time (`Store::now()` of the change),
/// its IDs and its kind-specific body fields (spec §6). The envelope fields
/// (`v`, `via`, `git`, `git_capture`, `call`) come from the [`AuditCtx`].
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    pub kind: AuditKind,
    pub at: String,
    pub ids: AuditIds,
    pub body: BTreeMap<String, serde_json::Value>,
}

impl AuditRecord {
    /// A record of `kind` at `at`, with no IDs and an empty body.
    pub fn new(kind: AuditKind, at: impl Into<String>) -> AuditRecord {
        AuditRecord {
            kind,
            at: at.into(),
            ids: AuditIds::default(),
            body: BTreeMap::new(),
        }
    }

    /// Sets body field `key`.
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> AuditRecord {
        self.body.insert(key.to_owned(), value.into());
        self
    }
}

/// The lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A version's content hash (spec §5.3): `sha256:` and the hex SHA-256 of
/// its manifest, one `<path>\0<sha256 hex>\0<size>\n` line per file in path
/// order. `None` when a file has no hash.
pub fn content_manifest_sha256(files: &BTreeMap<String, crate::model::FileMeta>) -> Option<String> {
    let mut manifest = Vec::new();
    for (path, meta) in files {
        let sha = meta.sha256.as_deref()?;
        manifest.extend_from_slice(path.as_bytes());
        manifest.push(0);
        manifest.extend_from_slice(sha.as_bytes());
        manifest.push(0);
        manifest.extend_from_slice(meta.size.to_string().as_bytes());
        manifest.push(b'\n');
    }
    Some(format!("sha256:{}", sha256_hex(&manifest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitctx::MAX_HEADER_BYTES;
    use serde_json::json;

    #[test]
    fn content_manifest_sha256_hashes_the_path_ordered_manifest() {
        let meta = |bytes: &[u8]| crate::model::FileMeta {
            content_type: "text/plain".into(),
            size: bytes.len() as u64,
            sha256: Some(sha256_hex(bytes)),
        };
        let files = BTreeMap::from([
            ("index.html".to_string(), meta(b"<p>hi</p>")),
            ("app.js".to_string(), meta(b"x")),
        ]);
        let manifest = format!(
            "app.js\0{}\01\nindex.html\0{}\09\n",
            sha256_hex(b"x"),
            sha256_hex(b"<p>hi</p>")
        );
        let expected = "sha256:33754657b6a0ce355999e0dc8adbb0d2c6c2780035a2c1d9e43d707782c3ad9a";
        assert_eq!(
            format!("sha256:{}", sha256_hex(manifest.as_bytes())),
            expected
        );
        assert_eq!(content_manifest_sha256(&files).as_deref(), Some(expected));
        assert_eq!(
            content_manifest_sha256(&BTreeMap::new()).as_deref(),
            Some(format!("sha256:{}", sha256_hex(b""))).as_deref()
        );
        let mut unhashed = files.clone();
        unhashed.get_mut("app.js").unwrap().sha256 = None;
        assert_eq!(content_manifest_sha256(&unhashed), None);
    }

    #[test]
    fn kind_names_match_spec() {
        // Spec §6.1–§6.8, in order.
        let spec = [
            "artifact.create",
            "version.publish",
            "artifact.update",
            "artifact.delete",
            "asset.upload",
            "asset.delete",
            "doc.write",
            "doc.move",
            "viewer.claim",
            "thread.open",
            "comment.add",
            "thread.resolve",
            "thread.reopen",
            "thread.delete",
            "thread.send",
            "feedback.delivered",
            "feedback.release",
            "thread.addressed",
            "live.page",
            "live.snapshot",
            "thread.move",
            "live.rule",
            "live.join",
            "live.split",
            "live.page_rekey",
            "live.page_merge",
            "live.join_answer",
            "watch.start",
            "watch.stop",
            "watch.update",
            "working.start",
            "working.stop",
            "question.ask",
            "question.answer",
            "question.decline",
            "question.release",
            "question.withdraw",
            "tool.call",
            "tool.call_id",
            "session.start",
            "session.join",
            "session.end",
            "backfill.skip",
        ];
        let names: Vec<&str> = AuditKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(names, spec);
        for k in AuditKind::ALL {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
            assert_eq!(AuditKind::parse(k.as_str()), Some(*k));
            assert_eq!(k.to_string(), k.as_str());
            let back: AuditKind = serde_json::from_value(json!(k.as_str())).unwrap();
            assert_eq!(back, *k);
        }
        assert_eq!(AuditKind::parse("tool.call_ID"), None);
    }

    #[test]
    fn actor_json_shapes() {
        let agent = Actor::Agent(AgentActor {
            session_id: Some("01JB8Q2WXYZ0000000000000AA".into()),
            harness: Some("claude".into()),
            harness_session_id: Some("3f2c".into()),
            agent_handle: Some("a_9f".into()),
            transcript_path: Some("/Users/alex/.claude/projects/p/3f2c.jsonl".into()),
        });
        let cases = [
            (
                agent,
                json!({"type":"agent","session_id":"01JB8Q2WXYZ0000000000000AA","harness":"claude",
                       "harness_session_id":"3f2c","agent_handle":"a_9f",
                       "transcript_path":"/Users/alex/.claude/projects/p/3f2c.jsonl"}),
            ),
            (
                Actor::Agent(AgentActor::default()),
                json!({"type":"agent","session_id":null}),
            ),
            (
                Actor::Owner {
                    public_id: "u_4be1".into(),
                },
                json!({"type":"owner","public_id":"u_4be1"}),
            ),
            (
                Actor::Viewer {
                    public_id: "u_77c0".into(),
                    display_name: Some("Sam".into()),
                },
                json!({"type":"viewer","public_id":"u_77c0","display_name":"Sam"}),
            ),
            (
                Actor::Viewer {
                    public_id: "u_77c0".into(),
                    display_name: None,
                },
                json!({"type":"viewer","public_id":"u_77c0"}),
            ),
            (Actor::Anonymous, json!({"type":"anonymous"})),
            (
                Actor::System {
                    reason: SystemReason::Ttl,
                },
                json!({"type":"system","reason":"ttl"}),
            ),
            (
                Actor::System {
                    reason: SystemReason::Rule,
                },
                json!({"type":"system","reason":"rule"}),
            ),
            (
                Actor::System {
                    reason: SystemReason::Backfill,
                },
                json!({"type":"system","reason":"backfill"}),
            ),
            (
                Actor::System {
                    reason: SystemReason::Daemon,
                },
                json!({"type":"system","reason":"daemon"}),
            ),
        ];
        for (actor, want) in cases {
            assert_eq!(serde_json::to_value(&actor).unwrap(), want);
            let back: Actor = serde_json::from_value(want).unwrap();
            assert_eq!(back, actor);
        }
    }

    #[test]
    fn via_names() {
        let names: Vec<&str> = Via::ALL.iter().map(|v| v.as_str()).collect();
        assert_eq!(
            names,
            [
                "mcp",
                "hook",
                "pi",
                "cli",
                "shell",
                "extension",
                "lan",
                "daemon"
            ]
        );
        for v in Via::ALL {
            assert_eq!(serde_json::to_value(v).unwrap(), json!(v.as_str()));
            assert_eq!(Via::parse(v.as_str()), Some(*v));
        }
        assert_eq!(Via::parse("MCP"), None);
        assert_eq!(Via::parse("ssh"), None);
        assert_eq!(Via::parse(""), None);
    }

    fn call() -> CallHeader {
        CallHeader {
            call_id: "01JBC0000000000000000000AB".into(),
            tool: "publish".into(),
            harness_tool: Some("mcp__plugin_clax_clax__publish".into()),
            args_sha256: format!("sha256:{}", "c6".repeat(32)),
            started_at: "2026-10-06T14:03:11.402Z".into(),
            harness_call_id: Some("toolu_01ABC".into()),
        }
    }

    #[test]
    fn call_header_roundtrip() {
        let c = call();
        let h = encode_call_header(&c).unwrap();
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(decode_call_header(&h), Ok(c));

        let bare = CallHeader {
            harness_tool: None,
            harness_call_id: None,
            ..call()
        };
        let h = encode_call_header(&bare).unwrap();
        assert_eq!(decode_call_header(&h), Ok(bare));
    }

    #[test]
    fn bad_call_headers_are_refused() {
        use base64::Engine as _;
        let enc = |v: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
        };
        // Refusals use fixed messages and never echo the input.
        let secret = "SECRET-VALUE";
        let mut echo = serde_json::to_value(call()).unwrap();
        echo["tool"] = json!(secret);
        echo["started_at"] = json!(secret);
        echo["call_id"] = json!(12);
        for bad in [
            enc(&echo),
            enc(&json!(secret)),
            enc(&json!({secret: 1})),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
            secret.to_string(),
        ] {
            let err = decode_call_header(&bad).unwrap_err();
            assert!(!err.contains(secret), "{err}");
            assert!(!err.contains("12"), "{err}");
        }
        assert!(decode_call_header("!!").is_err());
        assert!(decode_call_header("").is_err());
        let base = serde_json::to_value(call()).unwrap();
        let mut extra = base.clone();
        extra["args"] = json!({"body": "secret"});
        assert!(decode_call_header(&enc(&extra)).is_err());
        for (field, bad) in [
            ("call_id", json!("not-a-ulid")),
            ("tool", json!("")),
            ("tool", json!("Publish")),
            ("tool", json!("mcp__plugin_clax_clax__publish")),
            ("args_sha256", json!("c6".repeat(32))),
            ("args_sha256", json!("sha256:C6")),
            ("started_at", json!("now")),
            ("harness_tool", json!("")),
            ("harness_call_id", json!("a\u{7}b")),
        ] {
            let mut v = base.clone();
            v[field] = bad.clone();
            assert!(decode_call_header(&enc(&v)).is_err(), "{field}={bad}");
        }
    }

    #[test]
    fn oversized_call_header_is_refused() {
        let big = CallHeader {
            harness_call_id: Some("x".repeat(MAX_HEADER_BYTES)),
            ..call()
        };
        assert!(encode_call_header(&big).is_err());
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&big).unwrap());
        assert!(decode_call_header(&raw).is_err());
    }

    #[test]
    fn record_builder_and_system_ctx() {
        let r = AuditRecord::new(AuditKind::ThreadReopen, "2026-10-06T14:03:11.731Z")
            .with("moved", false);
        assert_eq!(r.ids, AuditIds::default());
        assert_eq!(r.body.get("moved"), Some(&json!(false)));
        let c = AuditCtx::system(SystemReason::Backfill);
        assert_eq!(c.via, Via::Daemon);
        assert_eq!(c.git, GitField::Absent);
        assert_eq!(c.call, None);
    }
}

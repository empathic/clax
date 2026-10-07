//! Audit events as Toolpath steps (spec 2026-10-06-toolpath-audit-design
//! §7.6, §10, §11): the one renderer the journal and every export share.
//!
//! [`render`] is a pure function of the stored row, the previous step's
//! sequence number, the [`RenderEnv`] and the [`Redaction`]: no clock
//! reads, no hash-map iteration, and nothing about the build doing the
//! rendering (the recording build is stored with each event), and
//! `serde_json`'s map keeps keys sorted, so the same inputs give the same
//! bytes. The journal renders with no browser base, so a line re-rendered
//! after a restart on another port or under a newer build is unchanged.
//!
//! A step's `change` maps the `clax://` URI of the object the event changed
//! to one `structural` perspective of type `clax.<kind>`, carrying the
//! event's body (§6) without its envelope. The envelope (`v`, `via`, `git`,
//! `git_capture`, `call`), the ID columns and the build go in
//! `meta.clax`. Relationships are `meta.refs` (§10.4). No step carries
//! `meta.source`: an agent's HEAD is where it stood, not what it changed,
//! so the commit is an `at-revision` ref.
//!
//! The URI forms of §10.2, plus, for objects that section does not name:
//!
//! ```text
//! clax://<install>/a/<artifact>/asset/<asset ID>     asset
//! clax://<install>/u/<public ID>                     viewer or owner identity
//! clax://<install>/rule/<rule ID>                    live-page rule
//! clax://<install>/site/<origin>                     joined site, by its key origin
//! clax://<install>/call/<call ID>                    tool call of the sessionless /mcp route
//! clax://<install>/step/<step ID>                    a recorded step (a tool call's `produced`)
//! clax://<install>/backfill/<table>/<row ID>         a history row the backfill skipped
//! ```
//!
//! Every segment is percent-encoded down to RFC 3986's unreserved
//! characters. A thread's URI nests under its artifact, so it changes when
//! the thread moves: a `thread.move` is keyed by the thread under its
//! target, with a `thread` ref to its old URI. Watch, working-record and
//! session events are keyed by the session, a batch `thread.send` by the
//! artifact, and `live.join`, `live.split` and `live.join_answer` by the
//! site's key origin (the asked origin, for an answer).

pub mod redact;

pub use redact::Redaction;

use crate::audit::{Actor, AgentActor, AuditKind};
use crate::store::audit::AuditRow;
use serde_json::{Map, Value, json};

/// The kind every Clax path declares in `meta.kind` (spec L8).
pub const KIND_URI: &str = "https://toolpath.net/kinds/clax-audit/v1.0.0";

use redact::ENVELOPE_FIELDS as ENVELOPE;

/// What a rendering needs beyond the row and the options: the install, and,
/// for an export only, where the browser reaches the daemon. Nothing about
/// the build doing the rendering: the build that recorded an event is
/// stored with it (spec §6), so a row renders to the same bytes under any
/// later build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderEnv {
    /// The install ID ([`Store::install_id`](crate::Store::install_id)).
    pub install: String,
    /// The browser base URL (`http://localhost:7480`) for `view` refs and
    /// `meta.clax.url`, in an export; `None` leaves them out, as the
    /// journal does, since the port can change between writes.
    pub view_base: Option<String>,
}

impl RenderEnv {
    /// The journal's environment for `install`: no browser URLs, so a line
    /// re-rendered after a restart on another port is byte-identical.
    pub fn journal(install: impl Into<String>) -> RenderEnv {
        RenderEnv {
            install: install.into(),
            view_base: None,
        }
    }

    /// An export's environment for `install`, with `view` refs under the
    /// browser base `view_base`.
    pub fn export(install: impl Into<String>, view_base: impl Into<String>) -> RenderEnv {
        RenderEnv {
            install: install.into(),
            view_base: Some(view_base.into()),
        }
    }
}

/// The build that recorded an event, from its stored envelope: its version
/// and commit, `unknown` when the row does not say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Build {
    version: String,
    commit: String,
}

impl Build {
    fn of(body: &Map<String, Value>) -> Build {
        let get = |k: &str| {
            body.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown")
                .to_string()
        };
        Build {
            version: get("clax_version"),
            commit: get("clax_commit"),
        }
    }
}

/// Why a row could not be rendered. The messages are fixed, and never
/// quote the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderError {
    /// The stored actor is not an actor object (spec §5.2).
    Actor,
    /// The stored body is not a JSON object.
    Body,
    /// The kind is not a dotted lowercase name.
    Kind,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RenderError::Actor => "the stored actor is not an actor object",
            RenderError::Body => "the stored body is not a JSON object",
            RenderError::Kind => "the stored kind is not a dotted lowercase name",
        })
    }
}

impl std::error::Error for RenderError {}

/// The step ID of the event numbered `seq`: `e` and twelve digits (spec L6).
pub fn step_id(seq: i64) -> String {
    format!("e{seq:012}")
}

/// The Toolpath provider ID of a Clax harness name (spec §9.4): `claude`
/// is `claude-code` and `gemini` is `gemini-cli`; every other harness
/// keeps its name.
pub fn provider_for(harness: &str) -> &str {
    match harness {
        "claude" => "claude-code",
        "gemini" => "gemini-cli",
        h => h,
    }
}

/// `s` with every character outside the actor pattern's `[A-Za-z0-9_.-]`
/// replaced by `-`; `-` for an empty string.
fn actor_segment(s: &str) -> String {
    if s.is_empty() {
        return "-".into();
    }
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The harness of an agent, or `unknown` when the history no longer
/// names it (or names it as an empty string).
fn harness_of(a: &AgentActor) -> &str {
    a.harness
        .as_deref()
        .filter(|h| !h.is_empty())
        .unwrap_or("unknown")
}

/// The harness session ID of an agent, when it has a non-empty one.
fn harness_session_of(a: &AgentActor) -> Option<&str> {
    a.harness_session_id.as_deref().filter(|s| !s.is_empty())
}

/// The actor string of `actor` (spec §10.1), matching the schema's actor
/// pattern; characters outside it become `-` (the `ActorDef` keeps the
/// original values). A system actor is `tool:clax/<clax_version>`, the
/// version of the build that recorded the event.
pub fn actor_string(actor: &Actor, clax_version: &str) -> String {
    match actor {
        Actor::Agent(a) => match (&a.session_id, harness_session_of(a)) {
            (None, _) => "agent:clax-mcp".into(),
            (Some(_), Some(hsid)) => format!(
                "agent:{}/{}",
                actor_segment(provider_for(harness_of(a))),
                actor_segment(hsid)
            ),
            (Some(sid), None) => format!(
                "agent:{}/clax-{}",
                actor_segment(provider_for(harness_of(a))),
                actor_segment(sid)
            ),
        },
        Actor::Owner { .. } => "human:clax-owner".into(),
        Actor::Viewer { public_id, .. } => {
            format!("human:clax-viewer/{}", actor_segment(public_id))
        }
        Actor::Anonymous => "human:clax-anonymous".into(),
        Actor::System { .. } => format!("tool:clax/{}", actor_segment(clax_version)),
    }
}

/// The display name of a harness's agent.
fn harness_name(harness: &str) -> &str {
    match harness {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "pi" => "Pi",
        "grok" => "Grok",
        "gemini" => "Gemini CLI",
        "unknown" => "Agent",
        h => h,
    }
}

/// The company behind a harness, where there is one.
fn company_for(harness: &str) -> Option<&'static str> {
    match harness {
        "claude" => Some("anthropic"),
        "codex" => Some("openai"),
        "grok" => Some("xai"),
        "gemini" => Some("google"),
        _ => None,
    }
}

/// The `ActorDef` definition of `actor` (spec §10.1), under `opts`: a
/// viewer's name goes under `--no-names`, and an agent's transcript
/// identity becomes `<provider>-transcript-sha256` (the path's hash) under
/// `--no-paths`. People's identities are scoped to `install`; Clax's own
/// names `clax_commit`, the build that recorded the event.
pub fn actor_def(actor: &Actor, install: &str, clax_commit: &str, opts: &Redaction) -> Value {
    let identity = |system: &str, id: &str| json!({"system": system, "id": id});
    match actor {
        Actor::Agent(a) if a.session_id.is_none() => json!({"name": "MCP client (no session)"}),
        Actor::Agent(a) => {
            let harness = harness_of(a);
            let provider = provider_for(harness);
            let mut ids = vec![identity(
                "clax-session",
                a.session_id.as_deref().unwrap_or_default(),
            )];
            if let Some(hsid) = harness_session_of(a) {
                ids.push(identity(&format!("{provider}-session"), hsid));
            }
            if let Some(handle) = &a.agent_handle {
                ids.push(identity("clax-agent", handle));
            }
            if let Some(t) = &a.transcript_path {
                if opts.no_paths {
                    ids.push(identity(
                        &format!("{provider}-transcript-sha256"),
                        &redact::redaction_hash(&Value::String(t.clone())),
                    ));
                } else {
                    ids.push(identity(&format!("{provider}-transcript"), t));
                }
            }
            let mut def = json!({"name": harness_name(harness), "identities": ids});
            if let Some(c) = company_for(harness) {
                def["provider"] = c.into();
            }
            def
        }
        Actor::Owner { public_id } => json!({
            "name": "Clax owner",
            "identities": [identity("clax", &format!("{install}/{public_id}"))],
        }),
        Actor::Viewer {
            public_id,
            display_name,
        } => {
            let mut def = json!({
                "identities": [identity("clax", &format!("{install}/{public_id}"))],
            });
            if let Some(n) = display_name.as_ref().filter(|_| !opts.no_names) {
                def["name"] = n.as_str().into();
            }
            def
        }
        Actor::Anonymous => json!({"name": "Anonymous viewer"}),
        Actor::System { .. } => json!({
            "name": "Clax",
            "identities": [identity("clax-build", clax_commit)],
        }),
    }
}

/// The actors `row`'s step names, with their `ActorDef` definitions: its
/// actor, then the actor its body names in `for_actor`, when that renders
/// to another string. Empty when the row does not render. [`render`]
/// gives the step and these together, parsing the row once.
pub fn actor_defs(row: &AuditRow, env: &RenderEnv, opts: &Redaction) -> Vec<(String, Value)> {
    render(row, None, env, opts)
        .map(|r| r.actors)
        .unwrap_or_default()
}

/// `old` and `new`, two definitions of one actor string, as one complete
/// definition: `new`'s fields, with the identities of both, de-duplicated
/// and sorted by `(system, id)`. One actor string can gather identities
/// over time (a harness session's later transcript, another Clax session
/// under the same harness session); the JSONL RFC overwrites an
/// `ActorDef`, so a writer re-emits the merged definition whenever it
/// grows (spec §7.2).
pub fn merge_actor_def(old: &Value, new: &Value) -> Value {
    let mut ids: Vec<(String, String)> = [old, new]
        .iter()
        .filter_map(|d| d.get("identities").and_then(Value::as_array))
        .flatten()
        .filter_map(|i| {
            Some((
                i.get("system")?.as_str()?.to_string(),
                i.get("id")?.as_str()?.to_string(),
            ))
        })
        .collect();
    ids.sort();
    ids.dedup();
    let mut out = new.clone();
    if let Some(m) = out.as_object_mut()
        && !ids.is_empty()
    {
        m.insert(
            "identities".into(),
            ids.into_iter()
                .map(|(system, id)| json!({"system": system, "id": id}))
                .collect(),
        );
    }
    out
}

/// A Clax object, for [`clax_uri`] (spec §10.2 and this module's
/// additions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Obj<'a> {
    Install,
    Artifact(&'a str),
    Version(&'a str, u64),
    Thread(&'a str, &'a str),
    Comment(&'a str, &'a str, &'a str),
    /// A document: artifact, collection path, document ID.
    Doc(&'a str, &'a str, &'a str),
    Asset(&'a str, &'a str),
    Session(&'a str),
    /// A tool call: its agent's session (none for the sessionless `/mcp`
    /// route) and the call ID.
    Call(Option<&'a str>, &'a str),
    Question(&'a str),
    /// A person by public ID.
    Person(&'a str),
    Rule(&'a str),
    /// A joined site, by the origin that keys it.
    Site(&'a str),
    /// A recorded step, by `seq`.
    Step(i64),
    /// A history row the backfill skipped: table, row ID.
    BackfillRow(&'a str, &'a str),
}

/// The opaque, install-scoped URI of `obj` (spec §10.2).
pub fn clax_uri(install: &str, obj: Obj<'_>) -> String {
    let base = format!("clax://{}", seg(install));
    match obj {
        Obj::Install => base,
        Obj::Artifact(a) => format!("{base}/a/{}", seg(a)),
        Obj::Version(a, n) => format!("{base}/a/{}/v/{n}", seg(a)),
        Obj::Thread(a, t) => format!("{base}/a/{}/t/{}", seg(a), seg(t)),
        Obj::Comment(a, t, c) => format!("{base}/a/{}/t/{}/c/{}", seg(a), seg(t), seg(c)),
        Obj::Doc(a, col, d) => {
            let col: Vec<String> = col.split('/').map(seg).collect();
            format!("{base}/a/{}/d/{}/{}", seg(a), col.join("/"), seg(d))
        }
        Obj::Asset(a, id) => format!("{base}/a/{}/asset/{}", seg(a), seg(id)),
        Obj::Session(s) => format!("{base}/s/{}", seg(s)),
        Obj::Call(Some(s), c) => format!("{base}/s/{}/call/{}", seg(s), seg(c)),
        Obj::Call(None, c) => format!("{base}/call/{}", seg(c)),
        Obj::Question(q) => format!("{base}/q/{}", seg(q)),
        Obj::Person(p) => format!("{base}/u/{}", seg(p)),
        Obj::Rule(r) => format!("{base}/rule/{}", seg(r)),
        Obj::Site(o) => format!("{base}/site/{}", seg(o)),
        Obj::Step(seq) => format!("{base}/step/{}", step_id(seq)),
        Obj::BackfillRow(t, r) => format!("{base}/backfill/{}/{}", seg(t), seg(r)),
    }
}

/// `s` percent-encoded as one URI path segment: everything but RFC 3986's
/// unreserved characters.
fn seg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A git remote URL in the short form Toolpath's git reader uses for a
/// repository's `base.uri`, for `at-revision` refs. Follows
/// `toolpath_git::normalize_git_url` (Toolpath commit `77dc16a5`): GitHub
/// and GitLab SSH and HTTPS remotes become `github:<owner>/<repo>` and
/// `gitlab:<owner>/<repo>` without `.git`; every other URL is unchanged.
pub fn normalize_remote(url: &str) -> String {
    for (prefix, host) in [
        ("git@github.com:", "github"),
        ("https://github.com/", "github"),
        ("git@gitlab.com:", "gitlab"),
        ("https://gitlab.com/", "gitlab"),
    ] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return format!("{host}:{}", rest.trim_end_matches(".git"));
        }
    }
    url.to_string()
}

/// A `file://` URL for the absolute path `p`, or `None` for a relative one.
fn file_url(p: &str) -> Option<String> {
    url::Url::from_file_path(p).ok().map(String::from)
}

/// The facts of one row that rendering reads, parsed once.
struct Event<'a> {
    row: &'a AuditRow,
    kind: Option<AuditKind>,
    env: &'a RenderEnv,
    actor: Actor,
    for_actor: Option<Actor>,
    /// The kind-specific body, redacted.
    body: Map<String, Value>,
    /// The envelope, redacted.
    envelope: Map<String, Value>,
}

impl Event<'_> {
    fn uri(&self, obj: Obj<'_>) -> String {
        clax_uri(&self.env.install, obj)
    }

    fn str(&self, field: &str) -> Option<&str> {
        self.body.get(field).and_then(Value::as_str)
    }

    fn u64(&self, field: &str) -> Option<u64> {
        self.body.get(field).and_then(Value::as_u64)
    }

    /// `field` for a sentence: a string as is, a number in decimal, else
    /// `unknown`.
    fn show(&self, field: &str) -> String {
        match self.body.get(field) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            _ => "unknown".into(),
        }
    }

    fn artifact(&self) -> Option<&str> {
        self.row.ids.artifact.as_deref()
    }

    fn thread(&self) -> Option<&str> {
        self.row.ids.thread.as_deref()
    }

    /// The agent the step concerns: its actor, or the agent a system or
    /// backfill actor names in `for_actor`.
    fn agent(&self) -> Option<&AgentActor> {
        match (&self.actor, &self.for_actor) {
            (Actor::Agent(a), _) => Some(a),
            (_, Some(Actor::Agent(a))) => Some(a),
            _ => None,
        }
    }

    /// The `agent-session` href of the step's agent: `agent://<provider>/<harness
    /// session ID>`, or its Clax session's URI when it has no harness ID.
    fn agent_session(&self) -> Option<String> {
        let agent = self.agent()?;
        match (harness_session_of(agent), agent.session_id.as_deref()) {
            (Some(hsid), _) => Some(format!(
                "agent://{}/{}",
                seg(provider_for(harness_of(agent))),
                seg(hsid)
            )),
            (None, Some(sid)) => Some(self.uri(Obj::Session(sid))),
            (None, None) => None,
        }
    }

    /// The Clax session of a tool call the step names, for its one URI:
    /// an agent actor's session (none for the sessionless `/mcp` route),
    /// else the row's session.
    fn call_session(&self) -> Option<&str> {
        match &self.actor {
            Actor::Agent(a) => a.session_id.as_deref(),
            _ => self.row.ids.session.as_deref(),
        }
    }

    /// `artifact <ID>`, or its title when the body still has it as text.
    fn artifact_name(&self) -> String {
        match self.str("title") {
            Some(t) => t.to_string(),
            None => format!("artifact {}", self.artifact().unwrap_or("unknown")),
        }
    }

    /// The change key: the URI of the object the event changed.
    fn key(&self) -> String {
        use AuditKind as K;
        let a = self.artifact();
        let t = self.thread();
        let s = self.row.ids.session.as_deref();
        let specific = match self.kind {
            Some(K::VersionPublish | K::LiveSnapshot) => {
                a.zip(self.u64("n")).map(|(a, n)| Obj::Version(a, n))
            }
            Some(K::AssetUpload | K::AssetDelete) => {
                a.zip(self.str("asset_id")).map(|(a, id)| Obj::Asset(a, id))
            }
            Some(K::DocWrite | K::DocMove) => match (a, self.str("collection"), self.str("doc_id"))
            {
                (Some(a), Some(c), Some(d)) => Some(Obj::Doc(a, c, d)),
                _ => None,
            },
            Some(K::ViewerClaim) => self.str("from_public_id").map(Obj::Person),
            Some(K::CommentAdd) => match (a, t, self.str("comment_id")) {
                (Some(a), Some(t), Some(c)) => Some(Obj::Comment(a, t, c)),
                _ => None,
            },
            Some(K::ThreadMove) => self
                .row
                .ids
                .artifact2
                .as_deref()
                .zip(t)
                .map(|(to, t)| Obj::Thread(to, t)),
            Some(K::LiveRule) => self.str("rule_id").map(Obj::Rule),
            Some(K::LiveJoin | K::LiveSplit | K::LiveJoinAnswer) => {
                self.row.ids.origin.as_deref().map(Obj::Site)
            }
            Some(
                K::WatchStart
                | K::WatchStop
                | K::WatchUpdate
                | K::WorkingStart
                | K::WorkingStop
                | K::SessionStart
                | K::SessionJoin
                | K::SessionEnd,
            ) => s.map(Obj::Session),
            Some(
                K::QuestionAsk
                | K::QuestionAnswer
                | K::QuestionDecline
                | K::QuestionRelease
                | K::QuestionWithdraw,
            ) => self.row.ids.question.as_deref().map(Obj::Question),
            Some(K::ToolCall | K::ToolCallId) => self
                .str("call_id")
                .map(|c| Obj::Call(self.call_session(), c)),
            Some(K::BackfillSkip) => self
                .str("table")
                .zip(self.str("row_id"))
                .map(|(t, r)| Obj::BackfillRow(t, r)),
            _ => None,
        };
        let generic = || match (a, t) {
            (Some(a), Some(t)) => Obj::Thread(a, t),
            (Some(a), None) => Obj::Artifact(a),
            _ => match (s, self.row.ids.question.as_deref()) {
                (_, Some(q)) => Obj::Question(q),
                (Some(s), None) => Obj::Session(s),
                (None, None) => Obj::Install,
            },
        };
        let obj = match self.kind {
            Some(K::ThreadSend) if t.is_none() => a.map(Obj::Artifact),
            Some(
                K::ArtifactCreate
                | K::ArtifactUpdate
                | K::ArtifactDelete
                | K::LivePage
                | K::LivePageRekey
                | K::LivePageMerge,
            ) => a.map(Obj::Artifact),
            _ => specific,
        };
        self.uri(obj.unwrap_or_else(generic))
    }

    /// The browser URL of the step's object, when the environment names a
    /// view base and the step is on an artifact: a moved thread's is its
    /// destination's, since a merge often merges its source page away.
    fn url(&self) -> Option<String> {
        let base = self.env.view_base.as_deref()?.trim_end_matches('/');
        let a = match self.kind {
            Some(AuditKind::ThreadMove) => self.row.ids.artifact2.as_deref(),
            _ => self.artifact(),
        }?;
        let n = match self.kind {
            Some(AuditKind::VersionPublish | AuditKind::LiveSnapshot) => self.u64("n"),
            _ => None,
        };
        Some(match n {
            Some(n) => format!("{base}/a/{}/v/{n}", seg(a)),
            None => format!("{base}/a/{}", seg(a)),
        })
    }

    /// The step's `meta.refs` (spec §10.4), in a fixed order, without
    /// duplicates and without a generic ref to the change key itself.
    fn refs(&self, key: &str, url: Option<&str>, opts: &Redaction) -> Vec<Value> {
        use AuditKind as K;
        let mut refs: Vec<(String, String)> = Vec::new();
        let mut push = |rel: &str, href: String| {
            if !refs.iter().any(|(r, h)| r == rel && *h == href) {
                refs.push((rel.to_string(), href));
            }
        };
        let a = self.artifact();
        let t = self.thread();
        let generic = |push: &mut dyn FnMut(&str, String), rel: &str, href: String| {
            if href != key {
                push(rel, href);
            }
        };

        // The objects the step concerns.
        if self.kind == Some(K::ThreadMove) {
            for (rel, field) in [
                ("moved-from", "from_artifact_id"),
                ("moved-to", "to_artifact_id"),
            ] {
                if let Some(x) = self.str(field) {
                    push(rel, self.uri(Obj::Artifact(x)));
                }
            }
            if let (Some(from), Some(t)) = (a, t) {
                generic(&mut push, "thread", self.uri(Obj::Thread(from, t)));
            }
        } else {
            if let Some(a) = a {
                generic(&mut push, "artifact", self.uri(Obj::Artifact(a)));
            }
            if let Some(a2) = self.row.ids.artifact2.as_deref() {
                generic(&mut push, "artifact", self.uri(Obj::Artifact(a2)));
            }
            if let (Some(a), Some(t)) = (a, t) {
                generic(&mut push, "thread", self.uri(Obj::Thread(a, t)));
            }
        }
        if let (Some(a), Some(Value::Array(ts))) = (a, self.body.get("thread_ids")) {
            for t in ts.iter().filter_map(Value::as_str) {
                generic(&mut push, "thread", self.uri(Obj::Thread(a, t)));
            }
        }
        let version_field = match self.kind {
            Some(K::ThreadOpen | K::ThreadAddressed) => Some("version_n"),
            Some(K::ThreadResolve) => Some("addressed_version"),
            _ => None,
        };
        if let (Some(a), Some(n)) = (a, version_field.and_then(|f| self.u64(f))) {
            push("version", self.uri(Obj::Version(a, n)));
        }
        if let (Some(src), Some(n)) = (
            self.body
                .get("source")
                .and_then(|s| s.get("artifact_id"))
                .and_then(Value::as_str),
            self.body
                .get("source")
                .and_then(|s| s.get("n"))
                .and_then(Value::as_u64),
        ) {
            push("copied-from", self.uri(Obj::Version(src, n)));
        }
        if let Some(q) = self.row.ids.question.as_deref() {
            generic(&mut push, "question", self.uri(Obj::Question(q)));
        }
        if let Some(s) = self.row.ids.session.as_deref() {
            let href = self.uri(Obj::Session(s));
            if self.agent_session().as_ref() != Some(&href) {
                generic(&mut push, "session", href);
            }
        }

        // What the step did to them.
        let thread_uri = a.zip(t).map(|(a, t)| self.uri(Obj::Thread(a, t)));
        match self.kind {
            Some(K::CommentAdd) => {
                if let Some(u) = &thread_uri {
                    push("replies-to", u.clone());
                }
            }
            Some(K::VersionPublish | K::LiveSnapshot) => {
                if let (Some(a), Some(Value::Array(ts))) = (a, self.body.get("addresses")) {
                    for t in ts.iter().filter_map(Value::as_str) {
                        push("addresses", self.uri(Obj::Thread(a, t)));
                    }
                }
            }
            Some(K::ThreadAddressed) => {
                if let Some(u) = &thread_uri {
                    push("addresses", u.clone());
                }
            }
            Some(K::ThreadResolve) => {
                if let Some(u) = &thread_uri {
                    if self.u64("addressed_version").is_some() {
                        push("addresses", u.clone());
                    }
                    push("resolves", u.clone());
                }
            }
            Some(K::QuestionAnswer) => {
                if let Some(q) = self.row.ids.question.as_deref() {
                    push("answers", self.uri(Obj::Question(q)));
                }
            }
            Some(K::ToolCall) => {
                if let Some(Value::Array(seqs)) = self.body.get("produced") {
                    for seq in seqs.iter().filter_map(Value::as_i64) {
                        push("produced", self.uri(Obj::Step(seq)));
                    }
                }
            }
            _ => {}
        }

        // Who acted, under which call, from which commit.
        if let Some(agent) = self.agent() {
            let provider = provider_for(harness_of(agent));
            if let Some(href) = self.agent_session() {
                push("agent-session", href);
            }
            if let (Some(hsid), Some(K::ToolCall | K::ToolCallId), Some(hc)) = (
                harness_session_of(agent),
                self.kind,
                self.str("harness_call_id"),
            ) {
                push(
                    "tool-use",
                    format!("agent://{}/{}/tool/{}", seg(provider), seg(hsid), seg(hc)),
                );
            }
            if !opts.no_paths
                && let Some(f) = agent.transcript_path.as_deref().and_then(file_url)
            {
                push("transcript", f);
            }
        }
        if let Some(call_id) = self
            .envelope
            .get("call")
            .and_then(|c| c.get("call_id"))
            .and_then(Value::as_str)
        {
            push(
                "tool-call",
                self.uri(Obj::Call(self.call_session(), call_id)),
            );
        }
        if let Some(rev) = self.at_revision(opts) {
            push("at-revision", rev);
        }
        if let Some(u) = url {
            push("view", u.to_string());
        }
        refs.into_iter()
            .map(|(rel, href)| json!({"rel": rel, "href": href}))
            .collect()
    }

    /// `git:<normalized remote>@<head>`, or `git:file://<repo root>@<head>`
    /// without a remote; none without a head, and none naming a local path
    /// under `--no-paths`.
    fn at_revision(&self, opts: &Redaction) -> Option<String> {
        let git = self.envelope.get("git")?;
        let head = git.get("head")?.as_str()?;
        let remote = git.get("remote_url").and_then(Value::as_str);
        match remote {
            Some(r) if !(opts.no_paths && redact::is_local_remote(r)) => {
                Some(format!("git:{}@{head}", normalize_remote(r)))
            }
            Some(_) => None,
            None if opts.no_paths => None,
            None => {
                let root = git.get("repo_root")?.as_str()?;
                Some(format!("git:{}@{head}", file_url(root)?))
            }
        }
    }

    /// One sentence saying what happened, from IDs and redacted fields
    /// only, so it holds nothing a redaction withholds.
    fn description(&self) -> String {
        use AuditKind as K;
        let art = || format!("artifact {}", self.artifact().unwrap_or("unknown"));
        let thread = || self.thread().unwrap_or("unknown").to_string();
        let page = || format!("{}{}", self.show("origin"), self.show("path"));
        let s = match self.kind {
            Some(K::ArtifactCreate) if self.str("kind") == Some("live") => {
                format!("Created live page artifact {}", self.artifact_name())
            }
            Some(K::ArtifactCreate) => format!("Created {}", self.artifact_name()),
            Some(K::VersionPublish) => format!(
                "Published version {} of {}",
                self.show("n"),
                self.artifact_name()
            ),
            Some(K::LiveSnapshot) => {
                let mut s = format!(
                    "Captured version {} of live page {}",
                    self.show("n"),
                    page()
                );
                if let Some(src) = self
                    .body
                    .get("source")
                    .and_then(|s| s.get("artifact_id"))
                    .and_then(Value::as_str)
                {
                    s.push_str(&format!(", copied from artifact {src}"));
                }
                s
            }
            Some(K::ArtifactUpdate) => {
                let fields: Vec<&str> = match self.body.get("fields") {
                    Some(Value::Object(f)) => f.keys().map(String::as_str).collect(),
                    _ => Vec::new(),
                };
                format!("Changed the {} of {}", and_list(&fields), art())
            }
            Some(K::ArtifactDelete) => format!("Deleted {}", self.artifact_name()),
            Some(K::AssetUpload) => {
                format!("Uploaded asset {} to {}", self.show("asset_id"), art())
            }
            Some(K::AssetDelete) => {
                format!("Deleted asset {} from {}", self.show("asset_id"), art())
            }
            Some(K::DocWrite) => {
                let verb = match self.str("op") {
                    Some("set") => "Set",
                    Some("update") => "Updated",
                    Some("delete") => "Deleted",
                    Some("str_replace") => "Edited",
                    Some("acquire") => "Leased",
                    _ => "Wrote",
                };
                format!(
                    "{verb} document {}/{} of {}",
                    self.show("collection"),
                    self.show("doc_id"),
                    art()
                )
            }
            Some(K::DocMove) => format!(
                "Moved private document {}/{} of {} to the owner",
                self.show("collection"),
                self.show("doc_id"),
                art()
            ),
            Some(K::ViewerClaim) => format!(
                "Merged person {} into {}",
                self.show("from_public_id"),
                self.show("to_public_id")
            ),
            Some(K::ThreadOpen) => format!(
                "Opened thread {} on version {} of {}",
                thread(),
                self.show("version_n"),
                art()
            ),
            Some(K::CommentAdd) => format!("Commented on thread {} of {}", thread(), art()),
            Some(K::ThreadResolve) => match self.u64("addressed_version") {
                Some(n) => format!("Resolved thread {} as addressed by version {n}", thread()),
                None => format!("Resolved thread {}", thread()),
            },
            Some(K::ThreadReopen) => format!("Reopened thread {}", thread()),
            Some(K::ThreadDelete) => format!("Deleted thread {}", thread()),
            Some(K::ThreadSend) => {
                let n = match self.body.get("thread_ids") {
                    Some(Value::Array(ts)) => ts.len(),
                    _ => 0,
                };
                let to = match self.body.get("target") {
                    Some(Value::String(w)) if w == "watchers" => "the watching agents".to_string(),
                    Some(Value::Object(t)) => format!(
                        "agent {}",
                        t.get("agent_handle")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    ),
                    _ => "an agent".to_string(),
                };
                format!(
                    "Sent {n} thread{} of {} to {to}",
                    if n == 1 { "" } else { "s" },
                    art()
                )
            }
            Some(K::FeedbackDelivered) => format!(
                "Delivered feedback {} on thread {} by {}",
                self.show("feedback_id"),
                thread(),
                self.show("tier")
            ),
            Some(K::FeedbackRelease) => format!(
                "Returned feedback {} on thread {} to undelivered ({})",
                self.show("feedback_id"),
                thread(),
                self.show("reason")
            ),
            Some(K::ThreadAddressed) => format!(
                "Linked thread {} to version {}",
                thread(),
                self.show("version_n")
            ),
            Some(K::LivePage) => format!("Created the live page for {}", page()),
            Some(K::ThreadMove) => format!(
                "Moved thread {} from artifact {} to artifact {} ({})",
                thread(),
                self.show("from_artifact_id"),
                self.show("to_artifact_id"),
                self.show("move_kind")
            ),
            Some(K::LiveRule) => match self.str("op") {
                Some("set") => format!(
                    "Set rule {} on {} for {}",
                    self.show("rule_id"),
                    self.show("origin"),
                    self.show("pattern")
                ),
                Some("delete") => format!("Took rule {} out of force", self.show("rule_id")),
                _ => format!("Dropped rule {}", self.show("rule_id")),
            },
            Some(K::LiveJoin) => match self.str("with") {
                Some(w) => format!("Joined {} to the site of {w}", self.show("origin")),
                None => format!(
                    "Joined {} to site {}",
                    self.show("origin"),
                    self.show("site")
                ),
            },
            Some(K::LiveSplit) => format!(
                "Split {} off site {}",
                self.show("origin"),
                self.show("before_site")
            ),
            Some(K::LivePageRekey) => format!(
                "Re-keyed live page {} from {} to {}",
                self.show("path"),
                self.show("from_origin"),
                self.show("to_origin")
            ),
            Some(K::LivePageMerge) => match self.str("merged_into") {
                Some(m) => format!("Merged live page {} into artifact {m}", page()),
                None => format!("Merged away live page {}", page()),
            },
            Some(K::LiveJoinAnswer) => format!(
                "Answered {} to joining {} with {}",
                self.show("answer"),
                self.show("origin"),
                self.show("with")
            ),
            Some(k @ (K::WatchStart | K::WatchStop | K::WatchUpdate)) => {
                let what = match self.str("target") {
                    Some("scope") => format!("the pages under {}", page()),
                    Some("page") => format!("live page {}", page()),
                    _ => art(),
                };
                match k {
                    K::WatchStart => format!("Started watching {what}"),
                    K::WatchStop => format!("Stopped watching {what}"),
                    _ => format!("Changed the watch on {what}"),
                }
            }
            Some(K::WorkingStart) => format!("Started working on {}", art()),
            Some(K::WorkingStop) => {
                format!("Stopped working on {} ({})", art(), self.show("reason"))
            }
            Some(
                k @ (K::QuestionAsk
                | K::QuestionAnswer
                | K::QuestionDecline
                | K::QuestionRelease
                | K::QuestionWithdraw),
            ) => {
                let verb = match k {
                    K::QuestionAsk => "Asked",
                    K::QuestionAnswer => "Answered",
                    K::QuestionDecline => "Declined",
                    K::QuestionRelease => "Released",
                    _ => "Withdrew",
                };
                format!(
                    "{verb} question {}",
                    self.row.ids.question.as_deref().unwrap_or("unknown")
                )
            }
            Some(K::ToolCall) => format!(
                "Called the {} tool, with outcome {}",
                self.show("tool"),
                self.show("outcome")
            ),
            Some(K::ToolCallId) => match self.str("call_id") {
                Some(c) => format!(
                    "Matched harness call {} to tool call {c}",
                    self.show("harness_call_id")
                ),
                None => format!(
                    "Found no tool call for harness call {}",
                    self.show("harness_call_id")
                ),
            },
            Some(k @ (K::SessionStart | K::SessionJoin)) => format!(
                "{} {} session {}",
                if k == K::SessionStart {
                    "Started"
                } else {
                    "Updated"
                },
                harness_name(self.str("harness").unwrap_or("unknown")),
                self.row.ids.session.as_deref().unwrap_or("unknown")
            ),
            Some(K::SessionEnd) => format!(
                "Ended session {} ({})",
                self.row.ids.session.as_deref().unwrap_or("unknown"),
                self.show("reason")
            ),
            Some(K::BackfillSkip) => {
                return format!(
                    "Skipped {} row {} while recording earlier history",
                    self.show("table"),
                    self.show("row_id")
                );
            }
            None => format!("Recorded {}", self.row.kind),
        };
        if self.row.backfilled {
            format!("{s}, recorded from earlier history")
        } else {
            s
        }
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => "nothing".into(),
        [one] => (*one).into(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Whether `kind` is a dotted lowercase name a structural type can carry.
fn is_kind_name(kind: &str) -> bool {
    !kind.is_empty()
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// A rendered row: its step, and the actors the step names with their
/// `ActorDef` definitions (its actor, then the one its body names in
/// `for_actor` when that renders to another string).
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub step: Value,
    pub actors: Vec<(String, Value)>,
}

/// Renders `row` as a Toolpath step (spec §10): ID `e<seq:012>`, the
/// previous step of its path (`prev`, a `seq`) as its only parent, the
/// actor string, the event's time, a `clax.<kind>` structural change keyed
/// by the changed object's `clax://` URI, and `meta` with a one-sentence
/// `description`, the §10.4 `refs`, and `clax`: the sequence number, kind,
/// install, `backfilled`, the envelope (`v`, `via`, the recording build's
/// `clax_version` and `clax_commit`, `git`, `git_capture`, `call`), the ID
/// columns, `system_reason` for a system actor, `for_actor` as an actor
/// string, and, in an export, the browser `url`. Never `meta.source`.
///
/// A pure function of the row, `prev`, `env` and `opts`: the same inputs
/// give the same bytes under any build. A kind this build does not know is
/// rendered from its columns and body alike, every body field unclassified
/// (see [`redact`]). Fails only when the row's actor, body or kind is
/// malformed; the caller then writes [`unrenderable_step`].
pub fn render(
    row: &AuditRow,
    prev: Option<i64>,
    env: &RenderEnv,
    opts: &Redaction,
) -> Result<Rendered, RenderError> {
    let actor: Actor = serde_json::from_str(&row.actor).map_err(|_| RenderError::Actor)?;
    let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(&row.body) else {
        return Err(RenderError::Body);
    };
    if !is_kind_name(&row.kind) {
        return Err(RenderError::Kind);
    }
    let build = Build::of(&body);
    let for_actor = body
        .get("for_actor")
        .and_then(|v| serde_json::from_value::<Actor>(v.clone()).ok());
    let mut envelope = Map::new();
    for k in ENVELOPE {
        if let Some(v) = body.remove(k) {
            envelope.insert(k.to_string(), v);
        }
    }
    opts.body(&row.kind, &mut body);
    opts.envelope(&mut envelope);
    let ev = Event {
        row,
        kind: AuditKind::parse(&row.kind),
        env,
        actor,
        for_actor,
        body,
        envelope,
    };

    let key = ev.key();
    let url = ev.url();
    let refs = ev.refs(&key, url.as_deref(), opts);
    let description = ev.description();

    let mut clax = Map::new();
    clax.insert("seq".into(), row.seq.into());
    clax.insert("kind".into(), row.kind.as_str().into());
    clax.insert("install".into(), env.install.as_str().into());
    clax.insert("backfilled".into(), row.backfilled.into());
    for (k, v) in &ev.envelope {
        clax.insert(k.clone(), v.clone());
    }
    clax.insert("clax_version".into(), build.version.as_str().into());
    clax.insert("clax_commit".into(), build.commit.as_str().into());
    let ids = &row.ids;
    for (k, v) in [
        ("artifact_id", &ids.artifact),
        ("artifact2_id", &ids.artifact2),
        ("thread_id", &ids.thread),
        ("session_id", &ids.session),
        ("question_id", &ids.question),
        ("call_id", &ids.call),
        ("origin", &ids.origin),
    ] {
        if let Some(v) = v {
            clax.insert(k.into(), v.as_str().into());
        }
    }
    if let Actor::System { reason } = &ev.actor {
        clax.insert(
            "system_reason".into(),
            serde_json::to_value(reason).expect("serialisable reason"),
        );
    }
    let actor_str = actor_string(&ev.actor, &build.version);
    let mut actors = vec![(
        actor_str.clone(),
        actor_def(&ev.actor, &env.install, &build.commit, opts),
    )];
    if let Some(who) = &ev.for_actor {
        let s = actor_string(who, &build.version);
        clax.insert("for_actor".into(), s.as_str().into());
        if s != actor_str {
            actors.push((s, actor_def(who, &env.install, &build.commit, opts)));
        }
    }
    if let Some(u) = &url {
        clax.insert("url".into(), u.as_str().into());
    }

    let mut structural = ev.body;
    structural.insert("type".into(), format!("clax.{}", row.kind).into());

    let mut step = json!({
        "id": step_id(row.seq),
        "actor": actor_str,
        "timestamp": row.at,
    });
    if let Some(p) = prev {
        step["parents"] = json!([step_id(p)]);
    }
    let mut meta = json!({"description": description, "clax": clax});
    if !refs.is_empty() {
        meta["refs"] = refs.into();
    }
    Ok(Rendered {
        step: json!({
            "step": step,
            "change": {key: {"structural": structural}},
            "meta": meta,
        }),
        actors,
    })
}

/// [`render`]'s step alone.
pub fn render_step(
    row: &AuditRow,
    prev: Option<i64>,
    env: &RenderEnv,
    opts: &Redaction,
) -> Result<Value, RenderError> {
    render(row, prev, env, opts).map(|r| r.step)
}

/// The step written in place of a row [`render`] refused (spec §7.3):
/// type `clax.unrenderable` on the install, carrying the row's `seq` and
/// kind (when it is a dotted name), made by Clax (`tool:clax/<version>` of
/// the recording build when the body still says, else `unknown`), so the
/// chain never breaks. Holds nothing else from the row.
pub fn unrenderable_step(row: &AuditRow, prev: Option<i64>, env: &RenderEnv) -> Value {
    let kind = if is_kind_name(&row.kind) {
        Value::from(row.kind.as_str())
    } else {
        Value::Null
    };
    let build = match serde_json::from_str::<Value>(&row.body) {
        Ok(Value::Object(b)) => Build::of(&b),
        _ => Build::of(&Map::new()),
    };
    let system = Actor::System {
        reason: crate::audit::SystemReason::Daemon,
    };
    let mut step = json!({
        "id": step_id(row.seq),
        "actor": actor_string(&system, &build.version),
        "timestamp": row.at,
    });
    if let Some(p) = prev {
        step["parents"] = json!([step_id(p)]);
    }
    json!({
        "step": step,
        "change": {
            clax_uri(&env.install, Obj::Install): {
                "structural": {"type": "clax.unrenderable", "seq": row.seq, "kind": kind}
            }
        },
        "meta": {
            "description": format!("Could not render event {}", step_id(row.seq)),
            "clax": {
                "seq": row.seq,
                "kind": kind,
                "install": env.install,
                "clax_version": build.version,
                "clax_commit": build.commit,
                "backfilled": row.backfilled,
                "unrenderable": true,
            },
        },
    })
}

#[cfg(test)]
#[path = "../../tests/toolpath/seal.rs"]
mod seal;

#[cfg(test)]
mod tests;

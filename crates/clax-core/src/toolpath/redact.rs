//! Export redaction (spec 2026-10-06-toolpath-audit-design §11): what
//! `--no-text`, `--no-names` and `--no-paths` replace when an audit event
//! is rendered, and with what.
//!
//! Redaction is deny by default. Every field of every kind's body, and of
//! the objects nested in it (an anchor, an `artifact.update`'s `fields`, an
//! actor in `for_actor`, the envelope's `git` and `call`), has a class:
//!
//! - **safe**: IDs, enums, counts, hashes, times, origins and URL paths
//!   (never a query or fragment); never replaced;
//! - **text**: comment bodies, version notes, labels and titles, artifact
//!   descriptions, question and answer text, working messages, an anchor's
//!   quoted page text, an artifact's declared `capabilities` (their
//!   configuration is open-ended), and free-form reasons (a backfill skip's,
//!   a failed queue claim's, a question's). `--no-text` replaces them with
//!   `{"redacted":"text","sha256":"sha256:<hex>"}`;
//! - **name**: display and author names. `--no-names` replaces them with
//!   `{"redacted":"name"}`; public IDs stay;
//! - **path**: local paths (`cwd`, `repo_root`, `transcript_path`, and a git
//!   remote that is a local path), and URLs that can carry a query or a
//!   fragment (an anchor's `route`, a moved thread's `from_url` and
//!   `to_url`), whose query can hold a token or a search. `--no-paths` replaces them with
//!   `{"redacted":"path","sha256":…}`; the renderer drops `file://` refs
//!   and hashes the transcript identity.
//!
//! A field with no class (a field this build does not know, or any field of
//! a kind it does not know) is replaced under any of the options with
//! `{"redacted":"unclassified","sha256":…}`, so a new field can never pass
//! a redaction unseen. A hash is unsalted, over a string's UTF-8 or else a
//! value's compact JSON. Argument hashes are never redacted: they are
//! already hashes, and they are the join key (§12). A `null` stays `null`.

use crate::audit::sha256_hex;
use serde_json::{Map, Value, json};

/// The redaction options a rendering applies. The default redacts nothing,
/// as the journal does unless `[toolpath] journal_text = false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Redaction {
    /// `--no-text`: hash free text.
    pub no_text: bool,
    /// `--no-names`: withhold display and author names.
    pub no_names: bool,
    /// `--no-paths`: hash local paths and drop `file://` refs.
    pub no_paths: bool,
}

/// The class of a body field (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Safe,
    Text,
    Name,
    Path,
    /// A git remote URL: a path when it names a local repository.
    Remote,
    /// An actor object (spec §5.2).
    Actor,
    /// A `thread.open` anchor.
    Anchor,
    /// An `artifact.update`'s changed fields.
    Fields,
    /// The envelope's git context.
    Git,
    /// The envelope's tool call.
    Call,
    Unclassified,
}

impl Redaction {
    /// Redacts nothing.
    pub const NONE: Redaction = Redaction {
        no_text: false,
        no_names: false,
        no_paths: false,
    };

    /// Every option.
    pub const ALL: Redaction = Redaction {
        no_text: true,
        no_names: true,
        no_paths: true,
    };

    /// Whether any option is on.
    pub fn any(&self) -> bool {
        self.no_text || self.no_names || self.no_paths
    }

    /// The options in force, by their CLI names (`no-text`, `no-names`,
    /// `no-paths`), as `graph.meta.clax.redaction` lists them.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.no_text, "no-text"),
            (self.no_names, "no-names"),
            (self.no_paths, "no-paths"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }

    /// Applies the options to the kind-specific fields of an event's body
    /// (the envelope already taken out), in place.
    pub(crate) fn body(&self, kind: &str, body: &mut Map<String, Value>) {
        for (field, v) in body.iter_mut() {
            self.apply(body_class(kind, field), v);
        }
    }

    /// Applies the options to the envelope (spec §6), in place.
    pub(crate) fn envelope(&self, envelope: &mut Map<String, Value>) {
        for (field, v) in envelope.iter_mut() {
            self.apply(envelope_class(field), v);
        }
    }

    fn apply(&self, class: Class, v: &mut Value) {
        if v.is_null() {
            return;
        }
        let nested = |v: &mut Value, of: fn(&str) -> Class| match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    self.apply(of(k), x);
                }
            }
            other => self.apply(Class::Unclassified, other),
        };
        match class {
            Class::Safe => {}
            Class::Text if self.no_text => *v = hashed("text", v),
            Class::Name if self.no_names => *v = json!({"redacted": "name"}),
            Class::Path if self.no_paths => *v = hashed("path", v),
            Class::Remote if self.no_paths && v.as_str().is_none_or(is_local_remote) => {
                *v = hashed("path", v)
            }
            Class::Text | Class::Name | Class::Path | Class::Remote => {}
            Class::Actor => nested(v, actor_class),
            Class::Anchor => nested(v, anchor_class),
            Class::Fields => nested(v, fields_class),
            Class::Git => nested(v, git_class),
            Class::Call => nested(v, call_class),
            Class::Unclassified if self.any() => *v = hashed("unclassified", v),
            Class::Unclassified => {}
        }
    }
}

/// The paths (`field` or `field/inner`) of the fields of a stored `kind`
/// body (envelope included) that have no class: under any option they are
/// hashed whole. Empty for a body this build fully knows.
pub fn unclassified(kind: &str, body: &Map<String, Value>) -> Vec<String> {
    fn walk(class: Class, at: &str, v: &Value, out: &mut Vec<String>) {
        let of: fn(&str) -> Class = match class {
            Class::Unclassified => return out.push(at.to_string()),
            Class::Actor => actor_class,
            Class::Anchor => anchor_class,
            Class::Fields => fields_class,
            Class::Git => git_class,
            Class::Call => call_class,
            _ => return,
        };
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    walk(of(k), &format!("{at}/{k}"), x, out);
                }
            }
            Value::Null => {}
            _ => out.push(at.to_string()),
        }
    }
    let mut out = Vec::new();
    for (k, v) in body {
        let class = if ENVELOPE_FIELDS.contains(&k.as_str()) {
            envelope_class(k)
        } else {
            body_class(kind, k)
        };
        walk(class, k, v, &mut out);
    }
    out
}

/// The envelope's fields (spec §6): rendered into `meta.clax`, not the
/// structural perspective.
pub(crate) const ENVELOPE_FIELDS: [&str; 7] = [
    "v",
    "via",
    "clax_version",
    "clax_commit",
    "git",
    "git_capture",
    "call",
];

fn envelope_class(field: &str) -> Class {
    match field {
        "v" | "via" | "clax_version" | "clax_commit" | "git_capture" => Class::Safe,
        "git" => Class::Git,
        "call" => Class::Call,
        _ => Class::Unclassified,
    }
}

/// The class of body field `field` of `kind`.
fn body_class(kind: &str, field: &str) -> Class {
    use Class::{Anchor, Fields, Name, Path, Safe, Text};
    match field {
        "for_actor" => return Class::Actor,
        "inferred" => return Safe,
        _ => {}
    }
    match (kind, field) {
        ("artifact.create", "title") => Text,
        ("artifact.create", "capabilities") => Text,
        ("artifact.create", "kind" | "icon" | "contract_version") => Safe,
        ("version.publish" | "live.snapshot", "note" | "label" | "title") => Text,
        (
            "version.publish" | "live.snapshot",
            "n" | "files" | "content_sha256" | "carried" | "addresses" | "by_page" | "origin"
            | "path" | "source" | "files_unreadable",
        ) => Safe,
        ("artifact.update", "fields") => Fields,
        ("artifact.delete", "title") => Text,
        ("artifact.delete", "current_version") => Safe,
        (
            "asset.upload" | "asset.delete",
            "asset_id" | "path" | "sha256" | "size" | "content_type" | "missing",
        ) => Safe,
        ("doc.write", "collection" | "doc_id" | "version" | "op" | "sha256") => Safe,
        ("doc.move", "from" | "to" | "collection" | "doc_id" | "version" | "sha256") => Safe,
        ("viewer.claim", "from_public_id" | "to_public_id") => Safe,
        ("thread.open", "anchor") => Anchor,
        ("thread.open", "version_n" | "live_path" | "has_clip" | "first_comment_id") => Safe,
        ("comment.add", "body") => Text,
        ("comment.add", "author_name") => Name,
        ("comment.add", "comment_id" | "author_kind" | "via_harness" | "via_page") => Safe,
        ("thread.resolve", "resolved_by" | "addressed_version") => Safe,
        ("thread.delete", "moved") => Safe,
        ("thread.send", "target" | "feedback_ids" | "batch_id" | "thread_ids") => Safe,
        ("feedback.delivered", "feedback_id" | "tier") => Safe,
        ("feedback.release", "feedback_id" | "tier") => Safe,
        ("feedback.release", "reason") => Text,
        ("thread.addressed", "version_n" | "source") => Safe,
        ("live.page", "origin" | "path") => Safe,
        (
            "thread.move",
            "from_artifact_id" | "to_artifact_id" | "move_kind" | "rule_id" | "move_id",
        ) => Safe,
        ("thread.move", "from_url" | "to_url") => Path,
        ("live.rule", "rule_id" | "op" | "origin" | "pattern" | "created_at" | "deleting") => Safe,
        ("live.join", "origin" | "with" | "site" | "joined" | "rules_moved" | "rules_dropped") => {
            Safe
        }
        ("live.split", "origin" | "before_site" | "site" | "never_with") => Safe,
        ("live.page_rekey", "from_origin" | "to_origin" | "path") => Safe,
        ("live.page_merge", "origin" | "path" | "merged_into") => Safe,
        ("live.join_answer", "origin" | "with" | "answer" | "until") => Safe,
        (
            "watch.start" | "watch.stop" | "watch.update",
            "target" | "replies_armed" | "source" | "origin" | "path" | "cause" | "move_id"
            | "fields",
        ) => Safe,
        ("working.start", "message") => Text,
        ("working.start", "key" | "thread_ids") => Safe,
        ("working.stop", "key" | "reason" | "duration_ms") => Safe,
        ("question.ask", "questions") => Text,
        ("question.ask", "source" | "tool_use_id") => Safe,
        ("question.answer", "answers") => Text,
        ("question.answer", "answered_via") => Safe,
        ("question.decline" | "question.release" | "question.withdraw", "reason") => Text,
        (
            "tool.call",
            "call_id" | "tool" | "harness_tool" | "args_sha256" | "started_at" | "ended_at"
            | "outcome" | "harness_call_id" | "produced",
        ) => Safe,
        ("tool.call_id", "call_id" | "harness_call_id" | "harness_tool" | "args_sha256") => Safe,
        ("session.start" | "session.join", "cwd" | "transcript_path") => Path,
        ("session.start" | "session.join", "harness" | "harness_session_id" | "pid") => Safe,
        ("session.end", "reason") => Safe,
        ("backfill.skip", "reason") => Text,
        ("backfill.skip", "table" | "row_id") => Safe,
        _ => Class::Unclassified,
    }
}

fn actor_class(field: &str) -> Class {
    match field {
        "type" | "session_id" | "harness" | "harness_session_id" | "agent_handle" | "public_id"
        | "reason" => Class::Safe,
        "display_name" => Class::Name,
        "transcript_path" => Class::Path,
        _ => Class::Unclassified,
    }
}

fn anchor_class(field: &str) -> Class {
    match field {
        "kind" | "selector" | "html_hash" | "file" => Class::Safe,
        "route" => Class::Path,
        "quote" | "prefix" | "suffix" => Class::Text,
        _ => Class::Unclassified,
    }
}

fn fields_class(field: &str) -> Class {
    match field {
        "icon" | "pinned" => Class::Safe,
        "title" | "description" | "capabilities" => Class::Text,
        _ => Class::Unclassified,
    }
}

fn git_class(field: &str) -> Class {
    match field {
        "repo_root" => Class::Path,
        "remote_url" => Class::Remote,
        "remote" | "branch" | "head" | "dirty" | "diff_sha256" | "diff_bytes"
        | "diff_truncated" | "untracked" | "captured_at" => Class::Safe,
        _ => Class::Unclassified,
    }
}

fn call_class(field: &str) -> Class {
    match field {
        "call_id" | "tool" | "harness_tool" | "args_sha256" | "started_at" | "harness_call_id" => {
            Class::Safe
        }
        _ => Class::Unclassified,
    }
}

/// `sha256:<hex>` of `v`: of a string's UTF-8, else of its compact JSON.
pub fn redaction_hash(v: &Value) -> String {
    let hex = match v {
        Value::String(s) => sha256_hex(s.as_bytes()),
        other => sha256_hex(other.to_string().as_bytes()),
    };
    format!("sha256:{hex}")
}

/// `{"redacted": what, "sha256": …}` for `v`.
fn hashed(what: &str, v: &Value) -> Value {
    json!({"redacted": what, "sha256": redaction_hash(v)})
}

/// Whether a git remote URL names a local repository rather than a host.
/// A remote is hosted when it has a scheme other than `file` (`https://`,
/// `ssh://`) or is in scp form, `[user@]host:path` with no `/` or `\`
/// before the `:` and a host longer than one letter (so `C:\repo` is
/// local); every other remote (`/srv/repo.git`, `../repo`, `repo-dir`,
/// `file:///x`) is local.
pub(crate) fn is_local_remote(url: &str) -> bool {
    if let Some((scheme, _)) = url.split_once("://") {
        return scheme.eq_ignore_ascii_case("file");
    }
    match url.split_once(':') {
        Some((before, _)) => {
            let host = before.rsplit('@').next().unwrap_or(before);
            before.contains('/')
                || before.contains('\\')
                || host.len() <= 1
                || before.eq_ignore_ascii_case("file")
        }
        None => true,
    }
}

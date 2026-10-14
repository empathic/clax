//! Cloudflare Artifacts push events: a `cf.artifacts.repo.pushed` event for
//! repository `a-<artifact ID>` asks that artifact to record the pushed
//! commits as versions. The event only says which repository to look at;
//! what becomes a version is read from the repository itself, so a
//! duplicate or replayed event changes nothing. Every other event is
//! acknowledged and ignored.

use std::collections::BTreeMap;

use serde_json::Value;

/// The event type of a push to an Artifacts repository.
pub const PUSHED_EVENT: &str = "cf.artifacts.repo.pushed";

/// The event's `type`, when it is a string.
pub fn event_type(body: &Value) -> Option<&str> {
    body.get("type")?.as_str()
}

/// The repository an Artifacts event names. Cloudflare's event schemas
/// document the repository in `source.repoName` (`source: {type:
/// "artifacts.repo", namespace, repoName}`) and the push in `payload`
/// (`ref`, `before`, `after`, `commits`, …). The name is read from the first
/// of `payload`, `source` and the body itself holding one of `repo_name`,
/// `repoName`, `repo` or `name`, as a string or as an object's `name`.
pub fn event_repo(body: &Value) -> Option<&str> {
    let b = body.as_object()?;
    for part in [
        b.get("payload").and_then(Value::as_object),
        b.get("source").and_then(Value::as_object),
        Some(b),
    ] {
        let Some(part) = part else { continue };
        for key in ["repo_name", "repoName", "repo", "name"] {
            match part.get(key) {
                Some(Value::String(s)) if !s.is_empty() => return Some(s),
                Some(Value::Object(o)) => {
                    if let Some(Value::String(s)) = o.get("name") {
                        return Some(s);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Whether `s` is a Clax artifact ID: 12 characters of lowercase Crockford
/// base32 (no i, l, o, u).
pub fn is_artifact_id(s: &str) -> bool {
    s.len() == 12
        && s.bytes()
            .all(|b| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&b))
}

/// The artifact whose repository a pushed event names, or `None` when the
/// event is not a push or names no repository of Clax's.
pub fn pushed_artifact(body: &Value) -> Option<&str> {
    if event_type(body) != Some(PUSHED_EVENT) {
        return None;
    }
    let id = event_repo(body)?.strip_prefix("a-")?;
    is_artifact_id(id).then_some(id)
}

/// How to handle one batch of event messages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchPlan {
    /// The messages to acknowledge at once (not pushes of Clax's).
    pub ack: Vec<usize>,
    /// Each artifact to reconcile once, with the messages that asked for it:
    /// acknowledge them when the reconcile succeeds or the artifact is gone,
    /// retry them otherwise.
    pub reconcile: BTreeMap<String, Vec<usize>>,
}

/// Plans a batch: each pushed event reconciles its artifact, at most once
/// per artifact per batch. Indexes are into `bodies`.
pub fn plan_batch<'a>(bodies: impl IntoIterator<Item = &'a Value>) -> BatchPlan {
    let mut plan = BatchPlan::default();
    for (i, body) in bodies.into_iter().enumerate() {
        match pushed_artifact(body) {
            Some(id) => plan.reconcile.entry(id.to_string()).or_default().push(i),
            None => plan.ack.push(i),
        }
    }
    plan
}

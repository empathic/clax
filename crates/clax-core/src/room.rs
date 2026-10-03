//! The grammar and bounds of the `room` capability (spec §9; contract
//! `room.d.ts`): peer labels, topic and room names, the size and depth of
//! relayed JSON, presence keys, and the topics an artifact declares in
//! `capabilities.room.topics`. Presence and message `data` are untrusted
//! data: they are checked for size and shape here and relayed as given.

use crate::db::Level;
use crate::{CoreError, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// The most named rooms one socket may be in at once.
pub const MAX_JOINED: usize = 16;
/// The most topics `capabilities.room.topics` may declare.
pub const MAX_TOPICS: usize = 16;
/// The most bytes of serialised JSON in a message's `data` or a merged presence.
pub const MAX_JSON_BYTES: usize = 4096;
/// The deepest nesting of arrays and objects in relayed JSON.
pub const MAX_JSON_DEPTH: usize = 8;
/// The most peers a `peers` snapshot lists (the receiver always among them).
pub const MAX_SNAPSHOT_PEERS: usize = 256;

/// Presence keys a page may not set: `prototype` and every name
/// `Object.prototype` carries, so a key never shadows an inherited member.
pub const RESERVED_PRESENCE_KEYS: &[&str] = &[
    "prototype",
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_argument", message)
}

/// A peer label: exactly 16 characters of `[0-9a-z]`.
pub fn peer_label_ok(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// `first` then at most 47 of `[a-z0-9_.-]`.
fn name_ok(s: &str, first: impl Fn(u8) -> bool) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && first(b[0])
        && b[1..].iter().all(|&c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-')
        })
}

/// A topic: `^[a-z][a-z0-9_.-]{0,47}$`.
pub fn topic_ok(s: &str) -> bool {
    name_ok(s, |c| c.is_ascii_lowercase())
}

/// A named room: `^[a-z0-9][a-z0-9_.-]{0,47}$`.
pub fn room_name_ok(s: &str) -> bool {
    name_ok(s, |c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// A presence key: `^[A-Za-z_][A-Za-z0-9_-]{0,63}$` and not in
/// [`RESERVED_PRESENCE_KEYS`].
pub fn presence_key_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        && !RESERVED_PRESENCE_KEYS.contains(&s)
}

/// The nesting depth of `v`: 0 for a scalar, 1 for a flat array or object.
fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Checks that `v` serialises to at most [`MAX_JSON_BYTES`] bytes and nests
/// at most [`MAX_JSON_DEPTH`] levels.
///
/// # Errors
/// `invalid_argument` naming `field`.
pub fn check_json(field: &str, v: &Value) -> Result<()> {
    let len = serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX);
    if len > MAX_JSON_BYTES {
        return Err(invalid(format!(
            "{field} is {len} bytes of JSON; the limit is {MAX_JSON_BYTES}"
        )));
    }
    if depth(v) > MAX_JSON_DEPTH {
        return Err(invalid(format!(
            "{field} nests deeper than {MAX_JSON_DEPTH} levels"
        )));
    }
    Ok(())
}

/// Checks a merged presence: an object whose keys pass [`presence_key_ok`],
/// within the bounds of [`check_json`].
///
/// # Errors
/// `invalid_argument` naming the first problem.
pub fn check_presence(state: &Value) -> Result<()> {
    let obj = state
        .as_object()
        .ok_or_else(|| invalid("presence is a JSON object"))?;
    if let Some(k) = obj.keys().find(|k| !presence_key_ok(k)) {
        return Err(invalid(format!(
            "'{k}' is not a presence key (^[A-Za-z_][A-Za-z0-9_-]{{0,63}}$, not a name Object.prototype carries)"
        )));
    }
    check_json("presence", state)
}

/// The topics an artifact declares in `capabilities.room.topics`, each
/// `"interact"` or `"admin"`: the lowest level that may send on it. An
/// undeclared topic needs `admin`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Topics(BTreeMap<String, Level>);

impl Topics {
    /// Reads `capabilities.room.topics`; absent is no declared topics.
    ///
    /// # Errors
    /// `invalid_capabilities` naming the first problem: `topics` is not an
    /// object, declares more than [`MAX_TOPICS`] topics, a name fails
    /// [`topic_ok`], or a value is not `"interact"` or `"admin"`.
    pub fn from_capabilities(caps: &Value) -> Result<Topics> {
        let Some(topics) = caps.get("room").and_then(|r| r.get("topics")) else {
            return Ok(Topics::default());
        };
        let bad = |m: String| CoreError::invalid("invalid_capabilities", m);
        let obj = topics
            .as_object()
            .ok_or_else(|| bad("capabilities.room.topics must be an object".into()))?;
        if obj.len() > MAX_TOPICS {
            return Err(bad(format!(
                "capabilities.room.topics declares {} topics; the limit is {MAX_TOPICS}",
                obj.len()
            )));
        }
        let mut out = BTreeMap::new();
        for (name, level) in obj {
            if !topic_ok(name) {
                return Err(bad(format!(
                    "capabilities.room.topics: '{name}' is not a topic (^[a-z][a-z0-9_.-]{{0,47}}$)"
                )));
            }
            let level = match level.as_str() {
                Some("interact") => Level::Interact,
                Some("admin") => Level::Admin,
                _ => {
                    return Err(bad(format!(
                        "capabilities.room.topics.{name} is \"interact\" or \"admin\""
                    )));
                }
            };
            out.insert(name.clone(), level);
        }
        Ok(Topics(out))
    }

    /// The lowest level that may send on `topic`.
    pub fn send_level(&self, topic: &str) -> Level {
        self.0.get(topic).copied().unwrap_or(Level::Admin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn peer_labels_are_16_of_digits_and_lowercase() {
        assert!(peer_label_ok("k3v6q2rt7wacd4fn"));
        for bad in [
            "short",
            "K3V6Q2RT7WACD4FN",
            "k3v6q2rt7wacd4fn0",
            "k3v6q2rt7wacd4f-",
            "",
        ] {
            assert!(!peer_label_ok(bad), "{bad}");
        }
    }

    #[test]
    fn topic_and_room_grammar() {
        assert!(topic_ok("reaction"));
        assert!(topic_ok("a.b_c-9"));
        assert!(topic_ok(&format!("a{}", "b".repeat(47))));
        assert!(!topic_ok(&format!("a{}", "b".repeat(48))));
        for bad in ["", "9lives", "Bad", "bad:topic", "_x"] {
            assert!(!topic_ok(bad), "{bad}");
        }
        assert!(room_name_ok("table-1"));
        assert!(room_name_ok("9lives"));
        for bad in ["", "Bad", "-x", "a b"] {
            assert!(!room_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn presence_keys_refuse_prototype_names() {
        assert!(check_presence(&json!({"cursor": [1, 2], "_x": 1, "a-b": null})).is_ok());
        for k in RESERVED_PRESENCE_KEYS {
            assert!(check_presence(&json!({ *k: 1 })).is_err(), "{k}");
        }
        for bad in [json!([]), json!("x"), json!({"9a": 1}), json!({"a b": 1})] {
            assert!(check_presence(&bad).is_err(), "{bad}");
        }
        assert!(check_presence(&json!({ "a".repeat(64): 1 })).is_ok());
        assert!(check_presence(&json!({ "a".repeat(65): 1 })).is_err());
    }

    #[test]
    fn json_bounds_are_4096_bytes_and_8_levels() {
        assert!(check_json("data", &json!("x".repeat(4094))).is_ok());
        assert!(check_json("data", &json!("x".repeat(4095))).is_err());
        let mut v = json!(1);
        for _ in 0..8 {
            v = json!([v]);
        }
        assert!(check_json("data", &v).is_ok());
        assert!(check_json("data", &json!([v])).is_err());
    }

    #[test]
    fn declared_topics_set_the_send_level() {
        let t = Topics::from_capabilities(
            &json!({"room": {"topics": {"reaction": "interact", "clear": "admin"}}}),
        )
        .unwrap();
        assert_eq!(t.send_level("reaction"), Level::Interact);
        assert_eq!(t.send_level("clear"), Level::Admin);
        assert_eq!(t.send_level("other"), Level::Admin);
        assert_eq!(
            Topics::from_capabilities(&json!({"room": {}})).unwrap(),
            Topics::default()
        );
    }

    #[test]
    fn malformed_topic_declarations_are_refused() {
        let many: serde_json::Map<String, Value> = (0..17)
            .map(|i| (format!("t{i}"), json!("interact")))
            .collect();
        for bad in [
            json!({"room": {"topics": []}}),
            json!({"room": {"topics": {"Bad": "interact"}}}),
            json!({"room": {"topics": {"ok": "view"}}}),
            json!({"room": {"topics": {"ok": "owner"}}}),
            json!({"room": {"topics": many}}),
        ] {
            assert!(Topics::from_capabilities(&bad).is_err(), "{bad}");
        }
    }
}

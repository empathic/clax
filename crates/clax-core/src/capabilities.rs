//! Validation of the declared `capabilities` object (spec §6 publish body,
//! §9). The declaration is a full set: the store replaces the stored object
//! with it; omitting it keeps the stored one and `{}` clears it.

use crate::db::{ReadDecl, Rules};
use crate::{CoreError, Result};
use serde_json::Value;

fn bad(message: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_capabilities", message)
}

/// Accepts a JSON object whose every value is an object. `db.rules` must pass
/// [`Rules::from_capabilities`]; `db.read` and `db.share` must pass
/// [`ReadDecl::check`] (the check that a list does not name its own artifact
/// is made where the artifact's ID is known); `comments.composer_only` and
/// `comments.customAnchors` are booleans; `user.scopes` is an array of
/// `"profile"` and `"email"`; `room.topics` must pass
/// [`Topics::from_capabilities`](crate::room::Topics::from_capabilities); `net.hosts` must pass
/// [`net_hosts`]; `shell` takes no options (`{}`). Other names are stored as
/// given.
///
/// # Errors
/// `invalid_capabilities` naming the first problem.
pub fn validate(caps: &Value) -> Result<()> {
    let obj = caps
        .as_object()
        .ok_or_else(|| bad("capabilities must be a JSON object"))?;
    for (name, cfg) in obj {
        if !cfg.is_object() {
            return Err(bad(format!(
                "capabilities.{name} must be an object ({{}} for defaults)"
            )));
        }
    }
    Rules::from_capabilities(caps)?;
    ReadDecl::check(caps)?;
    crate::room::Topics::from_capabilities(caps)?;
    net_hosts(caps)?;
    if obj
        .get("shell")
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty())
    {
        return Err(bad("capabilities.shell takes no options: declare it as {}"));
    }
    if let Some(c) = obj.get("comments") {
        for k in ["composer_only", "customAnchors"] {
            if c.get(k).is_some_and(|v| !v.is_boolean()) {
                return Err(bad(format!(
                    "capabilities.comments.{k} must be true or false"
                )));
            }
        }
    }
    if let Some(scopes) = obj.get("user").and_then(|u| u.get("scopes")) {
        let ok = scopes.as_array().is_some_and(|a| {
            a.iter()
                .all(|s| matches!(s.as_str(), Some("profile" | "email")))
        });
        if !ok {
            return Err(bad(
                "capabilities.user.scopes is an array of \"profile\" and \"email\"",
            ));
        }
    }
    Ok(())
}

/// Most hosts one `net` declaration may list.
pub const MAX_NET_HOSTS: usize = 32;

/// Whether `h` is a lowercase DNS name with at least two labels whose last
/// label is not numeric (so never an IP address), without a port, a
/// wildcard or a trailing dot.
fn is_host_name(h: &str) -> bool {
    let labels: Vec<&str> = h.split('.').collect();
    h.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && !labels[labels.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

/// The hosts `capabilities.net.hosts` declares, in declaration order: `None`
/// when `net` is not declared. A declared `net` lists 1 to
/// [`MAX_NET_HOSTS`] distinct lowercase host names (`api.example.com`; no
/// scheme, port, path, wildcard or IP address).
///
/// # Errors
/// `invalid_capabilities` naming the problem.
pub fn net_hosts(caps: &Value) -> Result<Option<Vec<String>>> {
    let Some(net) = caps.get("net") else {
        return Ok(None);
    };
    let hosts = net
        .get("hosts")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("capabilities.net.hosts is an array of host names"))?;
    if hosts.is_empty() || hosts.len() > MAX_NET_HOSTS {
        return Err(bad(format!(
            "capabilities.net.hosts lists 1 to {MAX_NET_HOSTS} hosts"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(hosts.len());
    for h in hosts {
        let h = h
            .as_str()
            .filter(|h| is_host_name(h))
            .ok_or_else(|| {
                bad(format!(
                    "capabilities.net.hosts: {h} is not a lowercase host name such as \"api.example.com\""
                ))
            })?;
        if !out.iter().any(|o| o == h) {
            out.push(h.to_string());
        }
    }
    Ok(Some(out))
}

/// The capabilities that reach past the page, and so need the owner's
/// approval before a version's declaration of them takes effect. The rest
/// (`db`, `comments`, `user`, `room`, `artifact/self`, `assets`,
/// `downloads`, `home`) act only inside the artifact and its own account's
/// view of it.
pub const GATED: &[&str] = &[
    "net", "sample", "account", "shell", "notify", "share", "file", "camera", "location",
];

/// What one declaration adds over another: new top-level capability names
/// and new `net` hosts.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Added {
    /// New top-level capability names, in the order `next` lists them (`net`
    /// among them only when the earlier declaration had none).
    pub capabilities: Vec<String>,
    /// `net` hosts the earlier declaration did not list.
    pub hosts: Vec<String>,
}

impl Added {
    /// Whether nothing is added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty() && self.hosts.is_empty()
    }
}

/// The string entries of `caps.net.hosts`; anything else is skipped, so a
/// malformed declaration yields no hosts rather than an error.
fn hosts_of(caps: &Value) -> Vec<&str> {
    caps.get("net")
        .and_then(|n| n.get("hosts"))
        .and_then(Value::as_array)
        .map(|h| h.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// What `next` declares beyond `live`: the Clax app's `addedCapabilities`
/// (`apps/clax_app/lib/bundle/updater.dart`).
#[must_use]
pub fn added(live: &Value, next: &Value) -> Added {
    let had = hosts_of(live);
    Added {
        capabilities: next
            .as_object()
            .into_iter()
            .flat_map(|m| m.keys())
            .filter(|k| live.get(k.as_str()).is_none())
            .cloned()
            .collect(),
        hosts: hosts_of(next)
            .into_iter()
            .filter(|h| !had.contains(h))
            .map(str::to_string)
            .collect(),
    }
}

/// What `next` declares that needs approval beyond what `approved` covers:
/// [`added`] over the approved declaration (`{}` when none is on record),
/// with `capabilities` kept to the [`GATED`] names.
#[must_use]
pub fn pending(approved: Option<&Value>, next: &Value) -> Added {
    let empty = Value::Object(serde_json::Map::new());
    let mut out = added(approved.unwrap_or(&empty), next);
    out.capabilities.retain(|c| GATED.contains(&c.as_str()));
    out
}

/// The declaration of `next` that is in force given what `approved`
/// covers: `next` without the pending gated names, `net.hosts` narrowed to
/// the approved hosts, and `net` dropped when none remain. A `next` that
/// is not an object has nothing in force (`{}`).
#[must_use]
pub fn in_force(approved: Option<&Value>, next: &Value) -> Value {
    let empty = Value::Object(serde_json::Map::new());
    let approved = approved.unwrap_or(&empty);
    let mut out = next.clone();
    let Some(map) = out.as_object_mut() else {
        return empty;
    };
    for name in pending(Some(approved), next).capabilities {
        map.remove(&name);
    }
    if let Some(net) = map.get_mut("net") {
        let ok = hosts_of(approved);
        let kept: Vec<Value> = hosts_of(next)
            .into_iter()
            .filter(|h| ok.contains(h))
            .map(|h| Value::String(h.to_string()))
            .collect();
        if kept.is_empty() {
            map.remove("net");
        } else if let Some(o) = net.as_object_mut() {
            o.insert("hosts".into(), Value::Array(kept));
        }
    }
    out
}

/// `approved` cut to what `next` still declares: each [`GATED`] name `next`
/// lacks is removed and `net.hosts` is narrowed to the hosts `next` lists
/// (`net` removed when none remain); other names are kept. A `next` that is
/// not an object declares nothing. A name or host a version drops is so no
/// longer approved, and asks again when a later version declares it.
#[must_use]
pub fn narrow(approved: &Value, next: &Value) -> Value {
    let mut out = approved.clone();
    let Some(map) = out.as_object_mut() else {
        return out;
    };
    let declares = |k: &str| next.as_object().is_some_and(|m| m.contains_key(k));
    map.retain(|k, _| !GATED.contains(&k.as_str()) || declares(k));
    if let Some(net) = map.get_mut("net") {
        let still = hosts_of(next);
        let kept: Vec<Value> = hosts_of(approved)
            .into_iter()
            .filter(|h| still.contains(h))
            .map(|h| Value::String(h.to_string()))
            .collect();
        if kept.is_empty() {
            map.remove("net");
        } else if let Some(o) = net.as_object_mut() {
            o.insert("hosts".into(), Value::Array(kept));
        }
    }
    out
}

/// The lowercase hex SHA-256 of `caps` in its JCS canonical form
/// ([`crate::audit::args::canonical`]): what an approval names to say
/// which declaration was shown.
#[must_use]
pub fn declaration_sha256(caps: &Value) -> String {
    crate::audit::sha256_hex(crate::audit::args::canonical(caps).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{added, declaration_sha256, in_force, narrow, net_hosts, pending, validate};
    use serde_json::json;

    #[test]
    fn accepts_the_contract_declarations() {
        for ok in [
            json!({}),
            json!({"db": {}, "user": {"scopes": ["profile", "email"]}, "artifact": {}}),
            json!({"comments": {"composer_only": true, "customAnchors": true}}),
            json!({"self": {}, "downloads": {}, "assets": {}, "room": {"topics": {"chat": "interact"}}}),
            json!({"db": {"read": ["7q3k9mzx2b4t"], "share": {"read": ["8r4m0nay3c5v"]}}}),
            json!({"net": {"hosts": ["api.example.com", "x-1.example.co.uk"]}}),
            json!({"shell": {}}),
        ] {
            validate(&ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
    }

    #[test]
    fn shell_takes_no_options() {
        let e = validate(&json!({"shell": {"controls": ["fork"]}})).unwrap_err();
        assert!(
            matches!(
                &e,
                crate::CoreError::Invalid {
                    code: "invalid_capabilities",
                    message,
                } if message.contains("capabilities.shell takes no options")
            ),
            "{e:?}"
        );
    }

    #[test]
    fn refuses_malformed_declarations() {
        for bad in [
            json!([]),
            json!({"db": true}),
            json!({"comments": {"composer_only": "yes"}}),
            json!({"user": {"scopes": ["profile", "phone"]}}),
            json!({"user": {"scopes": "profile"}}),
            json!({"db": {"rules": [{"path": "x", "write": "view"}]}}),
            json!({"room": {"topics": {"Bad:topic": "interact"}}}),
            json!({"room": {"topics": {"chat": "view"}}}),
            json!({"room": {"topics": ["chat"]}}),
            json!({"db": {"read": ["not an id"]}}),
            json!({"db": {"share": {"write": ["7q3k9mzx2b4t"]}}}),
            json!({"net": {}}),
            json!({"net": {"hosts": []}}),
            json!({"net": {"hosts": "api.example.com"}}),
            json!({"net": {"hosts": ["https://api.example.com"]}}),
            json!({"net": {"hosts": ["api.example.com:8443"]}}),
            json!({"net": {"hosts": ["*.example.com"]}}),
            json!({"net": {"hosts": ["API.example.com"]}}),
            json!({"net": {"hosts": ["localhost"]}}),
            json!({"net": {"hosts": ["10.0.0.1"]}}),
            json!({"net": {"hosts": ["[::1]"]}}),
            json!({"net": {"hosts": ["example.com."]}}),
            json!({"net": {"hosts": [7]}}),
        ] {
            let e = validate(&bad).unwrap_err();
            assert!(
                matches!(
                    e,
                    crate::CoreError::Invalid {
                        code: "invalid_capabilities",
                        ..
                    }
                ),
                "{bad}: {e:?}"
            );
        }
    }

    #[test]
    fn net_hosts_reads_the_declaration_once_per_host() {
        assert_eq!(net_hosts(&json!({"db": {}})).unwrap(), None);
        assert_eq!(
            net_hosts(
                &json!({"net": {"hosts": ["b.example.com", "a.example.com", "b.example.com"]}})
            )
            .unwrap(),
            Some(vec![
                "b.example.com".to_string(),
                "a.example.com".to_string()
            ])
        );
        let many: Vec<String> = (0..=super::MAX_NET_HOSTS)
            .map(|i| format!("h{i}.example.com"))
            .collect();
        assert!(net_hosts(&json!({"net": {"hosts": many}})).is_err());
    }

    #[test]
    fn added_is_the_clax_apps_rule() {
        // apps/clax_app/test/bundle/updater_test.dart 'addedCapabilities lists new capabilities and hosts'
        let d = added(
            &json!({"db": {}, "net": {"hosts": ["a.com"]}}),
            &json!({"db": {}, "notify": {}, "net": {"hosts": ["a.com", "b.com"]}}),
        );
        assert_eq!(d.capabilities, vec!["notify"]);
        assert_eq!(d.hosts, vec!["b.com"]);
        let first = added(&json!({}), &json!({"net": {"hosts": ["a.com"]}}));
        assert_eq!(
            (first.capabilities, first.hosts),
            (vec!["net".to_string()], vec!["a.com".to_string()])
        );
    }

    #[test]
    fn pending_keeps_what_reaches_past_the_page() {
        let p = pending(
            None,
            &json!({"db": {}, "comments": {}, "user": {}, "sample": {}, "shell": {}}),
        );
        assert_eq!(p.capabilities, vec!["sample", "shell"]);
        assert!(
            pending(
                Some(&json!({"sample": {}})),
                &json!({"sample": {}, "db": {}})
            )
            .is_empty()
        );
    }

    #[test]
    fn in_force_is_the_approved_declaration_on_gated_names() {
        let approved = json!({"net": {"hosts": ["a.com"]}});
        let next = json!({"db": {}, "sample": {}, "net": {"hosts": ["a.com", "b.com"]}});
        assert_eq!(
            in_force(Some(&approved), &next),
            json!({"db": {}, "net": {"hosts": ["a.com"]}})
        );
        assert_eq!(
            in_force(None, &json!({"net": {"hosts": ["a.com"]}, "db": {}})),
            json!({"db": {}})
        );
        // A host the next version drops is not in force, whatever was approved.
        assert_eq!(
            in_force(
                Some(&json!({"net": {"hosts": ["a.com", "b.com"]}})),
                &json!({"net": {"hosts": ["b.com"]}})
            ),
            json!({"net": {"hosts": ["b.com"]}})
        );
    }

    #[test]
    fn narrow_keeps_what_the_next_declaration_still_declares() {
        let approved =
            json!({"db": {}, "sample": {}, "shell": {}, "net": {"hosts": ["a.com", "b.com"]}});
        assert_eq!(
            narrow(
                &approved,
                &json!({"sample": {}, "net": {"hosts": ["b.com", "c.com"]}})
            ),
            json!({"db": {}, "sample": {}, "net": {"hosts": ["b.com"]}})
        );
        assert_eq!(
            narrow(&approved, &json!({"net": {"hosts": ["c.com"]}})),
            json!({"db": {}})
        );
        assert_eq!(narrow(&approved, &json!(null)), json!({"db": {}}));
        assert_eq!(narrow(&approved, &approved), approved);
    }

    #[test]
    fn the_declaration_hash_ignores_key_order() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"net":{"hosts":["a.com"]},"db":{}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"db":{},"net":{"hosts":["a.com"]}}"#).unwrap();
        assert_eq!(declaration_sha256(&a), declaration_sha256(&b));
        assert_eq!(declaration_sha256(&a).len(), 64);
        assert_ne!(declaration_sha256(&a), declaration_sha256(&json!({})));
    }

    #[test]
    fn in_force_of_a_declaration_that_is_not_an_object_is_empty() {
        for next in [json!(null), json!([]), json!("net"), json!(7)] {
            assert_eq!(
                in_force(Some(&json!({"net": {}})), &next),
                json!({}),
                "{next}"
            );
        }
    }
}

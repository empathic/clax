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

#[cfg(test)]
mod tests {
    use super::{net_hosts, validate};
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
}

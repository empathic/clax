//! Validation of the declared `capabilities` object (spec §6 publish body,
//! §9). The declaration is a full set: the store replaces the stored object
//! with it; omitting it keeps the stored one and `{}` clears it.

use crate::db::Rules;
use crate::{CoreError, Result};
use serde_json::Value;

fn bad(message: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_capabilities", message)
}

/// Accepts a JSON object whose every value is an object. `db.rules` must pass
/// [`Rules::from_capabilities`]; `comments.composer_only` and
/// `comments.customAnchors` are booleans; `user.scopes` is an array of
/// `"profile"` and `"email"`. Other names are stored as given.
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

#[cfg(test)]
mod tests {
    use super::validate;
    use serde_json::json;

    #[test]
    fn accepts_the_contract_declarations() {
        for ok in [
            json!({}),
            json!({"db": {}, "user": {"scopes": ["profile", "email"]}, "artifact": {}}),
            json!({"comments": {"composer_only": true, "customAnchors": true}}),
            json!({"self": {}, "downloads": {}, "assets": {}, "room": {"topics": {"chat": "interact"}}}),
        ] {
            validate(&ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
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
}

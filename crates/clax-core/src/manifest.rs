//! An artifact's manifest: the `clax.json` at the root of a version's
//! files, `{"capabilities": {…}}`. It is a file like any other (versioned,
//! forked, pushed), and a publish that carries it takes its `capabilities`
//! as the declaration ([`crate::publish::validate`]).

use crate::{CoreError, Result};
use serde_json::Value;

/// The path of an artifact's manifest among its files.
pub const MANIFEST_PATH: &str = "clax.json";

fn bad(message: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_manifest", message)
}

/// The `capabilities` object of a `clax.json`: UTF-8 JSON (a leading
/// byte-order mark is skipped), an object whose one key is `capabilities`,
/// whose value is an object. The declaration itself is checked by
/// [`crate::capabilities::validate`], not here.
///
/// # Errors
/// `invalid_manifest` naming the first problem.
pub fn parse(bytes: &[u8]) -> Result<Value> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| bad("clax.json must be UTF-8"))?;
    let v: Value =
        serde_json::from_str(text).map_err(|e| bad(format!("clax.json is not JSON: {e}")))?;
    let Value::Object(mut obj) = v else {
        return Err(bad(
            "clax.json must be an object: {\"capabilities\": {...}}",
        ));
    };
    if let Some(k) = obj.keys().find(|k| *k != "capabilities") {
        return Err(bad(format!(
            "clax.json holds only \"capabilities\"; '{k}' is not a field of it"
        )));
    }
    match obj.remove("capabilities") {
        Some(caps @ Value::Object(_)) => Ok(caps),
        Some(_) => Err(bad("clax.json's \"capabilities\" must be an object")),
        None => Err(bad(
            "clax.json must hold \"capabilities\": {} declares none",
        )),
    }
}

/// The `clax.json` declaring `caps`: two-space indented JSON with a
/// trailing newline.
pub fn render(caps: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(&serde_json::json!({ "capabilities": caps }))
        .expect("a JSON value serializes");
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn code(bytes: &[u8]) -> Option<&'static str> {
        match parse(bytes) {
            Ok(_) => None,
            Err(CoreError::Invalid { code, .. }) => Some(code),
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn a_manifest_is_an_object_with_only_capabilities() {
        assert_eq!(parse(br#"{"capabilities":{}}"#).unwrap(), json!({}));
        assert_eq!(
            parse(b"\xEF\xBB\xBF{\"capabilities\": {\"db\": {}}}\n").unwrap(),
            json!({"db": {}})
        );
        for bad in [
            &b"[]"[..],
            b"{}",
            br#"{"capabilities":[]}"#,
            br#"{"capabilities":null}"#,
            br#"{"capabilities":{},"x":1}"#,
            b"\xff\xfe{}",
            b"{\"capabilities\":",
            b"",
        ] {
            assert_eq!(
                code(bad),
                Some("invalid_manifest"),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn render_round_trips() {
        let caps = json!({"db": {"rules": []}, "user": {"scopes": ["profile"]}});
        let bytes = render(&caps);
        assert_eq!(parse(&bytes).unwrap(), caps);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.ends_with("}\n"), "{text:?}");
        assert!(
            text.starts_with("{\n  \"capabilities\": {\n    \""),
            "{text:?}"
        );
        assert_eq!(render(&json!({})), b"{\n  \"capabilities\": {}\n}\n");
    }
}

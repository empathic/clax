//! Reading a reply as one JSON value for `sample.json` (`sample.d.ts`): the
//! whole reply; else the body of its one Markdown code fence; else the text
//! from the first `{` or `[` to the last `}` or `]`.

use serde_json::Value;

fn one_fence(t: &str) -> Option<&str> {
    let open = t.find("```")?;
    let after = &t[open + 3..];
    let body = &after[after.find('\n')? + 1..];
    let close = body.find("```")?;
    if body[close + 3..].contains("```") {
        return None;
    }
    Some(&body[..close])
}

/// The JSON value the reply holds, or `None`.
pub fn parse(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str(t) {
        return Some(v);
    }
    if let Some(body) = one_fence(t)
        && let Ok(v) = serde_json::from_str(body.trim())
    {
        return Some(v);
    }
    let start = t.find(['{', '['])?;
    let end = t.rfind(['}', ']'])?;
    if end < start {
        return None;
    }
    serde_json::from_str(&t[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::parse;
    use serde_json::json;

    #[test]
    fn reads_whole_fenced_or_framed_values() {
        assert_eq!(parse(" [1, 2] "), Some(json!([1, 2])));
        assert_eq!(parse("\"just a string\""), Some(json!("just a string")));
        assert_eq!(parse("```json\n{\"a\": 1}\n```"), Some(json!({"a": 1})));
        assert_eq!(
            parse("Here you go:\n{\"a\": 1}\nHope that helps."),
            Some(json!({"a": 1}))
        );
    }

    #[test]
    fn refuses_two_values_and_no_value() {
        assert_eq!(parse("[1] and [2]"), None);
        assert_eq!(parse("no json here"), None);
        assert_eq!(parse("```\n{\"a\": 1}\n```\n```\n{\"b\": 2}\n```"), None);
        assert_eq!(parse(""), None);
    }
}

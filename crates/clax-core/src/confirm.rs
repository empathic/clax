//! Browser confirmations as the daemon (`clax_http::confirm`), the CLI and
//! the MCP client share them: while an agent session is live, a decision
//! that matters made with the daemon token answers 202 `{status:
//! "confirmation_pending", confirmation_id, confirm_url, decision,
//! expires_at}` (the ID also in [`CONFIRMATION_HEADER`]) and waits for a
//! click in the owner's browser; the same call made again with the
//! confirmation's ID in [`CONFIRMATION_HEADER`] goes ahead once it is
//! approved. Each client keeps only its own send and long-poll loop.

use serde_json::Value;

/// The request header that presents a confirmation's ID with the call it
/// confirms, and the response header that names it on a 202.
pub const CONFIRMATION_HEADER: &str = "x-clax-confirmation";

/// The 202's `status`.
pub const PENDING: &str = "confirmation_pending";

/// The longest `GET /api/confirmations/<id>?wait=` waits, in seconds.
pub const MAX_WAIT_SECS: u64 = 50;

/// Whether `id` is a confirmation's ID: `cf_` and 1 to 64 letters, digits,
/// `_` or `-`, so it is safe in a path.
pub fn is_id(id: &str) -> bool {
    id.strip_prefix("cf_").is_some_and(|rest| {
        (1..=64).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// The confirmation a 202 held a decision for: its ID and `confirm_url`,
/// given the response's `body` and its [`CONFIRMATION_HEADER`] (`header`,
/// when the client read it; else the body's `confirmation_id`). `None`
/// for any answer that is not a held decision, or whose ID is not
/// [`is_id`].
pub fn held(header: Option<&str>, body: &Value) -> Option<(String, String)> {
    if body["status"] != PENDING {
        return None;
    }
    let id = header
        .or_else(|| body["confirmation_id"].as_str())
        .filter(|id| is_id(id))?;
    let url = body["confirm_url"].as_str().unwrap_or_default();
    Some((id.to_string(), url.to_string()))
}

/// The one line a client shows the person for a 202 whose `confirm_url` is
/// `url`, without the control and format characters the daemon's answer
/// might carry.
pub fn line(url: &str) -> String {
    let url: String = url
        .chars()
        .filter(|&c| !c.is_control() && !crate::text::is_format_char(c))
        .collect();
    format!("Confirm in your browser: {url}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_held_decision_names_its_confirmation_by_header_or_body() {
        let body = json!({"status": PENDING, "confirmation_id": "cf_ab12",
                          "confirm_url": "http://localhost:1/confirm/cf_ab12"});
        let want = Some((
            "cf_ab12".to_string(),
            "http://localhost:1/confirm/cf_ab12".to_string(),
        ));
        assert_eq!(held(None, &body), want);
        assert_eq!(held(Some("cf_ab12"), &body), want);
        assert_eq!(held(Some("cf_../x"), &body), None);
        assert_eq!(held(None, &json!({"status": "ok"})), None);
        assert_eq!(
            held(None, &json!({"status": PENDING, "confirmation_id": "ab12"})),
            None
        );
    }

    #[test]
    fn confirmation_ids_are_cf_and_up_to_64_path_safe_characters() {
        assert!(is_id("cf_0123abcdef"));
        assert!(is_id(&format!("cf_{}", "a".repeat(64))));
        assert!(!is_id(&format!("cf_{}", "a".repeat(65))));
        assert!(!is_id("cf_"));
        assert!(!is_id("cf_a/b"));
        assert!(!is_id("x_abc"));
    }

    #[test]
    fn the_line_drops_control_and_format_characters() {
        assert_eq!(
            line("http://localhost:1/c\u{1b}[2J\u{202e}x"),
            "Confirm in your browser: http://localhost:1/c[2Jx"
        );
    }
}

//! @mentions in comment text (spec §10, "Participants and attention").

/// The public IDs of the viewers `body` mentions: `@` and their whole display
/// name, in any case, followed by the end, whitespace or punctuation, and not
/// preceded by a word character (so an email address is no mention). A
/// viewer named `agent` is never mentioned: `@agent` sends to the agent.
pub fn mentioned(body: &str, names: &[(String, String)]) -> Vec<String> {
    let lower = body.to_lowercase();
    let mut out = Vec::new();
    for (public_id, name) in names {
        let n = name.trim().to_lowercase();
        if n.is_empty() || n == "agent" {
            continue;
        }
        let needle = format!("@{n}");
        let mut from = 0;
        while let Some(i) = lower[from..].find(&needle) {
            let at = from + i;
            let before_ok = lower[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
            let after = lower[at + needle.len()..].chars().next();
            let after_ok = after.is_none_or(|c| c.is_whitespace() || ".,;:!?)]}'\"".contains(c));
            if before_ok && after_ok {
                out.push(public_id.clone());
                break;
            }
            from = at + needle.len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::mentioned;

    fn names() -> Vec<(String, String)> {
        vec![
            ("u_a".into(), "Alex".into()),
            ("u_m".into(), "Mia Kovač".into()),
            ("u_j".into(), "Jun".into()),
        ]
    }

    #[test]
    fn mentions_match_whole_names_in_any_case_at_a_boundary() {
        assert_eq!(mentioned("@alex and @JUN, look", &names()), ["u_a", "u_j"]);
        assert_eq!(mentioned("ask @mia kovač.", &names()), ["u_m"]);
        assert!(
            mentioned("@mia alone", &names()).is_empty(),
            "a two-word name needs both words"
        );
        assert!(mentioned("@alexander", &names()).is_empty());
        assert!(mentioned("email alex@example.com", &names()).is_empty());
        assert!(mentioned("@agent please", &[("u_x".into(), "agent".into())]).is_empty());
    }
}

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

/// One `@` in `body` that may start an agent mention: its byte offset and
/// the run of name characters (`[A-Za-z0-9._-]`) after it, lowercased. An
/// `@` after a letter or digit (an address) starts none, nor does a run
/// too long to be a name even without its closing full stops. Runs never
/// overlap, so all of them cost one pass over `body`.
fn at_runs(body: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    body.match_indices('@').filter_map(move |(i, _)| {
        let before_ok = body[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if !before_ok {
            return None;
        }
        let rest = &body[i + 1..];
        let len = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            .count();
        let run = &rest[..len];
        (run.trim_end_matches('.').len() <= crate::names::MAX_NAME)
            .then(|| (i, run.to_ascii_lowercase()))
    })
}

/// The name `run` mentions: `run` itself, or `run` without the full stops
/// that end a sentence after it (`@web.`). The run is maximal, so what
/// follows it is never a name character.
fn mentioned_in(run: &str, is_name: impl Fn(&str) -> bool) -> Option<&str> {
    if is_name(run) {
        return Some(run);
    }
    let bare = run.trim_end_matches('.');
    (bare.len() < run.len() && is_name(bare)).then_some(bare)
}

/// The live agent names `body` mentions (spec 2026-10-08 §11), each once,
/// in the order they first appear: `@` and the whole name in any case, at
/// the start or after a character that is not a letter or digit, and
/// followed by the end, whitespace, or punctuation other than `.`, `_` and
/// `-` (a sentence's closing `.` still ends it). `live_names` are agent
/// names (M22); each is returned as given there.
pub fn agent_mentions(body: &str, live_names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, run) in at_runs(body) {
        let found = mentioned_in(&run, |n| {
            live_names.iter().any(|l| l.eq_ignore_ascii_case(n))
        });
        if let Some(n) = found
            && let Some(l) = live_names.iter().find(|l| l.eq_ignore_ascii_case(n))
            && !out.contains(l)
        {
            out.push(l.clone());
        }
    }
    out
}

/// The most `@name`s [`unresolved_mentions`] checks against people's names
/// in one body; any after them are not reported.
pub const MAX_UNRESOLVED_LOOKED_AT: usize = 64;

/// The `@name`s in `body` that have an agent name's form but name no live
/// agent (spec §11 `unresolved_mentions`), each once, in order: not
/// `@agent` (which keeps its meaning), not a live name, and not where a
/// viewer of `viewer_names` is mentioned ([`mentioned`]'s rules, so
/// `@Mia Kovač` is a person's mention). Only the first
/// [`MAX_UNRESOLVED_LOOKED_AT`] candidates are checked.
pub fn unresolved_mentions(
    body: &str,
    live_names: &[String],
    viewer_names: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut looked = 0;
    for (i, run) in at_runs(body) {
        let live = |n: &str| live_names.iter().any(|l| l.eq_ignore_ascii_case(n));
        if mentioned_in(&run, live).is_some() {
            continue;
        }
        let bare = run.trim_end_matches('.');
        if bare == "agent" || !crate::names::is_valid(bare) || out.iter().any(|o| o == bare) {
            continue;
        }
        if looked == MAX_UNRESOLVED_LOOKED_AT {
            break;
        }
        looked += 1;
        // A viewer's mention at this `@` ([`mentioned`]'s rules); display
        // names are at most 60 characters, so a short prefix decides it.
        let rest: String = body[i + 1..]
            .chars()
            .take(64)
            .collect::<String>()
            .to_lowercase();
        let at_viewer = viewer_names.iter().any(|v| {
            let v = v.trim().to_lowercase();
            !v.is_empty()
                && v != "agent"
                && rest.starts_with(&v)
                && rest[v.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_whitespace() || ".,;:!?)]}'\"".contains(c))
        });
        if !at_viewer {
            out.push(bare.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{agent_mentions, mentioned, unresolved_mentions};

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

    fn live() -> Vec<String> {
        vec![
            "web".into(),
            "web-2".into(),
            "clax-main".into(),
            "a.b".into(),
        ]
    }

    #[test]
    fn agent_mentions_match_whole_live_names_at_a_boundary() {
        assert_eq!(agent_mentions("@web please", &live()), ["web"]);
        assert_eq!(
            agent_mentions("thanks @WEB.", &live()),
            ["web"],
            "a sentence's full stop"
        );
        assert_eq!(
            agent_mentions("(@web) and @web-2, @web", &live()),
            ["web", "web-2"]
        );
        assert_eq!(agent_mentions("@clax-main: look", &live()), ["clax-main"]);
        assert_eq!(agent_mentions("@a.b!", &live()), ["a.b"]);
        for none in [
            "@web-3",
            "@webby",
            "@web_x",
            "@web.x",
            "me@web",
            "x@web.dev",
            "é@web",
            "@ web",
            "@agent",
            "@a",
            "",
            "@",
        ] {
            assert!(agent_mentions(none, &live()).is_empty(), "{none:?}");
        }
        assert_eq!(
            agent_mentions("—@web", &live()),
            ["web"],
            "an em dash before"
        );
        assert_eq!(
            agent_mentions("@web\u{200B}", &live()),
            ["web"],
            "an invisible after"
        );
        assert!(
            agent_mentions("@ｗｅｂ", &live()).is_empty(),
            "full-width letters are other characters"
        );
        assert!(agent_mentions("@wеb", &live()).is_empty(), "a Cyrillic е");
    }

    #[test]
    fn unresolved_mentions_are_name_shaped_and_not_people_or_agent() {
        let viewers = vec!["Mia Kovač".to_string(), "Jun".to_string()];
        assert_eq!(
            unresolved_mentions(
                "@web @nope, @agent @Jun @mia kovač @Nope. x@y",
                &live(),
                &viewers
            ),
            ["nope"]
        );
        assert_eq!(
            unresolved_mentions("@web-3 @x__", &live(), &[]),
            ["web-3", "x__"]
        );
        assert!(unresolved_mentions(&format!("@{}", "x".repeat(40)), &live(), &[]).is_empty());
        assert!(unresolved_mentions("@-x @.x @", &live(), &[]).is_empty());
        assert_eq!(
            unresolved_mentions("İİ @Mia Kovač @mia", &live(), &viewers),
            ["mia"],
            "a lowercase that changes lengths before the @"
        );
        let many = "@x ".repeat(10_000) + &format!("@{}", "y".repeat(100_000));
        assert_eq!(unresolved_mentions(&many, &live(), &viewers), ["x"]);
        let distinct: String = (0..1000).map(|n| format!("@n{n} ")).collect();
        assert_eq!(
            unresolved_mentions(&distinct, &live(), &viewers).len(),
            super::MAX_UNRESOLVED_LOOKED_AT
        );
    }
}

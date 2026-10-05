//! Readable terminal output: colour only on a terminal (and never under
//! `NO_COLOR`), text from people made safe to print, short ages.

use std::io::IsTerminal;

/// Whether readable output on stdout is coloured: stdout is a terminal and
/// `NO_COLOR` is unset or empty (<https://no-color.org>).
pub fn color_on() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// Styles applied to readable output; all no-ops when colour is off.
#[derive(Clone, Copy)]
pub struct Paint {
    on: bool,
}

impl Paint {
    pub fn new(on: bool) -> Paint {
        Paint { on }
    }
    pub fn stdout() -> Paint {
        Paint::new(color_on())
    }
    fn wrap(self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn bold(self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn green(self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn cyan(self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn magenta(self, s: &str) -> String {
        self.wrap("35", s)
    }
}

/// Unicode format characters that reorder or hide text on a terminal
/// (bidirectional marks, overrides and isolates, the Arabic letter mark, the
/// line and paragraph separators, and the zero-width characters).
fn is_invisible_control(c: char) -> bool {
    matches!(
        c,
        '\u{61c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// `raw` (text from a person or an agent) safe to print on one line: line
/// breaks and tabs become spaces, every other control character, C1 controls
/// and bidirectional formatting characters included, is shown escaped
/// (`\x1b`, `\u{202e}`), so it can never move the cursor, change colours or
/// reorder the line.
pub fn clean_line(raw: &str) -> String {
    clean(raw, false)
}

/// [`clean_line`], keeping line breaks (`\r\n` and `\r` become `\n`).
pub fn clean_text(raw: &str) -> String {
    clean(&raw.replace("\r\n", "\n"), true)
}

fn clean(raw: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '\n' | '\r' if keep_newlines => out.push('\n'),
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() && (c as u32) < 0x80 => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if c.is_control() || is_invisible_control(c) => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// `s` cut to at most `max` characters, ending in `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One line of person-written text: [`clean_line`], whitespace runs
/// collapsed, cut to `max` characters.
pub fn snippet(raw: &str, max: usize) -> String {
    let one = clean_line(raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate(&one, max)
}

/// How long ago `rfc3339` was, as `now`, `42s`, `5m`, `3h`, `2d` or `6w`;
/// empty when it does not parse.
pub fn age(rfc3339: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return String::new();
    };
    span((now - t.with_timezone(&chrono::Utc)).num_seconds())
}

/// [`age`] as a phrase: `just now`, `5m ago`.
pub fn ago(rfc3339: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    match age(rfc3339, now).as_str() {
        "" => String::new(),
        "now" => "just now".to_string(),
        a => format!("{a} ago"),
    }
}

/// A duration in seconds as [`age`] shows it.
pub fn span(secs: i64) -> String {
    match secs.max(0) {
        s if s < 5 => "now".to_string(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 14 * 86_400 => format!("{}d", s / 86_400),
        s => format!("{}w", s / (7 * 86_400)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_and_bidi_controls_are_shown_not_obeyed() {
        let raw = "hi\x1b[31mred\x07\u{9b}2J\u{202e}evil\tend\r\nnext";
        let line = clean_line(raw);
        assert_eq!(line, "hi\\x1b[31mred\\x07\\u{9b}2J\\u{202e}evil end  next");
        assert!(!line.chars().any(|c| c.is_control()));
        let text = clean_text(raw);
        assert!(text.ends_with("evil end\nnext"), "{text}");
        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
    }

    #[test]
    fn the_arabic_letter_mark_and_zero_width_characters_are_shown() {
        for c in [
            '\u{61c}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{2061}', '\u{2062}',
            '\u{2063}', '\u{2064}', '\u{feff}',
        ] {
            let line = clean_line(&format!("a{c}b"));
            assert_eq!(line, format!("a\\u{{{:x}}}b", c as u32), "{:x}", c as u32);
        }
    }

    #[test]
    fn snippets_collapse_and_cut() {
        assert_eq!(snippet("a\n\n  b   c", 10), "a b c");
        assert_eq!(snippet("abcdefghijkl", 5), "abcd…");
        assert_eq!(truncate("héllo", 5), "héllo");
    }

    #[test]
    fn ages_are_short() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(age("2026-10-05T11:59:58Z", now), "now");
        assert_eq!(age("2026-10-05T11:59:00Z", now), "1m");
        assert_eq!(age("2026-10-05T09:00:00.000Z", now), "3h");
        assert_eq!(age("2026-10-03T12:00:00Z", now), "2d");
        assert_eq!(age("2026-08-01T12:00:00Z", now), "9w");
        assert_eq!(age("garbage", now), "");
    }

    #[test]
    fn paint_is_plain_when_off() {
        assert_eq!(Paint::new(false).bold("x"), "x");
        assert_eq!(Paint::new(true).bold("x"), "\x1b[1mx\x1b[0m");
    }
}

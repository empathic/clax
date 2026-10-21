//! A new artifact started from a brief (`POST /api/artifacts:new`): the
//! checks on its title and the placeholder page its version 1 holds until
//! an agent builds it. The brief itself is the artifact's first thread.

use crate::{CoreError, Result};

/// Longest title a brief's artifact may take, in characters: any
/// artifact's ([`crate::publish::MAX_TITLE_CHARS`]).
pub const MAX_TITLE_CHARS: usize = crate::publish::MAX_TITLE_CHARS;

/// The title, trimmed, checked as any artifact title is
/// ([`crate::publish::check_meta_text`]): `invalid_title` when it is blank,
/// longer than [`MAX_TITLE_CHARS`], or holds a control character (a line
/// break) or a hidden format character ([`crate::text::is_hidden_char`]: a
/// bidi control, a zero-width space), which would make one-line views read
/// differently from what was typed. The joiners (U+200C, U+200D), which
/// Persian and Indic words and emoji sequences need, and the direction
/// marks (U+200E, U+200F, U+061C) are allowed.
///
/// # Errors
/// As above.
pub fn check_title(title: &str) -> Result<String> {
    let t = title.trim();
    if t.is_empty() {
        return Err(CoreError::invalid("invalid_title", "a title is required"));
    }
    crate::publish::check_meta_text(Some(t), None, None).map_err(|e| match e {
        CoreError::Invalid {
            code: "title_too_long",
            ..
        } => CoreError::invalid(
            "invalid_title",
            format!("a title is at most {MAX_TITLE_CHARS} characters"),
        ),
        CoreError::Invalid { .. } => CoreError::invalid(
            "invalid_title",
            "the title holds a control or hidden format character",
        ),
        other => other,
    })?;
    Ok(t.to_string())
}

/// The brief, trimmed: `invalid_brief` when it is blank. Its length is the
/// first comment's (the store's comment limit).
///
/// # Errors
/// As above.
pub fn check_brief(brief: &str) -> Result<String> {
    let b = brief.trim();
    if b.is_empty() {
        return Err(CoreError::invalid(
            "invalid_brief",
            "a brief is required: say what you want built",
        ));
    }
    Ok(b.to_string())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Version 1 of an artifact started from a brief: its title and a line
/// saying no agent has built it yet, following the page contract (tokens on
/// `:root`, dark mode under the media query and `data-theme`, an explicit
/// body background, a 16 px gutter at phone width).
pub fn placeholder_html(title: &str) -> String {
    let t = escape_html(title);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{t}</title>
<style>
  :root {{ --bg: #f6f5f1; --card: #fffefb; --ink: #1d1c19; --muted: #6a675f; --line: #d9d5ca; --accent: #8a5a00; }}
  @media (prefers-color-scheme: dark) {{
    :root:not([data-theme="light"]) {{ --bg: #141412; --card: #1c1b18; --ink: #ecebe6; --muted: #a6a398; --line: #3a3832; --accent: #e0b25c; }}
  }}
  :root[data-theme="dark"] {{ --bg: #141412; --card: #1c1b18; --ink: #ecebe6; --muted: #a6a398; --line: #3a3832; --accent: #e0b25c; }}
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 48px 16px; background: var(--bg); color: var(--ink); font: 16px/1.55 ui-sans-serif, -apple-system, system-ui, sans-serif; }}
  main {{ width: 100%; max-width: 34rem; padding: 32px 28px; border: 1.5px dashed var(--line); border-radius: 14px; background: var(--card); }}
  .tag {{ margin: 0 0 12px; color: var(--accent); font-size: 12px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; }}
  h1 {{ margin: 0 0 12px; font-size: clamp(1.5rem, 5vw, 2rem); line-height: 1.2; overflow-wrap: anywhere; }}
  p {{ margin: 0; color: var(--muted); }}
  p + p {{ margin-top: 8px; }}
</style>
</head>
<body>
<main>
<p class="tag">Waiting for an agent</p>
<h1>{t}</h1>
<p>An agent hasn't built this page yet.</p>
<p>The brief is this artifact's first comment. Once an agent picks it up, its first version replaces this page.</p>
</main>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_trimmed_bounded_and_one_line() {
        assert_eq!(check_title("  Q3 plan ").unwrap(), "Q3 plan");
        let code = |r: Result<String>| match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("{other:?}"),
        };
        assert_eq!(code(check_title("   ")), "invalid_title");
        assert_eq!(code(check_title("a\nb")), "invalid_title");
        assert_eq!(code(check_title("a\u{202e}b")), "invalid_title");
        assert_eq!(code(check_title("a\u{200b}b")), "invalid_title");
        assert_eq!(code(check_title("a\u{2066}b")), "invalid_title");
        // Persian 'mi‌xaham' needs ZWNJ; a family emoji is joined with ZWJ.
        assert!(check_title("می\u{200c}خواهم").is_ok());
        assert!(check_title("Team \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}").is_ok());
        // Direction marks, which right-to-left text needs, as in any title.
        assert!(check_title("\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{200f}Q3").is_ok());
        assert_eq!(
            code(check_title(&"é".repeat(MAX_TITLE_CHARS + 1))),
            "invalid_title"
        );
        assert!(check_title(&"é".repeat(MAX_TITLE_CHARS)).is_ok());
        assert_eq!(code(check_brief(" \n ")), "invalid_brief");
        assert_eq!(check_brief(" build it\n").unwrap(), "build it");
    }

    #[test]
    fn the_placeholder_names_its_title_escaped_and_keeps_the_page_contract() {
        let html = placeholder_html("<b>Plan</b> & \"more\"");
        assert!(html.contains("<title>&lt;b&gt;Plan&lt;/b&gt; &amp; &quot;more&quot;</title>"));
        assert!(!html.contains("<b>Plan"));
        assert!(html.contains("hasn't built this page yet"));
        assert!(html.contains(":root {"));
        assert!(html.contains("@media (prefers-color-scheme: dark)"));
        assert!(html.contains(":root:not([data-theme=\"light\"])"));
        assert!(html.contains(":root[data-theme=\"dark\"]"));
        assert!(html.contains("body {") && html.contains("background: var(--bg)"));
        assert!(html.contains("width=device-width"));
        assert_eq!(
            crate::publish::html_title(&html).as_deref(),
            Some("<b>Plan</b> & \"more\"")
        );
    }
}

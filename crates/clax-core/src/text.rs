//! Character classes for text that people and agents read back: which
//! characters can make rendered text read differently from what was stored.

/// Unicode format (category Cf) characters, the bidi controls among them.
pub fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// A format character that one-line display text has no use for: every
/// [`is_format_char`] but the joiners (ZWNJ U+200C, ZWJ U+200D), which
/// emoji sequences and several scripts need, and the direction marks (LRM
/// U+200E, RLM U+200F, ALM U+061C), which right-to-left text needs and
/// which, unlike embeddings, overrides and isolates, reorder nothing around
/// them. Bidi controls, zero-width spaces and tag characters are hidden.
pub fn is_hidden_char(c: char) -> bool {
    is_format_char(c) && !matches!(c, '\u{200C}'..='\u{200F}' | '\u{061C}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joiners_and_marks_are_not_hidden() {
        for c in ['\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{061C}'] {
            assert!(is_format_char(c) && !is_hidden_char(c), "{:04X}", c as u32);
        }
        for c in ['\u{202E}', '\u{2066}', '\u{200B}', '\u{FEFF}', '\u{E0041}'] {
            assert!(is_hidden_char(c), "{:04X}", c as u32);
        }
        assert!(!is_hidden_char('a') && !is_hidden_char('\n'));
    }
}

//! Tags: one instance-wide list the owner manages, each tag a name, a
//! colour from [`COLOURS`] and an optional description, applied to
//! artifacts by the people who administer them. The rules here need no
//! database; the store keeps the tags (`clax_store::tags`).

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// The colours a tag may take: palette keys, not raw colours, so each theme
/// renders a tag in a shade that keeps its text legible.
pub const COLOURS: &[&str] = &[
    "gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "brown",
];

/// The colour a tag takes when its creator names none.
pub const DEFAULT_COLOUR: &str = "gray";

/// The longest tag name, in characters, after trimming.
pub const MAX_NAME_CHARS: usize = 40;

/// The longest tag description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 200;

/// The most tags one Clax keeps.
pub const MAX_TAGS: usize = 500;

/// The most tags one artifact carries.
pub const MAX_ARTIFACT_TAGS: usize = 20;

/// `name` as names are compared: lowercased, so two names that differ only
/// in case are one name.
pub fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// A tag as an artifact carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagRef {
    pub id: String,
    pub name: String,
    pub colour: String,
}

/// A tag of the list, as `GET /api/tags` serves it (`count` added there).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub colour: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Tag {
    /// The tag as an artifact carries it.
    pub fn as_ref(&self) -> TagRef {
        TagRef {
            id: self.id.clone(),
            name: self.name.clone(),
            colour: self.colour.clone(),
        }
    }
}

/// The prefix of a tag ID.
pub const ID_PREFIX: &str = "t_";
/// Random characters after [`ID_PREFIX`].
const ID_CHARS: usize = 10;

/// A new tag ID: `t_` and ten lowercase Crockford base32 characters.
pub fn new_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::rng().fill_bytes(&mut bytes);
    let mut n = u64::from_le_bytes(bytes);
    let mut out = String::with_capacity(ID_PREFIX.len() + ID_CHARS);
    out.push_str(ID_PREFIX);
    for _ in 0..ID_CHARS {
        out.push(crate::ids::ALPHABET[(n & 31) as usize] as char);
        n >>= 5;
    }
    out
}

/// Whether `s` has the shape of a tag ID ([`new_id`]).
pub fn is_id(s: &str) -> bool {
    s.strip_prefix(ID_PREFIX).is_some_and(|rest| {
        rest.len() == ID_CHARS && rest.bytes().all(|b| crate::ids::ALPHABET.contains(&b))
    })
}

/// `raw` as a tag name: trimmed, 1 to [`MAX_NAME_CHARS`] characters, with
/// no control or hidden format character
/// ([`crate::publish::check_meta_text`]'s rule), and not shaped like a tag
/// ID, so a name never reads as another tag's ID.
///
/// # Errors
/// `invalid_tag` naming what is wrong.
pub fn check_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(CoreError::invalid("invalid_tag", "a tag name is not blank"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CoreError::invalid(
            "invalid_tag",
            format!("a tag name is at most {MAX_NAME_CHARS} characters"),
        ));
    }
    if name
        .chars()
        .any(|c| c.is_control() || crate::text::is_hidden_char(c))
    {
        return Err(CoreError::invalid(
            "invalid_tag",
            "a tag name holds no control or hidden format character",
        ));
    }
    if is_id(name) {
        return Err(CoreError::invalid(
            "invalid_tag",
            format!("'{name}' reads as a tag ID; choose another name"),
        ));
    }
    Ok(name.to_string())
}

/// `raw` as a tag colour: one of [`COLOURS`].
///
/// # Errors
/// `invalid_colour` naming the colours.
pub fn check_colour(raw: &str) -> Result<String> {
    if COLOURS.contains(&raw) {
        Ok(raw.to_string())
    } else {
        Err(CoreError::invalid(
            "invalid_colour",
            format!("'{raw}' is not a tag colour: {}", COLOURS.join(", ")),
        ))
    }
}

/// `raw` as a tag description: at most [`MAX_DESCRIPTION_CHARS`]
/// characters, line breaks and the rest of [`check_name`]'s refused
/// characters refused; blank is none.
///
/// # Errors
/// `invalid_tag` naming what is wrong.
pub fn check_description(raw: &str) -> Result<Option<String>> {
    let d = raw.trim();
    if d.is_empty() {
        return Ok(None);
    }
    if d.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(CoreError::invalid(
            "invalid_tag",
            format!("a tag description is at most {MAX_DESCRIPTION_CHARS} characters"),
        ));
    }
    if d.chars()
        .any(|c| c.is_control() || crate::text::is_hidden_char(c))
    {
        return Err(CoreError::invalid(
            "invalid_tag",
            "a tag description holds no control or hidden format character",
        ));
    }
    Ok(Some(d.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_bounded_and_plain() {
        assert_eq!(check_name("  Design  ").unwrap(), "Design");
        assert_eq!(check_name(&"x".repeat(40)).unwrap().len(), 40);
        for bad in [
            "",
            "   ",
            "a\nb",
            "a\u{200b}b",
            "a\u{202e}b",
            "t_0123456789",
        ] {
            let e = check_name(bad).unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_tag",
                        ..
                    }
                ),
                "{bad:?}: {e}"
            );
        }
        assert!(check_name(&"x".repeat(41)).is_err());
        // Forty characters, not bytes.
        assert!(check_name(&"é".repeat(40)).is_ok());
    }

    #[test]
    fn colours_are_the_palette_keys() {
        for c in COLOURS {
            assert_eq!(check_colour(c).unwrap(), *c);
        }
        assert!(COLOURS.contains(&DEFAULT_COLOUR));
        for bad in ["#ff0000", "Red", "grey", ""] {
            assert!(matches!(
                check_colour(bad),
                Err(CoreError::Invalid {
                    code: "invalid_colour",
                    ..
                })
            ));
        }
    }

    #[test]
    fn ids_have_their_shape_and_differ() {
        let a = new_id();
        assert!(is_id(&a), "{a}");
        assert_ne!(a, new_id());
        assert!(!is_id("t_short"));
        assert!(!is_id("x_0123456789"));
    }

    #[test]
    fn descriptions_are_optional_and_one_line() {
        assert_eq!(check_description("  ").unwrap(), None);
        assert_eq!(check_description(" ok ").unwrap().as_deref(), Some("ok"));
        assert!(check_description("a\nb").is_err());
        assert!(check_description(&"d".repeat(201)).is_err());
    }
}

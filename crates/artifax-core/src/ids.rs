//! Identifiers: 12-character Crockford base32 artifact IDs and ULIDs for everything else.

use crate::error::{CoreError, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Lowercase Crockford base32 without i, l, o, u.
pub const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
pub const ARTIFACT_ID_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ArtifactId(String);

impl ArtifactId {
    /// 60 random bits rendered as 12 base32 characters.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 8];
        rand::rng().fill_bytes(&mut bytes);
        let mut n = u64::from_le_bytes(bytes);
        let mut out = String::with_capacity(ARTIFACT_ID_LEN);
        for _ in 0..ARTIFACT_ID_LEN {
            out.push(ALPHABET[(n & 31) as usize] as char);
            n >>= 5;
        }
        ArtifactId(out)
    }

    pub fn parse(s: &str) -> Result<Self> {
        let ok = s.len() == ARTIFACT_ID_LEN && s.bytes().all(|b| ALPHABET.contains(&b));
        if ok {
            Ok(ArtifactId(s.to_string()))
        } else {
            Err(CoreError::invalid(
                "invalid_id",
                format!("'{s}' is not an artifact ID"),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ArtifactId {
    type Error = CoreError;
    fn try_from(s: String) -> Result<Self> {
        ArtifactId::parse(&s)
    }
}

impl From<ArtifactId> for String {
    fn from(id: ArtifactId) -> String {
        id.0
    }
}

/// A new ULID. IDs from this process ascend: within one millisecond the random
/// part is incremented, so ordering rows by `(created_at, id)` is total. When
/// the random part would overflow (2^80 IDs in one millisecond), a fresh
/// random ULID is returned instead.
pub fn new_ulid() -> String {
    static GENERATOR: std::sync::OnceLock<std::sync::Mutex<ulid::Generator>> =
        std::sync::OnceLock::new();
    let mut g = GENERATOR
        .get_or_init(|| std::sync::Mutex::new(ulid::Generator::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    g.generate()
        .unwrap_or_else(|_| ulid::Ulid::new())
        .to_string()
}

/// True when `s` is a ULID in canonical form: 26 uppercase Crockford base32
/// characters that parse and render back to `s` (so the first character is at
/// most `7`).
pub fn is_ulid(s: &str) -> bool {
    ulid::Ulid::from_string(s).is_ok_and(|u| u.to_string() == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_12_crockford_chars() {
        for _ in 0..100 {
            let id = ArtifactId::generate();
            assert_eq!(id.as_str().len(), 12);
            assert!(id.as_str().bytes().all(|b| ALPHABET.contains(&b)), "{id:?}");
        }
    }

    #[test]
    fn parse_rejects_bad_ids() {
        assert!(ArtifactId::parse("7q3k9mzx2b4t").is_ok());
        assert!(ArtifactId::parse("7Q3K9MZX2B4T").is_err(), "uppercase");
        assert!(ArtifactId::parse("7q3k9mzx2b4").is_err(), "short");
        assert!(ArtifactId::parse("7q3k9mzx2b4tu").is_err(), "long");
        assert!(
            ArtifactId::parse("7q3k9mzx2b4i").is_err(),
            "i not in alphabet"
        );
    }

    #[test]
    fn ulids_are_26_chars_and_unique() {
        let a = new_ulid();
        let b = new_ulid();
        assert_eq!(a.len(), 26);
        assert_ne!(a, b);
    }

    #[test]
    fn ulids_ascend_within_one_millisecond() {
        let ids: Vec<String> = (0..1000).map(|_| new_ulid()).collect();
        for pair in ids.windows(2) {
            assert!(pair[0] < pair[1], "{} !< {}", pair[0], pair[1]);
            assert!(is_ulid(&pair[1]));
        }
    }

    #[test]
    fn is_ulid_accepts_only_canonical_strings() {
        assert!(is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(!is_ulid("01arz3ndektsv4rrffq69g5fav"), "lowercase");
        assert!(
            !is_ulid("81ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "first character past 7 overflows"
        );
        assert!(!is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FA"), "short");
        assert!(!is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAU"), "U is not Crockford");
        assert!(!is_ulid(""));
    }
}

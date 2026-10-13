//! Secrets the daemon mints for people and their machines (spec §6.2–§6.5,
//! §6.8): invite codes, user codes, access, manage and continuation tokens,
//! browser-link codes, and agent sessions' secrets.
//!
//! A [`Secret`] is shown to its holder once and never otherwise: its `Debug`
//! prints `Secret(…)`, and it has no `Display` or `Serialize`, so a log line,
//! an error or an event cannot carry one by accident. The store keeps only
//! [`sha256_hex`] of each, and compares hashes with [`constant_time_eq`].
//! Every code is drawn from the operating system's random source.

use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::fmt;

/// Prefix of an access token.
pub const ACCESS_PREFIX: &str = "cxg_";
/// Prefix of a manage token.
pub const MANAGE_PREFIX: &str = "cxm_";
/// Prefix of a continuation token.
pub const CONTINUE_PREFIX: &str = "cxc_";
/// Prefix of an agent session's secret (spec 2026-10-08 §6.8).
pub const SESSION_PREFIX: &str = "cxs_";

/// Crockford base32, uppercase, without I, L, O and U.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// The letters of a user code: A to Z without I and O.
const USER_LETTERS: &[u8; 24] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";

/// A secret value: shown to its holder once, otherwise only hashed.
/// It has no `PartialEq`: compare hashes with [`constant_time_eq`].
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(value)
    }

    /// The value itself, for handing to its holder.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// [`sha256_hex`] of the value.
    pub fn sha256_hex(&self) -> String {
        sha256_hex(&self.0)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// Lowercase hex SHA-256 of `s`'s UTF-8.
pub fn sha256_hex(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

/// SHA-256 of `s`'s UTF-8.
pub fn sha256(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `a` and `b` are equal, in time that depends only on their
/// lengths.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `N` bytes from the operating system's random source.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the operating system's random source is available");
    b
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A token: `prefix` and 43 base64url characters (32 random bytes).
pub fn token(prefix: &str) -> Secret {
    Secret(format!("{prefix}{}", base64url(&random_bytes::<32>())))
}

/// A browser-link code: 43 base64url characters (32 random bytes).
pub fn browser_code() -> Secret {
    Secret(base64url(&random_bytes::<32>()))
}

/// An invite code: 12 Crockford base32 characters (60 bits) in three groups
/// of four, `K7QF-M2XD-9PNA`.
pub fn invite_code() -> Secret {
    let mut n = u64::from_le_bytes(random_bytes::<8>());
    let mut out = String::with_capacity(14);
    for i in 0..12 {
        if i > 0 && i % 4 == 0 {
            out.push('-');
        }
        out.push(CROCKFORD[(n & 31) as usize] as char);
        n >>= 5;
    }
    Secret(out)
}

/// The canonical form of a typed invite code: uppercase, without
/// separators, I and L read as 1 and O as 0. `None` unless that leaves 12
/// Crockford base32 characters.
pub fn canonical_invite(code: &str) -> Option<String> {
    let out: String = code
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        })
        .collect();
    (out.len() == 12 && out.bytes().all(|b| CROCKFORD.contains(&b))).then_some(out)
}

/// A user code: four letters without I and O, a dash, four digits
/// (`WXYZ-4821`).
pub fn user_code() -> Secret {
    let b = random_bytes::<16>();
    let mut out = String::with_capacity(9);
    // Rejection-free enough: 256 mod 24 and 256 mod 10 skew each character by
    // under 0.4 %, which leaves well over 31 bits.
    for &x in &b[..4] {
        out.push(USER_LETTERS[x as usize % USER_LETTERS.len()] as char);
    }
    out.push('-');
    for &x in &b[4..8] {
        out.push((b'0' + x % 10) as char);
    }
    Secret(out)
}

/// The canonical form of a typed user code: uppercase, without separators.
/// `None` unless that leaves four letters and four digits.
pub fn canonical_user_code(code: &str) -> Option<String> {
    let out: String = code
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let b = out.as_bytes();
    (b.len() == 8
        && b[..4].iter().all(|c| USER_LETTERS.contains(c))
        && b[4..].iter().all(u8::is_ascii_digit))
    .then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        assert!(!format!("{:?}", Secret("cxg_abc".into())).contains("abc"));
        assert_eq!(
            format!("{:?}", Some(token(ACCESS_PREFIX))),
            "Some(Secret(…))"
        );
    }

    #[test]
    fn codes_have_the_spec_shapes() {
        let t = token(ACCESS_PREFIX);
        assert!(t.expose().starts_with("cxg_") && t.expose().len() == 4 + 43);
        assert_eq!(browser_code().expose().len(), 43);
        let inv = invite_code();
        let parts: Vec<&str> = inv.expose().split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [4, 4, 4]);
        let canon = canonical_invite(inv.expose()).unwrap();
        assert_eq!(canonical_invite(&canon.to_lowercase()), Some(canon.clone()));
        assert_eq!(
            canonical_invite("ilo0-0000-0000").as_deref(),
            Some("110000000000")
        );
        assert_eq!(canonical_invite("K7QFM2XD9PNU"), None, "U is not Crockford");
        assert_eq!(canonical_invite("K7QF"), None);
        let u = user_code();
        let b = u.expose().as_bytes();
        assert_eq!(b.len(), 9);
        assert!(
            b[..4]
                .iter()
                .all(|c| c.is_ascii_uppercase() && *c != b'I' && *c != b'O')
        );
        assert_eq!(b[4], b'-');
        assert!(b[5..].iter().all(u8::is_ascii_digit));
        assert_eq!(
            canonical_user_code("wxyz 4821").as_deref(),
            Some("WXYZ4821")
        );
        assert_eq!(canonical_user_code("WXIZ-4821"), None);
        assert_ne!(token(ACCESS_PREFIX).expose(), token(ACCESS_PREFIX).expose());
    }

    #[test]
    fn constant_time_eq_compares() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert_eq!(sha256_hex("").len(), 64);
        assert_eq!(hex(&sha256("x")), sha256_hex("x"));
    }
}

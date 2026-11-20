//! Macaroons over HMAC-SHA256 (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §18.1):
//! bearer credentials that anyone holding one can narrow, by appending a
//! first-party caveat, and no one can widen or forge without the key.
//!
//! A macaroon is an identifier, its caveats in order, and a signature:
//! `sig₀ = HMAC(key, identifier)`, `sigᵢ = HMAC(sigᵢ₋₁, caveatᵢ)`, the
//! signature the last. [`Macaroon::verify`] recomputes the chain and
//! compares its last step in constant time (`ring::hmac::verify`); it
//! says nothing of what the caveats mean, which is the caller's to check
//! (each one, every time: a caveat the caller does not understand must
//! refuse).
//!
//! Serialized ([`Macaroon::encode`]) as a prefix and the unpadded base64url
//! of `{"i": identifier, "c": [caveat, …], "s": base64url(signature)}`.

use crate::secret::base64url;
use base64::Engine as _;
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The most bytes [`Macaroon::decode`] reads: far past any macaroon Clax
/// mints, so a huge string costs nothing to refuse.
pub const MAX_ENCODED_BYTES: usize = 8 * 1024;

/// A macaroon (see the module docs). Its `Debug` shows the identifier and
/// caveats, never the signature.
#[derive(Clone, PartialEq, Eq)]
pub struct Macaroon {
    identifier: String,
    caveats: Vec<String>,
    sig: [u8; 32],
}

impl fmt::Debug for Macaroon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Macaroon")
            .field("identifier", &self.identifier)
            .field("caveats", &self.caveats)
            .finish_non_exhaustive()
    }
}

/// The wire form's JSON.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    i: String,
    c: Vec<String>,
    s: String,
}

/// The macaroon did not verify with the key it was checked with: it was
/// forged, changed, or made with another key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unverified;

fn mac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), msg);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// `HMAC-SHA256(key, label)`: a key derived from `key` for `label` alone.
#[must_use]
pub fn derive_key(key: &[u8], label: &str) -> [u8; 32] {
    mac(key, label.as_bytes())
}

impl Macaroon {
    /// A macaroon of `identifier`, no caveats, signed with `key`.
    #[must_use]
    pub fn mint(key: &[u8], identifier: &str) -> Macaroon {
        Macaroon {
            identifier: identifier.to_string(),
            caveats: Vec::new(),
            sig: mac(key, identifier.as_bytes()),
        }
    }

    /// This macaroon with `caveat` appended: it allows at most what this
    /// one does, and verifies with the same key.
    #[must_use]
    pub fn attenuate(&self, caveat: &str) -> Macaroon {
        let mut caveats = self.caveats.clone();
        caveats.push(caveat.to_string());
        Macaroon {
            identifier: self.identifier.clone(),
            caveats,
            sig: mac(&self.sig, caveat.as_bytes()),
        }
    }

    #[must_use]
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Its caveats, in the order they were added.
    #[must_use]
    pub fn caveats(&self) -> &[String] {
        &self.caveats
    }

    /// Whether this macaroon was made with `key`, its caveats as they are.
    ///
    /// # Errors
    /// [`Unverified`] when it was not.
    pub fn verify(&self, key: &[u8]) -> Result<(), Unverified> {
        let (last, before) = match self.caveats.split_last() {
            None => (self.identifier.as_bytes(), key.to_vec()),
            Some((last, rest)) => {
                let mut sig = mac(key, self.identifier.as_bytes());
                for c in rest {
                    sig = mac(&sig, c.as_bytes());
                }
                (last.as_bytes(), sig.to_vec())
            }
        };
        hmac::verify(
            &hmac::Key::new(hmac::HMAC_SHA256, &before),
            last,
            &self.sig,
        )
        .map_err(|_| Unverified)
    }

    /// `prefix` and the unpadded base64url of its JSON (see the module
    /// docs).
    #[must_use]
    pub fn encode(&self, prefix: &str) -> String {
        let wire = Wire {
            i: self.identifier.clone(),
            c: self.caveats.clone(),
            s: base64url(&self.sig),
        };
        let json = serde_json::to_vec(&wire).expect("a macaroon's JSON encodes");
        format!("{prefix}{}", base64url(&json))
    }

    /// The macaroon `s` encodes after `prefix`, or `None` for anything
    /// else: another prefix, more than [`MAX_ENCODED_BYTES`], bad base64url
    /// or JSON, a member it does not have, a signature not 32 bytes.
    /// Decoding verifies nothing.
    #[must_use]
    pub fn decode(prefix: &str, s: &str) -> Option<Macaroon> {
        if s.len() > MAX_ENCODED_BYTES {
            return None;
        }
        let body = s.strip_prefix(prefix)?;
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .ok()?;
        // An object alone: serde would also read `Wire` from an array.
        let v: serde_json::Value = serde_json::from_slice(&json).ok()?;
        if !v.is_object() {
            return None;
        }
        let wire: Wire = serde_json::from_value(v).ok()?;
        let sig: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&wire.s)
            .ok()?
            .try_into()
            .ok()?;
        Some(Macaroon {
            identifier: wire.i,
            caveats: wire.c,
            sig,
        })
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"a key of thirty-two bytes, as is";
    const P: &str = "cxt_";

    fn sample() -> Macaroon {
        Macaroon::mint(KEY, "v1/session")
            .attenuate("ops=read")
            .attenuate("expires=100")
    }

    #[test]
    fn a_macaroon_verifies_with_its_key_alone() {
        let m = Macaroon::mint(KEY, "v1/session");
        assert_eq!(m.verify(KEY), Ok(()));
        assert_eq!(sample().verify(KEY), Ok(()));
        assert_eq!(sample().verify(b"another key"), Err(Unverified));
        assert_eq!(m.verify(b""), Err(Unverified));
        assert_ne!(
            Macaroon::mint(KEY, "v1/a").encode(P),
            Macaroon::mint(KEY, "v1/b").encode(P)
        );
        assert!(!format!("{:?}", sample()).contains(&base64url(&sample().sig)));
    }

    #[test]
    fn an_appended_caveat_verifies_and_keeps_the_others() {
        let m = sample().attenuate("origin=https://a.example");
        assert_eq!(m.verify(KEY), Ok(()));
        assert_eq!(
            m.caveats(),
            ["ops=read", "expires=100", "origin=https://a.example"]
        );
        let back = Macaroon::decode(P, &m.encode(P)).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.verify(KEY), Ok(()));
    }

    /// Every change of a macaroon's parts, short of knowing the key,
    /// fails: no caveat can be removed, replaced or reordered, the
    /// identifier cannot change, the signature cannot be guessed.
    #[test]
    fn any_change_fails_verification() {
        let m = sample();
        let with = |identifier: &str, caveats: &[&str], sig: [u8; 32]| Macaroon {
            identifier: identifier.into(),
            caveats: caveats.iter().map(|c| (*c).to_string()).collect(),
            sig,
        };
        let mut flipped = m.sig;
        flipped[0] ^= 1;
        let changed = [
            with("v1/other", &["ops=read", "expires=100"], m.sig),
            with("v1/session", &["ops=read,act", "expires=100"], m.sig),
            with("v1/session", &["expires=100", "ops=read"], m.sig),
            with("v1/session", &["ops=read"], m.sig),
            with("v1/session", &["expires=100"], m.sig),
            with("v1/session", &[], m.sig),
            with("v1/session", &["ops=read", "expires=100"], flipped),
            with("v1/session", &["ops=read", "expires=100"], [0; 32]),
            // The signature before the last caveat, the last dropped.
            with(
                "v1/session",
                &["ops=read"],
                Macaroon::mint(KEY, "v1/session").attenuate("ops=read").sig,
            )
            .attenuate("ops=act,read"),
        ];
        for c in &changed[..changed.len() - 1] {
            assert_eq!(c.verify(KEY), Err(Unverified), "{c:?}");
        }
        // Dropping a caveat and appending another is a narrowing of the
        // shorter macaroon, which its holder could make: it verifies, and
        // holds the caveats it shows.
        let last = &changed[changed.len() - 1];
        assert_eq!(last.verify(KEY), Ok(()));
        assert_eq!(last.caveats(), ["ops=read", "ops=act,read"]);
    }

    #[test]
    fn what_does_not_decode_is_refused() {
        let good = sample().encode(P);
        assert!(Macaroon::decode(P, &good).is_some());
        let json = |v: serde_json::Value| format!("{P}{}", base64url(v.to_string().as_bytes()));
        let sig = base64url(&[7u8; 32]);
        for bad in [
            String::new(),
            P.to_string(),
            good.replacen(P, "cxz_", 1),
            format!("{good}!"),
            good[..good.len() - 3].to_string(),
            format!("{P}{}", "A".repeat(MAX_ENCODED_BYTES)),
            json(serde_json::json!({"i": "x", "c": []})),
            json(serde_json::json!({"i": "x", "c": [], "s": base64url(&[7u8; 31])})),
            json(serde_json::json!({"i": "x", "c": [1], "s": sig})),
            json(serde_json::json!({"i": "x", "c": [], "s": sig, "extra": 1})),
            json(serde_json::json!(["x", [], sig])),
        ] {
            assert!(Macaroon::decode(P, &bad).is_none(), "{bad}");
        }
        assert!(
            Macaroon::decode(P, &json(serde_json::json!({"i": "x", "c": [], "s": sig}))).is_some()
        );
    }

    #[test]
    fn a_derived_key_depends_on_its_key_and_label() {
        assert_eq!(derive_key(KEY, "a"), derive_key(KEY, "a"));
        assert_ne!(derive_key(KEY, "a"), derive_key(KEY, "b"));
        assert_ne!(derive_key(KEY, "a"), derive_key(b"other", "a"));
    }
}

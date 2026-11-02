//! The server's half of WebAuthn, as browser confirmations use it
//! (`clax_http::confirm`): checking a passkey's registration and its
//! assertions, with user verification required.
//!
//! The browser hands over what its `PublicKeyCredential` gives without
//! CBOR: the public key as SubjectPublicKeyInfo DER
//! (`AuthenticatorAttestationResponse.getPublicKey()`) with its COSE
//! algorithm (`getPublicKeyAlgorithm()`), the authenticator data, the
//! client data JSON and, for an assertion, the signature. No attestation is
//! checked (the daemon asks for `attestation: "none"`): a passkey is the
//! owner's because the owner's browser registered it, in a confirmed
//! decision. Two algorithms: ES256 (P-256 with SHA-256, the platform
//! authenticators') and EdDSA (Ed25519).
//!
//! Every check is the specification's (WebAuthn level 2, §7.1 and §7.2),
//! as far as it applies here:
//! - the client data's `type`, `challenge` (base64url, as the daemon
//!   issued it) and `origin` (the daemon's own) are the expected ones;
//! - the authenticator data's RP ID hash is SHA-256 of the daemon's host,
//!   and its flags say the user was present (UP) and verified (UV);
//! - a registration's attested credential ID is the one the browser named,
//!   and its attested COSE public key is the key of the SubjectPublicKeyInfo
//!   the browser handed over (so a key cannot be swapped in beside another
//!   authenticator's data);
//! - an assertion's signature verifies over the authenticator data and the
//!   SHA-256 of the client data JSON, and its signature counter is greater
//!   than the one stored, unless both are 0 (an authenticator that keeps
//!   no counter, as synced passkeys do).

use base64::Engine;
use ring::digest::{SHA256, digest};
use ring::signature::{ECDSA_P256_SHA256_ASN1, ED25519, UnparsedPublicKey};
use serde::Deserialize;

/// COSE ES256: ECDSA on P-256 with SHA-256.
pub const ES256: i64 = -7;
/// COSE EdDSA: Ed25519.
pub const EDDSA: i64 = -8;

/// The DER prefix of a P-256 public key's SubjectPublicKeyInfo, before its
/// 65-byte uncompressed point.
pub const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
/// The DER prefix of an Ed25519 public key's SubjectPublicKeyInfo, before
/// its 32-byte key.
pub const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// What the daemon expects of a ceremony: its origin (`http://localhost:<port>`),
/// its RP ID (that origin's host) and the challenge it issued (base64url).
#[derive(Clone, Copy, Debug)]
pub struct Expect<'a> {
    pub origin: &'a str,
    pub rp_id: &'a str,
    pub challenge: &'a str,
}

/// Why a ceremony was refused: a short reason, never a secret.
pub type Refusal = &'static str;

/// Unpadded (or padded) base64url `s` as bytes.
///
/// # Errors
/// For text that is not base64url.
pub fn from_b64url(s: &str) -> Result<Vec<u8>, Refusal> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|_| "not base64url")
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
}

/// The challenge client data JSON `cdj` names, as the daemon issued it.
///
/// # Errors
/// For client data that does not parse.
pub fn challenge_of(cdj: &[u8]) -> Result<String, Refusal> {
    serde_json::from_slice::<ClientData>(cdj)
        .map(|c| c.challenge)
        .map_err(|_| "the client data does not parse")
}

fn check_client_data(cdj: &[u8], kind: &str, e: &Expect<'_>) -> Result<(), Refusal> {
    let c: ClientData =
        serde_json::from_slice(cdj).map_err(|_| "the client data does not parse")?;
    if c.kind != kind {
        return Err("the client data is of another ceremony");
    }
    if c.challenge != e.challenge {
        return Err("the client data names another challenge");
    }
    if c.origin != e.origin {
        return Err("the client data names another origin");
    }
    Ok(())
}

/// Checks authenticator data `ad`'s RP ID hash and its UP and UV flags:
/// its signature counter.
fn check_auth_data(ad: &[u8], e: &Expect<'_>) -> Result<u32, Refusal> {
    if ad.len() < 37 {
        return Err("the authenticator data is too short");
    }
    if ad[..32] != *digest(&SHA256, e.rp_id.as_bytes()).as_ref() {
        return Err("the authenticator data names another RP ID");
    }
    let flags = ad[32];
    if flags & 0x01 == 0 {
        return Err("the user was not present");
    }
    if flags & 0x04 == 0 {
        return Err("the user was not verified");
    }
    Ok(u32::from_be_bytes([ad[33], ad[34], ad[35], ad[36]]))
}

/// The raw key a SubjectPublicKeyInfo `spki` of algorithm `alg` holds.
fn raw_key(spki: &[u8], alg: i64) -> Result<&[u8], Refusal> {
    let (prefix, len): (&[u8], usize) = match alg {
        ES256 => (&P256_SPKI_PREFIX, 65),
        EDDSA => (&ED25519_SPKI_PREFIX, 32),
        _ => return Err("the algorithm is neither ES256 nor EdDSA"),
    };
    match spki.strip_prefix(prefix) {
        Some(k) if k.len() == len => Ok(k),
        _ => Err("the public key is not of its algorithm"),
    }
}

/// A passkey's registration, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    /// The credential's ID, base64url.
    pub credential_id: String,
    /// Its public key, SubjectPublicKeyInfo DER.
    pub public_key: Vec<u8>,
    pub alg: i64,
    pub sign_count: u32,
}

/// Checks a registration (`navigator.credentials.create`): `credential_id`
/// (base64url), public key `spki` of algorithm `alg`, authenticator data
/// `ad` and client data JSON `cdj`, against `e`.
///
/// # Errors
/// Each check that fails, as its [`Refusal`].
pub fn verify_registration(
    e: &Expect<'_>,
    credential_id: &str,
    spki: &[u8],
    alg: i64,
    ad: &[u8],
    cdj: &[u8],
) -> Result<Registered, Refusal> {
    check_client_data(cdj, "webauthn.create", e)?;
    let count = check_auth_data(ad, e)?;
    if ad[32] & 0x40 == 0 || ad.len() < 55 {
        return Err("the authenticator data holds no credential");
    }
    let n = usize::from(u16::from_be_bytes([ad[53], ad[54]]));
    let id = ad
        .get(55..55 + n)
        .ok_or("the authenticator data is too short")?;
    if id != from_b64url(credential_id)?.as_slice() {
        return Err("the authenticator data names another credential");
    }
    let key = raw_key(spki, alg)?;
    let cose = ad
        .get(55 + n..)
        .ok_or("the authenticator data is too short")?;
    if cose_raw_key(cose, alg)? != key {
        return Err("the authenticator data attests another public key");
    }
    Ok(Registered {
        credential_id: crate::secret::base64url(id),
        public_key: spki.to_vec(),
        alg,
        sign_count: count,
    })
}

/// Checks an assertion (`navigator.credentials.get`) by the passkey whose
/// public key is `spki` (algorithm `alg`, last counter `stored`):
/// authenticator data `ad`, client data JSON `cdj` and signature `sig`,
/// against `e`. Its new counter.
///
/// # Errors
/// Each check that fails, as its [`Refusal`].
pub fn verify_assertion(
    e: &Expect<'_>,
    spki: &[u8],
    alg: i64,
    stored: u32,
    ad: &[u8],
    cdj: &[u8],
    sig: &[u8],
) -> Result<u32, Refusal> {
    check_client_data(cdj, "webauthn.get", e)?;
    let count = check_auth_data(ad, e)?;
    let key = raw_key(spki, alg)?;
    let mut signed = ad.to_vec();
    signed.extend_from_slice(digest(&SHA256, cdj).as_ref());
    let verified = match alg {
        ES256 => UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, key).verify(&signed, sig),
        _ => UnparsedPublicKey::new(&ED25519, key).verify(&signed, sig),
    };
    verified.map_err(|_| "the signature does not verify")?;
    if (count != 0 || stored != 0) && count <= stored {
        return Err("the signature counter did not grow: the passkey may be cloned");
    }
    Ok(count)
}

/// One CBOR item of a COSE key map: an integer, or a byte string.
#[derive(Debug, PartialEq)]
enum Cbor<'a> {
    Int(i64),
    Bytes(&'a [u8]),
}

/// The CBOR head at `b[*at..]`: major type and argument; advances `at`.
fn cbor_head(b: &[u8], at: &mut usize) -> Result<(u8, u64), Refusal> {
    let bad = "the attested public key is not a COSE key";
    let first = *b.get(*at).ok_or(bad)?;
    *at += 1;
    let (major, info) = (first >> 5, first & 0x1f);
    let len = match info {
        0..=23 => return Ok((major, u64::from(info))),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        _ => return Err(bad),
    };
    let bytes = b.get(*at..at.checked_add(len).ok_or(bad)?).ok_or(bad)?;
    *at += len;
    Ok((
        major,
        bytes.iter().fold(0u64, |n, x| (n << 8) | u64::from(*x)),
    ))
}

/// The integer or byte string at `b[*at..]`; advances `at`.
fn cbor_item<'a>(b: &'a [u8], at: &mut usize) -> Result<Cbor<'a>, Refusal> {
    let bad = "the attested public key is not a COSE key";
    let (major, n) = cbor_head(b, at)?;
    match major {
        0 => Ok(Cbor::Int(i64::try_from(n).map_err(|_| bad)?)),
        1 => Ok(Cbor::Int(-1 - i64::try_from(n).map_err(|_| bad)?)),
        2 => {
            let n = usize::try_from(n).map_err(|_| bad)?;
            let v = b.get(*at..at.checked_add(n).ok_or(bad)?).ok_or(bad)?;
            *at += n;
            Ok(Cbor::Bytes(v))
        }
        _ => Err(bad),
    }
}

/// The raw key the COSE key map at the start of `cose` holds for
/// algorithm `alg`: an EC2 P-256 key's uncompressed point (`04 ‖ x ‖ y`),
/// an OKP Ed25519 key's `x`. Its `alg`, when given, must be `alg`.
fn cose_raw_key(cose: &[u8], alg: i64) -> Result<Vec<u8>, Refusal> {
    let bad = "the attested public key is not a COSE key of its algorithm";
    let mut at = 0;
    let (major, n) = cbor_head(cose, &mut at)?;
    if major != 5 || n > 16 {
        return Err(bad);
    }
    let (mut kty, mut calg, mut crv, mut x, mut y) = (None, None, None, None, None);
    for _ in 0..n {
        let Cbor::Int(k) = cbor_item(cose, &mut at)? else {
            return Err(bad);
        };
        let v = cbor_item(cose, &mut at)?;
        match (k, v) {
            (1, Cbor::Int(v)) => kty = Some(v),
            (3, Cbor::Int(v)) => calg = Some(v),
            (-1, Cbor::Int(v)) => crv = Some(v),
            (-2, Cbor::Bytes(v)) => x = Some(v),
            (-3, Cbor::Bytes(v)) => y = Some(v),
            _ => {}
        }
    }
    if calg.is_some_and(|a| a != alg) {
        return Err(bad);
    }
    match (alg, kty, crv, x, y) {
        (ES256, Some(2), Some(1), Some(x), Some(y)) if x.len() == 32 && y.len() == 32 => {
            Ok([&[4u8][..], x, y].concat())
        }
        (EDDSA, Some(1), Some(6), Some(x), _) if x.len() == 32 => Ok(x.to_vec()),
        _ => Err(bad),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cose_key_decodes_to_the_raw_key_of_its_algorithm() {
        // OKP Ed25519 {1: 1, 3: -8, -1: 6, -2: x}.
        let ed = [
            &[0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21, 0x58, 32][..],
            &[7; 32],
        ]
        .concat();
        assert_eq!(cose_raw_key(&ed, EDDSA).unwrap(), vec![7; 32]);
        assert!(cose_raw_key(&ed, ES256).is_err());
        // EC2 P-256 {1: 2, 3: -7, -1: 1, -2: x, -3: y}.
        let ec = [
            &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 32][..],
            &[1; 32],
            &[0x22, 0x58, 32],
            &[2; 32],
        ]
        .concat();
        assert_eq!(
            cose_raw_key(&ec, ES256).unwrap(),
            [&[4u8][..], &[1; 32], &[2; 32]].concat()
        );
        assert!(cose_raw_key(&ec, EDDSA).is_err());
    }

    #[test]
    fn a_cose_key_with_a_huge_byte_string_length_is_refused_not_a_panic() {
        // A map of one entry, -2 => a byte string claiming 2^64 - 1 bytes.
        let cose = [
            0xa1, 0x21, 0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ];
        assert!(cose_raw_key(&cose, ES256).is_err());
        assert!(cose_raw_key(&cose, EDDSA).is_err());
    }
}

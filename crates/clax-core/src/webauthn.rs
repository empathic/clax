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
//! - a registration's attested credential ID is the one the browser named;
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

/// `bytes` as unpadded base64url.
pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

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
    raw_key(spki, alg)?;
    Ok(Registered {
        credential_id: b64url(id),
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

/// Authenticator data for `rp_id` with flags `flags`, counter `count` and,
/// for a registration, the attested credential `id` (with an all-zero
/// AAGUID and no COSE key, which these checks do not read).
pub fn auth_data(rp_id: &str, flags: u8, count: u32, id: Option<&[u8]>) -> Vec<u8> {
    let mut ad = digest(&SHA256, rp_id.as_bytes()).as_ref().to_vec();
    ad.push(flags);
    ad.extend_from_slice(&count.to_be_bytes());
    if let Some(id) = id {
        ad.extend_from_slice(&[0; 16]);
        ad.extend_from_slice(&(id.len() as u16).to_be_bytes());
        ad.extend_from_slice(id);
    }
    ad
}

/// Client data JSON for a ceremony of `kind` (`webauthn.create` or
/// `webauthn.get`) over `challenge` at `origin`, as a browser writes it.
pub fn client_data(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
    serde_json::json!({"type": kind, "challenge": challenge, "origin": origin, "crossOrigin": false})
        .to_string()
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair};

    const E: Expect<'static> = Expect {
        origin: "http://localhost:7480",
        rp_id: "localhost",
        challenge: "Y2hhbGxlbmdl",
    };

    fn p256() -> (EcdsaKeyPair, Vec<u8>) {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let mut spki = P256_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(kp.public_key().as_ref());
        (kp, spki)
    }

    fn sign(kp: &EcdsaKeyPair, ad: &[u8], cdj: &[u8]) -> Vec<u8> {
        let mut m = ad.to_vec();
        m.extend_from_slice(digest(&SHA256, cdj).as_ref());
        kp.sign(&SystemRandom::new(), &m).unwrap().as_ref().to_vec()
    }

    #[test]
    fn a_registration_names_its_credential_origin_rp_and_verified_user() {
        let (_, spki) = p256();
        let id = b"cred-1";
        let ad = auth_data("localhost", 0x45, 0, Some(id));
        let cdj = client_data("webauthn.create", E.challenge, E.origin);
        let r = verify_registration(&E, &b64url(id), &spki, ES256, &ad, &cdj).unwrap();
        assert_eq!((r.credential_id, r.sign_count), (b64url(id), 0));
        let wrong = |ad: &[u8], cdj: &[u8], id: &[u8]| {
            verify_registration(&E, &b64url(id), &spki, ES256, ad, cdj).unwrap_err()
        };
        let other = client_data("webauthn.create", E.challenge, "http://evil.example");
        assert!(wrong(&ad, &other, id).contains("origin"));
        let other = client_data("webauthn.create", "b3RoZXI", E.origin);
        assert!(wrong(&ad, &other, id).contains("challenge"));
        assert!(wrong(&auth_data("evil.example", 0x45, 0, Some(id)), &cdj, id).contains("RP ID"));
        assert!(wrong(&auth_data("localhost", 0x41, 0, Some(id)), &cdj, id).contains("verified"));
        assert!(wrong(&ad, &cdj, b"cred-2").contains("another credential"));
    }

    #[test]
    fn an_assertion_verifies_its_signature_and_a_growing_counter() {
        let (kp, spki) = p256();
        let cdj = client_data("webauthn.get", E.challenge, E.origin);
        let ad = auth_data("localhost", 0x05, 3, None);
        let sig = sign(&kp, &ad, &cdj);
        assert_eq!(
            verify_assertion(&E, &spki, ES256, 2, &ad, &cdj, &sig),
            Ok(3)
        );
        // A counter that did not grow, a signature by another key, another ceremony.
        assert!(
            verify_assertion(&E, &spki, ES256, 3, &ad, &cdj, &sig)
                .unwrap_err()
                .contains("counter")
        );
        let (_, other) = p256();
        assert!(
            verify_assertion(&E, &other, ES256, 0, &ad, &cdj, &sig)
                .unwrap_err()
                .contains("signature")
        );
        let create = client_data("webauthn.create", E.challenge, E.origin);
        assert!(verify_assertion(&E, &spki, ES256, 0, &ad, &create, &sig).is_err());
        // An authenticator that keeps no counter (0 and 0) is accepted.
        let ad0 = auth_data("localhost", 0x05, 0, None);
        let sig0 = sign(&kp, &ad0, &cdj);
        assert_eq!(
            verify_assertion(&E, &spki, ES256, 0, &ad0, &cdj, &sig0),
            Ok(0)
        );
    }

    #[test]
    fn an_ed25519_assertion_verifies() {
        let rng = SystemRandom::new();
        let kp = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref())
            .unwrap();
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(kp.public_key().as_ref());
        let cdj = client_data("webauthn.get", E.challenge, E.origin);
        let ad = auth_data("localhost", 0x05, 1, None);
        let mut m = ad.clone();
        m.extend_from_slice(digest(&SHA256, &cdj).as_ref());
        let sig = kp.sign(&m);
        assert_eq!(
            verify_assertion(&E, &spki, EDDSA, 0, &ad, &cdj, sig.as_ref()),
            Ok(1)
        );
        assert!(verify_assertion(&E, &spki, ES256, 0, &ad, &cdj, sig.as_ref()).is_err());
    }
}

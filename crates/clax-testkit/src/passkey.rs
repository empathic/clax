//! A software passkey (P-256, ES256) for the tests of browser
//! confirmations' passkeys (`clax_http::confirm`): it registers and
//! asserts as a platform authenticator would, user verified, with no
//! hardware, and its signature counter is set by the test.

use super::TestServer;
use clax_core::secret::base64url as b64url;
use clax_core::webauthn::{ED25519_SPKI_PREFIX, ES256, P256_SPKI_PREFIX};
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};

/// A passkey whose private key lives in the test.
pub struct SoftPasskey {
    key: EcdsaKeyPair,
    /// Its credential ID.
    pub id: Vec<u8>,
}

/// UP (user present) and UV (user verified).
const UP_UV: u8 = 0x05;

impl Default for SoftPasskey {
    fn default() -> Self {
        SoftPasskey::new()
    }
}

impl SoftPasskey {
    /// A new key with a random credential ID.
    pub fn new() -> SoftPasskey {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        SoftPasskey {
            key,
            id: clax_core::secret::random_bytes::<16>().to_vec(),
        }
    }

    /// A new key that claims credential ID `id`: what an agent forges to
    /// stand in for another passkey.
    pub fn claiming(id: &[u8]) -> SoftPasskey {
        SoftPasskey {
            id: id.to_vec(),
            ..SoftPasskey::new()
        }
    }

    /// Its credential ID, base64url.
    pub fn credential_id(&self) -> String {
        b64url(&self.id)
    }

    /// `POST /api/passkeys`'s body: a registration over `challenge` at
    /// `origin` for `rp_id`.
    pub fn registration(&self, challenge: &str, origin: &str, rp_id: &str) -> Value {
        let mut spki = P256_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(self.key.public_key().as_ref());
        json!({
            "credential_id": self.credential_id(),
            "public_key": b64url(&spki),
            "alg": ES256,
            "authenticator_data": b64url(&auth_data(
                rp_id,
                UP_UV | 0x40,
                0,
                Some((&self.id, &cose_key(&spki, ES256))),
            )),
            "client_data_json": b64url(&client_data("webauthn.create", challenge, origin)),
        })
    }

    /// An assertion over `challenge` at `origin` for `rp_id`, with
    /// signature counter `count`.
    pub fn assertion(&self, challenge: &str, origin: &str, rp_id: &str, count: u32) -> Value {
        let ad = auth_data(rp_id, UP_UV, count, None);
        let cdj = client_data("webauthn.get", challenge, origin);
        let mut signed = ad.clone();
        signed.extend_from_slice(digest(&SHA256, &cdj).as_ref());
        let sig = self.key.sign(&SystemRandom::new(), &signed).unwrap();
        json!({
            "credential_id": self.credential_id(),
            "authenticator_data": b64url(&ad),
            "client_data_json": b64url(&client_data("webauthn.get", challenge, origin)),
            "signature": b64url(sig.as_ref()),
        })
    }
}

impl TestServer {
    /// The passkeys' origin and RP ID here (`GET /api/passkeys`).
    pub async fn passkey_rp(&self) -> (String, String) {
        let (s, v) = self
            .as_browser(
                &self.owner_cookie(),
                reqwest::Method::GET,
                "/api/passkeys",
                Value::Null,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        (
            v["origin"]
                .as_str()
                .expect("passkeys are available")
                .to_string(),
            v["rp_id"].as_str().unwrap().to_string(),
        )
    }

    /// A challenge for `body` (`{purpose, …}`) from the browser holding
    /// `cookie`.
    pub async fn passkey_challenge(&self, cookie: &str, body: Value) -> String {
        let (s, v) = self
            .as_browser(
                cookie,
                reqwest::Method::POST,
                "/api/passkeys/challenge",
                body,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["challenge"].as_str().unwrap().to_string()
    }

    /// Registers `key` from the owner's browser and requires passkeys to
    /// confirm, while no agent is live.
    pub async fn require_passkey(&self, key: &SoftPasskey) {
        let (origin, rp) = self.passkey_rp().await;
        let c = self
            .passkey_challenge(&self.owner_cookie(), json!({"purpose": "register"}))
            .await;
        let (s, v) = self
            .as_browser(
                &self.owner_cookie(),
                reqwest::Method::POST,
                "/api/passkeys",
                key.registration(&c, &origin, &rp),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let (s, v) = self
            .as_browser(
                &self.owner_cookie(),
                reqwest::Method::PUT,
                "/api/passkeys/settings",
                json!({"required": true}),
            )
            .await;
        assert_eq!((s, v["required"].as_bool()), (200, Some(true)), "{v}");
    }
}

/// The COSE key map of SubjectPublicKeyInfo `spki` of algorithm `alg`, as
/// an authenticator attests it: EC2 P-256 `{1: 2, 3: -7, -1: 1, -2: x,
/// -3: y}` for ES256, else OKP Ed25519 `{1: 1, 3: -8, -1: 6, -2: x}`.
fn cose_key(spki: &[u8], alg: i64) -> Vec<u8> {
    let bstr = |v: &[u8]| [&[0x58, v.len() as u8][..], v].concat();
    if alg == ES256 {
        let key = spki.strip_prefix(&P256_SPKI_PREFIX[..]).unwrap();
        [
            &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21][..],
            &bstr(&key[1..33]),
            &[0x22],
            &bstr(&key[33..65]),
        ]
        .concat()
    } else {
        let key = spki.strip_prefix(&ED25519_SPKI_PREFIX[..]).unwrap();
        [
            &[0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21][..],
            &bstr(key),
        ]
        .concat()
    }
}

/// Authenticator data for `rp_id` with flags `flags`, counter `count` and,
/// for a registration, the attested credential: its `id` and COSE key
/// `cose` ([`cose_key`]), with an all-zero AAGUID.
fn auth_data(rp_id: &str, flags: u8, count: u32, attested: Option<(&[u8], &[u8])>) -> Vec<u8> {
    let mut ad = digest(&SHA256, rp_id.as_bytes()).as_ref().to_vec();
    ad.push(flags);
    ad.extend_from_slice(&count.to_be_bytes());
    if let Some((id, cose)) = attested {
        ad.extend_from_slice(&[0; 16]);
        ad.extend_from_slice(&(id.len() as u16).to_be_bytes());
        ad.extend_from_slice(id);
        ad.extend_from_slice(cose);
    }
    ad
}

/// Client data JSON for a ceremony of `kind` (`webauthn.create` or
/// `webauthn.get`) over `challenge` at `origin`, as a browser writes it.
fn client_data(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
    json!({"type": kind, "challenge": challenge, "origin": origin, "crossOrigin": false})
        .to_string()
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::webauthn::{EDDSA, Expect, verify_assertion, verify_registration};
    use ring::signature::Ed25519KeyPair;

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
        let cose = cose_key(&spki, ES256);
        let ad = auth_data("localhost", 0x45, 0, Some((id, &cose)));
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
        assert!(
            wrong(
                &auth_data("evil.example", 0x45, 0, Some((id, &cose))),
                &cdj,
                id
            )
            .contains("RP ID")
        );
        assert!(
            wrong(
                &auth_data("localhost", 0x41, 0, Some((id, &cose))),
                &cdj,
                id
            )
            .contains("verified")
        );
        assert!(wrong(&ad, &cdj, b"cred-2").contains("another credential"));
        // Another key beside this one's authenticator data.
        let (_, other) = p256();
        let swapped = verify_registration(&E, &b64url(id), &other, ES256, &ad, &cdj).unwrap_err();
        assert!(swapped.contains("another public key"), "{swapped}");
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

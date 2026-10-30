//! A software passkey (P-256, ES256) for the tests of browser
//! confirmations' passkeys (`clax_http::confirm`): it registers and
//! asserts as a platform authenticator would, user verified, with no
//! hardware, and its signature counter is set by the test.

use super::TestServer;
use clax_core::webauthn::{P256_SPKI_PREFIX, auth_data, b64url, client_data, cose_key};
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
            "alg": clax_core::webauthn::ES256,
            "authenticator_data": b64url(&auth_data(
                rp_id,
                UP_UV | 0x40,
                0,
                Some((&self.id, &cose_key(&spki, clax_core::webauthn::ES256).unwrap())),
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
    /// `method path` from the browser holding `cookie`, with JSON `body`:
    /// the status and the answer.
    pub async fn as_browser(
        &self,
        cookie: &str,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        let res = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("cookie", cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(Value::Null))
    }

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

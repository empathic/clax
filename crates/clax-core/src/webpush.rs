//! Web Push to the owner's devices: the daemon's VAPID key (RFC 8292), the
//! push subscriptions browsers hand it, and each message as the request a
//! push service takes (RFC 8030), its payload encrypted for the one
//! subscription (RFC 8291, `aes128gcm` of RFC 8188). The cryptography is
//! ring's, which the workspace already builds: ECDSA P-256 for the VAPID
//! token, ephemeral P-256 ECDH, HKDF-SHA-256 and AES-128-GCM.
//!
//! The key and the subscriptions are files of the home ([`Home::push_dir`],
//! 0700; each file 0600). Sending is the daemon's; nothing here does I/O
//! but those files.

use crate::home::Home;
use crate::remote::write_private;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use ring::{aead, agreement, hkdf};
use serde::{Deserialize, Serialize};

/// Why a key, a subscription or a message was refused.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// A subscription or payload that cannot be used, and why.
    #[error("{0}")]
    Invalid(String),
    /// The key file could not be read or written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// ring refused an operation (a key it cannot parse, a failed seal).
    #[error("cryptography failed: {0}")]
    Crypto(&'static str),
}

type Result<T> = std::result::Result<T, PushError>;

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url as browsers write a subscription's keys, with or without `=`
/// padding.
fn unb64(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok()
}

/// The record size every message declares: one record carries it.
const RECORD_SIZE: u32 = 4096;
/// The longest payload a message carries: one record with its delimiter and
/// tag, inside [`RECORD_SIZE`].
pub const MAX_PAYLOAD: usize = 3000;
/// The most subscriptions the home keeps; adding one past it drops the
/// oldest.
pub const MAX_SUBSCRIPTIONS: usize = 20;
/// How long a VAPID token is good for (RFC 8292 §2: at most 24 hours).
pub const TOKEN_LIFETIME_S: u64 = 12 * 60 * 60;
/// The VAPID `sub` claim: who runs the sender. Apple's push service refuses
/// a token without one; it must be an `https:` or `mailto:` URI.
pub const SUBJECT: &str = "https://github.com/empathic/clax";

/// The daemon's VAPID key: an ECDSA P-256 key whose public half browsers
/// subscribe with (`applicationServerKey`) and which signs each request's
/// token.
pub struct Vapid {
    key: EcdsaKeyPair,
}

impl Vapid {
    /// A new key, as PKCS#8.
    pub fn generate() -> Result<Vec<u8>> {
        let doc =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| PushError::Crypto("generate the VAPID key"))?;
        Ok(doc.as_ref().to_vec())
    }

    /// The key in `pkcs8`.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Vapid> {
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| PushError::Crypto("read the VAPID key"))?;
        Ok(Vapid { key })
    }

    /// The home's key (`push/vapid.key`), made on first use.
    pub fn load_or_create(home: &Home) -> Result<Vapid> {
        let path = home.push_vapid_key();
        match std::fs::read(&path) {
            Ok(bytes) => Vapid::from_pkcs8(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let pkcs8 = Vapid::generate()?;
                write_private(&path, &pkcs8)?;
                Vapid::from_pkcs8(&pkcs8)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The public key, uncompressed (65 bytes), base64url: what a browser's
    /// `pushManager.subscribe` takes as `applicationServerKey`.
    pub fn public_key(&self) -> String {
        b64(self.key.public_key().as_ref())
    }

    /// The `Authorization` header for a request to `endpoint` (RFC 8292
    /// §3): `vapid t=<ES256 JWT for the endpoint's origin, expiring at
    /// expires (Unix seconds)>, k=<public key>`.
    pub fn authorization(&self, endpoint: &url::Url, expires: u64) -> Result<String> {
        let aud = endpoint.origin().ascii_serialization();
        let header = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({"aud": aud, "exp": expires, "sub": SUBJECT});
        let claims = b64(claims.to_string().as_bytes());
        let signing = format!("{header}.{claims}");
        let sig = self
            .key
            .sign(&SystemRandom::new(), signing.as_bytes())
            .map_err(|_| PushError::Crypto("sign the VAPID token"))?;
        Ok(format!(
            "vapid t={signing}.{}, k={}",
            b64(sig.as_ref()),
            self.public_key()
        ))
    }
}

/// A browser's push subscription, as `PushSubscription.toJSON()` gives it:
/// the push service's URL for this device and the keys its payloads are
/// encrypted to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    /// The device's P-256 public key, uncompressed, base64url.
    pub p256dh: String,
    /// The 16-byte authentication secret, base64url.
    pub auth: String,
    /// When it was stored (RFC 3339).
    #[serde(default)]
    pub created_at: String,
}

/// A subscription's keys, decoded.
struct Keys {
    p256dh: Vec<u8>,
    auth: Vec<u8>,
}

impl Subscription {
    /// The subscription a browser sent, checked: an `https` endpoint (or
    /// `http` to a loopback host, a test's push service), a 65-byte
    /// uncompressed P-256 key and a 16-byte secret.
    pub fn new(
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        created_at: String,
    ) -> Result<Subscription> {
        let s = Subscription {
            endpoint: endpoint.to_string(),
            p256dh: p256dh.trim_end_matches('=').to_string(),
            auth: auth.trim_end_matches('=').to_string(),
            created_at,
        };
        s.url()?;
        s.keys()?;
        Ok(s)
    }

    /// The endpoint, if it is one a message may go to.
    pub fn url(&self) -> Result<url::Url> {
        let u = url::Url::parse(&self.endpoint)
            .map_err(|_| PushError::Invalid("endpoint is not a URL".into()))?;
        let loopback = match u.host() {
            Some(url::Host::Domain(d)) => d == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        let ok = match u.scheme() {
            "https" => u.host().is_some(),
            "http" => loopback,
            _ => false,
        };
        if !ok || !u.username().is_empty() || u.password().is_some() {
            return Err(PushError::Invalid(
                "endpoint must be an https URL with no credentials".into(),
            ));
        }
        Ok(u)
    }

    fn keys(&self) -> Result<Keys> {
        let p256dh = unb64(&self.p256dh)
            .filter(|k| k.len() == 65 && k[0] == 4)
            .ok_or_else(|| {
                PushError::Invalid("keys.p256dh is not an uncompressed P-256 key".into())
            })?;
        let auth = unb64(&self.auth)
            .filter(|k| k.len() == 16)
            .ok_or_else(|| PushError::Invalid("keys.auth is not a 16-byte secret".into()))?;
        Ok(Keys { p256dh, auth })
    }
}

/// The home's subscriptions (`push/subscriptions.json`): none when the file
/// is missing.
pub fn load_subscriptions(home: &Home) -> Result<Vec<Subscription>> {
    match std::fs::read(home.push_subscriptions()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| PushError::Invalid(format!("push/subscriptions.json: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_subscriptions(home: &Home, subs: &[Subscription]) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(subs).expect("subscriptions serialize");
    bytes.push(b'\n');
    write_private(&home.push_subscriptions(), &bytes)?;
    Ok(())
}

/// Keeps `sub`, replacing one of the same endpoint (a device subscribing
/// again), and drops the oldest past [`MAX_SUBSCRIPTIONS`]. The number kept.
pub fn add_subscription(home: &Home, sub: Subscription) -> Result<usize> {
    let mut subs = load_subscriptions(home)?;
    subs.retain(|s| s.endpoint != sub.endpoint);
    subs.push(sub);
    let excess = subs.len().saturating_sub(MAX_SUBSCRIPTIONS);
    subs.drain(..excess);
    save_subscriptions(home, &subs)?;
    Ok(subs.len())
}

/// Forgets the subscription of `endpoint`; whether there was one.
pub fn remove_subscription(home: &Home, endpoint: &str) -> Result<bool> {
    let mut subs = load_subscriptions(home)?;
    let before = subs.len();
    subs.retain(|s| s.endpoint != endpoint);
    if subs.len() == before {
        return Ok(false);
    }
    save_subscriptions(home, &subs)?;
    Ok(true)
}

/// What a push carries: a one-line title, the shell URL a tap opens (a path
/// such as `/a/<id>?thread=<id>`), and a tag, so a newer message about the
/// same thing replaces the older one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub title: String,
    pub url: String,
    pub tag: String,
}

/// A request for a push service: `POST endpoint` with these headers and
/// body.
#[derive(Debug)]
pub struct PushRequest {
    pub endpoint: url::Url,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// How long a push service keeps an undelivered message (`TTL`, seconds).
pub const TTL_S: u64 = 24 * 60 * 60;

/// The request that delivers `payload` to `sub`, signed by `vapid`, whose
/// token expires at `expires` (Unix seconds).
pub fn request(
    vapid: &Vapid,
    sub: &Subscription,
    payload: &[u8],
    expires: u64,
) -> Result<PushRequest> {
    let endpoint = sub.url()?;
    let keys = sub.keys()?;
    let body = encrypt(&keys.p256dh, &keys.auth, payload)?;
    Ok(PushRequest {
        headers: vec![
            ("authorization", vapid.authorization(&endpoint, expires)?),
            ("content-encoding", "aes128gcm".into()),
            ("content-type", "application/octet-stream".into()),
            ("ttl", TTL_S.to_string()),
            ("urgency", "high".into()),
        ],
        endpoint,
        body,
    })
}

/// HKDF output of `n` bytes.
struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[&[u8]], n: usize) -> Result<Vec<u8>> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let mut out = vec![0; n];
    prk.expand(info, Len(n))
        .and_then(|okm| okm.fill(&mut out))
        .map_err(|_| PushError::Crypto("derive a key"))?;
    Ok(out)
}

/// The content key and nonce of RFC 8291 §3.4 and RFC 8188 §2.2.
fn derive(
    ecdh_secret: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let ikm = hkdf(
        auth,
        ecdh_secret,
        &[b"WebPush: info\0", ua_public, as_public],
        32,
    )?;
    let cek = hkdf(salt, &ikm, &[b"Content-Encoding: aes128gcm\0"], 16)?;
    let nonce = hkdf(salt, &ikm, &[b"Content-Encoding: nonce\0"], 12)?;
    Ok((cek, nonce))
}

/// The `aes128gcm` body of RFC 8291 §4 for `plaintext`, from the parts of
/// one message: the shared secret, the subscription's secret and key, the
/// sender's ephemeral public key and the salt. One record, ending with its
/// delimiter (2), no padding.
fn seal(
    ecdh_secret: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let (cek, nonce) = derive(ecdh_secret, auth, ua_public, as_public, salt)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| PushError::Crypto("key"))?,
    );
    let nonce =
        aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| PushError::Crypto("nonce"))?;
    let mut record = plaintext.to_vec();
    record.push(2);
    key.seal_in_place_append_tag(nonce, aead::Aad::empty(), &mut record)
        .map_err(|_| PushError::Crypto("seal"))?;
    let mut body = Vec::with_capacity(21 + as_public.len() + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(u8::try_from(as_public.len()).map_err(|_| PushError::Crypto("key ID"))?);
    body.extend_from_slice(as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// `plaintext` encrypted for the subscription whose key is `ua_public`
/// and secret `auth`, with a new ephemeral key and salt (RFC 8291).
pub fn encrypt(ua_public: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_PAYLOAD {
        return Err(PushError::Invalid(format!(
            "a payload is at most {MAX_PAYLOAD} bytes"
        )));
    }
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| PushError::Crypto("salt"))?;
    let ephemeral = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| PushError::Crypto("ephemeral key"))?;
    let as_public = ephemeral
        .compute_public_key()
        .map_err(|_| PushError::Crypto("ephemeral key"))?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public);
    let secret = agreement::agree_ephemeral(ephemeral, &peer, |s| s.to_vec())
        .map_err(|_| PushError::Invalid("keys.p256dh is not a point on P-256".into()))?;
    seal(
        &secret,
        auth,
        ua_public,
        as_public.as_ref(),
        &salt,
        plaintext,
    )
}

/// A device's side of one message, as a browser holds a subscription's
/// keys: its key pair is ring's ephemeral kind, so it opens one message.
/// For tests of what the daemon sends.
pub struct Receiver {
    key: agreement::EphemeralPrivateKey,
    public: Vec<u8>,
    auth: [u8; 16],
}

impl Receiver {
    pub fn new() -> Receiver {
        let rng = SystemRandom::new();
        let key = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
            .expect("a P-256 key");
        let public = key
            .compute_public_key()
            .expect("its public key")
            .as_ref()
            .to_vec();
        let mut auth = [0u8; 16];
        rng.fill(&mut auth).expect("random bytes");
        Receiver { key, public, auth }
    }

    /// The subscription keys a browser would send: `p256dh` and `auth`,
    /// base64url.
    pub fn keys(&self) -> (String, String) {
        (b64(&self.public), b64(&self.auth))
    }

    /// The plaintext of `body`, an `aes128gcm` message to this device.
    pub fn open(self, body: &[u8]) -> Result<Vec<u8>> {
        let bad = || PushError::Invalid("not an aes128gcm message".into());
        let (salt, rest) = body.split_at_checked(16).ok_or_else(bad)?;
        let (rs, rest) = rest.split_at_checked(4).ok_or_else(bad)?;
        if rs != RECORD_SIZE.to_be_bytes() {
            return Err(bad());
        }
        let (&idlen, rest) = rest.split_first().ok_or_else(bad)?;
        let (as_public, record) = rest.split_at_checked(usize::from(idlen)).ok_or_else(bad)?;
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public);
        let secret =
            agreement::agree_ephemeral(self.key, &peer, |s| s.to_vec()).map_err(|_| bad())?;
        let (cek, nonce) = derive(&secret, &self.auth, &self.public, as_public, salt)?;
        let key = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| bad())?,
        );
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| bad())?;
        let mut record = record.to_vec();
        let plain = key
            .open_in_place(nonce, aead::Aad::empty(), &mut record)
            .map_err(|_| bad())?;
        match plain.iter().rposition(|&b| b != 0) {
            Some(end) if plain[end] == 2 => Ok(plain[..end].to_vec()),
            _ => Err(bad()),
        }
    }
}

impl Default for Receiver {
    fn default() -> Self {
        Receiver::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Vec<u8> {
        unb64(s).unwrap()
    }

    /// RFC 8291 Appendix A, from the ECDH secret on (ring computes no
    /// shared secret from a fixed private key).
    #[test]
    fn the_rfc_8291_example_encrypts_byte_for_byte() {
        let ua_public = d(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let as_public = d(
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
        );
        let auth = d("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = d("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let ecdh = d("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs");
        let (cek, nonce) = derive(&ecdh, &auth, &ua_public, &as_public, &salt).unwrap();
        assert_eq!(b64(&cek), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(b64(&nonce), "4h_95klXJ5E_qnoN");
        let body = seal(
            &ecdh,
            &auth,
            &ua_public,
            &as_public,
            &salt,
            b"When I grow up, I want to be a watermelon",
        )
        .unwrap();
        let (header, ciphertext) = body.split_at(86);
        assert_eq!(
            b64(header),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        assert_eq!(
            b64(ciphertext),
            "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ"
        );
    }

    #[test]
    fn a_device_opens_what_was_encrypted_for_it() {
        let r = Receiver::new();
        let (p256dh, auth) = r.keys();
        let body = encrypt(&d(&p256dh), &d(&auth), b"{\"title\":\"x\"}").unwrap();
        assert_eq!(r.open(&body).unwrap(), b"{\"title\":\"x\"}");
        // Another device's keys cannot.
        let other = Receiver::new();
        assert!(other.open(&body).is_err());
    }

    #[test]
    fn only_a_usable_subscription_is_kept() {
        let r = Receiver::new();
        let (p, a) = r.keys();
        let ok = |e: &str| Subscription::new(e, &p, &a, String::new()).is_ok();
        assert!(ok("https://web.push.apple.com/QGuQyavXutnMH"));
        assert!(ok("http://127.0.0.1:9/push"));
        assert!(ok("http://localhost:9/push"));
        assert!(
            !ok("http://web.push.apple.com/x"),
            "plain http off loopback"
        );
        assert!(!ok("https://user:pw@web.push.apple.com/x"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("not a url"));
        let e = "https://push.example/x";
        assert!(
            Subscription::new(e, &p, "AAAA", String::new()).is_err(),
            "short auth"
        );
        assert!(
            Subscription::new(e, &a, &a, String::new()).is_err(),
            "short key"
        );
        // Padding, as some browsers write it, is accepted.
        assert!(Subscription::new(e, &format!("{p}="), &format!("{a}=="), String::new()).is_ok());
    }

    #[test]
    fn the_vapid_token_is_an_es256_jwt_for_the_endpoints_origin() {
        let v = Vapid::from_pkcs8(&Vapid::generate().unwrap()).unwrap();
        let k = d(&v.public_key());
        assert_eq!((k.len(), k[0]), (65, 4));
        let url = url::Url::parse("https://web.push.apple.com:443/abc?x=1").unwrap();
        let h = v.authorization(&url, 1_800_000_000).unwrap();
        let (t, key) = h
            .strip_prefix("vapid t=")
            .unwrap()
            .split_once(", k=")
            .unwrap();
        assert_eq!(key, v.public_key());
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value = serde_json::from_slice(&d(parts[1])).unwrap();
        assert_eq!(
            claims,
            serde_json::json!({"aud": "https://web.push.apple.com", "exp": 1_800_000_000u64, "sub": SUBJECT})
        );
        let signed = format!("{}.{}", parts[0], parts[1]);
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, &k)
            .verify(signed.as_bytes(), &d(parts[2]))
            .expect("the token verifies with the public key");
    }

    #[test]
    fn the_key_and_subscriptions_are_owner_only_files_of_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().to_path_buf());
        let a = Vapid::load_or_create(&home).unwrap().public_key();
        assert_eq!(
            Vapid::load_or_create(&home).unwrap().public_key(),
            a,
            "made once"
        );
        let r = Receiver::new();
        let (p, k) = r.keys();
        let sub = |e: &str| Subscription::new(e, &p, &k, "t".into()).unwrap();
        assert_eq!(
            add_subscription(&home, sub("https://p.example/1")).unwrap(),
            1
        );
        assert_eq!(
            add_subscription(&home, sub("https://p.example/1")).unwrap(),
            1,
            "same device"
        );
        assert_eq!(
            add_subscription(&home, sub("https://p.example/2")).unwrap(),
            2
        );
        for i in 0..MAX_SUBSCRIPTIONS {
            add_subscription(&home, sub(&format!("https://p.example/n{i}"))).unwrap();
        }
        let subs = load_subscriptions(&home).unwrap();
        assert_eq!(subs.len(), MAX_SUBSCRIPTIONS);
        assert!(
            !subs.iter().any(|s| s.endpoint.ends_with("/1")),
            "the oldest went"
        );
        assert!(remove_subscription(&home, "https://p.example/n0").unwrap());
        assert!(!remove_subscription(&home, "https://p.example/n0").unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.push_dir()), 0o700);
            assert_eq!(mode(&home.push_vapid_key()), 0o600);
            assert_eq!(mode(&home.push_subscriptions()), 0o600);
        }
    }

    #[test]
    fn a_request_carries_the_encrypted_payload_and_the_token() {
        let v = Vapid::from_pkcs8(&Vapid::generate().unwrap()).unwrap();
        let r = Receiver::new();
        let (p, k) = r.keys();
        let sub = Subscription::new("https://push.example/d", &p, &k, String::new()).unwrap();
        let req = request(&v, &sub, b"hello", 1).unwrap();
        let h = |n: &str| req.headers.iter().find(|(k, _)| *k == n).unwrap().1.clone();
        assert_eq!(h("content-encoding"), "aes128gcm");
        assert_eq!(h("ttl"), TTL_S.to_string());
        assert!(h("authorization").starts_with("vapid t="));
        assert_eq!(r.open(&req.body).unwrap(), b"hello");
        assert!(request(&v, &sub, &[b'x'; MAX_PAYLOAD + 1], 1).is_err());
    }
}

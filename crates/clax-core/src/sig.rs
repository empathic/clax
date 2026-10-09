//! Key-bound agent requests, `clax-ed25519` (spec §6.6, M5).
//!
//! A connected machine holds an Ed25519 [`KeyPair`] and signs every request
//! over a fixed canonical string ([`SignInput::canonical`]): the method, the
//! path and query as sent, the `Host`, the time `t`, a nonce, the SHA-256 of
//! the body and the SHA-256 of the access token (an empty line for the grant
//! request, which has no token yet). The signature travels in
//!
//! ```text
//! Clax-Signature: t=<unix seconds>, n=<22 base64url chars>, s=<86 base64url chars>
//! ```
//!
//! which [`parse_header`] reads strictly. The daemon tests `t` and the nonce
//! with [`NonceCache::check`] before reading the body, verifies the signature
//! with [`verify`], and only then spends the nonce with
//! [`NonceCache::record`], so a bad signature spends nothing.

use crate::secret::{hex, random_bytes};
use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

/// First line of the canonical string.
pub const VERSION_LINE: &str = "clax-ed25519 v1";
/// Seconds `t` may differ from the daemon's clock: half of [`NONCE_WINDOW_S`],
/// as [`NonceCache`] takes it.
pub const MAX_SKEW_S: i64 = 120;
/// Seconds a verified nonce is remembered for its key.
pub const NONCE_WINDOW_S: i64 = 240;
/// Verified nonces remembered per key within [`NONCE_WINDOW_S`].
pub const NONCES_PER_KEY: usize = 65_536;
/// Length of a nonce: 16 random bytes in base64url.
pub const NONCE_LEN: usize = 22;
/// Length of a public key in bytes.
pub const PUBLIC_LEN: usize = 32;
/// Length of a signature in bytes.
pub const SIG_LEN: usize = 64;
/// Longest `Clax-Signature` value [`parse_header`] reads.
const MAX_HEADER_LEN: usize = 256;

/// Why a signed request is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SigError {
    /// `t` is more than [`MAX_SKEW_S`] from the daemon's clock.
    #[error("the request's time is too far from the daemon's clock")]
    Stale,
    /// The nonce was already used by this key within the window.
    #[error("the request's nonce was already used")]
    Replayed,
    /// The key has used [`NONCES_PER_KEY`] nonces within the window.
    #[error("too many requests from this key")]
    Full,
    /// A malformed header, key or signature, or one that does not verify.
    #[error("the request's signature is malformed or does not verify")]
    Bad,
}

impl SigError {
    /// The API error code for this refusal (spec §6.6).
    pub fn code(self) -> &'static str {
        match self {
            SigError::Stale => "stale_request",
            SigError::Replayed => "replayed_request",
            SigError::Full => "limit_reached",
            SigError::Bad => "invalid_signature",
        }
    }
}

/// A key pair's PKCS#8 (v2) document: private key material. Its `Debug`
/// prints `Pkcs8(…)` and it has no `PartialEq`, `Display` or `Serialize`.
#[derive(Clone)]
pub struct Pkcs8(Vec<u8>);

impl Pkcs8 {
    /// The document's bytes, for writing to the key file.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Pkcs8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pkcs8(…)")
    }
}

/// An Ed25519 key pair. Its `Debug` prints only the public key's
/// [`fingerprint`].
pub struct KeyPair {
    pair: Ed25519KeyPair,
    pkcs8: Pkcs8,
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyPair({})", fingerprint(&self.public()))
    }
}

impl KeyPair {
    /// A new key pair from the operating system's random source.
    pub fn generate() -> KeyPair {
        let doc = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .expect("the operating system's random source is available");
        KeyPair::from_pkcs8(doc.as_ref()).expect("a generated key pair parses")
    }

    /// The key pair in PKCS#8 v2 `doc`, as [`KeyPair::pkcs8`] wrote it.
    ///
    /// # Errors
    /// [`SigError::Bad`] for anything else.
    pub fn from_pkcs8(doc: &[u8]) -> Result<KeyPair, SigError> {
        let pair = Ed25519KeyPair::from_pkcs8(doc).map_err(|_| SigError::Bad)?;
        Ok(KeyPair {
            pair,
            pkcs8: Pkcs8(doc.to_vec()),
        })
    }

    /// The private key's PKCS#8 v2 document.
    pub fn pkcs8(&self) -> &Pkcs8 {
        &self.pkcs8
    }

    /// The public key.
    pub fn public(&self) -> [u8; PUBLIC_LEN] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("an Ed25519 public key is 32 bytes")
    }

    /// Signs `input`, giving the `Clax-Signature` header.
    pub fn sign(&self, input: &SignInput<'_>) -> SigHeader {
        let sig = self.pair.sign(&input.canonical());
        SigHeader {
            t: input.t,
            nonce: input.nonce.to_owned(),
            sig: sig
                .as_ref()
                .try_into()
                .expect("an Ed25519 signature is 64 bytes"),
        }
    }
}

/// What a request signs.
#[derive(Debug, Clone, Copy)]
pub struct SignInput<'a> {
    pub method: &'a str,
    /// The path and query exactly as sent on the request line.
    pub path_and_query: &'a str,
    /// The `Host` header as sent.
    pub host: &'a str,
    /// Unix seconds.
    pub t: i64,
    pub nonce: &'a str,
    pub body_sha256: [u8; 32],
    /// SHA-256 of the access token; `None` for the grant request, which
    /// signs an empty token line.
    pub token_sha256: Option<[u8; 32]>,
}

impl SignInput<'_> {
    /// The signed message: the UTF-8 of [`VERSION_LINE`], the method, the
    /// path and query, the host, `t`, the nonce, the body's SHA-256 in
    /// lowercase hex and the token's (or nothing), joined by `\n`, with no
    /// trailing newline.
    pub fn canonical(&self) -> Vec<u8> {
        let token = self.token_sha256.map(|h| hex(&h)).unwrap_or_default();
        format!(
            "{VERSION_LINE}\n{}\n{}\n{}\n{}\n{}\n{}\n{token}",
            self.method,
            self.path_and_query,
            self.host,
            self.t,
            self.nonce,
            hex(&self.body_sha256),
        )
        .into_bytes()
    }

    /// Whether a field holds a control character, which would let one
    /// canonical string stand for two inputs.
    fn ambiguous(&self) -> bool {
        [self.method, self.path_and_query, self.host, self.nonce]
            .iter()
            .any(|f| f.chars().any(char::is_control))
    }
}

/// A parsed `Clax-Signature` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigHeader {
    pub t: i64,
    /// [`NONCE_LEN`] base64url characters.
    pub nonce: String,
    pub sig: [u8; SIG_LEN],
}

impl fmt::Display for SigHeader {
    /// The header's value: `t=…, n=…, s=…`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t={}, n={}, s={}", self.t, self.nonce, b64(&self.sig))
    }
}

/// A fresh nonce: [`NONCE_LEN`] base64url characters (16 random bytes).
pub fn new_nonce() -> String {
    b64(&random_bytes::<16>())
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `s` decoded from canonical unpadded base64url into exactly `N` bytes.
fn unb64<const N: usize>(s: &str) -> Option<[u8; N]> {
    // The engine refuses padding, other alphabets and nonzero trailing bits.
    let v = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()?;
    v.try_into().ok()
}

/// Parses a `Clax-Signature` value: exactly the parameters `t`, `n` and
/// `s`, each once, in any order, separated by commas with optional spaces.
/// `t` is decimal Unix seconds without sign or leading zeros, `n` is
/// [`NONCE_LEN`] base64url characters, and `s` is a signature in unpadded
/// base64url.
///
/// # Errors
/// [`SigError::Bad`] for anything else, including non-UTF-8.
pub fn parse_header(value: impl AsRef<[u8]>) -> Result<SigHeader, SigError> {
    let value = value.as_ref();
    if value.len() > MAX_HEADER_LEN {
        return Err(SigError::Bad);
    }
    let value = std::str::from_utf8(value).map_err(|_| SigError::Bad)?;
    let (mut t, mut n, mut s) = (None, None, None);
    for param in value.split(',') {
        let (name, v) = param
            .trim_matches(' ')
            .split_once('=')
            .ok_or(SigError::Bad)?;
        let slot = match name {
            "t" => &mut t,
            "n" => &mut n,
            "s" => &mut s,
            _ => return Err(SigError::Bad),
        };
        if slot.replace(v).is_some() {
            return Err(SigError::Bad);
        }
    }
    let (t, n, s) = (
        t.ok_or(SigError::Bad)?,
        n.ok_or(SigError::Bad)?,
        s.ok_or(SigError::Bad)?,
    );
    let digits = !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if !digits || (t.len() > 1 && t.starts_with('0')) {
        return Err(SigError::Bad);
    }
    let t: i64 = t.parse().map_err(|_| SigError::Bad)?;
    let nonce_ok = n.len() == NONCE_LEN
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !nonce_ok {
        return Err(SigError::Bad);
    }
    let sig = unb64::<SIG_LEN>(s).ok_or(SigError::Bad)?;
    Ok(SigHeader {
        t,
        nonce: n.to_owned(),
        sig,
    })
}

/// Whether `sig` is `public`'s signature of `input`. `false`, never a panic,
/// for a public key or signature of the wrong length, and for an input
/// whose method, path, host or nonce holds a control character (which would
/// make the canonical string ambiguous).
pub fn verify(public: &[u8], input: &SignInput<'_>, sig: &[u8]) -> bool {
    if public.len() != PUBLIC_LEN || sig.len() != SIG_LEN || input.ambiguous() {
        return false;
    }
    UnparsedPublicKey::new(&ED25519, public)
        .verify(&input.canonical(), sig)
        .is_ok()
}

/// A public key in unpadded base64url, as the store keeps it.
pub fn encode_public(public: &[u8; PUBLIC_LEN]) -> String {
    b64(public)
}

/// The public key in unpadded base64url `s`.
///
/// # Errors
/// [`SigError::Bad`] unless `s` is exactly 32 bytes in canonical unpadded
/// base64url.
pub fn decode_public(s: &str) -> Result<[u8; PUBLIC_LEN], SigError> {
    unb64(s).ok_or(SigError::Bad)
}

/// A public key's fingerprint for people to compare: `SHA256:` and the
/// lowercase hex SHA-256 of its 32 bytes.
pub fn fingerprint(public: &[u8; PUBLIC_LEN]) -> String {
    format!("SHA256:{}", hex(&Sha256::digest(public)))
}

/// Nonces each key used recently. Time comes in as `now`; the cache never
/// reads a clock.
#[derive(Debug)]
pub struct NonceCache {
    window_s: i64,
    per_key: usize,
    keys: HashMap<[u8; PUBLIC_LEN], KeyNonces>,
}

#[derive(Debug, Default)]
struct KeyNonces {
    /// Each remembered nonce.
    seen: HashSet<Box<str>>,
    /// The same nonces and when each was recorded, oldest first.
    order: VecDeque<(i64, Box<str>)>,
}

impl KeyNonces {
    /// Forgets the nonces recorded more than `window_s` before `now`, from
    /// the oldest: each nonce is forgotten once, so this is amortised O(1)
    /// per record. A clock that stepped back only keeps nonces longer.
    fn evict(&mut self, now: i64, window_s: i64) {
        while let Some((at, _)) = self.order.front() {
            if now.saturating_sub(*at) <= window_s {
                break;
            }
            if let Some((_, n)) = self.order.pop_front() {
                self.seen.remove(&n);
            }
        }
    }
}

impl NonceCache {
    /// A cache remembering each verified nonce for `window_s` seconds
    /// ([`NONCE_WINDOW_S`]), at most `per_key` ([`NONCES_PER_KEY`]) per key
    /// at once. `t` may be up to half the window from `now`.
    pub fn new(window_s: i64, per_key: usize) -> NonceCache {
        NonceCache {
            window_s,
            per_key,
            keys: HashMap::new(),
        }
    }

    /// The test before the body is read: `t` is within half the window of
    /// `now`, `key` has not used `nonce` within the window, and `key` has
    /// room for another nonce. Spends nothing.
    ///
    /// # Errors
    /// [`SigError::Stale`], [`SigError::Replayed`] or [`SigError::Full`].
    pub fn check(
        &mut self,
        key: &[u8; PUBLIC_LEN],
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        if t.abs_diff(now) > self.skew() {
            return Err(SigError::Stale);
        }
        let Some(k) = self.keys.get_mut(key) else {
            return Ok(());
        };
        k.evict(now, self.window_s);
        if k.seen.contains(nonce) {
            Err(SigError::Replayed)
        } else if k.order.len() >= self.per_key {
            Err(SigError::Full)
        } else {
            Ok(())
        }
    }

    /// After the signature verifies: [`NonceCache::check`] again, then
    /// records `nonce` for `key` at `now`. Of two copies of one request, one
    /// is recorded and the other is [`SigError::Replayed`].
    ///
    /// # Errors
    /// As [`NonceCache::check`].
    pub fn record(
        &mut self,
        key: &[u8; PUBLIC_LEN],
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        self.check(key, nonce, t, now)?;
        let k = self.keys.entry(*key).or_default();
        let nonce: Box<str> = nonce.into();
        k.seen.insert(nonce.clone());
        k.order.push_back((now, nonce));
        Ok(())
    }

    /// Forgets every nonce older than the window, and every key left with
    /// none. For a periodic sweep: [`NonceCache::check`] already forgets a
    /// key's old nonces as it goes.
    pub fn sweep(&mut self, now: i64) {
        self.keys.retain(|_, k| {
            k.evict(now, self.window_s);
            !k.order.is_empty()
        });
    }

    /// How far `t` may be from `now`: half the window, so a nonce recorded
    /// at any `now` its `t` allows is remembered while that `t` stays fresh.
    fn skew(&self) -> u64 {
        (self.window_s / 2).unsigned_abs()
    }

    /// How many nonces are remembered, over every key.
    pub fn len(&self) -> usize {
        self.keys.values().map(|k| k.order.len()).sum()
    }

    /// Whether no nonce is remembered.
    pub fn is_empty(&self) -> bool {
        self.keys.values().all(|k| k.order.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng as _, SeedableRng as _, rngs::StdRng};

    fn sha(b: &[u8]) -> [u8; 32] {
        Sha256::digest(b).into()
    }

    fn input<'a>(
        method: &'a str,
        path_and_query: &'a str,
        host: &'a str,
        t: i64,
        nonce: &'a str,
        body: &[u8],
        token: &str,
    ) -> SignInput<'a> {
        SignInput {
            method,
            path_and_query,
            host,
            t,
            nonce,
            body_sha256: sha(body),
            token_sha256: Some(sha(token.as_bytes())),
        }
    }

    #[test]
    fn canonical_string_is_fixed() {
        let i = input(
            "POST",
            "/api/artifacts?x=1",
            "192.168.1.5:7480",
            1_760_000_000,
            "n0",
            b"{}",
            "cxg_t",
        );
        let s = String::from_utf8(i.canonical()).unwrap();
        let lines: Vec<_> = s.lines().collect();
        assert_eq!(
            lines[..6],
            [
                "clax-ed25519 v1",
                "POST",
                "/api/artifacts?x=1",
                "192.168.1.5:7480",
                "1760000000",
                "n0"
            ]
        );
        assert_eq!(
            lines[6],
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(lines[7], hex(&sha(b"cxg_t")));
        assert_eq!(lines.len(), 8);
        assert!(!s.ends_with('\n'));
        // The grant request has no token: its last line is empty.
        let grant = SignInput {
            token_sha256: None,
            ..i
        };
        let g = String::from_utf8(grant.canonical()).unwrap();
        assert!(
            g.ends_with("\n44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\n")
        );
    }

    #[test]
    fn sign_verify_and_tamper() {
        let k = KeyPair::generate();
        let i = input("GET", "/api/artifacts", "h", 100, "n1", b"", "t");
        let h = k.sign(&i);
        assert_eq!((h.t, h.nonce.as_str()), (100, "n1"));
        assert!(verify(&k.public(), &i, &h.sig));
        for bad in [
            input("POST", "/api/artifacts", "h", 100, "n1", b"", "t"),
            input("GET", "/api/artifacts?a", "h", 100, "n1", b"", "t"),
            input("GET", "/api/artifacts", "x", 100, "n1", b"", "t"),
            input("GET", "/api/artifacts", "h", 101, "n1", b"", "t"),
            input("GET", "/api/artifacts", "h", 100, "n2", b"", "t"),
            input("GET", "/api/artifacts", "h", 100, "n1", b"x", "t"),
            input("GET", "/api/artifacts", "h", 100, "n1", b"", "u"),
            SignInput {
                token_sha256: None,
                ..i
            },
        ] {
            assert!(!verify(&k.public(), &bad, &h.sig), "{bad:?}");
        }
        assert!(!verify(&KeyPair::generate().public(), &i, &h.sig));
        let mut flipped = h.sig;
        flipped[0] ^= 1;
        assert!(!verify(&k.public(), &i, &flipped));
    }

    #[test]
    fn control_characters_and_wrong_lengths_never_verify() {
        let k = KeyPair::generate();
        for (m, p, host, n) in [
            ("GET", "/a\nGET", "h", "n"),
            ("GET\r", "/a", "h", "n"),
            ("GET", "/a", "h\n1", "n"),
            ("GET", "/a", "h", "n\0"),
        ] {
            let i = input(m, p, host, 1, n, b"", "t");
            let h = k.sign(&i);
            assert!(!verify(&k.public(), &i, &h.sig), "{i:?}");
        }
        let i = input("GET", "/a", "h", 1, "n", b"", "t");
        let h = k.sign(&i);
        let public = k.public();
        assert!(!verify(&public[..31], &i, &h.sig));
        assert!(!verify(&[public.as_slice(), &[0]].concat(), &i, &h.sig));
        assert!(!verify(&public, &i, &h.sig[..63]));
        assert!(!verify(&public, &i, &[h.sig.as_slice(), &[0]].concat()));
        assert!(!verify(&[], &i, &[]));
    }

    #[test]
    fn replay_and_staleness_are_refused() {
        let mut c = NonceCache::new(240, 4);
        let key = [7u8; 32];
        assert!(c.record(&key, "a", 1000, 1000).is_ok());
        assert_eq!(c.check(&key, "a", 1000, 1001), Err(SigError::Replayed));
        assert_eq!(c.record(&key, "a", 1000, 1001), Err(SigError::Replayed));
        assert_eq!(c.check(&key, "b", 1000, 1121), Err(SigError::Stale));
        assert_eq!(c.check(&key, "b", 1000, 879), Err(SigError::Stale));
        assert_eq!(c.record(&key, "b", 1000, 1121), Err(SigError::Stale));
        assert!(c.check(&key, "b", 1000, 1120).is_ok(), "120 s is in time");
        assert!(c.check(&key, "b", 1000, 880).is_ok());
        for n in ["c", "d", "e"] {
            c.record(&key, n, 1000, 1000).unwrap();
        }
        assert_eq!(c.check(&key, "f", 1000, 1000), Err(SigError::Full));
        assert_eq!(c.record(&key, "f", 1000, 1000), Err(SigError::Full));
        assert!(
            c.record(&[8u8; 32], "a", 1000, 1000).is_ok(),
            "each key has its own nonces"
        );
        assert_eq!(
            c.check(&key, "a", 1240, 1240),
            Err(SigError::Replayed),
            "a nonce is remembered for the whole window"
        );
        assert!(
            c.record(&key, "f", 1300, 1300).is_ok(),
            "entries older than the window are evicted"
        );
        assert!(c.record(&key, "a", 1300, 1300).is_ok());
    }

    #[test]
    fn only_record_spends_a_nonce() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [1u8; 32];
        for _ in 0..3 {
            c.check(&key, "a", 50, 50).unwrap();
        }
        assert!(c.is_empty(), "a check spends nothing");
        c.record(&key, "a", 50, 50).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_replay_stays_refused_while_its_time_is_fresh() {
        // Recorded at the earliest `now` its `t` allows, the nonce is still
        // remembered at the latest.
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [2u8; 32];
        let t = 10_000;
        c.record(&key, "a", t, t - MAX_SKEW_S).unwrap();
        assert_eq!(
            c.check(&key, "a", t, t + MAX_SKEW_S),
            Err(SigError::Replayed)
        );
        assert_eq!(
            c.check(&key, "a", t, t + MAX_SKEW_S + 1),
            Err(SigError::Stale)
        );
    }

    #[test]
    fn sweep_forgets_old_nonces_and_idle_keys() {
        let mut c = NonceCache::new(240, 8);
        c.record(&[1u8; 32], "a", 0, 0).unwrap();
        c.record(&[2u8; 32], "a", 200, 200).unwrap();
        c.sweep(240);
        assert_eq!(c.len(), 2);
        c.sweep(241);
        assert_eq!(c.len(), 1);
        assert_eq!(c.keys.len(), 1, "an idle key is dropped");
        c.sweep(1000);
        assert!(c.is_empty());
    }

    #[test]
    fn extreme_times_neither_panic_nor_pass() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [3u8; 32];
        for (t, now) in [(i64::MIN, i64::MAX), (i64::MAX, i64::MIN), (i64::MAX, 0)] {
            assert_eq!(c.record(&key, "a", t, now), Err(SigError::Stale));
        }
        c.record(&key, "a", i64::MAX, i64::MAX).unwrap();
        c.record(&key, "b", i64::MIN, i64::MIN).unwrap();
        c.sweep(i64::MAX);
        c.sweep(i64::MIN);
    }

    #[test]
    fn pkcs8_round_trips_and_header_parses() {
        let k = KeyPair::generate();
        let back = KeyPair::from_pkcs8(k.pkcs8().expose()).unwrap();
        assert_eq!(back.public(), k.public());
        assert_eq!(KeyPair::from_pkcs8(b"not a key").err(), Some(SigError::Bad));
        assert_eq!(KeyPair::from_pkcs8(&[]).err(), Some(SigError::Bad));

        let n = new_nonce();
        assert_eq!(n.len(), NONCE_LEN);
        assert_ne!(n, new_nonce());
        let i = input("PUT", "/api/a/x?y", "h:1", 1_760_000_000, &n, b"b", "t");
        let h = back.sign(&i);
        let value = h.to_string();
        assert!(value.starts_with(&format!("t=1760000000, n={n}, s=")));
        let parsed = parse_header(&value).unwrap();
        assert_eq!(parsed, h);
        assert!(verify(&k.public(), &i, &parsed.sig));
        // Any order, optional spaces.
        let s = value.rsplit("s=").next().unwrap();
        let reordered = format!("s={s},n={n} ,  t=1760000000");
        assert_eq!(parse_header(reordered.as_bytes()).unwrap(), h);

        assert_eq!(decode_public(&encode_public(&k.public())), Ok(k.public()));
        assert_eq!(encode_public(&k.public()).len(), 43);
        let fp = fingerprint(&k.public());
        assert!(fp.starts_with("SHA256:") && fp.len() == 7 + 64);
    }

    #[test]
    fn headers_are_read_strictly() {
        let n = "AAAAAAAAAAAAAAAAAAAAAA";
        let s = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 64]);
        let good = format!("t=5, n={n}, s={s}");
        assert!(parse_header(&good).is_ok());
        let bad = [
            String::new(),
            format!("t=5, n={n}"),
            format!("n={n}, s={s}"),
            format!("t=5, s={s}"),
            format!("t=5, n={n}, s={s}, x=1"),
            format!("t=5, n={n}, s={s}, t=5"),
            format!("t=5, n={n}, n={n}, s={s}"),
            format!("t=5, n={n}, s={s},"),
            format!(",t=5, n={n}, s={s}"),
            format!("t=5,, n={n}, s={s}"),
            format!("t=5; n={n}; s={s}"),
            format!("T=5, n={n}, s={s}"),
            format!("t =5, n={n}, s={s}"),
            format!("t=\"5\", n={n}, s={s}"),
            format!("t=, n={n}, s={s}"),
            format!("t=-5, n={n}, s={s}"),
            format!("t=+5, n={n}, s={s}"),
            format!("t=05, n={n}, s={s}"),
            format!("t=5.0, n={n}, s={s}"),
            format!("t=99999999999999999999, n={n}, s={s}"),
            format!("t=5, n={}, s={s}", &n[1..]),
            format!("t=5, n={n}A, s={s}"),
            format!("t=5, n=AAAAAAAAAAAAAAAAAAAAA+, s={s}"),
            format!("t=5, n=AAAAAAAAAAAAAAAAAAAAA=, s={s}"),
            format!("t=5, n={n}, s={s}="),
            format!("t=5, n={n}, s={}", &s[1..]),
            format!("t=5, n={n}, s={s}AAAA"),
            format!("t=5, n={n}, s={}", s.replace('J', "+")),
            format!(
                "t=5, n={n}, s={}",
                base64::engine::general_purpose::STANDARD.encode([0xffu8; 64])
            ),
            format!("t=5, n={n}, s={}", "A".repeat(300)),
            format!("t=5,\tn={n}, s={s}\n"),
        ];
        for b in &bad {
            assert_eq!(parse_header(b), Err(SigError::Bad), "{b:?}");
        }
        // The last base64url character of a 64-byte signature carries
        // unused bits: they must be zero.
        let zero = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 64]);
        assert!(zero.ends_with("AA") && parse_header(format!("t=5, n={n}, s={zero}")).is_ok());
        let loose = format!("{}B", &zero[..zero.len() - 1]);
        assert_eq!(
            parse_header(format!("t=5, n={n}, s={loose}")),
            Err(SigError::Bad)
        );
        let mut non_utf8 = good.clone().into_bytes();
        non_utf8[2] = 0xff;
        assert_eq!(parse_header(&non_utf8), Err(SigError::Bad));
        assert_eq!(decode_public("AAAA"), Err(SigError::Bad));
        assert_eq!(decode_public(&"A".repeat(44)), Err(SigError::Bad));
        assert_eq!(
            decode_public(&format!("{}=", "A".repeat(43))),
            Err(SigError::Bad)
        );
    }

    #[test]
    fn random_bytes_never_panic() {
        let mut rng = StdRng::seed_from_u64(0x5eed);
        let k = KeyPair::generate();
        let i = input("GET", "/", "h", 1, "n", b"", "t");
        let good = k.sign(&i).to_string().into_bytes();
        let sig = k.sign(&i).sig;
        let mut c = NonceCache::new(NONCE_WINDOW_S, 16);
        for round in 0..3_000 {
            let len = rng.random_range(0..200);
            let mut b: Vec<u8> = (0..len).map(|_| rng.random()).collect();
            // Half the rounds mutate a valid header, to reach deeper.
            if round % 2 == 0 {
                b = good.clone();
                for _ in 0..rng.random_range(1..4) {
                    let at = rng.random_range(0..b.len());
                    b[at] = rng.random();
                }
            }
            let _ = parse_header(&b);
            let _ = KeyPair::from_pkcs8(&b);
            let text = String::from_utf8_lossy(&b);
            let _ = decode_public(&text);
            let split = b.len() / 2;
            assert!(!verify(&b[..split], &i, &b[split..]));
            let r = SignInput {
                method: &text,
                path_and_query: &text,
                host: &text,
                nonce: &text,
                t: rng.random(),
                ..i
            };
            assert!(!verify(&k.public(), &r, &sig));
            let _ = c.record(&k.public(), &text, rng.random(), rng.random());
        }
    }

    #[test]
    fn key_material_never_prints() {
        let k = KeyPair::generate();
        let doc = k.pkcs8().expose().to_vec();
        let shown = format!("{k:?} {:?}", k.pkcs8());
        assert_eq!(format!("{:?}", k.pkcs8()), "Pkcs8(…)");
        assert!(shown.contains(&fingerprint(&k.public())));
        // No run of the document's bytes, in any common encoding.
        let encodings = [
            hex(&doc),
            base64::engine::general_purpose::STANDARD.encode(&doc),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&doc),
            format!("{doc:?}"),
        ];
        // ring's PKCS#8 v2 Ed25519: a 16-byte prefix, the 32-byte seed, a
        // 3-byte tag and the public key.
        assert_eq!(doc.len(), 83);
        assert_eq!(doc[51..], k.public());
        let seed = &doc[16..48];
        for e in encodings.iter().chain(&[hex(seed), format!("{seed:?}")]) {
            assert!(!shown.contains(e.as_str()), "{e}");
            assert!(!shown.contains(&e[..e.len().min(16)]), "{e}");
        }
    }

    /// Cost of the nonce cache at full cap and of signing and verifying.
    /// Run with `--run-ignored only`; it prints and asserts nothing about
    /// time.
    #[test]
    #[ignore]
    fn cost() {
        use std::time::Instant;
        let key = [4u8; 32];
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let nonces: Vec<String> = (0..2 * NONCES_PER_KEY).map(|_| new_nonce()).collect();
        let start = Instant::now();
        for n in &nonces[..NONCES_PER_KEY] {
            c.record(&key, n, 0, 0).unwrap();
        }
        let fill = start.elapsed();
        let start = Instant::now();
        for n in &nonces[NONCES_PER_KEY..] {
            assert_eq!(c.check(&key, n, 0, 0), Err(SigError::Full));
        }
        let full = start.elapsed();
        // Steady state at the cap: one nonce a second over the window's
        // edge, each record evicting one.
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        // A nonce is kept through `now - at == window`: 241 seconds of them.
        let per = NONCES_PER_KEY as i64 / (NONCE_WINDOW_S + 1);
        for (j, n) in nonces[..NONCES_PER_KEY].iter().enumerate() {
            let now = j as i64 / per;
            c.record(&key, n, now, now).unwrap();
        }
        let start = Instant::now();
        let base = NONCES_PER_KEY as i64 / per;
        for (j, n) in nonces[NONCES_PER_KEY..].iter().enumerate() {
            let now = base + j as i64 / per;
            c.record(&key, n, now, now).unwrap();
        }
        let steady = start.elapsed();
        // The whole cap expiring at once, on one check.
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        for n in &nonces[..NONCES_PER_KEY] {
            c.record(&key, n, 0, 0).unwrap();
        }
        let start = Instant::now();
        c.record(&key, "x", 241, 241).unwrap();
        let burst = start.elapsed();

        let k = KeyPair::generate();
        let i = input("POST", "/api/artifacts", "h", 1, "n", b"{}", "t");
        let start = Instant::now();
        let mut h = k.sign(&i);
        for _ in 0..999 {
            h = k.sign(&i);
        }
        let sign = start.elapsed() / 1000;
        let start = Instant::now();
        for _ in 0..1000 {
            assert!(verify(&k.public(), &i, &h.sig));
        }
        let ver = start.elapsed() / 1000;
        let n = NONCES_PER_KEY as u32;
        println!(
            "fill {:?}/record, full-cap check {:?}, steady record {:?}, \
             burst evict of {n} {:?}, sign {sign:?}, verify {ver:?}",
            fill / n,
            full / n,
            steady / n,
            burst
        );
    }
}

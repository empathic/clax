//! Key-bound agent requests, `clax-ed25519` (spec §6.6, M5).
//!
//! A connected machine holds an Ed25519 [`KeyPair`] and signs every request
//! over a fixed canonical string ([`SignInput::canonical`]): the method, the
//! path and query as sent, the `Host`, the time `t`, a nonce, the SHA-256 of
//! the body, the SHA-256 of the access token (an empty line for the grant
//! request, which has no token yet) and the SHA-256 of the agent-side
//! headers ([`AGENT_HEADERS`], serialized by [`encode_agent_headers`]). The
//! signature travels in
//!
//! ```text
//! Clax-Signature: t=<unix seconds>, n=<22 base64url chars>, s=<86 base64url chars>
//! ```
//!
//! which [`parse_header`] reads strictly. The daemon tests `t` and the nonce
//! with [`NonceCache::check`] before reading the body, verifies the signature
//! with [`verify`], and only then spends the nonce with
//! [`NonceCache::record`], so a bad signature spends nothing; `record` does
//! not test `t` again, so a slow body cannot make a verified request stale.

use crate::secret::{hex, random_bytes};
use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

/// First line of the canonical string. `v2` is the only version: it signs
/// the agent-side headers ([`AGENT_HEADERS`]) as well as `v1`'s lines, and
/// `v1` is never accepted.
pub const VERSION_LINE: &str = "clax-ed25519 v2";
/// The agent-side headers a signature covers, in the order
/// [`encode_agent_headers`] takes them: the session the request acts as,
/// the agent-channel marker, the agent side that sent it, its git context
/// and the tool call it is made under. Lowercase; names match ignoring
/// case.
pub const AGENT_HEADERS: [&str; 5] = [
    "x-clax-session",
    "x-clax-channel",
    "x-clax-via",
    "x-clax-git",
    "x-clax-call",
];
/// Seconds a verified nonce is remembered for its key.
pub const NONCE_WINDOW_S: i64 = 240;
/// Seconds `t` may differ from the daemon's clock: half the window, as
/// [`NonceCache::skew`] derives it.
pub const MAX_SKEW_S: i64 = NONCE_WINDOW_S / 2;
/// Verified nonces remembered per key within [`NONCE_WINDOW_S`].
pub const NONCES_PER_KEY: usize = 65_536;
/// Keys a [`NonceCache`] holds at once by default. Signing keys are live
/// grants and pending requests (at most 20 pending, `MAX_PENDING`), all made
/// by the owner's approvals and invites, so a real install has tens; 4096
/// leaves room for every one while bounding what key churn can hold.
pub const MAX_KEYS: usize = 4096;
/// Nonces a [`NonceCache`] holds at once over every key, by default: 2^20,
/// about 40 MB at most, sixteen saturated keys. The cap is first come, first
/// served: one key cannot starve the others (its own cap,
/// [`NONCES_PER_KEY`], is a sixteenth of this), but sixteen saturated keys
/// can, refusing every other key until their nonces age out.
pub const MAX_NONCES: usize = 1 << 20;
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
    /// The key has used its nonces for the window, or the cache holds its
    /// most keys or nonces overall.
    #[error("too many recent signed requests")]
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
    ///
    /// # Errors
    /// [`SigError::Bad`] for an input the daemon refuses whatever the
    /// signature: a control character in the method, path, host or nonce
    /// (which [`verify`] refuses), or a nonce that is not [`NONCE_LEN`]
    /// canonical base64url characters (which [`parse_header`] and
    /// [`NonceCache::check`] refuse).
    pub fn sign(&self, input: &SignInput<'_>) -> Result<SigHeader, SigError> {
        if input.ambiguous() || nonce_bits(input.nonce).is_err() {
            return Err(SigError::Bad);
        }
        let sig = self.pair.sign(&input.canonical());
        Ok(SigHeader {
            t: input.t,
            nonce: input.nonce.to_owned(),
            sig: sig
                .as_ref()
                .try_into()
                .expect("an Ed25519 signature is 64 bytes"),
        })
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
    /// [`agent_headers_sha256`] of the request's [`AGENT_HEADERS`].
    pub agent_headers_sha256: [u8; 32],
}

impl SignInput<'_> {
    /// The signed message: the UTF-8 of [`VERSION_LINE`], the method, the
    /// path and query, the host, `t`, the nonce, the body's SHA-256 in
    /// lowercase hex, the token's (or nothing) and the agent headers'
    /// ([`agent_headers_sha256`]), joined by `\n`, with no trailing
    /// newline.
    pub fn canonical(&self) -> Vec<u8> {
        let token = self.token_sha256.map(|h| hex(&h)).unwrap_or_default();
        format!(
            "{VERSION_LINE}\n{}\n{}\n{}\n{}\n{}\n{}\n{token}\n{}",
            self.method,
            self.path_and_query,
            self.host,
            self.t,
            self.nonce,
            hex(&self.body_sha256),
            hex(&self.agent_headers_sha256),
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

/// The values of a request's [`AGENT_HEADERS`], in their order: `None` for
/// a header the request does not carry, the value's bytes as sent for one
/// it does.
pub type AgentHeaderValues<'a> = [Option<&'a [u8]>; AGENT_HEADERS.len()];

/// The values of [`AGENT_HEADERS`] among `headers` (name and value pairs,
/// names matched ignoring case; every other header is ignored).
///
/// # Errors
/// [`SigError::Bad`] when one of them is given more than once, even with
/// the same value: one signature never stands for two readings.
pub fn agent_header_values<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<AgentHeaderValues<'a>, SigError> {
    let mut values: AgentHeaderValues<'a> = [None; AGENT_HEADERS.len()];
    for (name, value) in headers {
        let Some(i) = AGENT_HEADERS
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        if values[i].replace(value).is_some() {
            return Err(SigError::Bad);
        }
    }
    Ok(values)
}

/// The fixed serialization of the agent headers' values that
/// [`agent_headers_sha256`] hashes: for each of [`AGENT_HEADERS`] in order,
/// one record ending in `\n`, which is `-` for an absent header and, for a
/// present one, `+`, the value's length in bytes in decimal, `:` and the
/// value's bytes. An absent header and an empty one differ (`-` and `+0:`),
/// and the length prefix keeps any value from reading as another record.
pub fn encode_agent_headers(values: &AgentHeaderValues<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        match v {
            None => out.push(b'-'),
            Some(v) => {
                out.extend_from_slice(format!("+{}:", v.len()).as_bytes());
                out.extend_from_slice(v);
            }
        }
        out.push(b'\n');
    }
    out
}

/// The SHA-256 of [`encode_agent_headers`] of `values`: the last line of
/// the canonical string, in lowercase hex.
pub fn agent_headers_sha256(values: &AgentHeaderValues<'_>) -> [u8; 32] {
    Sha256::digest(encode_agent_headers(values)).into()
}

/// [`agent_headers_sha256`] of a request that carries none of
/// [`AGENT_HEADERS`].
pub fn no_agent_headers_sha256() -> [u8; 32] {
    agent_headers_sha256(&[None; AGENT_HEADERS.len()])
}

/// `value` without its leading and trailing spaces and tabs.
///
/// A header value has no surrounding spaces or tabs: HTTP parsers drop
/// them (RFC 9110 §5.5), so the daemon reads, and hashes, a value sent as
/// `" agent\t"` as `"agent"`. A signer therefore sends and signs each of
/// [`AGENT_HEADERS`] trimmed by this function ([`signable_agent_headers`]),
/// so the value it signs is the one the daemon verifies.
pub fn trim_header_value(value: &[u8]) -> &[u8] {
    let blank = |b: &u8| *b == b' ' || *b == b'\t';
    let start = value.iter().position(|b| !blank(b)).unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| !blank(b))
        .map_or(start, |i| i + 1);
    &value[start..end]
}

/// `values` with each present value trimmed by [`trim_header_value`]: what
/// a signer sends and signs.
pub fn signable_agent_headers<'a>(values: &AgentHeaderValues<'a>) -> AgentHeaderValues<'a> {
    values.map(|v| v.map(trim_header_value))
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
        write!(
            f,
            "t={}, n={}, s={}",
            self.t,
            self.nonce,
            crate::secret::base64url(&self.sig)
        )
    }
}

/// A fresh nonce: [`NONCE_LEN`] base64url characters (16 random bytes).
pub fn new_nonce() -> String {
    crate::secret::base64url(&random_bytes::<16>())
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
/// `s`, each once, in any order, separated by commas. Spaces (not tabs) may
/// stand before and after each parameter, so also at the value's ends;
/// none may stand around `=`.
/// `t` is decimal Unix seconds without sign or leading zeros, `n` is
/// [`NONCE_LEN`] canonical base64url characters (16 bytes, the test
/// [`NonceCache::check`] applies), and `s` is a signature in unpadded
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
    nonce_bits(n)?;
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
    crate::secret::base64url(public)
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
///
/// Each key's nonces are kept in buckets of [`BUCKETS`]-th of the window,
/// oldest first. A bucket is dropped whole once its last second is older
/// than the window, so a nonce is remembered for at least the window (and
/// up to one bucket longer), and forgetting a full key is one deallocation
/// per bucket rather than one per nonce.
///
/// Memory is bounded three ways: `per_key` nonces per key, `max_keys` keys
/// and `max_total` nonces overall, each refused with [`SigError::Full`].
/// Every call also tidies up to [`TIDY_PER_CALL`] other keys, dropping their
/// old buckets and removing keys left with none, so idle keys leave without
/// a timer; [`NonceCache::sweep`] does the same for every key at once.
#[derive(Debug)]
pub struct NonceCache {
    window_s: i64,
    bucket_s: i64,
    per_key: usize,
    max_keys: usize,
    max_total: usize,
    total: usize,
    keys: HashMap<[u8; PUBLIC_LEN], KeyNonces>,
    /// Every key in `keys`, once, in the order [`NonceCache::tidy`] visits
    /// them.
    rotation: VecDeque<[u8; PUBLIC_LEN]>,
}

/// Buckets per window.
pub const BUCKETS: i64 = 16;
/// Other keys each [`NonceCache::check`] or [`NonceCache::record`] tidies.
/// With keys arriving once each, a key waits at most a quarter of a lap
/// past its expiry, so at most 4/3 of the keys live within the retention
/// are held.
pub const TIDY_PER_CALL: usize = 4;

#[derive(Debug, Default)]
struct KeyNonces {
    /// Oldest first, epochs increasing.
    buckets: VecDeque<Bucket>,
    len: usize,
}

#[derive(Debug)]
struct Bucket {
    /// Recording time divided by the bucket width.
    epoch: i64,
    /// Each nonce once, as its 16 decoded bytes.
    nonces: HashSet<u128>,
}

impl KeyNonces {
    fn contains(&self, n: u128) -> bool {
        self.buckets.iter().any(|b| b.nonces.contains(&n))
    }

    /// Drops the buckets whose last second is more than `window_s` before
    /// `now`, returning how many nonces went. A clock that stepped back only
    /// keeps nonces longer.
    fn evict(&mut self, now: i64, window_s: i64, bucket_s: i64) -> usize {
        let mut gone = 0;
        while let Some(b) = self.buckets.front() {
            let last = b
                .epoch
                .saturating_mul(bucket_s)
                .saturating_add(bucket_s - 1);
            if now.saturating_sub(last) <= window_s {
                break;
            }
            gone += b.nonces.len();
            self.buckets.pop_front();
        }
        self.len -= gone;
        gone
    }

    /// Records `n` at `at`. A time before the newest bucket goes into that
    /// bucket, which only keeps it longer.
    fn insert(&mut self, n: u128, at: i64, bucket_s: i64) {
        let epoch = at.div_euclid(bucket_s);
        match self.buckets.back_mut() {
            Some(b) if b.epoch >= epoch => {
                b.nonces.insert(n);
            }
            _ => self.buckets.push_back(Bucket {
                epoch,
                nonces: HashSet::from([n]),
            }),
        }
        self.len += 1;
    }
}

/// `nonce` as its 16 bytes, if it is [`NONCE_LEN`] canonical base64url
/// characters.
fn nonce_bits(nonce: &str) -> Result<u128, SigError> {
    if nonce.len() != NONCE_LEN {
        return Err(SigError::Bad);
    }
    unb64::<16>(nonce)
        .map(u128::from_be_bytes)
        .ok_or(SigError::Bad)
}

impl NonceCache {
    /// A cache remembering each verified nonce for `window_s` seconds
    /// ([`NONCE_WINDOW_S`]), at most `per_key` ([`NONCES_PER_KEY`]) per key
    /// at once, for at most [`MAX_KEYS`] keys and [`MAX_NONCES`] nonces in
    /// all. `t` may be up to half the window from `now`.
    pub fn new(window_s: i64, per_key: usize) -> NonceCache {
        NonceCache {
            window_s,
            bucket_s: (window_s / BUCKETS).max(1),
            per_key,
            max_keys: MAX_KEYS,
            max_total: MAX_NONCES,
            total: 0,
            keys: HashMap::new(),
            rotation: VecDeque::new(),
        }
    }

    /// The same cache with at most `max_keys` keys and `max_total` nonces.
    pub fn with_caps(mut self, max_keys: usize, max_total: usize) -> NonceCache {
        self.max_keys = max_keys;
        self.max_total = max_total;
        self
    }

    /// How far `t` may be from `now`: half the window, so a nonce is
    /// remembered while its `t` stays fresh ([`MAX_SKEW_S`] for
    /// [`NONCE_WINDOW_S`]).
    pub fn skew(&self) -> i64 {
        self.window_s / 2
    }

    /// The test before the body is read: `t` is within [`NonceCache::skew`]
    /// of `now`, the nonce has the shape [`parse_header`] reads, `key` has
    /// not used it within the window, and there is room to record it.
    /// Spends nothing.
    ///
    /// # Errors
    /// [`SigError::Stale`], [`SigError::Bad`], [`SigError::Replayed`] or
    /// [`SigError::Full`].
    pub fn check(
        &mut self,
        key: &[u8; PUBLIC_LEN],
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        if t.abs_diff(now) > self.skew().unsigned_abs() {
            return Err(SigError::Stale);
        }
        let n = nonce_bits(nonce)?;
        self.room(key, n, now)
    }

    /// After the signature verifies, the second presence test and the
    /// record. `t` is not tested again for staleness (a slow body must not
    /// make a verified request stale); the nonce is recorded at the later of
    /// `now` and `t − skew`, so it is remembered at least until `t + skew`
    /// however long the body took or wherever the clock stepped. Of two
    /// copies of one request, one is recorded and the other is
    /// [`SigError::Replayed`]. `t` must have passed [`NonceCache::check`];
    /// a `t` so far ahead that `t − skew` lies beyond `now` and a whole
    /// window, which no checked `t` reaches, is refused rather than
    /// recorded in the future, where its bucket would hold the key's later
    /// nonces until then.
    ///
    /// # Errors
    /// [`SigError::Stale`], [`SigError::Bad`], [`SigError::Replayed`] or
    /// [`SigError::Full`].
    pub fn record(
        &mut self,
        key: &[u8; PUBLIC_LEN],
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        let n = nonce_bits(nonce)?;
        if t.saturating_sub(self.skew()) > now.saturating_add(self.window_s) {
            return Err(SigError::Stale);
        }
        self.room(key, n, now)?;
        let at = now.max(t.saturating_sub(self.skew()));
        let k = match self.keys.entry(*key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                self.rotation.push_back(*key);
                e.insert(KeyNonces::default())
            }
        };
        k.insert(n, at, self.bucket_s);
        self.total += 1;
        Ok(())
    }

    /// Tidies, then whether `key` may record `n` at `now`.
    fn room(&mut self, key: &[u8; PUBLIC_LEN], n: u128, now: i64) -> Result<(), SigError> {
        self.tidy(now);
        let full = match self.keys.get_mut(key) {
            Some(k) => {
                self.total -= k.evict(now, self.window_s, self.bucket_s);
                if k.contains(n) {
                    return Err(SigError::Replayed);
                }
                k.len >= self.per_key
            }
            None => self.keys.len() >= self.max_keys,
        };
        if full || self.total >= self.max_total {
            return Err(SigError::Full);
        }
        Ok(())
    }

    /// Visits the next [`TIDY_PER_CALL`] keys in rotation: drops their old
    /// buckets and removes those left empty.
    fn tidy(&mut self, now: i64) {
        for _ in 0..TIDY_PER_CALL.min(self.rotation.len()) {
            let Some(key) = self.rotation.pop_front() else {
                break;
            };
            if let Some(k) = self.keys.get_mut(&key) {
                self.total -= k.evict(now, self.window_s, self.bucket_s);
                if k.len == 0 {
                    self.keys.remove(&key);
                } else {
                    self.rotation.push_back(key);
                }
            }
        }
    }

    /// Forgets every nonce older than the window, and every key left with
    /// none: for a periodic sweep, O(keys).
    pub fn sweep(&mut self, now: i64) {
        let (window_s, bucket_s) = (self.window_s, self.bucket_s);
        let mut gone = 0;
        self.keys.retain(|_, k| {
            gone += k.evict(now, window_s, bucket_s);
            k.len > 0
        });
        self.total -= gone;
        let keys = &self.keys;
        self.rotation.retain(|k| keys.contains_key(k));
    }

    /// Forgets every nonce of `key`: for a cache that evicts a key rather
    /// than refusing a new one.
    pub fn forget(&mut self, key: &[u8; PUBLIC_LEN]) {
        if let Some(k) = self.keys.remove(key) {
            self.total -= k.len;
            self.rotation.retain(|r| r != key);
        }
    }

    /// Whether `key` has nonces remembered.
    pub fn has_key(&self, key: &[u8; PUBLIC_LEN]) -> bool {
        self.keys.contains_key(key)
    }

    /// How many nonces are remembered, over every key.
    pub fn len(&self) -> usize {
        self.total
    }

    /// Whether no nonce is remembered.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// How many keys have nonces remembered.
    pub fn keys(&self) -> usize {
        self.keys.len()
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
            agent_headers_sha256: no_agent_headers_sha256(),
        }
    }

    #[test]
    fn canonical_string_is_fixed() {
        let session = b"01J9ZK3M4N5P6Q7R8S9T0V1W2X".as_slice();
        // Session, channel and via present, git absent, call empty.
        let values = [
            Some(session),
            Some(b"agent".as_slice()),
            Some(b"mcp".as_slice()),
            None,
            Some(b"".as_slice()),
        ];
        assert_eq!(
            encode_agent_headers(&values),
            b"+26:01J9ZK3M4N5P6Q7R8S9T0V1W2X\n+5:agent\n+3:mcp\n-\n+0:\n"
        );
        let i = SignInput {
            agent_headers_sha256: agent_headers_sha256(&values),
            ..input(
                "POST",
                "/api/artifacts?x=1",
                "192.168.1.5:7480",
                1_760_000_000,
                "n0",
                b"{}",
                "cxg_t",
            )
        };
        let s = String::from_utf8(i.canonical()).unwrap();
        assert_eq!(
            s,
            "clax-ed25519 v2\n\
             POST\n\
             /api/artifacts?x=1\n\
             192.168.1.5:7480\n\
             1760000000\n\
             n0\n\
             44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\n\
             e363d964cd2aef8c9f8019233052f146fd7c41533e194244bd856d9dedfdfcd4\n\
             0200b59637463a41412fc63797d0699f6daaab4bed5d838ac1a4c264398d66b5"
        );
        assert_eq!(s.lines().count(), 9);
        // A request with none of the headers.
        assert_eq!(
            hex(&no_agent_headers_sha256()),
            "a00d0d823baf03fa53cba1260dd2f71fc7e8694679ce444737a7168dfab351e8"
        );
        // The grant request has no token: its token line is empty.
        let grant = SignInput {
            token_sha256: None,
            ..i
        };
        let g = String::from_utf8(grant.canonical()).unwrap();
        assert!(g.ends_with(
            "\n44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\n\n\
             0200b59637463a41412fc63797d0699f6daaab4bed5d838ac1a4c264398d66b5"
        ));
    }

    #[test]
    fn signers_send_header_values_without_surrounding_spaces_or_tabs() {
        // HTTP parsers drop a value's leading and trailing spaces and tabs,
        // so the daemon hashes "agent" for a header sent as " agent\t ".
        assert_eq!(trim_header_value(b" agent\t "), b"agent");
        assert_eq!(trim_header_value(b"\t\t"), b"");
        assert_eq!(trim_header_value(b"a b"), b"a b");
        assert_eq!(trim_header_value(b""), b"");
        let sent = [None, Some(b"agent ".as_slice()), None, None, None];
        let received = [None, Some(b"agent".as_slice()), None, None, None];
        assert_ne!(agent_headers_sha256(&sent), agent_headers_sha256(&received));
        assert_eq!(
            agent_headers_sha256(&signable_agent_headers(&sent)),
            agent_headers_sha256(&received)
        );
    }

    #[test]
    fn agent_headers_are_read_once_each_in_a_fixed_order() {
        let pairs = [
            ("X-Clax-Call", b"c".as_slice()),
            ("content-type", b"x"),
            ("x-clax-session", b"s"),
        ];
        let v = agent_header_values(pairs).unwrap();
        assert_eq!(
            v,
            [
                Some(b"s".as_slice()),
                None,
                None,
                None,
                Some(b"c".as_slice())
            ]
        );
        // A header given twice is refused, whatever its values and case.
        for twice in [
            vec![
                ("x-clax-session", b"a".as_slice()),
                ("x-clax-session", b"b"),
            ],
            vec![("x-clax-via", b"mcp".as_slice()), ("X-Clax-Via", b"mcp")],
            vec![("x-clax-git", b"".as_slice()), ("x-clax-git", b"")],
        ] {
            assert_eq!(
                agent_header_values(twice.clone()),
                Err(SigError::Bad),
                "{twice:?}"
            );
        }
    }

    #[test]
    fn absent_and_empty_agent_headers_hash_differently() {
        for i in 0..AGENT_HEADERS.len() {
            let mut empty = [None; AGENT_HEADERS.len()];
            empty[i] = Some(b"".as_slice());
            assert_ne!(
                agent_headers_sha256(&empty),
                no_agent_headers_sha256(),
                "{}",
                AGENT_HEADERS[i]
            );
            // Nor does one header's value read as another's.
            let mut shifted = [None; AGENT_HEADERS.len()];
            shifted[(i + 1) % AGENT_HEADERS.len()] = Some(b"".as_slice());
            assert_ne!(agent_headers_sha256(&empty), agent_headers_sha256(&shifted));
        }
        // A value holding a record separator stays one value.
        let a = [Some(b"x\n-".as_slice()), None, None, None, None];
        let b = [Some(b"x".as_slice()), None, None, None, None];
        assert_ne!(encode_agent_headers(&a), encode_agent_headers(&b));
    }

    /// The `i`th test nonce: 22 base64url characters.
    fn nn(i: u32) -> String {
        crate::secret::base64url(&u128::from(i).to_be_bytes())
    }

    #[test]
    fn sign_verify_and_tamper() {
        let k = KeyPair::generate();
        let (n1, n2) = (nn(1), nn(2));
        let i = input("GET", "/api/artifacts", "h", 100, &n1, b"", "t");
        let h = k.sign(&i).unwrap();
        assert_eq!((h.t, h.nonce.as_str()), (100, n1.as_str()));
        assert!(verify(&k.public(), &i, &h.sig));
        for bad in [
            input("POST", "/api/artifacts", "h", 100, &n1, b"", "t"),
            input("GET", "/api/artifacts?a", "h", 100, &n1, b"", "t"),
            input("GET", "/api/artifacts", "x", 100, &n1, b"", "t"),
            input("GET", "/api/artifacts", "h", 101, &n1, b"", "t"),
            input("GET", "/api/artifacts", "h", 100, &n2, b"", "t"),
            input("GET", "/api/artifacts", "h", 100, &n1, b"x", "t"),
            input("GET", "/api/artifacts", "h", 100, &n1, b"", "u"),
            SignInput {
                token_sha256: None,
                ..i
            },
            SignInput {
                agent_headers_sha256: agent_headers_sha256(&[
                    Some(b"".as_slice()),
                    None,
                    None,
                    None,
                    None,
                ]),
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
    fn control_characters_and_wrong_lengths_never_sign_or_verify() {
        let k = KeyPair::generate();
        let n = nn(1);
        let good = input("GET", "/a", "h", 1, &n, b"", "t");
        let sig = k.sign(&good).unwrap().sig;
        for (m, p, host) in [
            ("GET", "/a\nGET", "h"),
            ("GET\r", "/a", "h"),
            ("GET", "/a", "h\n1"),
        ] {
            let i = input(m, p, host, 1, &n, b"", "t");
            assert_eq!(k.sign(&i), Err(SigError::Bad), "{i:?}");
            // A signature made over the same canonical bytes by hand still
            // does not verify.
            let raw: [u8; SIG_LEN] = k.pair.sign(&i.canonical()).as_ref().try_into().unwrap();
            assert!(!verify(&k.public(), &i, &raw), "{i:?}");
        }
        for bad_nonce in ["n1", "\0", &format!("{n}A"), &format!("{}+", &n[1..])] {
            let i = input("GET", "/a", "h", 1, bad_nonce, b"", "t");
            assert_eq!(k.sign(&i), Err(SigError::Bad), "{bad_nonce:?}");
        }
        let public = k.public();
        assert!(!verify(&public[..31], &good, &sig));
        assert!(!verify(&[public.as_slice(), &[0]].concat(), &good, &sig));
        assert!(!verify(&public, &good, &sig[..63]));
        assert!(!verify(&public, &good, &[sig.as_slice(), &[0]].concat()));
        assert!(!verify(&[], &good, &[]));
    }

    #[test]
    fn replay_and_staleness_are_refused() {
        let mut c = NonceCache::new(240, 4);
        let key = [7u8; 32];
        let [a, b, cc, d, e, f] = [1, 2, 3, 4, 5, 6].map(nn);
        assert!(c.record(&key, &a, 1000, 1000).is_ok());
        assert_eq!(c.check(&key, &a, 1000, 1001), Err(SigError::Replayed));
        assert_eq!(c.record(&key, &a, 1000, 1001), Err(SigError::Replayed));
        assert_eq!(c.check(&key, &b, 1000, 1121), Err(SigError::Stale));
        assert_eq!(c.check(&key, &b, 1000, 879), Err(SigError::Stale));
        assert!(c.check(&key, &b, 1000, 1120).is_ok(), "120 s is in time");
        assert!(c.check(&key, &b, 1000, 880).is_ok());
        for n in [&b, &cc, &d] {
            c.record(&key, n, 1000, 1000).unwrap();
        }
        assert_eq!(c.check(&key, &e, 1000, 1000), Err(SigError::Full));
        assert_eq!(c.record(&key, &e, 1000, 1000), Err(SigError::Full));
        assert_eq!(
            c.record(&key, &a, 1000, 1000),
            Err(SigError::Replayed),
            "a replay at the cap is still a replay"
        );
        assert!(
            c.record(&[8u8; 32], &a, 1000, 1000).is_ok(),
            "each key has its own nonces"
        );
        assert_eq!(
            c.check(&key, &a, 1240, 1240),
            Err(SigError::Replayed),
            "a nonce is remembered for the whole window"
        );
        assert!(
            c.record(&key, &f, 1300, 1300).is_ok(),
            "entries older than the window are evicted"
        );
        assert!(c.record(&key, &a, 1300, 1300).is_ok());
        assert_eq!(c.check(&key, "a", 1300, 1300), Err(SigError::Bad));
        assert_eq!(c.record(&key, "a", 1300, 1300), Err(SigError::Bad));
    }

    #[test]
    fn only_record_spends_a_nonce() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [1u8; 32];
        for _ in 0..3 {
            c.check(&key, &nn(1), 50, 50).unwrap();
        }
        assert!(c.is_empty() && c.keys() == 0, "a check spends nothing");
        c.record(&key, &nn(1), 50, 50).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_replay_stays_refused_while_its_time_is_fresh() {
        // Recorded at the earliest `now` its `t` allows, the nonce is still
        // remembered at the latest.
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [2u8; 32];
        let t = 10_000;
        c.record(&key, &nn(1), t, t - MAX_SKEW_S).unwrap();
        assert_eq!(
            c.check(&key, &nn(1), t, t + MAX_SKEW_S),
            Err(SigError::Replayed)
        );
        assert_eq!(
            c.check(&key, &nn(1), t, t + MAX_SKEW_S + 1),
            Err(SigError::Stale)
        );
    }

    #[test]
    fn a_slow_body_is_recorded_once() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [5u8; 32];
        let t = 10_000;
        let n = nn(9);
        c.check(&key, &n, t, t).unwrap();
        // The body took 300 s; the request verified.
        assert!(c.record(&key, &n, t, t + 300).is_ok());
        assert_eq!(c.record(&key, &n, t, t + 300), Err(SigError::Replayed));
        assert_eq!(c.check(&key, &n, t + 300, t + 300), Err(SigError::Replayed));
        // Another, checked at t and recorded at t + 500: a replay of it
        // within its time is refused.
        let m = nn(10);
        c.check(&key, &m, t, t).unwrap();
        c.record(&key, &m, t, t + 500).unwrap();
        assert_eq!(c.check(&key, &m, t + 500, t + 500), Err(SigError::Replayed));
    }

    #[test]
    fn a_clock_stepped_back_keeps_the_nonce_through_its_time() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [6u8; 32];
        let t = 10_000;
        let n = nn(3);
        c.check(&key, &n, t, t - MAX_SKEW_S).unwrap();
        // The clock stepped back 10 s before the record.
        c.record(&key, &n, t, t - MAX_SKEW_S - 10).unwrap();
        assert_eq!(
            c.check(&key, &n, t, t + MAX_SKEW_S),
            Err(SigError::Replayed)
        );
    }

    #[test]
    fn a_record_far_in_the_future_is_refused_and_holds_nothing() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, 4);
        let key = [7u8; 32];
        assert_eq!(c.record(&key, &nn(1), 1_000_000, 0), Err(SigError::Stale));
        assert!(c.is_empty() && c.keys() == 0);
        // The furthest a checked `t` and a stepped-back clock reach records.
        let edge = MAX_SKEW_S + NONCE_WINDOW_S;
        assert!(c.record(&key, &nn(2), edge, 0).is_ok());
        assert_eq!(c.record(&key, &nn(3), edge + 1, 0), Err(SigError::Stale));
        // The key's later nonces still record once their time comes.
        for i in 4..7 {
            c.record(&key, &nn(i), 600, 600).unwrap();
        }
    }

    #[test]
    fn the_header_and_the_cache_test_a_nonce_alike() {
        let s = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 64]);
        // 22 base64url characters whose last one carries nonzero unused bits.
        let loose = format!("{}B", "A".repeat(21));
        assert_eq!(
            parse_header(format!("t=5, n={loose}, s={s}")),
            Err(SigError::Bad)
        );
        let mut c = NonceCache::new(NONCE_WINDOW_S, 4);
        assert_eq!(c.check(&[1u8; 32], &loose, 5, 5), Err(SigError::Bad));
        let fine = nn(1);
        assert!(parse_header(format!("t=5, n={fine}, s={s}")).is_ok());
        assert!(c.check(&[1u8; 32], &fine, 5, 5).is_ok());
    }

    #[test]
    fn forgetting_a_key_frees_its_room() {
        let mut c = NonceCache::new(240, 4).with_caps(2, 100);
        c.record(&[1u8; 32], &nn(1), 0, 0).unwrap();
        c.record(&[2u8; 32], &nn(1), 0, 0).unwrap();
        assert_eq!(c.check(&[3u8; 32], &nn(1), 0, 0), Err(SigError::Full));
        c.forget(&[1u8; 32]);
        assert!(!c.has_key(&[1u8; 32]) && c.has_key(&[2u8; 32]));
        assert_eq!((c.len(), c.keys(), c.rotation.len()), (1, 1, 1));
        c.record(&[3u8; 32], &nn(1), 0, 0).unwrap();
        c.forget(&[9u8; 32]);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn sweep_forgets_old_nonces_and_idle_keys() {
        let mut c = NonceCache::new(240, 8);
        c.record(&[1u8; 32], &nn(1), 0, 0).unwrap();
        c.record(&[2u8; 32], &nn(1), 200, 200).unwrap();
        // The first bucket holds seconds 0 to 14: kept through 254.
        c.sweep(254);
        assert_eq!(c.len(), 2);
        c.sweep(255);
        assert_eq!((c.len(), c.keys()), (1, 1), "an idle key is dropped");
        c.sweep(1000);
        assert!(c.is_empty() && c.keys() == 0);
        assert!(c.rotation.is_empty());
    }

    #[test]
    fn one_shot_keys_stay_bounded_without_a_sweep() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let mut most = 0;
        // Ten new keys a second for an hour, each used once.
        for i in 0..36_000u32 {
            let now = i64::from(i / 10);
            let mut key = [0u8; 32];
            key[..4].copy_from_slice(&i.to_be_bytes());
            c.record(&key, &nn(i), now, now).unwrap();
            most = most.max(c.keys());
        }
        // Retention (a window and a bucket, 255 s) holds 2,550 of them;
        // tidying four keys a call holds at most 4/3 of that.
        assert!(most <= 2_550 * 4 / 3, "{most}");
        assert_eq!(c.keys(), c.len());
        assert_eq!(c.rotation.len(), c.keys());
    }

    #[test]
    fn keys_and_nonces_overall_are_capped() {
        let mut c = NonceCache::new(240, 100).with_caps(3, 5);
        for k in 1..=3u8 {
            c.record(&[k; 32], &nn(1), 0, 0).unwrap();
        }
        assert_eq!(c.check(&[4; 32], &nn(1), 0, 0), Err(SigError::Full));
        assert_eq!(c.record(&[4; 32], &nn(1), 0, 0), Err(SigError::Full));
        c.record(&[1; 32], &nn(2), 0, 0).unwrap();
        c.record(&[2; 32], &nn(2), 0, 0).unwrap();
        assert_eq!(c.record(&[3; 32], &nn(2), 0, 0), Err(SigError::Full));
        // Once the window passes, tidying frees both caps.
        for i in 0..4 {
            let _ = c.check(&[1; 32], &nn(10 + i), 300, 300);
        }
        assert_eq!(c.keys(), 0);
        c.record(&[4; 32], &nn(1), 300, 300).unwrap();
    }

    #[test]
    fn extreme_times_neither_panic_nor_pass() {
        let mut c = NonceCache::new(NONCE_WINDOW_S, NONCES_PER_KEY);
        let key = [3u8; 32];
        for (t, now) in [(i64::MIN, i64::MAX), (i64::MAX, i64::MIN), (i64::MAX, 0)] {
            assert_eq!(c.check(&key, &nn(1), t, now), Err(SigError::Stale));
        }
        c.record(&key, &nn(1), i64::MAX, i64::MAX).unwrap();
        c.record(&key, &nn(2), i64::MIN, i64::MIN).unwrap();
        c.record(&[9; 32], &nn(2), i64::MIN, i64::MIN).unwrap();
        c.sweep(i64::MAX);
        c.sweep(i64::MIN);
        let _ = c.check(&key, &nn(3), i64::MIN, i64::MIN);
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
        let h = back.sign(&i).unwrap();
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

    /// The vector `web/sdk/src/sign.test.ts` checks the TypeScript signer
    /// against: a fixed key (PKCS#8 v2 of the seed `[0x42; 32]`, as a
    /// queue service grant's `private_key` carries one) signs a reporter
    /// request with no agent headers.
    #[test]
    fn sdk_vector_is_fixed() {
        let seed = [0x42u8; 32];
        let public = decode_public("IVL40Zt5HSRFMkLhXy6rbLfP-ntqXtMAl5YOBpiB2xI").unwrap();
        let mut doc = vec![
            0x30, 0x53, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
            0x04, 0x20,
        ];
        doc.extend_from_slice(&seed);
        doc.extend_from_slice(&[0xa1, 0x23, 0x03, 0x21, 0x00]);
        doc.extend_from_slice(&public);
        let k = KeyPair::from_pkcs8(&doc).unwrap();
        assert_eq!(k.public(), public);
        assert_eq!(
            crate::secret::base64url(&doc),
            "MFMCAQEwBQYDK2VwBCIEIEJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCoSMDIQAhUvjRm3kdJEUyQuFfLqtst8_6e2pe0wCXlg4GmIHbEg"
        );
        let i = input(
            "POST",
            "/api/queues/q1/reporter/reports?cursor=c1",
            "clax.example:8443",
            1_760_000_000,
            "AAECAwQFBgcICQoLDA0ODw",
            br#"{"body":"hi"}"#,
            "cxr_vector",
        );
        assert_eq!(
            String::from_utf8(i.canonical()).unwrap(),
            "clax-ed25519 v2\n\
             POST\n\
             /api/queues/q1/reporter/reports?cursor=c1\n\
             clax.example:8443\n\
             1760000000\n\
             AAECAwQFBgcICQoLDA0ODw\n\
             8f1de4697a700dd8df293b4b51025bff262ca9f65a2af030cd50acb3818eebfd\n\
             1c2cec895430450c4ba5de411b6e3fa57ec583b47af14a0901bb47d3743e9ab1\n\
             a00d0d823baf03fa53cba1260dd2f71fc7e8694679ce444737a7168dfab351e8"
        );
        let h = k.sign(&i).unwrap();
        assert_eq!(
            h.to_string(),
            "t=1760000000, n=AAECAwQFBgcICQoLDA0ODw, \
             s=8DwcAbZkGf5Qhq47ATzagkKAuIr-T4h6vKczyevVcv1UchPiD_ba5rmjr8HyInFyYULBDia1-PZesVfaOyxtBA"
        );
        assert!(verify(&public, &i, &h.sig));
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
        let n = nn(1);
        let i = input("GET", "/", "h", 1, &n, b"", "t");
        let h = k.sign(&i).unwrap();
        let good = h.to_string().into_bytes();
        let mut c = NonceCache::new(NONCE_WINDOW_S, 16).with_caps(64, 512);
        let (mut parsed, mut reached) = (0, 0);
        for round in 0..1_000 {
            let len = rng.random_range(0..200);
            let mut b: Vec<u8> = (0..len).map(|_| rng.random()).collect();
            // Half the rounds mutate a valid header, to reach deeper.
            if round % 2 == 0 {
                b = good.clone();
                // Mostly bytes of the header's own alphabet, so that many
                // still parse.
                let alphabet = b"AQgw09-_,= tns";
                for _ in 0..rng.random_range(1..4) {
                    let at = rng.random_range(0..b.len());
                    b[at] = if rng.random_bool(0.8) {
                        alphabet[rng.random_range(0..alphabet.len())]
                    } else {
                        rng.random()
                    };
                }
            }
            if let Ok(p) = parse_header(&b) {
                parsed += 1;
                let r = SignInput {
                    t: p.t,
                    nonce: &p.nonce,
                    ..i
                };
                // Verifies only if the mutation left the header intact.
                assert_eq!(verify(&k.public(), &r, &p.sig), p == h);
                let now = rng.random_range(-200..200);
                let _ = c.check(&k.public(), &p.nonce, p.t, now);
                let _ = c.record(&k.public(), &p.nonce, p.t, now);
            }
            let _ = KeyPair::from_pkcs8(&b);
            let text = String::from_utf8_lossy(&b);
            let _ = decode_public(&text);
            let split = b.len() / 2;
            assert!(!verify(&b[..split], &i, &b[split..]));
            // Correctly sized keys and signatures reach ring's verify.
            let key: [u8; PUBLIC_LEN] = rng.random();
            let mut sig = [0u8; SIG_LEN];
            rng.fill(&mut sig[..]);
            assert!(!verify(&key, &i, &sig));
            assert!(!verify(&k.public(), &i, &sig));
            if rng.random_bool(0.1) {
                // Also with a real signature's halves kept.
                sig[..32].copy_from_slice(&h.sig[..32]);
                assert!(!verify(&key, &i, &sig));
            }
            reached += 1;
            let r = SignInput {
                method: &text,
                path_and_query: &text,
                host: &text,
                nonce: &text,
                t: rng.random(),
                ..i
            };
            assert!(!verify(&k.public(), &r, &h.sig));
            let _ = k.sign(&r);
            let mut key = [0u8; PUBLIC_LEN];
            key[0] = rng.random_range(0..100);
            let (t, now) = (rng.random(), rng.random());
            let _ = c.check(&key, &text, t, now);
            let _ = c.record(&key, &text, t, now);
            let _ = c.record(&key, &nn(rng.random()), t, now);
        }
        assert!(parsed > 100 && reached == 1_000, "{parsed}");
        assert!(c.keys() <= 64 && c.len() <= 512);
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
        /// Heap bytes the buckets' tables hold: a `u128` and a control
        /// byte per slot (hashbrown), plus the bucket structs.
        fn table_bytes(c: &NonceCache) -> usize {
            c.keys
                .values()
                .flat_map(|k| &k.buckets)
                .map(|b| b.nonces.capacity() * 17 * 8 / 7 + size_of::<Bucket>())
                .sum()
        }
        let key = [4u8; 32];
        let cap = NONCES_PER_KEY;
        let nonces: Vec<String> = (0..2 * cap as u32).map(nn).collect();
        let mut c = NonceCache::new(NONCE_WINDOW_S, cap);
        let start = Instant::now();
        for n in &nonces[..cap] {
            c.record(&key, n, 0, 0).unwrap();
        }
        let fill = start.elapsed();
        let full_key_bytes = table_bytes(&c);
        let start = Instant::now();
        for n in &nonces[cap..] {
            assert_eq!(c.check(&key, n, 0, 0), Err(SigError::Full));
        }
        let full = start.elapsed();
        // The whole cap expiring at once, on one record.
        let start = Instant::now();
        c.record(&key, &nn(u32::MAX), 300, 300).unwrap();
        let burst = start.elapsed();
        assert_eq!(c.len(), 1);
        // Steady state at the cap: an even rate filling the retention
        // (window plus a bucket), each new bucket dropping an old one.
        let mut c = NonceCache::new(NONCE_WINDOW_S, cap);
        let per = cap as i64 / (NONCE_WINDOW_S + NONCE_WINDOW_S / BUCKETS);
        for (j, n) in nonces[..cap].iter().enumerate() {
            let now = j as i64 / per;
            c.record(&key, n, now, now).unwrap();
        }
        let start = Instant::now();
        let base = cap as i64 / per;
        let mut worst = std::time::Duration::ZERO;
        for (j, n) in nonces[cap..].iter().enumerate() {
            let now = base + j as i64 / per;
            let one = Instant::now();
            c.record(&key, n, now, now).unwrap();
            worst = worst.max(one.elapsed());
        }
        let steady = start.elapsed();
        // The overall cap: sixteen saturated keys.
        let mut c = NonceCache::new(NONCE_WINDOW_S, cap);
        for k in 0..(MAX_NONCES / cap) as u8 {
            for n in &nonces[..cap] {
                c.record(&[k; 32], n, 0, 0).unwrap();
            }
        }
        assert_eq!(c.record(&[99; 32], &nonces[0], 0, 0), Err(SigError::Full));
        let all_bytes = table_bytes(&c);

        let k = KeyPair::generate();
        let n = nn(1);
        let i = input("POST", "/api/artifacts", "h", 1, &n, b"{}", "t");
        let start = Instant::now();
        let mut h = k.sign(&i).unwrap();
        for _ in 0..999 {
            h = k.sign(&i).unwrap();
        }
        let sign = start.elapsed() / 1000;
        let start = Instant::now();
        for _ in 0..1000 {
            assert!(verify(&k.public(), &i, &h.sig));
        }
        let ver = start.elapsed() / 1000;
        let n = cap as u32;
        println!(
            "fill {:?}/record, full-cap check {:?}, steady record {:?} (worst {worst:?}), \
             expiring a full key {burst:?}, full key {} KiB, {MAX_NONCES} nonces {} MiB, \
             sign {sign:?}, verify {ver:?}",
            fill / n,
            full / n,
            steady / n,
            full_key_bytes / 1024,
            all_bytes / (1024 * 1024),
        );
    }
}

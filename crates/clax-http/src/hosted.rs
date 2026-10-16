//! Hosted mode: the daemon serving the public internet at one origin
//! (`[hosted] public_origin`, [`clax_core::config::HostedConfig`]), behind a
//! proxy that terminates TLS and keeps the `Host` header. With it off (no
//! `[hosted]`), nothing here runs and the local daemon is unchanged.
//!
//! People sign in through the browser ([`clax_store::hosted`]):
//!
//! - `/setup` makes the first person, the owner, once, with the setup
//!   secret the daemon read from its environment at start;
//! - `/join/<code>` redeems an invite the owner made on People, making a
//!   connected person and signing their browser in;
//! - `/signin/<code>` redeems a one-use, ten-minute link a signed-in member
//!   made for another device;
//! - a guest is someone with the link of a page shared by link who gave a
//!   name (`PUT /api/viewers/me`), capped at commenting there.
//!
//! The sign-in pages say whose account a code signs into before it is
//! redeemed (`GET /api/hosted/invites/<code>`, `GET
//! /api/hosted/signin-links/<code>`), and a redemption that would end the
//! browser's live session of someone else answers 409 `session_exists`
//! unless its body confirms it (`{"replace": true}`): a link someone sent
//! cannot sign a browser in as them behind its person's back.
//!
//! A browser carries the session cookie: `HttpOnly`, `SameSite=Lax`,
//! `Path=/`, host-only, thirty days; on an `https` origin `Secure` and named
//! `__Host-clax_session`, else `clax_session`.
//! [`gate`] runs before every route:
//!
//! - the `Host` must be the public origin's, the content origin's
//!   ([`Hosted::with_content_origin`]), a local name (the CLI and agents
//!   on the machine), or an artifact's content host;
//! - a write (any method but `GET`, `HEAD` and `OPTIONS`) without an
//!   `Authorization` header must carry `Origin` equal to the public origin
//!   (CSRF), else 403 `forbidden_origin`;
//! - the `clax connect` browser link answers 404, and `GET /api/token`
//!   hands the owner's browser its session's shell token ([`shell_token`])
//!   and anyone else 403 `not_owner`: no request is from this machine, so
//!   nothing hands out the daemon token. The gate takes the shell token,
//!   beside the session's cookie, as the daemon token;
//! - content is served only at its key's address: a version's files at
//!   `/c/<aid>/v/<n>/k/<key>/…` ([`content_key`]), an asset's bytes at
//!   `/_blob/<id>?k=<key>` ([`asset_key`]), each with no referrer, else 404,
//!   on the public host and the content host alike; with a content origin,
//!   there and on an artifact's content host only, never on the public
//!   host, and the content origin serves nothing else but the UI's files
//!   and `/healthz`. The content frame is
//!   sandboxed, so its requests carry no cookie: the key, which only the
//!   routes that hand the version to someone who may read it carry, is the
//!   capability. An ID alone reads nothing. An agent's draft revision is
//!   served at its draft URL (`/c/<aid>/draft/<agent>/<token>/…`), whose
//!   token is the
//!   capability in the same way: minted per revision, it travels only in
//!   the drafts listing and the `drafts` events, which reach only people
//!   who may read the artifact ([`crate::routes::drafts`]);
//! - the shell's static files and `/healthz` are open;
//! - a request with `Authorization` (the daemon token, a grant, an agent
//!   token) is as on a local daemon, and so is the owner's session; any
//!   other `Authorization` answers 401 `unauthorized`;
//! - a person's session is limited by the person's rights, as their agents
//!   are ([`MemberRights`]): an artifact they may not read (by their rights,
//!   or its link's) answers 404 wherever a route names it, listings leave it
//!   out, a route's action needs its right (`crate::agent::Agent`), and the
//!   gallery's stream topic is theirs only when they read every artifact;
//! - anyone else (a guest, or no session) reaches only the routes of pages
//!   shared by link ([`guest_need`]), within the page's
//!   [`LinkAccess::rights`] (a guest's) or its viewer rights (no session);
//!   a write a guest could make answers 401 `guest_name_required` to no
//!   session; either may ask for a read git token of the page
//!   ([`crate::git_remotes`], rate limited as [`Attempt::GitToken`]); the
//!   gallery pages redirect to `/signin`, and the rest answers 401
//!   `sign_in_required`.
//!
//! Agent tokens a member mints here are bearer grants (their rights the
//! person's): `Authorization: Bearer cxa_…` is an agent credential
//! ([`crate::agent::authenticate`]) on every route agents of other
//! machines reach, and on the tool set's JSON-RPC adapter (`/mcp` on a
//! Worker, `/mcp/json` on the daemon, [`crate::routes::mcp_http`]).

use crate::error::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};
use clax_core::rights::RightType;
use clax_store::hosted::{LinkAccess, SessionViewer, SignedIn};
use clax_core::{CoreError, Secret};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The session cookie's name on an `http` origin (development on
/// localhost).
pub const SESSION_COOKIE: &str = "clax_session";
/// The session cookie's name on an `https` origin: the `__Host-` prefix
/// makes the browser refuse it unless it is `Secure`, host-only and
/// `Path=/`, so a sibling subdomain cannot set one for this host (a
/// session it chose, or a second cookie that signs the browser out).
pub const SECURE_SESSION_COOKIE: &str = "__Host-clax_session";
/// The most a body the gate reads to find its artifact may hold.
const PEEK_LIMIT: usize = 64 * 1024;

/// Hosted mode's settings, read once at start.
pub struct Hosted {
    /// The public origin, normalized (`https://clax.example`).
    pub origin: String,
    /// Its authority as a `Host` header names it (`clax.example`,
    /// `localhost:7490`).
    pub host: String,
    /// The origin is `https`: the cookie is `Secure`.
    pub secure: bool,
    /// The setup secret's SHA-256; `None` keeps setup closed.
    setup_secret: Option<[u8; 32]>,
    /// The daemon is behind Cloudflare, whose `CF-Connecting-IP` names the
    /// client ([`Hosted::client`]).
    behind_cloudflare: bool,
    /// Each client's attempts at setup, sign-in and naming a guest.
    limits: Limits,
    /// The content origin ([`Hosted::with_content_origin`]): its origin and
    /// its authority as a `Host` header names it.
    content: Option<(String, String)>,
    /// GitHub sign-in ([`crate::github`], [`Hosted::with_github`]).
    pub github: Option<crate::github::GitHub>,
    /// Each GitHub member's caps.
    pub member_caps: clax_store::github::MemberCaps,
    /// Whether pages may be listed in the public gallery
    /// ([`Hosted::with_public_gallery`]); off, nothing is listed and no
    /// one may list a page.
    pub public_gallery: bool,
}

impl std::fmt::Debug for Hosted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hosted")
            .field("origin", &self.origin)
            .field("content_origin", &self.content_origin())
            .field("setup_open", &self.setup_secret.is_some())
            .field("behind_cloudflare", &self.behind_cloudflare)
            .field("github", &self.github)
            .finish()
    }
}

/// What a client's attempts are counted for ([`Limits`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Attempt {
    /// `POST /api/hosted/setup`, the setup secret's guesses.
    Setup,
    /// Looking up or redeeming an invite, and redeeming a sign-in link:
    /// codes' guesses.
    SignIn,
    /// Naming a guest (`PUT /api/viewers/me` with no session, or a
    /// guest's rename): a viewer row and a session each.
    Guest,
    /// Starting a device sign-in (`POST /api/hosted/device/code`): a
    /// waiting row each.
    Device,
    /// A machine's poll of its device sign-in
    /// (`POST /api/hosted/device/token`): device codes' guesses.
    DevicePoll,
    /// A read git token minted for a page's link (a guest, or no
    /// session): a token at the repository host each.
    GitToken,
    /// A GitHub sign-in that makes a new person ([`crate::github`]).
    SignUp,
}

impl Attempt {
    /// The most attempts at once, and the seconds that win one back.
    pub fn rule(self) -> (f64, f64) {
        match self {
            Attempt::Setup => (5.0, 120.0),
            Attempt::SignIn => (10.0, 30.0),
            Attempt::Guest | Attempt::GitToken => (5.0, 120.0),
            Attempt::Device => (5.0, 60.0),
            Attempt::DevicePoll => (30.0, 2.0),
            Attempt::SignUp => (5.0, 600.0),
        }
    }
}

/// The most clients [`Limits`] keeps; past it, those whose attempts are all
/// back are forgotten, and when none are, the one with the most back: a
/// client that spent its attempts is the last forgotten, so many addresses
/// cannot give one back its attempts.
const MAX_CLIENTS: usize = 10_000;

/// A token bucket per client and [`Attempt`], on an injected clock: each
/// attempt takes one; they come back at [`Attempt::rule`]'s pace.
pub struct Limits {
    clock: std::sync::Arc<dyn clax_core::working::Clock>,
    buckets: std::sync::Mutex<std::collections::HashMap<(Attempt, std::net::IpAddr), Bucket>>,
}

/// A bucket's attempts left and when they were last counted.
type Bucket = (f64, chrono::DateTime<chrono::Utc>);

impl Limits {
    pub fn new(clock: std::sync::Arc<dyn clax_core::working::Clock>) -> Limits {
        Limits {
            clock,
            buckets: std::sync::Mutex::default(),
        }
    }

    /// Takes one of `client`'s `what` attempts; `Err` with the seconds until
    /// the next one when none is left.
    pub fn take(&self, what: Attempt, client: std::net::IpAddr) -> Result<(), u64> {
        let now = self.clock.now();
        let (burst, per) = what.rule();
        let refill = |(tokens, at): (f64, chrono::DateTime<chrono::Utc>), per: f64, burst: f64| {
            let elapsed = (now - at).num_milliseconds().max(0) as f64 / 1000.0;
            (tokens + elapsed / per).min(burst)
        };
        let mut m = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= MAX_CLIENTS && !m.contains_key(&(what, client)) {
            m.retain(|(w, _), b| {
                let (burst, per) = w.rule();
                refill(*b, per, burst) < burst
            });
            if m.len() >= MAX_CLIENTS {
                let fullest = m
                    .iter()
                    .map(|((w, c), b)| {
                        let (burst, per) = w.rule();
                        (refill(*b, per, burst) / burst, (*w, *c))
                    })
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, k)| k);
                if let Some(k) = fullest {
                    m.remove(&k);
                }
            }
        }
        let b = m.entry((what, client)).or_insert((burst, now));
        let tokens = refill(*b, per, burst);
        if tokens >= 1.0 {
            *b = (tokens - 1.0, now);
            Ok(())
        } else {
            *b = (tokens, now);
            Err(((1.0 - tokens) * per).ceil() as u64)
        }
    }
}

/// The key a client's attempts are counted under: an IPv4 address (an
/// IPv4-mapped IPv6 one as IPv4), or an IPv6 address's /64, which one
/// subscriber holds whole.
pub(crate) fn client_key(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip.to_canonical() {
        std::net::IpAddr::V6(v6) => {
            let mut o = v6.octets();
            o[8..].fill(0);
            std::net::IpAddr::V6(std::net::Ipv6Addr::from(o))
        }
        v4 => v4,
    }
}

/// 429 `rate_limited`, naming the seconds until the next attempt
/// (`retry_after_s`).
fn rate_limited(retry_after: u64) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        format!("too many attempts from this address: try again in {retry_after} s"),
    );
    e.extra.insert("retry_after_s".into(), json!(retry_after));
    e
}

/// The authority of a normalized origin as a `Host` header names it
/// (`clax.example`, `localhost:7490`).
fn authority(origin: &str) -> Option<String> {
    let u = url::Url::parse(origin).ok()?;
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => Some(format!("{h}:{p}")),
        (Some(h), None) => Some(h.to_string()),
        _ => None,
    }
}

fn sha256(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

impl Hosted {
    /// Hosted mode at `origin` (normalized by
    /// [`clax_core::config::hosted_origin`], else `None`), whose setup takes
    /// `setup_secret` (`None` or empty: setup stays closed).
    pub fn new(origin: &str, setup_secret: Option<&str>) -> Option<Hosted> {
        let origin = clax_core::config::hosted_origin(origin)?;
        let host = authority(&origin)?;
        Some(Hosted {
            secure: origin.starts_with("https://"),
            host,
            origin,
            setup_secret: setup_secret.filter(|s| !s.is_empty()).map(sha256),
            behind_cloudflare: false,
            limits: Limits::new(std::sync::Arc::new(clax_core::working::SystemClock)),
            content: None,
            github: None,
            member_caps: Default::default(),
            public_gallery: false,
        })
    }

    /// This hosted mode with artifact content on an origin of its own,
    /// `origin` (normalized as the public origin is; its host must differ
    /// from the public origin's): every version's files, drafts and asset
    /// bytes are served there, at their keys' addresses, and never on the
    /// public origin, which serves everything else and no content; the
    /// content origin serves only content, the web UI's files and
    /// `/healthz`. Content is framed sandboxed there (one origin for every
    /// artifact), and its bridge talks to the public origin only. `None`
    /// for an origin hosted mode does not take, or the public one's host.
    pub fn with_content_origin(self, origin: &str) -> Option<Hosted> {
        let origin = clax_core::config::hosted_origin(origin)?;
        let host = authority(&origin)?;
        if host.eq_ignore_ascii_case(&self.host) {
            return None;
        }
        Some(Hosted {
            content: Some((origin, host)),
            ..self
        })
    }

    /// The content origin, when content has one of its own.
    pub fn content_origin(&self) -> Option<&str> {
        self.content.as_ref().map(|(o, _)| o.as_str())
    }

    /// Whether `host` (a `Host` header) is the content origin's.
    pub fn is_content_host(&self, host: &str) -> bool {
        self.content
            .as_ref()
            .is_some_and(|(_, h)| h.eq_ignore_ascii_case(host))
    }

    /// This hosted mode signing people in with GitHub too, `github`
    /// ([`crate::github`]), each GitHub member keeping at most
    /// `max_artifacts` artifacts.
    pub fn with_github(self, github: Option<crate::github::GitHub>, max_artifacts: u64) -> Hosted {
        Hosted {
            github,
            member_caps: clax_store::github::MemberCaps { max_artifacts },
            ..self
        }
    }

    /// This hosted mode with the public gallery on or off (`on`). Off, the
    /// default, `GET /api/hosted/public` lists nothing, a page's sharing
    /// state never reads `public`, and listing a page is refused.
    pub fn with_public_gallery(self, on: bool) -> Hosted {
        Hosted {
            public_gallery: on,
            ..self
        }
    }

    /// This hosted mode with its attempts counted on `clock`.
    pub fn with_clock(self, clock: std::sync::Arc<dyn clax_core::working::Clock>) -> Hosted {
        Hosted {
            limits: Limits::new(clock),
            ..self
        }
    }

    /// This hosted mode behind Cloudflare (`behind`): its clients are named
    /// by `CF-Connecting-IP`.
    pub fn behind_cloudflare(self, behind: bool) -> Hosted {
        Hosted {
            behind_cloudflare: behind,
            ..self
        }
    }

    /// Hosted mode as `[hosted]` sets it, the setup secret read from the
    /// environment variable it names.
    pub fn from_config(c: &clax_core::config::HostedConfig) -> Option<Hosted> {
        let secret = std::env::var(&c.setup_secret_env).ok();
        let github = c.github.as_ref().and_then(|g| {
            let secret = std::env::var(&g.client_secret_env).unwrap_or_default();
            crate::github::GitHub::new(
                &g.client_id,
                &secret,
                g.owner_login.as_deref(),
                g.oauth_base.as_deref(),
                g.api_base.as_deref(),
            )
        });
        Hosted::new(&c.public_origin, secret.as_deref()).map(|h| {
            h.behind_cloudflare(c.behind_cloudflare)
                .with_github(github, c.member_max_artifacts)
                .with_public_gallery(c.public_gallery)
        })
    }

    /// The client a request comes from: behind Cloudflare, the address its
    /// `CF-Connecting-IP` names (Cloudflare sets it, and it is trusted only
    /// when `[hosted] behind_cloudflare` says Cloudflare is in front); else,
    /// or without it, the connection's peer. On `wasm32` the daemon is a
    /// Worker, always behind Cloudflare and with no connection of its own
    /// ([`crate::auth::conn_of`] is `None`): `CF-Connecting-IP` alone names
    /// the client.
    pub fn client(
        &self,
        headers: &HeaderMap,
        extensions: &axum::http::Extensions,
    ) -> std::net::IpAddr {
        let behind = self.behind_cloudflare || cfg!(target_arch = "wasm32");
        let cf = behind
            .then(|| {
                headers
                    .get("cf-connecting-ip")?
                    .to_str()
                    .ok()?
                    .trim()
                    .parse()
                    .ok()
            })
            .flatten();
        cf.or_else(|| crate::identity::peer_of(extensions).map(|p| p.ip()))
            .unwrap_or(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED))
    }

    /// Takes one of the request's client's `what` attempts: 429
    /// `rate_limited` when none is left ([`Limits`]).
    ///
    /// # Errors
    /// As above.
    pub fn attempt(
        &self,
        what: Attempt,
        headers: &HeaderMap,
        extensions: &axum::http::Extensions,
    ) -> Result<(), ApiError> {
        let client = client_key(self.client(headers, extensions));
        self.limits.take(what, client).map_err(rate_limited)
    }

    /// Whether the daemon has a setup secret: `/setup` is open until the
    /// first person is made.
    pub fn setup_open(&self) -> bool {
        self.setup_secret.is_some()
    }

    /// Whether `given` is the setup secret (hashes compared in constant
    /// time).
    fn setup_secret_matches(&self, given: &str) -> bool {
        self.setup_secret
            .is_some_and(|want| clax_core::secret::constant_time_eq(&sha256(given), &want))
    }

    /// The session cookie's name: [`SECURE_SESSION_COOKIE`] on `https`,
    /// else [`SESSION_COOKIE`].
    pub fn cookie_name(&self) -> &'static str {
        if self.secure {
            SECURE_SESSION_COOKIE
        } else {
            SESSION_COOKIE
        }
    }

    fn cookie(&self, value: &str, max_age: i64) -> HeaderValue {
        let secure = if self.secure { "; Secure" } else { "" };
        let name = self.cookie_name();
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; Max-Age={max_age}; HttpOnly{secure}; SameSite=Lax"
        ))
        .expect("session tokens are header-safe")
    }

    /// `Set-Cookie` for session `token`.
    pub fn session_cookie(&self, token: &Secret) -> HeaderValue {
        self.cookie(token.expose(), clax_store::hosted::SESSION_TTL_SECS)
    }

    /// `Set-Cookie` that removes the session cookie.
    pub fn clear_cookie(&self) -> HeaderValue {
        self.cookie("", 0)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }
}

/// HMAC-SHA-256 of `msg` under `key` (RFC 2104).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.map(|x| x ^ b);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// The first 128 bits of the HMAC of `msg` under a key of `label`'s, made
/// from the daemon token `token`, in lowercase hex: a capability only this
/// daemon mints. A new token makes every one worthless.
pub(crate) fn keyed_mac(token: &str, label: &str, msg: &str) -> String {
    keyed(token, label, msg)
}

fn keyed(token: &str, label: &str, msg: &str) -> String {
    let key = Sha256::new()
        .chain_update(label.as_bytes())
        .chain_update(b"\n")
        .chain_update(token.as_bytes())
        .finalize();
    hmac_sha256(&key, msg.as_bytes())[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The content key of version `n` of artifact `aid`: on a hosted daemon,
/// its content is served only at `/c/<aid>/v/<n>/k/<key>/…` (and
/// `/v/<n>/k/<key>/…` on its content host), so its ID alone reads nothing.
/// The routes that hand a version to someone who may read it carry it
/// ([`stamp_content_keys`]).
pub fn content_key(token: &str, aid: &str, n: u64) -> String {
    keyed(token, "clax hosted content key", &format!("{aid}/{n}"))
}

/// The key of asset `asset_id`: on a hosted daemon, its bytes are served
/// only at `/_blob/<asset_id>?k=<key>`.
pub fn asset_key(token: &str, asset_id: &str) -> String {
    keyed(token, "clax hosted asset key", asset_id)
}

/// The URL a hosted daemon serves asset `asset_id` at; a local daemon's
/// `/_blob/<asset_id>`.
pub fn asset_url(s: &AppState, asset_id: &str) -> String {
    match &s.hosted {
        Some(h) => format!(
            "{}/_blob/{asset_id}?k={}",
            h.content_origin().unwrap_or(""),
            asset_key(&s.token, asset_id)
        ),
        None => format!("/_blob/{asset_id}"),
    }
}

/// On a hosted daemon, sets `content_key` on every version of view `v`
/// (its `version` and each of its `versions`, which name their
/// `artifact_id` and `n`), and with a content origin, `content_origin` on
/// the view: where the shell frames its content
/// (`<content_origin>/c/<aid>/v/<n>/k/<key>/…`, sandboxed) and its drafts.
/// A local daemon's views are left as they are.
pub fn stamp_content_keys(s: &AppState, v: &mut Value) {
    let Some(h) = &s.hosted else {
        return;
    };
    if let (Some(o), Some(m)) = (h.content_origin(), v.as_object_mut()) {
        m.insert("content_origin".into(), json!(o));
    }
    let stamp = |x: &mut Value| {
        let aid = x.get("artifact_id").and_then(Value::as_str);
        let n = x.get("n").and_then(Value::as_u64);
        if let (Some(aid), Some(n)) = (aid, n) {
            x["content_key"] = json!(content_key(&s.token, aid, n));
        }
    };
    if let Some(x) = v.get_mut("version") {
        stamp(x);
    }
    for x in v
        .get_mut("versions")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        stamp(x);
    }
}

/// On a hosted daemon, sets `url` on every asset of `assets` (each naming
/// its `id`) to [`asset_url`].
pub fn stamp_asset_urls(s: &AppState, assets: &mut [Value]) {
    if s.hosted.is_none() {
        return;
    }
    for a in assets {
        if let Some(id) = a.get("id").and_then(Value::as_str).map(str::to_string) {
            a["url"] = json!(asset_url(s, &id));
        }
    }
}

/// What a hosted daemon makes of a content path (`/c/…`).
#[derive(Debug, PartialEq, Eq)]
enum Content {
    /// `/c/<aid>/v/<n>/k/<key>/<rest>` with the version's key: the path the
    /// content routes serve, `/c/<aid>/v/<n>/<rest>`.
    Unlocked(String),
    /// `/c/<aid>/v/<n>/k/<key>` with the version's key: its index is at the
    /// key with a slash.
    Slash(String),
    /// `/c/<aid>/draft/…`: a draft URL, whose token the route checks.
    Draft,
    /// Anything else: no such content.
    Locked,
}

fn content_path(token: &str, path: &str) -> Content {
    let Some(rest) = path.strip_prefix("/c/") else {
        return Content::Locked;
    };
    if let Some((aid, tail)) = rest.split_once('/')
        && tail.starts_with("draft/")
        && clax_core::ArtifactId::parse(aid).is_ok()
    {
        return Content::Draft;
    }
    let mut parts = rest.splitn(6, '/');
    let (Some(aid), Some("v"), Some(n), Some("k"), Some(key)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Content::Locked;
    };
    let tail = parts.next();
    let (Ok(id), Ok(num)) = (clax_core::ArtifactId::parse(aid), n.parse::<u32>()) else {
        return Content::Locked;
    };
    // The version as the routes read it: no leading zeros, no sign.
    if num.to_string() != n {
        return Content::Locked;
    }
    let want = content_key(token, id.as_str(), u64::from(num));
    if !clax_core::secret::constant_time_eq(key.as_bytes(), want.as_bytes()) {
        return Content::Locked;
    }
    match tail {
        None => Content::Slash(format!("{key}/")),
        Some(t) => Content::Unlocked(format!("/c/{aid}/v/{n}/{t}")),
    }
}

/// Whether `/_blob/<asset_id>` with `query` carries the asset's key.
fn blob_unlocked(token: &str, path: &str, query: Option<&str>) -> bool {
    let Some(id) = path.strip_prefix("/_blob/") else {
        return false;
    };
    let given = query
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("k="))
        .unwrap_or("");
    let want = asset_key(token, id);
    clax_core::secret::constant_time_eq(given.as_bytes(), want.as_bytes())
}

/// The gate's handling of content (`/c/…`, `/_blob/…`): unlocked by its
/// key, served with no referrer (its URL is the key); else 404.
async fn serve_content(s: &AppState, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    if path.starts_with("/c/") {
        match content_path(&s.token, &path) {
            Content::Unlocked(p) => {
                let uri = match &query {
                    Some(q) => format!("{p}?{q}"),
                    None => p,
                };
                match uri.parse() {
                    Ok(u) => *req.uri_mut() = u,
                    Err(_) => return ApiError::not_found().into_response(),
                }
            }
            Content::Slash(to) => return no_referrer(Redirect::permanent(&to).into_response()),
            Content::Draft => {}
            Content::Locked => return ApiError::not_found().into_response(),
        }
    } else if !blob_unlocked(&s.token, &path, query.as_deref()) {
        return ApiError::not_found().into_response();
    }
    no_referrer(next.run(req).await)
}

fn no_referrer(mut res: Response) -> Response {
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
}

/// What [`gate`] found, on every request it lets through.
#[derive(Clone, Debug, Default)]
pub struct HostedRequest {
    /// Who the session cookie signs in as; `None` without a live session,
    /// and for a request with `Authorization`, which speaks for that
    /// credential alone.
    pub session: Option<SessionViewer>,
    /// The request's `Origin` is the public origin.
    pub origin_checked: bool,
    /// A person's session (a member who is not the owner): their rights,
    /// which limit their browser as they limit their agents.
    pub member: Option<MemberRights>,
    /// The session cookie's SHA-256 (lowercase hex), with a live session:
    /// what a socket's host keeps to check the session again when it
    /// reopens the socket ([`crate::identity::SocketCredentials`]).
    pub cookie_sha256: Option<String>,
}

/// A signed-in person's rights in their browser: the person's multi-person
/// rights ([`clax_store::Store::person_rights`]), and at an artifact shared
/// by link, its [`LinkAccess::rights`] too. The owner's browser holds every
/// right and carries none of this.
#[derive(Clone, Debug)]
pub struct MemberRights {
    /// The person's public ID: what `own` locations are resolved by.
    pub person: String,
    pub rights: std::sync::Arc<clax_core::rights::Rights>,
}

impl MemberRights {
    /// Whether these rights allow `action` of `kind` at artifact `aid` in
    /// `st` (the person's rights, or the artifact's link's).
    ///
    /// # Errors
    /// The store's.
    pub fn allows_at(
        &self,
        st: &clax_store::Store,
        (kind, action): (RightType, &str),
        aid: &str,
    ) -> clax_core::Result<bool> {
        let own = |x: &str| {
            st.artifact_created_by(x, Some(&self.person))
                .unwrap_or(false)
        };
        if self.rights.allows(kind, action, Some(aid), &own) {
            return Ok(true);
        }
        let none = |_: &str| false;
        Ok(st
            .link_access(aid)?
            .is_some_and(|a| a.rights(aid).allows(kind, action, Some(aid), &none)))
    }

    /// [`MemberRights::allows_at`] on the blocking pool.
    ///
    /// # Errors
    /// The store's.
    pub async fn allows(
        &self,
        s: &AppState,
        right: (RightType, &'static str),
        aid: &str,
    ) -> Result<bool, ApiError> {
        let (m, aid) = (self.clone(), aid.to_string());
        s.store_call(move |st| m.allows_at(st, right, &aid)).await
    }
}

impl MemberRights {
    /// The rights of session `who` when it is a person's who is not the
    /// owner (as the gate gives a request of theirs); `None` for the owner,
    /// a guest and anyone else.
    ///
    /// # Errors
    /// The store's.
    pub fn of_session(
        st: &clax_store::Store,
        who: &SessionViewer,
    ) -> clax_core::Result<Option<MemberRights>> {
        if who.owner || !who.member() {
            return Ok(None);
        }
        let pid = who.viewer.public_id.clone();
        Ok(Some(MemberRights {
            rights: std::sync::Arc::new(st.person_rights(&pid)?),
            person: pid,
        }))
    }
}

/// Whether `member` may read the comments on artifact `aid` (`clax-comments`
/// `read`, by their rights or the page's link), as `GET
/// /api/artifacts/<aid>/threads` asks; `None` (the owner, a guest, a
/// visitor, a local daemon's browser) is not limited here: a guest's and a
/// visitor's link always reads comments ([`LinkAccess::rights`]).
///
/// # Errors
/// The store's.
pub fn reads_comments(
    st: &clax_store::Store,
    member: Option<&MemberRights>,
    aid: &str,
) -> clax_core::Result<bool> {
    match member {
        None => Ok(true),
        Some(m) => m.allows_at(st, (RightType::Comments, "read"), aid),
    }
}

/// Whether session `who` may read artifact `aid` now, as the gate decides
/// a route under it: the owner always; a person by their rights or the
/// page's link ([`MemberRights::allows_at`]); a guest while the page is
/// shared by link. What a host reopening a socket of `aid` checks again.
///
/// # Errors
/// The store's.
pub fn session_reads(
    st: &clax_store::Store,
    who: &SessionViewer,
    aid: &str,
) -> clax_core::Result<bool> {
    if who.owner {
        return Ok(true);
    }
    if who.member() {
        let pid = who.viewer.public_id.clone();
        let m = MemberRights {
            rights: std::sync::Arc::new(st.person_rights(&pid)?),
            person: pid,
        };
        return m.allows_at(st, (RightType::Artifacts, "read"), aid);
    }
    Ok(st.link_access(aid)?.is_some())
}

/// What a person's request needs before its route ([`gate`]); the route
/// then checks the right its action needs (`crate::agent::Agent`).
#[derive(Debug, PartialEq, Eq)]
enum MemberNeed {
    /// Nothing more here.
    Pass,
    /// To read artifact `aid` (its page, or a route under its API).
    Read(String),
    /// A viewer mark naming its artifact in the body's `artifact_id`.
    MarkBody,
    /// A stream's subscriptions, in the body.
    StreamBody,
    /// The daemon-wide event stream (`/api/events`): the owner's here.
    Refused,
}

fn member_need(method: &Method, path: &str, query: Option<&str>) -> MemberNeed {
    use MemberNeed::*;
    let read = |aid: &str| match clax_core::ArtifactId::parse(aid) {
        Ok(a) => Read(a.as_str().to_string()),
        Err(_) => Pass,
    };
    let query_artifact = || {
        query
            .unwrap_or("")
            .split('&')
            .find_map(|kv| kv.strip_prefix("artifact="))
            .map(str::to_string)
    };
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segs.as_slice() {
        ["a", aid, ..] => read(aid),
        ["api", "artifacts", aid, ..] => read(aid),
        ["api", "viewers", "me", "attention" | "seen"] if method == Method::GET => {
            match query_artifact() {
                Some(aid) => read(&aid),
                None => Pass,
            }
        }
        ["api", "viewers", "me", "seen" | "looked" | "presence"] if method == Method::PUT => {
            MarkBody
        }
        ["api", "stream", _] if method == Method::POST => StreamBody,
        ["api", "events"] => Refused,
        _ => Pass,
    }
}

/// A person's request through the gate: what [`member_need`] says, an
/// artifact they may not read answering 404 as one that does not exist.
/// A stream subscription's artifact topics need reading their artifact
/// (`docs:` its db), and the gallery's topic, which carries every
/// artifact's titles, is left out unless their rights read every artifact.
async fn member_gate(s: &AppState, m: &MemberRights, req: Request, next: Next) -> Response {
    let query = req.uri().query().map(str::to_string);
    let need = member_need(req.method(), req.uri().path(), query.as_deref());
    let read = (RightType::Artifacts, "read");
    let check = async |aid: &str, right| match m.allows(s, right, aid).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ApiError::not_found().into_response()),
        Err(e) => Err(e.into_response()),
    };
    match need {
        MemberNeed::Pass => next.run(req).await,
        MemberNeed::Refused => ApiError::forbidden(
            "forbidden",
            "the daemon's event stream is the owner's here: follow topics on /api/stream",
        )
        .into_response(),
        MemberNeed::Read(aid) => match check(&aid, read).await {
            Ok(()) => next.run(req).await,
            Err(r) => r,
        },
        MemberNeed::MarkBody => {
            let (req, body) = match peek(req).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let aid = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("artifact_id")?.as_str().map(str::to_string));
            match aid.filter(|a| clax_core::ArtifactId::parse(a).is_ok()) {
                Some(aid) => match check(&aid, read).await {
                    Ok(()) => next.run(req).await,
                    Err(r) => r,
                },
                // The route answers a missing or malformed one.
                None => next.run(req).await,
            }
        }
        MemberNeed::StreamBody => {
            let (mut req, body) = match peek(req).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let Ok(mut v) = serde_json::from_slice::<Value>(&body) else {
                return next.run(req).await;
            };
            let topics: Vec<String> = v
                .get("subscribe")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let every = m
                .rights
                .allows(RightType::Artifacts, "read", None, &|_| false);
            let mut kept = Vec::new();
            for t in topics {
                match clax_realtime::stream::Topic::parse(&t) {
                    Ok(clax_realtime::stream::Topic::Gallery) if !every => continue,
                    Ok(clax_realtime::stream::Topic::Docs(aid)) => {
                        if let Err(r) = check(&aid, (RightType::Db, "read")).await {
                            return r;
                        }
                    }
                    Ok(topic) => {
                        if let Some(aid) = topic.artifact()
                            && let Err(r) = check(aid, read).await
                        {
                            return r;
                        }
                    }
                    // The route answers a malformed one.
                    Err(_) => {}
                }
                kept.push(Value::String(t));
            }
            if v.get("subscribe").is_some() {
                v["subscribe"] = Value::Array(kept);
                let bytes = serde_json::to_vec(&v).unwrap_or_default();
                req.headers_mut().remove(header::CONTENT_LENGTH);
                *req.body_mut() = Body::from(bytes);
            }
            next.run(req).await
        }
    }
}

impl HostedRequest {
    /// `id` as a hosted daemon counts it: nothing is from this machine, so
    /// no owner, events or `clax_viewer` cookie counts; the session's viewer
    /// stands for the cookie, and a session of the owner's row is a browser
    /// of the owner's.
    pub fn identity(&self, id: crate::identity::Identity) -> crate::identity::Identity {
        let s = self.session.as_ref();
        crate::identity::Identity {
            local: false,
            events_cookie: false,
            owner_cookie: false,
            cookie: None,
            hosted_viewer: s.filter(|s| !s.owner).map(|s| s.viewer.clone()),
            hosted_owner: s.is_some_and(|s| s.owner),
            ..id
        }
    }
}

/// The value of the one session cookie (named `name`) in `headers`; `None`
/// when there is none, or more than one (another page set a cookie of the
/// name).
/// The value of the one cookie named `name`; `None` without one, or with
/// more than one.
pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    session_token(headers, name)
}

fn session_token(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string());
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// What a request from a guest, or from no session, needs ([`gate`]).
#[derive(Debug, PartialEq, Eq)]
pub enum GuestNeed {
    /// Open to anyone: names by public ID, a stream with no topics yet.
    Open,
    /// `action` of `kind` at artifact `aid`; `viewer`: a guest's viewer
    /// row acts (a comment, a seen mark), which no session has.
    At {
        aid: String,
        kind: RightType,
        action: &'static str,
        viewer: bool,
    },
    /// A viewer mark naming its artifact in the body's `artifact_id`.
    MarkBody,
    /// A stream's subscriptions, in the body.
    StreamBody,
    /// `GET` or `PUT /api/viewers/me`.
    ViewerMe,
    /// A gallery page: the sign-in page instead.
    Gallery,
    /// Nothing a guest may reach.
    Refused,
}

/// What a guest's `method` `path` (with `query`) needs.
pub fn guest_need(method: &Method, path: &str, query: Option<&str>) -> GuestNeed {
    use GuestNeed::*;
    let get = method == Method::GET || method == Method::HEAD;
    let post = method == Method::POST;
    let at = |aid: &str, kind, action, viewer| match clax_core::ArtifactId::parse(aid) {
        Ok(a) => At {
            aid: a.as_str().to_string(),
            kind,
            action,
            viewer,
        },
        Err(_) => Refused,
    };
    let query_artifact = || {
        query
            .unwrap_or("")
            .split('&')
            .find_map(|kv| kv.strip_prefix("artifact="))
            .map(str::to_string)
    };
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segs.as_slice() {
        [""] | ["inbox"] | ["people"] if get => Gallery,
        ["a", aid, ..] if get => at(aid, RightType::Artifacts, "read", false),
        ["api", "artifacts"] if get => match query_artifact() {
            Some(aid) => at(&aid, RightType::Artifacts, "read", false),
            None => Refused,
        },
        ["api", "artifacts", aid] if get => at(aid, RightType::Artifacts, "read", false),
        [
            "api",
            "artifacts",
            aid,
            "versions" | "files" | "assets" | "presence" | "working",
        ] if get => at(aid, RightType::Artifacts, "read", false),
        ["api", "artifacts", aid, "versions", _] if get => {
            at(aid, RightType::Artifacts, "read", false)
        }
        ["api", "artifacts", aid, "versions", _, "files", ..] if get => {
            at(aid, RightType::Artifacts, "read", false)
        }
        ["api", "artifacts", aid, "snapshot"] if get => {
            at(aid, RightType::Artifacts, "read", false)
        }
        // Anyone who reads a page through its link may clone it read-only
        // (`crate::git_remotes::Asker::Link`): what the Clone dialog offers,
        // and the token.
        ["api", "artifacts", aid, "git-token"] if get || post => {
            at(aid, RightType::Artifacts, "read", false)
        }
        // Live layers and agents' drafts reach signed-in people: a guest,
        // not a visitor who gave no name.
        ["api", "artifacts", aid, "live" | "drafts"] if get => {
            at(aid, RightType::Artifacts, "read", true)
        }
        ["api", "artifacts", aid, "threads"] if get => at(aid, RightType::Comments, "read", false),
        ["api", "artifacts", aid, "threads"] if post => at(aid, RightType::Comments, "write", true),
        ["api", "artifacts", aid, "threads:send"] if post => {
            at(aid, RightType::Comments, "send", true)
        }
        ["api", "artifacts", aid, "threads", _] if get => {
            at(aid, RightType::Comments, "read", false)
        }
        ["api", "artifacts", aid, "threads", _, "clip"] if get => {
            at(aid, RightType::Comments, "read", false)
        }
        ["api", "artifacts", aid, "threads", _, "comments"] if post => {
            at(aid, RightType::Comments, "write", true)
        }
        ["api", "artifacts", aid, "threads", _, "send"] if post => {
            at(aid, RightType::Comments, "send", true)
        }
        ["api", "artifacts", aid, "threads", _, "resolve" | "reopen"] if post => {
            at(aid, RightType::Comments, "resolve", true)
        }
        ["api", "viewers"] if get => Open,
        ["api", "viewers", "me"] if get || method == Method::PUT => ViewerMe,
        ["api", "viewers", "me", "attention"] if get => match query_artifact() {
            Some(aid) => at(&aid, RightType::Artifacts, "read", true),
            None => Refused,
        },
        ["api", "viewers", "me", "seen" | "looked" | "presence"] if method == Method::PUT => {
            MarkBody
        }
        ["api", "stream"] if get => Open,
        ["api", "stream", _] if post => StreamBody,
        _ => Refused,
    }
}

fn sign_in_required() -> Response {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "sign_in_required",
        "sign in to Clax first: ask a member for an invite or a sign-in link",
    )
    .into_response()
}

fn guest_name_required() -> Response {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "guest_name_required",
        "give your name first to comment on this page",
    )
    .into_response()
}

fn not_permitted() -> Response {
    ApiError::forbidden("not_permitted", "this page's link does not allow that").into_response()
}

/// Whether a guest (`guest`) or no session may `action` of `kind` at
/// `aid`: `Ok` when it may, else the answer.
async fn guest_may(
    s: &AppState,
    guest: bool,
    aid: &str,
    kind: RightType,
    action: &str,
    viewer: bool,
) -> Result<(), Response> {
    let lookup = aid.to_string();
    let access = s
        .store_call(move |st| st.link_access(&lookup))
        .await
        .map_err(IntoResponse::into_response)?;
    let Some(access) = access else {
        // Not shared, or no such artifact: one answer for both.
        return Err(ApiError::not_found().into_response());
    };
    let none = |_: &str| false;
    let may = |a: LinkAccess| a.rights(aid).allows(kind, action, Some(aid), &none);
    let held = if guest { access } else { LinkAccess::Viewer };
    if may(held) && (guest || !viewer) {
        Ok(())
    } else if !guest && may(access) && action == "read" {
        // A viewer's own marks: no session has a viewer.
        Err(sign_in_required())
    } else if !guest && may(access) {
        Err(guest_name_required())
    } else {
        Err(not_permitted())
    }
}

/// Reads `req`'s body (up to [`PEEK_LIMIT`]) and puts it back.
async fn peek(req: Request) -> Result<(Request, axum::body::Bytes), Response> {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, PEEK_LIMIT).await.map_err(|_| {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "the request body is too large",
        )
        .into_response()
    })?;
    Ok((Request::from_parts(parts, Body::from(bytes.clone())), bytes))
}

/// The artifacts a stream subscription names, when every topic it
/// subscribes to is one a guest may hold: an artifact's, its presence's or
/// its working records'.
fn stream_artifacts(body: &[u8]) -> Option<Vec<String>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let mut out = Vec::new();
    let none = Vec::new();
    let subscribe = match v.get("subscribe") {
        None => &none,
        Some(s) => s.as_array()?,
    };
    for t in subscribe {
        match clax_realtime::stream::Topic::parse(t.as_str()?).ok()? {
            clax_realtime::stream::Topic::Artifact(a)
            | clax_realtime::stream::Topic::Presence(a)
            | clax_realtime::stream::Topic::Working(a) => out.push(a),
            _ => return None,
        }
    }
    Some(out)
}

/// The owner's shell token for the session whose cookie value is
/// `session`: `cxo_` and a key of the session's ([`keyed`]). `GET
/// /api/token` hands it to the owner's browser in place of the daemon
/// token, and the gate takes it, only beside that session's cookie, as the
/// daemon token (in `Authorization: Bearer`, or the `token` query parameter
/// a WebSocket carries). So the owner's shell holds what a local owner's
/// does without the daemon token ever reaching a browser, and signing out
/// ends it.
pub fn shell_token(token: &str, session: &str) -> String {
    format!(
        "{SHELL_TOKEN_PREFIX}{}",
        keyed(token, "clax hosted owner shell token", session)
    )
}

/// Prefix of [`shell_token`].
pub const SHELL_TOKEN_PREFIX: &str = "cxo_";

/// The shell token `req` presents, if any: `Authorization: Bearer cxo_…`,
/// or a `token=cxo_…` query parameter.
fn shell_token_presented(req: &Request) -> Option<String> {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, t)| t.trim_start_matches(' ').to_string());
    let query = req.uri().query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("token="))
            .map(str::to_string)
    });
    bearer
        .into_iter()
        .chain(query)
        .find(|t| t.starts_with(SHELL_TOKEN_PREFIX))
}

/// Puts the daemon token where `req` carried the shell token.
fn swap_shell_token(req: &mut Request, token: &str) -> Result<(), ApiError> {
    let bearer =
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| ApiError::unauthorized())?;
    if req.headers().contains_key(header::AUTHORIZATION) {
        req.headers_mut().insert(header::AUTHORIZATION, bearer);
    }
    if let Some(q) = req.uri().query() {
        let q: Vec<String> = q
            .split('&')
            .map(|kv| match kv.strip_prefix("token=") {
                Some(t) if t.starts_with(SHELL_TOKEN_PREFIX) => format!("token={token}"),
                _ => kv.to_string(),
            })
            .collect();
        let uri = format!("{}?{}", req.uri().path(), q.join("&"));
        *req.uri_mut() = uri.parse().map_err(|_| ApiError::unauthorized())?;
    }
    Ok(())
}

/// The session whose cookie value is `cookie`, while it is live.
async fn lookup_session(
    s: &AppState,
    cookie: Option<String>,
) -> Result<Option<SessionViewer>, Response> {
    match cookie {
        Some(t) => s
            .store_call(move |st| st.browser_session(&t))
            .await
            .map_err(IntoResponse::into_response),
        None => Ok(None),
    }
}

/// `GET /api/token` on a hosted daemon: the owner's browser gets its shell
/// token ([`shell_token`]), and no cookie; anyone else 403, as a browser
/// of another machine does from a local daemon.
fn token_for(s: &AppState, session: Option<&SessionViewer>, cookie: Option<&str>) -> Response {
    match (session, cookie) {
        (Some(who), Some(c)) if who.owner => {
            no_store(Json(json!({"token": shell_token(&s.token, c)})).into_response())
        }
        _ => ApiError::forbidden("not_owner", "only the owner's browser holds a token here")
            .into_response(),
    }
}

fn forbidden_origin(h: &Hosted) -> Response {
    ApiError::forbidden(
        "forbidden_origin",
        format!("a browser's change must come from {}", h.origin),
    )
    .into_response()
}

/// The gate of a hosted daemon, before every route (see the module docs).
pub async fn gate(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(h) = s.hosted.clone() else {
        return next.run(req).await;
    };
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let content_host = req
        .extensions()
        .get::<crate::host::OnArtifactOrigin>()
        .is_some();
    let on_content = h.is_content_host(&host);
    if !(host.eq_ignore_ascii_case(&h.host)
        || on_content
        || crate::auth::is_local_host(&host)
        || content_host)
    {
        return ApiError::forbidden(
            "forbidden_host",
            format!("this Clax answers at {}", h.origin),
        )
        .into_response();
    }
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let origin_checked = origin == Some(h.origin.as_str());
    let write = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
    if path.starts_with("/connect/b/") {
        return ApiError::not_found().into_response();
    }
    let content = path.starts_with("/c/") || path.starts_with("/_blob/");
    // The content origin serves content, the UI's files and `/healthz`
    // alone; with one, the public origin serves no content.
    if on_content && !content {
        if path == "/healthz" || path.starts_with("/_clax/") {
            req.extensions_mut().insert(HostedRequest::default());
            return next.run(req).await;
        }
        return ApiError::not_found().into_response();
    }
    if content {
        if h.content_origin().is_some() && !on_content && !content_host {
            return ApiError::not_found().into_response();
        }
        req.extensions_mut().insert(HostedRequest::default());
        return serve_content(&s, req, next).await;
    }
    let open = content_host || path == "/healthz" || path.starts_with("/_clax/");
    let cookie = (!open)
        .then(|| session_token(req.headers(), h.cookie_name()))
        .flatten();
    let shell = shell_token_presented(&req);
    // The owner's shell token stands for the daemon token beside the
    // owner's session ([`shell_token`]); it is the daemon token from here on.
    if let Some(presented) = &shell {
        let session = match lookup_session(&s, cookie.clone()).await {
            Ok(v) => v,
            Err(r) => return r,
        };
        let held = match (&session, &cookie) {
            (Some(who), Some(c)) if who.owner => {
                let want = shell_token(&s.token, c);
                clax_core::secret::constant_time_eq(presented.as_bytes(), want.as_bytes())
            }
            _ => false,
        };
        if !held {
            return ApiError::unauthorized().into_response();
        }
        if write && !origin_checked {
            return forbidden_origin(&h);
        }
        if let Err(e) = swap_shell_token(&mut req, &s.token) {
            return e.into_response();
        }
        let cookie_sha256 = session
            .as_ref()
            .and(cookie.as_deref())
            .map(clax_core::secret::sha256_hex);
        req.extensions_mut().insert(HostedRequest {
            session,
            origin_checked,
            member: None,
            cookie_sha256,
        });
        return next.run(req).await;
    }
    let headers = req.headers();
    let authz = headers.contains_key(header::AUTHORIZATION);
    // `Authorization` speaks for its credential alone, so it must be one:
    // the daemon token, or an agent credential (`Clax-Grant`, or an agent
    // token), which `crate::agent::authenticate` checks next. Anything
    // else would pass the gate as no one, whom a local daemon lets read.
    if authz
        && !crate::auth::is_grant_request(headers)
        && !crate::auth::has_token(headers, &s.token)
    {
        return ApiError::unauthorized().into_response();
    }
    // A device sign-in's machine routes carry no cookie and act for no
    // one: the device code is their only credential.
    let machine = matches!(
        path.as_str(),
        "/api/hosted/device/code" | "/api/hosted/device/token"
    );
    if write && !authz && !origin_checked && !machine {
        return forbidden_origin(&h);
    }
    let session = if authz {
        None
    } else {
        match lookup_session(&s, cookie.clone()).await {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
    if path == "/api/token" {
        return token_for(&s, session.as_ref(), cookie.as_deref());
    }
    // A person's rights limit their browser ([`MemberRights`]).
    let member = match session.as_ref().filter(|v| v.member() && !v.owner) {
        Some(who) => {
            let pid = who.viewer.public_id.clone();
            let lookup = pid.clone();
            match s.store_call(move |st| st.person_rights(&lookup)).await {
                Ok(r) => Some(MemberRights {
                    person: pid,
                    rights: std::sync::Arc::new(r),
                }),
                Err(e) => return e.into_response(),
            }
        }
        None => None,
    };
    req.extensions_mut().insert(HostedRequest {
        session: session.clone(),
        origin_checked,
        member: member.clone(),
        cookie_sha256: session
            .as_ref()
            .and(cookie.as_deref())
            .map(clax_core::secret::sha256_hex),
    });
    let own = path.starts_with("/api/hosted/")
        || path == "/setup"
        || path == "/signin"
        || path == "/device"
        || path.starts_with("/signin/")
        || path.starts_with("/join/")
        || path == "/auth/github"
        || path == crate::github::CALLBACK_PATH;
    if let Some(m) = member.as_ref().filter(|_| !open && !own) {
        return member_gate(&s, m, req, next).await;
    }
    if open || own || authz || session.as_ref().is_some_and(SessionViewer::member) {
        return next.run(req).await;
    }
    // A guest, or no session (a non-member's session is never a member's).
    let guest = session.as_ref().is_some_and(|v| v.guest);
    let query = req.uri().query().map(str::to_string);
    match guest_need(&method, &path, query.as_deref()) {
        GuestNeed::Open => next.run(req).await,
        GuestNeed::Gallery => Redirect::to("/signin").into_response(),
        GuestNeed::Refused => sign_in_required(),
        GuestNeed::At {
            aid,
            kind,
            action,
            viewer,
        } => match guest_may(&s, guest, &aid, kind, action, viewer).await {
            Ok(()) => next.run(req).await,
            Err(r) => r,
        },
        GuestNeed::MarkBody => {
            let (req, body) = match peek(req).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let aid = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("artifact_id")?.as_str().map(str::to_string));
            let Some(aid) = aid.filter(|a| clax_core::ArtifactId::parse(a).is_ok()) else {
                return sign_in_required();
            };
            match guest_may(&s, guest, &aid, RightType::Artifacts, "read", true).await {
                Ok(()) => next.run(req).await,
                Err(r) => r,
            }
        }
        GuestNeed::StreamBody => {
            let (req, body) = match peek(req).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let Some(aids) = stream_artifacts(&body) else {
                return not_permitted();
            };
            for aid in aids {
                if let Err(r) =
                    guest_may(&s, guest, &aid, RightType::Artifacts, "read", false).await
                {
                    return r;
                }
            }
            next.run(req).await
        }
        GuestNeed::ViewerMe => viewer_me(&s, &h, session.as_ref(), req, next).await,
    }
}

/// `GET` and `PUT /api/viewers/me` for a guest or no session: with no
/// session, `GET` answers a viewer of no public ID and no name (and mints
/// none), and `PUT {display_name}`
/// makes a guest of that name and signs them in; a guest's `GET` is the
/// usual one and their `PUT` renames them by the guests' rules.
async fn viewer_me(
    s: &AppState,
    h: &Hosted,
    session: Option<&SessionViewer>,
    req: Request,
    next: Next,
) -> Response {
    let get = req.method() == Method::GET || req.method() == Method::HEAD;
    match (session, get) {
        (Some(_), true) => next.run(req).await,
        // No viewer yet: one of no public ID and no name, as the shell
        // reads a viewer (naming themselves makes them a guest).
        (None, true) => no_store(
            Json(json!({"viewer": {"public_id": "", "display_name": null, "created_at": ""}}))
                .into_response(),
        ),
        (who, false) => {
            if let Err(e) = h.attempt(Attempt::Guest, req.headers(), req.extensions()) {
                return e.into_response();
            }
            let ctx = crate::audit::DeferredAudit::of_request(req.headers(), req.extensions(), s);
            let (_, body) = match peek(req).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let Some(name) = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("display_name")?.as_str().map(str::to_string))
            else {
                return ApiError::bad_request("invalid_json", "send {\"display_name\": <name>}")
                    .into_response();
            };
            match who {
                None => match s
                    .store_call(move |st| st.create_guest(&ctx.resolve(st)?, &name))
                    .await
                {
                    Ok(signed) => signed_in_response(h, signed, json!({"viewer": null}), "viewer"),
                    Err(e) => e.into_response(),
                },
                Some(g) => {
                    let id = g.viewer.id.clone();
                    match s.store_call(move |st| st.rename_guest(&id, &name)).await {
                        Ok(v) => {
                            for aid in s.presence.rename(&v.public_id, v.display_name.as_deref()) {
                                clax_realtime::presence::announce(&s.events, &s.presence, &aid);
                            }
                            no_store(Json(json!({"viewer": v})).into_response())
                        }
                        Err(e) => e.into_response(),
                    }
                }
            }
        }
    }
}

fn no_store(mut res: Response) -> Response {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// A person's view as the sign-in routes answer it.
fn person_view(who: &SessionViewer) -> Value {
    json!({
        "public_id": who.viewer.public_id,
        "display_name": who.viewer.display_name,
        "owner": who.owner,
        "guest": who.guest,
    })
}

/// The answer of a sign-in: `shape` with `key` set to the person (or, for
/// `"viewer"`, the viewer as `GET /api/viewers/me` gives it), the session's
/// expiry, and the cookie.
fn signed_in_response(h: &Hosted, signed: SignedIn, mut shape: Value, key: &str) -> Response {
    shape[key] = if key == "viewer" {
        serde_json::to_value(&signed.who.viewer).unwrap_or(Value::Null)
    } else {
        person_view(&signed.who)
    };
    shape["expires_at"] = json!(signed.expires_at);
    let mut res = no_store(Json(shape).into_response());
    res.headers_mut()
        .insert(header::SET_COOKIE, h.session_cookie(&signed.session));
    res
}

/// Ends the session a browser held before it signed in anew (`old`, its
/// cookie value), so a cookie it no longer holds signs no one in.
fn end_replaced(
    st: &clax_store::Store,
    ctx: &clax_core::audit::AuditCtx,
    old: Option<String>,
) -> clax_core::Result<()> {
    if let Some(t) = old {
        st.end_browser_session(ctx, &t, "replaced")?;
    }
    Ok(())
}

fn hosted_of(s: &AppState) -> std::sync::Arc<Hosted> {
    s.hosted
        .clone()
        .expect("hosted routes are served in hosted mode only")
}

/// The browser's session; 401 `sign_in_required` without one.
fn session_of(r: &HostedRequest) -> Result<&SessionViewer, ApiError> {
    r.session.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "sign_in_required",
            "sign in to Clax first",
        )
    })
}

/// The browser's member session (the owner's or a person's); 401 without
/// a session, 403 `forbidden` for a guest.
fn member_of<'a>(r: &'a HostedRequest, what: &str) -> Result<&'a SessionViewer, ApiError> {
    let who = session_of(r)?;
    if who.member() {
        Ok(who)
    } else {
        Err(ApiError::forbidden(
            "forbidden",
            format!("a guest may not {what}"),
        ))
    }
}

/// 403 `browser_required` for a request with `Authorization`: these routes
/// are the browser's.
fn browser_only(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::AUTHORIZATION) {
        Err(ApiError::forbidden(
            "browser_required",
            "this is a browser's route: open Clax in a browser",
        ))
    } else {
        Ok(())
    }
}

/// `GET /api/hosted/me` → `{origin, setup_open, signed_in, person}`.
async fn me(
    State(s): State<AppState>,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    let open = h.setup_secret.is_some() && s.store_call(|st| st.hosted_needs_setup()).await?;
    Ok(no_store(
        Json(json!({
            "origin": h.origin,
            "setup_open": open,
            "github": h.github.is_some(),
            "signed_in": r.session.is_some(),
            "person": r.session.as_ref().map(person_view),
        }))
        .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupBody {
    secret: String,
    name: String,
}

/// `POST /api/hosted/setup {secret, name}`: makes the first person (the
/// owner) and signs them in, once. 404 `not_found` once setup is done, and
/// for a missing, unset or wrong secret alike.
async fn setup(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    audit: crate::audit::DeferredAudit,
    req: Result<Json<SetupBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    h.attempt(Attempt::Setup, &headers, &extensions)?;
    let b = crate::routes::artifacts::body(req)?;
    if !h.setup_secret_matches(&b.secret) {
        return Err(ApiError::not_found());
    }
    let old = session_token(&headers, h.cookie_name());
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.hosted_setup(&ctx, &b.name)?;
            end_replaced(st, &ctx, old)?;
            Ok(signed)
        })
        .await?;
    Ok(signed_in_response(&h, signed, json!({}), "person"))
}

/// `GET /api/hosted/invites/<code>` → `{name, expires_at}` while the invite
/// may be redeemed in a browser; else 404 `not_found`.
async fn peek_invite(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let peek = s.store_call(move |st| st.peek_invite(&code)).await?;
    match peek {
        Some(p) => Ok(no_store(Json(json!(p)).into_response())),
        None => Err(ApiError::not_found()),
    }
}

/// The body of a redemption (an invite's, a sign-in link's): `{replace}`,
/// or none. `replace` confirms ending the browser's current session.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemBody {
    #[serde(default)]
    replace: bool,
}

fn redeem_body(bytes: &[u8]) -> Result<RedeemBody, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(RedeemBody::default());
    }
    serde_json::from_slice(bytes).map_err(|e| {
        ApiError::bad_request(
            "invalid_json",
            format!("send {{\"replace\": true}} or nothing: {e}"),
        )
    })
}

/// 409 `session_exists`: the browser is signed in as `current`, whose
/// session a redemption would end; it is redone with `{"replace": true}`
/// once the person confirmed. `current` is the person as `GET
/// /api/hosted/me` names them.
fn session_exists(current: &SessionViewer) -> ApiError {
    let name = current
        .viewer
        .display_name
        .as_deref()
        .unwrap_or("an unnamed person");
    let mut e = ApiError::new(
        StatusCode::CONFLICT,
        "session_exists",
        format!("this browser is signed in as {name}: confirm to sign out of {name} first"),
    );
    e.extra.insert("current".into(), person_view(current));
    e
}

/// `POST /api/hosted/invites/<code>/accept {replace?}`: redeems the invite,
/// making a connected person under its name, and signs the browser in as
/// them. 404 `not_found` for an invite that is not live (one answer for
/// every case); 409 [`session_exists`] while the browser holds a live
/// session and `replace` is not set, spending nothing.
async fn accept_invite(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    code: Result<Path<String>, PathRejection>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    h.attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let b = redeem_body(&body)?;
    if let Some(current) = r.session.as_ref().filter(|_| !b.replace) {
        return Err(session_exists(current));
    }
    let old = session_token(&headers, h.cookie_name());
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.redeem_invite_in_browser(&ctx, &code)?;
            end_replaced(st, &ctx, old)?;
            Ok(signed)
        })
        .await
        .map_err(|e| match e.code {
            "invalid_invite" => ApiError::not_found(),
            _ => e,
        })?;
    crate::routes::grants::announce(
        &s,
        json!({"what": "person_joined", "person": signed.who.viewer.public_id}),
    );
    Ok(signed_in_response(&h, signed, json!({}), "person"))
}

/// `POST /api/hosted/signin-links` (a member) → `{url, expires_at}`: a
/// one-use link, good for ten minutes, that signs another browser in as
/// the caller.
async fn create_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let id = member_of(&r, "make sign-in links")?.viewer.id.clone();
    let (code, expires_at) = s.store_call(move |st| st.create_signin_link(&id)).await?;
    Ok(no_store(
        Json(json!({
            "url": h.url(&format!("/signin/{}", code.expose())),
            "expires_at": expires_at,
        }))
        .into_response(),
    ))
}

/// `POST /api/hosted/owner-signin-link` (the daemon token, in
/// `Authorization: Bearer`) → `{url, expires_at}`: a one-use link, good for
/// ten minutes, that signs a browser in as the owner. It is how the owner
/// gets back in from the machine that holds the deploy's owner token, when
/// no browser of theirs is signed in.
async fn create_owner_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !crate::auth::has_token(&headers, &s.token) {
        return Err(ApiError::unauthorized());
    }
    let h = hosted_of(&s);
    let (code, expires_at) = s
        .store_call(move |st| {
            let owner = st.owner_viewer(true)?;
            st.create_signin_link(&owner.id)
        })
        .await?;
    Ok(no_store(
        Json(json!({
            "url": h.url(&format!("/signin/{}", code.expose())),
            "expires_at": expires_at,
        }))
        .into_response(),
    ))
}

/// `GET /api/hosted/signin-links/<code>` → `{public_id, name, owner,
/// expires_at}`: whose account the live link signs into, spending
/// nothing; else 404 `not_found`.
async fn peek_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    match s.store_call(move |st| st.peek_signin_link(&code)).await? {
        Some(p) => Ok(no_store(Json(json!(p)).into_response())),
        None => Err(ApiError::not_found()),
    }
}

/// `POST /api/hosted/signin-links/<code>/accept {replace?}`: signs this
/// browser in as the link's member, once. 404 `not_found` for an unknown,
/// used or expired link; 409 [`session_exists`] while the browser holds a
/// live session of someone else and `replace` is not set, spending
/// nothing (so a link someone sent cannot swap the browser's person
/// without the page saying whose account it signs into).
async fn accept_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    code: Result<Path<String>, PathRejection>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    h.attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let b = redeem_body(&body)?;
    if let Some(current) = r.session.clone().filter(|_| !b.replace) {
        let peek = code.clone();
        let target = s.store_call(move |st| st.peek_signin_link(&peek)).await?;
        if target.is_some_and(|t| t.public_id != current.viewer.public_id) {
            return Err(session_exists(&current));
        }
    }
    let old = session_token(&headers, h.cookie_name());
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.redeem_signin_link(&ctx, &code)?;
            end_replaced(st, &ctx, old)?;
            Ok(signed)
        })
        .await?;
    Ok(signed_in_response(&h, signed, json!({}), "person"))
}

/// `POST /api/hosted/signout`: ends the browser's session and clears its
/// cookie; 204 whether or not one was live.
async fn sign_out(
    State(s): State<AppState>,
    headers: HeaderMap,
    audit: crate::audit::DeferredAudit,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    if let Some(t) = session_token(&headers, h.cookie_name()) {
        s.store_call(move |st| st.end_browser_session(&audit.resolve(st)?, &t, "sign_out"))
            .await?;
    }
    let mut res = no_store(StatusCode::NO_CONTENT.into_response());
    res.headers_mut()
        .insert(header::SET_COOKIE, h.clear_cookie());
    Ok(res)
}

fn token_view(t: &clax_store::hosted::AgentToken) -> Value {
    json!(t)
}

/// `GET /api/hosted/agent-tokens` (a member) → `{tokens}`, newest first,
/// without their tokens.
async fn list_tokens(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "mint agent tokens")?.viewer.id.clone();
    let tokens = s.store_call(move |st| st.agent_tokens(&id)).await?;
    Ok(no_store(
        Json(json!({"tokens": tokens.iter().map(token_view).collect::<Vec<_>>()})).into_response(),
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TokenBody {
    #[serde(default)]
    label: String,
}

/// `POST /api/hosted/agent-tokens {label?}` (a member) → `{id, label,
/// token, mcp_url, claude_command, codex_config, created_at}`: an agent
/// token for the caller's remote agents, shown this once, with the commands
/// that add it to Claude Code and Codex. Its rights are the caller's.
/// `mcp_url` is the MCP adapter's, which serves agent tokens
/// (`crate::routes::mcp_http::PATH`: `/mcp/json` on the daemon, `/mcp` on
/// a Durable Object).
async fn mint_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    extensions: axum::http::Extensions,
    req: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let id = member_of(&r, "mint agent tokens")?.viewer.id.clone();
    let b = crate::routes::artifacts::body(req)?;
    let peer = crate::identity::peer_of(&extensions)
        .map(|p| p.ip().to_string())
        .unwrap_or_default();
    let (t, token) = s
        .store_call(move |st| st.mint_agent_token(&audit.resolve(st)?, &id, &b.label, &peer))
        .await?;
    let mcp_url = h.url(crate::routes::mcp_http::PATH);
    let secret = token.expose();
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(json!({
                "id": t.id,
                "label": t.label,
                "created_at": t.created_at,
                "token": secret,
                "mcp_url": mcp_url,
                "claude_command": format!(
                    "claude mcp add --transport http clax {mcp_url} --header \"Authorization: Bearer {secret}\""
                ),
                "codex_config": format!(
                    "[mcp_servers.clax]\nurl = \"{mcp_url}\"\nhttp_headers = {{ \"Authorization\" = \"Bearer {secret}\" }}\n"
                ),
            })),
        )
            .into_response(),
    ))
}

/// `DELETE /api/hosted/agent-tokens/<id>` (a member, their own token):
/// revokes it and ends every agent registered under it. 404 `not_found` for
/// a token that is not the caller's.
async fn revoke_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "revoke agent tokens")?.viewer.id.clone();
    let gid = crate::routes::artifacts::path(gid)?;
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    let t = s
        .store_call(move |st| {
            if !st.agent_token_of(&id, &gid)? {
                return Err(CoreError::NotFound);
            }
            let ended = st.revoke_grant(
                &audit.resolve(st)?,
                &gid,
                clax_store::people::EndedBy::Person,
            )?;
            crate::routes::grants::announce_ended(&state, &ctx, st, &[ended], "person");
            st.agent_tokens(&id)
                .map(|ts| ts.into_iter().find(|t| t.id == gid))
        })
        .await?;
    Ok(no_store(
        Json(json!({"token": t.as_ref().map(token_view)})).into_response(),
    ))
}

/// `DELETE /api/hosted/agent-token` (an agent token, of itself) → 204: the
/// token is revoked, as its person revoking it on People would. Another
/// machine's Clax ends its own sign-in with it (`clax remote remove`). 401
/// for any other caller.
async fn revoke_own_token(
    State(s): State<AppState>,
    bearer: Option<Extension<crate::agent::BearerGrant>>,
    audit: crate::audit::DeferredAudit,
) -> Result<Response, ApiError> {
    let Some(Extension(b)) = bearer else {
        return Err(ApiError::unauthorized());
    };
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    s.store_call(move |st| {
        let ended = st.revoke_grant(
            &audit.resolve(st)?,
            &b.grant_id,
            clax_store::people::EndedBy::Person,
        )?;
        crate::routes::grants::announce_ended(&state, &ctx, st, &[ended], "person");
        Ok(())
    })
    .await?;
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}

/// `GET /api/hosted/links` (a member) → `{links: [{artifact_id, title,
/// access, url}]}`: the pages shared by link that the caller shares (the
/// owner every one, a person those they made, as [`set_link`] allows).
/// A link is the capability to its page, so a member is handed none they
/// could not have made.
async fn list_links(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let who = member_of(&r, "list shared links")?.clone();
    let h = hosted_of(&s);
    let gallery = h.public_gallery;
    let links = s
        .store_call(move |st| {
            let mut out = Vec::new();
            for (aid, access) in st.shared_by_link()? {
                let Ok(id) = clax_core::ArtifactId::parse(&aid) else {
                    continue;
                };
                if !who.owner && !st.artifact_created_by(&aid, Some(&who.viewer.public_id))? {
                    continue;
                }
                if let Some(a) = st.get_artifact(&id)? {
                    let listed = gallery && st.link_listed(&aid)?;
                    out.push((aid, a.title, access, listed));
                }
            }
            Ok(out)
        })
        .await?;
    Ok(no_store(
        Json(
            json!({"links": links.iter().map(|(aid, title, access, listed)| json!({
            "artifact_id": aid,
            "title": title,
            "access": access,
            "listed": listed,
            "url": h.url(&format!("/a/{aid}")),
        })).collect::<Vec<_>>()}),
        )
        .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkBody {
    /// `viewer`, `commenter`, or `null` to stop sharing.
    access: Option<String>,
    /// Whether the public gallery lists it (public); absent keeps it.
    #[serde(default)]
    listed: Option<bool>,
}

/// `PUT /api/hosted/artifacts/<aid>/link {access, listed?}` (the owner,
/// or the person who made the artifact; [`may_share`]): shares it by link
/// as `viewer` or `commenter`, or stops (`null`, which also unlists it),
/// and with `listed` lists it in the public gallery or not (absent keeps
/// it). → `{artifact_id, access, listed, url}`. The
/// artifact's git tokens (on stopping, every held token: a fork's need
/// read up its lineage) are rechecked before it answers
/// ([`crate::git_remotes::recheck`]). 400 `invalid_args` for another
/// access, 404 for no such artifact or one the caller may not read, 403
/// `forbidden` for anyone else, 403 `public_gallery_off` for `listed:
/// true` while the public gallery is off.
async fn set_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<LinkBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let who = member_of(&r, "share pages")?.clone();
    let aid = crate::routes::artifacts::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let b = crate::routes::artifacts::body(req)?;
    let access = match b.access.as_deref() {
        None => None,
        Some(a) => Some(LinkAccess::parse(a).ok_or_else(|| {
            ApiError::bad_request("invalid_args", "access is viewer, commenter or null")
        })?),
    };
    if b.listed == Some(true) && !h.public_gallery {
        return Err(ApiError::forbidden(
            "public_gallery_off",
            "this Clax lists no pages publicly",
        ));
    }
    let id = aid.as_str().to_string();
    let listed = b.listed;
    let gallery = h.public_gallery;
    let (_, listed) = s
        .store_call(move |st| {
            sharer(st, &who, &aid)?;
            st.set_general_access(&audit.resolve(st)?, aid.as_str(), access, listed)
        })
        .await
        .map_err(forbidden_code)?;
    // A person whose rights there came from the link, and the link's own
    // read tokens, lose their git tokens before this answers; when the
    // host fails, the next settle retries. Stopping sharing rechecks every
    // token, as a fork's tokens need read at the page too (its lineage).
    if let Ok(page) = clax_core::ArtifactId::parse(&id) {
        let only = access.is_some().then_some(&page);
        if crate::git_remotes::recheck(&s, only).await.is_err() {
            s.git_remotes.recheck_soon();
        }
    }
    Ok(no_store(
        Json(json!({
            "artifact_id": id,
            "access": access,
            "listed": listed && gallery,
            "url": h.url(&format!("/a/{id}")),
        }))
        .into_response(),
    ))
}

/// Whether session `who` may share artifact `aid` and set who else may
/// open it: the owner, and the person who made it. An editor may not.
///
/// # Errors
/// The store's.
pub fn may_share(st: &clax_store::Store, who: &SessionViewer, aid: &str) -> clax_core::Result<bool> {
    Ok(who.member() && (who.owner || st.artifact_created_by(aid, Some(&who.viewer.public_id))?))
}

/// `NotFound` without live artifact `aid`, or when `who` may not read it
/// ([`session_reads`]: an artifact they may not read is not there, as at
/// its own routes); `forbidden` when `who` reads it but may not share it
/// ([`may_share`]).
fn sharer(
    st: &clax_store::Store,
    who: &SessionViewer,
    aid: &clax_core::ArtifactId,
) -> clax_core::Result<()> {
    if st.get_artifact(aid)?.is_none() || !session_reads(st, who, aid.as_str())? {
        return Err(CoreError::NotFound);
    }
    if !may_share(st, who, aid.as_str())? {
        return Err(CoreError::invalid(
            "forbidden",
            "only the owner or the page's maker shares it",
        ));
    }
    Ok(())
}

/// A store refusal coded `forbidden` as 403.
fn forbidden_code(e: ApiError) -> ApiError {
    match e.code {
        "forbidden" => ApiError::forbidden("forbidden", e.message),
        _ => e,
    }
}

/// An artifact's general access as the sharing dialog names it:
/// `restricted` (only the people added), `link` (anyone with the link) or
/// `public` (anyone with the link, and listed in the public gallery).
fn general_of(access: Option<LinkAccess>, listed: bool) -> &'static str {
    match (access, listed) {
        (None, _) => "restricted",
        (Some(_), false) => "link",
        (Some(_), true) => "public",
    }
}

fn share_view(sh: &clax_store::shares::Share) -> Value {
    json!({
        "id": sh.id,
        "access": sh.access,
        "github_login": sh.github_login,
        "person": sh.person,
        "name": sh.name,
        "pending": sh.person.is_none(),
    })
}

/// `GET /api/hosted/artifacts/<aid>/sharing` (a member's browser) → for
/// whoever may share it ([`may_share`]) `{artifact_id, can_share: true,
/// general, link_access, url, people: [{id, access, github_login, person,
/// name, pending}], suggestions}`, the suggestions (people here, with
/// their GitHub logins) for the owner alone; for another member who may
/// read it, `{artifact_id, can_share: false}`. 404 for an artifact the
/// caller may not read, 401 without a session, 403 for a guest.
async fn get_sharing(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let gallery = h.public_gallery;
    let who = member_of(&r, "share pages")?.clone();
    let member = r.member.clone();
    let aid = crate::routes::artifacts::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let url = h.url(&format!("/a/{}", aid.as_str()));
    let v = s
        .store_call(move |st| {
            if st.get_artifact(&aid)?.is_none() {
                return Err(CoreError::NotFound);
            }
            let id = aid.as_str();
            if !may_share(st, &who, id)? {
                let reads = match &member {
                    Some(m) => m.allows_at(st, (RightType::Artifacts, "read"), id)?,
                    None => who.owner,
                };
                if !reads {
                    return Err(CoreError::NotFound);
                }
                return Ok(json!({"artifact_id": id, "can_share": false}));
            }
            let access = st.link_access(id)?;
            let listed = gallery && st.link_listed(id)?;
            let people: Vec<Value> = st.artifact_shares(id)?.iter().map(share_view).collect();
            let suggestions = if who.owner {
                json!(st.share_candidates()?)
            } else {
                json!([])
            };
            Ok(json!({
                "artifact_id": id,
                "can_share": true,
                "general": general_of(access, listed),
                "public_gallery": gallery,
                "link_access": access,
                "url": url,
                "people": people,
                "suggestions": suggestions,
            }))
        })
        .await?;
    Ok(no_store(Json(v).into_response()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareBody {
    /// A GitHub login (`@` optional).
    #[serde(default)]
    github_login: Option<String>,
    /// A person's public ID.
    #[serde(default)]
    person: Option<String>,
    /// `viewer`, `commenter` or `editor`.
    access: String,
}

/// `PUT /api/hosted/artifacts/<aid>/shares {github_login | person,
/// access}` (whoever may share it, [`may_share`]): shares the artifact
/// with that person, or changes the access of their share. → 200
/// `{share}`. A login no one has signed in with yet waits for its first
/// GitHub sign-in (`pending`). 400 `invalid_args` for no subject, both, or
/// another access; 400 `invalid_login`, `unknown_person` or
/// `already_owner`; 404 for no such artifact or one the caller may not
/// read; 403 `forbidden` for anyone else who may not share it.
async fn put_share(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<ShareBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    use clax_store::shares::{ShareAccess, ShareTo};
    browser_only(&headers)?;
    let who = member_of(&r, "share pages")?.clone();
    let aid = crate::routes::artifacts::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let b = crate::routes::artifacts::body(req)?;
    let access = ShareAccess::parse(&b.access).ok_or_else(|| {
        ApiError::bad_request("invalid_args", "access is viewer, commenter or editor")
    })?;
    let to = match (b.github_login, b.person) {
        (Some(l), None) => ShareTo::GitHub(l),
        (None, Some(p)) => ShareTo::Person(p),
        _ => {
            return Err(ApiError::bad_request(
                "invalid_args",
                "name exactly one of github_login or person",
            ));
        }
    };
    let page = aid.clone();
    let share = s
        .store_call(move |st| {
            sharer(st, &who, &aid)?;
            st.share_artifact(
                &audit.resolve(st)?,
                aid.as_str(),
                &to,
                access,
                Some(&who.viewer.public_id),
            )
        })
        .await
        .map_err(forbidden_code)?;
    // A lowered share loses the git tokens it no longer allows.
    if crate::git_remotes::recheck(&s, Some(&page)).await.is_err() {
        s.git_remotes.recheck_soon();
    }
    Ok(no_store(
        Json(json!({"share": share_view(&share)})).into_response(),
    ))
}

/// `DELETE /api/hosted/artifacts/<aid>/shares/<share_id>` (whoever may
/// share it, [`may_share`]): removes that share. → 200 `{share}`. Every
/// held git token is rechecked (a fork's need read up its lineage). 404
/// for no such artifact or share, or an artifact the caller may not read;
/// 403 `forbidden` for anyone else who may not share it.
async fn delete_share(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    ids: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let who = member_of(&r, "share pages")?.clone();
    let (aid, sid) = crate::routes::artifacts::path(ids)?;
    let aid = crate::routes::artifacts::parse_id(&aid)?;
    let share = s
        .store_call(move |st| {
            sharer(st, &who, &aid)?;
            st.unshare_artifact(&audit.resolve(st)?, aid.as_str(), &sid)
        })
        .await
        .map_err(forbidden_code)?;
    if crate::git_remotes::recheck(&s, None).await.is_err() {
        s.git_remotes.recheck_soon();
    }
    Ok(no_store(
        Json(json!({"share": share_view(&share)})).into_response(),
    ))
}

/// `GET /api/hosted/public` (anyone, signed in or not) → `{artifacts:
/// [{artifact_id, title, url}]}`: the public gallery, the artifacts shared
/// by link and listed, most recently updated first; none while the public
/// gallery is off ([`Hosted::with_public_gallery`]). Each opens through its
/// link, as any page shared by link does.
async fn public_list(State(s): State<AppState>) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    if !h.public_gallery {
        return Ok(no_store(Json(json!({"artifacts": []})).into_response()));
    }
    let rows = s.store_call(|st| st.public_artifacts()).await?;
    let artifacts: Vec<Value> = rows
        .iter()
        .map(|(aid, title)| {
            json!({"artifact_id": aid, "title": title, "url": h.url(&format!("/a/{aid}"))})
        })
        .collect();
    Ok(no_store(
        Json(json!({"artifacts": artifacts})).into_response(),
    ))
}

/// A sign-in page (`/join/<code>`, `/signin[/<code>]`): the gallery's
/// entry, which shows it by its path, sending no referrer (its URL holds a
/// code).
async fn signin_page(
    d: crate::agent::DeniedToGrants,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let mut res = crate::routes::shell::gallery_page(d, req).await?;
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

/// `/setup`: the sign-in page while setup is open; 404 once it is done or
/// when the daemon has no setup secret.
async fn setup_page(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    if h.setup_secret.is_none() || !s.store_call(|st| st.hosted_needs_setup()).await? {
        return Err(ApiError::not_found());
    }
    signin_page(d, req).await
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DeviceCodeBody {
    /// What the machine calls itself: the approval page names it.
    #[serde(default)]
    client_name: String,
}

/// `POST /api/hosted/device/code {client_name}` (another machine's Clax,
/// no credential) → `{device_code, user_code, verification_uri,
/// verification_uri_complete, expires_at, expires_in, interval}`: starts a
/// device sign-in ([`clax_store::device`]). The machine shows the
/// person the user code and opens `verification_uri_complete`, where a
/// signed-in member approves it, then polls `POST
/// /api/hosted/device/token` every `interval` seconds. Rate-limited per
/// client; 400 `invalid_args` for a bad name, 400 `limit_reached` with too
/// many waiting.
async fn device_code(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    req: Result<Json<DeviceCodeBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    h.attempt(Attempt::Device, &headers, &extensions)?;
    let b = crate::routes::artifacts::body(req)?;
    let peer = h.client(&headers, &extensions).to_string();
    let start = s
        .store_call(move |st| st.start_device(&b.client_name, &peer))
        .await?;
    let user = start.user_code.expose().to_string();
    Ok(no_store(
        Json(json!({
            "device_code": start.device_code.expose(),
            "user_code": user,
            "verification_uri": h.url("/device"),
            "verification_uri_complete": h.url(&format!("/device?code={user}")),
            "expires_at": start.expires_at,
            "expires_in": clax_store::device::DEVICE_TTL_SECS,
            "interval": clax_store::device::DEVICE_INTERVAL_SECS,
        }))
        .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceTokenBody {
    device_code: String,
}

/// `POST /api/hosted/device/token {device_code}` (the machine) → once the
/// sign-in is approved, `{token, token_type, grant_id, person: {public_id,
/// display_name}, origin, mcp_url}`, this once: an agent token of the
/// person who approved it. Before that, 400 `authorization_pending`; 400
/// `slow_down` for a poll sooner than `interval` after the last; 400
/// `access_denied` once denied (or the token revoked before this poll);
/// 400 `expired_token` for an unknown, expired or spent code.
async fn device_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    audit: crate::audit::DeferredAudit,
    req: Result<Json<DeviceTokenBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    use clax_store::device::DevicePoll;
    let h = hosted_of(&s);
    h.attempt(Attempt::DevicePoll, &headers, &extensions)?;
    let b = crate::routes::artifacts::body(req)?;
    let poll = s
        .store_call(move |st| st.poll_device(&audit.resolve(st)?, &b.device_code))
        .await?;
    let refuse = |code: &'static str, message: &str| Err(ApiError::bad_request(code, message));
    match poll {
        DevicePoll::Pending => refuse(
            "authorization_pending",
            "waiting for the person to approve this sign-in",
        ),
        DevicePoll::SlowDown => refuse(
            "slow_down",
            "polled too soon: wait the interval between polls",
        ),
        DevicePoll::Denied => refuse("access_denied", "this sign-in was denied"),
        DevicePoll::Expired => refuse(
            "expired_token",
            "this sign-in expired or was used: start a new one",
        ),
        DevicePoll::Issued {
            token,
            grant_id,
            person,
            person_name,
        } => Ok(no_store(
            Json(json!({
                "token": token.expose(),
                "token_type": "Bearer",
                "grant_id": grant_id,
                "person": {"public_id": person, "display_name": person_name},
                "origin": h.origin,
                "mcp_url": h.url(crate::routes::mcp_http::PATH),
            }))
            .into_response(),
        )),
    }
}

/// `GET /api/hosted/device/<user code>` (a member's browser) →
/// `{client_name, peer_ip, created_at, expires_at, same_address}`: the
/// machine a waiting sign-in names, for its approval page, and whether it
/// asked from this browser's own address (a code someone else started
/// does not, which the page says). 404 `not_found` for a code that is
/// unknown, decided or expired. Rate-limited as sign-in codes are.
async fn peek_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    member_of(&r, "approve sign-ins")?;
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let here = hosted_of(&s).client(&headers, &extensions).to_string();
    match s.store_call(move |st| st.peek_device(&code)).await? {
        Some(d) => {
            let mut v = json!(d);
            v["same_address"] = json!(d.peer_ip == here);
            Ok(no_store(Json(v).into_response()))
        }
        None => Err(ApiError::not_found()),
    }
}

/// `POST /api/hosted/device/<user code>/approve` (a member's browser) →
/// `{token: {id, label, …}}`: approves the sign-in as the caller. The
/// machine's next poll receives an agent token of theirs, labelled with
/// the machine's name, which their People page lists and revokes. 404 as
/// [`peek_device`].
async fn approve_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    audit: crate::audit::DeferredAudit,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "approve sign-ins")?.viewer.id.clone();
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let t = s
        .store_call(move |st| st.approve_device(&audit.resolve(st)?, &id, &code))
        .await?;
    Ok(no_store(
        Json(json!({"token": token_view(&t)})).into_response(),
    ))
}

/// `POST /api/hosted/device/<user code>/deny` (a member's browser): the
/// machine's next poll is refused `access_denied`. 204; 404 as
/// [`peek_device`].
async fn deny_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "deny sign-ins")?.viewer.id.clone();
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    s.store_call(move |st| st.deny_device(&id, &code)).await?;
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}

/// The hosted routes, merged into the daemon's router in hosted mode.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/setup", get(setup_page))
        .route("/signin", get(signin_page))
        .route("/signin/{code}", get(signin_page))
        .route("/join/{code}", get(signin_page))
        .route("/api/hosted/me", get(me))
        .route("/api/hosted/setup", post(setup))
        .route("/api/hosted/invites/{code}", get(peek_invite))
        .route("/api/hosted/invites/{code}/accept", post(accept_invite))
        .route("/api/hosted/signin-links", post(create_signin_link))
        .route(
            "/api/hosted/owner-signin-link",
            post(create_owner_signin_link),
        )
        .route("/api/hosted/signin-links/{code}", get(peek_signin_link))
        .route(
            "/api/hosted/signin-links/{code}/accept",
            post(accept_signin_link),
        )
        .route("/api/hosted/signout", post(sign_out))
        .route(
            "/api/hosted/agent-tokens",
            get(list_tokens).post(mint_token),
        )
        .route(
            "/api/hosted/agent-tokens/{id}",
            axum::routing::delete(revoke_token),
        )
        .route("/api/hosted/links", get(list_links))
        .route("/api/hosted/artifacts/{aid}/link", put(set_link))
        .route("/api/hosted/artifacts/{aid}/sharing", get(get_sharing))
        .route("/api/hosted/artifacts/{aid}/shares", put(put_share))
        .route(
            "/api/hosted/artifacts/{aid}/shares/{sid}",
            delete(delete_share),
        )
        .route("/api/hosted/public", get(public_list))
        .route("/device", get(signin_page))
        .route("/auth/github", get(crate::github::start))
        .route(crate::github::CALLBACK_PATH, get(crate::github::callback))
        .route("/api/hosted/agent-token", delete(revoke_own_token))
        .route("/api/hosted/device/code", post(device_code))
        .route("/api/hosted/device/token", post(device_token))
        .route("/api/hosted/device/{code}", get(peek_device))
        .route("/api/hosted/device/{code}/approve", post(approve_device))
        .route("/api/hosted/device/{code}/deny", post(deny_device))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_is_http_only_lax_and_secure_on_https() {
        let h = Hosted::new("https://Clax.Example", Some("s")).unwrap();
        assert_eq!((h.host.as_str(), h.secure), ("clax.example", true));
        let c = h.session_cookie(&Secret::new("cxs_abc".into()));
        assert_eq!(
            c.to_str().unwrap(),
            "__Host-clax_session=cxs_abc; Path=/; Max-Age=2592000; HttpOnly; Secure; SameSite=Lax"
        );
        assert_eq!(
            h.clear_cookie().to_str().unwrap(),
            "__Host-clax_session=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
        );
        let local = Hosted::new("http://localhost:7490", None).unwrap();
        assert_eq!(
            (local.host.as_str(), local.secure),
            ("localhost:7490", false)
        );
        assert!(
            !local
                .session_cookie(&Secret::new("cxs_abc".into()))
                .to_str()
                .unwrap()
                .contains("Secure")
        );
        assert!(Hosted::new("http://clax.example", None).is_none());
    }

    #[test]
    fn the_setup_secret_must_match_and_an_empty_one_keeps_setup_closed() {
        let h = Hosted::new("https://clax.example", Some("open sesame")).unwrap();
        assert!(h.setup_secret_matches("open sesame"));
        assert!(!h.setup_secret_matches("open sesame "));
        assert!(!h.setup_secret_matches(""));
        for none in [None, Some("")] {
            let h = Hosted::new("https://clax.example", none).unwrap();
            assert!(!h.setup_secret_matches(""));
        }
    }

    #[test]
    fn hmac_is_rfc_4231s() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn content_opens_only_at_its_versions_key() {
        let aid = "7q3k9mzx2b4t";
        let k = content_key("tok", aid, 3);
        assert_eq!(k.len(), 32);
        assert_ne!(k, content_key("tok", aid, 4));
        assert_ne!(k, content_key("other", aid, 3));
        assert_ne!(k, asset_key("tok", &format!("{aid}/3")));
        assert_eq!(
            content_path("tok", &format!("/c/{aid}/v/3/k/{k}/a/b.css")),
            Content::Unlocked(format!("/c/{aid}/v/3/a/b.css"))
        );
        assert_eq!(
            content_path("tok", &format!("/c/{aid}/v/3/k/{k}/")),
            Content::Unlocked(format!("/c/{aid}/v/3/"))
        );
        assert_eq!(
            content_path("tok", &format!("/c/{aid}/v/3/k/{k}")),
            Content::Slash(format!("{k}/"))
        );
        assert_eq!(
            content_path("tok", &format!("/c/{aid}/draft/a_s1/tok/index.html")),
            Content::Draft
        );
        for bad in [
            "/c/not-an-id/draft/a_s1/tok/".to_string(),
            format!("/c/{aid}/drafts/a_s1/tok/"),
            format!("/c/{aid}/v/3/"),
            format!("/c/{aid}/v/3/k/{k}x/"),
            format!("/c/{aid}/v/03/k/{k}/"),
            format!("/c/{aid}/v/4/k/{k}/"),
            format!("/c/{aid}/x/3/k/{k}/"),
            format!("/c/not-an-id/v/3/k/{k}/"),
            "/c/".to_string(),
        ] {
            assert_eq!(content_path("tok", &bad), Content::Locked, "{bad}");
        }
        let a = asset_key("tok", "01J9Z3K4M5N6P7Q8R9S0T1V2W3");
        assert!(blob_unlocked(
            "tok",
            "/_blob/01J9Z3K4M5N6P7Q8R9S0T1V2W3",
            Some(&format!("x=1&k={a}"))
        ));
        assert!(!blob_unlocked(
            "tok",
            "/_blob/01J9Z3K4M5N6P7Q8R9S0T1V2W3",
            None
        ));
        assert!(!blob_unlocked(
            "tok",
            "/_blob/01J9Z3K4M5N6P7Q8R9S0T1V2W4",
            Some(&format!("k={a}"))
        ));
    }

    #[test]
    fn the_shell_token_is_taken_from_the_header_or_the_query_and_swapped() {
        let t = shell_token("tok", "cxs_a");
        assert!(t.starts_with(SHELL_TOKEN_PREFIX));
        assert_ne!(t, shell_token("tok", "cxs_b"));
        assert_ne!(t, shell_token("other", "cxs_a"));
        let req = |uri: &str, auth: Option<&str>| {
            let mut b = axum::http::Request::builder().uri(uri);
            if let Some(a) = auth {
                b = b.header(header::AUTHORIZATION, a);
            }
            b.body(Body::empty()).unwrap()
        };
        assert_eq!(
            shell_token_presented(&req("/api/x", Some(&format!("bearer {t}")))),
            Some(t.clone())
        );
        assert_eq!(
            shell_token_presented(&req("/api/x", Some("Bearer tok"))),
            None
        );
        let mut r = req(&format!("/api/artifacts/a/room?peer=p&token={t}"), None);
        assert_eq!(shell_token_presented(&r), Some(t.clone()));
        swap_shell_token(&mut r, "tok").unwrap();
        assert_eq!(
            r.uri().to_string(),
            "/api/artifacts/a/room?peer=p&token=tok"
        );
        assert!(r.headers().get(header::AUTHORIZATION).is_none());
        let mut r = req("/api/x", Some(&format!("Bearer {t}")));
        swap_shell_token(&mut r, "tok").unwrap();
        assert_eq!(r.headers()[header::AUTHORIZATION], "Bearer tok");
        assert_eq!(r.uri().to_string(), "/api/x");
    }

    #[test]
    fn clients_are_cloudflares_word_only_behind_cloudflare_and_ipv6_counts_by_64() {
        let peer: std::net::SocketAddr = "203.0.113.5:4000".parse().unwrap();
        let mut ext = axum::http::Extensions::new();
        ext.insert(axum::extract::ConnectInfo(crate::auth::Conn {
            peer,
            local: peer,
        }));
        let mut h = HeaderMap::new();
        h.insert("cf-connecting-ip", "2001:db8:1:2:3:4:5:6".parse().unwrap());
        let plain = Hosted::new("https://clax.example", None).unwrap();
        assert_eq!(plain.client(&h, &ext), peer.ip());
        let cf = Hosted::new("https://clax.example", None)
            .unwrap()
            .behind_cloudflare(true);
        assert_eq!(
            client_key(cf.client(&h, &ext)),
            "2001:db8:1:2::".parse::<std::net::IpAddr>().unwrap()
        );
        h.insert("cf-connecting-ip", "not an address".parse().unwrap());
        assert_eq!(cf.client(&h, &ext), peer.ip());
        assert_eq!(
            client_key("::ffff:198.51.100.1".parse().unwrap()),
            "198.51.100.1".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn attempts_come_back_at_their_pace_per_client_and_kind() {
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-12T10:00:00Z"));
        let l = Limits::new(clock.clone());
        let a: std::net::IpAddr = "198.51.100.1".parse().unwrap();
        let b: std::net::IpAddr = "198.51.100.2".parse().unwrap();
        for _ in 0..5 {
            assert_eq!(l.take(Attempt::Guest, a), Ok(()));
        }
        assert_eq!(l.take(Attempt::Guest, a), Err(120));
        assert_eq!(l.take(Attempt::Guest, b), Ok(()));
        assert_eq!(l.take(Attempt::Setup, a), Ok(()));
        clock.advance(60);
        assert_eq!(l.take(Attempt::Guest, a), Err(60));
        clock.advance(60);
        assert_eq!(l.take(Attempt::Guest, a), Ok(()));
        assert_eq!(l.take(Attempt::Guest, a), Err(120));
    }

    #[test]
    fn a_full_table_never_gives_a_client_its_spent_attempts_back() {
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-12T10:00:00Z"));
        let l = Limits::new(clock);
        let a: std::net::IpAddr = "198.51.100.1".parse().unwrap();
        for _ in 0..5 {
            assert_eq!(l.take(Attempt::Guest, a), Ok(()));
        }
        assert_eq!(l.take(Attempt::Guest, a), Err(120));
        // Enough other clients to fill the table, each once.
        for i in 0..MAX_CLIENTS as u32 {
            let other = std::net::IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            assert_eq!(l.take(Attempt::Guest, other), Ok(()));
        }
        assert_eq!(l.take(Attempt::Guest, a), Err(120));
        let n = l.buckets.lock().unwrap().len();
        assert_eq!(n, MAX_CLIENTS);
    }

    #[test]
    fn only_one_session_cookie_counts() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, "a=1; clax_session=cxs_x".parse().unwrap());
        assert_eq!(session_token(&h, SESSION_COOKIE).as_deref(), Some("cxs_x"));
        assert_eq!(session_token(&h, SECURE_SESSION_COOKIE), None);
        h.append(header::COOKIE, "clax_session=cxs_y".parse().unwrap());
        assert_eq!(session_token(&h, SESSION_COOKIE), None);
        assert_eq!(session_token(&HeaderMap::new(), SESSION_COOKIE), None);
    }

    #[test]
    fn a_guest_reaches_only_a_shared_pages_routes() {
        use GuestNeed::*;
        let aid = "7q3k9mzx2b4t";
        let at = |kind, action, viewer| At {
            aid: aid.into(),
            kind,
            action,
            viewer,
        };
        let cases: &[(Method, &str, Option<&str>, GuestNeed)] = &[
            (Method::GET, "/", None, Gallery),
            (Method::GET, "/people", None, Gallery),
            (Method::GET, "/inbox", None, Gallery),
            (
                Method::GET,
                "/a/7q3k9mzx2b4t",
                None,
                at(RightType::Artifacts, "read", false),
            ),
            (
                Method::GET,
                "/a/7q3k9mzx2b4t/v/2/app.css",
                None,
                at(RightType::Artifacts, "read", false),
            ),
            (Method::GET, "/a/not-an-id", None, Refused),
            (
                Method::GET,
                "/api/artifacts",
                Some("artifact=7q3k9mzx2b4t"),
                at(RightType::Artifacts, "read", false),
            ),
            (Method::GET, "/api/artifacts", None, Refused),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t",
                None,
                at(RightType::Artifacts, "read", false),
            ),
            (Method::PATCH, "/api/artifacts/7q3k9mzx2b4t", None, Refused),
            (Method::DELETE, "/api/artifacts/7q3k9mzx2b4t", None, Refused),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/versions",
                None,
                Refused,
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/versions/3/files/a/b.css",
                None,
                at(RightType::Artifacts, "read", false),
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/snapshot",
                Some("tab=abc"),
                at(RightType::Artifacts, "read", false),
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/threads",
                None,
                at(RightType::Comments, "read", false),
            ),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/threads",
                None,
                at(RightType::Comments, "write", true),
            ),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/threads/t1/comments",
                None,
                at(RightType::Comments, "write", true),
            ),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/threads/t1/send",
                None,
                at(RightType::Comments, "send", true),
            ),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/threads:send",
                None,
                at(RightType::Comments, "send", true),
            ),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/threads/t1/resolve",
                None,
                at(RightType::Comments, "resolve", true),
            ),
            (
                Method::DELETE,
                "/api/artifacts/7q3k9mzx2b4t/threads/t1",
                None,
                Refused,
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/docs",
                None,
                Refused,
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/room",
                None,
                Refused,
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/live",
                None,
                at(RightType::Artifacts, "read", true),
            ),
            (
                Method::GET,
                "/api/artifacts/7q3k9mzx2b4t/drafts",
                None,
                at(RightType::Artifacts, "read", true),
            ),
            (
                Method::POST,
                "/api/artifacts/7q3k9mzx2b4t/sample",
                None,
                Refused,
            ),
            (Method::GET, "/api/viewers", Some("ids=x"), Open),
            (Method::GET, "/api/viewers/me", None, ViewerMe),
            (Method::PUT, "/api/viewers/me", None, ViewerMe),
            (
                Method::GET,
                "/api/viewers/me/attention",
                Some("artifact=7q3k9mzx2b4t"),
                at(RightType::Artifacts, "read", true),
            ),
            (Method::PUT, "/api/viewers/me/seen", None, MarkBody),
            (Method::GET, "/api/stream", None, Open),
            (Method::POST, "/api/stream/abc", None, StreamBody),
            (Method::GET, "/api/events", None, Refused),
            (Method::GET, "/api/people/me", None, Refused),
            (Method::GET, "/api/inbox", None, Refused),
            (Method::GET, "/api/threads", None, Refused),
            (Method::POST, "/api/sessions", None, Refused),
            (Method::GET, "/mcp", None, Refused),
            (Method::POST, "/mcp", None, Refused),
            (Method::POST, "/mcp/json", None, Refused),
        ];
        for (m, p, q, want) in cases {
            assert_eq!(&guest_need(m, p, *q), want, "{m} {p} {q:?}");
        }
    }

    #[test]
    fn a_guests_stream_holds_only_a_pages_topics() {
        assert_eq!(
            stream_artifacts(br#"{"subscribe": ["artifact:7q3k9mzx2b4t", "presence:7q3k9mzx2b4t", "working:8q3k9mzx2b4t"]}"#),
            Some(vec!["7q3k9mzx2b4t".into(), "7q3k9mzx2b4t".into(), "8q3k9mzx2b4t".into()])
        );
        assert_eq!(
            stream_artifacts(br#"{"subscribe": [], "unsubscribe": ["gallery"]}"#),
            Some(vec![])
        );
        assert_eq!(
            stream_artifacts(br#"{"unsubscribe": ["gallery"]}"#),
            Some(vec![])
        );
        for bad in [
            &br#"{"subscribe": ["gallery"]}"#[..],
            br#"{"subscribe": ["docs:7q3k9mzx2b4t"]}"#,
            br#"{"subscribe": ["inbox"]}"#,
            br#"{"subscribe": ["people"]}"#,
            br#"not json"#,
        ] {
            assert_eq!(
                stream_artifacts(bad),
                None,
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}

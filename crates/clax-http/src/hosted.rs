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
//! - the `Host` must be the public origin's, the retired content origin's
//!   ([`Hosted::with_retired_content_origin`]), a local name (the CLI and
//!   agents on the machine), or an artifact's site host;
//! - a write (any method but `GET`, `HEAD` and `OPTIONS`) without an
//!   `Authorization` header must carry `Origin` equal to the public origin
//!   (CSRF), else 403 `forbidden_origin`; the one exception is counting an
//!   app's install, which without `Origin` is taken as from no session
//!   ([`anonymous_write`]);
//! - the `clax connect` browser link answers 404, and `GET /api/token`
//!   hands the owner's browser its session's shell token ([`shell_token`])
//!   and anyone else 403 `not_owner`: no request is from this machine, so
//!   nothing hands out the daemon token. The gate takes the shell token,
//!   beside the session's cookie, as the daemon token;
//! - an artifact's content is served on its site host alone, whose own
//!   access check decides every request ([`crate::site_access`]): on the
//!   public host the keyed addresses of before (`/c/…`, `/_clip/…`) are
//!   404, and an asset's (`/_blob/<id>`, `/_clax/blob/<id>`) only
//!   redirects to its artifact's site; the retired content origin answers
//!   that redirect and nothing else. A thread's clip is read with the
//!   reader's own credential ([`clip_link`]);
//! - the shell's static files and `/healthz` are open;
//! - a request with `Authorization` speaks for its credential (the daemon
//!   token, a grant, an agent token); any other `Authorization` answers 401
//!   `unauthorized`;
//! - every caller is scoped to their reach, the owner included: a person's
//!   session by the person's rights, the owner's session (its cookie or
//!   its shell token) and the daemon token by the owner's
//!   ([`MemberRights`]), and a grant or agent token by its rights narrowed
//!   the same way (`crate::agent::Agent`). Granted rights apply only at
//!   artifacts the owner made or the person made; an artifact another
//!   person made is reached only through its share with them, or its link.
//!   An artifact out of reach answers 404 wherever a route names it,
//!   listings leave it out, a route's action needs its right
//!   (`crate::agent::Agent`), and no one holds the gallery's stream topic
//!   or the daemon-wide event stream (`/api/events`, 403);
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
//! Worker, `/mcp/json` on the daemon, `clax_server::routes::mcp_http`).

use crate::error::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use clax_core::Secret;
use clax_core::rights::RightType;
use clax_store::hosted::{LinkAccess, SessionViewer, SignedIn};
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
    pub setup_secret: Option<[u8; 32]>,
    /// The daemon is behind Cloudflare, whose `CF-Connecting-IP` names the
    /// client ([`Hosted::client`]).
    behind_cloudflare: bool,
    /// Each client's attempts at setup, sign-in and naming a guest.
    limits: Limits,
    /// The retired content origin's authority as a `Host` header names it
    /// ([`Hosted::with_retired_content_origin`]).
    retired_content: Option<String>,
    /// GitHub sign-in ([`crate::github`], [`Hosted::with_github`]).
    pub github: Option<crate::github::GitHub>,
    /// Each GitHub member's caps.
    pub member_caps: clax_store::github::MemberCaps,
    /// Whether pages may be listed in the public gallery
    /// ([`Hosted::with_public_gallery`]); off, nothing is listed and no
    /// one may list a page.
    pub public_gallery: bool,
    /// The Terms of Service and Privacy Policy shown and agreed to
    /// ([`Hosted::with_legal`]); `None` shows none.
    pub legal: Option<clax_core::config::Legal>,
    /// The site domain ([`Hosted::with_sites`]); `None` serves sites at
    /// `<id>.localhost` alone ([`crate::host::SiteHosts::local`]).
    pub sites: Option<crate::host::SiteHosts>,
    /// A request for the public or content host came from a loopback peer
    /// with `behind_cloudflare` off, and was logged ([`Hosted::note_peer`]).
    proxy_noted: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Hosted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hosted")
            .field("origin", &self.origin)
            .field("retired_content", &self.retired_content)
            .field("setup_open", &self.setup_secret.is_some())
            .field("behind_cloudflare", &self.behind_cloudflare)
            .field("github", &self.github)
            .field("legal", &self.legal)
            .field("sites", &self.sites)
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
    /// A world visitor's write through a site's `/_clax/api/` (a document,
    /// a batch, a lease, an asset), counted per client address.
    SiteWrite,
    /// The same, counted per artifact ([`Counted::Artifact`]): what every
    /// visitor of one site may write together.
    SiteWriteArtifact,
    /// Counting an install of an app (`POST /api/artifacts/<id>/installs`):
    /// a row each.
    Install,
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
            Attempt::SiteWrite => (60.0, 1.0),
            Attempt::SiteWriteArtifact => (600.0, 0.1),
            Attempt::Install => (10.0, 60.0),
        }
    }
}

/// What [`Limits`] counts attempts for: a client address ([`client_key`]),
/// or an artifact.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Counted {
    Address(std::net::IpAddr),
    Artifact(String),
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
    buckets: std::sync::Mutex<std::collections::HashMap<(Attempt, Counted), Bucket>>,
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
        self.take_for(what, Counted::Address(client))
    }

    /// Takes one of `key`'s `what` attempts, as [`Limits::take`].
    pub fn take_for(&self, what: Attempt, key: Counted) -> Result<(), u64> {
        self.take_each(&[(what, key)]).map_err(|(_, after)| after)
    }

    /// Takes one attempt of each of `keys` when each has one left, and
    /// none otherwise: `Err` with the index of the first that has none and
    /// the seconds until its next one.
    pub fn take_each(&self, keys: &[(Attempt, Counted)]) -> Result<(), (usize, u64)> {
        let now = self.clock.now();
        let refill = |(tokens, at): (f64, chrono::DateTime<chrono::Utc>), per: f64, burst: f64| {
            let elapsed = (now - at).num_milliseconds().max(0) as f64 / 1000.0;
            (tokens + elapsed / per).min(burst)
        };
        let mut m = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        for key in keys {
            if m.len() >= MAX_CLIENTS && !m.contains_key(key) {
                m.retain(|(w, _), b| {
                    let (burst, per) = w.rule();
                    refill(*b, per, burst) < burst
                });
                if m.len() >= MAX_CLIENTS {
                    let fullest = m
                        .iter()
                        .map(|((w, c), b)| {
                            let (burst, per) = w.rule();
                            (refill(*b, per, burst) / burst, (*w, c.clone()))
                        })
                        .max_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(_, k)| k);
                    if let Some(k) = fullest {
                        m.remove(&k);
                    }
                }
            }
        }
        let left: Vec<f64> = keys
            .iter()
            .map(|key| {
                let (burst, per) = key.0.rule();
                let b = m.entry(key.clone()).or_insert((burst, now));
                refill(*b, per, burst)
            })
            .collect();
        let spent = left.iter().position(|t| *t < 1.0);
        for (key, tokens) in keys.iter().zip(&left) {
            let b = m.get_mut(key).expect("entered above");
            *b = (
                if spent.is_none() {
                    tokens - 1.0
                } else {
                    *tokens
                },
                now,
            );
        }
        match spent {
            None => Ok(()),
            Some(i) => {
                let per = keys[i].0.rule().1;
                Err((i, ((1.0 - left[i]) * per).ceil() as u64))
            }
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
            retired_content: None,
            github: None,
            member_caps: Default::default(),
            public_gallery: false,
            legal: None,
            sites: None,
            proxy_noted: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// This hosted mode answering at `origin`, the retired content origin
    /// that served every artifact's content at keyed addresses before each
    /// artifact had a site of its own: there an asset's old address
    /// (`/_blob/<id>?k=…`) redirects to its artifact's site, and nothing
    /// else is served. `None` for an origin hosted mode does not take, or
    /// the public one's host.
    pub fn with_retired_content_origin(self, origin: &str) -> Option<Hosted> {
        let origin = clax_core::config::hosted_origin(origin)?;
        let host = authority(&origin)?;
        if host.eq_ignore_ascii_case(&self.host) {
            return None;
        }
        Some(Hosted {
            retired_content: Some(host),
            ..self
        })
    }

    /// Whether `host` (a `Host` header) is the retired content origin's.
    pub fn is_retired_content_host(&self, host: &str) -> bool {
        self.retired_content
            .as_ref()
            .is_some_and(|h| h.eq_ignore_ascii_case(host))
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

    /// This hosted mode showing `legal`'s Terms of Service and Privacy
    /// Policy: linked from the sign-in pages, each browser sign-in under
    /// their notice recorded as the person agreeing to `legal`'s version,
    /// and a signed-in person who has not agreed to it asked to. With
    /// [`Legal::Bundled`](clax_core::config::Legal::Bundled), `/terms` and
    /// `/privacy` serve the bundled documents.
    pub fn with_legal(self, legal: Option<clax_core::config::Legal>) -> Hosted {
        Hosted { legal, ..self }
    }

    /// This hosted mode serving each artifact's site under `sites`'s domain
    /// (`None`: at `<id>.localhost` alone).
    pub fn with_sites(self, sites: Option<crate::host::SiteHosts>) -> Hosted {
        Hosted { sites, ..self }
    }

    /// Whether this hosted mode serves the bundled documents at `/terms`
    /// and `/privacy`.
    pub fn serves_legal(&self) -> bool {
        matches!(self.legal, Some(clax_core::config::Legal::Bundled))
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
                .with_legal(c.legal.clone())
                .with_sites(
                    c.site_domain
                        .as_deref()
                        .and_then(crate::host::SiteHosts::new),
                )
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

    /// Logs a warning, once, when a request for the public or content host
    /// (`host`, its `Host` header) comes from a loopback peer and
    /// `behind_cloudflare` is off: a proxy on this machine, for which
    /// [`Hosted::client`] names every client by the proxy's address, so they
    /// share one client's attempts ([`Hosted::attempt`]) and connections
    /// (`crate::socket_caps`). The CLI and agents on this machine, which
    /// name a local host, are not such requests. True when it warned.
    pub fn note_peer(&self, host: &str, extensions: &axum::http::Extensions) -> bool {
        use std::sync::atomic::Ordering;
        let proxied = !self.behind_cloudflare
            && (host.eq_ignore_ascii_case(&self.host) || self.is_retired_content_host(host))
            && crate::identity::peer_of(extensions).is_some_and(crate::auth::is_loopback);
        if !proxied || self.proxy_noted.swap(true, Ordering::Relaxed) {
            return false;
        }
        tracing::warn!(
            host,
            "hosted mode is reached from a loopback peer, as through a proxy on this machine: \
             every client counts as that one address for sign-in limits and connection caps"
        );
        true
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

    /// Takes a world visitor's write on artifact `aid`'s site: one of the
    /// client's [`Attempt::SiteWrite`] and one of the artifact's
    /// [`Attempt::SiteWriteArtifact`], or neither: 429 `rate_limited` when
    /// either is spent.
    ///
    /// # Errors
    /// 429 as above.
    pub fn site_write(
        &self,
        headers: &HeaderMap,
        extensions: &axum::http::Extensions,
        aid: &str,
    ) -> Result<(), ApiError> {
        let client = client_key(self.client(headers, extensions));
        self.limits
            .take_each(&[
                (Attempt::SiteWrite, Counted::Address(client)),
                (
                    Attempt::SiteWriteArtifact,
                    Counted::Artifact(aid.to_string()),
                ),
            ])
            .map_err(|(i, after)| {
                let mut e = rate_limited(after);
                if i == 1 {
                    e.message = format!("too many writes to this site: try again in {after} s");
                }
                e
            })
    }

    /// Whether the daemon has a setup secret: `/setup` is open until the
    /// first person is made.
    pub fn setup_open(&self) -> bool {
        self.setup_secret.is_some()
    }

    /// Whether `given` is the setup secret (hashes compared in constant
    /// time).
    pub fn setup_secret_matches(&self, given: &str) -> bool {
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

    pub fn url(&self, path: &str) -> String {
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
pub fn keyed_mac(token: &str, label: &str, msg: &str) -> String {
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

/// The URL asset `asset_id` is served at, relative to its artifact's site
/// host: `/_clax/blob/<asset_id>`, or `/_blob/<asset_id>` for a `legacy`
/// artifact (`Artifact::legacy_paths`), whose pages already name that.
pub fn asset_url(asset_id: &str, legacy: bool) -> String {
    if legacy {
        format!("/_blob/{asset_id}")
    } else {
        format!("/_clax/blob/{asset_id}")
    }
}

/// The address a hosted daemon names thread `tid`'s clip on artifact `aid`
/// by, for an agent of another machine: its public origin's `GET
/// /api/artifacts/<aid>/threads/<tid>/clip`, which takes the reader's own
/// credential. `None` on a local daemon, whose agents read the clip's file.
pub fn clip_link(s: &AppState, aid: &str, tid: &str) -> Option<String> {
    s.hosted.as_ref().map(|h| clip_link_at(h, aid, tid))
}

/// [`clip_link`] on `h`.
pub fn clip_link_at(h: &Hosted, aid: &str, tid: &str) -> String {
    format!("{}/api/artifacts/{aid}/threads/{tid}/clip", h.origin)
}

/// Sets `url` on every asset of `assets` (each naming its `id`), assets of
/// one artifact, `legacy` or not, to [`asset_url`].
pub fn stamp_asset_urls(assets: &mut [Value], legacy: bool) {
    for a in assets {
        if let Some(id) = a.get("id").and_then(Value::as_str).map(str::to_string) {
            a["url"] = json!(asset_url(&id, legacy));
        }
    }
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
    /// The scope of whoever the request speaks for, on every request of a
    /// signed-in person, of the owner's browser (its cookie or its shell
    /// token) and of the daemon token: their reach ([`MemberRights`]),
    /// which limits their browser as it limits their agents.
    pub member: Option<MemberRights>,
    /// The session cookie's SHA-256 (lowercase hex), with a live session:
    /// what a socket's host keeps to check the session again when it
    /// reopens the socket ([`crate::identity::SocketCredentials`]).
    pub cookie_sha256: Option<String>,
}

/// A person's scope on a hosted daemon, the owner's included (the reach
/// rule): their granted rights apply only at artifacts the owner made or
/// they made, an artifact another person made is reached only through its
/// share with them, and at an artifact shared by link its
/// [`LinkAccess::rights`] apply too. Anything else is out of reach.
#[derive(Clone, Debug)]
pub struct MemberRights {
    /// The person's public ID, what `own` locations are resolved by; `None`
    /// for the owner (`own`: what the owner made).
    pub person: Option<String>,
    /// Their granted rights: the person's multi-person rights
    /// ([`clax_store::Store::person_rights`]), every right for the owner.
    /// They apply only within the person's reach.
    pub rights: std::sync::Arc<clax_core::rights::Rights>,
    /// The rights the artifacts shared with them hold
    /// ([`clax_store::Store::share_rights_of`]): all that reaches an
    /// artifact another person made.
    pub shared: std::sync::Arc<clax_core::rights::Rights>,
}

/// The share rights of `person` (a public ID; `None` the owner, by the
/// owner's viewer row); none without such a row.
///
/// # Errors
/// The store's.
pub fn shares_of(
    st: &clax_store::Store,
    person: Option<&str>,
) -> clax_core::Result<clax_core::rights::Rights> {
    let pid = match person {
        Some(p) => Some(p.to_string()),
        None => st.owner_public_id()?,
    };
    match pid.map(|p| st.share_rights_of(&p)) {
        None | Some(Err(clax_core::CoreError::NotFound)) => Ok(Default::default()),
        Some(r) => r,
    }
}

impl MemberRights {
    /// Person `public_id`'s scope.
    ///
    /// # Errors
    /// The store's ([`clax_store::Store::person_rights`]).
    pub fn of_person(st: &clax_store::Store, public_id: &str) -> clax_core::Result<MemberRights> {
        Ok(MemberRights {
            person: Some(public_id.to_string()),
            rights: std::sync::Arc::new(st.person_rights(public_id)?),
            shared: std::sync::Arc::new(shares_of(st, Some(public_id))?),
        })
    }

    /// The owner's scope: every right at what the owner made, and the
    /// shares of the owner's viewer row elsewhere.
    ///
    /// # Errors
    /// The store's.
    pub fn owner(st: &clax_store::Store) -> clax_core::Result<MemberRights> {
        Ok(MemberRights {
            person: None,
            rights: std::sync::Arc::new(clax_core::rights::Rights::full()),
            shared: std::sync::Arc::new(shares_of(st, None)?),
        })
    }

    /// Whether this scope allows `action` of `kind` at artifact `aid` in
    /// `st`: nothing at an artifact that is gone
    /// ([`clax_store::reach::Reach::Gone`]); within the person's reach their
    /// granted rights, beyond it their share rights; then the artifact's
    /// link's.
    ///
    /// # Errors
    /// The store's.
    pub fn allows_at(
        &self,
        st: &clax_store::Store,
        (kind, action): (RightType, &str),
        aid: &str,
    ) -> clax_core::Result<bool> {
        use clax_store::reach::Reach;
        let none = |_: &str| false;
        let granted = match st.reach(aid, self.person.as_deref())? {
            Reach::Gone => return Ok(false),
            Reach::Within => {
                let own = |x: &str| {
                    st.artifact_created_by(x, self.person.as_deref())
                        .unwrap_or(false)
                };
                self.rights.allows(kind, action, Some(aid), &own)
            }
            Reach::Beyond => self.shared.allows(kind, action, Some(aid), &none),
        };
        if granted {
            return Ok(true);
        }
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
    /// The scope of session `who` (as the gate gives a request of theirs):
    /// the owner's for the owner, a person's for a person; `None` for a
    /// guest and anyone else.
    ///
    /// # Errors
    /// The store's.
    pub fn of_session(
        st: &clax_store::Store,
        who: &SessionViewer,
    ) -> clax_core::Result<Option<MemberRights>> {
        if who.owner {
            return MemberRights::owner(st).map(Some);
        }
        if !who.member() {
            return Ok(None);
        }
        MemberRights::of_person(st, &who.viewer.public_id).map(Some)
    }
}

/// Whether `member` may read the comments on artifact `aid` (`clax-comments`
/// `read`, by their scope or the page's link), as `GET
/// /api/artifacts/<aid>/threads` asks; `None` (a guest, a visitor, a local
/// daemon's caller) is not limited here: a guest's and a visitor's link
/// always reads comments ([`LinkAccess::rights`]).
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
/// a route under it: the owner and a person by their scope or the page's
/// link ([`MemberRights::allows_at`]); a guest while the page is shared by
/// link. What a host reopening a socket of `aid` checks again.
///
/// # Errors
/// The store's.
pub fn session_reads(
    st: &clax_store::Store,
    who: &SessionViewer,
    aid: &str,
) -> clax_core::Result<bool> {
    match MemberRights::of_session(st, who)? {
        Some(m) => m.allows_at(st, (RightType::Artifacts, "read"), aid),
        None => Ok(st.link_access(aid)?.is_some()),
    }
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
    /// The daemon-wide event stream (`/api/events`): no one's here.
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

/// A scoped request through the gate (a person's, the owner's, the daemon
/// token's): what [`member_need`] says, an artifact out of their scope
/// answering 404 as one that does not exist. A stream subscription's
/// artifact topics need reading their artifact (`docs:` its db), and the
/// gallery's topic, which carries every artifact's titles, is left out:
/// no one's scope reads every artifact.
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
            "the daemon's event stream is no one's here: follow topics on /api/stream",
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
            let mut kept = Vec::new();
            for t in topics {
                match clax_realtime::stream::Topic::parse(&t) {
                    Ok(clax_realtime::stream::Topic::Gallery) => continue,
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

/// The session cookie's value: [`cookie_value`] of the cookie named `name`
/// ([`Hosted::cookie_name`]).
pub fn session_token(headers: &HeaderMap, name: &str) -> Option<String> {
    cookie_value(headers, name)
}

/// The value of the one cookie named `name` in `headers`; `None` without
/// one, or with more than one (another page set a cookie of the name).
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
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
        [""]
        | ["inbox"]
        | ["people"]
        | ["tags"]
        | ["admin"]
        | ["settings"]
        | ["me"]
        | ["audit"]
            if get =>
        {
            Gallery
        }
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
        // A site session for the page's frame: a link viewer's.
        ["api", "artifacts", aid, "site-code"] if post => {
            at(aid, RightType::Artifacts, "read", false)
        }
        // An app's head and bundles, and counting an install of it: anyone
        // who may open it, as the Clax app does without signing in.
        ["api", "artifacts", aid, "head"] if get => at(aid, RightType::Artifacts, "read", false),
        ["api", "artifacts", aid, "versions", _, "bundle"] if get => {
            at(aid, RightType::Artifacts, "read", false)
        }
        ["api", "artifacts", aid, "installs"] if post => {
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

/// The prefix of hosted mode's own API (`clax_server::hosted::router`).
pub const OWN_API: &str = "/api/hosted/";

/// Where an agent token revokes itself (`DELETE`): the one route of
/// [`OWN_API`] an agent token reaches, which takes its
/// [`crate::agent::BearerGrant`] ([`crate::agent::verify_layer`]).
pub const SELF_REVOKE_PATH: &str = "/api/hosted/agent-token";

/// Hosted mode's own pages outside [`OWN_API`]
/// (`clax_server::hosted::router` mounts each): a path, or a prefix when it
/// ends in `/`. [`LEGAL_PAGES`] are its own only while it serves them.
pub const OWN_PAGES: &[&str] = &[
    "/setup",
    "/signin",
    "/signin/",
    "/join/",
    "/device",
    "/auth/github",
    crate::github::CALLBACK_PATH,
    "/site-signin",
    // The Clax app's links, which Apple and Google fetch with no session;
    // each handler answers only on the public host.
    "/.well-known/apple-app-site-association",
    "/.well-known/assetlinks.json",
];

/// The bundled legal documents' pages ([`Hosted::serves_legal`]).
pub const LEGAL_PAGES: &[&str] = &["/terms", "/privacy"];

/// Whether `path` is one of hosted mode's own routes ([`OWN_API`],
/// [`OWN_PAGES`], and [`LEGAL_PAGES`] when `serves_legal`), which [`gate`]
/// hands on past a person's rights and a guest's needs: each handler checks
/// its own caller.
pub fn own_route(path: &str, serves_legal: bool) -> bool {
    path.starts_with(OWN_API)
        || OWN_PAGES.iter().any(|p| {
            if p.ends_with('/') {
                path.starts_with(p)
            } else {
                path == *p
            }
        })
        || (serves_legal && LEGAL_PAGES.contains(&path))
}

/// Whether `method` `path` is a change the gate takes without the public
/// `Origin`, as from no session: counting an app's install
/// (`POST /api/artifacts/<id>/installs`), which the Clax app sends
/// outside any browser. Without `Origin` its cookie is not read, so a
/// page elsewhere cannot make it speak for a signed-in browser.
pub fn anonymous_write(method: &Method, path: &str) -> bool {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    method == Method::POST && matches!(segs.as_slice(), ["api", "artifacts", _, "installs"])
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
    let content_host = req.extensions().get::<crate::host::SiteRequest>().is_some();
    let on_retired = h.is_retired_content_host(&host);
    if !(host.eq_ignore_ascii_case(&h.host)
        || on_retired
        || crate::auth::is_local_host(&host)
        || content_host)
    {
        return ApiError::forbidden(
            "forbidden_host",
            format!("this Clax answers at {}", h.origin),
        )
        .into_response();
    }
    h.note_peer(&host, req.extensions());
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let origin_checked = origin == Some(h.origin.as_str());
    let write = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
    if path.starts_with("/connect/b/") {
        return ApiError::not_found().into_response();
    }
    // A site host's paths are its artifact's own (`/c/…` among them): the
    // site routes serve them. Elsewhere no content is served: a version's
    // keyed addresses (`/c/…`) and a clip's (`/_clip/…`) are gone, and an
    // asset's address only redirects to its artifact's site, whose access
    // check decides; the retired content origin serves nothing else.
    if !content_host {
        if path.starts_with("/_blob/") || path.starts_with("/_clax/blob/") {
            req.extensions_mut().insert(HostedRequest::default());
            return next.run(req).await;
        }
        if on_retired || path.starts_with("/c/") || path.starts_with("/_clip/") {
            return ApiError::not_found().into_response();
        }
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
        // The owner's scope, as on the cookie's own requests.
        let member = match s.store_call(MemberRights::owner).await {
            Ok(m) => m,
            Err(e) => return e.into_response(),
        };
        req.extensions_mut().insert(HostedRequest {
            session,
            origin_checked,
            member: Some(member.clone()),
            cookie_sha256,
        });
        if own_route(&path, h.serves_legal()) {
            return next.run(req).await;
        }
        return member_gate(&s, &member, req, next).await;
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
    // A site host's writes (its `/_clax/api/`) come from the site's own
    // origin, which its routes check (`clax_server::routes::site_api`).
    // An app counting its install sends no `Origin`: such a request
    // speaks for no session, whatever cookie it carries ([`anonymous_write`]).
    let anonymous = !authz && !origin_checked && anonymous_write(&method, &path);
    if write && !authz && !origin_checked && !machine && !content_host && !anonymous {
        return forbidden_origin(&h);
    }
    let session = if authz || anonymous {
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
    // Whoever the request speaks for is scoped to their reach
    // ([`MemberRights`]): a person's browser, the owner's, and the daemon
    // token, which is the owner's. A grant's request carries its own
    // ([`crate::agent::Agent`]).
    let token = authz && crate::auth::has_token(req.headers(), &s.token);
    let scoped = match session.as_ref() {
        _ if token => s.store_call(|st| MemberRights::owner(st).map(Some)).await,
        Some(who) => {
            let who = who.clone();
            s.store_call(move |st| MemberRights::of_session(st, &who))
                .await
        }
        None => Ok(None),
    };
    let member = match scoped {
        Ok(m) => m,
        Err(e) => return e.into_response(),
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
    let own = own_route(&path, h.serves_legal());
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
pub async fn viewer_me(
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

pub fn no_store(mut res: Response) -> Response {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// A person's view as the sign-in routes answer it.
pub fn person_view(who: &SessionViewer) -> Value {
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
pub fn signed_in_response(h: &Hosted, signed: SignedIn, mut shape: Value, key: &str) -> Response {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_host_from_loopback_is_noted_once_and_only_off_cloudflare() {
        let peer = |p: &str| {
            let mut e = axum::http::Extensions::new();
            e.insert(axum::extract::ConnectInfo(crate::auth::Conn {
                peer: p.parse().unwrap(),
                local: "127.0.0.1:7490".parse().unwrap(),
            }));
            e
        };
        let h = Hosted::new("https://clax.example", None).unwrap();
        assert!(!h.note_peer("clax.example", &peer("203.0.113.9:5000")));
        assert!(
            !h.note_peer("localhost:7490", &peer("127.0.0.1:5000")),
            "the CLI"
        );
        assert!(h.note_peer("clax.example", &peer("127.0.0.1:5000")));
        assert!(
            !h.note_peer("clax.example", &peer("127.0.0.1:5001")),
            "once"
        );
        let cf = Hosted::new("https://clax.example", None)
            .unwrap()
            .behind_cloudflare(true);
        assert!(!cf.note_peer("clax.example", &peer("127.0.0.1:5000")));
    }

    #[test]
    fn hosted_modes_own_routes() {
        for p in [
            "/api/hosted/me",
            "/setup",
            "/signin",
            "/signin/abc",
            "/join/abc",
            "/device",
            "/auth/github",
            "/auth/github/callback",
        ] {
            assert!(own_route(p, false), "{p}");
        }
        for p in [
            "/setupx",
            "/signinx",
            "/join",
            "/api/hostedx",
            "/api/artifacts",
        ] {
            assert!(!own_route(p, true), "{p}");
        }
        assert!(own_route("/terms", true) && own_route("/privacy", true));
        assert!(!own_route("/terms", false) && !own_route("/privacy", false));
    }

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
    fn a_clip_is_named_at_the_route_that_takes_the_readers_credential() {
        let (aid, tid) = ("7q3k9mzx2b4t", "01J9ZZZZZZZZZZZZZZZZZZZZZZ");
        let h = Hosted::new("https://clax.example", None).unwrap();
        assert_eq!(
            clip_link_at(&h, aid, tid),
            format!("https://clax.example/api/artifacts/{aid}/threads/{tid}/clip")
        );
        assert_eq!(
            asset_url("01J9Z3K4M5N6P7Q8R9S0T1V2W3", false),
            "/_clax/blob/01J9Z3K4M5N6P7Q8R9S0T1V2W3"
        );
        assert_eq!(
            asset_url("01J9Z3K4M5N6P7Q8R9S0T1V2W3", true),
            "/_blob/01J9Z3K4M5N6P7Q8R9S0T1V2W3"
        );
    }

    #[test]
    fn a_retired_content_origin_is_a_host_of_its_own() {
        let h = Hosted::new("https://clax.example", None).unwrap();
        assert!(
            Hosted::new("https://clax.example", None)
                .unwrap()
                .with_retired_content_origin("https://clax.example")
                .is_none()
        );
        let h = h
            .with_retired_content_origin("https://content.clax.example")
            .unwrap();
        assert!(h.is_retired_content_host("Content.Clax.Example"));
        assert!(!h.is_retired_content_host("clax.example"));
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
    fn a_site_write_spends_both_its_buckets_or_neither() {
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-12T10:00:00Z"));
        let l = Limits::new(clock);
        let site = |n: u8| Counted::Artifact(format!("site{n}"));
        let addr = |n: u8| Counted::Address(std::net::IpAddr::from([198, 51, 100, n]));
        for i in 0..600u32 {
            let a = addr((i / 60) as u8);
            assert_eq!(
                l.take_each(&[
                    (Attempt::SiteWrite, a),
                    (Attempt::SiteWriteArtifact, site(1))
                ]),
                Ok(()),
                "{i}"
            );
        }
        // The site's bucket is spent: a fresh address keeps its own.
        let fresh = addr(200);
        assert_eq!(
            l.take_each(&[
                (Attempt::SiteWrite, fresh.clone()),
                (Attempt::SiteWriteArtifact, site(1))
            ]),
            Err((1, 1))
        );
        for _ in 0..60 {
            assert_eq!(
                l.take_each(&[
                    (Attempt::SiteWrite, fresh.clone()),
                    (Attempt::SiteWriteArtifact, site(2))
                ]),
                Ok(()),
                "the refused write spent none of the address's"
            );
        }
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
            (Method::GET, "/tags", None, Gallery),
            (Method::GET, "/admin", None, Gallery),
            (Method::GET, "/settings", None, Gallery),
            (Method::GET, "/me", None, Gallery),
            (Method::GET, "/audit", None, Gallery),
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

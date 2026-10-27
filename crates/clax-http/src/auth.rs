//! Who a request speaks for ([`Caller`]), the owner-only gate
//! ([`RequireToken`]), the `Host` check on every `/api` route (DNS
//! rebinding), the loopback check for /api/token, and the credential gate
//! of a daemon that serves no one without one ([`require_credential`]).

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use clax_core::model::Viewer;
use clax_core::rights::Rights;
use clax_core::visibility::Reader;
use std::net::SocketAddr;
use std::sync::Arc;

/// Who a request speaks for (spec §6.1, M8): every handler learns it from
/// here, through [`crate::identity::Identity::caller`] or the
/// [`crate::agent::Agent`] extractor, and never reads `Authorization`
/// itself.
#[derive(Clone, Debug)]
pub enum Caller {
    /// An owner browser credential (`browser`), or the daemon token on a
    /// request that names no session and carries no agent-channel marker
    /// (the CLI used by hand, scripts).
    Owner { browser: bool },
    /// The daemon token on a request that names a session (its path or
    /// `X-Clax-Session`) or carries `X-Clax-Channel: agent`.
    OwnerAgent { session: Option<String> },
    /// A verified `Clax-Grant` request (spec §6.6). `person` is `None` for a
    /// grant of the owner's row; `session` comes from `X-Clax-Session`.
    /// Never the owner, whatever its person.
    PersonAgent {
        person: Option<String>,
        grant: String,
        session: Option<String>,
        rights: Arc<Rights>,
        /// The visibility generation read before `rights` were (M17): a
        /// handler reuses them while the generation has not moved.
        generation: u64,
    },
    /// A `clax_viewer` cookie naming a connected, unrevoked person.
    Person { viewer: Viewer },
    /// Any other `clax_viewer` cookie with a viewer row.
    Viewer { viewer: Viewer },
    /// No credential.
    Anonymous,
}

/// Whose a person-scoped route's data is (spec §6.1): the owner's, a
/// connected person's (by public ID), an agent's, or no one's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Person {
    Owner,
    Viewer(String),
    Agent,
    None,
}

impl Caller {
    /// The person whose agent reads, for the visibility filter: `None` for
    /// callers that are not agent channels.
    pub fn reader(&self) -> Option<Reader> {
        match self {
            Caller::OwnerAgent { .. } | Caller::PersonAgent { person: None, .. } => {
                Some(Reader::Owner)
            }
            Caller::PersonAgent {
                person: Some(p), ..
            } => Some(Reader::Person(p.clone())),
            _ => None,
        }
    }

    /// An owner browser credential or a connected person's browser (M20's
    /// routes also require `SameOrigin`).
    pub fn browser(&self) -> bool {
        matches!(
            self,
            Caller::Owner { browser: true } | Caller::Person { .. }
        )
    }

    /// The session an agent caller speaks for.
    pub fn session(&self) -> Option<&str> {
        match self {
            Caller::OwnerAgent { session } | Caller::PersonAgent { session, .. } => {
                session.as_deref()
            }
            _ => None,
        }
    }

    /// An agent channel: the owner's agents and a grant's.
    pub fn is_agent(&self) -> bool {
        matches!(self, Caller::OwnerAgent { .. } | Caller::PersonAgent { .. })
    }

    /// The owner: a browser of theirs or the CLI by hand. An agent of a
    /// grant of the owner's row is not.
    pub fn is_owner(&self) -> bool {
        matches!(self, Caller::Owner { .. })
    }

    /// The owner's browser: what the grant decisions require (M20).
    pub fn owner_browser(&self) -> bool {
        matches!(self, Caller::Owner { browser: true })
    }

    /// 403 `not_for_agents` for the owner's agents and 403 `forbidden` for
    /// a person's: what a route that serves people answers an agent channel
    /// (spec §7.5, the `not_for_agents` rule).
    ///
    /// # Errors
    /// As above.
    pub fn refuse_agents(&self) -> Result<(), ApiError> {
        match self {
            Caller::OwnerAgent { .. } => Err(not_for_agents()),
            Caller::PersonAgent { .. } => Err(grants_refused()),
            _ => Ok(()),
        }
    }

    /// Whose a person-scoped route's data is.
    pub fn person(&self) -> Person {
        match self {
            Caller::Owner { .. } => Person::Owner,
            Caller::Person { viewer } => Person::Viewer(viewer.public_id.clone()),
            Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => Person::Agent,
            Caller::Viewer { .. } | Caller::Anonymous => Person::None,
        }
    }
}

/// Whether any `Authorization` header of `headers` carries scheme `scheme`
/// (matched ignoring case).
pub fn has_scheme(headers: &axum::http::HeaderMap, scheme: &str) -> bool {
    headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split_once(' ')
                .map_or(v, |(s, _)| s)
                .eq_ignore_ascii_case(scheme)
        })
}

/// The scheme of an agent credential of another machine (spec §6.6).
pub const GRANT_SCHEME: &str = "Clax-Grant";

/// Whether the request carries an agent credential, valid or not: a
/// signed one (`Authorization: Clax-Grant`), or an agent token
/// ([`bearer_agent_token`]).
pub fn is_grant_request(headers: &axum::http::HeaderMap) -> bool {
    has_scheme(headers, GRANT_SCHEME) || is_bearer_grant_request(headers)
}

/// Whether any `Authorization` header of `headers` is `Bearer cxa_…`: an
/// agent token a person minted in a hosted daemon's browser, a bearer
/// grant ([`clax_store::hosted`]), valid or not.
pub fn is_bearer_grant_request(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| bearer_value(v).is_some())
}

/// The agent token in the request's one `Authorization` header, `Bearer
/// cxa_…` (the scheme matched ignoring case); `None` for none, another
/// credential, or more than one header.
pub fn bearer_agent_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let mut all = headers.get_all(axum::http::header::AUTHORIZATION).iter();
    let (Some(v), None) = (all.next(), all.next()) else {
        return None;
    };
    bearer_value(v.to_str().ok()?)
}

fn bearer_value(v: &str) -> Option<&str> {
    let (scheme, token) = v.split_once(' ')?;
    let token = token.trim_matches(' ');
    (scheme.eq_ignore_ascii_case("bearer")
        && token.starts_with(clax_store::hosted::AGENT_TOKEN_PREFIX))
    .then_some(token)
}

/// Extractor for the routes that stay the owner's own: it admits a request
/// only when its `Authorization` header is `Bearer <token>` with the daemon's
/// token (the scheme matched case-insensitively, RFC 9110 section 11.1; the
/// token compared in constant time). An agent credential of another machine
/// is 403 `forbidden`, never a fallback to anything else.
pub struct RequireToken;

impl FromRequestParts<AppState> for RequireToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if is_grant_request(&parts.headers) {
            return Err(grants_refused());
        }
        if has_token(&parts.headers, &state.token) {
            Ok(RequireToken)
        } else {
            Err(ApiError::unauthorized())
        }
    }
}

/// 403 `forbidden` for an agent of another machine on a route that is not
/// theirs.
pub fn grants_refused() -> ApiError {
    ApiError::forbidden(
        "forbidden",
        "agents of another machine may not use this route",
    )
}

/// 403 `not_for_agents`: a route that serves people (the owner's browsers,
/// the CLI used by hand) refuses an agent channel; agents reach the same
/// content through the routes the filter covers.
pub fn not_for_agents() -> ApiError {
    ApiError::forbidden(
        "not_for_agents",
        "this serves people, not agents: open it in a browser",
    )
}

/// True when `headers` carry `Authorization: Bearer <token>` (scheme matched
/// case-insensitively, token compared in constant time).
pub fn has_token(headers: &axum::http::HeaderMap, token: &str) -> bool {
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = match header.split_once(' ') {
        Some((scheme, t)) if scheme.eq_ignore_ascii_case("bearer") => t.trim_start_matches(' '),
        _ => "",
    };
    constant_time_eq(presented.as_bytes(), token.as_bytes())
}

/// True when `presented` is the daemon's token (compared in constant time).
pub fn token_matches(presented: &str, token: &str) -> bool {
    constant_time_eq(presented.as_bytes(), token.as_bytes())
}

/// The name of the cookie that stands for the token on the event streams
/// (`/api/events`, `/api/stream`):
/// `clax_events_<port>`, the port from the request's `Host` (80 when it
/// names none), so daemons on one host name but different ports keep apart.
pub fn events_cookie_name(host: &str) -> String {
    let port = match host.rsplit_once(':') {
        Some((h, p))
            if !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (!h.starts_with('[') || h.ends_with(']')) =>
        {
            p
        }
        _ => "80",
    };
    format!("clax_events_{port}")
}

/// The events cookie's value for `token`: a SHA-256 of it (lowercase hex),
/// so the cookie grants the event stream's token level and never reveals the token.
pub fn events_cookie_value(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::new()
        .chain_update(b"clax events cookie\n")
        .chain_update(token.as_bytes())
        .finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// True when `headers` carry this port's events cookie with the value for
/// `token` (compared in constant time). Only the event streams read it:
/// `GET /api/events`, `GET /api/stream` and `POST /api/stream/<id>`.
pub fn has_events_cookie(headers: &axum::http::HeaderMap, token: &str) -> bool {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let name = events_cookie_name(host);
    let want = events_cookie_value(token);
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == name)
        .any(|(_, v)| constant_time_eq(v.as_bytes(), want.as_bytes()))
}

/// The name of the key cookie: `clax_key_<port>`, the port from the
/// request's `Host` as for the events cookie. Its value is the token itself.
/// No response sets it: the app that starts a daemon with
/// `require_credential` puts it in its own WebView's cookie store, so the
/// shell there passes [`require_credential`] and fetches the token as on a
/// desktop.
pub fn key_cookie_name(host: &str) -> String {
    events_cookie_name(host).replacen("clax_events_", "clax_key_", 1)
}

/// True when `headers` carry this port's key cookie with the token
/// (compared in constant time).
pub fn has_key_cookie(headers: &axum::http::HeaderMap, token: &str) -> bool {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let name = key_cookie_name(host);
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == name)
        .any(|(_, v)| constant_time_eq(v.as_bytes(), token.as_bytes()))
}

/// Paths a daemon with `require_credential` serves without a credential on
/// its own host: `/healthz`, `/.well-known/clax` (what this Clax is), the
/// shell's static files (`/_clax/...`, code and no data), and the old
/// addresses of content and assets (`/c/...`, none of which answers, and
/// `/_blob/...`, which redirect to the asset's site). A site host's requests pass the gate whatever their path: the
/// site's access decides each (`crate::site_access`).
pub fn open_without_credential(path: &str) -> bool {
    path == "/healthz"
        || path == "/.well-known/clax"
        || path.starts_with("/_clax/")
        || path.starts_with("/c/")
        || path.starts_with("/_blob/")
}

/// Middleware: on a daemon serving with `require_credential` (the app's
/// embedded daemon, whose loopback every other app on the device shares),
/// a request answers 401 `unauthorized` unless its path is
/// [`open_without_credential`] or it carries a credential: the bearer token,
/// the key cookie ([`key_cookie_name`]), or the owner or events cookie that
/// `GET /api/token` sets. `GET /api/token` is behind it like every other
/// route, so a program that can only reach loopback never gets the token.
/// A daemon without the option passes every request on.
pub async fn require_credential(
    axum::extract::State(s): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !s.require_credential
        || open_without_credential(req.uri().path())
        || req.extensions().get::<crate::host::SiteRequest>().is_some()
        || has_token(req.headers(), &s.token)
        || has_key_cookie(req.headers(), &s.token)
        || crate::identity::has_owner_cookie(req.headers(), &s.token)
        || has_events_cookie(req.headers(), &s.token)
    {
        return next.run(req).await;
    }
    ApiError::unauthorized().into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Both ends of the TCP connection a request arrived on. The daemon serves
/// with this as its connect info; `local` is the address the client reached,
/// which is the bind address, or an interface address for an unspecified bind.
#[derive(Clone, Copy, Debug)]
pub struct Conn {
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

/// The connection `extensions`' request arrived on, as its host recorded
/// it: natively axum's `ConnectInfo<Conn>`, which the daemon serves with.
/// On wasm32 always `None`: no request a Worker receives came over a
/// connection of the owner's machine, so none gets its loopback or LAN
/// trust (the token route, the owner and events cookies, live pages, the
/// bound address as an API host), whatever the request says of its peer.
/// `None` is a request that came over no connection of this machine's.
pub fn conn_of(extensions: &axum::http::Extensions) -> Option<Conn> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        extensions
            .get::<axum::extract::ConnectInfo<Conn>>()
            .map(|c| c.0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = extensions;
        None
    }
}

/// Extracts the request's [`Conn`] ([`conn_of`]). Natively it is
/// `ConnectInfo<Conn>`'s extractor, rejecting as it does; on wasm32, where
/// no request has one, it refuses 403 `not_loopback`.
pub struct PeerConn(pub Conn);

#[cfg(not(target_arch = "wasm32"))]
impl<S: Send + Sync> FromRequestParts<S> for PeerConn {
    type Rejection = <axum::extract::ConnectInfo<Conn> as FromRequestParts<S>>::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::ConnectInfo::<Conn>::from_request_parts(parts, state)
            .await
            .map(|c| PeerConn(c.0))
    }
}

#[cfg(target_arch = "wasm32")]
impl<S: Send + Sync> FromRequestParts<S> for PeerConn {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        conn_of(&parts.extensions).map(PeerConn).ok_or_else(|| {
            ApiError::forbidden("not_loopback", "the request came over no local connection")
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl
    axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, tokio::net::TcpListener>>
    for Conn
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::TcpListener>) -> Self {
        let peer = *stream.remote_addr();
        Conn {
            peer,
            local: stream.io().local_addr().unwrap_or(peer),
        }
    }
}

/// The daemon's listener: a TCP listener that sets socket options on each
/// connection it accepts (see [`crate::conn::tune_connection`]).
#[cfg(not(target_arch = "wasm32"))]
pub struct TunedListener(pub tokio::net::TcpListener);

#[cfg(not(target_arch = "wasm32"))]
impl axum::serve::Listener for TunedListener {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (mut io, addr) = axum::serve::Listener::accept(&mut self.0).await;
        crate::conn::tune_connection(&mut io);
        (io, addr)
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TunedListener>>
    for Conn
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, TunedListener>) -> Self {
        let peer = *stream.remote_addr();
        Conn {
            peer,
            local: stream.io().local_addr().unwrap_or(peer),
        }
    }
}

/// True when `host` (a `Host` header) may address the API: it names this
/// machine literally ([`is_local_host`]), or it is exactly the address and
/// port the connection arrived on (`local`; an IP literal, IPv6 bracketed,
/// the port optional only for port 80). DNS names other than `localhost` are
/// refused, so a page whose name was rebound to this machine cannot use the
/// API.
pub fn api_host_allowed(host: &str, local: Option<SocketAddr>) -> bool {
    if is_local_host(host) {
        return true;
    }
    let Some(local) = local else {
        return false;
    };
    let named = host.parse::<SocketAddr>().ok().or_else(|| {
        let bare = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        bare.parse::<std::net::IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, 80))
    });
    named.is_some_and(|n| {
        n.ip().to_canonical() == local.ip().to_canonical() && n.port() == local.port()
    })
}

/// True when `host` (a `Host` header) names one of `trusted`, the home's
/// `[serve] trusted_hosts` (lowercased): its name, with any numeric port
/// removed, equals an entry ignoring case. Never a suffix or a pattern:
/// `evil-phobos.example` and `phobos.example.evil.com` are not
/// `phobos.example`. Such a host is a reverse proxy's in front of the daemon
/// (`tailscale serve`), whose peer is this machine but whose browser is not.
pub fn trusted_host(host: &str, trusted: &[String]) -> bool {
    let name = match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return false,
        None => host,
    };
    !name.is_empty() && trusted.iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// Marker request extension: the request's `Host` is one of the home's
/// `[serve] trusted_hosts` ([`trusted_host`]), set by [`require_api_host`]
/// on every path. Its connection is a proxy's on this machine, so nothing
/// a loopback peer is trusted with alone ([`loopback_peer`]: live pages,
/// the extension's gateway) is granted to it.
#[derive(Clone, Copy, Debug)]
pub struct ViaTrustedHost;

/// Request extension: the tailnet user `tailscale serve` says sent the
/// request (its `Tailscale-User-Login` and `Tailscale-User-Name` headers),
/// set by [`require_api_host`] only when the home's `[serve]
/// tailscale_identity` is on and [`TailscaleUser::of_request`] trusts the
/// request. [`crate::identity::Identity`] makes it the request's viewer; it
/// grants nothing a viewer cookie does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailscaleUser {
    /// `Tailscale-User-Login`, such as `alex@github`.
    pub login: String,
    /// `Tailscale-User-Name`, when it is given once and decodes.
    pub name: Option<String>,
}

/// Login header `tailscale serve` sets for a tailnet user.
pub const TAILSCALE_LOGIN_HEADER: &str = "tailscale-user-login";
/// Display-name header `tailscale serve` sets for a tailnet user.
pub const TAILSCALE_NAME_HEADER: &str = "tailscale-user-name";

impl TailscaleUser {
    /// The tailnet user of a request, when the headers can be trusted:
    /// `enabled` (`[serve] tailscale_identity`), its `Host` is one of
    /// `trusted` ([`trusted_host`]: it came through the proxy, which removes
    /// any copy of these headers a client sends), and its peer is a loopback
    /// address (the proxy on this machine). Anything else, a LAN client or a
    /// local process naming no trusted host, could forge them, so they are
    /// ignored. `None` too without exactly one login header that decodes to
    /// a login ([`clax_store::viewers::is_tailscale_login`]).
    pub fn of_request(
        enabled: bool,
        trusted: &[String],
        headers: &axum::http::HeaderMap,
        uri: &axum::http::Uri,
        extensions: &axum::http::Extensions,
    ) -> Option<TailscaleUser> {
        if !enabled
            || !trusted_host(request_host(headers, uri), trusted)
            || !conn_of(extensions).is_some_and(|c| is_loopback(c.peer))
        {
            return None;
        }
        let one = |name: &str| {
            let mut all = headers.get_all(name).iter();
            let first = all.next()?;
            all.next().is_none().then_some(first)?.to_str().ok()
        };
        let login = one(TAILSCALE_LOGIN_HEADER).and_then(header_text)?;
        let login = login.trim();
        if !clax_store::viewers::is_tailscale_login(login) {
            return None;
        }
        Some(TailscaleUser {
            login: login.to_string(),
            name: one(TAILSCALE_NAME_HEADER)
                .and_then(header_text)
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
        })
    }
}

/// A Tailscale identity header's text: as it is, or, when it is RFC 2047
/// encoded words (`=?utf-8?q?...?=`, which `tailscale serve` sends for a
/// value that is not ASCII), their decoded UTF-8; `None` for an encoded
/// value that does not decode.
fn header_text(v: &str) -> Option<String> {
    if !v.starts_with("=?") {
        return Some(v.to_string());
    }
    let mut out = Vec::new();
    for word in v.split_ascii_whitespace() {
        let inner = word.strip_prefix("=?")?.strip_suffix("?=")?;
        let mut parts = inner.splitn(3, '?');
        let (charset, enc, text) = (parts.next()?, parts.next()?, parts.next()?);
        if !charset.eq_ignore_ascii_case("utf-8") || !enc.eq_ignore_ascii_case("q") {
            return None;
        }
        let mut bytes = text.bytes();
        while let Some(b) = bytes.next() {
            match b {
                b'_' => out.push(b' '),
                b'=' => {
                    let hex = [bytes.next()?, bytes.next()?];
                    out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
                }
                b => out.push(b),
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The request's `Host`, else the URI's authority; empty for neither.
fn request_host<'a>(headers: &'a axum::http::HeaderMap, uri: &'a axum::http::Uri) -> &'a str {
    headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| uri.authority().map(|a| a.as_str()))
        .unwrap_or("")
}

/// Whether the request (its `Host`, else the URI's authority, and the
/// connection it arrived on) may read what the API serves: the
/// [`api_host_allowed`] rule, or a name of `trusted` ([`trusted_host`]),
/// shared by [`require_api_host`] and by the shell page that embeds API data
/// (`clax_server::shell::boot`).
pub fn request_host_allowed(
    headers: &axum::http::HeaderMap,
    uri: &axum::http::Uri,
    extensions: &axum::http::Extensions,
    trusted: &[String],
) -> bool {
    let host = request_host(headers, uri);
    let local = conn_of(extensions).map(|c| c.local);
    api_host_allowed(host, local) || trusted_host(host, trusted)
}

/// Middleware: every `/api` path answers 403 `forbidden_host` unless the
/// request passes [`request_host_allowed`] with the home's
/// `[serve] trusted_hosts`. A site host's request
/// ([`crate::host::SiteRequest`]) is not checked: its paths, `/api/…` among
/// them, are its artifact's, which the site routes serve; nor are non-API
/// paths (the shell, `/healthz`). A request on a trusted host, on any path,
/// is marked [`ViaTrustedHost`], and with the home's `[serve]
/// tailscale_identity` on, it carries its [`TailscaleUser`] when it has one.
pub async fn require_api_host(
    axum::extract::State(s): axum::extract::State<AppState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if trusted_host(request_host(req.headers(), req.uri()), &s.trusted_hosts) {
        req.extensions_mut().insert(ViaTrustedHost);
    }
    if let Some(u) = TailscaleUser::of_request(
        s.tailscale_identity,
        &s.trusted_hosts,
        req.headers(),
        req.uri(),
        req.extensions(),
    ) {
        req.extensions_mut().insert(u);
    }
    let path = req.uri().path();
    if (path == "/api" || path.starts_with("/api/"))
        && req.extensions().get::<crate::host::SiteRequest>().is_none()
        && !request_host_allowed(req.headers(), req.uri(), req.extensions(), &s.trusted_hosts)
    {
        return ApiError::forbidden(
            "forbidden_host",
            "the API answers only to localhost, 127.0.0.1, [::1], the address the daemon is bound to, or a name in [serve] trusted_hosts",
        )
        .into_response();
    }
    next.run(req).await
}

pub fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().to_canonical().is_loopback()
}

/// Whether a request's connection is trusted as this machine's: its peer
/// is loopback ([`conn_of`]), and it is not a hosted daemon's request
/// ([`crate::hosted::HostedRequest`]), whose clients of the internet reach it
/// through a proxy that may itself connect from loopback; nor one on a
/// `[serve] trusted_hosts` name ([`ViaTrustedHost`]), whose loopback peer is
/// the proxy on this machine (`tailscale serve`), not its browser.
pub fn loopback_peer(extensions: &axum::http::Extensions) -> bool {
    extensions.get::<crate::hosted::HostedRequest>().is_none()
        && extensions.get::<ViaTrustedHost>().is_none()
        && conn_of(extensions).is_some_and(|c| is_loopback(c.peer))
}

/// True when a `Host` header names this machine literally: `localhost`,
/// `127.0.0.1` or `[::1]`, with an optional `:port`.
pub fn is_local_host(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, tail)) if tail.is_empty() || tail.starts_with(':') => {
                return inner == "::1";
            }
            _ => return false,
        }
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "localhost" | "127.0.0.1")
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_events_cookie_is_named_for_the_port_and_never_holds_the_token() {
        use super::{events_cookie_name, events_cookie_value, has_events_cookie};
        assert_eq!(events_cookie_name("localhost:7480"), "clax_events_7480");
        assert_eq!(events_cookie_name("127.0.0.1:61386"), "clax_events_61386");
        assert_eq!(events_cookie_name("[::1]:9000"), "clax_events_9000");
        assert_eq!(events_cookie_name("localhost"), "clax_events_80");
        assert_eq!(events_cookie_name("[::1]"), "clax_events_80");
        let v = events_cookie_value("tok");
        assert_eq!(v.len(), 64);
        assert!(!v.contains("tok"));
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "localhost:7480".parse().unwrap());
        h.insert(
            "cookie",
            format!("clax_viewer=x; clax_events_7480={v}")
                .parse()
                .unwrap(),
        );
        assert!(has_events_cookie(&h, "tok"));
        assert!(!has_events_cookie(&h, "other"));
        // Another port's cookie, or the token itself, is no token.
        h.insert("host", "localhost:7481".parse().unwrap());
        assert!(!has_events_cookie(&h, "tok"));
        h.insert("cookie", "clax_events_7481=tok".parse().unwrap());
        assert!(!has_events_cookie(&h, "tok"));
    }
    use super::*;

    #[test]
    fn local_hosts_are_accepted() {
        for h in [
            "localhost",
            "localhost:7480",
            "127.0.0.1:1",
            "[::1]:7480",
            "[::1]",
        ] {
            assert!(is_local_host(h), "{h}");
        }
    }

    #[test]
    fn other_hosts_are_rejected() {
        for h in [
            "evil.com",
            "localhost.evil.com",
            "7q3k9mzx2b4t.localhost",
            "",
            "[::1",
            "[::2]:1",
        ] {
            assert!(!is_local_host(h), "{h}");
        }
    }

    #[test]
    fn api_hosts_are_local_names_or_the_connections_own_address() {
        let lan: SocketAddr = "192.168.1.20:7480".parse().unwrap();
        let v6: SocketAddr = "[fe80::1]:7480".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:192.168.1.20]:7480".parse().unwrap();
        for (h, local) in [
            ("localhost:7480", None),
            ("[::1]", None),
            ("192.168.1.20:7480", Some(lan)),
            ("192.168.1.20:7480", Some(mapped)),
            ("[fe80::1]:7480", Some(v6)),
        ] {
            assert!(api_host_allowed(h, local), "{h} {local:?}");
        }
        for (h, local) in [
            ("192.168.1.20:7480", None),
            ("192.168.1.20:7481", Some(lan)),
            ("192.168.1.20", Some(lan)),
            ("192.168.1.21:7480", Some(lan)),
            ("rebind.example:7480", Some(lan)),
            ("mymac.local:7480", Some(lan)),
            ("", Some(lan)),
        ] {
            assert!(!api_host_allowed(h, local), "{h} {local:?}");
        }
    }

    #[test]
    fn the_key_cookie_holds_the_token_under_its_ports_name() {
        assert_eq!(key_cookie_name("127.0.0.1:41000"), "clax_key_41000");
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "127.0.0.1:41000".parse().unwrap());
        h.insert("cookie", "a=b; clax_key_41000=tok".parse().unwrap());
        assert!(has_key_cookie(&h, "tok"));
        assert!(!has_key_cookie(&h, "other"));
        h.insert("cookie", "clax_key_41001=tok".parse().unwrap());
        assert!(!has_key_cookie(&h, "tok"));
    }

    #[test]
    fn only_health_static_files_and_content_are_open_without_a_credential() {
        for p in ["/healthz", "/_clax/app.js", "/c/abc/v/1/", "/_blob/x"] {
            assert!(open_without_credential(p), "{p}");
        }
        for p in [
            "/api/token",
            "/api/artifacts",
            "/a/abc",
            "/",
            "/mcp",
            "/healthz/x",
        ] {
            assert!(!open_without_credential(p), "{p}");
        }
    }

    #[test]
    fn loopback_peers() {
        for a in ["127.0.0.1:1", "[::1]:1", "[::ffff:127.0.0.1]:1"] {
            assert!(is_loopback(a.parse().unwrap()), "{a}");
        }
        assert!(!is_loopback("10.0.0.1:1".parse().unwrap()));
    }

    #[test]
    fn a_hosted_request_has_no_loopback_peer() {
        let mut ext = axum::http::Extensions::new();
        ext.insert(axum::extract::ConnectInfo(Conn {
            peer: "127.0.0.1:5000".parse().unwrap(),
            local: "127.0.0.1:7490".parse().unwrap(),
        }));
        assert!(loopback_peer(&ext));
        ext.insert(crate::hosted::HostedRequest::default());
        assert!(!loopback_peer(&ext), "a same-machine proxy's connection");
    }

    #[test]
    fn a_request_on_a_trusted_host_has_no_loopback_peer() {
        let mut ext = axum::http::Extensions::new();
        ext.insert(axum::extract::ConnectInfo(Conn {
            peer: "127.0.0.1:5000".parse().unwrap(),
            local: "127.0.0.1:7480".parse().unwrap(),
        }));
        assert!(loopback_peer(&ext));
        ext.insert(ViaTrustedHost);
        assert!(!loopback_peer(&ext), "tailscale serve's connection");
    }

    #[test]
    fn tailscale_headers_count_only_through_the_trusted_proxy() {
        use axum::http::{HeaderMap, Uri};
        let trusted = ["phobos.tail556f5b.ts.net".to_string()];
        let headers = |host: &str, extra: &[(&str, &str)]| {
            let mut h = HeaderMap::new();
            h.insert("host", host.parse().unwrap());
            for (k, v) in extra {
                let name = axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
                h.append(name, v.parse().unwrap());
            }
            h
        };
        let conn = |peer: &str| {
            let mut e = axum::http::Extensions::new();
            e.insert(axum::extract::ConnectInfo(Conn {
                peer: peer.parse().unwrap(),
                local: "127.0.0.1:7480".parse().unwrap(),
            }));
            e
        };
        let uri = Uri::from_static("/api/viewers/me");
        let ids = [
            ("tailscale-user-login", "alex@github"),
            ("tailscale-user-name", "Alex Kesling"),
        ];
        let proxied = headers("phobos.tail556f5b.ts.net", &ids);
        let loopback = conn("127.0.0.1:50000");
        assert_eq!(
            TailscaleUser::of_request(true, &trusted, &proxied, &uri, &loopback),
            Some(TailscaleUser {
                login: "alex@github".into(),
                name: Some("Alex Kesling".into())
            })
        );
        // Off, another host, a LAN peer, or no connection: ignored.
        assert_eq!(
            TailscaleUser::of_request(false, &trusted, &proxied, &uri, &loopback),
            None
        );
        assert_eq!(
            TailscaleUser::of_request(true, &[], &proxied, &uri, &loopback),
            None
        );
        for host in ["localhost:7480", "127.0.0.1:7480", "evil.example"] {
            let h = headers(host, &ids);
            assert_eq!(
                TailscaleUser::of_request(true, &trusted, &h, &uri, &loopback),
                None,
                "{host}"
            );
        }
        for peer in ["192.168.1.9:5000", "100.64.0.7:5000"] {
            assert_eq!(
                TailscaleUser::of_request(true, &trusted, &proxied, &uri, &conn(peer)),
                None,
                "{peer}"
            );
        }
        assert_eq!(
            TailscaleUser::of_request(
                true,
                &trusted,
                &proxied,
                &uri,
                &axum::http::Extensions::new()
            ),
            None
        );
        // No login, two logins, or one that is not a login: no user. A
        // name given twice is dropped; an encoded one is decoded.
        for extra in [
            vec![("tailscale-user-name", "Alex")],
            vec![
                ("tailscale-user-login", "alex@github"),
                ("tailscale-user-login", "eve@github"),
            ],
            vec![("tailscale-user-login", "")],
            vec![("tailscale-user-login", "=?utf-8?q?a_b?=")],
        ] {
            let h = headers("phobos.tail556f5b.ts.net", &extra);
            assert_eq!(
                TailscaleUser::of_request(true, &trusted, &h, &uri, &loopback),
                None,
                "{extra:?}"
            );
        }
        let h = headers(
            "phobos.tail556f5b.ts.net:443",
            &[
                ("tailscale-user-login", "mia@example.com"),
                ("tailscale-user-name", "=?utf-8?q?Mia_Kova=C4=8D?="),
            ],
        );
        assert_eq!(
            TailscaleUser::of_request(true, &trusted, &h, &uri, &loopback).and_then(|u| u.name),
            Some("Mia Kovač".into())
        );
        let h = headers(
            "phobos.tail556f5b.ts.net",
            &[
                ("tailscale-user-login", "mia@example.com"),
                ("tailscale-user-name", "Mia"),
                ("tailscale-user-name", "Eve"),
            ],
        );
        assert_eq!(
            TailscaleUser::of_request(true, &trusted, &h, &uri, &loopback),
            Some(TailscaleUser {
                login: "mia@example.com".into(),
                name: None
            })
        );
    }

    #[test]
    fn trusted_hosts_match_exactly_ignoring_case_and_port() {
        let trusted = ["phobos.tail556f5b.ts.net".to_string()];
        for h in [
            "phobos.tail556f5b.ts.net",
            "PHOBOS.tail556f5b.ts.net",
            "phobos.tail556f5b.ts.net:443",
            "phobos.tail556f5b.ts.net:7480",
        ] {
            assert!(trusted_host(h, &trusted), "{h}");
        }
        for h in [
            "evil-phobos.tail556f5b.ts.net",
            "phobos.tail556f5b.ts.net.evil.com",
            "x.phobos.tail556f5b.ts.net",
            "tail556f5b.ts.net",
            "phobos.tail556f5b.ts.net.",
            "phobos.tail556f5b.ts.net:",
            "phobos.tail556f5b.ts.net:x",
            "phobos",
            "",
            ":443",
        ] {
            assert!(!trusted_host(h, &trusted), "{h}");
        }
        assert!(!trusted_host("phobos.tail556f5b.ts.net", &[]));
    }
}

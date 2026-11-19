//! The daemon's browser source (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §4.3,
//! §10.3): [`DaemonBrowser`], which opens a Chrome DevTools Protocol
//! session on Cloudflare Browser Run over one WebSocket,
//!
//! ```text
//! wss://api.cloudflare.com/client/v4/accounts/{account}/browser-run/devtools/browser?keep_alive=10000
//! Authorization: Bearer <token>
//! ```
//!
//! with the instance's Cloudflare credentials ([`crate::cloudflare`]),
//! read at each open so an admin's change takes effect at once. Every
//! policy of a test (the CDP sequence, the request policy, caps, metering)
//! is `clax_bot::browser`'s; this module provides only the socket and what
//! the daemon's own site serves a page (the bridge's files, the site's
//! response headers, its API as the run's person), through the site's own
//! code ([`DaemonBrowser::serving`]).
//!
//! # Browsing
//!
//! A browse's page asks two things only the daemon knows
//! (`clax_bot::browser::browse`): the instance's own hosts, a hosted
//! daemon's public origin and site domain ([`DaemonBrowser::of`]), and
//! whether a name resolves to public addresses alone, by the system's
//! resolver, the one list of local ranges the web tools use
//! ([`crate::web_source::is_public`]).
//!
//! # Concurrency
//!
//! At most `browser.max_concurrent` browsers are open at once on the
//! instance, read at each open: a session holds its slot until its
//! transport is dropped, and an open past the limit is refused
//! [`CODE_BROWSER_BUSY`] before anything is sent. Lowering the setting
//! closes nothing: it refuses opens until fewer are open.
//!
//! # Refusals
//!
//! An upgrade Browser Run refuses is [`CODE_BROWSER_UNAVAILABLE`]: 401 and
//! 403 are not retryable (the token lacks Browser Rendering – Edit); 429
//! is, with its `Retry-After` in the message; a 5xx, a socket error and an
//! upgrade that does not finish within [`OPEN_TIMEOUT`] are; any other
//! status is not.

use crate::cloudflare::{Credentials, credentials, has_credentials, usable};
use crate::web_source::{Lookup, SystemLookup};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use clax_bot::browser::{
    BrowserError, BrowserSource, CODE_BROWSER_BUSY, CODE_BROWSER_UNAVAILABLE, CdpTransport,
    Deadline, Served,
};
use clax_bot::web::OwnHosts;
use clax_bot_sandbox::{CODE_CANCELLED, CancelToken};
use clax_core::ArtifactId;
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::bot_settings::Setting;
use clax_store::roles::OWNER;
use clax_store::secrets::{CLOUDFLARE_ORIGIN, Kek};
use clax_store::site_sessions::Holder;
use futures::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use url::Url;

/// The path segment of Browser Run's API. Cloudflare's documentation also
/// spells it `browser-rendering`; only the opt-in live test can tell
/// which one Browser Run answers.
pub const BROWSER_RUN_PATH: &str = "browser-run";

/// How long Browser Run keeps a browser after its socket drops, in
/// milliseconds: its minimum. A session ends its browser itself
/// (`Browser.close`), so a longer value only bills an orphan longer.
pub const KEEP_ALIVE_MS: u32 = 10_000;

/// `filter` with tungstenite's client handshake log turned off, whatever
/// `filter` asked for: at trace it prints the whole upgrade request, the
/// Cloudflare token's `Authorization` header included. Every process that
/// runs a [`DaemonBrowser`] and installs a log subscriber builds its filter
/// through this; a WebSocket client added later with a credential in its
/// request needs the same.
#[must_use]
pub fn hiding_credentials(filter: tracing_subscriber::EnvFilter) -> tracing_subscriber::EnvFilter {
    filter.add_directive(
        "tungstenite::handshake::client=off"
            .parse()
            .expect("a constant directive"),
    )
}

/// How long the WebSocket's upgrade may take, from the connection's start.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// The message of an open without credentials.
const NOT_CONFIGURED: &str = "browser use is not configured on this instance: an admin sets the Cloudflare account ID and API token";

/// The daemon's [`BrowserSource`]: a WebSocket to Browser Run with the
/// instance's Cloudflare credentials (see the module docs).
pub struct DaemonBrowser {
    store: Arc<Store>,
    kek: Option<Kek>,
    /// Debug builds only, for tests: where sockets go instead of
    /// Cloudflare's origin, the token still opened for Cloudflare's origin.
    #[cfg(debug_assertions)]
    send_to: Option<Url>,
    /// The TLS configuration of every socket (the web source's); the
    /// reason it could not be built otherwise.
    tls: Result<Arc<rustls::ClientConfig>, String>,
    /// How many sessions are open now.
    in_use: Arc<Mutex<u32>>,
    /// The daemon whose site a test's page is served as: its bridge
    /// files, its site headers, its site API. `None`: none of them.
    site: Option<AppState>,
    /// The instance's own hosts, or why they cannot be named.
    own: Result<OwnHosts, String>,
    /// The resolver a browse's names are checked with.
    lookup: Arc<dyn Lookup>,
}

impl DaemonBrowser {
    /// The source of the instance on `store`, opening the token with `kek`.
    #[must_use]
    pub fn new(store: Arc<Store>, kek: Option<Kek>) -> DaemonBrowser {
        DaemonBrowser {
            store,
            kek,
            #[cfg(debug_assertions)]
            send_to: None,
            tls: crate::web_source::fetch_tls().map(Arc::new),
            in_use: Arc::new(Mutex::new(0)),
            site: None,
            own: Ok(OwnHosts::default()),
            lookup: Arc::new(SystemLookup),
        }
    }

    /// The source of the daemon on `state`: its store and key-encryption
    /// key, and its site's files, headers and API.
    #[must_use]
    pub fn of(state: &AppState) -> DaemonBrowser {
        let hosted = state.hosted.as_ref();
        let own = match hosted {
            Some(h) => OwnHosts::new(
                Some(h.origin.as_str()),
                h.sites.as_ref().map(|s| s.domain()),
            )
            .map_err(|e| e.to_string()),
            None => Ok(OwnHosts::default()),
        };
        DaemonBrowser::new(state.store.clone(), state.bot.kek.clone())
            .serving(state.clone())
            .with_own_hosts(own)
    }

    /// This source, answering `own` as the instance's own hosts, which a
    /// browse never reaches (`Err`: why they cannot be named, and every
    /// browse is refused).
    #[must_use]
    pub fn with_own_hosts(mut self, own: Result<OwnHosts, String>) -> DaemonBrowser {
        self.own = own;
        self
    }

    /// This source, checking a browse's names with `lookup`.
    #[cfg(test)]
    #[warn(clippy::disallowed_methods)]
    fn with_lookup(mut self, lookup: Arc<dyn Lookup>) -> DaemonBrowser {
        self.lookup = lookup;
        self
    }

    /// This source, serving a test's page as `state`'s site serves its
    /// pages: the bridge's files ([`crate::shell::bridge_file`]), the
    /// site's response headers ([`clax_http::site_access::site_policy`]) and
    /// its API ([`crate::routes::site_api::get_as`]).
    #[must_use]
    pub fn serving(mut self, state: AppState) -> DaemonBrowser {
        self.site = Some(state);
        self
    }

    /// Debug builds only, for tests: this source opening its sockets on
    /// `base`, a local fake of Browser Run (`http://127.0.0.1:<port>`),
    /// over plain WebSocket, the token still opened for Cloudflare's
    /// origin alone.
    ///
    /// # Panics
    /// When `base` is not `http` on a loopback address.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn sending_to(mut self, base: Url) -> DaemonBrowser {
        let loopback = match base.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(d)) => d == "localhost",
            None => false,
        };
        assert!(
            base.scheme() == "http" && loopback,
            "a browser source sends only to a local fake: {base}"
        );
        self.send_to = Some(base);
        self
    }

    /// The origin sockets go to: Cloudflare's (in tests, `send_to`).
    fn base(&self) -> Url {
        #[cfg(debug_assertions)]
        if let Some(t) = &self.send_to {
            return t.clone();
        }
        Url::parse(CLOUDFLARE_ORIGIN).expect("Cloudflare's origin is a URL")
    }

    /// The credentials held now and the instance's concurrent browsers,
    /// read on a blocking thread.
    async fn settings(&self) -> Result<(Option<Credentials>, u32), BrowserError> {
        let (store, kek) = (self.store.clone(), self.kek.clone());
        let read = move || -> clax_core::Result<(Option<Credentials>, u32)> {
            let creds = usable(credentials(&store, kek.as_ref(), CLOUDFLARE_ORIGIN))?;
            let max = store
                .bot_setting(Setting::BrowserMaxConcurrent)?
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(1);
            Ok((creds, max))
        };
        let read_error = |e: String| {
            BrowserError::new(
                CODE_BROWSER_UNAVAILABLE,
                format!("the browser settings could not be read ({e})"),
                true,
            )
        };
        clax_rt::spawn_blocking(read)
            .await
            .map_err(|e| read_error(e.to_string()))?
            .map_err(|e| read_error(e.to_string()))
    }

    /// A slot of the instance's `max` concurrent browsers, or `None` when
    /// they are all in use.
    fn take_slot(&self, max: u32) -> Option<Slot> {
        let mut open = self.in_use.lock().unwrap_or_else(|e| e.into_inner());
        if *open >= max {
            return None;
        }
        *open += 1;
        Some(Slot(self.in_use.clone()))
    }

    /// The URL of a new session on `account`.
    fn session_url(&self, account: &str) -> Result<Url, BrowserError> {
        let mut url = self.base();
        let no_url = |url: &Url| unavailable(format!("no WebSocket URL for {url}"), false);
        let scheme = if url.scheme() == "http" { "ws" } else { "wss" };
        if url.set_scheme(scheme).is_err() {
            return Err(no_url(&url));
        }
        let shown = url.clone();
        url.path_segments_mut()
            .map_err(|()| no_url(&shown))?
            .pop_if_empty()
            .extend([
                "client",
                "v4",
                "accounts",
                account,
                BROWSER_RUN_PATH,
                "devtools",
                "browser",
            ]);
        url.set_query(Some(&format!("keep_alive={KEEP_ALIVE_MS}")));
        Ok(url)
    }

    /// Connects to `url` with `token`, over this source's TLS. tungstenite
    /// logs this request, token and all, at trace: the daemon's log filter
    /// turns that off ([`hiding_credentials`]).
    async fn connect(&self, url: &Url, token: &str) -> Result<Socket, BrowserError> {
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| unavailable(format!("the session request is invalid ({e})"), false))?;
        let bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| unavailable("the Cloudflare token is not a valid header", false))?;
        req.headers_mut().insert(header::AUTHORIZATION, bearer);
        let connector = match url.scheme() {
            "wss" => Connector::Rustls(
                self.tls
                    .clone()
                    .map_err(|e| unavailable(format!("TLS could not be set up ({e})"), false))?,
            ),
            _ => Connector::Plain,
        };
        let connect =
            tokio_tungstenite::connect_async_tls_with_config(req, None, true, Some(connector));
        match clax_rt::time::timeout(OPEN_TIMEOUT, connect).await {
            Err(_) => Err(unavailable(
                format!(
                    "Browser Run did not answer within {} s; try again",
                    OPEN_TIMEOUT.as_secs()
                ),
                true,
            )),
            Ok(Err(e)) => Err(refusal(&e)),
            Ok(Ok((ws, _))) => Ok(ws),
        }
    }
}

/// One of the instance's concurrent browsers, given back when dropped.
struct Slot(Arc<Mutex<u32>>);

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *open = open.saturating_sub(1);
    }
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// [`CODE_BROWSER_UNAVAILABLE`] with `message`.
fn unavailable(message: impl Into<String>, retryable: bool) -> BrowserError {
    BrowserError::new(CODE_BROWSER_UNAVAILABLE, message, retryable)
}

/// What a failed upgrade means for the model (see the module docs).
fn refusal(e: &WsError) -> BrowserError {
    let WsError::Http(resp) = e else {
        return unavailable(
            format!("Browser Run could not be reached ({e}); try again"),
            true,
        );
    };
    let status = resp.status().as_u16();
    match status {
        401 | 403 => unavailable(
            format!(
                "Browser Run refused the instance's Cloudflare token (HTTP {status}): the token lacks Browser Rendering – Edit, which an admin adds to it"
            ),
            false,
        ),
        429 => {
            let after = resp
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let wait = match after {
                Some(s) => format!("; try again after {s} s"),
                None => "; try again later".to_string(),
            };
            unavailable(
                format!("Browser Run is limiting this instance's browsers (HTTP 429){wait}"),
                true,
            )
        }
        500..=599 => unavailable(
            format!("Browser Run failed (HTTP {status}); try again"),
            true,
        ),
        _ => unavailable(
            format!("Browser Run refused the session (HTTP {status})"),
            false,
        ),
    }
}

/// One CDP connection over a Browser Run WebSocket, holding its slot.
struct WsTransport {
    ws: Socket,
    closed: bool,
    _slot: Slot,
}

impl WsTransport {
    /// A failure of the open socket: the browser had opened.
    fn failed(e: &WsError) -> BrowserError {
        unavailable(format!("the connection to Browser Run failed ({e})"), true).opened(true)
    }
}

#[async_trait::async_trait]
impl CdpTransport for WsTransport {
    async fn send(&mut self, frame: String) -> Result<(), BrowserError> {
        if self.closed {
            return Err(unavailable("the connection to Browser Run is closed", false).opened(true));
        }
        self.ws
            .send(Message::text(frame))
            .await
            .map_err(|e| Self::failed(&e))
    }

    async fn recv(&mut self) -> Result<Option<String>, BrowserError> {
        if self.closed {
            return Ok(None);
        }
        loop {
            match self.ws.next().await {
                None | Some(Ok(Message::Close(_))) => {
                    self.closed = true;
                    return Ok(None);
                }
                Some(Ok(Message::Text(t))) => return Ok(Some(t.as_str().to_string())),
                Some(Ok(Message::Binary(b))) => {
                    return String::from_utf8(b.to_vec()).map(Some).map_err(|_| {
                        unavailable("Browser Run sent a frame that is not text", false).opened(true)
                    });
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => {
                    self.closed = true;
                    return Ok(None);
                }
                Some(Err(e)) => return Err(Self::failed(&e)),
            }
        }
    }

    async fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let _ = clax_rt::time::timeout(Duration::from_secs(5), self.ws.close(None)).await;
    }
}

#[async_trait::async_trait]
impl BrowserSource for DaemonBrowser {
    async fn open(&self, cancel: &CancelToken) -> Result<Box<dyn CdpTransport>, BrowserError> {
        let cancelled = || BrowserError::new(CODE_CANCELLED, "the run was cancelled", false);
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let (creds, max) = self.settings().await?;
        let Some((account, token)) = creds else {
            return Err(unavailable(NOT_CONFIGURED, false));
        };
        let Some(slot) = self.take_slot(max) else {
            return Err(BrowserError::new(
                CODE_BROWSER_BUSY,
                format!(
                    "the instance's {max} concurrent browsers are all in use; try again in a moment"
                ),
                true,
            ));
        };
        let url = self.session_url(&account)?;
        let ws = tokio::select! {
            r = self.connect(&url, token.expose()) => r?,
            () = cancel.cancelled() => return Err(cancelled()),
        };
        Ok(Box::new(WsTransport {
            ws,
            closed: false,
            _slot: slot,
        }))
    }

    /// With a key-encryption key, an account ID and a sealed token that
    /// opens now ([`has_credentials`]), a store error reading as none.
    fn configured(&self) -> bool {
        has_credentials(&self.store, self.kek.as_ref()).unwrap_or(false)
    }

    /// A timer on the runtime's clock, until `at` on the store's.
    fn deadline(&self, at: DateTime<Utc>) -> Deadline {
        let wait = (at - self.store.now_utc())
            .to_std()
            .unwrap_or(Duration::ZERO);
        Box::pin(clax_rt::time::sleep(wait))
    }

    fn clax_file(&self, path: &str) -> Option<Served> {
        let (content_type, body) = crate::shell::bridge_file(&*self.site.as_ref()?.web, path)?;
        Some(Served {
            status: 200,
            headers: vec![("Content-Type".into(), content_type)],
            body,
        })
    }

    /// The headers the site layer puts on a response of `artifact`'s own
    /// site host (its isolation headers and its lockdown policy, framed as
    /// that host's pages are), none without a site.
    fn site_headers(&self, artifact: &str) -> Vec<(String, String)> {
        let (Some(s), Ok(aid)) = (&self.site, ArtifactId::parse(artifact)) else {
            return Vec::new();
        };
        let mut h = HeaderMap::new();
        clax_http::site_access::site_headers(
            clax_http::site_access::site_policy(s, false),
            &site_host(s, &aid),
            None,
            &mut h,
        );
        pairs(&h)
    }

    /// The site API's answer as the run's person: the owner for
    /// [`OWNER`], else the connected person of that public ID.
    async fn site_api(&self, person: &str, artifact: &str, path_and_query: &str) -> Served {
        let (Some(s), Ok(aid)) = (&self.site, ArtifactId::parse(artifact)) else {
            return not_found();
        };
        let holder = if person == OWNER {
            Holder::Owner
        } else {
            Holder::Person(person.to_string())
        };
        let res = crate::routes::site_api::get_as(s, holder, &aid, path_and_query).await;
        let (status, headers) = (res.status().as_u16(), pairs(res.headers()));
        match axum::body::to_bytes(res.into_body(), crate::loopback::RESPONSE_LIMIT).await {
            Ok(body) => Served {
                status,
                headers,
                body: body.to_vec(),
            },
            Err(_) => Served {
                status: 502,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: br#"{"error":{"code":"too_large"}}"#.to_vec(),
            },
        }
    }

    fn own_hosts(&self) -> Result<OwnHosts, String> {
        self.own.clone()
    }

    /// Whether every address the system's resolver answers for `host` is
    /// public; a name that does not resolve is refused too.
    async fn host_public(&self, host: &str) -> Result<(), String> {
        let addrs = self
            .lookup
            .lookup(host)
            .await
            .map_err(|e| format!("{host} does not resolve ({e})"))?;
        if addrs.is_empty() {
            return Err(format!("{host} resolves to no address"));
        }
        match addrs.iter().find(|ip| !crate::web_source::is_public(**ip)) {
            Some(bad) => Err(format!(
                "{host} resolves to {bad}, a private or local address"
            )),
            None => Ok(()),
        }
    }
}

/// Artifact `aid`'s site host on `s`, as its pages are served there
/// (`<aid>.localhost:<port>` on a local daemon).
fn site_host(s: &AppState, aid: &ArtifactId) -> String {
    let public = clax_http::site_access::public_origin(s);
    let host = public.split_once("://").map_or(public.as_str(), |(_, h)| h);
    let origin = clax_http::site_access::site_origin(s, aid, host);
    origin
        .split_once("://")
        .map_or(origin.as_str(), |(_, h)| h)
        .to_string()
}

/// `h` as name and value pairs, a value that is not text left out.
fn pairs(h: &HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
        .collect()
}

/// 404 `not_found`, as JSON.
fn not_found() -> Served {
    Served {
        status: 404,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: br#"{"error":{"code":"not_found"}}"#.to_vec(),
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::audit::AuditCtx;
    use clax_store::secrets::SecretSlot;
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
    const TOKEN: &str = "cf-test-token-1234";

    /// `fut`'s output, bounded by the hang guard of `clax_testkit`, which
    /// this crate's unit tests cannot reach (it depends on this crate).
    #[expect(
        clippy::disallowed_methods,
        reason = "within is the HANG guard on a wait"
    )]
    async fn within<F: std::future::IntoFuture>(what: &str, fut: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(120), fut)
            .await
            .unwrap_or_else(|_| panic!("{what}: not after the hang guard"))
    }

    /// A log line of tungstenite's client handshake, which prints the
    /// whole upgrade request with its `Authorization` header, is dropped by
    /// a filter [`hiding_credentials`] made, whatever the filter asked for.
    #[test]
    fn the_handshake_trace_is_never_logged() {
        use std::io::Write;
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for asked in [
            "trace",
            "tungstenite=trace",
            "tungstenite::handshake=trace",
            "tungstenite::handshake::client=trace",
        ] {
            let buf = Buf::default();
            let out = buf.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(hiding_credentials(tracing_subscriber::EnvFilter::new(
                    asked,
                )))
                .with_writer(move || out.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                tracing::trace!(target: "tungstenite::handshake::client", "Request: Bearer {TOKEN}");
                tracing::trace!(target: "tungstenite::protocol", "a frame");
            });
            let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
            assert!(!logged.contains(TOKEN), "{asked}: {logged}");
            if matches!(asked, "trace" | "tungstenite=trace") {
                assert!(logged.contains("a frame"), "{asked}: {logged}");
            }
        }
    }

    /// What the fake Browser Run saw of an upgrade.
    #[derive(Clone, Debug)]
    struct Seen {
        path_and_query: String,
        authorization: String,
    }

    /// How the fake answers an upgrade: accept it, or refuse it with a
    /// status and headers.
    #[derive(Clone)]
    enum Answer {
        Accept,
        Refuse(u16, Vec<(&'static str, &'static str)>),
    }

    /// A fake Browser Run on 127.0.0.1: it answers each upgrade as told,
    /// then echoes each text frame back prefixed `echo:` until the client
    /// closes, and closes the socket itself on the frame `bye`.
    struct FakeRun {
        addr: std::net::SocketAddr,
        seen: Arc<Mutex<Vec<Seen>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    impl FakeRun {
        async fn start(answer: Answer) -> FakeRun {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let s = seen.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = listener.accept().await else {
                        return;
                    };
                    let (s, answer) = (s.clone(), answer.clone());
                    tokio::spawn(async move {
                        #[expect(
                            clippy::result_large_err,
                            reason = "tungstenite's Callback answers its ErrorResponse by value"
                        )]
                        let cb = move |req: &Request, resp: Response| {
                            s.lock().unwrap().push(Seen {
                                path_and_query: req
                                    .uri()
                                    .path_and_query()
                                    .map(ToString::to_string)
                                    .unwrap_or_default(),
                                authorization: req
                                    .headers()
                                    .get("authorization")
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                            match &answer {
                                Answer::Accept => Ok(resp),
                                Answer::Refuse(status, headers) => {
                                    let mut r = ErrorResponse::new(Some("refused".into()));
                                    *r.status_mut() =
                                        tokio_tungstenite::tungstenite::http::StatusCode::from_u16(
                                            *status,
                                        )
                                        .unwrap();
                                    for (k, v) in headers {
                                        r.headers_mut().insert(*k, HeaderValue::from_static(v));
                                    }
                                    Err(r)
                                }
                            }
                        };
                        let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(sock, cb).await else {
                            return;
                        };
                        while let Some(Ok(m)) = ws.next().await {
                            match m {
                                Message::Text(t) if t.as_str() == "bye" => {
                                    let _ = ws.close(None).await;
                                }
                                Message::Text(t) => {
                                    let _ = ws.send(Message::text(format!("echo:{t}"))).await;
                                }
                                Message::Close(_) => return,
                                _ => {}
                            }
                        }
                    });
                }
            });
            FakeRun {
                addr,
                seen,
                _task: task,
            }
        }

        fn base(&self) -> Url {
            Url::parse(&format!("http://{}", self.addr)).unwrap()
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// A store holding an account ID and a token, and its key.
    fn configured_store() -> (tempfile::TempDir, Arc<Store>, Kek) {
        let dir = tempfile::tempdir().unwrap();
        let st = Arc::new(Store::open(&Home::at(dir.path().join("ax"))).unwrap());
        let kek = Kek::generate();
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, Some(json!(ACCOUNT)))],
        )
        .unwrap();
        st.set_secret(&AuditCtx::DAEMON, &kek, &SecretSlot::Cloudflare, TOKEN)
            .unwrap();
        (dir, st, kek)
    }

    async fn browser(run: &FakeRun) -> (tempfile::TempDir, DaemonBrowser) {
        let (dir, st, kek) = configured_store();
        (
            dir,
            DaemonBrowser::new(st, Some(kek)).sending_to(run.base()),
        )
    }

    async fn open(b: &DaemonBrowser) -> Result<Box<dyn CdpTransport>, BrowserError> {
        within("open", b.open(&CancelToken::new())).await
    }

    #[tokio::test]
    async fn open_sends_the_path_keep_alive_and_token() {
        let run = FakeRun::start(Answer::Accept).await;
        let (_d, b) = browser(&run).await;
        assert!(b.configured());
        let mut t = open(&b).await.unwrap();
        within("close", t.close()).await;
        let seen = run.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].path_and_query,
            format!("/client/v4/accounts/{ACCOUNT}/browser-run/devtools/browser?keep_alive=10000")
        );
        assert_eq!(seen[0].authorization, format!("Bearer {TOKEN}"));
        // Cloudflare's own URL, as the daemon builds it.
        let real = DaemonBrowser::new(b.store.clone(), None);
        assert_eq!(
            real.session_url(ACCOUNT).unwrap().as_str(),
            format!(
                "wss://api.cloudflare.com/client/v4/accounts/{ACCOUNT}/browser-run/devtools/browser?keep_alive=10000"
            )
        );
    }

    #[tokio::test]
    async fn a_401_is_not_retryable_and_names_the_permission() {
        for status in [401, 403] {
            let run = FakeRun::start(Answer::Refuse(status, vec![])).await;
            let (_d, b) = browser(&run).await;
            let e = open(&b).await.err().expect("refused");
            assert_eq!(e.code, CODE_BROWSER_UNAVAILABLE);
            assert!(!e.retryable, "{e}");
            assert!(!e.opened, "{e}");
            assert!(e.message.contains("Browser Rendering – Edit"), "{e}");
            assert_eq!(*b.in_use.lock().unwrap(), 0, "the slot is given back");
        }
    }

    #[tokio::test]
    async fn a_429_is_retryable_with_retry_after() {
        let run = FakeRun::start(Answer::Refuse(429, vec![("retry-after", "17")])).await;
        let (_d, b) = browser(&run).await;
        let e = open(&b).await.err().expect("refused");
        assert_eq!(
            (e.code, e.retryable, e.opened),
            (CODE_BROWSER_UNAVAILABLE, true, false)
        );
        assert!(e.message.contains("after 17 s"), "{e}");
        let run = FakeRun::start(Answer::Refuse(503, vec![])).await;
        let (_d, b) = browser(&run).await;
        let e = open(&b).await.err().expect("refused");
        assert!(e.retryable, "a 5xx is retryable: {e}");
        let run = FakeRun::start(Answer::Refuse(404, vec![])).await;
        let (_d, b) = browser(&run).await;
        let e = open(&b).await.err().expect("refused");
        assert!(!e.retryable, "another 4xx is not: {e}");
    }

    #[tokio::test]
    async fn a_refused_connection_is_retryable_and_no_credentials_is_not_configured() {
        // A port nothing listens on.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        drop(listener);
        let (_d, st, kek) = configured_store();
        let b = DaemonBrowser::new(st.clone(), Some(kek)).sending_to(base.clone());
        let e = open(&b).await.err().expect("refused");
        assert_eq!(
            (e.code, e.retryable),
            (CODE_BROWSER_UNAVAILABLE, true),
            "{e}"
        );
        let none = DaemonBrowser::new(st, None).sending_to(base);
        assert!(!none.configured());
        let e = open(&none).await.err().expect("refused");
        assert_eq!(
            (e.code, e.retryable),
            (CODE_BROWSER_UNAVAILABLE, false),
            "{e}"
        );
        assert!(e.message.contains("not configured"), "{e}");
    }

    #[tokio::test]
    async fn the_semaphore_refuses_past_max_concurrent() {
        let run = FakeRun::start(Answer::Accept).await;
        let (_d, b) = browser(&run).await;
        let first = open(&b).await.unwrap();
        let _second = open(&b).await.unwrap();
        let e = open(&b).await.err().expect("busy");
        assert_eq!(
            (e.code, e.retryable, e.opened),
            (CODE_BROWSER_BUSY, true, false)
        );
        assert!(e.message.contains('2'), "{e}");
        assert_eq!(run.seen().len(), 2, "a busy open sends nothing");
        drop(first);
        let _third = open(&b).await.unwrap();
        // The setting is read at each open.
        b.store
            .set_bot_settings(
                &AuditCtx::DAEMON,
                &[(Setting::BrowserMaxConcurrent, Some(json!(3)))],
            )
            .unwrap();
        let _fourth = open(&b).await.unwrap();
        assert_eq!(
            open(&b).await.err().map(|e| e.code),
            Some(CODE_BROWSER_BUSY)
        );
    }

    #[tokio::test]
    async fn frames_go_both_ways() {
        let run = FakeRun::start(Answer::Accept).await;
        let (_d, b) = browser(&run).await;
        let mut t = open(&b).await.unwrap();
        within(
            "send",
            t.send(r#"{"id":1,"method":"Browser.getVersion"}"#.into()),
        )
        .await
        .unwrap();
        assert_eq!(
            within("recv", t.recv()).await.unwrap().as_deref(),
            Some(r#"echo:{"id":1,"method":"Browser.getVersion"}"#)
        );
        // The peer closing is the end of frames, not an error.
        within("send", t.send("bye".into())).await.unwrap();
        assert_eq!(within("recv", t.recv()).await.unwrap(), None);
        assert_eq!(within("recv", t.recv()).await.unwrap(), None);
        within("close", t.close()).await;
        within("close", t.close()).await;
        drop(t);
        assert_eq!(*b.in_use.lock().unwrap(), 0, "the slot is given back");
    }

    #[tokio::test]
    async fn cancel_before_or_during_open_opens_nothing() {
        let run = FakeRun::start(Answer::Accept).await;
        let (_d, b) = browser(&run).await;
        let cancel = CancelToken::new();
        cancel.cancel();
        let e = within("a cancelled open", b.open(&cancel))
            .await
            .err()
            .expect("cancelled");
        assert_eq!(e.code, CODE_CANCELLED);
        assert!(run.seen().is_empty());
        assert_eq!(*b.in_use.lock().unwrap(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_fires_at_the_stores_time() {
        let (_d, st, kek) = configured_store();
        let b = DaemonBrowser::new(st.clone(), Some(kek));
        let at = st.now_utc() + chrono::Duration::seconds(60);
        let mut d = b.deadline(at);
        assert!(futures::poll!(&mut d).is_pending());
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(futures::poll!(&mut d).is_pending());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(futures::poll!(&mut d).is_ready());
        // A deadline already passed fires at once.
        let mut past = b.deadline(st.now_utc() - chrono::Duration::seconds(1));
        assert!(futures::poll!(&mut past).is_ready());
    }

    /// Names to addresses, for the browse checks.
    struct Names;

    impl Lookup for Names {
        fn lookup(&self, host: &str) -> crate::web_source::LookupFuture {
            let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
            let r = match host {
                "public.example" => Ok(vec![ip("93.184.215.14")]),
                "rebind.example" => Ok(vec![ip("93.184.215.14"), ip("127.0.0.1")]),
                "metadata.example" => Ok(vec![ip("169.254.169.254")]),
                "empty.example" => Ok(vec![]),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such host",
                )),
            };
            Box::pin(async move { r })
        }
    }

    #[tokio::test]
    async fn a_browse_reaches_only_names_on_public_addresses() {
        let (_d, st, kek) = configured_store();
        let b = DaemonBrowser::new(st, Some(kek)).with_lookup(Arc::new(Names));
        assert_eq!(b.host_public("public.example").await, Ok(()));
        for refused in [
            "rebind.example",
            "metadata.example",
            "empty.example",
            "nowhere.example",
        ] {
            assert!(b.host_public(refused).await.is_err(), "{refused}");
        }
        let why = b.host_public("rebind.example").await.unwrap_err();
        assert!(why.contains("127.0.0.1"), "{why}");
        // A local daemon has no own hosts; one that cannot name its hosts
        // says why.
        assert_eq!(b.own_hosts(), Ok(OwnHosts::default()));
        let b = b.with_own_hosts(Err("no host".into()));
        assert_eq!(b.own_hosts(), Err("no host".to_string()));
    }
}

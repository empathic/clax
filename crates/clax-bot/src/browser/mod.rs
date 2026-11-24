//! ClaxBot in a browser (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`): the
//! sessions `clax.browser.open` holds across a run's steps
//! ([`live`]), the one-call `clax.browser.test`, and [`BrowserSource`],
//! where browsers come from.
//!
//! # One driver, any host
//!
//! Everything about browser use lives in this crate (spec §4.4): the
//! Chrome DevTools Protocol session, the request policy that serves a
//! clax or supplied content from its synthetic origin, locators and
//! actions ([`locate`], [`act`]), caps, the answers' shapes and metering. A host provides only a [`BrowserSource`]: a [`CdpTransport`]
//! to a browser (a WebSocket to Cloudflare Browser Run on the daemon),
//! its timer, Clax's own UI files, the site's response headers and a
//! read-only `GET` of the page runtime's API. Its calls are charged
//! through the one Spend path every capability shares
//! (`crate::clax`'s `spend`, spec §4.6).
//!
//! [`cdp`] is one CDP connection over any transport; [`fake`] is a
//! browser in memory for tests ([`FakeChrome`], [`FakeBrowserSource`]).
//!
//! # Who may use it
//!
//! A run may use a browser by its capabilities
//! ([`crate::capability::Capabilities::now`]): the `browser.open` kinds it
//! held when it was made are kept on it
//! ([`clax_store::bot_runs::Run::caps`]) and declare its operations; what
//! changes later is checked at each call.
//!
//! # Errors
//!
//! One vocabulary (spec §5.4): each [`BrowserError`] has a code below, a
//! message written for the model (what to do next), a `retryable` flag,
//! and whether a browser had opened when it failed (`opened`: a call whose
//! browser opened is charged for its time, whatever it answered).
//! Arguments that fail their check are
//! [`api::CODE_BAD_ARGS`](crate::api::CODE_BAD_ARGS).

pub mod act;
pub mod browse;
pub mod cap;
pub mod cdp;
pub mod fake;
pub mod held;
pub mod layer;
pub mod live;
pub mod locate;
pub mod opts;
pub mod record;
pub mod serve;
pub mod session;

pub use cdp::{Cdp, CdpEvent, CdpFail};
pub use fake::{ChromeLog, FakeBrowserSource, FakeChrome, PageModel};

use chrono::{DateTime, Utc};
use clax_bot_sandbox::{CancelToken, MaybeSend, MaybeSendSync};
use std::fmt;

/// The operation name of a test of a clax.
pub const OP_TEST: &str = "browser.test";
pub use layer::{OP_ACT, OP_CLOSE, OP_EXPECT, OP_OPEN, OP_PAGE};

/// The run did not declare the operation, or the admin, the person or the
/// host's credentials turned browser use off mid-run.
pub const CODE_BROWSER_DISABLED: &str = "browser_disabled";
/// The run's tests (or browses) are used up.
pub const CODE_BROWSER_CAP: &str = "browser_cap";
pub use crate::capability::{
    CODE_CREDITS_EXHAUSTED, CODE_URL_NOT_ALLOWED, CODE_URL_NOT_IN_PRIOR_CONTEXT,
};
/// No such version of the artifact, or no such page in the target.
pub const CODE_BROWSER_TARGET_NOT_FOUND: &str = "browser_target_not_found";
/// The instance's concurrent browsers are all in use; retryable.
pub const CODE_BROWSER_BUSY: &str = "browser_busy";
/// Browser Run refused or failed before the page opened: retryable unless
/// the token lacks the permission (401, 403).
pub const CODE_BROWSER_UNAVAILABLE: &str = "browser_unavailable";
/// The deadline passed before the page loaded.
pub const CODE_BROWSER_TIMEOUT: &str = "browser_timeout";
/// The session's browser is gone (the daemon restarted, the socket
/// dropped, the page crashed or detached), or this process holds no such
/// session for the run; not retryable: open a new one.
pub const CODE_BROWSER_SESSION_LOST: &str = "browser_session_lost";
/// The session was closed (`close`, its idle time, its lifetime, a
/// revoke, credits, the run's end); the reason is in the message.
pub const CODE_BROWSER_SESSION_CLOSED: &str = "browser_session_closed";
/// The page's capability does not allow the call: its classes, its expiry,
/// its origins, a condition the host does not understand, or it was
/// revoked (spec §18.3); not retryable, the reason is in the message.
pub const CODE_BROWSER_FORBIDDEN: &str = "browser_forbidden";

/// A browser call that failed: a code above, a message for the model,
/// whether trying again may succeed, and whether a browser had opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    /// Whether a browser was open when the call failed: from the
    /// WebSocket's upgrade on. A call whose browser opened is charged for
    /// its measured time (spec §8.2).
    pub opened: bool,
}

impl BrowserError {
    /// An error before any browser opened.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> BrowserError {
        BrowserError {
            code,
            message: message.into(),
            retryable,
            opened: false,
        }
    }

    /// This error, marked as having had a browser open (or not).
    #[must_use]
    pub fn opened(mut self, opened: bool) -> BrowserError {
        self.opened = opened;
        self
    }
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BrowserError {}

/// One answer the host serves the page under test: a status, its headers
/// and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A future that resolves at a deadline ([`BrowserSource::deadline`]):
/// `Send` natively; not on wasm32, whose timers are JavaScript values.
#[cfg(not(target_arch = "wasm32"))]
pub type Deadline = futures::future::BoxFuture<'static, ()>;
/// A future that resolves at a deadline (see the native definition).
#[cfg(target_arch = "wasm32")]
pub type Deadline = futures::future::LocalBoxFuture<'static, ()>;

/// One CDP connection: JSON text frames each way.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CdpTransport: MaybeSend {
    /// Sends one frame.
    async fn send(&mut self, frame: String) -> Result<(), BrowserError>;
    /// The next frame; `None` once the socket closed. Cancel-safe: a
    /// `recv` dropped before it completed has taken no frame, so the next
    /// one reads it ([`cdp`], "Cancellation of a wait").
    async fn recv(&mut self) -> Result<Option<String>, BrowserError>;
    /// Closes the socket. Closing a closed socket does nothing.
    async fn close(&mut self);
}

/// Where browsers come from: the only part of browser use a host provides
/// (see the module docs). A source answers what it was asked and nothing
/// more: it does not check the grant, caps, credits or the request policy.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait BrowserSource: MaybeSendSync {
    /// Opens a browser session (a WebSocket to Browser Run on the daemon).
    /// A refusal before the browser opened is a [`BrowserError`] with
    /// `opened` false: [`CODE_BROWSER_BUSY`] when the instance's
    /// concurrent browsers are all in use, else
    /// [`CODE_BROWSER_UNAVAILABLE`].
    async fn open(&self, cancel: &CancelToken) -> Result<Box<dyn CdpTransport>, BrowserError>;
    /// Whether the source can open browsers now (credentials present). A
    /// host whose credentials can be removed while runs are going answers
    /// false once they are, and a run's calls are then refused
    /// `browser_disabled` (`not_configured`,
    /// [`crate::capability::Capabilities::now`]).
    fn configured(&self) -> bool;
    /// A future that resolves at `at` (the host's timer; tests drive it).
    fn deadline(&self, at: DateTime<Utc>) -> Deadline;
    /// One of Clax's own UI files a page needs (`/_clax/bridge.js`,
    /// `/_clax/bridge/<part>`), as the host's site serves it; `None` for
    /// a path the site does not serve. `path` is the URL's path as the
    /// page sent it, still percent-encoded, never spelling `/`, `\` or
    /// `.` as an escape ([`serve::Site::answer`] refuses those): a source
    /// answers only an exact match of the bridge's files.
    fn clax_file(&self, path: &str) -> Option<Served>;
    /// The site's response headers for a page of `artifact` (its content
    /// security policy among them), as the host's site serves them.
    fn site_headers(&self, artifact: &str) -> Vec<(String, String)>;
    /// A `GET` of the page runtime's API (`/_clax/api/…`) as the run's
    /// person would make it on the artifact's site, scoped to `artifact`;
    /// `path_and_query` is percent-encoded as [`BrowserSource::clax_file`]'s
    /// path is.
    async fn site_api(&self, person: &str, artifact: &str, path_and_query: &str) -> Served;
    /// The instance's own hosts, which a browse never reaches
    /// ([`browse`]): none by default (a local host's are loopback, which
    /// every browse refuses already); a hosted one's public origin and
    /// site domain. `Err` says why they cannot be named, and every browse
    /// is then refused.
    ///
    /// # Errors
    /// As above.
    fn own_hosts(&self) -> Result<crate::web::url::OwnHosts, String> {
        Ok(crate::web::url::OwnHosts::default())
    }
    /// Whether the name `host` resolves, by the host's resolver, to public
    /// addresses alone ([`crate::web::url::is_local_addr`]): a browse's
    /// page reaches no other name ([`browse::BrowsePolicy`]). A host
    /// that cannot resolve names refuses every one (the default).
    ///
    /// # Errors
    /// Why `host` may not be reached, in words.
    async fn host_public(&self, host: &str) -> Result<(), String> {
        Err(format!("this host cannot check where {host} resolves"))
    }
}

//! ClaxBot in a browser (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`): the
//! values `clax.browser.test` and `clax.browser.browse` are made of, and
//! [`BrowserSource`], where browsers come from.
//!
//! # One driver, any host
//!
//! Everything about browser use lives in this crate (spec §4.4): the
//! Chrome DevTools Protocol session, the request policy that serves a
//! clax from its synthetic origin, steps, caps, the result's shape and
//! metering. A host provides only a [`BrowserSource`]: a [`CdpTransport`]
//! to a browser (a WebSocket to Cloudflare Browser Run on the daemon),
//! its timer, Clax's own UI files, the site's response headers and a
//! read-only `GET` of the page runtime's API. It is its own operation
//! family, built on none of the web tools' internals (spec §4.1).
//!
//! [`cdp`] is one CDP connection over any transport; [`fake`] is a
//! browser in memory for tests ([`FakeChrome`], [`FakeBrowserSource`]).
//!
//! # Who may use it
//!
//! [`BrowserAccess`] (in [`access`]) is what a run may do in a browser
//! now: off, and why, or the operations the admin turned on with their
//! settings. A run's [`BrowserShape`], which operations it declares, is
//! fixed when the run is made and kept on it
//! ([`clax_store::bot_runs::Run::browser`]); what changes later is checked
//! at each call.
//!
//! # Errors
//!
//! One vocabulary (spec §5.4): each [`BrowserError`] has a code below, a
//! message written for the model (what to do next), a `retryable` flag,
//! and whether a browser had opened when it failed (`opened`: a call whose
//! browser opened is charged for its time, whatever it answered).
//! Arguments that fail their check are
//! [`api::CODE_BAD_ARGS`](crate::api::CODE_BAD_ARGS).

pub mod access;
pub mod cdp;
pub mod fake;
pub mod opts;
pub mod serve;
pub mod session;

pub use access::{
    BrowseGrant, BrowseRule, BrowserAccess, BrowserGrant, BrowserOff, BrowserShape, TestGrant,
};
pub use cdp::{Cdp, CdpEvent, CdpFail};
pub use fake::{FakeBrowserSource, FakeChrome, PageModel};

use chrono::{DateTime, Utc};
use clax_bot_sandbox::{CancelToken, MaybeSend, MaybeSendSync};
use std::fmt;

/// The operation name of a test of a clax.
pub const OP_TEST: &str = "browser.test";
/// The operation name of a browse of a public URL.
pub const OP_BROWSE: &str = "browser.browse";

/// The run did not declare the operation, or the admin, the person or the
/// host's credentials turned browser use off mid-run.
pub const CODE_BROWSER_DISABLED: &str = "browser_disabled";
/// The run's tests (or browses) are used up.
pub const CODE_BROWSER_CAP: &str = "browser_cap";
/// The person cannot pay for one call at its time cap.
pub const CODE_CREDITS_EXHAUSTED: &str = "credits_exhausted";
/// No such version of the artifact, or no such page in the target.
pub const CODE_BROWSER_TARGET_NOT_FOUND: &str = "browser_target_not_found";
/// `browse`: the URL fails the host check or the URL rule.
pub const CODE_URL_NOT_ALLOWED: &str = "url_not_allowed";
/// The instance's concurrent browsers are all in use; retryable.
pub const CODE_BROWSER_BUSY: &str = "browser_busy";
/// Browser Run refused or failed before the page opened: retryable unless
/// the token lacks the permission (401, 403).
pub const CODE_BROWSER_UNAVAILABLE: &str = "browser_unavailable";
/// The deadline passed before the page loaded.
pub const CODE_BROWSER_TIMEOUT: &str = "browser_timeout";

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
    /// The next frame; `None` once the socket closed.
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
    /// `browser_disabled` ([`BrowserAccess::now`], not configured).
    fn configured(&self) -> bool;
    /// A future that resolves at `at` (the host's timer; tests drive it).
    fn deadline(&self, at: DateTime<Utc>) -> Deadline;
    /// One of Clax's own UI files a page needs (`/_clax/bridge.js`,
    /// `/_clax/bridge/<part>`), as the host's site serves it; `None` for
    /// a path the site does not serve.
    fn clax_file(&self, path: &str) -> Option<Served>;
    /// The site's response headers for a page of `artifact` (its content
    /// security policy among them), as the host's site serves them.
    fn site_headers(&self, artifact: &str) -> Vec<(String, String)>;
    /// A `GET` of the page runtime's API (`/_clax/api/…`) as the run's
    /// person would make it on the artifact's site.
    async fn site_api(&self, person: &str, artifact: &str, path_and_query: &str) -> Served;
}

//! ClaxBot on the web (spec
//! `docs/superpowers/specs/2026-11-02-claxbot-web-tools-design.md`): the
//! values `clax.web.search` and `clax.web.fetch` are made of, and
//! [`WebSource`], where search results and pages come from.
//!
//! # One implementation, three sources
//!
//! Everything about web use lives in this crate: the argument checks, the
//! URL rule, the page store, windowing, the result shape, the error
//! vocabulary and metering. A host provides only a [`WebSource`]: the
//! Worker's over its AI binding, the daemon's over Cloudflare's REST API,
//! and [`FakeWebSource`] for tests (spec §4.3). A source answers what it
//! was asked and nothing more: it does not check caps, credits or where a
//! URL came from, and it calls the `hop` check it is given on every
//! redirect target before following it, after refusing an `https` to
//! `http` redirect itself ([`url::redirect_allowed`]).
//!
//! # URLs
//!
//! A URL a source is asked to fetch is a [`PublicUrl`]: `http` or
//! `https`, without credentials, at most 2,000 characters, and not an IP
//! literal in a private, loopback, link-local or unique-local range or a
//! name like `localhost` (spec §5.3). Two URLs are the same when their
//! [`PublicUrl::normalized`] forms are equal; that is the only match a
//! source of URLs (spec §5.2) makes.
//!
//! # Pages and windows
//!
//! A fetched page's text is kept whole for the run, up to
//! [`page::STORE_BYTES`], and a script reads it a window at a time
//! ([`page::window`]): at most [`page::PAGE_CHARS`] characters, ending at
//! a line break when one falls in the window's last tenth. Offsets and
//! lengths count Unicode scalar values (`char`s), never bytes. An offset at
//! or past the end is an empty window, not an error.
//!
//! # Errors
//!
//! One vocabulary across sources (spec §6.3): each [`WebError`] has a code
//! below, a message written for the model (what to do next) and a
//! `retryable` flag. Arguments that fail their check are
//! [`api::CODE_BAD_ARGS`](crate::api::CODE_BAD_ARGS). Whether a failed call
//! is charged is [`charged`]: a failed search never is; a fetch that went
//! out on the network is, whatever it answered (spec §7.2).

pub mod fake;
pub mod page;
pub mod url;

pub use fake::FakeWebSource;
pub use url::PublicUrl;

use clax_bot_sandbox::{CancelToken, MaybeSendSync};
use serde_json::Value;
use std::fmt;
use std::time::Duration;

/// The admin or the person turned the operation off mid-run, or the run
/// did not declare it.
pub const CODE_WEB_DISABLED: &str = "web_disabled";
/// The run's search or fetch cap is reached.
pub const CODE_WEB_CAP: &str = "web_cap";
/// The person cannot pay for the call.
pub const CODE_CREDITS_EXHAUSTED: &str = "credits_exhausted";
/// No source of the run holds the URL (spec §5.2).
pub const CODE_URL_NOT_IN_PRIOR_CONTEXT: &str = "url_not_in_prior_context";
/// The URL rule or the host check refuses the URL or a redirect target.
pub const CODE_URL_NOT_ALLOWED: &str = "url_not_allowed";
/// The deadline passed, conversion included.
pub const CODE_WEB_TIMEOUT: &str = "web_timeout";
/// The body passed the byte limit.
pub const CODE_WEB_TOO_LARGE: &str = "web_too_large";
/// Images, audio, video, or a binary body in raw mode.
pub const CODE_WEB_UNSUPPORTED_CONTENT_TYPE: &str = "web_unsupported_content_type";
/// A bot challenge answered the fetch.
pub const CODE_WEB_BLOCKED: &str = "web_blocked";
/// The source failed: a Cloudflare 5xx, a rate limit, payment required at
/// the gateway, or a redirect chain too long.
pub const CODE_WEB_UNAVAILABLE: &str = "web_unavailable";

/// The operation name of a search, as [`charged`] takes it.
pub const OP_SEARCH: &str = "web.search";
/// The operation name of a fetch, as [`charged`] takes it.
pub const OP_FETCH: &str = "web.fetch";

/// Whether a call of `op` ([`OP_SEARCH`] or [`OP_FETCH`]) that failed with
/// `code` is charged and counts against the run's cap (spec §6.3,
/// "Charged"): only a fetch that went out on the network, so one that
/// timed out, was too large, had an unsupported type or was blocked. A
/// failed search never is, and neither is a call refused before it went
/// out or one the source could not make (`web_unavailable`).
#[must_use]
pub fn charged(code: &str, op: &str) -> bool {
    op == OP_FETCH
        && matches!(
            code,
            CODE_WEB_TIMEOUT
                | CODE_WEB_TOO_LARGE
                | CODE_WEB_UNSUPPORTED_CONTENT_TYPE
                | CODE_WEB_BLOCKED
        )
}

/// The search provider a gateway sends a query to; each has its own cost
/// line, `web.search.<as_str>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    Ceramic,
    Exa,
    Linkup,
}

impl Provider {
    /// `ceramic`, `exa` or `linkup`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Ceramic => "ceramic",
            Provider::Exa => "exa",
            Provider::Linkup => "linkup",
        }
    }
}

/// A search, as a source is asked to run it: `limit` is already the
/// grant's, and `gateway` and `byok_alias` are the admin's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    pub query: String,
    pub limit: u8,
    pub provider: Provider,
    /// The AI Gateway the search goes through.
    pub gateway: String,
    /// The gateway's stored provider key to bill, instead of Cloudflare
    /// credits.
    pub byok_alias: Option<String>,
}

/// One search result, as the provider gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub age: Option<String>,
}

/// A source's answer to a search: its hits, and the provider's whole
/// response for the audit record (never shown to a script).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchAnswer {
    pub hits: Vec<SearchHit>,
    pub request_id: Option<String>,
    pub raw: Value,
}

/// What a fetch returns: text converted to Markdown where the type allows
/// (`Auto`), or a textual body as it came (`Raw`, which refuses binary
/// types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Auto,
    Raw,
}

/// The bounds of one fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FetchLimits {
    /// Past this many body bytes, `web_too_large`.
    pub max_bytes: u64,
    /// Past this, `web_timeout`; conversion included.
    pub timeout: Duration,
    /// Past this many redirects, `web_unavailable`.
    pub max_redirects: u8,
}

impl Default for FetchLimits {
    /// 5,000,000 bytes, 30 seconds and 5 redirects (spec §6.3).
    fn default() -> Self {
        FetchLimits {
            max_bytes: 5_000_000,
            timeout: Duration::from_secs(30),
            max_redirects: 5,
        }
    }
}

/// A page a source fetched. A readable HTTP error page (a 4xx or 5xx with
/// a text body) is a page with its `status`, not an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedPage {
    /// The URL the last redirect led to, normalised.
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    /// The whole text: converted, or raw (see [`Format`]).
    pub text: String,
    /// The page's links, absolute and normalised ([`page::links_of_html`],
    /// [`page::links_of_markdown`]).
    pub links: Vec<String>,
    pub request_id: Option<String>,
}

/// A web call that failed: a code above, a message for the model, and
/// whether trying again may succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
}

impl WebError {
    /// An error with no request ID.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> WebError {
        WebError {
            code,
            message: message.into(),
            retryable,
            request_id: None,
        }
    }

    /// This error, with the request ID of the call that failed.
    #[must_use]
    pub fn with_request_id(mut self, request_id: Option<String>) -> WebError {
        self.request_id = request_id;
        self
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WebError {}

/// Where search results and pages come from: the only part of web use a
/// host provides (see the module docs).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait WebSource: MaybeSendSync {
    /// Runs `q`. Zero hits is an answer, not an error.
    async fn search(&self, q: &SearchQuery, cancel: &CancelToken)
    -> Result<SearchAnswer, WebError>;

    /// Fetches `url` within `limits`, calling `hop` on every redirect
    /// target before following it and answering `hop`'s error when it
    /// refuses one. (`hop` is spelled higher-ranked, `for<'u>`, so that
    /// `async_trait` does not tie its argument to `url`'s lifetime: a
    /// source calls it on a target it parsed itself.)
    async fn fetch(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_follow_the_spec_table() {
        assert!(!charged(CODE_WEB_TIMEOUT, OP_SEARCH));
        assert!(charged(CODE_WEB_TIMEOUT, OP_FETCH));
        assert!(!charged(CODE_URL_NOT_ALLOWED, OP_FETCH));
        assert!(!charged(CODE_WEB_CAP, OP_FETCH));
        assert!(!charged(CODE_WEB_CAP, OP_SEARCH));
        for code in [
            CODE_WEB_TOO_LARGE,
            CODE_WEB_UNSUPPORTED_CONTENT_TYPE,
            CODE_WEB_BLOCKED,
        ] {
            assert!(charged(code, OP_FETCH), "{code}");
            assert!(!charged(code, OP_SEARCH), "{code}");
        }
        for code in [
            CODE_WEB_DISABLED,
            CODE_CREDITS_EXHAUSTED,
            crate::api::CODE_BAD_ARGS,
            CODE_URL_NOT_IN_PRIOR_CONTEXT,
            CODE_WEB_UNAVAILABLE,
        ] {
            assert!(!charged(code, OP_FETCH), "{code}");
            assert!(!charged(code, OP_SEARCH), "{code}");
        }
    }

    #[test]
    fn provider_names() {
        assert_eq!(Provider::Ceramic.as_str(), "ceramic");
        assert_eq!(Provider::Exa.as_str(), "exa");
        assert_eq!(Provider::Linkup.as_str(), "linkup");
    }
}

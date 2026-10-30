//! The wire names of browser confirmations, which the daemon
//! (`clax_http::confirm`), the CLI and the MCP client share: while an agent
//! session is live, a decision that matters made with the daemon token
//! answers 202 `{status: "confirmation_pending", confirmation_id,
//! confirm_url, decision, expires_at}` and waits for a click in the
//! owner's browser; the same call made again with the confirmation's ID in
//! [`CONFIRMATION_HEADER`] goes ahead once it is approved.

/// The request header that presents a confirmation's ID with the call it
/// confirms, and the response header that names it on a 202.
pub const CONFIRMATION_HEADER: &str = "x-clax-confirmation";

/// The 202's `status`.
pub const PENDING: &str = "confirmation_pending";

/// The longest `GET /api/confirmations/<id>?wait=` waits, in seconds.
pub const MAX_WAIT_SECS: u64 = 50;

/// The one line a client shows the person for a 202 whose `confirm_url` is
/// `url`.
pub fn line(url: &str) -> String {
    format!("Confirm in your browser: {url}")
}

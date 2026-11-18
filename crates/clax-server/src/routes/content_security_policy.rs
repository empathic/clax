//! The `Content-Security-Policy` Clax sends on an artifact's own content
//! (`artifacts.content_security_policy`), the `content_security_policy` of
//! `PATCH /api/artifacts/<aid>` ([`super::artifacts::patch`]).
//!
//! - With `self` (every artifact's at first) Clax sends a
//!   `Content-Security-Policy` that holds the site's pages to their own
//!   site (`'self'`, `clax_http::content_policy::lockdown`) and answers a
//!   service worker's script with Clax's, which unregisters itself. With
//!   `unrestricted` it sends no source restrictions on the artifact's own
//!   content, only `frame-ancestors` (`super::content`), so a `<meta>`
//!   Content Security Policy of the page's own, if any, is the only one,
//!   and the artifact's own service worker script is served
//!   (`clax_http::host::site_layer`). Either holds from the next request,
//!   and is recorded as `artifact.update` with
//!   `fields.content_security_policy`.
//! - It belongs to the artifact, not to a version, and does not travel: a
//!   fork, a push, a clone or a bundle starts `self`. A draft follows its
//!   artifact's.
//! - Who may manage the artifact changes it
//!   ([`clax_http::agent::administers`]); anyone else 403 `admin_only`.
//!   Never a page (403 `not_for_pages`, for a request carrying
//!   `x-clax-via: page`). An agent's `unrestricted` waits on its person, as
//!   its opening of a site to the world does
//!   ([`super::setting_change::SettingChange`]); `self` never asks.

use clax_core::ArtifactId;
use clax_http::error::ApiError;
use clax_store::site_sessions::ContentSecurityPolicy;

/// The approval question's options.
pub const SERVE_UNRESTRICTED: &str = "Serve it unrestricted";
pub const KEEP_TO_ITS_SITE: &str = "Keep it to its own site";

/// Parses `content_security_policy`: `self` or `unrestricted`; 400
/// `invalid_argument` for anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<ContentSecurityPolicy, ApiError> {
    ContentSecurityPolicy::parse(v).ok_or_else(|| {
        ApiError::bad_request(
            "invalid_argument",
            format!("content_security_policy is self or unrestricted, not '{v}'"),
        )
    })
}

/// Artifact `id`'s `content_security_policy`: `self` or `unrestricted`.
///
/// # Errors
/// The store's.
pub fn of(st: &clax_store::Store, id: &ArtifactId) -> clax_core::Result<&'static str> {
    Ok(st.content_security_policy(id)?.as_str())
}

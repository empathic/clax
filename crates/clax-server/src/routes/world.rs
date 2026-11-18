//! Opening an artifact's site to the world (`artifacts.site_access`), the
//! `site_access` of `PATCH /api/artifacts/<aid>` ([`super::artifacts::patch`]).
//!
//! - `private` closes the site; `world` opens it, so anyone loads it and
//!   uses its runtime as a visitor (`clax_http::site_access`). Either takes
//!   effect on the next request: every site request reads it. Each change
//!   publishes `Event::SiteAccess`, on which a visitor's room socket closes
//!   (4403 `revoked`) when the site closes, and a holder's is checked again
//!   (`super::room`).
//! - Who may manage the artifact changes it
//!   ([`clax_http::agent::administers`]: the page's maker and their
//!   agents, and on a local daemon the owner, by hand or in their browser,
//!   and their agents); anyone else 403 `admin_only`.
//! - An agent (the owner's with a session, or a person's) opens a site to
//!   the world only with its person's approval, as `publish_public`
//!   publishes ([`super::remotes`]); closing never asks. It is one of a
//!   site's switches ([`super::site_switch::Switch::OpenToWorld`]), turned
//!   as that module says.

use clax_http::error::ApiError;

use super::site_switch::Turn;

/// The approval question's options.
pub const OPEN: &str = "Open to the world";
pub const KEEP_PRIVATE: &str = "Keep it private";

/// Parses `site_access`: `world` widens, `private` narrows; 400
/// `invalid_argument` for anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<Turn, ApiError> {
    match v {
        "world" => Ok(Turn::Widen),
        "private" => Ok(Turn::Narrow),
        _ => Err(ApiError::bad_request(
            "invalid_argument",
            format!("site_access is private or world, not '{v}'"),
        )),
    }
}

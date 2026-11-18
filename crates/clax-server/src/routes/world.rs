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
//!   publishes ([`super::remotes`]); closing never asks. The change is a
//!   [`super::setting_change::SettingChange::SiteAccess`], admitted and
//!   applied as that module says.

use clax_http::error::ApiError;

/// The approval question's options.
pub const OPEN: &str = "Open to the world";
pub const KEEP_PRIVATE: &str = "Keep it private";

/// Who may load an artifact's site (`site_access`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteAccess {
    /// `private`: those it is shared with.
    Private,
    /// `world`: anyone, as a visitor.
    World,
}

/// Parses `site_access`: `private` or `world`; 400 `invalid_argument` for
/// anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<SiteAccess, ApiError> {
    match v {
        "world" => Ok(SiteAccess::World),
        "private" => Ok(SiteAccess::Private),
        _ => Err(ApiError::bad_request(
            "invalid_argument",
            format!("site_access is private or world, not '{v}'"),
        )),
    }
}

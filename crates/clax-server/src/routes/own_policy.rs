//! Letting an artifact's site set its own content policy
//! (`artifacts.content_policy`), the `content_policy` of `PATCH
//! /api/artifacts/<aid>` ([`super::artifacts::patch`]).
//!
//! - `locked` (every artifact's at first) holds the site's pages to their
//!   own site (`clax_http::content_policy::lockdown`) and answers a service
//!   worker's script with Clax's, which unregisters itself; `own` lifts
//!   both for the artifact's own content, keeping who may frame it
//!   (`super::content`, `clax_http::host::site_layer`). Either holds from
//!   the next request, and is recorded as `artifact.update` with
//!   `fields.content_policy`.
//! - It belongs to the artifact, not to a version, and does not travel: a
//!   fork, a push, a clone or a bundle starts `locked`. A draft follows its
//!   artifact's.
//! - Who may manage the artifact changes it
//!   ([`clax_http::agent::administers`]); anyone else 403 `admin_only`.
//!   Never a page (403 `not_for_pages`, for a request carrying
//!   `x-clax-via: page`). An agent's `own` waits on its person, as its
//!   opening of a site to the world does: it is one of a site's switches
//!   ([`super::site_switch::Switch::OwnContentPolicy`]); `locked` never asks.

use clax_core::ArtifactId;
use clax_http::error::ApiError;
use clax_store::site_sessions::{LOCKED_POLICY, OWN_POLICY};

use super::site_switch::Turn;

/// The approval question's options.
pub const LET_IT: &str = "Let it set its own content policy";
pub const KEEP_LOCKED: &str = "Keep it locked";

/// Parses `content_policy`: `own` widens, `locked` narrows; 400
/// `invalid_argument` for anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<Turn, ApiError> {
    match v {
        OWN_POLICY => Ok(Turn::Widen),
        LOCKED_POLICY => Ok(Turn::Narrow),
        _ => Err(ApiError::bad_request(
            "invalid_argument",
            format!("content_policy is locked or own, not '{v}'"),
        )),
    }
}

/// Artifact `id`'s `content_policy`: `own` or `locked`.
///
/// # Errors
/// The store's.
pub fn of(st: &clax_store::Store, id: &ArtifactId) -> clax_core::Result<&'static str> {
    Ok(if st.content_policy_own(id)? {
        OWN_POLICY
    } else {
        LOCKED_POLICY
    })
}

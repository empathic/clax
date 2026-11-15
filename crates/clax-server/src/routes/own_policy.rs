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
//!   opening of a site to the world does ([`super::world::Gate`]); `locked`
//!   never asks.

use clax_core::ArtifactId;
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::site_sessions::{LOCKED_POLICY, OWN_POLICY, SET_OWN_POLICY};

use super::world::{Change, Gate};

/// The approval question's options.
pub const LET_IT: &str = "Let it set its own content policy";
pub const KEEP_LOCKED: &str = "Keep it locked";

/// Letting a site set its own content policy, as [`Gate`] asks it.
pub(crate) static GATE: Gate = Gate {
    kind: SET_OWN_POLICY,
    widened: clax_store::Store::content_policy_own,
    to: OWN_POLICY,
    admin_only: "only the page's maker (on a local Clax, the owner too) sets its site's content policy",
    no_session: "an agent lets a site set its own content policy with publish_public's content_policy, which asks the person first",
    header: "Content policy",
    question: "Let \"{title}\" set its own content policy? Its pages may then load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website.",
    yes: (
        LET_IT,
        "Its pages, and every later version's, load and connect where they choose; lock it again at any time",
    ),
    no: (
        KEEP_LOCKED,
        "Its pages load and connect to its own site alone",
    ),
    what: "letting this site set its own content policy",
    declined: "they chose to keep it locked",
};

/// Parses `content_policy`: `Some(true)` for `own`, `Some(false)` for
/// `locked`; 400 `invalid_argument` for anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<bool, ApiError> {
    match v {
        OWN_POLICY => Ok(true),
        LOCKED_POLICY => Ok(false),
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

/// Sets artifact `id`'s content policy, `own` or `locked`, as `caller`,
/// waiting on question `qid` for an agent's `own` (see the module docs).
///
/// # Errors
/// As the module docs say, and the store's.
pub async fn change(
    s: &AppState,
    caller: &Caller,
    audit: DeferredAudit,
    ctx: clax_core::audit::AuditCtx,
    id: &ArtifactId,
    own: bool,
    qid: Option<String>,
) -> Result<Change, ApiError> {
    if let Some(pending) = GATE.admit(s, caller, audit, id, own, qid).await? {
        return Ok(Change::Pending(pending));
    }
    let i2 = id.clone();
    s.store_call(move |st| st.set_content_policy_own(&ctx, &i2, own))
        .await?;
    Ok(Change::Done)
}

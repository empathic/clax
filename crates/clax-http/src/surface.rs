//! The suggestion of where an agent session should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1), gathered from the store's
//! session signals, the person's presence on the session's pages, and the
//! streams the person holds. The routes that answer with it
//! (`where_to_ask`, `questions`, `feedback`) share it.

use crate::agent::{Agent, Need};
use crate::error::ApiError;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use clax_core::CoreError;
use clax_core::surface::{Delivery, Signals, Suggestion, TerminalSignal, suggest};
use clax_store::Store;
use clax_store::inbox::recipient_key;

fn parse(t: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The session's page, as agent `a` may see it: its newest working
/// record's artifact, else the one it last published to or started
/// watching, through the same gate as an artifact the request names
/// ([`Agent::named_artifact`]): for a person's agent, a live page or one
/// its rights do not let it read is no page.
///
/// # Errors
/// The store's.
pub async fn page<N: Need>(
    s: &AppState,
    a: &Agent<N>,
    sid: &str,
) -> Result<Option<String>, ApiError> {
    let raw = match s.working.newest_artifact_of(sid) {
        Some(aid) => Some(aid),
        None => {
            let id = sid.to_string();
            s.store_call(move |st| st.session_artifact(&id)).await?
        }
    };
    Ok(match raw {
        Some(raw) => a
            .named_artifact(s, &raw)
            .await
            .ok()
            .map(|id| id.as_str().to_string()),
        None => None,
    })
}

/// Session `sid`'s signals now, with `page` its page ([`page`]); a page
/// deleted since places nothing, and with no page the delivery's artifact
/// is the session's page. The person whose presence counts is the
/// session's own: a connected person's for their agents, else the
/// owner's; no other viewer's focus or input is read. Only their tabs on
/// the session's pages (its delivery's artifact and its page) count. A
/// delivered thread no longer on its artifact places nothing.
///
/// # Errors
/// `NotFound` for an unknown session; the store's.
pub fn signals(
    s: &AppState,
    st: &Store,
    sid: &str,
    page: Option<&str>,
) -> clax_core::Result<Signals> {
    let sess = st.get_session(sid)?.ok_or(CoreError::NotFound)?;
    let kept = st.session_signals(sid)?.unwrap_or_default();
    let viewer = match &sess.person {
        Some(p) => Some(p.clone()),
        None => st.owner_public_id()?,
    };
    let key = recipient_key(sess.person.as_deref()).to_string();
    let delivery = match (
        kept.delivered_at.as_deref().and_then(parse),
        kept.delivered_artifact_id,
    ) {
        (Some(at), Some(aid)) => {
            let thread_id = match kept.delivered_thread_id {
                Some(t) => st
                    .get_thread(&t)?
                    .filter(|x| x.artifact_id == aid)
                    .map(|x| x.id),
                None => None,
            };
            Some(Delivery {
                at,
                artifact_id: aid,
                thread_id,
            })
        }
        _ => None,
    };
    let page = match page.map(clax_core::ArtifactId::parse) {
        Some(Ok(id)) => st.get_artifact(&id)?.map(|a| a.id.as_str().to_string()),
        _ => None,
    };
    let mut pages: Vec<&str> = Vec::with_capacity(2);
    pages.extend(delivery.as_ref().map(|d| d.artifact_id.as_str()));
    pages.extend(page.as_deref());
    pages.dedup();
    let clax_at = viewer.as_deref().and_then(|v| {
        pages
            .iter()
            .filter_map(|a| s.presence.engaged_at(a, v))
            .max()
    });
    let panel_open = s.stream.holds_panel(&key);
    let artifact_id = page.or_else(|| delivery.as_ref().map(|d| d.artifact_id.clone()));
    Ok(Signals {
        terminal: kept
            .terminal_at
            .as_deref()
            .and_then(parse)
            .map(|at| TerminalSignal {
                at,
                inferred: kept.terminal_inferred,
            }),
        delivery,
        clax_at,
        artifact_id,
        clax_open: panel_open || s.stream.holds_surface(&key),
        panel_open,
    })
}

/// [`suggest`] for session `sid` with page `page`, now by the question
/// clock.
///
/// # Errors
/// [`signals`]'.
pub fn suggestion(
    s: &AppState,
    st: &Store,
    sid: &str,
    page: Option<&str>,
) -> clax_core::Result<Suggestion> {
    Ok(suggest(&signals(s, st, sid, page)?, s.question_clock.now()))
}

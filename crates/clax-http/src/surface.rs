//! The suggestion of where an agent session should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1), gathered from the store's
//! session signals, the person's presence on the session's pages, and the
//! streams the person holds. The routes that answer with it
//! (`where_to_ask`, `questions`, `feedback`) share it.
//!
//! The person whose presence is read is the session's own, and only the
//! session's own credential reaches a session route: the daemon token is
//! refused a session of a person's grant, and a grant one of another grant
//! (`foreign_session`, `crate::identity`). So no caller learns another
//! person's focus or input from a suggestion.

use crate::agent::{Agent, Need};
use crate::error::ApiError;
use crate::state::AppState;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use clax_core::CoreError;
use clax_core::model::Session;
use clax_core::surface::{Delivery, Signals, Suggestion, TerminalSignal, suggest};
use clax_store::Store;
use clax_store::engagement::SessionSignals;
use clax_store::inbox::recipient_key;
use serde_json::{Value, json};

fn parse(t: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The session's page, as agent `a` may see it: its newest working
/// record's artifact, else the one it last published to or started
/// watching, through the same gate as an artifact the request names
/// ([`Agent::named_artifact`]): for a person's agent, a live page or one
/// its rights do not let it read is no page, and so is a stored ID that
/// does not parse.
///
/// # Errors
/// The store's, and the gate's other than its refusals.
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
    let Some(raw) = raw.filter(|r| clax_core::ArtifactId::parse(r).is_ok()) else {
        return Ok(None);
    };
    match a.named_artifact(s, &raw).await {
        Ok(id) => Ok(Some(id.as_str().to_string())),
        Err(e) if matches!(e.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => Ok(None),
        Err(e) => Err(e),
    }
}

/// What one read of session `sid` gives: the session, its kept signals,
/// and the signals the suggestion weighs.
struct Gathered {
    session: Session,
    kept: SessionSignals,
    signals: Signals,
}

fn gather(s: &AppState, st: &Store, sid: &str, page: Option<&str>) -> clax_core::Result<Gathered> {
    let session = st.get_session(sid)?.ok_or(CoreError::NotFound)?;
    let kept = st.session_signals(sid)?.unwrap_or_default();
    let viewer = match &session.person {
        Some(p) => Some(p.clone()),
        None => st.owner_public_id()?,
    };
    let key = recipient_key(session.person.as_deref()).to_string();
    let delivery = match (
        kept.delivered_at.as_deref().and_then(parse),
        kept.delivered_artifact_id.clone(),
    ) {
        (Some(at), Some(aid)) => {
            let thread_id = match &kept.delivered_thread_id {
                Some(t) => st
                    .get_thread(t)?
                    .filter(|x| x.artifact_id == aid)
                    .map(|x| x.id),
                None => None,
            };
            Some(Delivery {
                at,
                artifact_id: aid,
                thread_id,
                written_at: kept.delivered_written_at.as_deref().and_then(parse),
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
    let signals = Signals {
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
    };
    Ok(Gathered {
        session,
        kept,
        signals,
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
    Ok(gather(s, st, sid, page)?.signals)
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

/// `GET /api/sessions/<sid>/where-to-ask`'s reply for live session `sid`
/// with page `page`, from one read of it: `{where_to_ask, signals:
/// {terminal_at, terminal_inferred, delivered_at}}`.
///
/// # Errors
/// `NotFound` for an unknown session, `unknown_session` for an ended one;
/// the store's.
pub fn where_to_ask_reply(
    s: &AppState,
    st: &Store,
    sid: &str,
    page: Option<&str>,
) -> clax_core::Result<Value> {
    let g = gather(s, st, sid, page)?;
    if g.session.ended_at.is_some() {
        return Err(CoreError::invalid(
            "unknown_session",
            "no live session has this ID",
        ));
    }
    let sugg = suggest(&g.signals, s.question_clock.now());
    Ok(json!({"where_to_ask": sugg, "signals": {
        "terminal_at": g.kept.terminal_at,
        "terminal_inferred": g.kept.terminal_inferred,
        "delivered_at": g.kept.delivered_at,
    }}))
}

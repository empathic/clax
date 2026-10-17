//! `GET /api/sessions/<sid>/where-to-ask`: where the session should ask
//! its person now (spec 2026-10-16-ask-where-attention-is-design §1);
//! `POST /api/sessions/<sid>/terminal`: the person typed a prompt to it in
//! the terminal (Pi's input event; Claude Code's prompt hook says so on its
//! feedback poll instead). Both take the session's token or its agent
//! token; an unknown session is 404, an ended one 400 `unknown_session`.

use super::artifacts::path;
use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use clax_core::CoreError;
use clax_http::agent::TokenOrAgent;
use clax_http::audit::DeferredAudit;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::Store;
use serde_json::{Value, json};

fn live(st: &Store, sid: &str) -> clax_core::Result<()> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(x) if x.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "no live session has this ID",
        )),
        Some(_) => Ok(()),
    }
}

/// → `{where_to_ask, signals}`: the suggestion (`surface`, `where`,
/// `confidence`, `why`) and the session's own signals it read
/// (`terminal_at`, `terminal_inferred`, `delivered_at`).
pub async fn where_to_ask(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::SessionWhereToAsk>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let page = clax_http::surface::page(&s, &a, &sid).await?;
    let st = s.clone();
    let v = s
        .store_call(move |db| {
            clax_http::surface::where_to_ask_reply(&st, db, &sid, page.as_deref())
        })
        .await?;
    Ok(Json(v))
}

/// Records the person's prompt now, by the daemon's clock, and withdraws
/// the session's open hook in-terminal questions (`question.withdraw`,
/// reason `lapsed`, as the session's agent), announcing each →
/// `{terminal_at, lapsed}`.
pub async fn terminal(
    State(s): State<AppState>,
    TokenOrAgent(_a): TokenOrAgent<clax_http::agent::SessionTerminal>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let st = s.clone();
    let v = s
        .store_call(move |db| {
            live(db, &sid)?;
            let ctx = audit.for_session(db, &sid)?;
            let lapsed = db.note_terminal(&ctx, &sid)?;
            clax_http::views::question::announce_ids(&st, db, &lapsed);
            let at = db.session_signals(&sid)?.and_then(|k| k.terminal_at);
            Ok(json!({"terminal_at": at, "lapsed": lapsed}))
        })
        .await?;
    Ok(Json(v))
}

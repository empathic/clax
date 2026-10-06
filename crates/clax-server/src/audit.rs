//! The audit journal's daemon side (spec 2026-10-06-toolpath-audit-design
//! §4): the [`AuditCtx`] each request that acts resolves, and the wake-up
//! the store sends the journal appender after a commit that recorded events.
//!
//! The context is built from what the daemon already trusts. The actor comes
//! from [`Identity`] ([`Identity::audit_actor`]), never from a second check
//! of credentials. The agent side's own claims (`x-clax-via`,
//! `x-clax-session`, `x-clax-git`, `x-clax-call`) count only on a request
//! that carries the token; a browser's are ignored. A header that does not
//! decode never fails the request: a bad `x-clax-git` is the capture
//! outcome `invalid`, a bad `x-clax-call` is no call, and a bad `x-clax-via`
//! or `x-clax-session` is inferred as though absent. Header values are never
//! logged.

use crate::identity::Identity;
use crate::routes::artifacts::{SESSION_HEADER, VIA_HEADER};
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use clax_core::audit::{Actor, AuditCtx, CallHeader, Via, decode_call_header};
use clax_core::gitctx::{self, GitField};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

/// Header carrying the agent side's git context (base64url JSON, §9).
pub const GIT_HEADER: &str = "x-clax-git";

/// Header carrying the identity of the tool call a request is made under
/// (base64url JSON, §6.7).
pub const CALL_HEADER: &str = "x-clax-call";

/// The journal appender's wake-up: a channel of capacity one. A nudge is a
/// `try_send`, so it never blocks; nudges while one is pending coalesce,
/// because the appender reads every row past its cursor once woken.
pub struct AuditWake {
    tx: SyncSender<()>,
    rx: Mutex<Option<Receiver<()>>>,
}

impl AuditWake {
    pub fn new() -> Arc<AuditWake> {
        let (tx, rx) = sync_channel(1);
        Arc::new(AuditWake {
            tx,
            rx: Mutex::new(Some(rx)),
        })
    }

    /// Tells the appender there are new rows. Returns at once: a pending
    /// wake-up already covers them, and without an appender there is no one
    /// to tell.
    pub fn nudge(&self) {
        let _ = self.tx.try_send(());
    }

    /// The receiving end, for the one appender; `None` once taken.
    pub fn take_receiver(&self) -> Option<Receiver<()>> {
        self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Makes `store` nudge this wake-up after each commit that recorded an
    /// audit event.
    pub fn install(self: &Arc<Self>, store: &clax_core::Store) {
        let wake = Arc::clone(self);
        store.set_audit_nudge(move || wake.nudge());
    }
}

/// The text value of header `name`; `None` when absent or not visible ASCII.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The channel an agent side may name for itself in `x-clax-via`. The
/// browser channels (`shell`, `extension`, `lan`) follow from the
/// credentials, and `daemon` is internal, so naming them counts for nothing.
fn named_channel(headers: &HeaderMap) -> Option<Via> {
    header(headers, VIA_HEADER)
        .and_then(Via::parse)
        .filter(|v| matches!(v, Via::Mcp | Via::Hook | Via::Pi | Via::Cli))
}

/// The channel of a request with credentials `id`: what the token holder
/// names, else the extension, a browser of the owner's (the shell), the
/// token (the MCP shim when it acts for a session, else the CLI), or a LAN
/// viewer.
pub fn channel(id: &Identity, named: Option<Via>, agent_session: bool) -> Via {
    match named {
        Some(v) if id.token => v,
        _ if id.extension => Via::Extension,
        _ if id.owner_browser() => Via::Shell,
        _ if id.token && agent_session => Via::Mcp,
        _ if id.token => Via::Cli,
        _ => Via::Lan,
    }
}

/// The git half of the context: the decoded `x-clax-git` of a token holder.
fn git_field(id: &Identity, headers: &HeaderMap) -> GitField {
    if !id.token {
        return GitField::Absent;
    }
    match headers.get(GIT_HEADER) {
        None => GitField::Absent,
        Some(v) => match v.to_str() {
            Ok(s) => gitctx::decode_header(s),
            Err(_) => GitField::Capture("invalid"),
        },
    }
}

/// The decoded `x-clax-call` of a token holder; `None` without one, or when
/// it does not decode (the reason, never the value, is logged).
fn call_field(id: &Identity, headers: &HeaderMap) -> Option<CallHeader> {
    if !id.token {
        return None;
    }
    let value = headers.get(CALL_HEADER)?;
    let decoded = value
        .to_str()
        .map_err(|_| "header is not visible ASCII".to_string())
        .and_then(decode_call_header);
    match decoded {
        Ok(call) => Some(call),
        Err(why) => {
            tracing::debug!("ignoring {CALL_HEADER}: {why}");
            None
        }
    }
}

/// The actor and channel of a request with credentials `id` that names
/// channel `named` and session `session`, reading the rows they name.
fn actor_and_channel(
    st: &clax_core::Store,
    id: &Identity,
    named: Option<Via>,
    session: Option<&str>,
) -> clax_core::Result<(Actor, Via)> {
    let actor = id.audit_actor(st, session, named)?;
    let agent_session = matches!(&actor, Actor::Agent(a) if a.session_id.is_some());
    Ok((actor, channel(id, named, agent_session)))
}

/// The audit context of a request that acts. Resolving it reads the
/// session and viewer rows, and makes the viewer row of a first-time
/// viewer or the owner as [`Identity::ensure_viewer`] does, so it belongs
/// only on routes that record events.
impl FromRequestParts<AppState> for AuditCtx {
    type Rejection = crate::error::ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let headers = &parts.headers;
        let id = Identity::from_parts(headers, &parts.extensions, &state.token);
        let named = named_channel(headers);
        let session = header(headers, SESSION_HEADER).map(str::to_string);
        let git = git_field(&id, headers);
        let call = call_field(&id, headers);
        let (actor, via) = state
            .store_call(move |st| actor_and_channel(st, &id, named, session.as_deref()))
            .await?;
        Ok(AuditCtx {
            actor,
            via,
            git,
            call,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_follows_the_credentials() {
        let token = Identity {
            token: true,
            ..Identity::default()
        };
        let extension = Identity {
            extension: true,
            ..Identity::default()
        };
        let shell = Identity {
            owner_cookie: true,
            ..Identity::default()
        };
        let viewer = Identity {
            cookie: Some("01J9Z3K4M5N6P7Q8R9S0T1V2W3".into()),
            ..Identity::default()
        };
        assert_eq!(channel(&token, Some(Via::Hook), true), Via::Hook);
        assert_eq!(channel(&token, None, true), Via::Mcp);
        assert_eq!(channel(&token, None, false), Via::Cli);
        assert_eq!(channel(&extension, Some(Via::Mcp), false), Via::Extension);
        assert_eq!(channel(&shell, Some(Via::Cli), false), Via::Shell);
        assert_eq!(channel(&viewer, Some(Via::Mcp), true), Via::Lan);
        assert_eq!(channel(&Identity::default(), None, false), Via::Lan);
    }
}

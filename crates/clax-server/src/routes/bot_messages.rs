//! A host's two routes to a run it started (`docs/contract.md`, "ClaxBot
//! surfaces"), beside the protocol's others
//! ([`super::bot_surfaces`]):
//!
//! - `POST /api/bot/surfaces/<name>/runs/<id>/messages` (a
//!   `MessageRequest`) → a `MessageResponse`: a message to the run, of
//!   the type its kind takes as its second parameter, given to its model
//!   between steps ([`clax_store::bot_messages`]).
//! - `GET /api/bot/surfaces/<name>/runs/<id>?registration=<id>` → a
//!   `RunState`: where the run is now, for a host that missed the events
//!   a `serve` delivers.
//!
//! Who may: the daemon's token ([`DaemonToken`]), for a run of the
//! surface `<name>` that the registration named started; any other run
//! is 404 `unknown_run`, whoever started it. The router mounts them
//! where it mounts the protocol's others.

use super::artifacts::path;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_bot::broker::CallBroker;
use clax_bot::clax_surface::Definition;
use clax_bot::clax_surface::wire::{
    CODE_INVALID_REQUEST, CODE_KEY_CONFLICT, CODE_RUN_ENDED, CODE_TAKES_NO_MESSAGES,
    CODE_TOO_LARGE, CODE_TOO_MANY_MESSAGES, CODE_UNKNOWN_RUN, End, MessageRequest, MessageResponse,
    RunState,
};
use clax_core::CoreError;
use clax_http::agent::DeniedToGrants;
use clax_http::bot_surfaces::{DaemonToken, refused};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::bot_messages::{self, MessageBounds, NewMessage};
use clax_store::bot_runs::Run;
use std::sync::Arc;

/// `?registration=` of the `GET`.
#[derive(serde::Deserialize)]
pub struct Registration {
    registration: String,
}

fn unknown_run() -> ApiError {
    refused(
        CODE_UNKNOWN_RUN,
        "the surface has no such run that this registration started",
    )
}

/// Run `id` of the surface `name`, started by `registration`, with the
/// name of its kind within the surface and the hash of the definition
/// its kind's binding was made with; `None` for any other run.
fn started_run(
    st: &Store,
    name: &str,
    id: &str,
    registration: &str,
) -> clax_core::Result<Option<(Run, String, Option<String>)>> {
    let Some((started_by, _)) = st.run_origin(id)? else {
        return Ok(None);
    };
    if started_by.as_deref() != Some(registration) {
        return Ok(None);
    }
    let Some((run, made, _)) = st.run_with_steps_as_made(id)? else {
        return Ok(None);
    };
    let Some(kind) = made
        .kind
        .strip_prefix(name)
        .and_then(|k| k.strip_prefix('.'))
    else {
        return Ok(None);
    };
    let hash = made
        .bindings()
        .ok()
        .and_then(|b| b.of(name).and_then(|b| b.hash().map(str::to_string)));
    Ok(Some((run, kind.to_string(), hash)))
}

/// The definition the run's kind was declared in, as its binding was
/// made with it.
fn definition_of(
    broker: &Arc<clax_http::bot_surfaces::Broker>,
    registration: &str,
    hash: Option<&str>,
) -> Option<Arc<Definition>> {
    hash?;
    broker
        .registration(registration, hash)
        .map(|served| served.definition)
}

/// `POST /api/bot/surfaces/<name>/runs/<id>/messages` (see the module
/// documentation). The message is checked against the type the run's
/// kind takes as its second parameter, in the definition the run was
/// started with: a kind with one parameter takes none (400
/// `takes_no_messages`), a message that is not of the type is 400
/// `invalid_request`, and one that holds content is 400 too, while
/// none is stored. A run that has ended is 409 `run_ended`; a key the
/// run holds with another message 409 `key_conflict`, and with the
/// same message the answer the first was given; a message larger than
/// the hub's `run_message_bytes` 413 `too_large`; one more than the
/// hub's `run_messages` waiting 429 `too_many_messages`. A message to
/// a run between steps wakes its driver. A message to a parked run
/// leaves it parked on the call it waits on, and wakes nothing: the
/// answer says `waiting`, and the message is given at the run's first
/// turn after the call is answered, its deadline comes or the run is
/// cancelled.
pub async fn send(
    State(s): State<AppState>,
    _token: DaemonToken,
    _d: DeniedToGrants,
    p: Result<Path<(String, String)>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (name, id) = path(p)?;
    let surfaces = s.bot_surfaces.clone();
    let limits = surfaces.limits();
    let b: MessageRequest = super::bot_surfaces::message(body, limits.request_bytes)?;
    let broker = surfaces.broker().clone();
    let run = id.clone();
    let sent = s
        .store_call(move |st| {
            let invalid = |m: String| refused(CODE_INVALID_REQUEST, m);
            let Some((_, kind, hash)) = started_run(st, &name, &run, &b.registration)? else {
                return Ok(Err(unknown_run()));
            };
            let Some(def) = definition_of(&broker, &b.registration, hash.as_deref()) else {
                return Ok(Err(unknown_run()));
            };
            let Some(kind) = def.kind(&kind) else {
                return Ok(Err(unknown_run()));
            };
            if kind.message.is_none() {
                return Ok(Err(refused(
                    CODE_TAKES_NO_MESSAGES,
                    format!(
                        "kind {} takes no messages after it starts: it has one parameter",
                        kind.name
                    ),
                )));
            }
            let held = match kind.message_content_places(&def, &b.message) {
                Ok(held) => held,
                Err(e) => return Ok(Err(invalid(e.message))),
            };
            if !held.is_empty() {
                return Ok(Err(invalid(format!(
                    "kind {}: the message holds content at {}: content reaches a run in the \
                     answers to its calls alone",
                    kind.name,
                    held.join(", ")
                ))));
            }
            let appended = st.append_message(
                NewMessage {
                    run_id: &run,
                    registration_id: &b.registration,
                    body: &b.message,
                    key: b.key.as_deref(),
                },
                MessageBounds {
                    undelivered: limits.run_messages,
                    bytes: limits.run_message_bytes,
                },
            );
            Ok(match appended {
                Ok(appended) => Ok(appended),
                Err(CoreError::NotFound) => Err(unknown_run()),
                Err(CoreError::Invalid { code, message }) => Err(match code {
                    bot_messages::CODE_NOT_THE_STARTER => unknown_run(),
                    bot_messages::CODE_RUN_ENDED => refused(CODE_RUN_ENDED, message),
                    bot_messages::CODE_KEY_REUSED => refused(CODE_KEY_CONFLICT, message),
                    bot_messages::CODE_TOO_MANY_MESSAGES => {
                        refused(CODE_TOO_MANY_MESSAGES, message)
                    }
                    bot_messages::CODE_MESSAGE_TOO_LARGE => refused(CODE_TOO_LARGE, message),
                    _ => return Err(CoreError::Invalid { code, message }),
                }),
                Err(e) => return Err(e),
            })
        })
        .await??;
    if sent.new && !sent.waiting {
        // A run between steps has its next turn to take.
        s.bot.runs.started(&id);
    }
    Ok(Json(MessageResponse {
        message: sent.message.id,
        seq: u64::try_from(sent.message.seq).unwrap_or(0),
        waiting: sent.waiting,
    }))
}

/// `GET /api/bot/surfaces/<name>/runs/<id>?registration=<id>` (see the
/// module documentation): the run's status, its committed steps, what a
/// parked run waits on (as an event's `waiting_on`: its surface, and
/// the operation and deadline when that surface is `<name>`), how many
/// messages it has not been given, and, once it has ended, the `seq`s of
/// those it never will be, and how it ended and its result as its
/// registration is told them (`ended`), while that is kept.
pub async fn state(
    State(s): State<AppState>,
    _token: DaemonToken,
    _d: DeniedToGrants,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<Registration>, QueryRejection>,
) -> Result<Json<RunState>, ApiError> {
    let (name, id) = path(p)?;
    let Query(q) =
        q.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, CODE_INVALID_REQUEST, e.body_text()))?;
    let state = s
        .store_call(move |st| {
            let Some((run, kind, _)) = started_run(st, &name, &id, &q.registration)? else {
                return Ok(None);
            };
            let steps = st.run_with_steps(&id)?.map_or(0, |(_, steps)| {
                steps.iter().filter(|s| s.committed_at.is_some()).count()
            });
            let waiting_on = st.run_parking(&id)?.and_then(|p| p.waiting_on).map(|on| {
                let surface = on["surface"].as_str().unwrap_or_default().to_string();
                let mut shown = serde_json::json!({"surface": surface});
                if surface == name {
                    let op = on["op"].as_str().unwrap_or_default();
                    let prefix = format!("{name}.");
                    shown["op"] = op.strip_prefix(&prefix).unwrap_or(op).into();
                    shown["deadline"] = on["deadline"].clone();
                }
                shown
            });
            let undelivered: Vec<u64> = st
                .undelivered_messages(&id)?
                .iter()
                .map(|m| u64::try_from(m.seq).unwrap_or(0))
                .collect();
            let messages_waiting = undelivered.len();
            let kept = if run.status.is_terminal() {
                st.kept_end(&id, &q.registration)?
            } else {
                None
            };
            let (end, result) = match kept {
                Some(kept) => (serde_json::from_value::<End>(kept.end).ok(), kept.result),
                None => (None, None),
            };
            Ok(Some(RunState {
                run: run.id,
                kind,
                status: run.status.as_str().to_string(),
                steps: u64::try_from(steps).unwrap_or(0),
                waiting_on,
                messages_waiting: u64::try_from(messages_waiting).unwrap_or(0),
                undelivered: if run.status.is_terminal() {
                    undelivered
                } else {
                    Vec::new()
                },
                end,
                result,
            }))
        })
        .await?;
    state.map(Json).ok_or_else(unknown_run)
}

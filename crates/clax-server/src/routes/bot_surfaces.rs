//! The ClaxBot surfaces host protocol's routes (`docs/contract.md`,
//! "ClaxBot surfaces"), over the daemon's hub of registered surfaces
//! ([`clax_http::bot_surfaces`]):
//!
//! - `PUT /api/bot/surfaces/<name>` (a `RegisterRequest`) → a
//!   `Registered`: registers the surface, or goes on as a registration.
//! - `POST /api/bot/surfaces/<name>/serve` (a `ServeRequest`) → a
//!   `ServeResponse`: one long-poll, which brings the host's answers and
//!   is answered with its calls and ended runs.
//! - `POST /api/bot/surfaces/<name>/runs` (a `StartRunRequest`) → `{run}`:
//!   starts a run of one of the surface's kinds, as the token's person.
//! - `GET /api/bot/surfaces` → `{surfaces}`: the registrations the daemon
//!   holds now, with their joins, for an operator and `clax doctor`.
//! - `PUT /api/bot/surfaces/<name>/joins`: [`super::bot_joins`].
//!
//! Who may: the daemon's token and nothing else
//! ([`clax_http::bot_surfaces::DaemonToken`]). The router mounts them on a
//! native daemon that is not hosted, and nowhere else.
//!
//! A refusal is `{"error": {"code", "message", …}}` under the status its
//! code has ([`clax_http::bot_surfaces::status`]). On every route but the
//! `GET`, a body that is not the message is 400 `invalid_request`, and
//! one longer than the hub's `request_bytes` 413 `too_large`. Besides:
//!
//! - `PUT …/<name>`: 400 `unsupported_version` (with `supported`, the
//!   versions this daemon speaks, whatever else the message holds),
//!   `invalid_definition` (with `errors`, and `unread` for a manifest),
//!   `name_mismatch`, `invalid_settings`, `invalid_prompt` and
//!   `prompt_too_large`; 404 `unknown_registration` (a `registration`
//!   this daemon did not give one of this surface); 429
//!   `too_many_registrations` and `too_many_changes`; 500
//!   `internal_error` (no ID could be given) and `storage_failed`.
//! - `POST …/serve`: 400 `too_many_answers`, 404 `unknown_registration`,
//!   500 `storage_failed`.
//! - `POST …/runs`: 400 `invalid_request` (also a kind the surface does
//!   not have, a request not of its type or holding content, an `allow`
//!   that is not allow patterns), 404 `unknown_registration`, 429
//!   `too_many_runs`, 501 `not_yet` (`bind`), 500 `internal_error`. A
//!   run that is refused for its person (`no_flag`, `credits_exhausted`,
//!   `not_configured`, `model_not_priced`) is answered as a thread run
//!   is: 403 with that code.

use super::artifacts::path;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use clax_bot::bot_surfaces::{CODE_TOO_MANY_RUNS, allow_patterns};
use clax_core::CoreError;
use clax_http::agent::DeniedToGrants;
use clax_http::audit::DeferredAudit;
use clax_http::bot_surfaces::{self as hub, DaemonToken, Registrant, refusal, refused};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::bot_runs::{Bindings, NewRunOf, StoredBinding};
use clax_surface::RunScope;
use clax_surface::wire::{
    CODE_INTERNAL_ERROR, CODE_INVALID_REQUEST, CODE_NOT_YET, CODE_TOO_LARGE, RegisterRequest,
    Registered, ServeRequest, ServeResponse, StartRunRequest, StartRunResponse,
};
use serde_json::{Value, json};

/// The message in `body`, read as JSON within `limit` bytes.
pub(crate) fn message<T: serde::de::DeserializeOwned>(
    body: Result<Bytes, BytesRejection>,
    limit: usize,
) -> Result<T, ApiError> {
    parsed(&body_within(body, limit)?)
}

/// The bytes of `body`, when they are `limit` bytes at the most.
fn body_within(body: Result<Bytes, BytesRejection>, limit: usize) -> Result<Bytes, ApiError> {
    match body {
        Ok(bytes) if bytes.len() <= limit => Ok(bytes),
        Ok(_) => Err(too_large(limit)),
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => Err(too_large(limit)),
        Err(e) => Err(refused(
            CODE_INVALID_REQUEST,
            format!("the request's body could not be read: {}", e.body_text()),
        )),
    }
}

/// `bytes` read as the message `T`.
fn parsed<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes)
        .map_err(|e| refused(CODE_INVALID_REQUEST, format!("the request's body: {e}")))
}

/// A registration's `clax_surface`, read leniently: whatever else the
/// object holds, and whatever a later version may put there.
#[derive(serde::Deserialize)]
struct Version {
    clax_surface: Option<Value>,
}

/// The refusal of a registration of a version this daemon does not
/// speak, read before the rest of its message, which a later version may
/// write otherwise: `None` when it is of this version, or when its
/// version is not a number (the message is then refused as the rest of
/// it is read).
fn other_version(bytes: &[u8]) -> Option<ApiError> {
    let asked = serde_json::from_slice::<Version>(bytes)
        .ok()?
        .clax_surface?
        .as_u64()?;
    (asked != clax_surface::wire::VERSION)
        .then(|| refusal(clax_surface::wire::ErrorBody::unsupported_version(asked)))
}

fn too_large(limit: usize) -> ApiError {
    refused(
        CODE_TOO_LARGE,
        format!("a request's body is {limit} bytes at the most"),
    )
}

/// `PUT /api/bot/surfaces/<name>` (see the module documentation).
pub async fn register(
    State(s): State<AppState>,
    token: DaemonToken,
    _d: DeniedToGrants,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Registered>, ApiError> {
    let name = path(p)?;
    let surfaces = s.bot_surfaces.clone();
    let bytes = body_within(body, surfaces.limits().request_bytes)?;
    if let Some(refused) = other_version(&bytes) {
        return Err(refused);
    }
    let request: RegisterRequest = parsed(&bytes)?;
    let fresh = hub::fresh_id();
    let person = token.person();
    let registered = s
        .store_call(move |st| {
            let registrant = Registrant {
                ctx: audit.resolve(st)?,
                person: person.to_string(),
            };
            Ok(surfaces.register(registrant, &name, &request, &fresh))
        })
        .await?
        .map_err(|e| hub::register_refusal(&e))?;
    Ok(Json(registered))
}

/// `POST /api/bot/surfaces/<name>/serve` (see the module documentation).
pub async fn serve(
    State(s): State<AppState>,
    _token: DaemonToken,
    _d: DeniedToGrants,
    p: Result<Path<String>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ServeResponse>, ApiError> {
    let name = path(p)?;
    let request: ServeRequest = message(body, s.bot_surfaces.limits().request_bytes)?;
    hub::serve(&s, name, request)
        .await
        .map(Json)
        .map_err(|e| hub::serve_refusal(&e))
}

/// What a run of a kind is made with, read from its registration.
struct Plan {
    /// The hash of the registration's definition.
    hash: String,
    /// The registration's prompt file, when it has one: its SHA-256 and
    /// its text, of the same copy of the registration as `hash`.
    prompt: Option<(String, String)>,
    /// The operations the run has, in the definition's order.
    granted: Vec<String>,
    /// What the run's binding keeps of the registration's settings, of
    /// the same copy of it ([`clax_bot::outside::kept_with_run`]).
    settings: Value,
}

/// `POST /api/bot/surfaces/<name>/runs` (see the module documentation):
/// the kind is one of the registration's own definition, the request is
/// checked against it and holds no content (none is stored yet), and the
/// run has the operations the kind's `@allow`, the registration's `allow`
/// setting and the request's `allow` all name, fixed in its bindings
/// (version 2: the surface, its definition's hash, the registration, the
/// hash of its prompt file when it has one, its settings but `allow`, no
/// scope). The definition, the prompt file and the settings are the
/// registration's as one copy of it has them, so the run keeps all three
/// as it was started with them, whatever its host registers later and
/// over a restart, and the prompt file is one that was checked
/// against that definition; the prompt file's text is kept under its hash
/// again before the run is written, so nothing drops it in between. It
/// has no artifact and no binding of Clax's own surface. A
/// request that names other surfaces to bind (`bind`) is 501 `not_yet`.
/// The request is checked against a copy of the registration, with the
/// daemon's broker let go: a host whose requests are large to check
/// holds up no other host's `serve` and no run's call. A registration
/// may have [`clax_bot::bot_surfaces::HubLimits::runs_per_registration`]
/// runs that have not ended, counted in the transaction that makes the
/// run ([`clax_store::Store::create_run_of_within`]), so starts made at
/// once never pass the limit together; one more is 429 `too_many_runs`.
/// Every refusal comes before the run is written; a start refused there
/// because another took the last place has given the registration's
/// prompt file again (kept under its hash, as every start does).
pub async fn start(
    State(s): State<AppState>,
    token: DaemonToken,
    _d: DeniedToGrants,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<StartRunResponse>, ApiError> {
    let name = path(p)?;
    let surfaces = s.bot_surfaces.clone();
    let limits = surfaces.limits();
    let b: StartRunRequest = message(body, limits.request_bytes)?;
    if b.bind.is_some() {
        return Err(refused(
            CODE_NOT_YET,
            "`bind` is not available yet: a run has its own surface's operations and no other's",
        ));
    }
    let (bot, person) = (s.bot.clone(), token.person());
    let run = s
        .store_call(move |st| {
            let unknown =
                || refusal(clax_bot::bot_surfaces::ServeError::UnknownRegistration.body());
            if surfaces.find(&name, &b.registration).is_err() {
                return Ok(Err(unknown()));
            }
            let planned = surfaces.snapshot(&b.registration).map(|reg| {
                let invalid = |m: String| refused(CODE_INVALID_REQUEST, m);
                let def = &*reg.definition;
                let kind = def.kind(&b.kind).ok_or_else(|| {
                    invalid(format!(
                        "the surface `{}` has no kind {:?}: a run is of one of its own kinds, \
                         named without the surface",
                        reg.name, b.kind
                    ))
                })?;
                kind.check_request(def, &b.request)
                    .map_err(|e| invalid(e.message))?;
                let held = kind
                    .content_places(def, &b.request)
                    .map_err(|e| invalid(e.message))?;
                if !held.is_empty() {
                    return Err(invalid(format!(
                        "kind {}: the request holds content at {}: content reaches a run \
                         in the answers to its calls alone",
                        kind.name,
                        held.join(", ")
                    )));
                }
                let operator = reg
                    .settings
                    .allow
                    .as_deref()
                    .map(|list| allow_patterns(list, &limits.definition))
                    .transpose()
                    .map_err(|m| refused(CODE_INTERNAL_ERROR, m))?;
                let starter = b
                    .allow
                    .as_deref()
                    .map(|list| allow_patterns(list, &limits.definition))
                    .transpose()
                    .map_err(invalid)?;
                // What the hub withholds for content is the registration's
                // `withheld`: none, on a daemon that keeps content.
                let scope = RunScope {
                    operator: operator.as_deref(),
                    starter: starter.as_deref(),
                    content: true,
                };
                let ops = def
                    .run_operations(&b.kind, &scope)
                    .ok_or_else(|| refused(CODE_INTERNAL_ERROR, "the kind is gone"))?;
                let prefix = format!("{}.", reg.name);
                let held_back = |op: &str| {
                    let op = op.strip_prefix(&prefix).unwrap_or(op);
                    reg.withheld.iter().any(|w| w.op == op)
                };
                Ok(Plan {
                    hash: reg.hash.clone(),
                    prompt: reg.prompt_hash.clone().zip(reg.prompt.clone()),
                    granted: ops
                        .granted
                        .iter()
                        .filter(|op| !held_back(op))
                        .map(|op| (*op).to_string())
                        .collect(),
                    settings: clax_bot::outside::kept_with_run(&reg.settings),
                })
            });
            let plan = match planned {
                None => return Ok(Err(unknown())),
                Some(Err(e)) => return Ok(Err(e)),
                Some(Ok(plan)) => plan,
            };
            let bindings = Bindings::bound(vec![
                StoredBinding::bound(
                    name.as_str(),
                    Some(plan.hash),
                    Some(b.registration.clone()),
                    plan.prompt.as_ref().map(|(hash, _)| hash.clone()),
                    Value::Null,
                    plan.granted,
                )
                .with_settings(plan.settings),
            ])
            .map_err(|e| CoreError::invalid("run_invalid", format!("{e:?}")))?;
            let too_many = |message: String| refused(CODE_TOO_MANY_RUNS, message);
            // Read first, so a start that is plainly over the limit
            // writes nothing; the count that holds starts made at once to
            // the limit is the one the run's own transaction makes.
            let live = st.live_runs_started_by(&b.registration)?;
            if live >= limits.runs_per_registration {
                return Ok(Err(too_many(format!(
                    "the registration has {live} runs that have not ended, which is the \
                     limit: start another when one has ended"
                ))));
            }
            let ctx = audit.resolve(st)?;
            let (key_source, model) = super::bot::admit_run(st, &bot, &ctx, person, None)?;
            if let Some((hash, text)) = &plan.prompt {
                st.put_surface_prompt(hash, text, st.now_utc())?;
            }
            let made = st.create_run_of_within(
                &ctx,
                NewRunOf {
                    kind: format!("{name}.{}", b.kind),
                    request: b.request,
                    bindings,
                    person: person.to_string(),
                    model,
                    key_source,
                    started_by: b.registration,
                    parent_run: None,
                    agent_grant: None,
                },
                limits.runs_per_registration,
            );
            match made {
                Ok(run) => Ok(Ok(run)),
                Err(CoreError::Invalid { code, message })
                    if code == clax_store::bot_runs::CODE_TOO_MANY_RUNS =>
                {
                    Ok(Err(too_many(message)))
                }
                Err(e) => Err(e),
            }
        })
        .await??;
    s.bot.runs.started(&run.id);
    Ok(Json(StartRunResponse { run: run.id }))
}

/// `GET /api/bot/surfaces` → `{surfaces: [{name, registration, hash,
/// connected, withheld, registered_at, last_served_at, joins}]}` (`joins`
/// as `PUT …/joins` answers them, less the registration:
/// [`super::bot_joins::joins_listed`]): the
/// registrations the token's person made that the daemon holds now, the
/// oldest first. A registration is held from its registration until its
/// host has not served it for the hub's lease, and none outlasts the
/// daemon: a host goes on as its registration by registering again with
/// its ID.
pub async fn list(
    State(s): State<AppState>,
    token: DaemonToken,
    _d: DeniedToGrants,
) -> Result<Json<Value>, ApiError> {
    let surfaces = s.bot_surfaces.clone();
    let person = token.person();
    let listed = s
        .store_call(move |st| super::bot_joins::joins_listed(st, surfaces.list(person)?))
        .await?;
    Ok(Json(json!({ "surfaces": listed })))
}

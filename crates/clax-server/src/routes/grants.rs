//! The GNAP-shaped grant flow (spec §4.2, §6.2–§6.4): invites, grant
//! requests, continuation, token revocation, and the owner's decisions.
//!
//! The client's calls (`POST /api/grants`, `POST|DELETE …/continue`,
//! `DELETE …/token`) are signed with the client's key and verified before
//! anything else ([`crate::grant_auth`]). The owner's changes (invites,
//! approving, denying and revoking grants) need a browser of the owner's
//! on a same-origin request (M20): the daemon token alone is 403
//! `browser_required`. Every change is announced on the owner-only `people`
//! stream topic; no code or token ever appears in an event, a log line or
//! an error.

use super::artifacts::{body, path};
use crate::audit::DeferredAudit;
use crate::auth::Caller;
use crate::error::ApiError;
use crate::grant_auth::{self, Refusal, Signed};
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use crate::views::grant::grant_view;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::rights::Rights;
use clax_core::sig;
use clax_core::store::people::{
    CONTINUE_WAIT_SECS, ContinueOutcome, CredentialKind, EndedBy, Grant, GrantStart, Invite,
    InvitePerson, NewGrant, RevokedGrant,
};
use clax_core::{CoreError, Event, Secret, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 403 `browser_required` unless a browser of the owner's makes the request
/// (M20): [`Caller::browser`] for the owner.
fn require_browser(caller: &Caller, what: &str) -> Result<(), ApiError> {
    if caller.browser() && caller.is_owner() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "browser_required",
            format!("{what} needs the owner's browser: open People in Clax"),
        ))
    }
}

fn invite_view(i: &Invite) -> Value {
    json!({
        "id": i.id,
        "person": {"public_id": i.person, "display_name": i.person_name},
        "owner": i.owner,
        "created_at": i.created_at,
        "expires_at": i.expires_at,
        "used_at": i.used_at,
        "grant_id": i.grant_id,
        "revoked_at": i.revoked_at,
    })
}

pub(crate) fn announce(s: &AppState, change: Value) {
    s.events.publish(Event::People { change });
}

/// Announces what ending grants ended, as a session's end is announced:
/// released feedback, withdrawn questions, the sessions' waiters, working
/// records and followers; then `grant_ended` on the `people` topic. The
/// grant's tokens are marked ended in the index first. A grant that had
/// ended already announces nothing. `ctx` is `s`'s, made on the runtime
/// ([`AppState::feedback_ctx`]).
pub fn announce_ended(
    s: &AppState,
    ctx: &crate::feedback::FeedbackCtx,
    st: &Store,
    ended: &[RevokedGrant],
    by: &str,
) {
    for r in ended.iter().filter(|r| r.ended) {
        grant_auth::note_stale(s.grant_auth.index.ended(&r.grant_id), "grant ended");
        crate::state::sessions_ended(
            s,
            ctx,
            st,
            &clax_core::audit::AuditCtx::DAEMON,
            &r.touched,
            &r.withdrawn_questions,
            &r.sessions_ended,
        );
        let grant = st.grant(&r.grant_id).ok().map(|g| grant_view(&g));
        announce(
            s,
            json!({"what": "grant_ended", "by": by, "grant_id": r.grant_id, "grant": grant,
                   "sessions_ended": r.sessions_ended, "withdrawn_questions": r.withdrawn_questions}),
        );
    }
}

/// Expires what is due ([`Store::expire_grants`]) and announces it,
/// rebuilds the token index when a change to it failed, and sweeps both
/// nonce caches. The daemon runs it on start and every minute.
pub async fn sweep(s: &AppState) -> Result<usize, ApiError> {
    s.grant_auth.sweep(s.store.now_utc().timestamp());
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    s.store_call(move |st| {
        let ended = st.expire_grants()?;
        announce_ended(&state, &ctx, st, &ended, "expiry");
        // Requests purged since leave the index's record of ended grants.
        crate::grant_auth::forget_purged(&state.grant_auth.index, st);
        if state.grant_auth.index.rebuild_if_stale(st)? {
            tracing::info!("grant index rebuilt");
        }
        Ok(ended.len())
    })
    .await
}

/// `GET /api/_test/grants/index` (debug builds): the token index's size
/// and state, and the nonce caches' keys.
#[cfg(debug_assertions)]
pub async fn test_index(
    State(s): State<AppState>,
    _d: crate::agent::DeniedToGrants,
    _t: crate::auth::RequireToken,
) -> Json<Value> {
    let (requests, keys) = s.grant_auth.cached_keys();
    let index = &s.grant_auth.index;
    Json(json!({"tokens": index.len(), "stale": index.is_stale(),
                "request_keys": requests, "keys": keys}))
}

/// `POST /api/_test/grants/index/fail` (debug builds): the token index's
/// next change fails, as one after the store committed might.
#[cfg(debug_assertions)]
pub async fn test_fail_index(
    State(s): State<AppState>,
    _d: crate::agent::DeniedToGrants,
    _t: crate::auth::RequireToken,
) -> StatusCode {
    s.grant_auth.index.fail_next_change();
    StatusCode::NO_CONTENT
}

/// `Cache-Control: no-store` on a response that carries a token or a code
/// (RFC 9635 §3).
fn no_store(mut res: Response) -> Response {
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    res
}

/// `POST /api/_test/grants/sweep` (debug builds): runs [`sweep`] now.
#[cfg(debug_assertions)]
pub async fn test_sweep(
    State(s): State<AppState>,
    _d: crate::agent::DeniedToGrants,
    _t: crate::auth::RequireToken,
) -> Result<Json<Value>, ApiError> {
    let ended = sweep(&s).await?;
    let (requests, keys) = s.grant_auth.cached_keys();
    Ok(Json(
        json!({"ended": ended, "request_keys": requests, "keys": keys}),
    ))
}

// ---------------------------------------------------------------- invites

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    viewer: Option<String>,
    #[serde(default)]
    me: bool,
    #[serde(default)]
    rights: Option<Value>,
}

/// The address a LAN machine reaches this daemon at, or why there is none:
/// the bound address, or for an unspecified bind the address of the
/// interface the default route leaves by (found with a connected UDP
/// socket, which sends nothing), read at each invite since it can change.
fn lan_address(s: &AppState) -> Result<IpAddr, ApiError> {
    let not_on_lan = |msg: &str| ApiError::new(StatusCode::CONFLICT, "not_on_lan", msg.to_string());
    if s.bind.is_loopback() {
        return Err(not_on_lan(
            "Clax listens on this machine only: restart it with `clax serve --bind 0.0.0.0` to invite people",
        ));
    }
    if let Some(ip) = s.lan_host {
        return Ok(ip);
    }
    if !s.bind.is_unspecified() {
        return Ok(s.bind);
    }
    let found = |local: IpAddr, out: IpAddr| -> Option<IpAddr> {
        let sock = std::net::UdpSocket::bind((local, 0)).ok()?;
        sock.connect((out, 9)).ok()?;
        let ip = sock.local_addr().ok()?.ip();
        (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
    };
    found(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
    )
    .or_else(|| {
        found(
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        )
    })
    .ok_or_else(|| not_on_lan("this machine has no LAN address"))
}

/// `POST /api/people/invites` (owner, browser) `{name | viewer | me,
/// rights?}` → 201 `{invite: {id, command, expires_at}}` (spec §6.2). The
/// command carries the code, shown this once. 409 `not_on_lan` when the
/// daemon listens on loopback only.
pub async fn create_invite(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    req: Result<Json<InviteBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    require_browser(&caller, "inviting a person")?;
    let b = body(req)?;
    let target = match (b.name, b.viewer, b.me) {
        (Some(n), None, false) => InvitePerson::New(n),
        (None, Some(v), false) => InvitePerson::Viewer(v),
        (None, None, true) => InvitePerson::Owner,
        _ => {
            return Err(ApiError::bad_request(
                "invalid_args",
                "name exactly one of name, viewer or me",
            ));
        }
    };
    let rights = b.rights.as_ref().map(Rights::parse).transpose()?;
    let host = lan_address(&s)?;
    let base = match host {
        IpAddr::V6(ip) => format!("http://[{ip}]:{}", s.port),
        IpAddr::V4(ip) => format!("http://{ip}:{}", s.port),
    };
    let state = s.clone();
    let (invite, code) = s
        .store_call(move |st| {
            let made = st.create_invite(&audit.resolve(st)?, target, rights)?;
            announce(
                &state,
                json!({"what": "invite_created", "invite": invite_view(&made.0)}),
            );
            Ok(made)
        })
        .await?;
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(json!({"invite": {
                "id": invite.id,
                "command": format!("clax connect {base}/i/{}", code.expose()),
                "expires_at": invite.expires_at,
                "person": {"public_id": invite.person, "display_name": invite.person_name},
            }})),
        )
            .into_response(),
    ))
}

/// `GET /api/people/invites` (owner) → `{invites}`, newest first, without
/// their codes.
pub async fn list_invites(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    who.require_owner("sees invites")?;
    let invites = s.store_call(|st| st.list_invites()).await?;
    Ok(Json(
        json!({"invites": invites.iter().map(invite_view).collect::<Vec<_>>()}),
    ))
}

/// `DELETE /api/people/invites/<id>` (owner, browser) → `{invite}`; an
/// invite used or revoked already stays as it was.
pub async fn revoke_invite(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    require_browser(&caller, "revoking an invite")?;
    let id = path(id)?;
    let state = s.clone();
    let invite = s
        .store_call(move |st| {
            let i = st.revoke_invite(&audit.resolve(st)?, &id)?;
            announce(
                &state,
                json!({"what": "invite_revoked", "invite": invite_view(&i)}),
            );
            Ok(i)
        })
        .await?;
    Ok(Json(json!({"invite": invite_view(&invite)})))
}

// --------------------------------------------------------- grant requests

#[derive(Deserialize)]
struct GrantRequest {
    #[serde(default)]
    access_token: Option<AccessTokenRequest>,
    client: ClientRequest,
    #[serde(default)]
    interact: Option<InteractRequest>,
    #[serde(default)]
    clax: Option<ClaxRequest>,
}

#[derive(Deserialize)]
struct AccessTokenRequest {
    #[serde(default)]
    access: Option<Value>,
    #[serde(default)]
    flags: Vec<String>,
}

#[derive(Deserialize)]
struct ClientRequest {
    key: KeyRequest,
    #[serde(default)]
    display: Option<DisplayRequest>,
}

#[derive(Deserialize)]
struct KeyRequest {
    proof: String,
    jwk: Jwk,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    crv: String,
    x: String,
}

#[derive(Deserialize)]
struct DisplayRequest {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct InteractRequest {
    #[serde(default)]
    start: Vec<String>,
}

#[derive(Deserialize)]
struct ClaxRequest {
    #[serde(default)]
    invite: Option<String>,
}

fn invalid_args(msg: &str) -> ApiError {
    ApiError::bad_request("invalid_args", msg.to_string())
}

/// The grant request in `body` and its client's key.
fn parse_request(body: &[u8]) -> Result<(GrantRequest, [u8; 32]), ApiError> {
    let r: GrantRequest = serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?;
    let k = &r.client.key;
    if k.proof != "clax-ed25519" || k.jwk.kty != "OKP" || k.jwk.crv != "Ed25519" {
        return Err(invalid_args(
            "client.key is {proof: \"clax-ed25519\", jwk: {kty: \"OKP\", crv: \"Ed25519\", x}}",
        ));
    }
    let key = sig::decode_public(&k.jwk.x)
        .map_err(|_| invalid_args("client.key.jwk.x is a 32-byte key in unpadded base64url"))?;
    Ok((r, key))
}

/// The browser link for `code`, on the host the client reached.
fn browser_link(host: &str, code: Option<&Secret>) -> Option<String> {
    code.map(|c| format!("http://{host}/connect/b/{}", c.expose()))
}

/// A grant response with issued tokens: the access token, its manage URI
/// and token, the access the grant holds, the continuation, the subject,
/// the person, whether it is the owner's own machine, the owner's name
/// (`clax connect` asks about the owner by it, M15) and the browser link.
/// Builds from values only, so nothing fails once the store has issued the
/// tokens.
fn issued_response(
    grant: &Grant,
    access: &Rights,
    token: &Secret,
    manage: &Secret,
    continue_token: &Secret,
    browser: Option<String>,
    owner_name: Option<String>,
) -> Value {
    let person = grant.person.clone().unwrap_or_default();
    let mut clax = json!({"person": {"public_id": person, "display_name": grant.person_name},
                          "owner": {"display_name": owner_name}, "own_machine": grant.owner});
    if let Some(link) = browser {
        clax["browser_link"] = json!(link);
    }
    json!({
        "access_token": {
            "value": token.expose(),
            "access": access.to_json(),
            "manage": {
                "uri": format!("/api/grants/{}/token", grant.id),
                "access_token": {"value": manage.expose()},
            },
        },
        "continue": {
            "access_token": {"value": continue_token.expose()},
            "uri": format!("/api/grants/{}/continue", grant.id),
        },
        "subject": {"sub_ids": [{"format": "opaque", "id": person}]},
        "clax": clax,
    })
}

/// The owner's name, as they set it; `None` when unset or unreadable (a
/// response that issues tokens never fails over it).
fn owner_name(st: &Store) -> Option<String> {
    st.owner().ok().flatten().and_then(|o| o.display_name)
}

/// The body a continuation call may carry: `{"clax": {"approve_owner":
/// true}}`, `clax connect`'s yes to "Let <owner>'s comments and pages reach
/// your agents everywhere?" (M15).
#[derive(Deserialize, Default)]
struct ContinueBody {
    #[serde(default)]
    clax: Option<ContinueClax>,
}

#[derive(Deserialize, Default)]
struct ContinueClax {
    #[serde(default)]
    approve_owner: bool,
}

/// `POST /api/grants` (signed with the presented key, no token): a GNAP
/// grant request (spec §6.3). With `clax.invite`, 200 with the access
/// token, `continue` (for deletion), `subject` and `clax.browser_link`
/// (none for the owner's own machine); without, 201 with `continue
/// {access_token, uri, wait}` and `interact.user_code`, the request waiting
/// for the owner.
pub async fn request(
    State(s): State<AppState>,
    _g: crate::agent::GrantFlow,
    audit: DeferredAudit,
    req: axum::extract::Request,
) -> Result<Response, Refusal> {
    let (
        Signed {
            host, key, peer_ip, ..
        },
        r,
    ) = grant_auth::grant_request(&s, req, parse_request).await?;
    let flags = r
        .access_token
        .as_ref()
        .map(|a| a.flags.as_slice())
        .unwrap_or_default();
    if !flags.is_empty() {
        return Err(invalid_args("no token flags are offered: Clax tokens are key-bound").into());
    }
    let access = match r.access_token.and_then(|a| a.access) {
        Some(v) => Rights::parse(&v).map_err(ApiError::from)?,
        None => Rights::full(),
    };
    let invite = r.clax.and_then(|c| c.invite).map(Secret::new);
    if invite.is_none()
        && !r
            .interact
            .as_ref()
            .is_some_and(|i| i.start.iter().any(|m| m == "user_code"))
    {
        return Err(invalid_args(
            "without an invite, ask for interact.start [\"user_code\"]: the owner approves the request",
        )
        .into());
    }
    let g = NewGrant {
        key,
        client_name: r.client.display.and_then(|d| d.name).unwrap_or_default(),
        peer_ip,
        access,
        invite,
    };
    let state = s.clone();
    let (status, v) = s
        .store_call(move |st| match st.request_grant(&audit.grant_flow(), g)? {
            GrantStart::Approved {
                grant,
                access,
                token,
                manage,
                continue_token,
                browser_code,
            } => {
                grant_auth::note_stale(
                    state
                        .grant_auth
                        .index
                        .issued(&grant.id, key, &token, &manage, &continue_token),
                    "tokens issued",
                );
                let link = browser_link(&host, browser_code.as_ref());
                let owner = owner_name(st);
                let v = issued_response(
                    &grant,
                    &access,
                    &token,
                    &manage,
                    &continue_token,
                    link,
                    owner,
                );
                announce(
                    &state,
                    json!({"what": "grant_connected", "grant": grant_view(&grant)}),
                );
                Ok((StatusCode::OK, v))
            }
            GrantStart::Pending {
                grant,
                continue_token,
                user_code,
            } => {
                announce(
                    &state,
                    json!({"what": "grant_requested", "grant": grant_view(&grant)}),
                );
                Ok((
                    StatusCode::CREATED,
                    json!({
                        "continue": {
                            "access_token": {"value": continue_token.expose()},
                            "uri": format!("/api/grants/{}/continue", grant.id),
                            "wait": CONTINUE_WAIT_SECS,
                        },
                        "interact": {"user_code": user_code.expose()},
                    }),
                ))
            }
        })
        .await?;
    Ok(no_store((status, Json(v)).into_response()))
}

/// `POST /api/grants/<gid>/continue` (signed, `GNAP` continuation token):
/// the client's poll. Pending: 200 with `continue` (400 `too_fast` sooner
/// than `wait` after the last). Approved: 200 with the access token,
/// `subject`, `clax.browser_link` and a new continuation token, once.
/// Denied or expired: 403 `denied`.
///
/// With the body `{"clax": {"approve_owner": true}}`, on a grant whose
/// tokens were issued, the call is instead `clax connect`'s answer to its
/// question about the owner (spec §3.1, M15): the person approves the
/// owner's content everywhere, and the answer is 200 `{"clax":
/// {"owner_approval": {id, scope} | null}}` (null for the owner's own
/// machine); 409 `not_issued` for a grant without live tokens, 409
/// `answer_closed` past ten minutes of the tokens' issue, 409
/// `owner_answered` once the person has made or revoked any approval of
/// the owner (it is answered once, at connect, §21 item 3).
pub async fn poll(
    State(s): State<AppState>,
    _g: crate::agent::GrantFlow,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
    req: axum::extract::Request,
) -> Result<Response, Refusal> {
    let gid = path(gid)?;
    let Signed {
        host,
        token,
        key,
        t,
        nonce,
        body,
        ..
    } = grant_auth::gnap(&s, req, &gid, CredentialKind::Continue).await?;
    let token = token.expect("a continuation call carries its token");
    let asked: ContinueBody = if body.is_empty() {
        ContinueBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    if asked.clax.is_some_and(|c| c.approve_owner) {
        let ctx = s.feedback_ctx();
        let v = s
            .store_call(move |st| {
                let made = st.approve_owner_on_connect(
                    &audit.grant_flow(),
                    &gid,
                    token.expose(),
                    t,
                    &nonce,
                )?;
                Ok(match made {
                    Some((a, touched)) => {
                        crate::feedback::apply(&ctx, st, &touched);
                        json!({"clax": {"owner_approval": {"id": a.id, "scope": a.scope}}})
                    }
                    None => json!({"clax": {"owner_approval": null}}),
                })
            })
            .await?;
        return Ok(Json(v).into_response());
    }
    let state = s.clone();
    let v = s
        .store_call(move |st| {
            match st.continue_grant(&audit.grant_flow(), &gid, token.expose(), t, &nonce)? {
                ContinueOutcome::Pending { grant } => Ok(json!({
                    "continue": {
                        "access_token": {"value": token.expose()},
                        "uri": format!("/api/grants/{}/continue", grant.id),
                        "wait": CONTINUE_WAIT_SECS,
                    },
                })),
                ContinueOutcome::Approved {
                    grant,
                    access,
                    token: issued,
                    manage,
                    continue_token,
                    browser_code,
                } => {
                    grant_auth::note_stale(
                        state.grant_auth.index.issued(
                            &grant.id,
                            key,
                            &issued,
                            &manage,
                            &continue_token,
                        ),
                        "tokens issued",
                    );
                    let link = browser_link(&host, browser_code.as_ref());
                    let owner = owner_name(st);
                    let v = issued_response(
                        &grant,
                        &access,
                        &issued,
                        &manage,
                        &continue_token,
                        link,
                        owner,
                    );
                    announce(
                        &state,
                        json!({"what": "grant_connected", "grant": grant_view(&grant)}),
                    );
                    Ok(v)
                }
            }
        })
        .await?;
    Ok(no_store(Json(v).into_response()))
}

/// `DELETE /api/grants/<gid>/continue` (signed, `GNAP` continuation
/// token): the person disconnects the machine (`clax connect --off`). The
/// grant ends with its tokens and sessions, as a revocation. 204.
pub async fn disconnect(
    State(s): State<AppState>,
    _g: crate::agent::GrantFlow,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
    req: axum::extract::Request,
) -> Result<Response, Refusal> {
    let gid = path(gid)?;
    let Signed {
        token, t, nonce, ..
    } = grant_auth::gnap(&s, req, &gid, CredentialKind::Continue).await?;
    let token = token.expect("a continuation call carries its token");
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    s.store_call(move |st| {
        let ended = st.disconnect_grant(&audit.grant_flow(), &gid, token.expose(), t, &nonce)?;
        announce_ended(&state, &ctx, st, &[ended], "person");
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /api/grants/<gid>/token` (signed, `GNAP` manage token): revokes
/// the access token and with it the whole grant (spec §6.4). 204.
pub async fn revoke_token(
    State(s): State<AppState>,
    _g: crate::agent::GrantFlow,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
    req: axum::extract::Request,
) -> Result<Response, Refusal> {
    let gid = path(gid)?;
    let Signed { token, .. } = grant_auth::gnap(&s, req, &gid, CredentialKind::Manage).await?;
    let token = token.expect("a manage call carries its token");
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    s.store_call(move |st| {
        let ended = st.revoke_token(&audit.grant_flow(), &gid, token.expose())?;
        announce_ended(&state, &ctx, st, &[ended], "person");
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ------------------------------------------------------- owner decisions

#[derive(Deserialize)]
pub struct ListGrantsQuery {
    #[serde(default)]
    status: Option<String>,
}

/// `GET /api/grants?status=` (owner) → `{grants}`, newest first, of one
/// status when given.
pub async fn list(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    who: Identity,
    q: Result<Query<ListGrantsQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    who.require_owner("sees grants")?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let grants = s.store_call(|st| st.list_grants()).await?;
    let grants: Vec<Value> = grants
        .iter()
        .filter(|g| q.status.as_deref().is_none_or(|w| g.status == w))
        .map(grant_view)
        .collect();
    Ok(Json(json!({"grants": grants})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveBody {
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    rights: Option<Value>,
    /// Refused: a request only makes a new person.
    #[serde(default)]
    viewer: Option<Value>,
    /// Refused: a request only makes a new person.
    #[serde(default)]
    me: Option<Value>,
}

/// `POST /api/grants/<gid>/approve` (owner, browser) `{user_code, name,
/// rights?}` → `{grant}`: the request becomes a new person named `name`
/// (spec §6.3). 400 `wrong_code` for another code (the third denies the
/// request); 400 `invalid_args` for a body naming an existing viewer or the
/// owner, which take an invite; 409 `name_taken`; 409 `not_pending`.
pub async fn approve(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
    req: Result<Json<ApproveBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    require_browser(&caller, "approving a connection")?;
    let gid = path(gid)?;
    let b = body(req)?;
    if b.viewer.is_some() || b.me.is_some() {
        return Err(invalid_args(
            "a request only makes a new person: to connect a machine of an existing person or your own, make an invite",
        ));
    }
    let (Some(code), Some(name)) = (b.user_code, b.name) else {
        return Err(invalid_args("send the request's user_code and a name"));
    };
    let rights = b.rights.as_ref().map(Rights::parse).transpose()?;
    let state = s.clone();
    let grant = s
        .store_call(move |st| {
            match st.approve_grant(&audit.resolve(st)?, &gid, &code, &name, rights) {
                Ok(g) => {
                    announce(
                        &state,
                        json!({"what": "grant_approved", "grant": grant_view(&g)}),
                    );
                    Ok(g)
                }
                Err(
                    e @ CoreError::Invalid {
                        code: "wrong_code", ..
                    },
                ) => {
                    if let Ok(g) = st.grant(&gid)
                        && g.status == "denied"
                    {
                        announce(
                            &state,
                            json!({"what": "grant_denied", "grant": grant_view(&g)}),
                        );
                    }
                    Err(e)
                }
                Err(e) => Err(e),
            }
        })
        .await?;
    Ok(Json(json!({"grant": grant_view(&grant)})))
}

/// `POST /api/grants/<gid>/deny` (owner, browser) → `{grant}`; 409
/// `not_pending` unless it is pending.
pub async fn deny(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    require_browser(&caller, "denying a connection")?;
    let gid = path(gid)?;
    let state = s.clone();
    let grant = s
        .store_call(move |st| {
            let g = st.deny_grant(&audit.resolve(st)?, &gid)?;
            announce(
                &state,
                json!({"what": "grant_denied", "grant": grant_view(&g)}),
            );
            Ok(g)
        })
        .await?;
    Ok(Json(json!({"grant": grant_view(&grant)})))
}

/// `DELETE /api/grants/<gid>` (owner, browser) → `{grant}`: ends the grant
/// with its tokens and live sessions (spec §6.4). A grant that has ended
/// already stays as it was.
pub async fn revoke(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    audit: DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    require_browser(&caller, "revoking a connection")?;
    let gid = path(gid)?;
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    let grant = s
        .store_call(move |st| {
            let ended = st.revoke_grant(&audit.resolve(st)?, &gid, EndedBy::Owner)?;
            announce_ended(&state, &ctx, st, &[ended], "owner");
            st.grant(&gid)
        })
        .await?;
    Ok(Json(json!({"grant": grant_view(&grant)})))
}

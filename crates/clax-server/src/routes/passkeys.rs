//! The owner's passkeys for browser confirmations ([`clax_http::confirm`],
//! [`clax_store::passkeys`]): opt-in user verification (Touch ID, or any
//! passkey) on every confirmation.
//!
//! - `GET /api/passkeys`: whether passkeys can be used here, whether they
//!   are required, whether the first-use offer was put off, and the
//!   registered ones.
//! - `POST /api/passkeys/challenge {purpose, confirmation_id?,
//!   credential_id?}`: a challenge for one ceremony.
//! - `POST /api/passkeys`: registers one (a confirmed decision while an
//!   agent is live).
//! - `PUT /api/passkeys/settings {required?, offer_dismissed?,
//!   assertion?}`: turning the requirement off takes an assertion.
//! - `POST /api/passkeys/<credential_id>/remove {assertion?}`: removing one
//!   while required takes an assertion.
//!
//! Each is the owner's browser's alone (an owner browser session or the
//! in-process app's frame, `SameOrigin`): 403 `browser_required`
//! otherwise, `not_for_agents` for the owner's agents. A hosted daemon has
//! none (404).

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use clax_core::webauthn::{Expect, challenge_of, from_b64url, verify_registration};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::Caller;
use clax_http::confirm::{self, Assertion, Confirming, Decision, passkey_error, passkey_rp};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::passkeys::ConfirmPrefs;
use serde::Deserialize;
use serde_json::{Value, json};

fn local_only(s: &AppState) -> Result<(), ApiError> {
    if s.hosted.is_some() {
        Err(ApiError::not_found())
    } else {
        Ok(())
    }
}

/// 403 `browser_required` unless the owner's browser makes the request.
fn browser_only(caller: &Caller, by: &Confirming) -> Result<(), ApiError> {
    if by.may_answer(caller) {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "browser_required",
            "passkeys are set up in the owner's browser: open Settings in Clax",
        ))
    }
}

fn body<T>(req: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    req.map(|Json(b)| b)
        .map_err(|e| ApiError::bad_request("invalid_json", e.body_text()))
}

/// The passkey settings as the routes show them.
fn view(s: &AppState, st: &clax_store::Store) -> clax_core::Result<Value> {
    let prefs = st.confirm_prefs()?;
    let keys = st.passkeys()?;
    let rp = passkey_rp(s);
    Ok(json!({
        "available": rp.is_some(),
        "unavailable_reason": rp.is_none().then_some(
            "Passkeys need a secure address: open Clax at http://localhost to use one"
        ),
        "rp_id": rp.as_ref().map(|(r, _)| r),
        "origin": rp.as_ref().map(|(_, o)| o),
        "require_passkey": prefs.require_passkey,
        "required": prefs.require_passkey && !keys.is_empty(),
        "offer_dismissed": prefs.offer_dismissed,
        "passkeys": keys,
    }))
}

/// `GET /api/passkeys` (the owner) → `{available, unavailable_reason,
/// rp_id, origin, require_passkey, required, offer_dismissed, passkeys:
/// [{credential_id, alg, created_at}]}`: `available` is false where the
/// daemon's browser address is no secure context with a valid RP ID (a LAN
/// address), with the reason; `required` is whether confirming takes an
/// assertion now.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reads the passkeys")?;
    let st = s.clone();
    Ok(Json(s.store_call(move |db| view(&st, db)).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeBody {
    /// `register`, `confirm`, `disable` or `remove`.
    purpose: String,
    #[serde(default)]
    confirmation_id: Option<String>,
    #[serde(default)]
    credential_id: Option<String>,
}

/// `POST /api/passkeys/challenge {purpose, confirmation_id?,
/// credential_id?}` (the owner's browser) → `{challenge, rp_id, user_id,
/// timeout_ms, allow}`: a challenge (base64url, single use, ten minutes)
/// bound to `register`, `confirm` (with `confirmation_id`), `disable` or
/// `remove` (with `credential_id`); `allow` names the registered passkeys
/// for an assertion. 400 `invalid_args` for another purpose or a missing
/// ID; 403 `passkey_unavailable` where passkeys cannot be used.
pub async fn challenge(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    by: Confirming,
    req: Result<Json<ChallengeBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    browser_only(&caller, &by)?;
    let b = body(req)?;
    let Some((rp_id, _)) = passkey_rp(&s) else {
        return Err(ApiError::forbidden(
            "passkey_unavailable",
            "passkeys need a secure address: open Clax at http://localhost to use one",
        ));
    };
    let id_ok = |v: &Option<String>| {
        v.as_deref()
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.contains(':'))
            .map(str::to_string)
    };
    let purpose = match (
        b.purpose.as_str(),
        id_ok(&b.confirmation_id),
        id_ok(&b.credential_id),
    ) {
        ("register", _, _) => "register".to_string(),
        ("disable", _, _) => "disable".to_string(),
        ("confirm", Some(c), _) => format!("confirm:{c}"),
        ("remove", _, Some(k)) => format!("remove:{k}"),
        _ => {
            return Err(ApiError::bad_request(
                "invalid_args",
                "purpose is register, disable, confirm with confirmation_id, or remove with credential_id",
            ));
        }
    };
    let (challenge, allow) = s
        .store_call(move |db| {
            let allow: Vec<String> = db
                .passkeys()?
                .into_iter()
                .map(|p| p.credential_id)
                .collect();
            Ok((db.new_passkey_challenge(&purpose)?, allow))
        })
        .await?;
    Ok(Json(json!({
        "challenge": challenge,
        "rp_id": rp_id,
        // One user handle for the owner of this Clax home.
        "user_id": clax_core::webauthn::b64url(b"clax-owner"),
        "timeout_ms": clax_store::passkeys::CHALLENGE_TTL_SECS * 1000,
        "allow": allow,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterBody {
    credential_id: String,
    /// SubjectPublicKeyInfo DER, base64url (`getPublicKey()`).
    public_key: String,
    /// The COSE algorithm (`getPublicKeyAlgorithm()`): -7 or -8.
    alg: i64,
    authenticator_data: String,
    client_data_json: String,
}

/// `POST /api/passkeys {credential_id, public_key, alg,
/// authenticator_data, client_data_json}` (the owner's browser) → 201
/// `{passkey, settings}`: registers the passkey a `register` challenge was
/// answered with, user verified. While an agent is live it is a confirmed
/// decision whatever browser asks ([`Decision::RegisterPasskey`]): 202
/// `confirmation_pending`, and the same call again with the confirmation's
/// ID once it is approved. 403 `passkey_refused` for a registration that
/// does not check; 409 `passkey_exists`.
pub async fn register(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    by: Confirming,
    req: Result<Json<RegisterBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    browser_only(&caller, &by)?;
    let b = body(req)?;
    let Some((rp_id, origin)) = passkey_rp(&s) else {
        return Err(ApiError::forbidden(
            "passkey_unavailable",
            "passkeys need a secure address: open Clax at http://localhost to use one",
        ));
    };
    let decision = Decision::RegisterPasskey {
        credential_id: b.credential_id.clone(),
        body_sha256: confirm::registration_sha256(
            &b.credential_id,
            &b.public_key,
            b.alg,
            &b.authenticator_data,
            &b.client_data_json,
        ),
    };
    confirm::require_confirmation(&s, &caller, &by, decision).await?;
    let st = s.clone();
    // Who added it, shown with it from now on: from which browser, and
    // the owner's agents live then.
    let via = if by.answers_with_passkey() {
        "a browser session"
    } else {
        "the Clax app"
    };
    let v = s
        .store_call(move |db| {
            let refused = |why: &str| CoreError::invalid("passkey_refused", why.to_string());
            let decode = |v: &str| from_b64url(v).map_err(refused);
            let (spki, ad, cdj) = (
                decode(&b.public_key)?,
                decode(&b.authenticator_data)?,
                decode(&b.client_data_json)?,
            );
            let challenge = challenge_of(&cdj).map_err(refused)?;
            if !db.take_passkey_challenge(&challenge, "register")? {
                return Err(refused(
                    "the challenge is not a registration's, or was used or expired",
                ));
            }
            let e = Expect {
                origin: &origin,
                rp_id: &rp_id,
                challenge: &challenge,
            };
            let r = verify_registration(&e, &b.credential_id, &spki, b.alg, &ad, &cdj)
                .map_err(refused)?;
            let agents: Vec<String> = db
                .live_agent_sessions()?
                .iter()
                .map(|a| match &a.name {
                    Some(n) => format!("{n} ({})", a.harness),
                    None => format!("{} {}", a.harness, a.agent_handle),
                })
                .collect();
            let passkey = db.add_passkey(&r, &json!({"via": via, "agents_live": agents}))?;
            Ok(json!({"passkey": passkey, "settings": view(&st, db)?}))
        })
        .await
        .map_err(passkey_error)?;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsBody {
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    offer_dismissed: Option<bool>,
    #[serde(default)]
    assertion: Option<Assertion>,
}

/// `PUT /api/passkeys/settings {required?, offer_dismissed?, assertion?}`
/// (the owner's browser) → the settings: `required: true` needs a
/// registered passkey (400 `no_passkey`); turning it off while it is in
/// force takes `assertion` over a `disable` challenge (403
/// `passkey_required` without it); `offer_dismissed` puts the first-use
/// offer off.
pub async fn settings(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    by: Confirming,
    req: Result<Json<SettingsBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    browser_only(&caller, &by)?;
    let b = body(req)?;
    let st = s.clone();
    let with_passkey = by.answers_with_passkey();
    let v = s
        .store_call(move |db| {
            let mut p: ConfirmPrefs = db.confirm_prefs()?;
            match b.required {
                Some(true) if db.passkeys()?.is_empty() => {
                    return Err(CoreError::invalid(
                        "no_passkey",
                        "set up a passkey first: then confirming can require it",
                    ));
                }
                Some(false) if with_passkey && db.passkeys_required()? => {
                    let Some(a) = &b.assertion else {
                        return Err(CoreError::invalid(
                            "passkey_required",
                            "turning this off needs your passkey",
                        ));
                    };
                    confirm::check_assertion(&st, db, "disable", a)?;
                }
                _ => {}
            }
            if let Some(r) = b.required {
                p.require_passkey = r;
            }
            if let Some(o) = b.offer_dismissed {
                p.offer_dismissed = o;
            }
            db.set_confirm_prefs(p)?;
            view(&st, db)
        })
        .await
        .map_err(passkey_error)?;
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RemoveBody {
    #[serde(default)]
    assertion: Option<Assertion>,
}

/// `POST /api/passkeys/<credential_id>/remove {assertion?}` (the owner's
/// browser) → the settings: removes a passkey. While passkeys are required
/// it takes `assertion` over a `remove` challenge for that passkey (403
/// `passkey_required` without it). 404 for no such passkey.
pub async fn remove(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    caller: Caller,
    by: Confirming,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<RemoveBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    browser_only(&caller, &by)?;
    let id = p
        .map(|Path(id)| id)
        .map_err(|e| ApiError::bad_request("invalid_args", e.body_text()))?;
    let b = body(req)?;
    let st = s.clone();
    let with_passkey = by.answers_with_passkey();
    let v = s
        .store_call(move |db| {
            if with_passkey && db.passkeys_required()? {
                let Some(a) = &b.assertion else {
                    return Err(CoreError::invalid(
                        "passkey_required",
                        "removing a passkey needs your passkey",
                    ));
                };
                confirm::check_assertion(&st, db, &format!("remove:{id}"), a)?;
            }
            if !db.remove_passkey(&id)? {
                return Err(CoreError::NotFound);
            }
            view(&st, db)
        })
        .await
        .map_err(passkey_error)?;
    Ok(Json(v))
}

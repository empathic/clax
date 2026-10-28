//! Web Push subscriptions of the owner's devices ([`clax_http::web_push`]):
//! the daemon's public VAPID key, and storing and forgetting a device's
//! subscription. Only the owner's browser
//! ([`Identity::owner_browser`]) reaches them:
//! 403 `forbidden` for anyone else (a tailnet or LAN viewer, a connected
//! person), 403 `browser_required` for the owner's token alone, and agents
//! are refused. A local daemon's alone: a hosted one has none of these
//! routes.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use clax_core::webpush::{Message, PushError, Subscription};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_http::web_push;
use serde::Deserialize;
use serde_json::{Value, json};

/// What a device is told when it subscribes: that the next item waiting on
/// the owner will reach it.
pub const CONFIRMATION: &str = "Clax notifications are on for this device";

/// 403 `browser_required` unless the owner's browser makes the request (the
/// caller has passed `require_owner`).
fn owner_device(who: &Identity) -> Result<(), ApiError> {
    if who.owner_browser() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "browser_required",
            "push notifications are turned on from a browser: Notify me on this device in the Clax inbox",
        ))
    }
}

fn failed(e: PushError) -> ApiError {
    match e {
        PushError::Invalid(m) => ApiError::bad_request("invalid_subscription", m),
        e => {
            tracing::error!(error = %e, "push files");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "push notifications are unavailable",
            )
        }
    }
}

fn local_only(s: &AppState) -> Result<(), ApiError> {
    if s.hosted.is_some() {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// `GET /api/web-push` → `{public_key, devices}`: the VAPID public key a
/// browser subscribes with (`applicationServerKey`, base64url, made on
/// first use) and how many devices are subscribed.
pub async fn status(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    local_only(&s)?;
    who.require_owner("reads the push key")?;
    owner_device(&who)?;
    let key = web_push::vapid(&s.home).map_err(failed)?.public_key();
    let n = web_push::subscriptions(&s.home).map_err(failed)?.len();
    Ok(Json(json!({"public_key": key, "devices": n})))
}

#[derive(Deserialize)]
pub struct SubscriptionKeys {
    p256dh: String,
    auth: String,
}

/// A `PushSubscription.toJSON()`.
#[derive(Deserialize)]
pub struct SubscribeBody {
    endpoint: String,
    keys: SubscriptionKeys,
    /// `toJSON()` sends it; it is not kept.
    #[serde(default, rename = "expirationTime")]
    _expiration_time: Option<Value>,
}

/// `POST /api/web-push/subscriptions` `{endpoint, keys: {p256dh, auth}}`
/// (a `PushSubscription.toJSON()`) → 201 `{devices}`: the device is
/// subscribed (again, for an endpoint already kept), and is sent
/// [`CONFIRMATION`] at once. 400 `invalid_subscription` for an endpoint
/// that is not `https` (or `http` to a loopback host) or keys that are not
/// a P-256 key and a 16-byte secret.
pub async fn subscribe(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    body: Result<Json<SubscribeBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    local_only(&s)?;
    who.require_owner("subscribes a device to push notifications")?;
    owner_device(&who)?;
    let b = super::artifacts::body(body)?;
    let sub = Subscription::new(
        &b.endpoint,
        &b.keys.p256dh,
        &b.keys.auth,
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
    .map_err(failed)?;
    let n = web_push::subscribe(&s.home, sub.clone()).map_err(failed)?;
    let home = s.home.clone();
    clax_rt::spawn(async move {
        let msg = Message {
            title: CONFIRMATION.to_string(),
            url: "/inbox".to_string(),
            tag: "clax-push-on".to_string(),
        };
        web_push::send_one(&home, &sub, &msg).await;
    });
    Ok((StatusCode::CREATED, Json(json!({"devices": n}))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsubscribeBody {
    endpoint: String,
}

/// `DELETE /api/web-push/subscriptions` `{endpoint}` → `{removed, devices}`:
/// the device's subscription is forgotten (`removed: false` when none was
/// kept).
pub async fn unsubscribe(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    body: Result<Json<UnsubscribeBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    local_only(&s)?;
    who.require_owner("unsubscribes a device from push notifications")?;
    owner_device(&who)?;
    let b = super::artifacts::body(body)?;
    let removed = web_push::unsubscribe(&s.home, &b.endpoint).map_err(failed)?;
    let n = web_push::subscriptions(&s.home).map_err(failed)?.len();
    Ok(Json(json!({"removed": removed, "devices": n})))
}

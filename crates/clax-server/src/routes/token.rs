use axum::{
    Json,
    http::header,
    response::{IntoResponse, Response},
};
use clax_http::error::ApiError;
use clax_http::identity::Identity;

/// The bearer token, for the shell of a browser of the owner's: a request
/// carrying a live owner browser session ([`clax_http::local_trust`]) from
/// this machine, which only a redeemed login code opens, or the key cookie
/// (whose value is the token, [`clax_http::identity::Browser::Key`]). Anyone else gets
/// 401 `unauthorized`, whatever its peer, `Host`, `Origin` or
/// `Sec-Fetch-Site` say: reaching the port is no credential. It sets no
/// cookie and claims no viewer (the login route does both, `GET /login`).
/// A hosted daemon's gate answers this path itself
/// ([`clax_http::hosted::gate`]).
pub async fn token(
    axum::extract::State(s): axum::extract::State<clax_http::state::AppState>,
    _d: clax_http::agent::DeniedToGrants,
    who: Identity,
) -> Result<Response, ApiError> {
    if !who.owner_cookie && who.browser != clax_http::identity::Browser::Key {
        return Err(ApiError::unauthorized());
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"token": s.token})),
    )
        .into_response())
}

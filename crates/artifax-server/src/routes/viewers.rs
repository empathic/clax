//! `GET/PUT /api/viewers/me`: the browser viewer behind the `artifax_viewer`
//! cookie. Both refuse a request with a foreign `Origin` ([`SameOrigin`]).

use super::artifacts::body;
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie, set_cookie};
use artifax_core::new_ulid;
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameBody {
    display_name: String,
}

async fn respond(
    s: AppState,
    cookie: Option<String>,
    name: Option<String>,
) -> Result<Response, ApiError> {
    let (id, fresh) = match cookie {
        Some(id) => (id, false),
        None => (new_ulid(), true),
    };
    let id2 = id.clone();
    let viewer = s
        .store_call(move |st| st.upsert_viewer(&id2, name.as_deref()))
        .await?;
    let mut res = Json(json!({"viewer": viewer})).into_response();
    if fresh {
        res.headers_mut().insert(SET_COOKIE, set_cookie(&id));
    }
    Ok(res)
}

/// The viewer, created (and its cookie set) on first contact.
pub async fn me(
    State(s): State<AppState>,
    _o: SameOrigin,
    ViewerCookie(c): ViewerCookie,
) -> Result<Response, ApiError> {
    respond(s, c, None).await
}

/// Sets the display name; an empty name clears it.
pub async fn set_me(
    State(s): State<AppState>,
    _o: SameOrigin,
    ViewerCookie(c): ViewerCookie,
    req: Result<Json<NameBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let b = body(req)?;
    respond(s, c, Some(b.display_name)).await
}

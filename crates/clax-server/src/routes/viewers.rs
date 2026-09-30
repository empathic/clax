//! `GET/PUT /api/viewers/me`: the browser viewer behind the `clax_viewer`
//! cookie; `GET /api/viewers`: other viewers by public ID or name. All refuse
//! a request with a foreign `Origin` ([`SameOrigin`]).

use super::artifacts::body;
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie, set_cookie};
use axum::Json;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::HeaderMap;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Response};
use clax_core::new_ulid;
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

/// Most public IDs one lookup takes.
pub const MAX_LOOKUP_IDS: usize = 64;
/// Most viewers a name search returns.
pub const MAX_SEARCH: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupQuery {
    ids: Option<String>,
    q: Option<String>,
}

/// `GET /api/viewers?ids=<comma-separated public IDs>` (at most
/// [`MAX_LOOKUP_IDS`]) names the viewers a page refers to; unknown IDs are
/// left out. `GET /api/viewers?q=<text>` (token only) returns up to
/// [`MAX_SEARCH`] named viewers whose name contains the text, ignoring case.
/// Both answer `{viewers: [{id, display_name}]}` with public IDs, never
/// cookies.
pub async fn lookup(
    State(s): State<AppState>,
    _o: SameOrigin,
    headers: HeaderMap,
    q: Result<Query<LookupQuery>, QueryRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let viewers = match (q.ids, q.q) {
        (Some(ids), None) => {
            let ids: Vec<String> = ids
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if ids.len() > MAX_LOOKUP_IDS {
                return Err(ApiError::bad_request(
                    "invalid_argument",
                    format!("at most {MAX_LOOKUP_IDS} IDs per lookup"),
                ));
            }
            if ids.iter().any(|i| !clax_core::is_public_id(i)) {
                return Err(ApiError::bad_request(
                    "invalid_argument",
                    "ids takes viewer public IDs",
                ));
            }
            s.store_call(move |st| st.viewers_by_public_ids(&ids))
                .await?
        }
        (None, Some(text)) => {
            if !crate::auth::has_token(&headers, &s.token) {
                return Err(ApiError::unauthorized());
            }
            let text = text.trim().to_string();
            // No stored name is longer than MAX_NAME_CHARS, so a longer query matches none.
            if text.is_empty() || text.chars().count() > clax_core::store::viewers::MAX_NAME_CHARS {
                Vec::new()
            } else {
                s.store_call(move |st| st.search_viewers(&text, MAX_SEARCH))
                    .await?
            }
        }
        _ => {
            return Err(ApiError::bad_request(
                "invalid_argument",
                "pass exactly one of ids and q",
            ));
        }
    };
    Ok(Json(json!({
        "viewers": viewers
            .iter()
            .map(|v| json!({"id": v.public_id, "display_name": v.display_name}))
            .collect::<Vec<_>>()
    })))
}

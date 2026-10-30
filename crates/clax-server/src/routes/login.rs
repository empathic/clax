//! A local daemon's browser login ([`clax_http::local_trust`]): a holder of
//! the daemon token makes a one-use code (`POST /api/browser-login`), and
//! the browser it opens redeems it (`GET /login`), becoming a browser of
//! the owner's. A hosted daemon answers both 404: its browsers sign in at
//! `/signin`.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::Via;
use clax_http::auth::RequireToken;
use clax_http::error::ApiError;
use clax_http::local_trust::{
    LOGIN_PATH, local_path, login_page, login_page_csp, session_cookies,
};
use clax_http::state::AppState;
use clax_store::browser_login::LOGIN_CODE_TTL_SECS;
use clax_store::viewers::Claim;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Default)]
struct CreateBody {
    next: Option<String>,
}

/// `/login?code=<code>&next=<next>`, its query encoded.
pub fn login_path(code: &str, next: &str) -> String {
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", code)
        .append_pair("next", next)
        .finish();
    format!("{LOGIN_PATH}?{q}")
}

/// `POST /api/browser-login` (the token) `{next?}` → 201 `{path, url,
/// expires_in}`: a one-use login code, good for
/// [`LOGIN_CODE_TTL_SECS`] seconds, in the path a browser redeems it at
/// (`/login?code=…&next=…`; `url` is that path on the daemon's browser
/// address). `next`, `/` when absent, is where the login sends the browser;
/// 400 `invalid_next` unless it is a path of this daemon
/// ([`local_path`]). The code is shown to no one: the opener hands it to
/// the browser. 404 on a hosted daemon.
pub async fn create(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    if s.hosted.is_some() {
        return Err(ApiError::not_found());
    }
    let b: CreateBody = if body.is_empty() {
        CreateBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    let next = b.next.unwrap_or_else(|| "/".to_string());
    if !local_path(&next) {
        return Err(ApiError::bad_request(
            "invalid_next",
            "next must be a path of this daemon, starting with one /",
        ));
    }
    let code = s.store_call(|st| st.create_login_code()).await?;
    let path = login_path(code.expose(), &next);
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(json!({
            "path": path,
            "url": format!("{}{path}", s.browser_base),
            "expires_in": LOGIN_CODE_TTL_SECS,
        })),
    )
        .into_response())
}

/// `DELETE /api/browser-login` (the token, by hand: 403 `not_for_agents`
/// for the owner's agents) → `{ended}`: ends every owner browser session
/// ([`clax_http::local_trust::OwnerSessions::end_all`]), so no browser's
/// owner or events cookie stands for the owner from then on, and every
/// event stream, room socket and live socket opened with one ends; `ended`
/// is how many live ones there were. Each browser of the owner's then needs a
/// new login code (`clax open`, `clax login`). What `clax logout` calls.
/// It narrows who acts as the owner, so it asks for no confirmation. 404
/// on a hosted daemon.
pub async fn end_all(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
) -> Result<Response, ApiError> {
    if s.hosted.is_some() {
        return Err(ApiError::not_found());
    }
    d.refuse_agents()?;
    let sessions = s.owner_sessions.clone();
    let ended = s.store_call(move |st| sessions.end_all(st)).await?;
    tracing::info!(ended, "ended every owner browser session");
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(json!({"ended": ended})),
    )
        .into_response())
}

/// Debug builds only: `POST /api/_test/viewers` (the token) → `{cookie,
/// viewer}`: a viewer minted for a browser of this machine, with no name,
/// and its `clax_viewer` cookie, for the browser tests of what such a
/// viewer may do. Reaching the port alone mints none.
#[cfg(debug_assertions)]
pub async fn test_viewer(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    let cookie = clax_core::secret::viewer_cookie().expose().to_string();
    let minted = cookie.clone();
    let viewer = s
        .store_call(move |st| st.mint_viewer(&minted, true))
        .await?;
    Ok(axum::Json(json!({"cookie": cookie, "viewer": viewer})))
}

#[derive(Deserialize)]
pub struct RedeemQuery {
    code: Option<String>,
    next: Option<String>,
}

/// A small page with `status`, a heading and a line, for a browser.
fn page(status: StatusCode, title: &str, line: &str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>{title}</title><style>:root{{color-scheme:light dark}}\
             body{{font:16px/1.5 system-ui,sans-serif;margin:0;padding:2rem 16px;background:Canvas;color:CanvasText}}\
             </style></head><body><h1>{title}</h1><p>{line}</p></body></html>"
        ),
    )
        .into_response()
}

/// `GET /login?code=<code>&next=<next>`: redeems a login code in the
/// browser that opened it, from this machine
/// ([`clax_http::identity::from_this_machine`]; 403 `forbidden_host`
/// otherwise, and for a `Host` the API does not answer to), and, where
/// the state names a `proc_root` (Linux), from a socket of the daemon's own
/// user ([`clax_http::peer_user::redeemer_is_this_user`]; 403
/// `not_this_user` otherwise, and when that cannot be told, the code
/// unspent). The browser
/// becomes a browser of the owner's: a new owner browser session is opened
/// and set as its owner and events cookies ([`session_cookies`]), in
/// place of any it held (which stays live, `clax logout` ending it with
/// the rest); and when it
/// carries exactly one `clax_viewer`
/// cookie, the viewer that names is claimed for the owner
/// ([`clax_store::Store::claim_for_owner`]: only a viewer minted on this
/// machine), the cookie removed, and a viewer the claim retired leaves
/// every presence list and room at once. Then 200 with the login's page
/// ([`login_page`]), whose script writes the session's proof into the
/// origin's `localStorage` and goes on to `next`. 400 for a
/// `next` that is not a path of this daemon ([`local_path`]), before the
/// code is spent; 410 for a code spent before, past its time, or never
/// made. The answer is never cached and sends no referrer. 404 on a hosted
/// daemon.
pub async fn redeem(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    uri: axum::http::Uri,
    q: Result<Query<RedeemQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if s.hosted.is_some() {
        return ApiError::not_found().into_response();
    }
    if !clax_http::auth::request_host_allowed(&headers, &uri, &extensions)
        || !clax_http::identity::from_this_machine(&headers, clax_http::auth::conn_of(&extensions))
    {
        return ApiError::forbidden(
            "forbidden_host",
            "a login link opens only in a browser on this machine",
        )
        .into_response();
    }
    // Where the opener took the link on its command line (Linux), another
    // user of the machine may have read it there: only a connection of the
    // daemon's own user redeems it, and one whose user cannot be told is
    // refused. The code is not spent either way.
    if let (Some(proc), Some(c)) = (&s.proc_root, clax_http::auth::conn_of(&extensions)) {
        let refused = match clax_http::peer_user::redeemer_is_this_user(proc, c.peer, c.local) {
            Ok(true) => None,
            Ok(false) => {
                Some("this login link opens only in a browser of the user Clax runs as".to_string())
            }
            Err(why) => Some(format!(
                "could not tell which user's browser opened this login link ({why}); run `clax login` as the user Clax runs as and open the link it prints"
            )),
        };
        if let Some(message) = refused {
            return ApiError::forbidden("not_this_user", message).into_response();
        }
    }
    let (code, next) = match q {
        Ok(Query(q)) => (
            q.code.unwrap_or_default(),
            q.next.unwrap_or_else(|| "/".into()),
        ),
        Err(_) => (String::new(), String::new()),
    };
    if !local_path(&next) {
        return page(
            StatusCode::BAD_REQUEST,
            "Not a Clax address",
            "This login link sends the browser somewhere other than this Clax.",
        );
    }
    let redeemed = s.store_call(move |st| st.redeem_login_code(&code)).await;
    match redeemed {
        Ok(true) => {}
        Ok(false) => {
            return page(
                StatusCode::GONE,
                "This login link was used or has expired",
                "Run <code>clax open</code> or <code>clax login</code> in your terminal for a new one.",
            );
        }
        Err(e) => return e.into_response(),
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    // Always a new session, whose proof the page writes: the proof of one
    // this browser held may be gone from its storage. The held one is left
    // live: the browser's cookie and storage now hold the new one, so no
    // copy of the old cookie is proven but by whoever already held both.
    let sessions = s.owner_sessions.clone();
    let opened = s.store_call(move |st| sessions.open(st)).await;
    let opened = match opened {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    let mut cookies = session_cookies(&host, &opened.cookie);
    // Exactly one viewer cookie: with several, another page planted one
    // and none can be trusted to be this browser's.
    if let [cookie] = clax_http::viewer::read_all(&headers).as_slice() {
        let cookie = cookie.clone();
        let claimed = match s
            .store_call(move |st| st.claim_for_owner(Via::Shell, &cookie))
            .await
        {
            Ok(c) => c,
            Err(e) => return e.into_response(),
        };
        tracing::debug!(?claimed, "claiming a browser viewer for the owner");
        let retired = match &claimed {
            Claim::Nothing => None,
            Claim::Adopted { retired } => retired.clone(),
            Claim::Merged(old) => Some(old.clone()),
        };
        if claimed != Claim::Nothing {
            cookies.push(clax_http::identity::clear_viewer_cookie());
        }
        if let Some(old) = retired {
            for aid in s.presence.forget(&old) {
                clax_realtime::presence::announce(&s.events, &s.presence, &aid);
            }
            s.rooms.evict_viewer(&old);
        }
    }
    let nonce = clax_core::secret::token("").expose().to_string();
    let mut res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
            (header::X_FRAME_OPTIONS, "DENY".to_string()),
            (header::CONTENT_SECURITY_POLICY, login_page_csp(&nonce)),
        ],
        login_page(opened.proof.expose(), &next, &nonce),
    )
        .into_response();
    for c in cookies {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    res
}

//! A fake GitHub for GitHub sign-in tests (`clax_server::github`), on a
//! loopback port of its own: an OAuth App's authorize page, its token
//! exchange and the API's `GET /user`, for the accounts a test adds.
//!
//! - `GET /login/oauth/authorize?redirect_uri&state…` signs in as the
//!   account [`FakeGitHub::sign_in_as`] chose (as a browser that clicked
//!   "Authorize" would) and redirects to `redirect_uri` with `code` and
//!   `state`; it keeps each query it was sent ([`FakeGitHub::authorized`]).
//! - `POST /login/oauth/access_token` exchanges `code-<id>` for
//!   `tok-<id>` when the form names the app's client ID and secret, keeping
//!   the PKCE verifier it was sent ([`FakeGitHub::verifiers`]); any other
//!   answers GitHub's `{"error": …}`.
//! - `GET /user` answers the account `Bearer tok-<id>` names, else 401.
//! - `GET /users/<login>` answers `{id, login, type}` of the account (or
//!   organization, [`FakeGitHub::organization`]) holding `login` now (any
//!   case; the login as it was added), else 404; 503 while
//!   [`FakeGitHub::set_unavailable`]. It needs the app's client ID and
//!   secret as basic auth (a wrong pair is 401), and keeps each login it
//!   was asked ([`FakeGitHub::lookups`]). Renaming an account is adding it
//!   again under its new login ([`FakeGitHub::account`]); another account
//!   may then take the old one.

use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Inner {
    client_id: String,
    client_secret: String,
    users: Arc<Mutex<HashMap<i64, Value>>>,
    verifiers: Arc<Mutex<Vec<String>>>,
    authorized: Arc<Mutex<Vec<HashMap<String, String>>>>,
    next: Arc<Mutex<Option<i64>>>,
    lookups: Arc<Mutex<Vec<String>>>,
    unavailable: Arc<Mutex<bool>>,
}

/// A running fake GitHub (see the module docs).
#[derive(Clone)]
pub struct FakeGitHub {
    /// Its base URL (`http://127.0.0.1:<port>`), both the OAuth base and
    /// the API base.
    pub base: String,
    inner: Inner,
}

impl FakeGitHub {
    /// Starts a fake GitHub whose OAuth App is `client_id` with
    /// `client_secret`.
    pub async fn start(client_id: &str, client_secret: &str) -> FakeGitHub {
        let inner = Inner {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            ..Inner::default()
        };
        let app = axum::Router::new()
            .route("/login/oauth/authorize", get(authorize))
            .route("/login/oauth/access_token", post(access_token))
            .route("/user", get(user))
            .route("/users/{login}", get(users))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FakeGitHub { base, inner }
    }

    /// Adds (or replaces: a rename) account `id` with `login` and profile
    /// `name`.
    pub fn account(&self, id: i64, login: &str, name: Option<&str>) {
        self.inner.users.lock().unwrap().insert(
            id,
            json!({"id": id, "login": login, "name": name, "type": "User",
                   "avatar_url": format!("https://avatars.example/{id}")}),
        );
    }

    /// Adds organization `id` holding `login`: `GET /users/<login>` finds
    /// it, and no one signs in as it.
    pub fn organization(&self, id: i64, login: &str) {
        self.inner.users.lock().unwrap().insert(
            id,
            json!({"id": id, "login": login, "name": null, "type": "Organization"}),
        );
    }

    /// Makes `GET /users/<login>` answer 503 (`true`) or answer again.
    pub fn set_unavailable(&self, unavailable: bool) {
        *self.inner.unavailable.lock().unwrap() = unavailable;
    }

    /// The logins `GET /users/<login>` was asked, in order.
    pub fn lookups(&self) -> Vec<String> {
        self.inner.lookups.lock().unwrap().clone()
    }

    /// The OAuth App of a hosted daemon through this fake
    /// ([`clax_http::hosted::Hosted::with_github`]), whose owner's login
    /// is `owner_login`.
    pub fn hosted_app(&self, owner_login: Option<&str>) -> clax_http::github::GitHub {
        clax_http::github::GitHub::new(
            &self.inner.client_id,
            &self.inner.client_secret,
            owner_login,
            Some(&self.base),
            Some(&self.base),
        )
        .expect("a client ID and secret")
    }

    /// The account the authorize page signs in as from now on.
    pub fn sign_in_as(&self, id: i64) {
        *self.inner.next.lock().unwrap() = Some(id);
    }

    /// The PKCE verifiers the token exchanges sent, in order.
    pub fn verifiers(&self) -> Vec<String> {
        self.inner.verifiers.lock().unwrap().clone()
    }

    /// Turns GitHub sign-in on for local daemon state `s`, as `[github]`
    /// would, through this fake, on `http://localhost:<its port>`: for
    /// [`crate::TestServer::spawn_with`]'s setup.
    pub fn turn_on(&self, s: &mut clax_http::state::AppState) {
        let github = clax_http::github::GitHub::new(
            &self.inner.client_id,
            &self.inner.client_secret,
            None,
            Some(&self.base),
            Some(&self.base),
        )
        .expect("a client ID and secret");
        s.local_github = Some(Arc::new(clax_http::github::LocalGitHub {
            github,
            origins: vec![format!("http://localhost:{}", s.port)],
        }));
    }

    /// The authorize page's queries, in order.
    pub fn authorized(&self) -> Vec<HashMap<String, String>> {
        self.inner.authorized.lock().unwrap().clone()
    }
}

async fn authorize(State(f): State<Inner>, Query(q): Query<HashMap<String, String>>) -> Response {
    f.authorized.lock().unwrap().push(q.clone());
    let (Some(to), Some(state), Some(id)) = (
        q.get("redirect_uri"),
        q.get("state"),
        *f.next.lock().unwrap(),
    ) else {
        return (StatusCode::BAD_REQUEST, "no redirect_uri, state or account").into_response();
    };
    if q.get("client_id") != Some(&f.client_id) {
        return (StatusCode::NOT_FOUND, "no such OAuth App").into_response();
    }
    // `state` is base64url, so it needs no escaping.
    let sep = if to.contains('?') { '&' } else { '?' };
    (
        StatusCode::FOUND,
        [(
            header::LOCATION,
            format!("{to}{sep}code=code-{id}&state={state}"),
        )],
    )
        .into_response()
}

async fn access_token(
    State(f): State<Inner>,
    Form(form): Form<HashMap<String, String>>,
) -> axum::Json<Value> {
    if form.get("client_id") != Some(&f.client_id)
        || form.get("client_secret") != Some(&f.client_secret)
    {
        return axum::Json(json!({"error": "incorrect_client_credentials"}));
    }
    f.verifiers
        .lock()
        .unwrap()
        .push(form.get("code_verifier").cloned().unwrap_or_default());
    let code = form.get("code").cloned().unwrap_or_default();
    match code.strip_prefix("code-") {
        Some(id) => {
            axum::Json(json!({"access_token": format!("tok-{id}"), "token_type": "bearer"}))
        }
        None => axum::Json(json!({"error": "bad_verification_code"})),
    }
}

async fn user(State(f): State<Inner>, headers: HeaderMap) -> Response {
    if !headers.contains_key("user-agent") {
        return (StatusCode::FORBIDDEN, "GitHub's API needs a User-Agent").into_response();
    }
    let id: Option<i64> = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer tok-"))
        .and_then(|i| i.parse().ok());
    match id.and_then(|i| f.users.lock().unwrap().get(&i).cloned()) {
        Some(u) => axum::Json(u).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// `bytes` in standard, padded base64 (RFC 4648), as basic auth sends a
/// user and password.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

async fn users(
    State(f): State<Inner>,
    axum::extract::Path(login): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    f.lookups.lock().unwrap().push(login.clone());
    if !headers.contains_key("user-agent") {
        return (StatusCode::FORBIDDEN, "GitHub's API needs a User-Agent").into_response();
    }
    let want = format!(
        "Basic {}",
        base64(format!("{}:{}", f.client_id, f.client_secret).as_bytes())
    );
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(want.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if *f.unavailable.lock().unwrap() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let found = f
        .users
        .lock()
        .unwrap()
        .values()
        .find(|u| {
            u["login"]
                .as_str()
                .is_some_and(|l| l.eq_ignore_ascii_case(&login))
        })
        .cloned();
    match found {
        Some(u) => axum::Json(json!({"id": u["id"], "login": u["login"], "type": u["type"]}))
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

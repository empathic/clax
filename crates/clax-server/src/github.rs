//! GitHub sign-in: `/auth/github[?next=<path>]` ([`start`]) and
//! [`CALLBACK_PATH`] ([`callback`]), a GitHub OAuth App's web flow with
//! PKCE, on a hosted daemon ([`clax_http::hosted::Hosted::github`]) and on
//! a local one with `[github]` ([`clax_http::github::LocalGitHub`];
//! [`local_router`]). The flow cookie keeps `next`, a path on the
//! origin ([`clax_http::site_access::safe_next`]: anything else is `/`),
//! under its MAC, and a sign-in redirects there. The client the daemon
//! holds is [`clax_http::github::GitHub`].
//!
//! On a local daemon the redirect names only one of `[github] origins`,
//! the one whose authority is the request's `Host` (any other host answers
//! 400, and the callback, whose flow cookie is host-only, returns to that
//! same host). A sign-in makes the browser the account's person by a new
//! `clax_viewer` cookie ([`clax_store::Store::github_sign_in_local`]):
//! never the owner, with no rights of its own, and a person is made only
//! for a login a pending queue invite or share names.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use clax_http::github::*;
use clax_http::hosted::{Attempt, Hosted};
use clax_http::state::AppState;
use clax_store::github::{GitHubAccount, GitHubOutcome, GitHubUser};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// One sign-in's site: the GitHub app, the origin the OAuth App returns
/// to, and, on a hosted daemon, its hosted mode.
struct Flow {
    github: GitHub,
    origin: String,
    secure: bool,
    hosted: Option<Arc<Hosted>>,
}

impl Flow {
    /// The sign-in a request to `s` with `headers` runs: the hosted one, or
    /// the local one on one of its origins; `Err` with the answer (404 when
    /// GitHub sign-in is off, 400 on a host none of `[github] origins`
    /// names).
    fn of(s: &AppState, headers: &HeaderMap) -> Result<Flow, Response> {
        if let Some(h) = &s.hosted {
            let Some(g) = h.github.clone() else {
                return Err(clax_http::error::ApiError::not_found().into_response());
            };
            return Ok(Flow {
                github: g,
                origin: h.origin.clone(),
                secure: h.secure,
                hosted: Some(h.clone()),
            });
        }
        let Some(l) = &s.local_github else {
            return Err(clax_http::error::ApiError::not_found().into_response());
        };
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let Some(origin) = l.origin_for_host(host) else {
            let first = l.origins.first().map_or("", String::as_str);
            let f = Flow {
                github: l.github.clone(),
                origin: first.to_string(),
                secure: false,
                hosted: None,
            };
            return Err(failed(
                &f,
                StatusCode::BAD_REQUEST,
                &format!("GitHub sign-in on this Clax starts at {first}/auth/github."),
            ));
        };
        Ok(Flow {
            github: l.github.clone(),
            origin: origin.to_string(),
            secure: origin.starts_with("https://"),
            hosted: None,
        })
    }

    fn redirect_uri(&self) -> String {
        format!("{}{CALLBACK_PATH}", self.origin)
    }
}

fn flow_cookie_name(f: &Flow) -> &'static str {
    if f.secure {
        "__Host-clax_github"
    } else {
        "clax_github"
    }
}

fn flow_cookie(f: &Flow, value: &str, max_age: i64) -> HeaderValue {
    let secure = if f.secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{}={value}; Path=/; Max-Age={max_age}; HttpOnly{secure}; SameSite=Lax",
        flow_cookie_name(f)
    ))
    .expect("flow cookies are header-safe")
}

/// A local daemon's two GitHub sign-in routes (a hosted one mounts them in
/// [`crate::hosted::router`]); both answer 404 while `[github]` is off.
pub fn local_router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/auth/github", axum::routing::get(start))
        .route(CALLBACK_PATH, axum::routing::get(callback))
}

/// The MAC of a flow cookie's `body` under the daemon token `token`.
fn flow_mac(token: &str, body: &str) -> String {
    clax_http::hosted::keyed_mac(token, "clax github flow", body)
}

/// The PKCE S256 challenge of `verifier`.
fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Where a sign-in returns to: `next` when it is a path on the public
/// origin ([`clax_http::site_access::safe_next`]) a `Location` holds as
/// it is (ASCII), else `/`.
fn return_path(next: &str) -> String {
    let next = clax_http::site_access::safe_next(next);
    if next.is_ascii() { next } else { "/".into() }
}

#[derive(Deserialize)]
pub struct StartQuery {
    next: Option<String>,
}

/// `GET /auth/github[?next=<path>]`: redirects to GitHub's authorize page
/// (see the module docs); 404 when GitHub sign-in is off.
pub async fn start(
    State(s): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<StartQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let f = match Flow::of(&s, &headers) {
        Ok(f) => f,
        Err(res) => return res,
    };
    let g = &f.github;
    let next = q.ok().and_then(|Query(q)| q.next);
    let next = return_path(next.as_deref().unwrap_or("/"));
    let state = clax_core::secret::browser_code();
    let verifier = clax_core::secret::browser_code();
    let expires = (s.store.now_utc() + chrono::Duration::seconds(FLOW_TTL_SECS)).timestamp();
    let body = format!(
        "{}.{}.{expires}.{}",
        state.expose(),
        verifier.expose(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(next.as_bytes())
    );
    let value = format!("{body}.{}", flow_mac(&s.token, &body));
    let mut url = url::Url::parse(&format!("{}/login/oauth/authorize", g.oauth_base))
        .expect("the OAuth base is a URL");
    url.query_pairs_mut()
        .append_pair("client_id", &g.client_id)
        .append_pair("redirect_uri", &f.redirect_uri())
        .append_pair("scope", "read:user")
        .append_pair("state", state.expose())
        .append_pair("code_challenge", &challenge(verifier.expose()))
        .append_pair("code_challenge_method", "S256")
        .append_pair("allow_signup", "true");
    let mut res = (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, url.as_str().to_string())],
    )
        .into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, flow_cookie(&f, &value, FLOW_TTL_SECS));
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The flow cookie's verifier and where the sign-in returns to, when the
/// cookie is this daemon's, live, and made for `state`.
fn verified_flow(
    s: &AppState,
    f: &Flow,
    headers: &HeaderMap,
    state: &str,
) -> Option<(String, String)> {
    let value = clax_http::hosted::cookie_value(headers, flow_cookie_name(f))?;
    let (body, mac) = value.rsplit_once('.')?;
    let want = flow_mac(&s.token, body);
    if !clax_core::secret::constant_time_eq(mac.as_bytes(), want.as_bytes()) {
        return None;
    }
    let mut parts = body.split('.');
    let (st, verifier, expires) = (parts.next()?, parts.next()?, parts.next()?);
    let expires: i64 = expires.parse().ok()?;
    // A cookie of before `next` (three parts) returns to the gallery.
    let next = parts
        .next()
        .and_then(|n| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(n)
                .ok()
        })
        .and_then(|n| String::from_utf8(n).ok())
        .map_or_else(|| "/".to_string(), |n| return_path(&n));
    let fresh = s.store.now_utc().timestamp() < expires;
    (fresh && clax_core::secret::constant_time_eq(st.as_bytes(), state.as_bytes()))
        .then(|| (verifier.to_string(), next))
}

/// `GET /auth/github/callback`: signs the browser in (see the module docs).
pub async fn callback(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    audit: clax_http::audit::DeferredAudit,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let f = match Flow::of(&s, &headers) {
        Ok(f) => f,
        Err(res) => return res,
    };
    if let Some(h) = &f.hosted
        && h.attempt(Attempt::SignIn, &headers, &extensions).is_err()
    {
        return failed(
            &f,
            StatusCode::TOO_MANY_REQUESTS,
            "Too many sign-ins from this address. Wait a minute, then try again.",
        );
    }
    if q.error.is_some() {
        return failed(&f, StatusCode::FORBIDDEN, "GitHub did not sign you in.");
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return failed(&f, StatusCode::BAD_REQUEST, "GitHub sent no sign-in code.");
    };
    let Some((verifier, next)) = verified_flow(&s, &f, &headers, &state) else {
        return failed(
            &f,
            StatusCode::BAD_REQUEST,
            "This sign-in expired or was started in another browser.",
        );
    };
    let user = match clax_rt::sendable(fetch_user(&f, &code, &verifier)).await {
        Ok(u) => u,
        Err(why) => {
            tracing::warn!(error = %why, "GitHub sign-in failed");
            return failed(
                &f,
                StatusCode::BAD_GATEWAY,
                "GitHub could not confirm who you are.",
            );
        }
    };
    let Some(h) = f.hosted.clone() else {
        return local_sign_in(&s, &f, audit, user, next).await;
    };
    let g = &f.github;
    let owner_login = g.is_owner(&user.login);
    let id = user.id;
    let known = match s.store_call(move |st| st.github_known(id)).await {
        Ok(k) => k,
        Err(e) => return e.into_response(),
    };
    if !owner_login && !known && h.attempt(Attempt::SignUp, &headers, &extensions).is_err() {
        return failed(
            &f,
            StatusCode::TOO_MANY_REQUESTS,
            "Too many new accounts from this address. Try again later.",
        );
    }
    let old = clax_http::hosted::cookie_value(&headers, h.cookie_name());
    let replaced = old.is_some();
    let legal = h.legal.as_ref().map(|l| l.version().to_string());
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.github_sign_in(&ctx, &user, owner_login)?;
            crate::hosted::agree_at_sign_in(st, &signed.0, legal.as_deref())?;
            if let Some(t) = old {
                st.end_browser_session(&ctx, &t, "replaced")?;
            }
            Ok(signed)
        })
        .await;
    let (signed, outcome) = match signed {
        Ok(x) => x,
        Err(e) if e.code == "github_blocked" => {
            return failed(
                &f,
                StatusCode::FORBIDDEN,
                "This GitHub account may not sign in here.",
            );
        }
        Err(e) if e.code == "github_not_owner" => {
            return failed(
                &f,
                StatusCode::FORBIDDEN,
                "Another GitHub account owns this Clax.",
            );
        }
        Err(e) => return e.into_response(),
    };
    if replaced {
        // The session it replaced ended, and its site sessions with it.
        crate::hosted::access_changed(&s, None);
    }
    if outcome == GitHubOutcome::New {
        crate::routes::grants::announce(
            &s,
            serde_json::json!({"what": "person_joined", "person": signed.who.viewer.public_id}),
        );
    }
    // A person's quickstart: made at their first sign-in, moved to a newer
    // release at each later one (a seed that failed is retried, and is a
    // no-op otherwise). Awaited before the redirect so the first page load
    // sees it; errors are logged only, never failing the sign-in.
    if outcome != GitHubOutcome::Owner {
        crate::quickstart::sign_in(&s, &signed.who.viewer.public_id).await;
    }
    let mut res = (StatusCode::SEE_OTHER, [(header::LOCATION, next)]).into_response();
    let hs = res.headers_mut();
    hs.append(header::SET_COOKIE, h.session_cookie(&signed.session));
    hs.append(header::SET_COOKIE, flow_cookie(&f, "", 0));
    hs.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    hs.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
}

/// The callback's end on a local daemon: the browser becomes `user`'s
/// person by a new `clax_viewer` cookie, replacing any it held
/// ([`clax_store::Store::github_sign_in_local`]); no quickstart is seeded
/// and no hosted session made. The owner is never signed in this way.
async fn local_sign_in(
    s: &AppState,
    f: &Flow,
    audit: clax_http::audit::DeferredAudit,
    user: GitHubUser,
    next: String,
) -> Response {
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.github_sign_in_local(&ctx, &user)
        })
        .await;
    let (public_id, cookie, outcome) = match signed {
        Ok(x) => x,
        Err(e) if e.code == "github_blocked" => {
            return failed(
                f,
                StatusCode::FORBIDDEN,
                "This GitHub account may not sign in here.",
            );
        }
        Err(e) if e.code == "github_owner_account" => {
            return failed(
                f,
                StatusCode::FORBIDDEN,
                "This GitHub account is this Clax's owner's. The owner signs in on this machine, not with GitHub.",
            );
        }
        Err(e) if e.code == "github_not_invited" => {
            return failed(
                f,
                StatusCode::FORBIDDEN,
                "Nobody here has added this GitHub account yet. Ask the person who runs this Clax to add your GitHub login, then sign in again.",
            );
        }
        Err(e) => return e.into_response(),
    };
    if outcome == GitHubOutcome::New {
        crate::routes::grants::announce(
            s,
            serde_json::json!({"what": "person_joined", "person": public_id}),
        );
    }
    let mut res = (StatusCode::SEE_OTHER, [(header::LOCATION, next)]).into_response();
    let hs = res.headers_mut();
    hs.append(header::SET_COOKIE, clax_http::viewer::set_cookie(&cookie));
    hs.append(header::SET_COOKIE, flow_cookie(f, "", 0));
    hs.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    hs.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct UserAnswer {
    id: i64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

fn timeout(r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    #[cfg(not(target_arch = "wasm32"))]
    {
        r.timeout(std::time::Duration::from_secs(20))
    }
    #[cfg(target_arch = "wasm32")]
    {
        r
    }
}

/// Exchanges `code` (with `verifier`) for a token and reads its account.
async fn fetch_user(f: &Flow, code: &str, verifier: &str) -> Result<GitHubUser, String> {
    let g = &f.github;
    let http = reqwest::Client::new();
    let redirect = f.redirect_uri();
    let form = [
        ("client_id", g.client_id.as_str()),
        ("client_secret", g.client_secret.as_str()),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", verifier),
    ];
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let res = timeout(
        http.post(format!("{}/login/oauth/access_token", g.oauth_base))
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("user-agent", "clax")
            .body(body),
    )
    .send()
    .await
    .map_err(|e| format!("token exchange: {e}"))?;
    let t: TokenAnswer = res
        .json()
        .await
        .map_err(|e| format!("token exchange answer: {e}"))?;
    let Some(token) = t.access_token else {
        return Err(format!(
            "token exchange refused: {}",
            t.error.unwrap_or_default()
        ));
    };
    let res = timeout(
        http.get(format!("{}/user", g.api_base))
            .header("accept", "application/vnd.github+json")
            .header("authorization", format!("Bearer {token}"))
            .header("user-agent", "clax"),
    )
    .send()
    .await
    .map_err(|e| format!("GET /user: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("GET /user answered {}", res.status()));
    }
    let u: UserAnswer = res
        .json()
        .await
        .map_err(|e| format!("GET /user answer: {e}"))?;
    Ok(GitHubUser {
        id: u.id,
        login: u.login,
        name: u.name,
        avatar_url: u.avatar_url,
    })
}

/// What GitHub answered for a login ([`lookup_login`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The account it names now.
    Found(GitHubAccount),
    /// No such login, or one an organization holds (no one signs in as
    /// it).
    Unknown,
    /// GitHub did not answer: unreachable, rate limited, or an answer that
    /// is not one; why.
    Unavailable(String),
}

#[derive(Deserialize)]
struct UsersAnswer {
    id: i64,
    login: String,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

/// Whether GitHub answered `res` because the caller is rate limited: 429,
/// or 403 with no requests left (`x-ratelimit-remaining: 0`), as its REST
/// API answers a spent limit.
fn rate_limited(res: &reqwest::Response) -> bool {
    res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (res.status() == reqwest::StatusCode::FORBIDDEN
            && res
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                == Some("0"))
}

/// Asks GitHub's API (`GET {api_base}/users/<login>`) which account
/// `login` names now: first with the OAuth App's client ID and secret as
/// basic auth (the app's own rate limit), and, when GitHub refuses those
/// credentials (401, or a 403 that is not a spent limit), again without
/// any, as the endpoint is public (60 an hour per address). `login` must
/// already be one ([`clax_store::shares::github_login`]): it is a path
/// segment of the request, escaped as one. Only a network error, a 5xx, a
/// spent rate limit or an answer that is not one is
/// [`Lookup::Unavailable`].
pub async fn lookup_login(g: &GitHub, login: &str) -> Lookup {
    let Ok(mut url) = url::Url::parse(&g.api_base) else {
        return Lookup::Unavailable(format!("the API base {} is not a URL", g.api_base));
    };
    match url.path_segments_mut() {
        Ok(mut p) => {
            p.pop_if_empty().push("users").push(login);
        }
        Err(()) => return Lookup::Unavailable("the API base cannot take a path".into()),
    }
    let http = reqwest::Client::new();
    let ask = |with_app: bool| {
        let r = http
            .get(url.clone())
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "clax");
        let r = if with_app {
            r.basic_auth(&g.client_id, Some(&g.client_secret))
        } else {
            r
        };
        timeout(r).send()
    };
    let mut res = match ask(true).await {
        Ok(r) => r,
        Err(e) => return Lookup::Unavailable(format!("GET /users: {e}")),
    };
    let refused = res.status() == reqwest::StatusCode::UNAUTHORIZED
        || (res.status() == reqwest::StatusCode::FORBIDDEN && !rate_limited(&res));
    if refused {
        tracing::warn!(status = %res.status(), "GitHub refused the OAuth App's credentials on GET /users; asking without them");
        res = match ask(false).await {
            Ok(r) => r,
            Err(e) => return Lookup::Unavailable(format!("GET /users: {e}")),
        };
    }
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Lookup::Unknown;
    }
    if !res.status().is_success() {
        return Lookup::Unavailable(format!("GET /users answered {}", res.status()));
    }
    let u: UsersAnswer = match res.json().await {
        Ok(u) => u,
        Err(e) => return Lookup::Unavailable(format!("GET /users answer: {e}")),
    };
    if u.kind.as_deref().is_some_and(|k| k != "User") {
        return Lookup::Unknown;
    }
    if !u.login.eq_ignore_ascii_case(login) {
        return Lookup::Unavailable(format!("GET /users/{login} answered {}", u.login));
    }
    Lookup::Found(GitHubAccount {
        id: u.id,
        login: u.login,
    })
}

/// The GitHub account login `raw` names now, before access is given by it
/// (a share, a queue invite), on behalf of `person` (a public ID or member
/// key, whose lookups are rate limited on a hosted daemon). Nothing is
/// kept on any error.
///
/// # Errors
/// 400 `invalid_login` (checked before any request); 409
/// `github_sign_in_off` where no one signs in with GitHub (no client to
/// ask with, and no sign-in a grant could bind at); 429 `rate_limited`;
/// 400 `unknown_github_login` for a login no GitHub account holds (or an
/// organization's); 503 `github_unavailable` when GitHub does not answer,
/// so no grant is made by the login alone.
pub async fn resolve_login(
    s: &AppState,
    raw: &str,
    person: &str,
) -> Result<GitHubAccount, clax_http::error::ApiError> {
    use clax_http::error::ApiError;
    let login = clax_store::shares::github_login(raw)?;
    let Some(g) = s.github().cloned() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "github_sign_in_off",
            "no one signs in to this Clax with GitHub, so a GitHub login added here would never take effect: turn on GitHub sign-in ([github] in config.toml), or choose a person here",
        ));
    };
    if let Some(h) = &s.hosted {
        h.github_lookup(person)?;
    }
    match clax_rt::sendable(lookup_login(&g, &login)).await {
        Lookup::Found(a) => Ok(a),
        Lookup::Unknown => Err(ApiError::bad_request(
            "unknown_github_login",
            format!("no GitHub account has the login {login}: check its spelling"),
        )),
        Lookup::Unavailable(why) => {
            tracing::warn!(error = %why, "GitHub login lookup failed");
            Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "github_unavailable",
                "GitHub did not say which account that login is, so nothing was added: try again in a minute",
            ))
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page a failed sign-in answers: `why`, a link to sign in again (and,
/// on a hosted daemon, to its other ways), the instance's Terms of Service
/// and Privacy Policy when it shows them, and Clax's source on GitHub.
/// The flow cookie is cleared.
fn failed(f: &Flow, status: StatusCode, why: &str) -> Response {
    let others = if f.hosted.is_some() {
        " · <a href=\"/signin\">Other ways to sign in</a>"
    } else {
        ""
    };
    let legal = f
        .hosted
        .as_ref()
        .and_then(|h| h.legal.as_ref())
        .map_or(String::new(), |l| {
            format!(
                "<a href=\"{}\">Terms of Service</a> · <a href=\"{}\">Privacy Policy</a> · ",
                escape(l.terms_url()),
                escape(l.privacy_url())
            )
        });
    let footer = format!(
        "<footer>{legal}<a href=\"https://github.com/empathic/clax\" target=\"_blank\" rel=\"noopener\">Source on GitHub</a></footer>"
    );
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Sign in · Clax</title>\
<style>:root{{color-scheme:light dark;--bg:#fafaf9;--fg:#1c1917;--muted:#57534e;--card:#fff;--border:#e7e5e4}}\
@media (prefers-color-scheme:dark){{:root{{--bg:#1c1917;--fg:#f5f5f4;--muted:#a8a29e;--card:#292524;--border:#44403c}}}}\
body{{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}}\
main{{max-width:480px;margin:0 auto;padding:48px 16px}}section{{background:var(--card);border:1px solid var(--border);border-radius:12px;padding:22px 24px}}\
h1{{margin:0 0 10px;font-size:22px}}p{{margin:0 0 12px}}a{{color:var(--fg)}}\
footer{{margin-top:16px;text-align:center;font-size:12.5px;color:var(--muted)}}footer a{{color:inherit}}</style></head>\
<body><main><section><h1>Sign-in failed</h1><p>{}</p><p><a href=\"/auth/github\">Sign in again</a>{others}</p></section>{}</main></body></html>",
        escape(why),
        footer
    );
    let mut res = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, flow_cookie(f, "", 0));
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_rfc_7636s() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_owner_login_is_compared_without_case_or_an_at() {
        let g = GitHub::new("id", "secret", Some("@AKesling"), None, None).unwrap();
        assert!(g.is_owner("akesling"));
        assert!(!g.is_owner("akeslin"));
        assert!(GitHub::new("id", "", None, None, None).is_none());
    }
}

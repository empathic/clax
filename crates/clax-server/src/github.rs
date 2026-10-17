//! GitHub sign-in on a hosted daemon: `/auth/github` ([`start`]) and
//! [`CALLBACK_PATH`] ([`callback`]), a GitHub OAuth App's web flow with PKCE.
//! The client the daemon holds is [`clax_http::github::GitHub`].

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use clax_http::github::*;
use clax_http::hosted::{Attempt, Hosted};
use clax_http::state::AppState;
use clax_store::github::{GitHubOutcome, GitHubUser};
use serde::Deserialize;
use sha2::{Digest, Sha256};

fn flow_cookie_name(h: &Hosted) -> &'static str {
    if h.secure {
        "__Host-clax_github"
    } else {
        "clax_github"
    }
}

fn flow_cookie(h: &Hosted, value: &str, max_age: i64) -> HeaderValue {
    let secure = if h.secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{}={value}; Path=/; Max-Age={max_age}; HttpOnly{secure}; SameSite=Lax",
        flow_cookie_name(h)
    ))
    .expect("flow cookies are header-safe")
}

/// The MAC of a flow cookie's `body` under the daemon token `token`.
fn flow_mac(token: &str, body: &str) -> String {
    clax_http::hosted::keyed_mac(token, "clax github flow", body)
}

/// The PKCE S256 challenge of `verifier`.
fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn github_of(s: &AppState) -> Option<(std::sync::Arc<Hosted>, GitHub)> {
    let h = s.hosted.clone()?;
    let g = h.github.clone()?;
    Some((h, g))
}

/// `GET /auth/github`: redirects to GitHub's authorize page (see the
/// module docs); 404 when GitHub sign-in is off.
pub async fn start(State(s): State<AppState>) -> Response {
    let Some((h, g)) = github_of(&s) else {
        return clax_http::error::ApiError::not_found().into_response();
    };
    let state = clax_core::secret::browser_code();
    let verifier = clax_core::secret::browser_code();
    let expires = (s.store.now_utc() + chrono::Duration::seconds(FLOW_TTL_SECS)).timestamp();
    let body = format!("{}.{}.{expires}", state.expose(), verifier.expose());
    let value = format!("{body}.{}", flow_mac(&s.token, &body));
    let mut url = url::Url::parse(&format!("{}/login/oauth/authorize", g.oauth_base))
        .expect("the OAuth base is a URL");
    url.query_pairs_mut()
        .append_pair("client_id", &g.client_id)
        .append_pair("redirect_uri", &format!("{}{CALLBACK_PATH}", h.origin))
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
        .append(header::SET_COOKIE, flow_cookie(&h, &value, FLOW_TTL_SECS));
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

/// The flow cookie's verifier, when the cookie is this daemon's, live, and
/// made for `state`.
fn verified_flow(s: &AppState, h: &Hosted, headers: &HeaderMap, state: &str) -> Option<String> {
    let value = clax_http::hosted::cookie_value(headers, flow_cookie_name(h))?;
    let (body, mac) = value.rsplit_once('.')?;
    let want = flow_mac(&s.token, body);
    if !clax_core::secret::constant_time_eq(mac.as_bytes(), want.as_bytes()) {
        return None;
    }
    let mut parts = body.split('.');
    let (st, verifier, expires) = (parts.next()?, parts.next()?, parts.next()?);
    let expires: i64 = expires.parse().ok()?;
    let fresh = s.store.now_utc().timestamp() < expires;
    (fresh && clax_core::secret::constant_time_eq(st.as_bytes(), state.as_bytes()))
        .then(|| verifier.to_string())
}

/// `GET /auth/github/callback`: signs the browser in (see the module docs).
pub async fn callback(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    audit: clax_http::audit::DeferredAudit,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some((h, g)) = github_of(&s) else {
        return clax_http::error::ApiError::not_found().into_response();
    };
    if h.attempt(Attempt::SignIn, &headers, &extensions).is_err() {
        return failed(
            &h,
            StatusCode::TOO_MANY_REQUESTS,
            "Too many sign-ins from this address. Wait a minute, then try again.",
        );
    }
    if q.error.is_some() {
        return failed(&h, StatusCode::FORBIDDEN, "GitHub did not sign you in.");
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return failed(&h, StatusCode::BAD_REQUEST, "GitHub sent no sign-in code.");
    };
    let Some(verifier) = verified_flow(&s, &h, &headers, &state) else {
        return failed(
            &h,
            StatusCode::BAD_REQUEST,
            "This sign-in expired or was started in another browser.",
        );
    };
    let user = match crate::public::sendable(fetch_user(&g, &h, &code, &verifier)).await {
        Ok(u) => u,
        Err(why) => {
            tracing::warn!(error = %why, "GitHub sign-in failed");
            return failed(
                &h,
                StatusCode::BAD_GATEWAY,
                "GitHub could not confirm who you are.",
            );
        }
    };
    let owner_login = g.is_owner(&user.login);
    let id = user.id;
    let known = match s.store_call(move |st| st.github_known(id)).await {
        Ok(k) => k,
        Err(e) => return e.into_response(),
    };
    if !owner_login && !known && h.attempt(Attempt::SignUp, &headers, &extensions).is_err() {
        return failed(
            &h,
            StatusCode::TOO_MANY_REQUESTS,
            "Too many new accounts from this address. Try again later.",
        );
    }
    let old = clax_http::hosted::cookie_value(&headers, h.cookie_name());
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.github_sign_in(&ctx, &user, owner_login)?;
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
                &h,
                StatusCode::FORBIDDEN,
                "This GitHub account may not sign in here.",
            );
        }
        Err(e) if e.code == "github_not_owner" => {
            return failed(
                &h,
                StatusCode::FORBIDDEN,
                "Another GitHub account owns this Clax.",
            );
        }
        Err(e) => return e.into_response(),
    };
    if outcome == GitHubOutcome::New {
        crate::routes::grants::announce(
            &s,
            serde_json::json!({"what": "person_joined", "person": signed.who.viewer.public_id}),
        );
    }
    let mut res = (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
    let hs = res.headers_mut();
    hs.append(header::SET_COOKIE, h.session_cookie(&signed.session));
    hs.append(header::SET_COOKIE, flow_cookie(&h, "", 0));
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
async fn fetch_user(
    g: &GitHub,
    h: &Hosted,
    code: &str,
    verifier: &str,
) -> Result<GitHubUser, String> {
    let http = reqwest::Client::new();
    let redirect = format!("{}{CALLBACK_PATH}", h.origin);
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

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page a failed sign-in answers: `why`, and a link to sign in again.
/// The flow cookie is cleared.
fn failed(h: &Hosted, status: StatusCode, why: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Sign in · Clax</title>\
<style>:root{{color-scheme:light dark;--bg:#fafaf9;--fg:#1c1917;--muted:#57534e;--card:#fff;--border:#e7e5e4}}\
@media (prefers-color-scheme:dark){{:root{{--bg:#1c1917;--fg:#f5f5f4;--muted:#a8a29e;--card:#292524;--border:#44403c}}}}\
body{{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}}\
main{{max-width:480px;margin:0 auto;padding:48px 16px}}section{{background:var(--card);border:1px solid var(--border);border-radius:12px;padding:22px 24px}}\
h1{{margin:0 0 10px;font-size:22px}}p{{margin:0 0 12px}}a{{color:var(--fg)}}</style></head>\
<body><main><section><h1>Sign-in failed</h1><p>{}</p><p><a href=\"/auth/github\">Sign in again</a> · <a href=\"/signin\">Other ways to sign in</a></p></section></main></body></html>",
        escape(why)
    );
    let mut res = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, flow_cookie(h, "", 0));
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[cfg(test)]
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

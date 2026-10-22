//! Who may read an artifact's site, decided on every request to its site
//! host ([`site_access_layer`]).
//!
//! - A site open to the world (`artifacts.site_access`) is read by anyone,
//!   as a [`SiteCaller::Visitor`], unless a site session names its holder.
//! - Any other site needs the site cookie ([`SITE_COOKIE`], on `https`
//!   [`SECURE_SITE_COOKIE`]), naming a site session
//!   ([`clax_store::site_sessions`]) of this artifact whose holder may read
//!   it now ([`holder_reads`]): a share removed, a link turned off or a
//!   browser signed out refuses the very next request.
//! - Without one, a navigation (top-level, or of a frame) is sent to the
//!   public host's `/site-signin?host=<host>&next=<path>`, which makes a
//!   session when the browser may read the artifact (partitioned for a
//!   frame); anything else answers 401 `site_session_required`.
//!
//! The cookie is host-only, `HttpOnly` and `Path=/`, and lasts as long as
//! the browser's session; its session row lasts an hour from its last use.
//! Made for the shell's frame (a third-party context on a site domain of
//! its own) it is `SameSite=None; Secure; Partitioned`, keyed to the
//! shell's site; made for a top-level visit, `SameSite=Lax`.
//!
//! Every code is bound to a **state** the redeeming browser holds in a
//! cookie of the site host ([`STATE_COOKIE`], on `https`
//! [`SECURE_STATE_COOKIE`]: host-only, `Path=/`, ten
//! minutes, a fresh value per handshake): `/_clax/session` redeems a code
//! only beside it, so a code minted for one browser signs no other in. A
//! top-level handshake's state is set by the site's redirect to
//! `/site-signin`, which names it (`&state=`); a frame's by
//! `/_clax/session/start`, which hands it to the shell (`postMessage`) for
//! `POST /api/artifacts/<id>/site-code`.
//!
//! A response to a site that is not open to the world is private to its
//! browser: `Cache-Control` says `private` (`public` becomes `private`,
//! `no-cache` becomes `private, no-cache`) and it carries no
//! `Access-Control-Allow-Origin`.

use crate::error::ApiError;
use crate::host::SiteRequest;
use crate::hosted::{MemberRights, cookie_value};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use clax_core::ArtifactId;
use clax_core::rights::RightType;
use clax_store::site_sessions::Holder;

/// The site cookie's name on an `http` site (a local daemon).
pub const SITE_COOKIE: &str = "clax_site";
/// The site cookie's name on an `https` site: the `__Host-` prefix keeps a
/// sibling host from setting one for this host.
pub const SECURE_SITE_COOKIE: &str = "__Host-clax_site";

/// The state cookie's name on an `http` site.
pub const STATE_COOKIE: &str = "clax_site_state";
/// The state cookie's name on an `https` site: the `__Host-` prefix makes
/// the browser refuse one with a `Domain` (or another path, or unsecured),
/// so a sibling site under the site domain cannot plant its own state in a
/// victim's browser (until the site domain is on the Public Suffix List
/// such a sibling is same-site).
pub const SECURE_STATE_COOKIE: &str = "__Host-clax_site_state";
/// The state cookie's path, as `__Host-` requires.
pub const STATE_PATH: &str = "/";
/// How long a handshake's state lasts.
pub const STATE_TTL_SECS: i64 = 600;
/// The longest `next` kept ([`safe_next`]).
pub const MAX_NEXT: usize = 2048;

/// Who a site request reads as ([`site_access_layer`] inserts it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteCaller {
    /// A site session's holder, who may read the artifact.
    Holder(Holder),
    /// Anyone, on a site open to the world: their visitor ID (empty until
    /// the site mints them one).
    Visitor(String),
}

/// Whether the site is served on `https` (the public origin is).
pub fn secure(s: &AppState) -> bool {
    s.hosted.as_ref().is_some_and(|h| h.secure)
}

/// The site cookie's name: [`SECURE_SITE_COOKIE`] when `secure`, else
/// [`SITE_COOKIE`].
pub fn site_cookie_name(secure: bool) -> &'static str {
    if secure {
        SECURE_SITE_COOKIE
    } else {
        SITE_COOKIE
    }
}

/// `Set-Cookie` for site session `value`: host-only, `Path=/`, `HttpOnly`,
/// lasting the browser's session; `SameSite=None; Secure; Partitioned`
/// when `partitioned` (the shell's frame), else `SameSite=Lax`, `Secure`
/// when `secure`.
pub fn site_cookie(secure: bool, partitioned: bool, value: &str) -> HeaderValue {
    let name = site_cookie_name(secure);
    let tail = if partitioned {
        "; Secure; SameSite=None; Partitioned"
    } else if secure {
        "; Secure; SameSite=Lax"
    } else {
        "; SameSite=Lax"
    };
    HeaderValue::from_str(&format!("{name}={value}; Path=/; HttpOnly{tail}"))
        .expect("site session IDs are header-safe")
}

/// The state cookie's name: [`SECURE_STATE_COOKIE`] when `secure`, else
/// [`STATE_COOKIE`].
pub fn state_cookie_name(secure: bool) -> &'static str {
    if secure {
        SECURE_STATE_COOKIE
    } else {
        STATE_COOKIE
    }
}

/// A new handshake state: 32 random bytes, base64url.
pub fn new_state() -> String {
    clax_core::secret::token("").expose().to_string()
}

/// `Set-Cookie` for handshake state `value` (empty with `max_age` 0 clears
/// it): host-only, `Path=/`, `HttpOnly`, partitioned as the site cookie
/// it leads to.
fn state_cookie_with(secure: bool, partitioned: bool, value: &str, max_age: i64) -> HeaderValue {
    let name = state_cookie_name(secure);
    let tail = if partitioned {
        "; Secure; SameSite=None; Partitioned"
    } else if secure {
        "; Secure; SameSite=Lax"
    } else {
        "; SameSite=Lax"
    };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path={STATE_PATH}; Max-Age={max_age}; HttpOnly{tail}"
    ))
    .expect("states are header-safe")
}

/// `Set-Cookie` for handshake state `value`, good for [`STATE_TTL_SECS`].
pub fn state_cookie(secure: bool, partitioned: bool, value: &str) -> HeaderValue {
    state_cookie_with(secure, partitioned, value, STATE_TTL_SECS)
}

/// The state cookie of `headers` ([`state_cookie_name`]): one cookie of
/// that very name, or `None` (on `https`, a `clax_site_state` or
/// `__Secure-clax_site_state` a sibling host could set counts for nothing).
pub fn state_in(headers: &HeaderMap, secure: bool) -> Option<String> {
    cookie_value(headers, state_cookie_name(secure)).filter(|v| !v.is_empty())
}

/// `Set-Cookie` that clears the state cookie.
pub fn clear_state_cookie(secure: bool, partitioned: bool) -> HeaderValue {
    state_cookie_with(secure, partitioned, "", 0)
}

/// Whether `holder` may read artifact `aid` now, as `GET
/// /api/artifacts/<id>` decides for them: the owner always; a person by
/// their rights (their shares among them) or the artifact's link; a guest
/// and a link viewer while it is shared by link. No one reads one that is
/// deleted, or that does not exist.
///
/// # Errors
/// The store's.
pub fn holder_reads(
    st: &clax_store::Store,
    holder: &Holder,
    aid: &ArtifactId,
) -> clax_core::Result<bool> {
    if st.get_artifact(aid)?.is_none() {
        return Ok(false);
    }
    match holder {
        Holder::Owner => Ok(true),
        Holder::Person(pid) => {
            let rights = match st.person_rights(pid) {
                Ok(r) => r,
                Err(clax_core::CoreError::NotFound | clax_core::CoreError::Invalid { .. }) => {
                    return Ok(false);
                }
                Err(e) => return Err(e),
            };
            MemberRights {
                person: pid.clone(),
                rights: std::sync::Arc::new(rights),
            }
            .allows_at(st, (RightType::Artifacts, "read"), aid.as_str())
        }
        Holder::Guest(_) | Holder::Link => Ok(st.link_access(aid.as_str())?.is_some()),
    }
}

/// The public origin: hosted mode's, else the local daemon's browser base
/// (`http://localhost:7480`).
pub fn public_origin(s: &AppState) -> String {
    match &s.hosted {
        Some(h) => h.origin.clone(),
        None => s.browser_base.trim_end_matches('/').to_string(),
    }
}

/// Artifact `aid`'s site origin for a request whose `Host` is `host`
/// (the public host's or a site host's): the public origin's scheme, and
/// the `Host`'s port unless it is the scheme's default.
pub fn site_origin(s: &AppState, aid: &ArtifactId, host: &str) -> String {
    let port = host.rsplit_once(':').and_then(|(_, p)| p.parse().ok());
    let scheme = if secure(s) { "https" } else { "http" };
    s.sites().origin(aid, scheme, port)
}

/// `next` as a path on the site's own host: one of at most [`MAX_NEXT`]
/// bytes that starts with a single `/` and holds no backslash or control
/// character; anything else is `/`.
pub fn safe_next(next: &str) -> String {
    let ok = next.len() <= MAX_NEXT
        && next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && !next.chars().any(char::is_control);
    if ok { next.to_string() } else { "/".into() }
}

/// Whether `req` is a navigation of a document, top-level or framed: a
/// `GET` with `Sec-Fetch-Mode: navigate` and `Sec-Fetch-Dest: document`,
/// `iframe` or `frame`, or one that accepts `text/html` and names no
/// subresource as its destination (no `Sec-Fetch-Dest`, or `empty`), as a
/// client that is not a browser sends (Node's `fetch` says `cors`).
fn navigation(method: &Method, headers: &HeaderMap) -> bool {
    if method != Method::GET && method != Method::HEAD {
        return false;
    }
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    let dest = h("sec-fetch-dest");
    if h("sec-fetch-mode") == Some("navigate") {
        return dest.is_none_or(|d| matches!(d, "document" | "iframe" | "frame"));
    }
    dest.is_none_or(|d| d == "empty") && h("accept").is_some_and(|a| a.contains("text/html"))
}

/// Whether `headers` are a framed navigation's (`Sec-Fetch-Dest: iframe` or
/// `frame`): a site session made for it is partitioned, a frame being a
/// third-party context on a site domain of its own.
pub fn framed(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|d| d == "iframe" || d == "frame")
}

/// The public host's sign-in for site `host` that returns to `next`, for
/// the handshake whose state is `state`.
pub fn signin_url(s: &AppState, host: &str, next: &str, state: &str) -> String {
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("host", host)
        .append_pair("next", next)
        .append_pair("state", state)
        .finish();
    format!("{}/site-signin?{q}", public_origin(s))
}

/// A site's 302 to the public host's `/site-signin` for `next`: a new
/// handshake state, set in the state cookie (partitioned for a frame,
/// `headers` being a framed navigation's) and named in the address.
pub fn signin_redirect(s: &AppState, headers: &HeaderMap, next: &str) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let state = new_state();
    let to = signin_url(s, host, next, &state);
    let mut res = (StatusCode::FOUND, [(header::LOCATION, to)]).into_response();
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::SET_COOKIE,
        state_cookie(secure(s), framed(headers), &state),
    );
    res
}

/// `res` made private to its browser, as every response of a site that is
/// not open to the world is (see the module docs).
fn private(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    let cache = h
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let private = match cache.as_deref() {
        Some(c) if c.starts_with("public") => format!("private{}", &c["public".len()..]),
        Some(c)
            if c.split(',')
                .any(|d| d.trim() == "private" || d.trim() == "no-store") =>
        {
            c.to_string()
        }
        Some(c) => format!("private, {c}"),
        None => "private".to_string(),
    };
    if let Ok(v) = HeaderValue::from_str(&private) {
        h.insert(header::CACHE_CONTROL, v);
    }
    res
}

/// Who a request on the public host would read an artifact's site as, and
/// the browser session (its cookie's SHA-256) a site session made for them
/// ends with: on a hosted daemon, the browser's session (the owner, a
/// person, a guest), the daemon token's owner, or with neither a link
/// viewer; on a local daemon, the owner alone. `None` for anyone else.
pub fn request_holder(
    s: &AppState,
    headers: &HeaderMap,
    extensions: &axum::http::Extensions,
) -> Option<(Holder, Option<String>)> {
    let token = crate::auth::has_token(headers, &s.token);
    if s.hosted.is_none() {
        let who = crate::identity::Identity::from_parts(headers, extensions, &s.token);
        return who.is_owner().then_some((Holder::Owner, None));
    }
    let h = extensions.get::<crate::hosted::HostedRequest>()?;
    match &h.session {
        Some(who) => {
            let holder = if who.owner {
                Holder::Owner
            } else if who.member() {
                Holder::Person(who.viewer.public_id.clone())
            } else if who.guest {
                Holder::Guest(who.viewer.public_id.clone())
            } else {
                return None;
            };
            Some((holder, h.cookie_sha256.clone()))
        }
        None if token => Some((Holder::Owner, None)),
        None if headers.contains_key(header::AUTHORIZATION) => None,
        None => Some((Holder::Link, None)),
    }
}

/// The holder the site cookie of `headers` names on artifact `aid`, when
/// they may read it now.
///
/// # Errors
/// The store's.
pub async fn caller_holder(
    s: &AppState,
    headers: &HeaderMap,
    aid: &ArtifactId,
) -> Result<Option<Holder>, ApiError> {
    let Some(id) = cookie_value(headers, site_cookie_name(secure(s))) else {
        return Ok(None);
    };
    let aid = aid.clone();
    s.store_call(move |st| {
        Ok(match st.site_session(&id, &aid)? {
            Some(h) if holder_reads(st, &h, &aid)? => Some(h),
            _ => None,
        })
    })
    .await
}

/// The access check of a site request (see the module docs): inserts
/// [`SiteCaller`], or answers. A request on no site host passes untouched.
pub async fn site_access_layer(
    State(s): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let Some(site) = req.extensions().get::<SiteRequest>().cloned() else {
        return next.run(req).await;
    };
    let holder = match caller_holder(&s, req.headers(), &site.artifact).await {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let aid = site.artifact.clone();
    let world = match s.store_call(move |st| st.site_open_to_world(&aid)).await {
        Ok(w) => w,
        Err(e) => return e.into_response(),
    };
    let caller = match holder {
        Some(h) => SiteCaller::Holder(h),
        None if world => SiteCaller::Visitor(String::new()),
        None if navigation(req.method(), req.headers()) => {
            let next = req
                .uri()
                .path_and_query()
                .map_or("/", |p| p.as_str())
                .to_string();
            return private(signin_redirect(&s, req.headers(), &safe_next(&next)));
        }
        None => {
            return private(
                ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "site_session_required",
                    "this artifact's site needs a site session: open it from Clax",
                )
                .into_response(),
            );
        }
    };
    req.extensions_mut().insert(caller);
    let res = next.run(req).await;
    if world { res } else { private(res) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_is_host_only_and_http_only_and_partitioned_in_a_frame() {
        let framed = site_cookie(true, true, "v").to_str().unwrap().to_string();
        assert_eq!(
            framed,
            "__Host-clax_site=v; Path=/; HttpOnly; Secure; SameSite=None; Partitioned"
        );
        let top = site_cookie(true, false, "v").to_str().unwrap().to_string();
        assert_eq!(
            top,
            "__Host-clax_site=v; Path=/; HttpOnly; Secure; SameSite=Lax"
        );
        let local = site_cookie(false, false, "v").to_str().unwrap().to_string();
        assert_eq!(local, "clax_site=v; Path=/; HttpOnly; SameSite=Lax");
        assert!(!local.contains("Domain") && !local.contains("Max-Age"));
    }

    #[test]
    fn the_state_cookie_is_sent_to_the_session_alone_for_ten_minutes() {
        assert_eq!(
            state_cookie(true, true, "s").to_str().unwrap(),
            "__Host-clax_site_state=s; Path=/; Max-Age=600; HttpOnly; Secure; SameSite=None; Partitioned"
        );
        assert_eq!(
            state_cookie(false, false, "s").to_str().unwrap(),
            "clax_site_state=s; Path=/; Max-Age=600; HttpOnly; SameSite=Lax"
        );
        assert!(
            clear_state_cookie(false, true)
                .to_str()
                .unwrap()
                .starts_with("clax_site_state=; Path=/; Max-Age=0;")
        );
        assert_ne!(new_state(), new_state());
        // On https the state is `__Host-`: host-only, `Path=/`, never a
        // `Domain`, so no sibling host's cookie can stand for it.
        for partitioned in [false, true] {
            let c = state_cookie(true, partitioned, "s")
                .to_str()
                .unwrap()
                .to_string();
            assert!(c.starts_with("__Host-clax_site_state=s; Path=/;"), "{c}");
            assert!(c.contains("Secure") && !c.contains("Domain"), "{c}");
        }
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("clax_site_state=planted; __Secure-clax_site_state=planted"),
        );
        assert_eq!(state_in(&h, true), None, "a cookie a sibling could set");
        assert_eq!(state_in(&h, false).as_deref(), Some("planted"));
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-clax_site_state=mine"),
        );
        assert_eq!(state_in(&h, true).as_deref(), Some("mine"));
    }

    #[test]
    fn a_private_sites_response_is_private_to_its_browser() {
        let with = |cache: Option<&str>| {
            let mut r = Response::new(Body::empty());
            if let Some(c) = cache {
                r.headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_str(c).unwrap());
            }
            r.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            let r = private(r);
            assert!(
                r.headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .is_none()
            );
            r.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            with(Some("public, max-age=31536000, immutable")),
            "private, max-age=31536000, immutable"
        );
        assert_eq!(with(Some("no-cache")), "private, no-cache");
        assert_eq!(with(Some("no-store")), "no-store");
        assert_eq!(with(None), "private");
    }

    #[test]
    fn next_stays_on_the_site_host() {
        for (given, want) in [
            ("/", "/"),
            ("/_clax/v/3/?x=1", "/_clax/v/3/?x=1"),
            ("https://evil.example/", "/"),
            ("//evil.example/", "/"),
            ("/\\evil.example", "/"),
            ("evil", "/"),
            ("", "/"),
            ("/a\nb", "/"),
        ] {
            assert_eq!(safe_next(given), want, "{given:?}");
        }
        let long = format!("/{}", "a".repeat(MAX_NEXT));
        assert_eq!(safe_next(&long), "/");
        assert_eq!(safe_next(&long[..MAX_NEXT]), &long[..MAX_NEXT]);
    }

    #[test]
    fn a_navigation_is_a_top_level_document_get() {
        let mut h = HeaderMap::new();
        h.insert("accept", HeaderValue::from_static("text/html,*/*"));
        assert!(navigation(&Method::GET, &h));
        assert!(!navigation(&Method::POST, &h));
        h.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        assert!(navigation(&Method::GET, &h), "a client's fetch of a page");
        h.insert("sec-fetch-dest", HeaderValue::from_static("script"));
        assert!(!navigation(&Method::GET, &h));
        h.remove("sec-fetch-dest");
        h.insert("accept", HeaderValue::from_static("application/json"));
        assert!(!navigation(&Method::GET, &h));
        h.insert("sec-fetch-mode", HeaderValue::from_static("navigate"));
        h.insert("sec-fetch-dest", HeaderValue::from_static("document"));
        assert!(navigation(&Method::GET, &h));
        h.insert("sec-fetch-dest", HeaderValue::from_static("iframe"));
        assert!(navigation(&Method::GET, &h) && framed(&h));
        h.insert("sec-fetch-dest", HeaderValue::from_static("image"));
        assert!(!navigation(&Method::GET, &h) && !framed(&h));
        assert!(!navigation(&Method::GET, &HeaderMap::new()));
    }
}

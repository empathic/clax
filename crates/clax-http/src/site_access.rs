//! Who may read an artifact's site, decided on every request to its site
//! host ([`site_access_layer`]).
//!
//! - A site open to the world (`artifacts.site_access`) is read by anyone,
//!   as a [`SiteCaller::Visitor`], unless a site session names its holder.
//!   A visitor is known by the visitor cookie ([`VISITOR_COOKIE`], on
//!   `https` [`SECURE_VISITOR_COOKIE`]: host-only, `HttpOnly`, `Path=/`,
//!   `SameSite=Lax`, a year), which the first navigation or `/_clax/api/`
//!   response without one sets: it holds a secret this daemon minted
//!   (anything else is replaced), from which [`visitor_id`] derives their
//!   ID.
//! - Every response the access decides carries no
//!   `Access-Control-Allow-Origin`, one that sets a cookie is `private,
//!   no-store`, a world visitor's revalidates on every request, and anyone
//!   else's is private to their browser; every site response carries
//!   the site's isolation headers ([`site_response_headers`]).
//! - Any other site needs the site cookie ([`SITE_COOKIE`], on `https`
//!   [`SECURE_SITE_COOKIE`]), naming a site session
//!   ([`clax_store::site_sessions`]) of this artifact whose holder may read
//!   it now ([`holder_reads`]): a share removed, a link turned off or a
//!   browser signed out refuses the very next request. A socket opened on
//!   it keeps [`SiteCredentials`] (the session ID's SHA-256, never the
//!   cookie) and is checked again by them ([`SiteCredentials::check`])
//!   whenever who may read changes (`clax_core::Event::Access`,
//!   `Event::SiteAccess`) and when its host reopens it; its messages renew
//!   the session as requests do (at most once every
//!   `clax_store::site_sessions::SITE_SESSION_RENEW_SECS`).
//! - Without one, a navigation (top-level, or of a frame) is sent to the
//!   public host's `/site-signin?host=<host>&next=<path>`, which makes a
//!   session when the browser may read the artifact (partitioned for a
//!   frame), except a frame that sends none of the site's cookies, which
//!   gets [`no_cookie_page`] (the browser keeps them from it, so that
//!   sign-in could not land); anything else answers 401
//!   `site_session_required`.
//!
//! The cookie is host-only, `HttpOnly` and `Path=/`, and lasts as long as
//! the browser's session; its session row lasts an hour from its last use.
//! Made for the shell's frame (a third-party context on a site domain of
//! its own) it is `SameSite=None; Secure; Partitioned`, keyed to the
//! shell's site; made for a top-level visit, `SameSite=Lax`.
//!
//! Every code is bound to a **state** the redeeming browser holds in a
//! cookie of the site host ([`STATE_COOKIE`], `Path=/_clax/session`, ten
//! minutes, shared by the browser's handshakes, [`state_for`]):
//! `/_clax/session` redeems a code
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

/// The visitor cookie's name on an `http` site (a local daemon).
pub const VISITOR_COOKIE: &str = "clax_visitor";
/// The visitor cookie's name on an `https` site.
pub const SECURE_VISITOR_COOKIE: &str = "__Host-clax_visitor";
/// How long a visitor cookie lasts: a year.
pub const VISITOR_MAX_AGE_S: u64 = 365 * 24 * 60 * 60;

/// Who a site request reads as ([`site_access_layer`] inserts it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteCaller {
    /// A site session's holder, who may read the artifact.
    Holder(Holder),
    /// Anyone, on a site open to the world: their visitor ID
    /// ([`visitor_id`]), stable per browser and per site.
    Visitor(String),
}

/// The site session a request's [`SiteCaller::Holder`] was found by: its
/// ID's SHA-256 (lowercase hex). [`site_access_layer`] inserts it beside
/// the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteSessionHash(pub String);

/// What a socket opened on a site host keeps to check its caller again
/// (after a change to who may read, or when its host reopens it): a
/// holder's site session by its ID's SHA-256, never the cookie; a world
/// visitor's visitor ID.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteCredentials {
    Session(String),
    Visitor(String),
}

impl SiteCredentials {
    /// The credentials of a site request whose caller is `caller`, from
    /// its `extensions` ([`SiteSessionHash`]); `None` for a holder found by
    /// no site session.
    pub fn of(caller: &SiteCaller, extensions: &axum::http::Extensions) -> Option<SiteCredentials> {
        match caller {
            SiteCaller::Holder(_) => extensions
                .get::<SiteSessionHash>()
                .map(|h| SiteCredentials::Session(h.0.clone())),
            SiteCaller::Visitor(v) => Some(SiteCredentials::Visitor(v.clone())),
        }
    }

    /// Whether these are a world visitor's.
    pub fn visitor(&self) -> bool {
        matches!(self, SiteCredentials::Visitor(_))
    }

    /// Who these let in on artifact `aid`'s site now, as
    /// [`site_access_layer`] would decide a request: a site session that is
    /// live (renewed by this check) and whose holder reads `aid`
    /// ([`holder_reads`]); a visitor while the site is open to the world.
    /// `None` when they no longer let anyone in.
    ///
    /// # Errors
    /// The store's.
    pub fn check(
        &self,
        st: &clax_store::Store,
        aid: &ArtifactId,
    ) -> clax_core::Result<Option<SiteCaller>> {
        match self {
            SiteCredentials::Session(hash) => Ok(match st.site_session_of_hash(hash, aid)? {
                Some(h) if holder_reads(st, &h, aid)? => Some(SiteCaller::Holder(h)),
                _ => None,
            }),
            SiteCredentials::Visitor(v) => Ok(st
                .site_open_to_world(aid)?
                .then(|| SiteCaller::Visitor(v.clone()))),
        }
    }
}

/// The visitor cookie's name: [`SECURE_VISITOR_COOKIE`] when `secure`, else
/// [`VISITOR_COOKIE`].
pub fn visitor_cookie_name(secure: bool) -> &'static str {
    if secure {
        SECURE_VISITOR_COOKIE
    } else {
        VISITOR_COOKIE
    }
}

/// `Set-Cookie` for visitor secret `value`: host-only, `Path=/`, `HttpOnly`,
/// `SameSite=Lax` (`Secure` when `secure`), for [`VISITOR_MAX_AGE_S`].
pub fn visitor_cookie(secure: bool, value: &str) -> HeaderValue {
    let name = visitor_cookie_name(secure);
    let tail = if secure {
        "; Secure; SameSite=Lax"
    } else {
        "; SameSite=Lax"
    };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; Max-Age={VISITOR_MAX_AGE_S}; HttpOnly{tail}"
    ))
    .expect("visitor secrets are header-safe")
}

/// What the visitor cookie's secret is keyed by ([`crate::hosted::keyed_mac`]).
const VISITOR_SECRET_LABEL: &str = "clax site visitor cookie";
/// What a visitor ID is keyed by.
const VISITOR_ID_LABEL: &str = "clax site visitor ID";

/// A new visitor secret, minted by this daemon: 22 base64url characters
/// (16 random bytes), a `.`, and their MAC under a key made from the daemon
/// token `token`.
pub fn mint_visitor_secret(token: &str) -> String {
    let nonce = clax_core::secret::token("").expose()[..22].to_string();
    let mac = crate::hosted::keyed_mac(token, VISITOR_SECRET_LABEL, &nonce);
    format!("{nonce}.{mac}")
}

/// Whether `v` is a visitor secret this daemon minted
/// ([`mint_visitor_secret`]): anything else, a value a client chose
/// among them, is no visitor's and is replaced.
pub fn visitor_secret_ok(token: &str, v: &str) -> bool {
    let Some((nonce, mac)) = v.split_once('.') else {
        return false;
    };
    nonce.len() == 22
        && nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && clax_core::secret::constant_time_eq(
            mac.as_bytes(),
            crate::hosted::keyed_mac(token, VISITOR_SECRET_LABEL, nonce).as_bytes(),
        )
}

/// The visitor ID visitor secret `secret` stands for on artifact `aid`'s
/// site: `v_` and 22 lowercase hex characters of a MAC of both under a key
/// made from the daemon token `token`. The cookie holds the secret, never
/// the ID, so an ID a page shows (a document's path, the room's presence)
/// cannot be replayed as a cookie, and one browser's IDs on two sites are
/// unrelated.
pub fn visitor_id(token: &str, aid: &ArtifactId, secret: &str) -> String {
    let msg = format!("{secret} {}", aid.as_str());
    let mac = crate::hosted::keyed_mac(token, VISITOR_ID_LABEL, &msg);
    format!("v_{}", &mac[..22])
}

/// A site response's caching and CORS headers, by who it was answered to,
/// in one place: it never carries `Access-Control-Allow-Origin`; one that
/// sets a cookie is `private, no-store`; a visitor's on a site open to the
/// world (`world_visitor`) is `public, no-cache`, a version's files and an
/// asset's `immutable` included, so a shared cache revalidates every one
/// and a site made private is refused on the next request;
/// anyone else's is private to their browser (`public` becomes `private`,
/// `no-cache` `private, no-cache`; `no-store` stays). A world visitor's
/// public answer varies by `Cookie`; the access layer never makes the page
/// runtime's (`/_clax/api/`) public.
fn cache_headers(h: &mut HeaderMap, world_visitor: bool) {
    h.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    if h.contains_key(header::SET_COOKIE) {
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        return;
    }
    if !world_visitor {
        return;
    }
    let cc = h
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let short = if cc.contains("no-store") || cc.contains("private") {
        None
    } else if world_visitor {
        // Shared, but revalidated on every request: the site may be made
        // private at any moment, and a CDN may ignore `Vary: Cookie` (a
        // holder's cookie makes a request theirs).
        h.append(header::VARY, HeaderValue::from_static("Cookie"));
        Some("public, no-cache".to_string())
    } else {
        Some("public, no-cache")
    };
    if let Some(v) = short {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(v));
    }
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

/// The handshake state for a request with `headers`: the browser's own,
/// when its state cookie holds one ([`new_state`]'s form, 43 base64url
/// characters), else a new one. A browser's handshakes share their state,
/// so two of its tabs signing in at once never overwrite each other's (the
/// one cookie per host and partition holds one); a state is the browser's
/// alone either way, and each code it is bound to redeems once.
pub fn state_for(headers: &HeaderMap, secure: bool) -> String {
    cookie_value(headers, state_cookie_name(secure))
        .filter(|v| {
            v.len() == 43
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .unwrap_or_else(new_state)
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

/// The origins of the shell that frames the pages of the site whose `Host`
/// is `host`: a hosted daemon's public origin; on a local daemon the
/// shell's loopback names `localhost` and `127.0.0.1` at the
/// `Host`'s port (CSP names no IPv6 literal, so a shell at `[::1]` frames
/// no site).
pub fn shell_origins(s: &AppState, host: &str) -> Vec<String> {
    shell_origins_of(hosted_origin(s), host)
}

/// The public origin of a hosted daemon, `None` on a local one.
fn hosted_origin(s: &AppState) -> Option<&str> {
    s.hosted.as_ref().map(|h| h.origin.as_str())
}

/// [`shell_origins`] from the hosted public origin (`None` on a local
/// daemon).
fn shell_origins_of(hosted: Option<&str>, host: &str) -> Vec<String> {
    if let Some(origin) = hosted {
        return vec![origin.to_string()];
    }
    let port = host
        .rsplit_once(':')
        .filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .map(|(_, p)| format!(":{p}"))
        .unwrap_or_default();
    ["localhost", "127.0.0.1"]
        .iter()
        .map(|n| format!("http://{n}{port}"))
        .collect()
}

/// `frame-ancestors 'self' <shell origins>` for the site whose `Host` is
/// `host` ([`shell_origins`]): only the artifact's own pages and the shell
/// may frame a site page.
pub fn frame_ancestors(s: &AppState, host: &str) -> String {
    frame_ancestors_of(hosted_origin(s), host)
}

/// [`frame_ancestors`] from the hosted public origin.
fn frame_ancestors_of(hosted: Option<&str>, host: &str) -> String {
    let mut p = String::from("frame-ancestors 'self'");
    for o in shell_origins_of(hosted, host) {
        p.push(' ');
        p.push_str(&o);
    }
    p
}

/// `max-age` of `Strict-Transport-Security` on an `https` site: a year.
pub const HSTS: &str = "max-age=31536000; includeSubDomains";

/// `Cross-Origin-Resource-Policy`, which `http` names no constant for.
pub const CORP: axum::http::HeaderName =
    axum::http::HeaderName::from_static("cross-origin-resource-policy");

/// The headers every response of the site whose `Host` is `host` carries
/// (the site layer, [`crate::host::site_layer`], sets them, an error's
/// included): `Referrer-Policy: no-referrer`, `X-Content-Type-Options:
/// nosniff`, `Cross-Origin-Resource-Policy: same-origin` unless the
/// response named one (a site open to the world's are `cross-origin`,
/// [`site_access_layer`]), `Strict-Transport-Security` ([`HSTS`]) on
/// `https`, and on an HTML response without a policy of its own
/// `Content-Security-Policy:` [`frame_ancestors`].
pub fn site_response_headers(s: &AppState, host: &str, res: &mut Response) {
    let answer = res.extensions().get::<SiteAnswer>().copied();
    site_headers(hosted_origin(s), secure(s), host, answer, res.headers_mut());
}

/// [`site_response_headers`] on response headers `h`, for a server that
/// holds no [`AppState`] (a Worker serving the UI's files from its static
/// assets): `hosted` is a hosted daemon's public origin (`None` on a local
/// one), `secure` whether sites are served on `https`, `answer` what the
/// access layer decided, if it decided the response.
pub fn site_headers(
    hosted: Option<&str>,
    secure: bool,
    host: &str,
    answer: Option<SiteAnswer>,
    h: &mut HeaderMap,
) {
    if let Some(a) = answer {
        cache_headers(h, a.public);
        if a.world {
            h.insert(CORP, HeaderValue::from_static("cross-origin"));
        }
    }
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if !h.contains_key(CORP) {
        h.insert(CORP, HeaderValue::from_static("same-origin"));
    }
    if secure {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        );
    }
    let html = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.trim_start().to_ascii_lowercase().starts_with("text/html"));
    if html
        && !h.contains_key(header::CONTENT_SECURITY_POLICY)
        && let Ok(v) = HeaderValue::from_str(&frame_ancestors_of(hosted, host))
    {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
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

/// A site's 302 to the public host's `/site-signin` for `next`: the
/// handshake's state ([`state_for`]), set in the state cookie (partitioned for a frame,
/// `headers` being a framed navigation's) and named in the address.
pub fn signin_redirect(s: &AppState, headers: &HeaderMap, next: &str) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let state = state_for(headers, secure(s));
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

/// Whether `headers` are a framed request's that carries none of the site's
/// cookies (its site cookie, its state cookie): the browser keeps them
/// from the frame, as one that refuses a third-party context's cookies,
/// partitioned or not, does. Every framed handshake sets the state cookie
/// first (`/_clax/session/start`) and a redemption the site cookie, so a
/// frame whose browser keeps cookies sends one of them.
pub fn cookieless_frame(s: &AppState, headers: &HeaderMap) -> bool {
    let secure = secure(s);
    framed(headers)
        && cookie_value(headers, site_cookie_name(secure)).is_none()
        && state_in(headers, secure).is_none()
}

/// How a site sends a navigation without a session on to sign in for
/// `next`: [`signin_redirect`], or for a frame without the site's cookies
/// ([`cookieless_frame`]), where that sign-in could not land,
/// [`no_cookie_page`].
pub fn signin(s: &AppState, headers: &HeaderMap, next: &str) -> Response {
    if cookieless_frame(s, headers) {
        no_cookie_page(s, headers)
    } else {
        signin_redirect(s, headers, next)
    }
}

/// The document a frame without the site's cookies gets
/// ([`cookieless_frame`]): it tells the shell's origins alone
/// (`postMessage` of `{"type": "clax:no-cookie"}`, [`shell_origins`]) and
/// answers the shell's `{"type": "clax:storage-access"}` with
/// `document.requestStorageAccess()`'s outcome (`{"type":
/// "clax:storage-access", "granted"}`), to the shell's window and origin
/// alone. It runs only its own script (a nonce), only the shell may frame
/// it, and it says nothing of the artifact.
pub fn no_cookie_page(s: &AppState, headers: &HeaderMap) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let shells = shell_origins(s, host);
    let nonce = new_state();
    // Inside a script element: no `<` may close it.
    let list = serde_json::json!(shells)
        .to_string()
        .replace('<', "\\u003c");
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><script nonce=\"{nonce}\">const S={list};\
addEventListener(\"message\",e=>{{if(e.source!==parent||!S.includes(e.origin)||!e.data||e.data.type!==\"clax:storage-access\")return;\
Promise.resolve(document.requestStorageAccess?.()).then(()=>!!document.requestStorageAccess,()=>!1)\
.then(g=>parent.postMessage({{type:\"clax:storage-access\",granted:g}},e.origin))}});\
for(const o of S)parent.postMessage({{type:\"clax:no-cookie\"}},o);</script>"
    );
    let mut res = axum::response::Html(html).into_response();
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; frame-ancestors {}",
        shells.join(" ")
    )) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
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
/// they may read it now, and the session's [`SiteSessionHash`].
///
/// # Errors
/// The store's.
pub async fn caller_holder(
    s: &AppState,
    headers: &HeaderMap,
    aid: &ArtifactId,
) -> Result<Option<(Holder, SiteSessionHash)>, ApiError> {
    let Some(id) = cookie_value(headers, site_cookie_name(secure(s))) else {
        return Ok(None);
    };
    let hash = clax_core::secret::sha256_hex(&id);
    let aid = aid.clone();
    s.store_call(move |st| {
        Ok(match st.site_session_of_hash(&hash, &aid)? {
            Some(h) if holder_reads(st, &h, &aid)? => Some((h, SiteSessionHash(hash))),
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
    if let Some(h) = holder {
        req.extensions_mut().insert(SiteCaller::Holder(h));
        let mut res = next.run(req).await;
        site_response_headers(&mut res, false);
        return res;
    }
    let aid = site.artifact.clone();
    let world = match s.store_call(move |st| st.site_open_to_world(&aid)).await {
        Ok(w) => w,
        Err(e) => return e.into_response(),
    };
    let caller = match holder {
        Some((h, hash)) => {
            req.extensions_mut().insert(hash);
            SiteCaller::Holder(h)
        }
        None if world => {
            let path = req.uri().path().to_string();
            let secure = secure(&s);
            let held = cookie_value(req.headers(), visitor_cookie_name(secure))
                .filter(|v| visitor_secret_ok(&s.token, v));
            // Only a navigation and the runtime's own calls set the cookie: a
            // file or an asset a shared cache may keep never does. Two first
            // requests at once each set one; the browser keeps the last set.
            let mints = held.is_none()
                && (navigation(req.method(), req.headers())
                    || req.uri().path().starts_with("/_clax/api/"));
            let secret = held.unwrap_or_else(|| mint_visitor_secret(&s.token));
            req.extensions_mut().insert(SiteCaller::Visitor(visitor_id(
                &s.token,
                &site.artifact,
                &secret,
            )));
            let mut res = next.run(req).await;
            if mints {
                res.headers_mut()
                    .append(header::SET_COOKIE, visitor_cookie(secure, &secret));
            }
            // The runtime's answers are the caller's own (its documents,
            // its whoami): never a shared cache's.
            let public = !path.starts_with("/_clax/api/");
            site_response_headers(&mut res, public);
            return res;
        }
        None if navigation(req.method(), req.headers()) => {
            let next = req
                .uri()
                .path_and_query()
                .map_or("/", |p| p.as_str())
                .to_string();
            let mut res = signin(&s, req.headers(), &safe_next(&next));
            res.extensions_mut().insert(SiteAnswer::default());
            return res;
        }
        None => {
            let mut res = ApiError::new(
                StatusCode::UNAUTHORIZED,
                "site_session_required",
                "this artifact's site needs a site session: open it from Clax",
            )
            .into_response();
            res.extensions_mut().insert(SiteAnswer::default());
            return res;
        }
    };
    req.extensions_mut().insert(caller);
    let mut res = next.run(req).await;
    if !world {
        return private(res);
    }
    // A public site's images and files may be embedded elsewhere.
    res.headers_mut()
        .insert(CORP, HeaderValue::from_static("cross-origin"));
    res
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
            let mut r = r;
            cache_headers(r.headers_mut(), false);
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
    fn the_visitor_cookie_lasts_a_year_and_holds_a_secret_not_the_id() {
        let c = visitor_cookie(true, "s").to_str().unwrap().to_string();
        assert_eq!(
            c,
            "__Host-clax_visitor=s; Path=/; Max-Age=31536000; HttpOnly; Secure; SameSite=Lax"
        );
        let local = visitor_cookie(false, "s").to_str().unwrap().to_string();
        assert_eq!(
            local,
            "clax_visitor=s; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax"
        );
        let secret = mint_visitor_secret("tok");
        assert!(visitor_secret_ok("tok", &secret));
        assert!(!visitor_secret_ok("another token", &secret));
        let (nonce, _) = secret.split_once('.').unwrap();
        for forged in [
            nonce.to_string(),
            format!("{nonce}.00000000000000000000000000000000"),
            "v_0123456789abcdef012345".to_string(),
            clax_core::secret::token("").expose().to_string(),
        ] {
            assert!(!visitor_secret_ok("tok", &forged), "{forged}");
        }
        let a = ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        let b = ArtifactId::parse("8q3k9mzx2b4t").unwrap();
        let id = visitor_id("tok", &a, &secret);
        assert!(id.starts_with("v_") && id.len() == 24, "{id}");
        assert_eq!(id, visitor_id("tok", &a, &secret));
        assert_ne!(id, visitor_id("tok", &b, &secret));
        assert!(clax_core::db::doc_path(&format!("data/users/{id}/n")).is_ok());
    }

    #[test]
    fn a_world_visitors_answer_revalidates_every_time_and_a_cookie_is_private() {
        let after = |cc: &str, cookie: bool, world: bool| {
            let mut r = Response::new(Body::empty());
            let h = r.headers_mut();
            h.insert(header::CACHE_CONTROL, HeaderValue::from_str(cc).unwrap());
            h.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            if cookie {
                h.insert(header::SET_COOKIE, HeaderValue::from_static("a=b"));
            }
            cache_headers(r.headers_mut(), world);
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
        let year = "public, max-age=31536000, immutable";
        assert_eq!(after("no-cache", false, true), "public, no-cache");
        let mut r = Response::new(Body::empty());
        r.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        cache_headers(r.headers_mut(), true);
        assert_eq!(
            r.headers()[header::VARY],
            "Cookie",
            "a public answer varies by Cookie"
        );
        assert_eq!(after(year, false, true), "public, no-cache");
        assert_eq!(after("no-store", false, true), "no-store");
        assert_eq!(after("no-cache", true, true), "private, no-store");
        assert_eq!(after(year, true, false), "private, no-store");
        assert_eq!(
            after("no-cache", false, false),
            "no-cache",
            "a holder's stays"
        );
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

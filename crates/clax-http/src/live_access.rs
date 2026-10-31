//! Which requests may see live pages (spec
//! 2026-10-05-chrome-overlay-design L10): the rule, the extractor for
//! handlers that take an artifact ID outside the path, and the middleware
//! that answers 404 for the rest.

use crate::auth::{has_token, loopback_peer};
use crate::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap};
use clax_core::CoreError;
use clax_realtime::live::LiveIds;

/// Whether the request may see live pages: it came from a loopback peer
/// ([`loopback_peer`]: never on a hosted daemon, which a proxy on its
/// machine may reach from loopback for anyone), or it carries the token;
/// never with an agent credential of another machine, whatever its peer
/// (spec §7.5). (Unlike [`crate::identity::is_local`], the `Host` does not
/// matter: the `/api` host rule applies on its own.)
pub fn sees_live_pages(headers: &HeaderMap, ext: &Extensions, token: &str) -> bool {
    !crate::auth::is_grant_request(headers) && (has_token(headers, token) || loopback_peer(ext))
}

/// Whether `path` is one of the live-page routes (`/api/live/…`), which
/// only a caller that may see live pages reaches.
pub fn live_route(path: &str) -> bool {
    path.starts_with("/api/live/")
}

/// Which artifacts a request may name, for handlers that take an artifact ID
/// outside the path (a query or a body): live pages only when
/// [`sees_live_pages`]; and nothing but live pages when the extension
/// gateway admitted it ([`crate::extension::ViaExtension`]).
pub struct SeesLive {
    /// The request may see live pages ([`sees_live_pages`]).
    pub may_see: bool,
    /// The request may name live pages alone (the extension's).
    pub live_only: bool,
}

impl FromRequestParts<crate::state::AppState> for SeesLive {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(
        parts: &mut Parts,
        s: &crate::state::AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(SeesLive {
            may_see: sees_live_pages(&parts.headers, &parts.extensions, &s.token),
            live_only: parts
                .extensions
                .get::<crate::extension::ViaExtension>()
                .is_some(),
        })
    }
}

impl SeesLive {
    /// 404 `not_found`, as for a missing artifact, when `id` is a live page
    /// this request may not see, or is not a live page and the request may
    /// name live pages alone.
    ///
    /// # Errors
    /// That 404.
    pub fn check(&self, ids: &LiveIds, id: &str) -> Result<(), ApiError> {
        let live = ids.contains(id);
        if (live && !self.may_see) || (!live && self.live_only) {
            Err(CoreError::NotFound.into())
        } else {
            Ok(())
        }
    }
}

/// `seg` percent-decoded as the router decodes a path parameter; `None`
/// when it does not decode to UTF-8 (the router refuses it too).
pub(crate) fn percent_decoded(seg: &str) -> Option<String> {
    if !seg.contains('%') {
        return Some(seg.to_string());
    }
    let b = seg.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).copied().and_then(hex),
            b.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push(u8::try_from(h * 16 + l).unwrap_or(0));
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The artifact a path names: `/api/artifacts/<aid>…`,
/// `/api/hosted/artifacts/<aid>…` (hosted mode's sharing routes) or
/// `/a/<aid>…`, decoded as the handlers decode it (so `%37…` names the
/// same artifact as `7…`).
pub fn artifact_in(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("/api/artifacts/")
        .or_else(|| path.strip_prefix("/api/hosted/artifacts/"))
        .or_else(|| path.strip_prefix("/a/"))?;
    let seg = percent_decoded(rest.split('/').next().unwrap_or(""))?;
    Some(seg.split(':').next().unwrap_or("").to_string())
}

/// Middleware: a live page's API, content and shell paths, and the
/// `/api/live/…` routes, answer 404 to a caller [`sees_live_pages`]
/// refuses, exactly as a missing artifact does.
/// A site host's requests (`<aid>.localhost`) reach the site routes, which
/// hide a live page's site themselves (`clax_server::routes::site`).
pub async fn hide_live_pages(
    axum::extract::State(s): axum::extract::State<crate::state::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = req.uri().path();
    let names_live =
        live_route(path) || artifact_in(path).is_some_and(|aid| s.live_ids.contains(&aid));
    if names_live && !sees_live_pages(req.headers(), req.extensions(), &s.token) {
        return crate::error::ApiError::from(clax_core::CoreError::NotFound).into_response();
    }
    next.run(req).await
}

/// Whether a request speaks for the owner or one of the owner's agents,
/// read from its headers and marks alone: it carries the daemon token, an
/// owner browser session that counts
/// ([`crate::local_trust::session_counts`]: the in-process app's frame
/// included), or the extension gateway admitted it
/// ([`crate::extension::ViaExtension`]: the owner's browser); never with
/// an agent credential of another machine.
pub fn owners_own(headers: &HeaderMap, ext: &Extensions, token: &str) -> bool {
    if crate::auth::is_grant_request(headers) {
        return false;
    }
    has_token(headers, token)
        || (ext.get::<crate::local_trust::OwnerSession>().is_some()
            && crate::local_trust::session_counts(headers, ext))
        || ext.get::<crate::extension::ViaExtension>().is_some()
}

/// The artifact a session's watch path names
/// (`/api/sessions/<sid>/watches/<aid>`), decoded as [`artifact_in`]
/// decodes one.
fn watched_in(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (_, rest) = rest.split_once('/')?;
    let aid = rest.strip_prefix("watches/")?;
    percent_decoded(aid.split('/').next().unwrap_or(""))
}

/// Middleware of a local daemon: a followed remote artifact's mirror
/// (`artifacts.remote_mirror`, `clax_store::remotes`) holds threads a
/// remote shared with the owner alone, so its API and shell paths, and a
/// session's watch of it, answer
/// 404, exactly as a missing artifact does, to every caller but the owner
/// and the owner's agents ([`owners_own`]), whatever their rights. Only a
/// request that is not the owner's and names an artifact reads the store.
/// A hosted daemon keeps no mirrors.
pub async fn hide_mirrors(
    axum::extract::State(s): axum::extract::State<crate::state::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if s.hosted.is_none()
        && let Some(aid) = artifact_in(req.uri().path()).or_else(|| watched_in(req.uri().path()))
        && !owners_own(req.headers(), req.extensions(), &s.token)
    {
        match s.store_call(move |st| st.is_remote_mirror(&aid)).await {
            Ok(false) => {}
            Ok(true) => {
                return crate::error::ApiError::from(clax_core::CoreError::NotFound)
                    .into_response();
            }
            Err(e) => return e.into_response(),
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Conn;
    use axum::extract::ConnectInfo;

    #[test]
    fn the_artifact_a_watch_path_names() {
        assert_eq!(
            watched_in("/api/sessions/s1/watches/7q3k9mzx2b4t").as_deref(),
            Some("7q3k9mzx2b4t")
        );
        assert_eq!(
            watched_in("/api/sessions/s1/watches/%37q3k9mzx2b4t").as_deref(),
            Some("7q3k9mzx2b4t")
        );
        assert_eq!(watched_in("/api/sessions/s1/watches"), None);
        assert_eq!(watched_in("/api/sessions/s1"), None);
    }

    #[test]
    fn the_artifact_a_path_names() {
        for (p, want) in [
            ("/api/artifacts/7q3k9mzx2b4t", Some("7q3k9mzx2b4t")),
            (
                "/api/artifacts/7q3k9mzx2b4t/threads/x",
                Some("7q3k9mzx2b4t"),
            ),
            (
                "/api/artifacts/7q3k9mzx2b4t/threads:send",
                Some("7q3k9mzx2b4t"),
            ),
            ("/api/artifacts/7q3k9mzx2b4t:x", Some("7q3k9mzx2b4t")),
            ("/c/7q3k9mzx2b4t/v/1/", None),
            ("/a/7q3k9mzx2b4t", Some("7q3k9mzx2b4t")),
            (
                "/api/hosted/artifacts/7q3k9mzx2b4t/link",
                Some("7q3k9mzx2b4t"),
            ),
            ("/api/artifacts", None),
            ("/api/threads", None),
            ("/_clax/bridge.js", None),
            (
                "/api/artifacts/%37q3k9mzx2b4t/threads",
                Some("7q3k9mzx2b4t"),
            ),
            ("/a/%37%71%33k9mzx2b4t", Some("7q3k9mzx2b4t")),
            ("/a/7q3k9mzx2b4t%3Ax", Some("7q3k9mzx2b4t")),
        ] {
            assert_eq!(artifact_in(p).as_deref(), want, "{p}");
        }
        assert_eq!(artifact_in("/a/%ff"), None);
    }

    #[test]
    fn loopback_or_the_token_sees_live_pages() {
        let conn = |peer: &str| {
            let mut e = Extensions::new();
            e.insert(ConnectInfo(Conn {
                peer: peer.parse().unwrap(),
                local: "0.0.0.0:7480".parse().unwrap(),
            }));
            e
        };
        let none = HeaderMap::new();
        let mut token = HeaderMap::new();
        token.insert("authorization", "Bearer tok".parse().unwrap());
        assert!(sees_live_pages(&none, &conn("127.0.0.1:5000"), "tok"));
        assert!(sees_live_pages(&none, &conn("[::1]:5000"), "tok"));
        assert!(!sees_live_pages(&none, &conn("192.168.1.9:5000"), "tok"));
        assert!(sees_live_pages(&token, &conn("192.168.1.9:5000"), "tok"));
        assert!(!sees_live_pages(&token, &conn("192.168.1.9:5000"), "other"));
        assert!(!sees_live_pages(&none, &Extensions::new(), "tok"));
    }

    #[test]
    fn a_hosted_request_from_loopback_sees_no_live_pages_without_the_token() {
        let mut ext = Extensions::new();
        ext.insert(ConnectInfo(Conn {
            peer: "127.0.0.1:5000".parse().unwrap(),
            local: "127.0.0.1:7490".parse().unwrap(),
        }));
        ext.insert(crate::hosted::HostedRequest::default());
        let none = HeaderMap::new();
        let mut token = HeaderMap::new();
        token.insert("authorization", "Bearer tok".parse().unwrap());
        assert!(!sees_live_pages(&none, &ext, "tok"), "a same-machine proxy");
        assert!(sees_live_pages(&token, &ext, "tok"));
    }
}

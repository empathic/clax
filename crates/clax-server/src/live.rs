//! Live pages on the daemon (spec 2026-10-05-chrome-overlay-design L10):
//! the set of live-page artifact IDs, and the rule that hides them from
//! callers that are neither on this machine nor hold the token.

use crate::auth::{Conn, has_token, is_loopback};
use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap};
use clax_core::Store;
use std::collections::HashSet;
use std::sync::RwLock;

/// Every live page's artifact ID, kept in step with the store: loaded at
/// start, added to when a comment creates a page, removed from on delete.
#[derive(Default)]
pub struct LiveIds(RwLock<HashSet<String>>);

impl LiveIds {
    /// The live pages the store holds now.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<LiveIds> {
        Ok(LiveIds(RwLock::new(
            st.live_page_ids()?.into_iter().collect(),
        )))
    }

    /// Records `id` as a live page.
    pub fn insert(&self, id: &str) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.to_string());
    }

    /// Forgets `id` (its artifact was deleted).
    pub fn remove(&self, id: &str) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
    }

    /// Whether `id` is a live page.
    pub fn contains(&self, id: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(id)
    }
}

/// Whether the request may see live pages: it came from a loopback peer, or
/// it carries the token. (Unlike [`crate::identity::is_local`], the `Host`
/// does not matter: the `/api` host rule applies on its own.)
pub fn sees_live_pages(headers: &HeaderMap, ext: &Extensions, token: &str) -> bool {
    has_token(headers, token)
        || ext
            .get::<ConnectInfo<Conn>>()
            .is_some_and(|c| is_loopback(c.0.peer))
}

/// Whether `path` is one of the live-page routes (`/api/live/…`), which
/// only a caller that may see live pages reaches.
fn live_route(path: &str) -> bool {
    path.starts_with("/api/live/")
}

/// The artifact a path names: `/api/artifacts/<aid>…`, `/c/<aid>/…`, `/a/<aid>…`.
fn artifact_in(path: &str) -> Option<&str> {
    let rest = path
        .strip_prefix("/api/artifacts/")
        .or_else(|| path.strip_prefix("/c/"))
        .or_else(|| path.strip_prefix("/a/"))?;
    Some(rest.split(['/', ':']).next().unwrap_or(""))
}

/// Middleware: a live page's API, content and shell paths, and the
/// `/api/live/…` routes, answer 404 to a caller [`sees_live_pages`]
/// refuses, exactly as a missing artifact does.
/// Requests to an artifact host (`<aid>.localhost`) reach it rewritten to
/// `/c/<aid>/…`, so they are covered too.
pub async fn hide_live_pages(
    axum::extract::State(s): axum::extract::State<crate::state::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = req.uri().path();
    let names_live =
        live_route(path) || artifact_in(path).is_some_and(|aid| s.live_ids.contains(aid));
    if names_live && !sees_live_pages(req.headers(), req.extensions(), &s.token) {
        return crate::error::ApiError::from(clax_core::CoreError::NotFound).into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("/c/7q3k9mzx2b4t/v/1/", Some("7q3k9mzx2b4t")),
            ("/a/7q3k9mzx2b4t", Some("7q3k9mzx2b4t")),
            ("/api/artifacts", None),
            ("/api/threads", None),
            ("/_clax/bridge.js", None),
        ] {
            assert_eq!(artifact_in(p), want, "{p}");
        }
    }

    #[test]
    fn live_ids_follow_inserts_and_removes() {
        let ids = LiveIds::default();
        assert!(!ids.contains("a"));
        ids.insert("a");
        assert!(ids.contains("a"));
        ids.remove("a");
        assert!(!ids.contains("a"));
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
}

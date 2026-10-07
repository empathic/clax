//! Live pages on the daemon (spec 2026-10-05-chrome-overlay-design L10):
//! the set of live-page artifact IDs, and the rule that hides them from
//! callers that are neither on this machine nor hold the token.

use crate::auth::{Conn, has_token, is_loopback};
use crate::error::ApiError;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap};
use clax_core::live::PageKey;
use clax_core::store::live::EnsuredPage;
use clax_core::{CoreError, Store};
use std::collections::HashMap;
use std::sync::RwLock;

/// Every live page's artifact ID, with its origin (its site's key), kept in
/// step with the store: loaded at start, added to by
/// [`LiveIds::ensure_page`] (the one way the server makes a live page),
/// removed from on delete; and the joined sites (spec
/// 2026-10-05-chrome-overlay-design §7.2): each joined origin's site key,
/// and each site's origins, the most recently used first, reloaded after a
/// join or a split ([`LiveIds::reload_sites`]).
#[derive(Default)]
pub struct LiveIds(RwLock<HashMap<String, String>>, RwLock<Sites>);

/// The joined sites as [`LiveIds`] keeps them.
#[derive(Default)]
struct Sites {
    /// Each joined origin's site key.
    key_of: HashMap<String, String>,
    /// Each joined site's origins with when Clax last used them, the most
    /// recently used first.
    origins: HashMap<String, Vec<(String, String)>>,
    /// When each joined origin's use was last written to the store.
    written: HashMap<String, std::time::Instant>,
}

/// How often a joined origin's use is written to the store at most.
pub const TOUCH_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

impl LiveIds {
    /// The live pages and joined sites the store holds now.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<LiveIds> {
        // Pages a join merged away stay live pages: hidden from the LAN.
        let ids = LiveIds(
            RwLock::new(
                st.live_pages()?
                    .into_iter()
                    .map(|p| (p.artifact_id, p.origin))
                    .chain(
                        st.merged_live_pages()?
                            .into_iter()
                            .map(|m| (m.artifact_id, m.origin)),
                    )
                    .collect(),
            ),
            RwLock::default(),
        );
        ids.reload_sites(st)?;
        Ok(ids)
    }

    /// [`Store::ensure_live_page_linking`] (a version it writes links the
    /// threads of `pending` still pending on the page), recording the page
    /// in this set before anything announces it. Every server path that finds or creates a
    /// live page goes through here, so none can leave one out of the set.
    ///
    /// # Errors
    /// The store's.
    pub fn ensure_page(
        &self,
        st: &Store,
        key: &PageKey,
        title: &str,
        snapshot: Option<&[u8]>,
        pending: &[String],
    ) -> clax_core::Result<EnsuredPage> {
        let e = st.ensure_live_page_linking(key, title, snapshot, pending)?;
        self.insert(&e.artifact.id, &e.origin);
        Ok(e)
    }

    /// Records `id` as a live page of `origin`.
    pub fn insert(&self, id: &str, origin: &str) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.to_string(), origin.to_string());
    }

    /// The origin of the live page `id` (its site's key), or `None` for any
    /// other artifact.
    pub fn origin_of(&self, id: &str) -> Option<String> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
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
            .contains_key(id)
    }

    /// Reads the joined sites from the store again, and records the live
    /// pages of `rekeyed` (a join or split re-keyed them) under their
    /// origin now.
    ///
    /// # Errors
    /// The store's.
    pub fn reload_sites(&self, st: &Store) -> clax_core::Result<()> {
        let rows = st.site_memberships()?;
        let mut g = self
            .1
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.key_of.clear();
        g.origins.clear();
        for (origin, key, used) in rows {
            g.key_of.insert(origin.clone(), key.clone());
            g.origins.entry(key).or_default().push((origin, used));
        }
        for list in g.origins.values_mut() {
            list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
        Ok(())
    }

    /// Records each page of `ids` as kept under `origin` now.
    pub fn rekey(&self, ids: &[String], origin: &str) {
        let mut g = self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for id in ids {
            if let Some(o) = g.get_mut(id) {
                *o = origin.to_string();
            }
        }
    }

    /// The key of `origin`'s site: the site it joined, else itself.
    pub fn site_key(&self, origin: &str) -> String {
        self.1
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .key_of
            .get(origin)
            .cloned()
            .unwrap_or_else(|| origin.to_string())
    }

    /// The origins of the site whose key is `key`, the most recently used
    /// first (only `key` for a site of one origin).
    pub fn site_origins(&self, key: &str) -> Vec<String> {
        self.1
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .origins
            .get(key)
            .map(|l| l.iter().map(|(o, _)| o.clone()).collect())
            .unwrap_or_else(|| vec![key.to_string()])
    }

    /// Records that Clax used `origin` now, when it is an origin of a
    /// joined site: written to the store at most once per [`TOUCH_EVERY`]
    /// per origin, and its site's order follows at once.
    ///
    /// # Errors
    /// The store's.
    pub fn touch(&self, st: &Store, origin: &str) -> clax_core::Result<()> {
        {
            let g = self
                .1
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(key) = g.key_of.get(origin) else {
                return Ok(());
            };
            let first = g
                .origins
                .get(key)
                .and_then(|l| l.first())
                .map(|x| x.0.as_str());
            let fresh = g
                .written
                .get(origin)
                .is_some_and(|t| t.elapsed() < TOUCH_EVERY);
            if fresh && first == Some(origin) {
                return Ok(());
            }
        }
        st.touch_site_origin(origin)?;
        let now = clax_core::Store::now();
        let mut g = self
            .1
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.written
            .insert(origin.to_string(), std::time::Instant::now());
        if let Some(key) = g.key_of.get(origin).cloned()
            && let Some(list) = g.origins.get_mut(&key)
            && let Some(i) = list.iter().position(|(o, _)| o == origin)
        {
            let mut e = list.remove(i);
            e.1 = now;
            list.insert(0, e);
        }
        Ok(())
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
pub(crate) fn live_route(path: &str) -> bool {
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

/// The artifact a path names: `/api/artifacts/<aid>…`, `/c/<aid>/…`,
/// `/a/<aid>…`, decoded as the handlers decode it (so `%37…` names the
/// same artifact as `7…`).
pub(crate) fn artifact_in(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("/api/artifacts/")
        .or_else(|| path.strip_prefix("/c/"))
        .or_else(|| path.strip_prefix("/a/"))?;
    let seg = percent_decoded(rest.split('/').next().unwrap_or(""))?;
    Some(seg.split(':').next().unwrap_or("").to_string())
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
        live_route(path) || artifact_in(path).is_some_and(|aid| s.live_ids.contains(&aid));
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
            (
                "/api/artifacts/%37q3k9mzx2b4t/threads",
                Some("7q3k9mzx2b4t"),
            ),
            ("/c/%37%71%33k9mzx2b4t/v/1/", Some("7q3k9mzx2b4t")),
            ("/a/7q3k9mzx2b4t%3Ax", Some("7q3k9mzx2b4t")),
        ] {
            assert_eq!(artifact_in(p).as_deref(), want, "{p}");
        }
        assert_eq!(artifact_in("/c/%ff/v/1/"), None);
    }

    #[test]
    fn live_ids_follow_inserts_and_removes() {
        let ids = LiveIds::default();
        assert!(!ids.contains("a"));
        ids.insert("a", "http://x");
        assert!(ids.contains("a"));
        assert_eq!(ids.origin_of("a").as_deref(), Some("http://x"));
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

#[cfg(test)]
mod store_tests {
    use super::*;
    use clax_core::Home;

    fn key(path: &str) -> PageKey {
        PageKey {
            origin: "http://localhost:5173".into(),
            path: path.into(),
        }
    }

    #[test]
    fn every_live_page_in_the_store_is_in_the_set_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let pages: Vec<String> = ["/a", "/b", "/c"]
            .into_iter()
            .map(|p| st.ensure_live_page(&key(p), "t", None).unwrap().artifact.id)
            .collect();
        let ids = LiveIds::load(&st).unwrap();
        for p in &pages {
            assert!(ids.contains(p), "{p}");
        }
        for p in st.live_page_ids().unwrap() {
            assert!(ids.contains(&p));
        }
    }

    #[test]
    fn a_page_a_join_merged_away_is_still_a_live_page_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let other = |path: &str| PageKey {
            origin: "http://localhost:5174".into(),
            path: path.into(),
        };
        st.ensure_live_page(&key("/"), "t", None).unwrap();
        let b = st
            .ensure_live_page(&other("/"), "t", None)
            .unwrap()
            .artifact
            .id;
        st.join_origins("http://localhost:5174", "http://localhost:5173")
            .unwrap();
        let (merged, _) = st.settle_joined_pages("http://localhost:5173").unwrap();
        assert_eq!(merged, vec![b.clone()]);
        assert!(
            st.live_page_of(&clax_core::ArtifactId::parse(&b).unwrap())
                .unwrap()
                .is_none()
        );
        // The set a restarted daemon loads keeps it, so the LAN rule hides it.
        let ids = LiveIds::load(&st).unwrap();
        assert!(ids.contains(&b));
    }

    #[test]
    fn ensure_page_records_the_page_it_makes() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let ids = LiveIds::default();
        let e = ids
            .ensure_page(&st, &key("/"), "t", Some(b"<p>"), &[])
            .unwrap();
        assert!(ids.contains(&e.artifact.id));
    }
}

//! Live pages on the daemon (spec 2026-10-05-chrome-overlay-design L10):
//! the set of live-page artifact IDs. The rule that hides them from callers
//! that are neither on this machine nor hold the token is
//! the HTTP layer's `live_access`.

use clax_core::Store;
use clax_core::audit::AuditCtx;
use clax_core::live::PageKey;
use clax_core::store::live::EnsuredPage;
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
    /// in this set before anything announces it; the store records the
    /// changes under `ctx`. Every server path that finds or creates a
    /// live page goes through here, so none can leave one out of the set.
    ///
    /// # Errors
    /// The store's.
    pub fn ensure_page(
        &self,
        st: &Store,
        ctx: &AuditCtx,
        key: &PageKey,
        title: &str,
        snapshot: Option<&[u8]>,
        pending: &[String],
    ) -> clax_core::Result<EnsuredPage> {
        let e = st.ensure_live_page_linking(ctx, key, title, snapshot, pending)?;
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
        let now = st.now();
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

#[cfg(test)]
mod tests {
    use super::*;

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
            .map(|p| {
                st.ensure_live_page(&AuditCtx::DAEMON, &key(p), "t", None)
                    .unwrap()
                    .artifact
                    .id
            })
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
        st.ensure_live_page(&clax_core::audit::AuditCtx::DAEMON, &key("/"), "t", None)
            .unwrap();
        let b = st
            .ensure_live_page(&clax_core::audit::AuditCtx::DAEMON, &other("/"), "t", None)
            .unwrap()
            .artifact
            .id;
        st.join_origins(
            &clax_core::audit::AuditCtx::DAEMON,
            "http://localhost:5174",
            "http://localhost:5173",
        )
        .unwrap();
        let (merged, _) = st
            .settle_joined_pages(&clax_core::audit::AuditCtx::DAEMON, "http://localhost:5173")
            .unwrap();
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
            .ensure_page(&st, &AuditCtx::DAEMON, &key("/"), "t", Some(b"<p>"), &[])
            .unwrap();
        assert!(ids.contains(&e.artifact.id));
    }
}

//! Bounded cache of wrapped HTML pages keyed by (artifact, version, file), least
//! recently used first out once the wrapped pages exceed a byte budget. A file
//! that cannot be wrapped (not UTF-8) is cached as such, so it is not read again
//! to find out. Versions are immutable, so an entry only becomes stale when its
//! artifact is deleted.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// The daemon's budget for wrapped pages: 32 MiB.
pub const DEFAULT_MAX_BYTES: usize = 32 << 20;
/// What a "not wrappable" entry counts against the budget besides its key.
const NEGATIVE_COST: usize = 64;

type Key = (String, u32, String);

pub struct WrapCache {
    inner: Mutex<Inner>,
    max_bytes: usize,
}

struct Inner {
    map: HashMap<Key, Option<Arc<String>>>,
    /// Least recently used first.
    order: VecDeque<Key>,
    bytes: usize,
    /// The bridge version the cached pages name (see [`WrapCache::follow_bridge`]).
    bridge: String,
}

/// What an entry counts against the budget: its key's bytes, plus the wrapped
/// page, or [`NEGATIVE_COST`] for a page that cannot be wrapped.
fn cost(key: &Key, v: &Option<Arc<String>>) -> usize {
    key.0.len()
        + key.2.len()
        + std::mem::size_of::<u32>()
        + v.as_ref().map_or(NEGATIVE_COST, |s| s.len())
}

impl Inner {
    fn touch(&mut self, key: &Key) {
        if let Some(i) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(i).expect("position is in range");
            self.order.push_back(k);
        }
    }
}

impl WrapCache {
    /// A cache holding at most `max_bytes` of wrapped pages.
    pub fn new(max_bytes: usize) -> Self {
        WrapCache {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
                bridge: String::new(),
            }),
            max_bytes,
        }
    }

    /// The cached wrap of `file` in version `n` of `artifact_id`, or `wrap()`'s
    /// result, cached. `Ok(None)` means the file cannot be wrapped. A page
    /// larger than the whole budget is returned but not cached; older entries
    /// are evicted, least recently used first, to make room. An error is
    /// returned and not cached.
    pub fn get_or_wrap(
        &self,
        artifact_id: &str,
        n: u32,
        file: &str,
        wrap: impl FnOnce() -> std::io::Result<Option<String>>,
    ) -> std::io::Result<Option<Arc<String>>> {
        let key = (artifact_id.to_string(), n, file.to_string());
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(v) = g.map.get(&key).cloned() {
                g.touch(&key);
                return Ok(v);
            }
        }
        let value = wrap()?.map(Arc::new);
        let size = cost(&key, &value);
        let mut g = self.inner.lock().unwrap();
        if let Some(v) = g.map.get(&key).cloned() {
            // A concurrent caller filled it while we were wrapping.
            g.touch(&key);
            return Ok(v);
        }
        if size > self.max_bytes {
            return Ok(value);
        }
        while g.bytes + size > self.max_bytes {
            let Some(old) = g.order.pop_front() else {
                break;
            };
            if let Some(v) = g.map.remove(&old) {
                g.bytes -= cost(&old, &v);
            }
        }
        g.bytes += size;
        g.order.push_back(key.clone());
        g.map.insert(key, value.clone());
        Ok(value)
    }

    /// Whether a wrap (or a "not wrappable" result) of `file` in version `n` of
    /// `artifact_id` is cached.
    pub fn contains(&self, artifact_id: &str, n: u32, file: &str) -> bool {
        let key = (artifact_id.to_string(), n, file.to_string());
        self.inner.lock().unwrap().map.contains_key(&key)
    }

    /// Drops every cached page when `bridge` is not the bridge version they
    /// were wrapped with, so pages name the current bridge URL. A release
    /// build's version never changes; a debug build's follows the file on
    /// disk (`just dev` rebuilds it under a running daemon).
    pub fn follow_bridge(&self, bridge: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.bridge != bridge {
            g.map.clear();
            g.order.clear();
            g.bytes = 0;
            g.bridge = bridge.to_string();
        }
    }

    /// Bytes the cached entries count against the budget (keys included).
    pub fn bytes(&self) -> usize {
        self.inner.lock().unwrap().bytes
    }

    /// Drops every cached page of every version of `artifact_id`.
    pub fn remove_artifact(&self, artifact_id: &str) {
        let mut g = self.inner.lock().unwrap();
        let mut freed = 0;
        g.map.retain(|k, v| {
            let keep = k.0 != artifact_id;
            if !keep {
                freed += cost(k, v);
            }
            keep
        });
        g.bytes -= freed;
        g.order.retain(|(id, _, _)| id != artifact_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(c: &WrapCache, a: &str, f: &str, body: &str) -> Option<String> {
        let body = body.to_string();
        c.get_or_wrap(a, 1, f, || Ok(Some(body)))
            .unwrap()
            .map(|s| s.to_string())
    }

    #[test]
    fn caches_per_file_and_invalidates_on_remove() {
        let c = WrapCache::new(1024);
        let mut calls = 0;
        for _ in 0..2 {
            let v = c.get_or_wrap("x", 1, "index.html", || {
                calls += 1;
                Ok(Some("A".into()))
            });
            assert_eq!(v.unwrap().as_deref().map(String::as_str), Some("A"));
        }
        assert_eq!(calls, 1);
        assert_eq!(
            page(&c, "x", "about.html", "B").as_deref(),
            Some("B"),
            "files are cached apart"
        );
        page(&c, "w", "about.html", "W");
        c.remove_artifact("x");
        assert!(!c.contains("x", 1, "index.html") && !c.contains("x", 1, "about.html"));
        assert!(c.contains("w", 1, "about.html"), "other artifacts stay");
        assert_eq!(c.bytes(), "w".len() + "about.html".len() + 4 + 1);
        assert_eq!(page(&c, "x", "about.html", "B2").as_deref(), Some("B2"));
    }

    #[test]
    fn a_page_that_cannot_be_wrapped_is_remembered() {
        let c = WrapCache::new(1024);
        let mut reads = 0;
        for _ in 0..3 {
            let v = c.get_or_wrap("x", 1, "latin1.html", || {
                reads += 1;
                Ok(None)
            });
            assert!(v.unwrap().is_none());
        }
        assert_eq!(reads, 1);
        let e = c.get_or_wrap("x", 1, "gone.html", || {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(e.is_err());
        assert!(!c.contains("x", 1, "gone.html"), "errors are not cached");
    }

    #[test]
    fn a_new_bridge_version_drops_every_page() {
        let c = WrapCache::new(1 << 20);
        c.follow_bridge("aaaaaaaaaaaa");
        c.get_or_wrap("a", 1, "index.html", || Ok(Some("x".into())))
            .unwrap();
        c.follow_bridge("aaaaaaaaaaaa");
        assert!(c.contains("a", 1, "index.html"), "same version: kept");
        c.follow_bridge("bbbbbbbbbbbb");
        assert!(!c.contains("a", 1, "index.html"), "new version: dropped");
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn entries_cost_their_key_too() {
        let c = WrapCache::new(1 << 20);
        page(&c, "a", "index.html", "aaaa");
        let key = "a".len() + "index.html".len() + 4;
        assert_eq!(c.bytes(), key + 4);
        c.get_or_wrap("a", 1, "latin1.html", || Ok(None)).unwrap();
        assert_eq!(
            c.bytes(),
            key + 4 + "a".len() + "latin1.html".len() + 4 + 64
        );
    }

    #[test]
    fn evicts_least_recently_used_pages_past_the_byte_budget() {
        // Each of a, b and c costs 15 bytes of key and 4 of page.
        let c = WrapCache::new(40);
        page(&c, "a", "index.html", "aaaa");
        page(&c, "b", "index.html", "bbbb");
        page(&c, "a", "index.html", "unused: a hit"); // a is now the most recent
        page(&c, "c", "index.html", "cccc");
        assert!(c.contains("a", 1, "index.html") && c.contains("c", 1, "index.html"));
        assert!(
            !c.contains("b", 1, "index.html"),
            "b was least recently used"
        );
        assert_eq!(c.bytes(), 38);
        assert_eq!(
            page(&c, "big", "index.html", "x".repeat(30).as_str()).map(|s| s.len()),
            Some(30)
        );
        assert!(
            !c.contains("big", 1, "index.html"),
            "larger than the budget: served, not cached"
        );
        assert_eq!(c.bytes(), 38);
    }
}

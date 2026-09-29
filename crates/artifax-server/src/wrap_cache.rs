//! Bounded cache of wrapped HTML pages keyed by (artifact, version, file). Versions are
//! immutable, so an entry only becomes stale when its artifact is deleted.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

type Key = (String, u32, String);

pub struct WrapCache {
    inner: Mutex<Inner>,
    capacity: usize,
}

struct Inner {
    map: HashMap<Key, Arc<String>>,
    order: VecDeque<Key>,
}

impl WrapCache {
    pub fn new(capacity: usize) -> Self {
        WrapCache {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
            capacity,
        }
    }

    /// The cached wrap of `file` in version `n` of `artifact_id`, or `wrap()`'s
    /// result, cached (the oldest entry is evicted at capacity). An error is
    /// returned and not cached.
    pub fn get_or_wrap(
        &self,
        artifact_id: &str,
        n: u32,
        file: &str,
        wrap: impl FnOnce() -> std::io::Result<String>,
    ) -> std::io::Result<Arc<String>> {
        let key = (artifact_id.to_string(), n, file.to_string());
        if let Some(v) = self.inner.lock().unwrap().map.get(&key) {
            return Ok(v.clone());
        }
        let value = Arc::new(wrap()?);
        let mut g = self.inner.lock().unwrap();
        if let Some(v) = g.map.get(&key) {
            // A concurrent caller filled it while we were wrapping.
            return Ok(v.clone());
        }
        if g.map.len() >= self.capacity
            && let Some(old) = g.order.pop_front()
        {
            g.map.remove(&old);
        }
        g.order.push_back(key.clone());
        g.map.insert(key, value.clone());
        Ok(value)
    }

    /// Whether a wrap of `file` in version `n` of `artifact_id` is cached.
    pub fn contains(&self, artifact_id: &str, n: u32, file: &str) -> bool {
        let key = (artifact_id.to_string(), n, file.to_string());
        self.inner.lock().unwrap().map.contains_key(&key)
    }

    /// Drops every cached page of every version of `artifact_id`.
    pub fn remove_artifact(&self, artifact_id: &str) {
        let mut g = self.inner.lock().unwrap();
        g.map.retain(|(id, _, _), _| id != artifact_id);
        g.order.retain(|(id, _, _)| id != artifact_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caches_per_version_and_invalidates_on_remove() {
        let c = WrapCache::new(2);
        let mut calls = 0;
        let a = c
            .get_or_wrap("x", 1, "index.html", || {
                calls += 1;
                Ok("A".into())
            })
            .unwrap();
        let b = c
            .get_or_wrap("x", 1, "index.html", || {
                calls += 1;
                Ok("B".into())
            })
            .unwrap();
        assert_eq!(*a, "A");
        assert_eq!(*b, "A");
        assert_eq!(calls, 1);
        c.get_or_wrap("y", 1, "index.html", || Ok("Y".into()))
            .unwrap();
        c.get_or_wrap("z", 1, "index.html", || Ok("Z".into()))
            .unwrap();
        let again = c
            .get_or_wrap("x", 1, "index.html", || {
                calls += 1;
                Ok("A2".into())
            })
            .unwrap();
        assert_eq!(*again, "A2", "evicted after capacity");
        c.remove_artifact("y");
        let y = c
            .get_or_wrap("y", 1, "index.html", || Ok("Y2".into()))
            .unwrap();
        assert_eq!(*y, "Y2");
    }

    #[test]
    fn files_of_one_version_are_cached_apart_and_removed_together() {
        let c = WrapCache::new(8);
        let i = c.get_or_wrap("x", 1, "index.html", || Ok("I".into()));
        let a = c.get_or_wrap("x", 1, "about.html", || Ok("A".into()));
        assert_eq!(*i.unwrap(), "I");
        assert_eq!(
            *a.unwrap(),
            "A",
            "a second file is not served the first's wrap"
        );
        c.get_or_wrap("x", 2, "about.html", || Ok("A2".into()))
            .unwrap();
        c.get_or_wrap("w", 1, "about.html", || Ok("W".into()))
            .unwrap();
        assert!(c.contains("x", 1, "about.html") && c.contains("x", 2, "about.html"));
        c.remove_artifact("x");
        for (n, f) in [(1, "index.html"), (1, "about.html"), (2, "about.html")] {
            assert!(!c.contains("x", n, f), "{n} {f}");
        }
        assert!(c.contains("w", 1, "about.html"), "other artifacts stay");
        let again = c.get_or_wrap("x", 1, "about.html", || Ok("A3".into()));
        assert_eq!(*again.unwrap(), "A3");
    }
}

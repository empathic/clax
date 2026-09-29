//! Bounded cache of wrapped index documents keyed by (artifact, version). Versions are immutable,
//! so an entry only becomes stale when its artifact is deleted.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub struct WrapCache {
    inner: Mutex<Inner>,
    capacity: usize,
}

struct Inner {
    map: HashMap<(String, u32), Arc<String>>,
    order: VecDeque<(String, u32)>,
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

    pub fn get_or_wrap(
        &self,
        artifact_id: &str,
        n: u32,
        wrap: impl FnOnce() -> std::io::Result<String>,
    ) -> std::io::Result<Arc<String>> {
        let key = (artifact_id.to_string(), n);
        if let Some(v) = self.inner.lock().unwrap().map.get(&key) {
            return Ok(v.clone());
        }
        let value = Arc::new(wrap()?);
        let mut g = self.inner.lock().unwrap();
        if g.map.contains_key(&key) {
            // A concurrent caller filled it while we were wrapping.
            return Ok(g.map[&key].clone());
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

    pub fn remove_artifact(&self, artifact_id: &str) {
        let mut g = self.inner.lock().unwrap();
        g.map.retain(|(id, _), _| id != artifact_id);
        g.order.retain(|(id, _)| id != artifact_id);
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
            .get_or_wrap("x", 1, || {
                calls += 1;
                Ok("A".into())
            })
            .unwrap();
        let b = c
            .get_or_wrap("x", 1, || {
                calls += 1;
                Ok("B".into())
            })
            .unwrap();
        assert_eq!(*a, "A");
        assert_eq!(*b, "A");
        assert_eq!(calls, 1);
        c.get_or_wrap("y", 1, || Ok("Y".into())).unwrap();
        c.get_or_wrap("z", 1, || Ok("Z".into())).unwrap();
        let again = c
            .get_or_wrap("x", 1, || {
                calls += 1;
                Ok("A2".into())
            })
            .unwrap();
        assert_eq!(*again, "A2", "evicted after capacity");
        c.remove_artifact("y");
        let y = c.get_or_wrap("y", 1, || Ok("Y2".into())).unwrap();
        assert_eq!(*y, "Y2");
    }
}

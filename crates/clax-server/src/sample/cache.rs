//! Answer caching for `sample()` (`sample.d.ts` `cache`): per artifact, per
//! viewer, per question (verb, tier, every turn, the images), in the daemon's
//! memory. A stored answer is replayed while it is younger than both the
//! window it was stored with and the window of the call asking now. An
//! identical call made while the first is still running follows the first's
//! flight instead of asking again. Only successful answers are stored.

use super::flight::{Done, Flight};
use super::request::MAX_GC;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Stored {
    done: Done,
    at: Instant,
    gc: Duration,
}

#[derive(Default)]
pub struct AnswerCache {
    entries: Mutex<HashMap<String, Stored>>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
}

impl AnswerCache {
    /// The cache key of a question asked by `viewer` (the viewer cookie, kept
    /// in memory only; `None` for a cookieless caller) in artifact `aid`.
    pub fn key(aid: &str, viewer: Option<&str>, input_key: &str) -> String {
        match viewer {
            Some(v) => format!("{aid}|v:{v}|{input_key}"),
            None => format!("{aid}|anon|{input_key}"),
        }
    }

    pub fn get(&self, key: &str, gc: Duration, now: Instant) -> Option<Done> {
        let entries = self.entries.lock().expect("cache lock");
        let e = entries.get(key)?;
        (now.saturating_duration_since(e.at) < gc.min(e.gc)).then(|| e.done.clone())
    }

    /// Stores (or overwrites) an answer; entries older than a day are dropped.
    pub fn put(&self, key: String, done: Done, gc: Duration, now: Instant) {
        let mut entries = self.entries.lock().expect("cache lock");
        entries.retain(|_, e| now.saturating_duration_since(e.at) < MAX_GC);
        entries.insert(key, Stored { done, at: now, gc });
    }

    /// The running flight for `key`, if one is still unfinished.
    pub fn flight(&self, key: &str) -> Option<Arc<Flight>> {
        self.flights
            .lock()
            .expect("flights lock")
            .get(key)
            .filter(|f| !f.is_finished())
            .cloned()
    }

    /// Makes `f` the flight later identical calls follow.
    pub fn begin(&self, key: String, f: Arc<Flight>) {
        self.flights.lock().expect("flights lock").insert(key, f);
    }

    /// Stops sharing `f` (only if it is still the flight for `key`).
    pub fn end(&self, key: &str, f: &Arc<Flight>) {
        let mut flights = self.flights.lock().expect("flights lock");
        if flights.get(key).is_some_and(|cur| Arc::ptr_eq(cur, f)) {
            flights.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::request::Tier;

    fn done(t: &str) -> Done {
        Done {
            text: t.into(),
            truncated: false,
            model_tier_applied: Tier::Default,
            value: None,
        }
    }

    #[test]
    fn an_answer_replays_while_younger_than_both_windows() {
        let c = AnswerCache::default();
        let t0 = Instant::now();
        let k = AnswerCache::key("7q3k9mzx2b4t", Some("01J0"), "text|default|\"q\"|0");
        c.put(k.clone(), done("a"), Duration::from_secs(60), t0);
        assert_eq!(
            c.get(&k, Duration::from_secs(300), t0 + Duration::from_secs(59)),
            Some(done("a"))
        );
        assert_eq!(
            c.get(&k, Duration::from_secs(300), t0 + Duration::from_secs(60)),
            None
        );
        assert_eq!(
            c.get(&k, Duration::from_secs(10), t0 + Duration::from_secs(11)),
            None
        );
    }

    #[test]
    fn keys_separate_artifacts_and_viewers() {
        let a = AnswerCache::key("7q3k9mzx2b4t", Some("v1"), "k");
        assert_ne!(a, AnswerCache::key("7q3k9mzx2b4t", Some("v2"), "k"));
        assert_ne!(a, AnswerCache::key("zzzzzzzzzzzz", Some("v1"), "k"));
        assert_ne!(
            AnswerCache::key("7q3k9mzx2b4t", None, "k"),
            AnswerCache::key("7q3k9mzx2b4t", Some("-"), "k")
        );
    }

    #[test]
    fn flights_are_shared_until_they_end() {
        let c = AnswerCache::default();
        let f = Flight::new();
        c.begin("k".into(), f.clone());
        assert!(Arc::ptr_eq(&c.flight("k").unwrap(), &f));
        let newer = Flight::new();
        c.begin("k".into(), newer.clone());
        c.end("k", &f);
        assert!(Arc::ptr_eq(&c.flight("k").unwrap(), &newer));
        c.end("k", &newer);
        assert!(c.flight("k").is_none());
    }
}

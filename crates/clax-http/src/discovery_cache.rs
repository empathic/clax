//! What other Clax instances' `GET /.well-known/clax` said, in memory
//! (`clax_server::resolve`): an answer naming a Clax is kept, for a
//! connected remote's origin, until a call to that origin fails
//! ([`DiscoveryCache::forget`]), and for any other origin for
//! [`POSITIVE_TTL_S`] (or until forgotten); any other answer, or none, for
//! [`NEGATIVE_TTL_S`]; each on the state's clock.

use chrono::{DateTime, Utc};
use clax_core::working::Clock;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// How long an origin that is not a Clax instance (or did not answer) is
/// taken as one, before discovery asks it again: 10 minutes.
pub const NEGATIVE_TTL_S: i64 = 600;

/// How long an origin that is not a connected remote's, and said it is a
/// Clax instance, is taken as one before discovery asks it again: an
/// hour. Nothing calls such an origin, so no failed call forgets it; a site
/// that once served a Clax-shaped `/.well-known/clax` and stopped makes
/// its URLs `sign_in_required` for an hour at most, while an agent that
/// reads several pages of an instance it is about to ask to connect asks
/// its discovery about once.
pub const POSITIVE_TTL_S: i64 = 3600;

/// The most origins the cache holds. Past it, expired answers go first,
/// then every negative one.
pub const MAX_ENTRIES: usize = 1024;

/// A Clax instance, as its discovery describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    /// `hosted` or `local`.
    pub kind: String,
    /// Its name, or its host when it gave none.
    pub name: String,
    /// The domain its artifacts' sites are served under, when it has one.
    pub site_domain: Option<String>,
}

/// What the cache knows of an origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cached {
    /// A Clax instance.
    Clax(Instance),
    /// Not one, as of less than [`NEGATIVE_TTL_S`] ago.
    NotClax,
    /// Nothing: ask it.
    Unknown,
}

enum Entry {
    /// `until`: `None` for a connected remote's origin, kept until
    /// forgotten.
    Clax {
        instance: Instance,
        until: Option<DateTime<Utc>>,
    },
    NotClax {
        until: DateTime<Utc>,
    },
}

impl Entry {
    fn live(&self, now: DateTime<Utc>) -> bool {
        match self {
            Entry::Clax { until, .. } => until.is_none_or(|u| u > now),
            Entry::NotClax { until } => *until > now,
        }
    }
}

/// The discovery answers of other origins, by origin
/// (`scheme://host[:port]`, as `url::Origin::ascii_serialization` writes it).
pub struct DiscoveryCache {
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<String, Entry>>,
}

impl DiscoveryCache {
    /// An empty cache whose negative answers expire on `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> DiscoveryCache {
        DiscoveryCache {
            clock,
            entries: Mutex::default(),
        }
    }

    /// What is known of `origin`.
    pub fn get(&self, origin: &str) -> Cached {
        let now = self.clock.now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get(origin) {
            Some(e) if !e.live(now) => {
                entries.remove(origin);
                Cached::Unknown
            }
            Some(Entry::Clax { instance, .. }) => Cached::Clax(instance.clone()),
            Some(Entry::NotClax { .. }) => Cached::NotClax,
            None => Cached::Unknown,
        }
    }

    /// `origin`, a connected remote's, is the Clax instance `instance`,
    /// until [`Self::forget`].
    pub fn put_clax(&self, origin: &str, instance: Instance) {
        self.put(
            origin,
            Entry::Clax {
                instance,
                until: None,
            },
        );
    }

    /// `origin`, not a connected remote's, is the Clax instance
    /// `instance`, for [`POSITIVE_TTL_S`] or until [`Self::forget`].
    pub fn put_clax_unconnected(&self, origin: &str, instance: Instance) {
        let until = self.clock.now() + chrono::Duration::seconds(POSITIVE_TTL_S);
        self.put(
            origin,
            Entry::Clax {
                instance,
                until: Some(until),
            },
        );
    }

    /// `origin` is not a Clax instance, or did not answer, for
    /// [`NEGATIVE_TTL_S`].
    pub fn put_not_clax(&self, origin: &str) {
        let until = self.clock.now() + chrono::Duration::seconds(NEGATIVE_TTL_S);
        self.put(origin, Entry::NotClax { until });
    }

    /// Drops what is known of `origin`: a call to it failed, so the next
    /// resolution asks it again.
    pub fn forget(&self, origin: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(origin);
    }

    fn put(&self, origin: &str, entry: Entry) {
        let now = self.clock.now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_ENTRIES && !entries.contains_key(origin) {
            entries.retain(|_, e| e.live(now));
            if entries.len() >= MAX_ENTRIES {
                entries.retain(|_, e| matches!(e, Entry::Clax { .. }));
            }
            if entries.len() >= MAX_ENTRIES {
                entries.clear();
            }
        }
        entries.insert(origin.to_string(), entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::working::ManualClock;

    fn instance() -> Instance {
        Instance {
            kind: "hosted".into(),
            name: "Acme".into(),
            site_domain: None,
        }
    }

    #[test]
    fn a_negative_answer_lasts_ten_minutes_and_a_positive_one_until_forgotten() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let c = DiscoveryCache::new(clock.clone());
        assert_eq!(c.get("https://a.example"), Cached::Unknown);
        c.put_not_clax("https://a.example");
        c.put_clax("https://b.example", instance());
        clock.advance(NEGATIVE_TTL_S - 1);
        assert_eq!(c.get("https://a.example"), Cached::NotClax);
        clock.advance(1);
        assert_eq!(c.get("https://a.example"), Cached::Unknown);
        clock.advance(86_400 * 365);
        assert_eq!(c.get("https://b.example"), Cached::Clax(instance()));
        c.forget("https://b.example");
        assert_eq!(c.get("https://b.example"), Cached::Unknown);
    }

    #[test]
    fn a_positive_answer_for_an_unconnected_origin_lasts_an_hour() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let c = DiscoveryCache::new(clock.clone());
        c.put_clax_unconnected("https://c.example", instance());
        c.put_clax("https://d.example", instance());
        clock.advance(POSITIVE_TTL_S - 1);
        assert_eq!(c.get("https://c.example"), Cached::Clax(instance()));
        clock.advance(1);
        assert_eq!(c.get("https://c.example"), Cached::Unknown);
        assert_eq!(c.get("https://d.example"), Cached::Clax(instance()));
    }

    #[test]
    fn a_full_cache_drops_negative_answers_before_positive_ones() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let c = DiscoveryCache::new(clock);
        c.put_clax("https://kept.example", instance());
        for i in 0..MAX_ENTRIES - 1 {
            c.put_not_clax(&format!("https://{i}.example"));
        }
        c.put_not_clax("https://new.example");
        assert_eq!(c.get("https://kept.example"), Cached::Clax(instance()));
        assert_eq!(c.get("https://new.example"), Cached::NotClax);
        assert_eq!(c.get("https://0.example"), Cached::Unknown);
    }
}

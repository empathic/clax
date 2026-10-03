//! Presence (spec §10 "Presence"): which viewers have an artifact open now,
//! whether they are here or away, and, when they share it, where they look.
//! Reports live in memory only: a daemon restart starts with none, and a
//! report lapses [`PRESENCE_TTL_SECS`] after it was made.

use crate::working::{Clock, clean_line};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Seconds after the last report that a viewer reads as gone.
pub const PRESENCE_TTL_SECS: i64 = 90;
/// Seconds a gone viewer stays listed before the entry is dropped.
pub const GONE_KEEP_SECS: i64 = 600;
/// Longest location, in characters, after cleaning.
pub const MAX_WHERE_CHARS: usize = 80;
/// Most viewers one artifact lists. Anyone who reaches the daemon can make
/// viewers, so the list, and each `presence` event, which carries it whole,
/// stays bounded.
pub const MAX_PEOPLE: usize = 64;

/// A viewer's presence. `Gone` is never reported: it is a report that lapsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Here,
    Away,
    Gone,
}

/// One viewer's presence as anyone may read it: never the cookie.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PresenceView {
    pub public_id: String,
    pub display_name: Option<String>,
    pub state: State,
    /// Where the viewer looks; only while `Here`, and only when shared.
    pub r#where: Option<String>,
    /// The last report.
    pub since: String,
}

struct Entry {
    display_name: Option<String>,
    state: State,
    where_: Option<String>,
    last_report: DateTime<Utc>,
}

type Key = (String, String);

/// The registry, keyed by (artifact ID, viewer public ID). Every method takes
/// the lock once; none blocks on I/O.
pub struct Presence {
    clock: Arc<dyn Clock>,
    entries: Mutex<BTreeMap<Key, Entry>>,
}

fn lapsed(e: &Entry, now: DateTime<Utc>) -> bool {
    now > e.last_report + Duration::seconds(PRESENCE_TTL_SECS)
}

fn view(public_id: &str, e: &Entry, now: DateTime<Utc>) -> PresenceView {
    let gone = e.state == State::Gone || lapsed(e, now);
    PresenceView {
        public_id: public_id.to_string(),
        display_name: e.display_name.clone(),
        state: if gone { State::Gone } else { e.state },
        r#where: if gone { None } else { e.where_.clone() },
        since: e.last_report.to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

/// What a viewer sees of an entry, apart from the time of the report.
fn visible(v: &PresenceView) -> (Option<&str>, State, Option<&str>) {
    (v.display_name.as_deref(), v.state, v.r#where.as_deref())
}

impl Presence {
    pub fn new(clock: Arc<dyn Clock>) -> Presence {
        Presence {
            clock,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Records a report from `public_id` on `aid`. `where_` is cleaned to one
    /// line of at most [`MAX_WHERE_CHARS`] and kept only while `Here`.
    /// Returns whether the view anyone sees changed; `None` when `aid`
    /// already lists [`MAX_PEOPLE`] others, none of them gone (a newcomer
    /// otherwise takes the place of the gone one whose last report is oldest).
    pub fn report(
        &self,
        aid: &str,
        public_id: &str,
        display_name: Option<&str>,
        state: State,
        where_: Option<&str>,
    ) -> Option<bool> {
        let now = self.clock.now();
        let state = if state == State::Gone {
            State::Away
        } else {
            state
        };
        let where_ = match state {
            State::Here => where_.and_then(|w| clean_line(w, MAX_WHERE_CHARS).0),
            _ => None,
        };
        let entry = Entry {
            display_name: display_name.map(str::to_string),
            state,
            where_,
            last_report: now,
        };
        let key = (aid.to_string(), public_id.to_string());
        let mut map = self.entries.lock().unwrap();
        let mut evicted = false;
        if !map.contains_key(&key) {
            let mut count = 0;
            let mut oldest_gone: Option<(Key, DateTime<Utc>)> = None;
            for (k, e) in map
                .range((aid.to_string(), String::new())..)
                .take_while(|((a, _), _)| a == aid)
            {
                count += 1;
                let gone = e.state == State::Gone || lapsed(e, now);
                if gone
                    && oldest_gone
                        .as_ref()
                        .is_none_or(|(_, at)| e.last_report < *at)
                {
                    oldest_gone = Some((k.clone(), e.last_report));
                }
            }
            if count >= MAX_PEOPLE {
                map.remove(&oldest_gone?.0);
                evicted = true;
            }
        }
        let before = map.get(&key).map(|e| view(public_id, e, now));
        let after = view(public_id, &entry, now);
        map.insert(key, entry);
        Some(evicted || before.as_ref().map(visible) != Some(visible(&after)))
    }

    /// Marks lapsed reports `Gone`, drops those gone past [`GONE_KEEP_SECS`],
    /// and returns the artifacts whose view changed.
    pub fn sweep(&self) -> Vec<String> {
        let now = self.clock.now();
        let drop_after = Duration::seconds(PRESENCE_TTL_SECS + GONE_KEEP_SECS);
        let mut changed = BTreeSet::new();
        let mut map = self.entries.lock().unwrap();
        map.retain(|(aid, _), e| {
            if now > e.last_report + drop_after {
                changed.insert(aid.clone());
                return false;
            }
            if e.state != State::Gone && lapsed(e, now) {
                e.state = State::Gone;
                e.where_ = None;
                changed.insert(aid.clone());
            }
            true
        });
        changed.into_iter().collect()
    }

    /// The artifact's viewers: here, then away, then gone, each by name.
    pub fn for_artifact(&self, aid: &str) -> Vec<PresenceView> {
        let now = self.clock.now();
        let map = self.entries.lock().unwrap();
        let mut out: Vec<PresenceView> = map
            .range((aid.to_string(), String::new())..)
            .take_while(|((a, _), _)| a == aid)
            .map(|((_, pid), e)| view(pid, e, now))
            .collect();
        out.sort_by(|a, b| {
            (
                a.state,
                a.display_name.as_deref().map(str::to_lowercase),
                &a.public_id,
            )
                .cmp(&(
                    b.state,
                    b.display_name.as_deref().map(str::to_lowercase),
                    &b.public_id,
                ))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working::ManualClock;
    use std::sync::Arc;

    fn reg() -> (Arc<ManualClock>, Presence) {
        let c = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
        (c.clone(), Presence::new(c))
    }

    #[test]
    fn a_report_is_here_until_it_lapses_then_gone_then_dropped() {
        let (c, p) = reg();
        p.report(
            "a1",
            "u_a",
            Some("Alex"),
            State::Here,
            Some("«Quarterly goals»"),
        );
        assert_eq!(p.for_artifact("a1")[0].state, State::Here);
        assert_eq!(
            p.for_artifact("a1")[0].r#where.as_deref(),
            Some("«Quarterly goals»")
        );
        c.advance(91);
        assert_eq!(p.sweep(), vec!["a1".to_string()]);
        let v = &p.for_artifact("a1")[0];
        assert_eq!(
            (v.state, v.r#where.is_none()),
            (State::Gone, true),
            "a lapsed report keeps no location"
        );
        c.advance(600);
        p.sweep();
        assert!(p.for_artifact("a1").is_empty());
    }

    #[test]
    fn where_is_cleaned_and_bounded_and_away_keeps_no_location() {
        let (_c, p) = reg();
        p.report(
            "a1",
            "u_a",
            None,
            State::Here,
            Some(&format!("  {}\n", "x".repeat(100))),
        );
        assert_eq!(
            p.for_artifact("a1")[0]
                .r#where
                .as_ref()
                .unwrap()
                .chars()
                .count(),
            80
        );
        p.report("a1", "u_a", None, State::Away, Some("chart"));
        assert!(p.for_artifact("a1")[0].r#where.is_none());
    }

    #[test]
    fn a_repeated_report_changes_nothing_and_artifacts_stay_apart() {
        let (_c, p) = reg();
        assert_eq!(
            p.report("a1", "u_a", Some("Alex"), State::Here, None),
            Some(true)
        );
        assert_eq!(
            p.report("a1", "u_a", Some("Alex"), State::Here, None),
            Some(false)
        );
        assert_eq!(
            p.report("a1", "u_b", Some("Bea"), State::Away, None),
            Some(true)
        );
        assert_eq!(
            p.report("a2", "u_a", Some("Alex"), State::Here, None),
            Some(true)
        );
        let names: Vec<_> = p
            .for_artifact("a1")
            .into_iter()
            .map(|v| v.public_id)
            .collect();
        assert_eq!(names, ["u_a", "u_b"]);
        assert_eq!(p.for_artifact("a2").len(), 1);
    }

    #[test]
    fn an_artifact_lists_at_most_max_people_and_a_newcomer_takes_the_oldest_gone_place() {
        let (c, p) = reg();
        for i in 0..MAX_PEOPLE {
            assert_eq!(
                p.report("a1", &format!("u_{i}"), None, State::Here, None),
                Some(true)
            );
        }
        assert_eq!(
            p.report("a1", "u_new", None, State::Here, None),
            None,
            "full"
        );
        assert_eq!(p.for_artifact("a1").len(), MAX_PEOPLE);
        // Those already listed still report; another artifact is apart.
        assert_eq!(p.report("a1", "u_3", None, State::Away, None), Some(true));
        assert_eq!(p.report("a2", "u_new", None, State::Here, None), Some(true));
        // Once some lapse, a newcomer takes the place of the oldest of them.
        c.advance(60);
        for i in 1..MAX_PEOPLE {
            p.report("a1", &format!("u_{i}"), None, State::Here, None);
        }
        c.advance(40);
        assert_eq!(p.report("a1", "u_new", None, State::Here, None), Some(true));
        let ids: Vec<_> = p
            .for_artifact("a1")
            .into_iter()
            .map(|v| v.public_id)
            .collect();
        assert_eq!(ids.len(), MAX_PEOPLE);
        assert!(ids.contains(&"u_new".to_string()) && !ids.contains(&"u_0".to_string()));
    }
}

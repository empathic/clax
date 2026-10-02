//! The working signal (spec §10 "Working"): which harness session is acting
//! on which artifact, and on which of its threads, now. Records live in
//! memory only. Each lapses [`WORKING_TTL_SECS`] after its last renewal, so a
//! daemon restart starts with none and never shows stale work.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Seconds after its last renewal that a record lapses.
pub const WORKING_TTL_SECS: i64 = 120;
/// Longest message, in characters, after [`clean_message`].
pub const MAX_MESSAGE_CHARS: usize = 140;
/// Most threads one record names.
pub const MAX_WORKING_THREADS: usize = 20;

/// Where the registry reads the time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that moves only when told to.
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl ManualClock {
    /// A clock stopped at `rfc3339`.
    ///
    /// # Panics
    /// When `rfc3339` does not parse.
    pub fn at(rfc3339: &str) -> ManualClock {
        ManualClock(Mutex::new(
            DateTime::parse_from_rfc3339(rfc3339)
                .expect("an RFC 3339 time")
                .with_timezone(&Utc),
        ))
    }
    pub fn advance(&self, secs: i64) {
        *self.0.lock().unwrap() += Duration::seconds(secs);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// The session a record belongs to, as the daemon knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub session_id: String,
    pub harness: String,
    /// The session's opaque agent handle (`a_…`), safe to show anyone.
    pub agent: String,
}

/// A record as anyone may read it: never names the session.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkingView {
    /// A ULID minted when the record was created.
    pub key: String,
    /// The agent's handle: tells two sessions of one harness apart.
    pub agent: String,
    pub harness: String,
    pub message: Option<String>,
    pub thread_ids: Vec<String>,
    pub started_at: String,
    pub last_heartbeat: String,
}

/// A record as its session's token holder reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionWorking {
    #[serde(flatten)]
    pub view: WorkingView,
    pub session_id: String,
    pub artifact_id: String,
    pub expires_at: String,
}

/// An explicit update: each `Some` field replaces the stored one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetWorking {
    pub thread_ids: Option<Vec<String>>,
    pub message: Option<String>,
}

/// The artifacts whose working list changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed(pub BTreeSet<String>);

impl Changed {
    pub fn merge(&mut self, other: Changed) {
        self.0.extend(other.0);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn one(aid: &str) -> Changed {
        Changed([aid.to_string()].into())
    }
}

struct Record {
    key: String,
    agent: String,
    harness: String,
    message: Option<String>,
    threads: Vec<String>,
    /// The record has named a thread at some point: losing its last one clears it.
    had_threads: bool,
    started_at: DateTime<Utc>,
    heartbeat: DateTime<Utc>,
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl Record {
    fn view(&self) -> WorkingView {
        WorkingView {
            key: self.key.clone(),
            agent: self.agent.clone(),
            harness: self.harness.clone(),
            message: self.message.clone(),
            thread_ids: self.threads.clone(),
            started_at: stamp(self.started_at),
            last_heartbeat: stamp(self.heartbeat),
        }
    }
    fn add(&mut self, threads: &[String]) {
        for t in threads {
            if !self.threads.contains(t) && self.threads.len() < MAX_WORKING_THREADS {
                self.threads.push(t.clone());
            }
        }
        self.had_threads |= !self.threads.is_empty();
    }
}

/// [`clean_line`] bounded by [`MAX_MESSAGE_CHARS`].
pub fn clean_message(raw: &str) -> (Option<String>, bool) {
    clean_line(raw, MAX_MESSAGE_CHARS)
}

/// `raw` as one line: whitespace runs (line and paragraph separators included)
/// become one space, other control characters are dropped, the ends are
/// trimmed. Empty is `None`. Past `max` characters it is cut to one
/// character less plus `…`; the flag says so.
pub fn clean_line(raw: &str, max: usize) -> (Option<String>, bool) {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    let one = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return (None, false);
    }
    if one.chars().count() <= max {
        return (Some(one), false);
    }
    let mut cut: String = one.chars().take(max - 1).collect();
    cut.push('…');
    (Some(cut), true)
}

type Key = (String, String);

/// The registry. Every method takes the lock once; none blocks on I/O.
pub struct Working {
    clock: Arc<dyn Clock>,
    skew: Mutex<Duration>,
    records: Mutex<BTreeMap<Key, Record>>,
}

impl Working {
    pub fn new(clock: Arc<dyn Clock>) -> Working {
        Working {
            clock,
            skew: Mutex::new(Duration::zero()),
            records: Mutex::new(BTreeMap::new()),
        }
    }

    /// The registry's time: its clock plus any [`Working::skew`].
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now() + *self.skew.lock().unwrap()
    }

    /// Moves the registry's time forward by `secs` (the debug build's test route).
    pub fn skew(&self, secs: i64) {
        *self.skew.lock().unwrap() += Duration::seconds(secs);
    }

    fn live(&self, r: &Record, now: DateTime<Utc>) -> bool {
        now - r.heartbeat < Duration::seconds(WORKING_TTL_SECS)
    }

    fn upsert<'a>(
        map: &'a mut BTreeMap<Key, Record>,
        who: &Actor,
        aid: &str,
        now: DateTime<Utc>,
    ) -> &'a mut Record {
        let r = map
            .entry((who.session_id.clone(), aid.to_string()))
            .or_insert_with(|| Record {
                key: crate::new_ulid(),
                agent: who.agent.clone(),
                harness: who.harness.clone(),
                message: None,
                threads: Vec::new(),
                had_threads: false,
                started_at: now,
                heartbeat: now,
            });
        r.heartbeat = now;
        r
    }

    fn session_view(sid: &str, aid: &str, r: &Record) -> SessionWorking {
        SessionWorking {
            view: r.view(),
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            expires_at: stamp(r.heartbeat + Duration::seconds(WORKING_TTL_SECS)),
        }
    }

    /// Drops lapsed records from `map` first, so a lapsed record is never updated in place.
    fn prune(&self, map: &mut BTreeMap<Key, Record>, now: DateTime<Utc>) -> Changed {
        let mut changed = Changed::default();
        map.retain(|(_, aid), r| {
            let keep = self.live(r, now);
            if !keep {
                changed.0.insert(aid.clone());
            }
            keep
        });
        changed
    }

    /// Creates or updates the record; `Some` fields replace (an empty thread
    /// list clears the threads without clearing the record). Always renews.
    pub fn set(&self, who: &Actor, aid: &str, s: SetWorking) -> (SessionWorking, Changed) {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let r = Self::upsert(&mut map, who, aid, now);
        if let Some(t) = s.thread_ids {
            r.threads.clear();
            r.had_threads = false;
            r.add(&t);
        }
        if let Some(m) = s.message {
            r.message = Some(m);
        }
        changed.merge(Changed::one(aid));
        (Self::session_view(&who.session_id, aid, r), changed)
    }

    /// Creates or renews the record and adds `threads` (feedback reached the session).
    pub fn mark(&self, who: &Actor, aid: &str, threads: &[String]) -> Changed {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let before = map
            .get(&(who.session_id.clone(), aid.to_string()))
            .map(|r| r.threads.clone());
        let r = Self::upsert(&mut map, who, aid, now);
        r.add(threads);
        if before.as_ref() != Some(&r.threads) {
            changed.merge(Changed::one(aid));
        }
        changed
    }

    /// Renews every live record of session `sid`; how many.
    pub fn renew(&self, sid: &str) -> usize {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut n = 0;
        for ((s, _), r) in map.iter_mut() {
            if s == sid && self.live(r, now) {
                r.heartbeat = now;
                n += 1;
            }
        }
        n
    }

    /// Removes `threads` from the record, or the record when `None`. A record
    /// that named threads and has none left is removed.
    pub fn clear(&self, sid: &str, aid: &str, threads: Option<&[String]>) -> Changed {
        let mut map = self.records.lock().unwrap();
        let key = (sid.to_string(), aid.to_string());
        let Some(r) = map.get_mut(&key) else {
            return Changed::default();
        };
        match threads {
            None => {
                map.remove(&key);
            }
            Some(ts) => {
                let n = r.threads.len();
                r.threads.retain(|t| !ts.contains(t));
                if r.threads.len() == n {
                    return Changed::default();
                }
                if r.had_threads && r.threads.is_empty() {
                    map.remove(&key);
                }
            }
        }
        Changed::one(aid)
    }

    /// The session replied to or resolved `tid`: renews the session, then
    /// takes `tid` out of its record on `aid`.
    pub fn thread_done(&self, sid: &str, aid: &str, tid: &str) -> Changed {
        self.renew(sid);
        self.clear(sid, aid, Some(&[tid.to_string()]))
    }

    /// `tid` was resolved by a viewer or deleted: out of every record on `aid`.
    pub fn thread_gone(&self, aid: &str, tid: &str) -> Changed {
        let sessions: Vec<String> = {
            let map = self.records.lock().unwrap();
            map.keys()
                .filter(|(_, a)| a == aid)
                .map(|(s, _)| s.clone())
                .collect()
        };
        let mut changed = Changed::default();
        for s in sessions {
            changed.merge(self.clear(&s, aid, Some(&[tid.to_string()])));
        }
        changed
    }

    /// Every record of `sid` (turn end or session end).
    pub fn end_session(&self, sid: &str) -> Changed {
        let mut changed = Changed::default();
        self.records.lock().unwrap().retain(|(s, aid), _| {
            let keep = s != sid;
            if !keep {
                changed.0.insert(aid.clone());
            }
            keep
        });
        changed
    }

    /// Every record on `aid`.
    pub fn artifact_gone(&self, aid: &str) -> Changed {
        let mut map = self.records.lock().unwrap();
        let n = map.len();
        map.retain(|(_, a), _| a != aid);
        if map.len() == n {
            Changed::default()
        } else {
            Changed::one(aid)
        }
    }

    /// Removes lapsed records; the artifacts whose list changed.
    pub fn sweep(&self) -> Changed {
        let now = self.now();
        self.prune(&mut self.records.lock().unwrap(), now)
    }

    /// The live records on `aid`, newest `started_at` first.
    pub fn for_artifact(&self, aid: &str) -> Vec<WorkingView> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        let mut v: Vec<_> = map
            .iter()
            .filter(|((_, a), r)| a == aid && self.live(r, now))
            .map(|(_, r)| (r.started_at, r.view()))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.key.cmp(&a.1.key)));
        v.into_iter().map(|(_, x)| x).collect()
    }

    /// The live records of `sid`, by artifact ID.
    pub fn for_session(&self, sid: &str) -> Vec<SessionWorking> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.iter()
            .filter(|((s, _), r)| s == sid && self.live(r, now))
            .map(|((s, a), r)| Self::session_view(s, a, r))
            .collect()
    }

    /// The threads the live record (`sid`, `aid`) names.
    pub fn threads_of(&self, sid: &str, aid: &str) -> Vec<String> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.get(&(sid.to_string(), aid.to_string()))
            .filter(|r| self.live(r, now))
            .map(|r| r.threads.clone())
            .unwrap_or_default()
    }

    /// Every artifact with live records, and its list ([`Working::for_artifact`] order).
    pub fn all(&self) -> BTreeMap<String, Vec<WorkingView>> {
        let aids: BTreeSet<String> = self
            .records
            .lock()
            .unwrap()
            .keys()
            .map(|(_, a)| a.clone())
            .collect();
        aids.into_iter()
            .map(|a| (a.clone(), self.for_artifact(&a)))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Arc<ManualClock>, Working) {
        let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
        (clock.clone(), Working::new(clock))
    }
    fn claude(sid: &str) -> Actor {
        Actor {
            session_id: sid.into(),
            harness: "claude".into(),
            // Hex of the session ID, so no view text contains the ID itself.
            agent: format!("a_{}", hex(sid)),
        }
    }
    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mark_creates_then_adds_threads_and_keeps_the_key() {
        let (_c, w) = fixture();
        assert_eq!(
            w.mark(&claude("s1"), "a1", &ids(&["t1"])).0,
            ["a1".to_string()].into()
        );
        let key = w.for_artifact("a1")[0].key.clone();
        w.mark(&claude("s1"), "a1", &ids(&["t2", "t1"]));
        let v = w.for_artifact("a1");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].thread_ids, ids(&["t1", "t2"]));
        assert_eq!(v[0].key, key);
        assert_eq!(v[0].harness, "claude");
        assert_eq!(v[0].message, None);
    }

    #[test]
    fn set_replaces_given_fields_and_keeps_started_at() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        let started = w.for_artifact("a1")[0].started_at.clone();
        c.advance(30);
        let (s, changed) = w.set(
            &claude("s1"),
            "a1",
            SetWorking {
                thread_ids: None,
                message: Some("Tightening spacing".into()),
            },
        );
        assert!(!changed.is_empty());
        assert_eq!(s.view.message.as_deref(), Some("Tightening spacing"));
        assert_eq!(s.view.thread_ids, ids(&["t1"]));
        assert_eq!(s.view.started_at, started);
        assert_eq!(s.view.last_heartbeat, "2026-09-30T10:00:30.000Z");
        assert_eq!(s.expires_at, "2026-09-30T10:02:30.000Z");
        let (s, _) = w.set(
            &claude("s1"),
            "a1",
            SetWorking {
                thread_ids: Some(vec![]),
                message: None,
            },
        );
        assert!(s.view.thread_ids.is_empty());
        assert_eq!(s.view.message.as_deref(), Some("Tightening spacing"));
    }

    #[test]
    fn records_lapse_120_s_after_the_last_renewal() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(119);
        assert_eq!(w.for_artifact("a1").len(), 1);
        assert_eq!(w.renew("s1"), 1);
        c.advance(119);
        assert_eq!(w.for_artifact("a1").len(), 1, "renewed at 119 s");
        c.advance(1);
        assert!(
            w.for_artifact("a1").is_empty(),
            "hidden at 120 s, before any sweep"
        );
        assert_eq!(w.sweep().0, ["a1".to_string()].into());
        assert!(w.sweep().is_empty(), "swept once");
        assert_eq!(w.renew("s1"), 0, "a lapsed record is not revived");
    }

    #[test]
    fn replying_to_the_last_named_thread_clears_the_record() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.thread_done("s1", "a1", "t1");
        assert_eq!(w.for_artifact("a1")[0].thread_ids, ids(&["t2"]));
        assert!(!w.thread_done("s1", "a1", "t2").is_empty());
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn a_record_that_never_named_threads_survives_a_reply() {
        let (_c, w) = fixture();
        w.set(
            &claude("s1"),
            "a1",
            SetWorking {
                thread_ids: None,
                message: Some("Refactoring".into()),
            },
        );
        assert!(w.thread_done("s1", "a1", "t9").is_empty());
        assert_eq!(w.for_artifact("a1").len(), 1);
    }

    #[test]
    fn thread_gone_touches_every_session_on_the_artifact() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        w.mark(
            &Actor {
                session_id: "s2".into(),
                harness: "codex".into(),
                agent: "a_c0de".into(),
            },
            "a1",
            &ids(&["t1", "t2"]),
        );
        w.thread_gone("a1", "t1");
        let v = w.for_artifact("a1");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].harness, "codex");
        assert_eq!(v[0].thread_ids, ids(&["t2"]));
    }

    #[test]
    fn end_session_clear_and_artifact_gone() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        w.mark(&claude("s1"), "a2", &[]);
        w.mark(&claude("s2"), "a2", &[]);
        assert_eq!(
            w.end_session("s1").0,
            ["a1".to_string(), "a2".to_string()].into()
        );
        assert_eq!(w.for_artifact("a2").len(), 1);
        assert!(!w.clear("s2", "a2", None).is_empty());
        w.mark(&claude("s3"), "a3", &[]);
        assert!(!w.artifact_gone("a3").is_empty());
        assert!(w.all().is_empty());
    }

    #[test]
    fn clear_with_threads_removes_only_those() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.clear("s1", "a1", Some(&ids(&["t1"])));
        assert_eq!(w.threads_of("s1", "a1"), ids(&["t2"]));
    }

    #[test]
    fn views_are_newest_first_and_carry_no_session_id() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(1);
        w.mark(
            &Actor {
                session_id: "s2".into(),
                harness: "pi".into(),
                agent: "a_beef".into(),
            },
            "a1",
            &[],
        );
        let v = w.for_artifact("a1");
        assert_eq!(v[0].harness, "pi");
        let json = serde_json::to_string(&v).unwrap();
        assert!(
            !json.contains("s1") && !json.contains("s2") && !json.contains("session"),
            "{json}"
        );
        assert_ne!(v[0].key, v[1].key);
        assert!(v.iter().all(|x| x.agent.starts_with("a_")));
        assert_eq!(v[0].agent, "a_beef");
        assert_eq!(w.for_session("s2")[0].session_id, "s2");
        assert_eq!(w.for_session("s2")[0].artifact_id, "a1");
    }

    #[test]
    fn skew_moves_the_registry_clock() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        w.skew(121);
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn messages_are_one_line_and_bounded() {
        assert_eq!(
            clean_message("  a\n\tb\u{2028}c\u{7}d  "),
            (Some("a b cd".into()), false)
        );
        assert_eq!(clean_message(" \n "), (None, false));
        let long = "x".repeat(141);
        let (m, cut) = clean_message(&long);
        assert!(cut);
        assert_eq!(m.as_deref().unwrap().chars().count(), 140);
        assert!(m.unwrap().ends_with('…'));
        assert_eq!(
            clean_message(&"é".repeat(140)),
            (Some("é".repeat(140)), false)
        );
    }
}

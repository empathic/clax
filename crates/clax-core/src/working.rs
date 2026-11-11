//! The working signal (spec §10 "Working"): which harness session is acting
//! on which artifact, and on which of its threads, now. Records live in
//! memory only. Each lapses [`WORKING_TTL_SECS`] after its last renewal
//! ([`BACKGROUND_TTL_SECS`] while it names background threads), so a daemon
//! restart starts with none and never shows stale work.
//!
//! A thread the agent hands to work that goes on after its turn (subagents
//! running in the background) is flagged background: a turn end and a
//! publish keep it, and it ends when the agent replies to or resolves it,
//! clears it, or the record lapses.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Seconds after its last renewal that a record lapses.
pub const WORKING_TTL_SECS: i64 = 120;
/// Seconds after its last renewal that a record naming background threads
/// lapses: work in the background may renew it only through its own tool
/// calls, or not at all where the harness runs it as another session.
pub const BACKGROUND_TTL_SECS: i64 = 600;
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
    /// The session's agent name (spec 2026-10-08 §11) when the record was
    /// made.
    pub name: Option<String>,
}

/// A record as anyone may read it: never names the session.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkingView {
    /// A ULID minted when the record was created.
    pub key: String,
    /// The agent's handle: tells two sessions of one harness apart.
    pub agent: String,
    pub harness: String,
    /// Its session's agent name when the record was made, which it shows
    /// as.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: Option<String>,
    pub thread_ids: Vec<String>,
    /// Those of `thread_ids` handed to work in the background, which
    /// outlive the agent's turn; absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub background_thread_ids: Vec<String>,
    pub started_at: String,
    pub last_heartbeat: String,
    /// The session the record is of, for the visibility filter (a
    /// record's message is its session's person's). Never served.
    #[serde(skip)]
    pub session_id: String,
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

/// An explicit update: each `Some` field replaces the stored one, except
/// that with `background` the `thread_ids` join the record's threads (none
/// is dropped) and are flagged background.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetWorking {
    pub thread_ids: Option<Vec<String>>,
    pub message: Option<String>,
    pub background: bool,
}

/// Why a working record ended (audit spec 2026-10-06 §6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// Its session cleared it, replied to its last thread, published, or
    /// ended its turn.
    Explicit,
    /// It lapsed [`WORKING_TTL_SECS`] after its last renewal.
    Ttl,
    /// Its session ended.
    SessionEnd,
    /// Its last thread was resolved, by its agent or a viewer.
    Resolved,
    /// Its last thread, or its artifact, was deleted.
    Deleted,
}

impl StopReason {
    /// The recorded name: `explicit`, `ttl`, `session_end`, `resolved` or
    /// `deleted`.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Explicit => "explicit",
            StopReason::Ttl => "ttl",
            StopReason::SessionEnd => "session_end",
            StopReason::Resolved => "resolved",
            StopReason::Deleted => "deleted",
        }
    }
}

/// A record's start or end, as the audit journal records it (spec §6.5).
/// Renewals and updates of a record that goes on are neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transition {
    /// A record was made, with its first message and threads.
    Started {
        session_id: String,
        artifact_id: String,
        key: String,
        message: Option<String>,
        thread_ids: Vec<String>,
    },
    /// A record ended, `duration_ms` after it started. A lapsed record
    /// ends when it lapsed, however much later it is removed, and with
    /// reason [`StopReason::Ttl`].
    Stopped {
        session_id: String,
        artifact_id: String,
        key: String,
        harness: String,
        agent: String,
        reason: StopReason,
        duration_ms: i64,
    },
}

/// What a registry change did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    /// The artifacts whose working list changed.
    pub artifacts: BTreeSet<String>,
    /// The records that started or ended on the way, in order.
    pub transitions: Vec<Transition>,
}

impl Changed {
    pub fn merge(&mut self, other: Changed) {
        self.artifacts.extend(other.artifacts);
        self.transitions.extend(other.transitions);
    }
    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }
    fn one(aid: &str) -> Changed {
        Changed {
            artifacts: [aid.to_string()].into(),
            transitions: Vec::new(),
        }
    }
}

/// Why a record ends. Only [`End::Done`] (the agent said so) and
/// [`End::TurnEnd`] (its turn ended) report the record as finished work
/// (spec 2026-10-06-agent-questions-and-inbox-design §7.1); a publish, a
/// reply or resolve, a lapse and the session's end make none. A turn end
/// and a publish keep a record's background threads: they end only its
/// other threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Done,
    TurnEnd,
    Publish,
    Thread,
    Lapse,
    SessionEnd,
}

impl End {
    fn finishes(self) -> bool {
        matches!(self, End::Done | End::TurnEnd)
    }

    fn spares_background(self) -> bool {
        matches!(self, End::TurnEnd | End::Publish)
    }

    /// The reason the audit journal records for a record ended this way
    /// (a lapsed record is recorded as [`StopReason::Ttl`] whatever ends it).
    fn stop_reason(self) -> StopReason {
        match self {
            End::SessionEnd => StopReason::SessionEnd,
            End::Lapse => StopReason::Ttl,
            End::Done | End::TurnEnd | End::Publish | End::Thread => StopReason::Explicit,
        }
    }
}

/// A record that ended as finished work: what the inbox keeps of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub session_id: String,
    pub artifact_id: String,
    /// The record's key ([`WorkingView::key`]).
    pub key: String,
    pub message: Option<String>,
    pub thread_ids: Vec<String>,
}

struct Record {
    key: String,
    agent: String,
    harness: String,
    name: Option<String>,
    message: Option<String>,
    threads: Vec<String>,
    /// Those of `threads` flagged background, in the order flagged.
    background: Vec<String>,
    /// The record has named a thread at some point: losing its last one clears it.
    had_threads: bool,
    started_at: DateTime<Utc>,
    heartbeat: DateTime<Utc>,
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl Record {
    fn view(&self, sid: &str) -> WorkingView {
        WorkingView {
            key: self.key.clone(),
            agent: self.agent.clone(),
            harness: self.harness.clone(),
            name: self.name.clone(),
            message: self.message.clone(),
            thread_ids: self.threads.clone(),
            background_thread_ids: self.background.clone(),
            started_at: stamp(self.started_at),
            last_heartbeat: stamp(self.heartbeat),
            session_id: sid.to_string(),
        }
    }
    fn ended(&self, sid: &str, aid: &str) -> Ended {
        Ended {
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            key: self.key.clone(),
            message: self.message.clone(),
            thread_ids: self.threads.clone(),
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
    /// How long after its last renewal the record lapses.
    fn ttl(&self) -> Duration {
        Duration::seconds(if self.background.is_empty() {
            WORKING_TTL_SECS
        } else {
            BACKGROUND_TTL_SECS
        })
    }
    /// Keeps only the background threads; whether any other went.
    fn spare_background(&mut self) -> bool {
        let n = self.threads.len();
        let background = &self.background;
        self.threads.retain(|t| background.contains(t));
        self.threads.len() != n
    }
}

/// [`clean_line`] bounded by [`MAX_MESSAGE_CHARS`].
pub fn clean_message(raw: &str) -> (Option<String>, bool) {
    clean_line(raw, MAX_MESSAGE_CHARS)
}

/// `raw` as one line: whitespace runs (line and paragraph separators included)
/// become one space, other control characters and hidden format characters
/// ([`crate::text::is_hidden_char`]: bidi controls, zero-width spaces, tags)
/// are dropped, the ends are trimmed. Empty is `None`. Past `max` characters
/// it is cut to one character less plus `…`; the flag says so.
pub fn clean_line(raw: &str, max: usize) -> (Option<String>, bool) {
    let kept: String = raw
        .chars()
        .filter(|&c| c.is_whitespace() || !(c.is_control() || crate::text::is_hidden_char(c)))
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
        now - r.heartbeat < r.ttl()
    }

    /// The record of `who` on `aid`, made when missing (`true`), renewed.
    fn upsert<'a>(
        map: &'a mut BTreeMap<Key, Record>,
        who: &Actor,
        aid: &str,
        now: DateTime<Utc>,
    ) -> (bool, &'a mut Record) {
        let mut made = false;
        let r = map
            .entry((who.session_id.clone(), aid.to_string()))
            .or_insert_with(|| {
                made = true;
                Record {
                    key: crate::new_ulid(),
                    agent: who.agent.clone(),
                    harness: who.harness.clone(),
                    name: who.name.clone(),
                    message: None,
                    threads: Vec::new(),
                    background: Vec::new(),
                    had_threads: false,
                    started_at: now,
                    heartbeat: now,
                }
            });
        r.heartbeat = now;
        (made, r)
    }

    fn started(sid: &str, aid: &str, r: &Record) -> Transition {
        Transition::Started {
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            key: r.key.clone(),
            message: r.message.clone(),
            thread_ids: r.threads.clone(),
        }
    }

    /// The end of record (`sid`, `aid`) at `now` for `reason`; a record
    /// that had already lapsed ended then, for [`StopReason::Ttl`].
    fn stopped(
        &self,
        sid: &str,
        aid: &str,
        r: &Record,
        reason: StopReason,
        now: DateTime<Utc>,
    ) -> Transition {
        let (end, reason) = if self.live(r, now) {
            (now, reason)
        } else {
            (r.heartbeat + r.ttl(), StopReason::Ttl)
        };
        Transition::Stopped {
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            key: r.key.clone(),
            harness: r.harness.clone(),
            agent: r.agent.clone(),
            reason,
            duration_ms: (end - r.started_at).num_milliseconds().max(0),
        }
    }

    fn session_view(sid: &str, aid: &str, r: &Record) -> SessionWorking {
        SessionWorking {
            view: r.view(sid),
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            expires_at: stamp(r.heartbeat + r.ttl()),
        }
    }

    /// Drops lapsed records from `map` first, so a lapsed record is never updated in place.
    fn prune(&self, map: &mut BTreeMap<Key, Record>, now: DateTime<Utc>) -> Changed {
        let mut changed = Changed::default();
        map.retain(|(sid, aid), r| {
            let keep = self.live(r, now);
            if !keep {
                changed.artifacts.insert(aid.clone());
                changed
                    .transitions
                    .push(self.stopped(sid, aid, r, StopReason::Ttl, now));
            }
            keep
        });
        changed
    }

    /// Creates or updates the record; `Some` fields replace (an empty thread
    /// list clears the threads without clearing the record), except that
    /// with `background` the threads are added and flagged background.
    /// Always renews.
    pub fn set(&self, who: &Actor, aid: &str, s: SetWorking) -> (SessionWorking, Changed) {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let (made, r) = Self::upsert(&mut map, who, aid, now);
        if let Some(t) = s.thread_ids {
            if s.background {
                r.add(&t);
                for x in &t {
                    if r.threads.contains(x) && !r.background.contains(x) {
                        r.background.push(x.clone());
                    }
                }
            } else {
                r.threads.clear();
                r.had_threads = false;
                r.add(&t);
                let threads = &r.threads;
                r.background.retain(|x| threads.contains(x));
            }
        }
        if let Some(m) = s.message {
            r.message = Some(m);
        }
        changed.merge(Changed::one(aid));
        if made {
            changed
                .transitions
                .push(Self::started(&who.session_id, aid, r));
        }
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
        let (made, r) = Self::upsert(&mut map, who, aid, now);
        r.add(threads);
        if before.as_ref() != Some(&r.threads) {
            changed.merge(Changed::one(aid));
        }
        if made {
            changed
                .transitions
                .push(Self::started(&who.session_id, aid, r));
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

    /// Removes `threads` from the record, or the record when `None`, for
    /// reason `why`. A record that named threads and has none left is
    /// removed. A removed live record is also returned as [`Ended`] when
    /// `why` finishes work ([`End::Done`], [`End::TurnEnd`]); its message
    /// and threads are those it held before this call.
    pub fn clear(
        &self,
        sid: &str,
        aid: &str,
        threads: Option<&[String]>,
        why: End,
    ) -> (Changed, Vec<Ended>) {
        self.clear_for(sid, aid, threads, why, why.stop_reason())
    }

    /// [`Working::clear`], ending a removed record for `reason`.
    fn clear_for(
        &self,
        sid: &str,
        aid: &str,
        threads: Option<&[String]>,
        why: End,
        reason: StopReason,
    ) -> (Changed, Vec<Ended>) {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let key = (sid.to_string(), aid.to_string());
        let Some(r) = map.get_mut(&key) else {
            return (Changed::default(), Vec::new());
        };
        let before = self.live(r, now).then(|| r.ended(sid, aid));
        let removed = match threads {
            None if why.spares_background() && !r.background.is_empty() => {
                let changed = if r.spare_background() {
                    Changed::one(aid)
                } else {
                    Changed::default()
                };
                return (changed, Vec::new());
            }
            None => true,
            Some(ts) => {
                let n = r.threads.len();
                r.threads.retain(|t| !ts.contains(t));
                if r.threads.len() == n {
                    return (Changed::default(), Vec::new());
                }
                r.background.retain(|t| !ts.contains(t));
                r.had_threads && r.threads.is_empty()
            }
        };
        let mut changed = Changed::one(aid);
        let mut ended = Vec::new();
        if removed && let Some(r) = map.remove(&key) {
            changed
                .transitions
                .push(self.stopped(sid, aid, &r, reason, now));
            if why.finishes() {
                ended.extend(before);
            }
        }
        (changed, ended)
    }

    /// The session replied to ([`StopReason::Explicit`]) or resolved
    /// ([`StopReason::Resolved`]) `tid`: renews the session, then takes
    /// `tid` out of its record on `aid`, ending the record for `reason`
    /// when it was its last thread.
    pub fn thread_done(&self, sid: &str, aid: &str, tid: &str, reason: StopReason) -> Changed {
        self.renew(sid);
        self.clear_for(sid, aid, Some(&[tid.to_string()]), End::Thread, reason)
            .0
    }

    /// `tid` was resolved by a viewer ([`StopReason::Resolved`]) or deleted
    /// ([`StopReason::Deleted`]): out of every record on `aid`.
    pub fn thread_gone(&self, aid: &str, tid: &str, reason: StopReason) -> Changed {
        let sessions: Vec<String> = {
            let map = self.records.lock().unwrap();
            map.keys()
                .filter(|(_, a)| a == aid)
                .map(|(s, _)| s.clone())
                .collect()
        };
        let mut changed = Changed::default();
        for s in sessions {
            changed.merge(
                self.clear_for(&s, aid, Some(&[tid.to_string()]), End::Thread, reason)
                    .0,
            );
        }
        changed
    }

    /// Every record of `sid`, for reason `why` ([`End::TurnEnd`] or
    /// [`End::SessionEnd`]); the removed live records are also returned as
    /// [`Ended`] when `why` finishes work. A lapsed record is removed but
    /// never reported: it ended by lapsing. A turn end keeps a live
    /// record's background threads, ending only its other threads.
    pub fn end_session(&self, sid: &str, why: End) -> (Changed, Vec<Ended>) {
        let now = self.now();
        let mut changed = Changed::default();
        let mut ended = Vec::new();
        self.records.lock().unwrap().retain(|(s, aid), r| {
            if s == sid && why.spares_background() && !r.background.is_empty() && self.live(r, now)
            {
                if r.spare_background() {
                    changed.artifacts.insert(aid.clone());
                }
                return true;
            }
            let keep = s != sid;
            if !keep {
                changed.artifacts.insert(aid.clone());
                changed
                    .transitions
                    .push(self.stopped(s, aid, r, why.stop_reason(), now));
                if why.finishes() && self.live(r, now) {
                    ended.push(r.ended(s, aid));
                }
            }
            keep
        });
        (changed, ended)
    }

    /// Every record on `aid`, ended for [`StopReason::Deleted`].
    pub fn artifact_gone(&self, aid: &str) -> Changed {
        let now = self.now();
        let mut changed = Changed::default();
        self.records.lock().unwrap().retain(|(s, a), r| {
            let keep = a != aid;
            if !keep {
                changed.artifacts.insert(a.clone());
                changed
                    .transitions
                    .push(self.stopped(s, a, r, StopReason::Deleted, now));
            }
            keep
        });
        changed
    }

    /// Removes lapsed records; the artifacts whose list changed.
    pub fn sweep(&self) -> Changed {
        let now = self.now();
        self.prune(&mut self.records.lock().unwrap(), now)
    }

    /// When the next record lapses (from then on [`Working::sweep`] removes
    /// it); `None` with none held. A host that sweeps on a timer of its own
    /// choosing sets it for then.
    pub fn next_lapse(&self) -> Option<DateTime<Utc>> {
        self.records
            .lock()
            .unwrap()
            .values()
            .map(|r| r.heartbeat + r.ttl())
            .min()
    }

    /// The live records on `aid`, newest `started_at` first.
    pub fn for_artifact(&self, aid: &str) -> Vec<WorkingView> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        let mut v: Vec<_> = map
            .iter()
            .filter(|((_, a), r)| a == aid && self.live(r, now))
            .map(|((s, _), r)| (r.started_at, r.view(s)))
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

    /// The artifact of session `sid`'s most recently renewed live record
    /// (the latest started among equals), if it has one: what a mirrored
    /// question with no artifact of its own is about.
    pub fn newest_artifact_of(&self, sid: &str) -> Option<String> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.iter()
            .filter(|((s, _), r)| s == sid && self.live(r, now))
            .max_by_key(|(_, r)| (r.heartbeat, r.started_at))
            .map(|((_, a), _)| a.clone())
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

    /// The threads the live record (`sid`, `aid`) names that are not
    /// background: those a publish addresses unasked.
    pub fn foreground_threads_of(&self, sid: &str, aid: &str) -> Vec<String> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.get(&(sid.to_string(), aid.to_string()))
            .filter(|r| self.live(r, now))
            .map(|r| {
                r.threads
                    .iter()
                    .filter(|t| !r.background.contains(t))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every live record with its session and artifact (the working roster),
    /// newest `started_at` first.
    pub fn roster(&self) -> Vec<SessionWorking> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        let mut v: Vec<_> = map
            .iter()
            .filter(|(_, r)| self.live(r, now))
            .map(|((s, a), r)| (r.started_at, Self::session_view(s, a, r)))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.view.key.cmp(&a.1.view.key)));
        v.into_iter().map(|(_, x)| x).collect()
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
            name: None,
        }
    }
    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_next_lapse_is_the_oldest_renewal_plus_the_ttl() {
        let (c, w) = fixture();
        assert_eq!(w.next_lapse(), None);
        w.mark(&claude("s1"), "a1", &[]);
        let first = c.now() + Duration::seconds(WORKING_TTL_SECS);
        c.advance(30);
        w.mark(&claude("s2"), "a2", &[]);
        assert_eq!(w.next_lapse(), Some(first));
        c.advance(WORKING_TTL_SECS - 30);
        w.sweep();
        assert_eq!(
            w.next_lapse(),
            Some(first + Duration::seconds(30)),
            "s1's record lapsed at its time and is gone"
        );
        c.advance(30);
        w.sweep();
        assert_eq!(w.next_lapse(), None);
    }

    #[test]
    fn the_newest_artifact_is_the_most_recently_renewed_live_record() {
        let (c, w) = fixture();
        assert_eq!(w.newest_artifact_of("s1"), None);
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(5);
        w.mark(&claude("s1"), "a2", &[]);
        w.mark(&claude("s2"), "a3", &[]);
        assert_eq!(w.newest_artifact_of("s1").as_deref(), Some("a2"));
        c.advance(5);
        w.mark(&claude("s1"), "a1", &[]);
        assert_eq!(w.newest_artifact_of("s1").as_deref(), Some("a1"));
        // Renewed together: the later started wins.
        w.renew("s1");
        assert_eq!(w.newest_artifact_of("s1").as_deref(), Some("a2"));
        c.advance(WORKING_TTL_SECS + 1);
        assert_eq!(w.newest_artifact_of("s1"), None);
    }

    #[test]
    fn mark_creates_then_adds_threads_and_keeps_the_key() {
        let (_c, w) = fixture();
        assert_eq!(
            w.mark(&claude("s1"), "a1", &ids(&["t1"])).artifacts,
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
    fn the_roster_names_sessions_and_artifacts_newest_first_and_drops_lapsed() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        c.advance(100);
        w.mark(&claude("s2"), "a2", &ids(&["t2"]));
        let r = w.roster();
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].session_id.as_str(), r[0].artifact_id.as_str()),
            ("s2", "a2")
        );
        assert_eq!(
            (r[1].session_id.as_str(), r[1].artifact_id.as_str()),
            ("s1", "a1")
        );
        assert_eq!(r[1].view.thread_ids, ids(&["t1"]));
        c.advance(30);
        let r = w.roster();
        assert_eq!(
            r.len(),
            1,
            "s1's record lapsed 120 s after its last renewal"
        );
        assert_eq!(r[0].session_id, "s2");
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
                ..SetWorking::default()
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
                ..SetWorking::default()
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
        assert_eq!(w.sweep().artifacts, ["a1".to_string()].into());
        assert!(w.sweep().is_empty(), "swept once");
        assert_eq!(w.renew("s1"), 0, "a lapsed record is not revived");
    }

    #[test]
    fn replying_to_the_last_named_thread_clears_the_record() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.thread_done("s1", "a1", "t1", StopReason::Explicit);
        assert_eq!(w.for_artifact("a1")[0].thread_ids, ids(&["t2"]));
        assert!(
            !w.thread_done("s1", "a1", "t2", StopReason::Explicit)
                .is_empty()
        );
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
                ..SetWorking::default()
            },
        );
        assert!(
            w.thread_done("s1", "a1", "t9", StopReason::Explicit)
                .is_empty()
        );
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
                name: None,
            },
            "a1",
            &ids(&["t1", "t2"]),
        );
        w.thread_gone("a1", "t1", StopReason::Resolved);
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
            w.end_session("s1", End::SessionEnd).0.artifacts,
            ["a1".to_string(), "a2".to_string()].into()
        );
        assert_eq!(w.for_artifact("a2").len(), 1);
        assert!(!w.clear("s2", "a2", None, End::Publish).0.is_empty());
        w.mark(&claude("s3"), "a3", &[]);
        assert!(!w.artifact_gone("a3").is_empty());
        assert!(w.all().is_empty());
    }

    #[test]
    fn clear_with_threads_removes_only_those() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.clear("s1", "a1", Some(&ids(&["t1"])), End::Done);
        assert_eq!(w.threads_of("s1", "a1"), ids(&["t2"]));
    }

    #[test]
    fn only_done_and_turn_end_report_finished_work() {
        let (c, w) = fixture();
        let msg = SetWorking {
            thread_ids: Some(ids(&["t1"])),
            message: Some("Recoloured".into()),
            ..SetWorking::default()
        };
        w.set(&claude("s1"), "a1", msg.clone());
        let (changed, ended) = w.clear("s1", "a1", None, End::Done);
        assert!(!changed.is_empty());
        assert_eq!(ended.len(), 1);
        assert_eq!(
            (ended[0].session_id.as_str(), ended[0].artifact_id.as_str()),
            ("s1", "a1")
        );
        assert_eq!(ended[0].message.as_deref(), Some("Recoloured"));
        assert_eq!(ended[0].thread_ids, ids(&["t1"]));
        assert!(!ended[0].key.is_empty());
        // Done that takes the last named thread ends the record with it.
        w.set(&claude("s1"), "a1", msg.clone());
        let (_, ended) = w.clear("s1", "a1", Some(&ids(&["t1"])), End::Done);
        assert_eq!(ended[0].thread_ids, ids(&["t1"]));
        for why in [End::Publish, End::Thread, End::Lapse, End::SessionEnd] {
            w.set(&claude("s1"), "a1", msg.clone());
            assert!(w.clear("s1", "a1", None, why).1.is_empty(), "{why:?}");
        }
        w.set(&claude("s1"), "a1", msg.clone());
        w.set(&claude("s1"), "a2", msg.clone());
        assert!(w.end_session("s1", End::SessionEnd).1.is_empty());
        w.set(&claude("s1"), "a1", msg.clone());
        w.set(&claude("s1"), "a2", msg.clone());
        let (_, ended) = w.end_session("s1", End::TurnEnd);
        assert_eq!(ended.len(), 2);
        w.set(&claude("s1"), "a1", msg);
        c.advance(WORKING_TTL_SECS);
        assert!(
            w.end_session("s1", End::TurnEnd).1.is_empty(),
            "a lapsed record ended by lapsing"
        );
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
                name: None,
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

    fn started_keys(c: &Changed) -> Vec<&str> {
        c.transitions
            .iter()
            .filter_map(|t| match t {
                Transition::Started { key, .. } => Some(key.as_str()),
                Transition::Stopped { .. } => None,
            })
            .collect()
    }

    fn stops(c: &Changed) -> Vec<(StopReason, i64)> {
        c.transitions
            .iter()
            .filter_map(|t| match t {
                Transition::Stopped {
                    reason,
                    duration_ms,
                    ..
                } => Some((*reason, *duration_ms)),
                Transition::Started { .. } => None,
            })
            .collect()
    }

    #[test]
    fn records_report_their_starts_and_ends_but_not_renewals() {
        let (c, w) = fixture();
        let (_, made) = w.set(
            &claude("s1"),
            "a1",
            SetWorking {
                thread_ids: Some(ids(&["t1"])),
                message: Some("m".into()),
                ..SetWorking::default()
            },
        );
        let key = w.for_artifact("a1")[0].key.clone();
        assert_eq!(
            made.transitions,
            [Transition::Started {
                session_id: "s1".into(),
                artifact_id: "a1".into(),
                key: key.clone(),
                message: Some("m".into()),
                thread_ids: ids(&["t1"]),
            }]
        );
        // Updates, renewals and marks of a record that goes on: nothing.
        assert!(
            w.set(&claude("s1"), "a1", SetWorking::default())
                .1
                .transitions
                .is_empty()
        );
        assert!(
            w.mark(&claude("s1"), "a1", &ids(&["t2"]))
                .transitions
                .is_empty()
        );
        w.renew("s1");
        assert!(
            w.clear("s1", "a1", Some(&ids(&["t1"])), End::Done)
                .0
                .transitions
                .is_empty()
        );
        c.advance(5);
        // Its last thread resolved: an end for that reason, 5 s after it
        // started.
        assert_eq!(
            stops(&w.thread_done("s1", "a1", "t2", StopReason::Resolved)),
            [(StopReason::Resolved, 5000)]
        );
        // A viewer's resolve or a delete of the last thread, and a deleted
        // artifact, name their cause.
        w.mark(&claude("s5"), "a6", &ids(&["t6"]));
        assert_eq!(
            stops(&w.thread_gone("a6", "t6", StopReason::Deleted)),
            [(StopReason::Deleted, 0)]
        );
        w.mark(&claude("s5"), "a6", &[]);
        assert_eq!(stops(&w.artifact_gone("a6")), [(StopReason::Deleted, 0)]);
        // A mark makes a record too.
        assert_eq!(started_keys(&w.mark(&claude("s1"), "a2", &[])).len(), 1);
        assert_eq!(
            stops(&w.end_session("s1", End::SessionEnd).0),
            [(StopReason::SessionEnd, 0)]
        );
        // A lapsed record ends when it lapsed, whoever removes it.
        w.mark(&claude("s2"), "a3", &[]);
        c.advance(300);
        assert_eq!(stops(&w.artifact_gone("a3")), [(StopReason::Ttl, 120_000)]);
        w.mark(&claude("s3"), "a4", &[]);
        c.advance(121);
        let swept = w.mark(&claude("s4"), "a5", &[]);
        assert_eq!(stops(&swept), [(StopReason::Ttl, 120_000)]);
        assert_eq!(started_keys(&swept).len(), 1);
        assert!(w.sweep().transitions.is_empty());
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
        assert_eq!(
            clean_message("fixed \u{202E}etadpu\u{200B}\u{2066}x\u{2069}"),
            (Some("fixed etadpux".into()), false)
        );
        assert_eq!(
            clean_message("\u{1F469}\u{200D}\u{1F4BB} \u{200F}abc"),
            (Some("\u{1F469}\u{200D}\u{1F4BB} \u{200F}abc".into()), false)
        );
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

    fn background(ts: &[&str]) -> SetWorking {
        SetWorking {
            thread_ids: Some(ids(ts)),
            background: true,
            ..SetWorking::default()
        }
    }

    #[test]
    fn background_threads_join_the_record_without_replacing_its_threads() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t2"]));
        let (v, changed) = w.set(&claude("s1"), "a1", background(&["t1"]));
        assert_eq!(changed.artifacts, ["a1".to_string()].into());
        assert_eq!(v.view.thread_ids, ids(&["t2", "t1"]));
        assert_eq!(v.view.background_thread_ids, ids(&["t1"]));
    }

    #[test]
    fn a_turn_end_keeps_the_background_threads_and_ends_the_others() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.set(&claude("s1"), "a1", background(&["t1"]));
        w.mark(&claude("s1"), "a2", &ids(&["t3"]));
        let (changed, ended) = w.end_session("s1", End::TurnEnd);
        assert_eq!(
            changed.artifacts,
            ["a1".to_string(), "a2".to_string()].into()
        );
        assert_eq!(
            ended
                .iter()
                .map(|e| e.artifact_id.as_str())
                .collect::<Vec<_>>(),
            ["a2"],
            "only the record that ended is finished work"
        );
        let v = w.for_artifact("a1");
        assert_eq!(v[0].thread_ids, ids(&["t1"]));
        assert_eq!(v[0].background_thread_ids, ids(&["t1"]));
        assert!(w.for_artifact("a2").is_empty());
        let (changed, _) = w.end_session("s1", End::TurnEnd);
        assert!(changed.is_empty(), "a later turn end leaves them be");
        assert_eq!(w.for_artifact("a1")[0].thread_ids, ids(&["t1"]));
    }

    #[test]
    fn a_publish_keeps_the_background_threads_and_done_clears_them() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t2"]));
        w.set(&claude("s1"), "a1", background(&["t1"]));
        assert_eq!(w.foreground_threads_of("s1", "a1"), ids(&["t2"]));
        w.clear("s1", "a1", None, End::Publish);
        assert_eq!(w.threads_of("s1", "a1"), ids(&["t1"]));
        assert!(w.foreground_threads_of("s1", "a1").is_empty());
        let (_, ended) = w.clear("s1", "a1", None, End::Done);
        assert_eq!(ended.len(), 1);
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn replying_to_the_last_background_thread_ends_the_record() {
        let (_c, w) = fixture();
        w.set(&claude("s1"), "a1", background(&["t1", "t2"]));
        w.end_session("s1", End::TurnEnd);
        w.thread_done("s1", "a1", "t1", StopReason::Explicit);
        assert_eq!(w.for_artifact("a1")[0].background_thread_ids, ids(&["t2"]));
        w.thread_done("s1", "a1", "t2", StopReason::Explicit);
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn a_record_with_background_threads_lapses_after_the_longer_ttl() {
        let (c, w) = fixture();
        let (v, _) = w.set(&claude("s1"), "a1", background(&["t1"]));
        let lapse = c.now() + Duration::seconds(BACKGROUND_TTL_SECS);
        assert_eq!(v.expires_at, stamp(lapse));
        assert_eq!(w.next_lapse(), Some(lapse));
        c.advance(WORKING_TTL_SECS + 1);
        assert_eq!(w.for_artifact("a1").len(), 1);
        c.advance(BACKGROUND_TTL_SECS - WORKING_TTL_SECS - 1);
        let changed = w.sweep();
        assert!(w.for_artifact("a1").is_empty());
        assert!(matches!(
            changed.transitions[..],
            [Transition::Stopped {
                reason: StopReason::Ttl,
                ..
            }]
        ));
    }

    #[test]
    fn renaming_the_threads_keeps_the_background_flag_only_on_threads_still_named() {
        let (c, w) = fixture();
        w.set(&claude("s1"), "a1", background(&["t1"]));
        let (v, _) = w.set(
            &claude("s1"),
            "a1",
            SetWorking {
                thread_ids: Some(ids(&["t2"])),
                ..SetWorking::default()
            },
        );
        assert!(v.view.background_thread_ids.is_empty());
        assert_eq!(
            v.expires_at,
            stamp(c.now() + Duration::seconds(WORKING_TTL_SECS))
        );
    }

    #[test]
    fn the_session_end_ends_background_threads_too() {
        let (_c, w) = fixture();
        w.set(&claude("s1"), "a1", background(&["t1"]));
        w.end_session("s1", End::SessionEnd);
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn background_threads_are_served_only_when_there_are_some() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        let v = serde_json::to_value(&w.for_artifact("a1")[0]).unwrap();
        assert!(v.get("background_thread_ids").is_none(), "{v}");
        w.set(&claude("s1"), "a1", background(&["t1"]));
        let v = serde_json::to_value(&w.for_artifact("a1")[0]).unwrap();
        assert_eq!(v["background_thread_ids"], serde_json::json!(["t1"]));
    }
}

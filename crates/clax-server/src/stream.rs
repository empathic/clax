//! The multiplexed event stream behind `GET /api/stream`: one connection per
//! client, with topics subscribed and unsubscribed over `POST
//! /api/stream/<id>` while it stays open.
//!
//! Every event published on the bus reaches [`Hub::dispatch`] on the
//! publishing thread. Under one lock the hub numbers the event (one sequence
//! for the whole daemon), projects it once per channel that has subscribers
//! into a small JSON delta, keeps it in that channel's ring for resume, and
//! offers it to each subscriber's bounded queue. Topics nobody subscribed to
//! cost one map lookup and nothing else.
//!
//! A client whose queue is full is behind: the hub stops offering it that
//! topic's events and the client gets `resync` for the topic, its queued
//! events for the topic dropped. Memory per client is its queue
//! ([`QUEUE`] pointers into shared items) and one entry per subscription.
//!
//! A connection that drops leaves its stream detached for [`GRACE`]: a
//! reconnect naming it in `Last-Event-ID` (`<stream>:<seq>`) gets the same
//! subscriptions back and every event after `<seq>` still in the rings, or
//! `resync` for a topic whose ring no longer reaches back that far.
//!
//! One lock guards the hub, so one sequence orders every channel and a
//! resume point names a place in all of a stream's topics at once. A
//! dispatch holds it for one projection per channel and one `try_send` per
//! subscriber: about 60 µs for an event 5,000 subscribers receive and
//! 250 µs for 20,000 (release build, Apple M-series). Opening, subscribing,
//! detaching and sweeping cost the stream's own topics and a log-time index
//! of detached streams, never a scan of every stream. Sharding the channels
//! would need an ordering step for resume and is not worth it at that cost.

use axum::body::Bytes;
use clax_core::db::{Caller, Level};
use clax_core::presence::PresenceView;
use clax_core::{ArtifactId, Event};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};

/// Events one client may have queued before it counts as behind.
pub const QUEUE: usize = 64;
/// Recent events each channel keeps for resume.
pub const RING: usize = 64;
/// Topics one stream may hold. A browser's one shared stream carries the
/// union of every open tab's topics (four per artifact view), so the bound
/// leaves room for dozens of tabs while keeping each client's cost fixed.
pub const MAX_TOPICS: usize = 256;
/// How long a dropped connection's stream waits for a reconnect.
pub const GRACE: Duration = Duration::from_secs(60);
/// Detached streams kept at most; the longest detached goes first.
pub const MAX_DETACHED: usize = 4096;

/// A topic a client subscribes to, by name: `gallery`, `artifact:<id>`,
/// `presence:<id>`, `working:<id>`, `docs:<id>`, and `site:<origin>` (the
/// `artifact` events of every live page of a site; spec
/// 2026-10-05-chrome-overlay-design §9.5).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Topic {
    Gallery,
    Artifact(String),
    Presence(String),
    Working(String),
    Docs(String),
    Site(String),
}

impl Topic {
    /// Parses a topic name; the artifact ID must be well formed, and a
    /// site's origin written as the daemon normalizes it (spec §7: scheme,
    /// lowercased host, port unless the scheme's default; no path).
    ///
    /// # Errors
    /// A message naming what is wrong.
    pub fn parse(s: &str) -> Result<Topic, String> {
        if s == "gallery" {
            return Ok(Topic::Gallery);
        }
        if let Some(origin) = s.strip_prefix("site:") {
            let normal = clax_core::live::parse_page_url(origin)
                .ok()
                .filter(|p| p.key.origin == origin)
                .is_some();
            if !normal {
                return Err(format!("'{origin}' is not a normalized origin"));
            }
            return Ok(Topic::Site(origin.to_string()));
        }
        let (kind, aid) = s
            .split_once(':')
            .ok_or_else(|| format!("unknown topic '{s}'"))?;
        let aid = ArtifactId::parse(aid)
            .map_err(|_| format!("'{aid}' is not an artifact ID"))?
            .as_str()
            .to_string();
        match kind {
            "artifact" => Ok(Topic::Artifact(aid)),
            "presence" => Ok(Topic::Presence(aid)),
            "working" => Ok(Topic::Working(aid)),
            "docs" => Ok(Topic::Docs(aid)),
            _ => Err(format!("unknown topic '{s}'")),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Topic::Gallery => "gallery".into(),
            Topic::Artifact(a) => format!("artifact:{a}"),
            Topic::Presence(a) => format!("presence:{a}"),
            Topic::Working(a) => format!("working:{a}"),
            Topic::Docs(a) => format!("docs:{a}"),
            Topic::Site(o) => format!("site:{o}"),
        }
    }

    /// The artifact the topic is about; `None` for the gallery and a site.
    pub fn artifact(&self) -> Option<&str> {
        match self {
            Topic::Gallery | Topic::Site(_) => None,
            Topic::Artifact(a) | Topic::Presence(a) | Topic::Working(a) | Topic::Docs(a) => Some(a),
        }
    }

    /// The channels a subscriber at `caller` joins. A `docs` topic joins
    /// one channel per level up to the caller's, and the caller's own
    /// viewer channel, which carries its private documents.
    fn chans(&self, caller: &Caller) -> Vec<Chan> {
        match self {
            Topic::Gallery => vec![Chan::Gallery],
            Topic::Artifact(a) => vec![Chan::Artifact(a.clone())],
            Topic::Presence(a) => vec![Chan::Presence(a.clone())],
            Topic::Working(a) => vec![Chan::Working(a.clone())],
            Topic::Site(o) => vec![Chan::Site(o.clone())],
            Topic::Docs(a) => {
                let mut v: Vec<Chan> = [Level::View, Level::Interact, Level::Admin, Level::Owner]
                    .into_iter()
                    .filter(|l| *l <= caller.level)
                    .map(|l| Chan::DocsAt(a.clone(), l))
                    .collect();
                if let Some(me) = &caller.viewer {
                    v.push(Chan::DocsOf(a.clone(), me.clone()));
                }
                v
            }
        }
    }
}

/// Where the hub files an event: a topic's channel. Clients never name a
/// channel; `DocsAt` holds documents readable at a level, `DocsOf` the
/// documents only one viewer may see.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Chan {
    Gallery,
    Artifact(String),
    Presence(String),
    Working(String),
    DocsAt(String, Level),
    DocsOf(String, String),
    /// The `artifact` channel's events of every live page of an origin.
    Site(String),
}

impl Chan {
    fn topic(&self) -> String {
        match self {
            Chan::Gallery => "gallery".into(),
            Chan::Site(o) => format!("site:{o}"),
            Chan::Artifact(a) => format!("artifact:{a}"),
            Chan::Presence(a) => format!("presence:{a}"),
            Chan::Working(a) => format!("working:{a}"),
            Chan::DocsAt(a, _) | Chan::DocsOf(a, _) => format!("docs:{a}"),
        }
    }
}

/// Which subscribers of a channel an item reaches.
#[derive(Clone, Copy, Debug)]
enum Gate {
    Any,
    /// Only subscribers on this machine or holding the token (a live page's
    /// gallery event; see [`crate::live::sees_live_pages`]).
    Local,
    /// Subscribers at `min` or above, except those at `unless` or above
    /// (they have it from a `DocsAt` channel already).
    Level {
        min: Level,
        unless: Option<Level>,
    },
}

impl Gate {
    fn admits(self, level: Level, local: bool) -> bool {
        match self {
            Gate::Any => true,
            Gate::Local => local,
            Gate::Level { min, unless } => level >= min && unless.is_none_or(|u| level < u),
        }
    }
}

/// One event as one topic carries it, shared by every subscriber.
#[derive(Debug)]
struct Item {
    seq: u64,
    topic: Arc<str>,
    /// `event: <name>\ndata: <json>\n`; each client adds its `id:` line.
    frame: Bytes,
    gate: Gate,
}

/// What a connection learns from the hub outside its queue.
#[derive(Default)]
struct Shared {
    /// Topics that fell behind, with the sequence of the first event dropped.
    lagged: Mutex<Vec<(Arc<str>, u64)>>,
    notify: Notify,
}

struct Sub {
    tx: Option<mpsc::Sender<Arc<Item>>>,
    level: Level,
    /// The stream may see live pages.
    local: bool,
    lagged: bool,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Channel {
    subs: HashMap<u64, Sub>,
    ring: VecDeque<Arc<Item>>,
    /// The sequence of the newest event that left the ring; 0 for none.
    evicted: u64,
    /// The last presence list, which the next `presence` event is a diff against.
    people: Vec<PresenceView>,
}

struct StreamEntry {
    key: u64,
    /// Bumped by each connection that attaches; a connection detaches only its own.
    epoch: u64,
    caller: Caller,
    /// The stream may see live pages ([`crate::live::sees_live_pages`] of
    /// the request that opened it).
    local: bool,
    /// The hash of the extension credential the stream was opened with
    /// (through the extension gateway): such a stream takes live pages'
    /// topics alone ([`live_only_admits`]), only requests with the same
    /// credential change or resume it, and it ends when the credential
    /// stops being live ([`Hub::end_streams_where`], [`Conn::checking`]).
    credential: Option<String>,
    /// Each topic with the sequence current when it was subscribed.
    topics: BTreeMap<Topic, u64>,
    tx: Option<mpsc::Sender<Arc<Item>>>,
    shared: Arc<Shared>,
    detached_at: Option<Instant>,
}

#[derive(Default)]
struct Inner {
    seq: u64,
    next_key: u64,
    chans: HashMap<Chan, Channel>,
    streams: HashMap<String, StreamEntry>,
    /// The detached streams, longest detached first (by when, then key), so
    /// a detach, an eviction and a sweep never scan every stream.
    detached: BTreeMap<(Instant, u64), String>,
}

/// Why a subscription change was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SubError {
    /// No such stream, or it belongs to another caller.
    UnknownStream,
    /// The stream would hold more than [`MAX_TOPICS`].
    TooMany,
    /// A topic of a live page, on a stream that may not see live pages:
    /// answered as a missing artifact.
    Hidden,
    /// A topic a live-only stream may not take ([`live_only_admits`]).
    Forbidden,
}

/// Whether a live-only stream (one opened through the extension gateway) may
/// take topic `t`: an `artifact`, `presence` or `working` topic of a live
/// page in `live`; never `gallery` or a `docs` topic (spec
/// 2026-10-05-chrome-overlay-design §9.5).
pub fn live_only_admits(live: &crate::live::LiveIds, t: &Topic) -> bool {
    match t {
        Topic::Gallery | Topic::Docs(_) => false,
        Topic::Site(_) => true,
        Topic::Artifact(a) | Topic::Presence(a) | Topic::Working(a) => live.contains(a),
    }
}

/// The hub: every topic's channel and every stream.
#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
    /// The live pages, whose gallery events reach only local streams.
    live: Arc<crate::live::LiveIds>,
}

/// Counts for logs and tests.
#[derive(Debug, PartialEq, Eq)]
pub struct Stats {
    pub streams: usize,
    pub attached: usize,
    pub channels: usize,
}

/// A connection's view of its stream, as [`Hub::open`] hands it out.
pub struct Opened {
    pub id: String,
    pub resumed: bool,
    pub seq: u64,
    pub topics: Vec<String>,
    epoch: u64,
    rx: mpsc::Receiver<Arc<Item>>,
    shared: Arc<Shared>,
    /// Frames sent before the queue: `resync` for gaps, then the replay.
    prelude: Vec<Bytes>,
}

fn sse(name: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {data}\n"))
}

fn resync_frame(topic: &str, reason: &str) -> Bytes {
    Bytes::from(format!(
        "event: resync\ndata: {}\n\n",
        json!({"topic": topic, "reason": reason})
    ))
}

fn id_line(id: &str, seq: u64) -> Bytes {
    Bytes::from(format!("id: {id}:{seq}\n\n"))
}

/// `v` as an object with `topic` added and `type` removed.
fn with_topic(topic: &str, v: Value) -> Value {
    let mut out = Map::new();
    out.insert("topic".into(), json!(topic));
    if let Value::Object(m) = v {
        for (k, x) in m {
            if k != "type" {
                out.insert(k, x);
            }
        }
    }
    Value::Object(out)
}

/// A thread view without its comments: their count and the newest one.
fn thread_delta(thread: &Value) -> Value {
    let mut t = thread.clone();
    if let Value::Object(m) = &mut t {
        let comments = m.remove("comments").unwrap_or(Value::Null);
        m.remove("clip_path");
        let list = comments.as_array().map(Vec::as_slice).unwrap_or(&[]);
        m.insert("comment_count".into(), json!(list.len()));
        m.insert(
            "last_comment".into(),
            list.last().cloned().unwrap_or(Value::Null),
        );
    }
    t
}

/// A thread as a gallery card needs it: no bodies, no names.
fn thread_summary(artifact_id: &str, thread: &Value) -> Value {
    let comments = thread["comments"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    json!({
        "artifact_id": artifact_id,
        "thread_id": thread["id"],
        "status": thread["status"],
        "sent_to_agent": thread["sent_to_agent"],
        "comments": comments.len(),
        "last_at": comments.last().map_or(&Value::Null, |c| &c["created_at"]),
    })
}

/// The changes from `old` to `new`: people added or changed, and the public
/// IDs no longer listed.
fn presence_diff(old: &[PresenceView], new: &[PresenceView]) -> (Vec<PresenceView>, Vec<String>) {
    let changed = new
        .iter()
        .filter(|p| !old.iter().any(|o| o == *p))
        .cloned()
        .collect();
    let gone = old
        .iter()
        .filter(|o| !new.iter().any(|p| p.public_id == o.public_id))
        .map(|o| o.public_id.clone())
        .collect();
    (changed, gone)
}

/// The channels `ev` goes to, with each one's gate.
fn routes(ev: &Event) -> Vec<(Chan, Gate)> {
    let a = ev.artifact_id().to_string();
    match ev {
        Event::Version { .. }
        | Event::Thread { .. }
        | Event::ThreadDeleted { .. }
        | Event::ThreadMoved { .. } => {
            vec![(Chan::Gallery, Gate::Any), (Chan::Artifact(a), Gate::Any)]
        }
        Event::ArtifactDeleted { .. } => vec![
            (Chan::Gallery, Gate::Any),
            (Chan::Artifact(a.clone()), Gate::Any),
            (Chan::Presence(a.clone()), Gate::Any),
            (Chan::Working(a.clone()), Gate::Any),
            (Chan::DocsAt(a, Level::View), Gate::Any),
        ],
        Event::FeedbackState { .. } => vec![(Chan::Artifact(a), Gate::Any)],
        Event::Working { .. } => vec![(Chan::Gallery, Gate::Any), (Chan::Working(a), Gate::Any)],
        Event::Presence { .. } => vec![(Chan::Presence(a), Gate::Any)],
        // The thread delta carries the newest comment and the resolve.
        Event::Comment { .. } | Event::ThreadResolved { .. } => vec![],
        Event::Doc {
            private_to,
            read_level,
            self_read,
            ..
        } => match (private_to, self_read) {
            (Some(owner), _) => vec![(
                Chan::DocsOf(a, owner.clone()),
                Gate::Level {
                    min: Level::View,
                    unless: None,
                },
            )],
            (None, Some((owner, level))) => vec![
                (Chan::DocsAt(a.clone(), *read_level), Gate::Any),
                (
                    Chan::DocsOf(a, owner.clone()),
                    Gate::Level {
                        min: *level,
                        unless: Some(*read_level),
                    },
                ),
            ],
            (None, None) => vec![(Chan::DocsAt(a, *read_level), Gate::Any)],
        },
    }
}

/// `ev` as `chan` carries it: the SSE name and data; `None` when there is
/// nothing to send (a presence report that changed nothing).
fn project(
    ev: &Event,
    chan: &Chan,
    topic: &str,
    ch: &mut Channel,
) -> Option<(&'static str, Value)> {
    let data = match (ev, chan) {
        (
            Event::Version {
                artifact_id,
                n,
                by_page,
                title,
                at,
            },
            _,
        ) => {
            let mut v = json!({"topic": topic, "artifact_id": artifact_id, "n": n, "title": title, "at": at});
            if *by_page && matches!(chan, Chan::Artifact(_)) {
                v["by_page"] = json!(true);
            }
            v
        }
        (
            Event::Thread {
                artifact_id,
                thread,
            },
            Chan::Gallery,
        ) => with_topic(topic, thread_summary(artifact_id, thread)),
        (
            Event::Thread {
                artifact_id,
                thread,
            },
            _,
        ) => {
            json!({"topic": topic, "artifact_id": artifact_id, "thread": thread_delta(thread)})
        }
        (
            Event::Working {
                artifact_id,
                working,
            },
            Chan::Gallery,
        ) => {
            let summary: Vec<Value> = working
                .iter()
                .map(|w| json!({"agent": w.agent, "harness": w.harness, "threads": w.thread_ids.len(), "started_at": w.started_at}))
                .collect();
            json!({"topic": topic, "artifact_id": artifact_id, "working": summary})
        }
        (
            Event::Presence {
                artifact_id,
                people,
            },
            _,
        ) => {
            let (changed, gone) = presence_diff(&ch.people, people);
            ch.people = people.clone();
            if changed.is_empty() && gone.is_empty() {
                return None;
            }
            json!({"topic": topic, "artifact_id": artifact_id, "people": changed, "gone": gone})
        }
        _ => with_topic(topic, serde_json::to_value(ev).ok()?),
    };
    Some((ev.name(), data))
}

impl Hub {
    /// A hub that keeps the gallery events of the pages in `live` to
    /// streams that may see live pages.
    pub fn new(live: Arc<crate::live::LiveIds>) -> Arc<Hub> {
        Arc::new(Hub {
            inner: Mutex::default(),
            live,
        })
    }

    /// Makes `bus` hand every event to this hub. A bus feeds one hub.
    pub fn listen(self: &Arc<Self>, bus: &clax_core::EventBus) {
        let hub = self.clone();
        if !bus.set_tap(Box::new(move |e| hub.dispatch(e))) {
            tracing::warn!("the event bus already feeds a stream hub");
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Files `ev` in each subscribed channel it belongs to and offers it to
    /// their subscribers.
    pub fn dispatch(&self, ev: &Event) {
        let mut g = self.lock();
        if g.chans.is_empty() {
            return;
        }
        let site = self.live.origin_of(ev.artifact_id());
        let live = site.is_some();
        let mut all = routes(ev);
        // A live page's `artifact` events also go to its site's channel.
        if let Some(origin) = site
            && all.iter().any(|(c, _)| matches!(c, Chan::Artifact(_)))
        {
            all.push((Chan::Site(origin), Gate::Local));
        }
        let present: Vec<(Chan, Gate)> = all
            .into_iter()
            .filter(|(c, _)| g.chans.contains_key(c))
            .map(|(c, gate)| match c {
                Chan::Gallery if live => (c, Gate::Local),
                _ => (c, gate),
            })
            .collect();
        if present.is_empty() {
            return;
        }
        g.seq += 1;
        let seq = g.seq;
        for (chan, gate) in present {
            let topic = chan.topic();
            let ch = g.chans.get_mut(&chan).expect("checked above");
            let Some((name, data)) = project(ev, &chan, &topic, ch) else {
                continue;
            };
            let item = Arc::new(Item {
                seq,
                topic: topic.into(),
                frame: sse(name, &data),
                gate,
            });
            if ch.ring.len() == RING
                && let Some(old) = ch.ring.pop_front()
            {
                ch.evicted = old.seq;
            }
            ch.ring.push_back(item.clone());
            for sub in ch.subs.values_mut() {
                if sub.lagged || !item.gate.admits(sub.level, sub.local) {
                    continue;
                }
                let Some(tx) = &sub.tx else { continue };
                if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(item.clone()) {
                    sub.lagged = true;
                    sub.shared
                        .lagged
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((item.topic.clone(), seq));
                    sub.shared.notify.notify_one();
                }
            }
        }
    }

    /// Opens a stream for `caller`, or reattaches the one `resume` names
    /// (its ID and the last sequence the client saw) when that stream is
    /// still held and belongs to the same caller with the same `local`
    /// (whether it may see live pages) and `credential` (the hash of the
    /// extension credential of a request through the extension gateway,
    /// which makes the stream live-only; see [`live_only_admits`]).
    pub fn open(
        &self,
        caller: Caller,
        local: bool,
        credential: Option<String>,
        resume: Option<(&str, u64)>,
    ) -> Opened {
        let (tx, rx) = mpsc::channel(QUEUE);
        let mut g = self.lock();
        sweep_locked(&mut g, Instant::now());
        let seq = g.seq;
        if let Some((id, last)) = resume
            && g.streams.get(id).is_some_and(|s| {
                s.caller == caller && s.local == local && s.credential == credential
            })
        {
            let Inner {
                chans,
                streams,
                detached,
                ..
            } = &mut *g;
            let s = streams.get_mut(id).expect("checked above");
            s.epoch += 1;
            s.tx = Some(tx.clone());
            if let Some(at) = s.detached_at.take() {
                detached.remove(&(at, s.key));
            }
            s.shared = Arc::new(Shared::default());
            let mut prelude = Vec::new();
            let mut replay: Vec<Arc<Item>> = Vec::new();
            for (topic, since) in &s.topics {
                let from = last.max(*since);
                let tchans = topic.chans(&s.caller);
                let gap = tchans
                    .iter()
                    .filter_map(|c| chans.get(c))
                    .any(|ch| ch.evicted > from);
                if gap {
                    prelude.push(resync_frame(&topic.name(), "gap"));
                }
                for c in &tchans {
                    let Some(ch) = chans.get_mut(c) else { continue };
                    if let Some(sub) = ch.subs.get_mut(&s.key) {
                        sub.tx = Some(tx.clone());
                        sub.lagged = false;
                        sub.shared = s.shared.clone();
                    }
                    if !gap {
                        replay.extend(
                            ch.ring
                                .iter()
                                .filter(|i| i.seq > from && i.gate.admits(s.caller.level, s.local))
                                .cloned(),
                        );
                    }
                }
            }
            replay.sort_by_key(|i| i.seq);
            for i in replay {
                prelude.push(i.frame.clone());
                prelude.push(id_line(id, i.seq));
            }
            return Opened {
                id: id.to_string(),
                resumed: true,
                seq,
                topics: s.topics.keys().map(Topic::name).collect(),
                epoch: s.epoch,
                rx,
                shared: s.shared.clone(),
                prelude,
            };
        }
        let id = new_id();
        g.next_key += 1;
        let shared = Arc::new(Shared::default());
        let entry = StreamEntry {
            key: g.next_key,
            epoch: 1,
            caller,
            local,
            credential,
            topics: BTreeMap::new(),
            tx: Some(tx),
            shared: shared.clone(),
            detached_at: None,
        };
        g.streams.insert(id.clone(), entry);
        Opened {
            id,
            resumed: false,
            seq,
            topics: vec![],
            epoch: 1,
            rx,
            shared,
            prelude: vec![],
        }
    }

    /// [`Hub::update_via`] for a request that did not come through the
    /// extension gateway.
    ///
    /// # Errors
    /// [`SubError`].
    pub fn update(
        &self,
        id: &str,
        caller: &Caller,
        add: &[Topic],
        remove: &[Topic],
    ) -> Result<(u64, Vec<String>), SubError> {
        self.update_via(id, caller, None, add, remove)
    }

    /// Subscribes stream `id` to `add` and unsubscribes it from `remove`,
    /// for `caller` with `credential` (the extension credential's hash, for
    /// a request through the extension gateway), who must be the caller
    /// that opened it with the same credential, or none. Returns the
    /// current sequence and the stream's topics after the change: every
    /// event of an added topic numbered above that sequence reaches it.
    ///
    /// # Errors
    /// [`SubError`].
    pub fn update_via(
        &self,
        id: &str,
        caller: &Caller,
        credential: Option<&str>,
        add: &[Topic],
        remove: &[Topic],
    ) -> Result<(u64, Vec<String>), SubError> {
        let mut g = self.lock();
        let seq = g.seq;
        let Inner { chans, streams, .. } = &mut *g;
        let s = streams
            .get_mut(id)
            .filter(|s| s.caller == *caller && s.credential.as_deref() == credential)
            .ok_or(SubError::UnknownStream)?;
        let after = s
            .topics
            .keys()
            .filter(|t| !remove.contains(t))
            .chain(add.iter().filter(|t| !s.topics.contains_key(t)))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if after > MAX_TOPICS {
            return Err(SubError::TooMany);
        }
        if !s.local
            && add.iter().any(|t| {
                matches!(t, Topic::Site(_)) || t.artifact().is_some_and(|a| self.live.contains(a))
            })
        {
            return Err(SubError::Hidden);
        }
        if s.credential.is_some() && !add.iter().all(|t| live_only_admits(&self.live, t)) {
            return Err(SubError::Forbidden);
        }
        for t in remove {
            if s.topics.remove(t).is_some() {
                leave(chans, s.key, &t.chans(&s.caller));
            }
        }
        for t in add {
            if s.topics.contains_key(t) {
                continue;
            }
            s.topics.insert(t.clone(), seq);
            for c in t.chans(&s.caller) {
                chans.entry(c).or_default().subs.insert(
                    s.key,
                    Sub {
                        tx: s.tx.clone(),
                        level: s.caller.level,
                        local: s.local,
                        lagged: false,
                        shared: s.shared.clone(),
                    },
                );
            }
        }
        Ok((seq, s.topics.keys().map(Topic::name).collect()))
    }

    /// Lets events of `topic` reach stream `id` again after its `resync`.
    fn clear_lag(&self, id: &str, epoch: u64, topic: &str) {
        let mut g = self.lock();
        let Inner { chans, streams, .. } = &mut *g;
        let Some(s) = streams.get(id).filter(|s| s.epoch == epoch) else {
            return;
        };
        for t in s.topics.keys().filter(|t| t.name() == topic) {
            for c in t.chans(&s.caller) {
                if let Some(sub) = chans.get_mut(&c).and_then(|ch| ch.subs.get_mut(&s.key)) {
                    sub.lagged = false;
                }
            }
        }
    }

    /// The connection of generation `epoch` closed: stream `id` keeps its
    /// subscriptions, receiving nothing, for [`GRACE`].
    fn detach(&self, id: &str, epoch: u64) {
        let mut g = self.lock();
        let Inner {
            chans,
            streams,
            detached,
            ..
        } = &mut *g;
        let Some(s) = streams.get_mut(id).filter(|s| s.epoch == epoch) else {
            return;
        };
        if s.detached_at.is_some() {
            return;
        }
        s.tx = None;
        let now = Instant::now();
        s.detached_at = Some(now);
        detached.insert((now, s.key), id.to_string());
        for t in s.topics.keys() {
            for c in t.chans(&s.caller) {
                if let Some(sub) = chans.get_mut(&c).and_then(|ch| ch.subs.get_mut(&s.key)) {
                    sub.tx = None;
                }
            }
        }
        while g.detached.len() > MAX_DETACHED {
            let Some((_, oldest)) = g.detached.pop_first() else {
                break;
            };
            drop_stream(&mut g, &oldest);
        }
    }

    /// Ends every stream opened with an extension credential whose hash
    /// `ended` holds (a revoked or expired one): it receives nothing more,
    /// its connection's body ends, and it cannot be resumed.
    pub fn end_streams_where(&self, ended: impl Fn(&str) -> bool) {
        let mut g = self.lock();
        let gone: Vec<String> = g
            .streams
            .iter()
            .filter(|(_, s)| s.credential.as_deref().is_some_and(&ended))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            drop_stream(&mut g, &id);
        }
    }

    /// Ends stream `id` while the connection of generation `epoch` holds it.
    fn end(&self, id: &str, epoch: u64) {
        let mut g = self.lock();
        if g.streams.get(id).is_some_and(|s| s.epoch == epoch) {
            drop_stream(&mut g, id);
        }
    }

    /// Drops streams detached for longer than [`GRACE`].
    pub fn sweep(&self) {
        sweep_locked(&mut self.lock(), Instant::now());
    }

    /// The caller level of each stream with a connection open, for tests.
    pub fn attached_levels(&self) -> Vec<Level> {
        let g = self.lock();
        g.streams
            .values()
            .filter(|s| s.tx.is_some())
            .map(|s| s.caller.level)
            .collect()
    }

    pub fn stats(&self) -> Stats {
        let g = self.lock();
        Stats {
            streams: g.streams.len(),
            attached: g.streams.values().filter(|s| s.tx.is_some()).count(),
            channels: g.chans.len(),
        }
    }
}

fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Removes subscriber `key` from `cs`, and each channel left with no subscriber.
fn leave(chans: &mut HashMap<Chan, Channel>, key: u64, cs: &[Chan]) {
    for c in cs {
        if let Some(ch) = chans.get_mut(c) {
            ch.subs.remove(&key);
            if ch.subs.is_empty() {
                chans.remove(c);
            }
        }
    }
}

fn drop_stream(g: &mut Inner, id: &str) {
    if let Some(s) = g.streams.remove(id) {
        if let Some(at) = s.detached_at {
            g.detached.remove(&(at, s.key));
        }
        for t in s.topics.keys() {
            leave(&mut g.chans, s.key, &t.chans(&s.caller));
        }
    }
}

fn sweep_locked(g: &mut Inner, now: Instant) {
    while let Some(first) = g.detached.first_entry() {
        if now.saturating_duration_since(first.key().0) < GRACE {
            break;
        }
        let id = first.remove();
        drop_stream(g, &id);
    }
}

/// Parses `Last-Event-ID`: `<stream>:<seq>`.
pub fn parse_last_event_id(v: &str) -> Option<(&str, u64)> {
    let (id, seq) = v.trim().rsplit_once(':')?;
    let ok = id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then_some(())?;
    Some((id, seq.parse().ok()?))
}

/// One open connection: produces the SSE body. Dropping it detaches the stream.
pub struct Conn {
    hub: Arc<Hub>,
    id: String,
    epoch: u64,
    rx: mpsc::Receiver<Arc<Item>>,
    shared: Arc<Shared>,
    out: VecDeque<Bytes>,
    /// Per topic that fell behind, the first sequence dropped: queued events
    /// of the topic up to it are skipped.
    dropped: HashMap<Arc<str>, u64>,
    keep_alive: tokio::time::Interval,
    shutdown: tokio::sync::watch::Receiver<bool>,
    shutdown_live: bool,
    /// Whether the stream may still deliver (see [`Conn::checking`]).
    check: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl Conn {
    /// The connection for `o`, starting with its `ready` event.
    pub fn new(
        hub: Arc<Hub>,
        o: Opened,
        keep_alive: Duration,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Conn {
        let ready = json!({"stream": o.id, "seq": o.seq, "resumed": o.resumed, "topics": o.topics});
        let mut out = VecDeque::with_capacity(o.prelude.len() + 1);
        out.push_back(Bytes::from(format!("event: ready\ndata: {ready}\n\n")));
        out.extend(o.prelude);
        let mut keep_alive =
            tokio::time::interval_at(tokio::time::Instant::now() + keep_alive, keep_alive);
        keep_alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Conn {
            hub,
            id: o.id,
            epoch: o.epoch,
            rx: o.rx,
            shared: o.shared,
            out,
            dropped: HashMap::new(),
            keep_alive,
            shutdown,
            shutdown_live: true,
            check: None,
        }
    }

    /// This connection, asking `still_live` before it hands out each chunk:
    /// once it answers `false` (the extension credential the stream was
    /// opened with was revoked or expired), the stream ends and its body
    /// with it, before anything more is delivered.
    pub fn checking(mut self, still_live: impl Fn() -> bool + Send + Sync + 'static) -> Conn {
        self.check = Some(Box::new(still_live));
        self
    }

    /// `chunk`, unless the check refuses: then the stream ends.
    fn deliver(&mut self, chunk: Bytes) -> Option<Bytes> {
        if self.check.as_ref().is_some_and(|f| !f()) {
            self.out.clear();
            self.hub.end(&self.id, self.epoch);
            return None;
        }
        Some(chunk)
    }

    /// The next chunk of the body; `None` ends it (daemon shutdown, or a
    /// newer connection took the stream over).
    pub async fn next(&mut self) -> Option<Bytes> {
        loop {
            if let Some(b) = self.out.pop_front() {
                return self.deliver(b);
            }
            enum Wake {
                Stop,
                NoShutdown,
                Lagged,
                Item(Option<Arc<Item>>),
                KeepAlive,
            }
            let shutdown_live = self.shutdown_live;
            let wake = tokio::select! {
                biased;
                r = self.shutdown.wait_for(|v| *v), if shutdown_live => {
                    if r.is_ok() { Wake::Stop } else { Wake::NoShutdown }
                }
                () = self.shared.notify.notified() => Wake::Lagged,
                item = self.rx.recv() => Wake::Item(item),
                _ = self.keep_alive.tick() => Wake::KeepAlive,
            };
            match wake {
                Wake::Stop | Wake::Item(None) => return None,
                // No shutdown source: never end on it.
                Wake::NoShutdown => self.shutdown_live = false,
                Wake::Lagged => self.take_lagged(),
                Wake::KeepAlive => return self.deliver(Bytes::from_static(b": keep-alive\n\n")),
                Wake::Item(Some(i)) => {
                    if let Some(d) = self.dropped.get(&i.topic) {
                        if i.seq <= *d {
                            continue;
                        }
                        // Sequences only grow: no later item of the topic is behind the mark.
                        self.dropped.remove(&i.topic);
                    }
                    self.out.push_back(i.frame.clone());
                    self.out.push_back(id_line(&self.id, i.seq));
                }
            }
        }
    }

    fn take_lagged(&mut self) {
        let lagged =
            std::mem::take(&mut *self.shared.lagged.lock().unwrap_or_else(|e| e.into_inner()));
        for (topic, seq) in lagged {
            // Cleared before the client sees `resync`, so whatever it
            // refetches is at least as new as every event dropped.
            self.hub.clear_lag(&self.id, self.epoch, &topic);
            self.out.push_back(resync_frame(&topic, "behind"));
            self.dropped.insert(topic, seq);
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.hub.detach(&self.id, self.epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(level: Level, v: Option<&str>) -> Caller {
        Caller {
            level,
            viewer: v.map(str::to_string),
        }
    }

    const A: &str = "7q3k9mzx2b4t";

    fn version(n: u32) -> Event {
        Event::Version {
            artifact_id: A.into(),
            n,
            by_page: false,
            title: Some("T".into()),
            at: None,
        }
    }

    fn drain(o: &mut Opened) -> Vec<(u64, String)> {
        let mut out = vec![];
        while let Ok(i) = o.rx.try_recv() {
            out.push((i.seq, String::from_utf8(i.frame.to_vec()).unwrap()));
        }
        out
    }

    #[test]
    fn topics_parse_and_name_round_trip() {
        for t in [
            "gallery",
            "artifact:7q3k9mzx2b4t",
            "presence:7q3k9mzx2b4t",
            "working:7q3k9mzx2b4t",
            "docs:7q3k9mzx2b4t",
        ] {
            assert_eq!(Topic::parse(t).unwrap().name(), t);
        }
        for bad in [
            "",
            "gallery:x",
            "artifact:",
            "artifact:NOPE",
            "threads:7q3k9mzx2b4t",
        ] {
            assert!(Topic::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_live_pages_gallery_events_reach_only_local_streams() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert(A, "http://localhost:5173");
        let hub = Hub::new(live);
        let c = viewer(Level::View, None);
        let mut near = hub.open(c.clone(), true, None, None);
        let mut far = hub.open(c.clone(), false, None, None);
        for o in [&near, &far] {
            hub.update(&o.id, &c, &[Topic::Gallery], &[]).unwrap();
        }
        hub.dispatch(&version(2));
        assert_eq!(drain(&mut near).len(), 1);
        assert_eq!(drain(&mut far).len(), 0);
        // Another artifact's gallery events reach both.
        hub.dispatch(&Event::Version {
            artifact_id: "2b4t7q3k9mzx".into(),
            n: 1,
            by_page: false,
            title: None,
            at: None,
        });
        assert_eq!(drain(&mut near).len(), 1);
        assert_eq!(drain(&mut far).len(), 1);
        // A resume replays the gallery to the local stream only, and never
        // turns a stream local.
        let (fid, nid) = (far.id.clone(), near.id.clone());
        drop((near, far));
        assert!(!hub.open(c.clone(), true, None, Some((&fid, 0))).resumed);
        let mut back = hub.open(c.clone(), true, None, Some((&nid, 0)));
        assert!(back.resumed);
        assert_eq!(back.prelude.len(), 4, "two events, each with its id line");
        assert!(drain(&mut back).is_empty());
    }

    #[test]
    fn a_live_only_stream_takes_live_pages_topics_alone() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert(A, "http://localhost:5173");
        let hub = Hub::new(live);
        let c = viewer(Level::Interact, Some("u_x"));
        let o = hub.open(c.clone(), true, Some("h".into()), None);
        for t in [
            Topic::Artifact(A.into()),
            Topic::Working(A.into()),
            Topic::Presence(A.into()),
        ] {
            assert!(hub.update_via(&o.id, &c, Some("h"), &[t], &[]).is_ok());
        }
        for t in [
            Topic::Gallery,
            Topic::Docs(A.into()),
            Topic::Artifact("aaaaaaaaaaaa".into()),
            Topic::Presence("aaaaaaaaaaaa".into()),
        ] {
            assert_eq!(
                hub.update_via(&o.id, &c, Some("h"), std::slice::from_ref(&t), &[]),
                Err(SubError::Forbidden),
                "{}",
                t.name()
            );
        }
        // A refused change changes nothing, and the stream resumes only with
        // its own credential.
        assert_eq!(
            hub.update_via(&o.id, &c, Some("h"), &[], &[])
                .unwrap()
                .1
                .len(),
            3
        );
        let id = o.id.clone();
        drop(o);
        assert!(!hub.open(c.clone(), true, None, Some((&id, 0))).resumed);
        assert!(
            !hub.open(c.clone(), true, Some("other".into()), Some((&id, 0)))
                .resumed
        );
        let full = hub.open(c.clone(), true, None, None);
        let fid = full.id.clone();
        drop(full);
        assert!(
            !hub.open(c.clone(), true, Some("h".into()), Some((&fid, 0)))
                .resumed
        );
    }

    #[test]
    fn only_a_request_with_the_openers_credential_or_none_changes_a_stream() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert(A, "http://localhost:5173");
        let hub = Hub::new(live);
        let c = viewer(Level::Interact, Some("u_x"));
        let t = [Topic::Artifact(A.into())];
        let ext = hub.open(c.clone(), true, Some("h".into()), None);
        let shell = hub.open(c.clone(), true, None, None);
        assert_eq!(
            hub.update(&ext.id, &c, &t, &[]),
            Err(SubError::UnknownStream)
        );
        assert_eq!(
            hub.update_via(&ext.id, &c, Some("other"), &t, &[]),
            Err(SubError::UnknownStream)
        );
        assert_eq!(
            hub.update_via(&shell.id, &c, Some("h"), &t, &[]),
            Err(SubError::UnknownStream)
        );
        assert!(hub.update_via(&ext.id, &c, Some("h"), &t, &[]).is_ok());
        assert!(hub.update(&shell.id, &c, &t, &[]).is_ok());
    }

    #[test]
    fn ending_a_credentials_streams_closes_them_and_forgets_them() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert(A, "http://localhost:5173");
        let hub = Hub::new(live);
        let c = viewer(Level::Interact, Some("u_x"));
        let t = [Topic::Artifact(A.into())];
        let mut gone = hub.open(c.clone(), true, Some("revoked".into()), None);
        let mut kept = hub.open(c.clone(), true, Some("live".into()), None);
        let mut shell = hub.open(c.clone(), true, None, None);
        hub.update_via(&gone.id, &c, Some("revoked"), &t, &[])
            .unwrap();
        hub.update_via(&kept.id, &c, Some("live"), &t, &[]).unwrap();
        hub.update(&shell.id, &c, &t, &[]).unwrap();
        hub.end_streams_where(|h| h == "revoked");
        hub.dispatch(&version(2));
        assert!(gone.rx.try_recv().is_err());
        assert!(gone.rx.is_closed(), "its connection's body ends");
        assert_eq!(drain(&mut kept).len(), 1);
        assert_eq!(drain(&mut shell).len(), 1);
        let id = gone.id.clone();
        assert!(
            !hub.open(c.clone(), true, Some("revoked".into()), Some((&id, 0)))
                .resumed
        );
    }

    #[test]
    fn a_stream_that_may_not_see_live_pages_cannot_subscribe_to_one() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert(A, "http://localhost:5173");
        let hub = Hub::new(live);
        let c = viewer(Level::View, None);
        let far = hub.open(c.clone(), false, None, None);
        for t in [
            Topic::Artifact(A.into()),
            Topic::Working(A.into()),
            Topic::Presence(A.into()),
            Topic::Docs(A.into()),
        ] {
            assert_eq!(
                hub.update(&far.id, &c, std::slice::from_ref(&t), &[]),
                Err(SubError::Hidden),
                "{}",
                t.name()
            );
        }
        assert!(hub.update(&far.id, &c, &[Topic::Gallery], &[]).is_ok());
        let near = hub.open(c.clone(), true, None, None);
        assert!(
            hub.update(&near.id, &c, &[Topic::Artifact(A.into())], &[])
                .is_ok()
        );
    }

    #[test]
    fn unsubscribed_topics_cost_no_sequence_and_no_channel() {
        let hub = Hub::new(Default::default());
        let _o = hub.open(viewer(Level::View, None), true, None, None);
        hub.dispatch(&version(2));
        assert_eq!(hub.lock().seq, 0);
        assert_eq!(hub.stats().channels, 0);
    }

    #[test]
    fn events_reach_the_topics_subscribed_with_small_deltas() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let mut o = hub.open(c.clone(), true, None, None);
        hub.update(&o.id, &c, &[Topic::Gallery], &[]).unwrap();
        hub.dispatch(&version(2));
        hub.dispatch(&Event::Thread {
            artifact_id: A.into(),
            thread: json!({"id": "t1", "status": "open", "sent_to_agent": false,
                "comments": [{"id": "c1", "body": "secret words", "created_at": "x"}]}),
        });
        let got = drain(&mut o);
        assert_eq!(got.len(), 2);
        assert!(got[0].1.starts_with("event: version\ndata: {"));
        assert!(got[0].1.contains("\"topic\":\"gallery\""));
        assert!(got[1].1.contains("\"comments\":1"), "{}", got[1].1);
        assert!(
            !got[1].1.contains("secret words"),
            "gallery threads carry no bodies"
        );
    }

    #[test]
    fn a_full_queue_marks_the_topic_behind_and_stops_offering_it() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let mut o = hub.open(c.clone(), true, None, None);
        hub.update(&o.id, &c, &[Topic::Artifact(A.into())], &[])
            .unwrap();
        for n in 0..(QUEUE as u32 + 10) {
            hub.dispatch(&version(n));
        }
        let lagged = o.shared.lagged.lock().unwrap().clone();
        assert_eq!(lagged.len(), 1, "one resync per topic, not per event");
        assert_eq!(&*lagged[0].0, "artifact:7q3k9mzx2b4t");
        assert_eq!(lagged[0].1, QUEUE as u64 + 1);
        assert_eq!(
            drain(&mut o).len(),
            QUEUE,
            "the queue never grows past its bound"
        );
        hub.clear_lag(&o.id, o.epoch, "artifact:7q3k9mzx2b4t");
        hub.dispatch(&version(999));
        assert_eq!(drain(&mut o).len(), 1);
    }

    #[test]
    fn private_documents_reach_only_their_viewer_once() {
        let hub = Hub::new(Default::default());
        let me = viewer(Level::Interact, Some("u_me"));
        let other = viewer(Level::Admin, Some("u_other"));
        let mut a = hub.open(me.clone(), true, None, None);
        let mut b = hub.open(other.clone(), true, None, None);
        hub.update(&a.id, &me, &[Topic::Docs(A.into())], &[])
            .unwrap();
        hub.update(&b.id, &other, &[Topic::Docs(A.into())], &[])
            .unwrap();
        let doc =
            |private_to: Option<&str>, read_level, self_read: Option<(&str, Level)>| Event::Doc {
                artifact_id: A.into(),
                path: "p".into(),
                version: Some(1),
                private_to: private_to.map(str::to_string),
                read_level,
                self_read: self_read.map(|(o, l)| (o.to_string(), l)),
            };
        hub.dispatch(&doc(Some("u_me"), Level::Owner, None));
        assert_eq!(drain(&mut a).len(), 1);
        assert_eq!(
            drain(&mut b).len(),
            0,
            "never another viewer, even at admin"
        );
        hub.dispatch(&doc(None, Level::Admin, None));
        assert_eq!(drain(&mut a).len(), 0);
        assert_eq!(drain(&mut b).len(), 1);
        // In my `{self}` subtree, readable at view; others need admin.
        hub.dispatch(&doc(None, Level::Admin, Some(("u_me", Level::View))));
        assert_eq!(drain(&mut a).len(), 1);
        assert_eq!(drain(&mut b).len(), 1);
        hub.dispatch(&doc(None, Level::Interact, Some(("u_me", Level::View))));
        assert_eq!(
            drain(&mut a).len(),
            1,
            "once, though both channels carry it"
        );
    }

    #[test]
    fn a_reconnect_replays_from_the_ring_or_resyncs_on_a_gap() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let o = hub.open(c.clone(), true, None, None);
        let id = o.id.clone();
        hub.update(&id, &c, &[Topic::Artifact(A.into()), Topic::Gallery], &[])
            .unwrap();
        hub.dispatch(&version(2));
        let epoch = o.epoch;
        drop(o);
        hub.detach(&id, epoch);
        hub.dispatch(&version(3));
        let o = hub.open(c.clone(), true, None, Some((&id, 1)));
        assert!(o.resumed);
        let text: Vec<String> = o
            .prelude
            .iter()
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect();
        assert_eq!(
            text.len(),
            4,
            "version 3 on both topics, each with its id: {text:?}"
        );
        assert!(text[1].starts_with(&format!("id: {id}:2")));
        // Another caller cannot take the stream.
        let o2 = hub.open(viewer(Level::Admin, None), true, None, Some((&id, 2)));
        assert!(!o2.resumed);
        // A gap the ring no longer covers is a resync.
        let epoch = o.epoch;
        drop(o);
        hub.detach(&id, epoch);
        for n in 0..(RING as u32 + 5) {
            hub.dispatch(&version(10 + n));
        }
        let o = hub.open(c, true, None, Some((&id, 2)));
        let first = String::from_utf8(o.prelude[0].to_vec()).unwrap();
        assert!(first.starts_with("event: resync"), "{first}");
        assert!(first.contains("\"reason\":\"gap\""));
    }

    #[test]
    fn unsubscribing_the_last_subscriber_drops_the_channel() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let o = hub.open(c.clone(), true, None, None);
        let t = [Topic::Presence(A.into())];
        hub.update(&o.id, &c, &t, &[]).unwrap();
        assert_eq!(hub.stats().channels, 1);
        hub.update(&o.id, &c, &[], &t).unwrap();
        assert_eq!(hub.stats().channels, 0);
        assert_eq!(
            hub.update(&o.id, &viewer(Level::Owner, None), &t, &[]),
            Err(SubError::UnknownStream)
        );
        let many: Vec<Topic> = (0..=MAX_TOPICS)
            .map(|i| Topic::Artifact(format!("7q3k9mzx2b{:02}", i)))
            .collect();
        assert_eq!(hub.update(&o.id, &c, &many, &[]), Err(SubError::TooMany));
    }

    #[tokio::test]
    async fn a_checked_connection_ends_before_delivering_once_the_check_fails() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert(A, "http://localhost:5173");
        let hub = Hub::new(live);
        let c = viewer(Level::View, None);
        let o = hub.open(c.clone(), true, Some("h".into()), None);
        let id = o.id.clone();
        hub.update_via(&id, &c, Some("h"), &[Topic::Artifact(A.into())], &[])
            .unwrap();
        let live = Arc::new(AtomicBool::new(true));
        let l = live.clone();
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let mut conn = Conn::new(hub.clone(), o, Duration::from_secs(3600), rx)
            .checking(move || l.load(Ordering::SeqCst));
        assert!(conn.next().await.unwrap().starts_with(b"event: ready"));
        hub.dispatch(&version(2));
        assert!(conn.next().await.unwrap().starts_with(b"event: version"));
        assert!(conn.next().await.unwrap().starts_with(b"id: "));
        hub.dispatch(&version(3));
        live.store(false, Ordering::SeqCst);
        assert!(conn.next().await.is_none(), "nothing past the failed check");
        assert_eq!(hub.stats().streams, 0, "the stream is gone, not resumable");
    }

    #[tokio::test]
    async fn a_topic_past_its_resync_leaves_no_mark_on_the_connection() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let o = hub.open(c.clone(), true, None, None);
        hub.update(&o.id, &c, &[Topic::Artifact(A.into())], &[])
            .unwrap();
        for n in 0..(QUEUE as u32 + 5) {
            hub.dispatch(&version(n));
        }
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let mut conn = Conn::new(hub.clone(), o, Duration::from_secs(3600), rx);
        async fn next(conn: &mut Conn) -> Option<Bytes> {
            tokio::time::timeout(Duration::from_millis(200), conn.next())
                .await
                .ok()
                .flatten()
        }
        let mut saw_resync = false;
        while let Some(b) = next(&mut conn).await {
            saw_resync |= b.starts_with(b"event: resync");
        }
        assert!(saw_resync);
        assert_eq!(conn.dropped.len(), 1);
        hub.dispatch(&version(999));
        let b = next(&mut conn).await.unwrap();
        assert!(b.starts_with(b"event: version"));
        assert!(
            conn.dropped.is_empty(),
            "the mark goes with the first event past it"
        );
    }

    #[test]
    fn a_disconnect_storm_costs_each_stream_little_and_keeps_the_newest_detached() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let n = MAX_DETACHED * 4;
        let opening = Instant::now();
        let opened: Vec<Opened> = (0..n)
            .map(|_| {
                let o = hub.open(c.clone(), true, None, None);
                hub.update(&o.id, &c, &[Topic::Gallery], &[]).unwrap();
                o
            })
            .collect();
        let open_took = opening.elapsed();
        // Every connection drops at once (a network blip). Each detach must
        // not scan every stream: that is quadratic under the hub's lock.
        let start = Instant::now();
        for o in &opened {
            hub.detach(&o.id, o.epoch);
        }
        let took = start.elapsed();
        // Detaching costs about half as much as opening (both linear); a
        // quadratic detach takes seconds. On a busy machine both slow down
        // alike, so the limit grows with the opening's time past 1.5 s.
        let limit = Duration::from_millis(1500).max(open_took * 4);
        assert!(
            took < limit,
            "{n} detaches took {took:?} (opening them took {open_took:?})"
        );
        assert_eq!(hub.stats().streams, MAX_DETACHED);
        // The longest detached went first; the newest can still resume.
        let last = opened.last().unwrap();
        assert!(hub.open(c.clone(), true, None, Some((&last.id, 0))).resumed);
        assert!(!hub.open(c, true, None, Some((&opened[0].id, 0))).resumed);
    }

    #[test]
    fn a_stream_detached_past_the_grace_cannot_resume() {
        let hub = Hub::new(Default::default());
        let c = viewer(Level::View, None);
        let o = hub.open(c.clone(), true, None, None);
        hub.update(&o.id, &c, &[Topic::Gallery], &[]).unwrap();
        hub.detach(&o.id, o.epoch);
        let now = Instant::now();
        sweep_locked(&mut hub.lock(), now + GRACE - Duration::from_secs(1));
        assert_eq!(hub.stats().streams, 1);
        sweep_locked(&mut hub.lock(), now + GRACE + Duration::from_secs(1));
        assert_eq!(
            hub.stats(),
            Stats {
                streams: 0,
                attached: 0,
                channels: 0
            }
        );
        assert!(!hub.open(c, true, None, Some((&o.id, 0))).resumed);
    }

    #[test]
    fn last_event_ids_parse_strictly() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(parse_last_event_id(&format!("{id}:42")), Some((id, 42)));
        assert_eq!(parse_last_event_id("x:1"), None);
        assert_eq!(parse_last_event_id(&format!("{id}:x")), None);
        assert_eq!(parse_last_event_id("42"), None);
    }
}

//! The live layers (spec 2026-10-11-clax-dev-hosted-design §16.3, §16.5):
//! what people on an artifact see of each other as it happens — pointers
//! and selections in the page, "is typing…" and comment drafts in a thread —
//! relayed between people's live sockets and never stored.
//!
//! This module is the relay's logic alone: the wire messages and their
//! checks ([`parse_message`]), the live colours ([`pick_color`]), the inbound
//! rate ([`TokenBucket`]) and the batching ([`Relay`]). Time is passed in (ms
//! on any monotonic scale), so the daemon and tests drive it with their own
//! clocks; nothing here starts a thread, a timer or a socket.
//!
//! The relay keeps each sender's latest state, keyed by person (one person
//! with several tabs is one sender: the socket that wrote last owns the
//! state). Updates only mark the relay dirty; the caller calls
//! [`Relay::batches`] at most once per [`FLUSH_MS`], which answers, for each
//! receiver, the senders in its full-detail set whose state changed since it
//! was last sent, so a sender's updates reach a receiver at most about
//! 1000 / [`FLUSH_MS`] times a second however often they arrive. The cursor
//! (pointer and selection) and the writing (typing and draft) change apart,
//! so each goes only when it changed for that receiver. A receiver gets full
//! detail for at most [`LIVE_CAP`] senders, ranked: the people in the thread
//! it has open (they have it open, or type or draft in it), then the most
//! recently active; the rest are a count.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Most senders a receiver gets in full detail (§16.5).
pub const LIVE_CAP: usize = 30;
/// Least time between two flushes of the relay, in ms: about ten a second per sender.
pub const FLUSH_MS: u64 = 100;
/// Longest draft text relayed, in characters; a longer one is cut.
pub const MAX_DRAFT_CHARS: usize = 2000;
/// Longest live socket message read, in bytes.
pub const MAX_LIVE_MESSAGE_BYTES: usize = 16 * 1024;
/// Colours in the live palette (the shell's `--live-0` … `--live-7`).
pub const LIVE_COLORS: u8 = 8;
/// Deepest element path an anchor may name.
pub const MAX_ANCHOR_DEPTH: usize = 64;
/// Largest child index or text offset an anchor may name.
const MAX_INDEX: i64 = 10_000_000;
/// Longest file name a cursor may name, in bytes.
const MAX_FILE_BYTES: usize = 512;

/// Live messages a socket may send per second, sustained.
pub const LIVE_RATE: f64 = 30.0;
/// Live messages a socket may send at once.
pub const LIVE_BURST: f64 = 60.0;
/// Seconds in a row with dropped messages after which the socket is closed.
pub const LIVE_OVERRUN_SECONDS: u32 = 5;

// ── Messages ────────────────────────────────────────────────────────────────

/// A point in the page, anchored so it lands right at any width: the
/// element at `path` (child indexes from `<body>`), the character `offset`
/// in its text (-1 when the point is on no text), and `x`, `y` as
/// fractions of the element's box (for a point on no text).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LivePoint {
    pub path: Vec<u32>,
    pub offset: i64,
    pub x: f64,
    pub y: f64,
}

/// What a person has selected or is commenting on in the page: a text range
/// (from `start` to `end`, both on text), an element, or an area drawn on an
/// element (`x`, `y`, `w`, `h` as fractions of the element's box), as comment
/// mode's picker makes them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveSelection {
    Range {
        start: LivePoint,
        end: LivePoint,
    },
    Element {
        path: Vec<u32>,
    },
    Area {
        path: Vec<u32>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
}

/// A person's pointer and selection in `file` of `version`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveCursor {
    pub version: u32,
    pub file: String,
    pub anchor: Option<LivePoint>,
    pub selection: Option<LiveSelection>,
}

/// A thread a person writes in: a thread ID, or `None` for a new comment.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InThread {
    pub thread_id: Option<String>,
}

/// A draft: its thread and the text so far.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveDraft {
    pub thread_id: Option<String>,
    pub text: String,
}

/// A message a page sends on its live socket.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientMessage {
    /// The thread the page has open (ranks senders for it; not shown).
    Open(Option<String>),
    /// A keystroke in a thread's box (`None`: the new-comment box).
    Typing(Option<String>),
    /// The draft in a thread's box; empty when cleared, posted or cancelled.
    Draft(LiveDraft),
    /// The pointer and selection.
    Cursor(LiveCursor),
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Open {
        thread_id: Option<String>,
    },
    Typing {
        thread_id: Option<String>,
    },
    Draft {
        thread_id: Option<String>,
        text: String,
    },
    Cursor {
        version: u32,
        file: String,
        anchor: Option<LivePoint>,
        selection: Option<LiveSelection>,
    },
}

fn fraction(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

fn path_ok(p: &[u32]) -> bool {
    p.len() <= MAX_ANCHOR_DEPTH && p.iter().all(|&i| i64::from(i) <= MAX_INDEX)
}

fn point_ok(p: &LivePoint) -> bool {
    path_ok(&p.path)
        && (p.offset == -1 || (0..=MAX_INDEX).contains(&p.offset))
        && fraction(p.x)
        && fraction(p.y)
}

fn selection_ok(s: &LiveSelection) -> bool {
    match s {
        LiveSelection::Range { start, end } => {
            point_ok(start) && point_ok(end) && start.offset >= 0 && end.offset >= 0
        }
        LiveSelection::Element { path } => path_ok(path),
        LiveSelection::Area { path, x, y, w, h } => {
            path_ok(path) && [x, y, w, h].iter().all(|v| fraction(**v))
        }
    }
}

/// A thread ID as a live message may name it: a ULID, or `None`.
fn thread_ok(t: &Option<String>) -> bool {
    t.as_deref().is_none_or(crate::is_ulid)
}

/// The live message `raw`, or `None` when it is not a well-formed one: too
/// long, not JSON, an unknown `t`, a malformed thread ID, anchor or file.
/// A draft longer than [`MAX_DRAFT_CHARS`] is cut to it. Whether the thread
/// is one of the artifact's is not checked: a receiver shows a draft only on
/// a thread it has.
pub fn parse_message(raw: &str) -> Option<ClientMessage> {
    if raw.len() > MAX_LIVE_MESSAGE_BYTES {
        return None;
    }
    let w: Wire = serde_json::from_str(raw).ok()?;
    Some(match w {
        Wire::Open { thread_id } => {
            thread_ok(&thread_id).then_some(())?;
            ClientMessage::Open(thread_id)
        }
        Wire::Typing { thread_id } => {
            thread_ok(&thread_id).then_some(())?;
            ClientMessage::Typing(thread_id)
        }
        Wire::Draft { thread_id, text } => {
            thread_ok(&thread_id).then_some(())?;
            let text = match text.char_indices().nth(MAX_DRAFT_CHARS) {
                Some((i, _)) => text[..i].to_string(),
                None => text,
            };
            ClientMessage::Draft(LiveDraft { thread_id, text })
        }
        Wire::Cursor {
            version,
            file,
            anchor,
            selection,
        } => {
            let ok = version >= 1
                && !file.is_empty()
                && file.len() <= MAX_FILE_BYTES
                && anchor.as_ref().is_none_or(point_ok)
                && selection.as_ref().is_none_or(selection_ok);
            ok.then_some(())?;
            ClientMessage::Cursor(LiveCursor {
                version,
                file,
                anchor,
                selection,
            })
        }
    })
}

// ── Colours ─────────────────────────────────────────────────────────────────

fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in s.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    h
}

/// The live colour for `person` arriving while the colours in `taken` are
/// held: its preferred colour (from its ID) unless someone present has it,
/// else the next free one; once every colour is taken, its preferred one.
pub fn pick_color(person: &str, taken: &[u8]) -> u8 {
    let preferred = u8::try_from(fnv1a(person) % u32::from(LIVE_COLORS)).unwrap_or(0);
    let mut held = [false; LIVE_COLORS as usize];
    for &c in taken {
        if let Some(h) = held.get_mut(usize::from(c)) {
            *h = true;
        }
    }
    if held.iter().all(|&h| h) {
        return preferred;
    }
    let mut c = preferred;
    while held[usize::from(c)] {
        c = (c + 1) % LIVE_COLORS;
    }
    c
}

// ── Inbound rate ────────────────────────────────────────────────────────────

/// What a socket's message gets from its [`TokenBucket`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Take {
    Ok,
    /// Over the rate: dropped.
    Drop,
    /// Over the rate for [`LIVE_OVERRUN_SECONDS`] in a row: close the socket.
    Close,
}

/// A token bucket for one socket's inbound live messages: [`LIVE_RATE`] a
/// second, [`LIVE_BURST`] at once. A message over the rate is dropped; a
/// socket with messages dropped in each of [`LIVE_OVERRUN_SECONDS`] seconds
/// in a row is to be closed.
#[derive(Clone, Debug)]
pub struct TokenBucket {
    tokens: f64,
    last: u64,
    second: u64,
    dropped_this_second: u32,
    over_seconds: u32,
    rate: f64,
    burst: f64,
    overrun: u32,
}

impl TokenBucket {
    /// A full bucket at `now` (ms) with the default rate, burst and overrun.
    pub fn new(now: u64) -> TokenBucket {
        TokenBucket::with(now, LIVE_RATE, LIVE_BURST, LIVE_OVERRUN_SECONDS)
    }

    /// A full bucket at `now` (ms) of `rate` a second, `burst` at once,
    /// closing after `overrun` seconds over the rate in a row.
    pub fn with(now: u64, rate: f64, burst: f64, overrun: u32) -> TokenBucket {
        TokenBucket {
            tokens: burst,
            last: now,
            second: now,
            dropped_this_second: 0,
            over_seconds: 0,
            rate,
            burst,
            overrun,
        }
    }

    /// Takes one message at `now` (ms; earlier times count as `now` of the
    /// last take).
    pub fn take(&mut self, now: u64) -> Take {
        let now = now.max(self.last);
        if now - self.second >= 1000 {
            // A whole quiet second or more ends a run.
            self.over_seconds = if now - self.second < 2000 && self.dropped_this_second > 0 {
                self.over_seconds + 1
            } else {
                0
            };
            self.second = now;
            self.dropped_this_second = 0;
        }
        #[allow(clippy::cast_precision_loss)]
        let refill = (now - self.last) as f64 * self.rate / 1000.0;
        self.tokens = (self.tokens + refill).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Take::Ok;
        }
        self.dropped_this_second += 1;
        if self.over_seconds + 1 >= self.overrun {
            Take::Close
        } else {
            Take::Drop
        }
    }
}

// ── The relay ───────────────────────────────────────────────────────────────

#[derive(Debug)]
struct SenderState {
    typing: Option<InThread>,
    draft: Option<LiveDraft>,
    cursor: Option<LiveCursor>,
    /// The thread the sender has open.
    open: Option<Option<String>>,
    /// Bumped when the cursor changes.
    c_rev: u64,
    /// Bumped when typing or the draft changes.
    t_rev: u64,
    /// When the sender last changed something receivers see (ms).
    active: u64,
    /// The socket that wrote last.
    socket: u64,
}

impl SenderState {
    fn shows(&self) -> bool {
        self.typing.is_some() || self.draft.is_some() || self.cursor.is_some()
    }

    fn in_thread(&self, t: &Option<String>) -> bool {
        self.open.as_ref() == Some(t)
            || self.typing.as_ref().is_some_and(|x| &x.thread_id == t)
            || self.draft.as_ref().is_some_and(|x| &x.thread_id == t)
    }
}

/// A receiving socket: its key, its person, the thread it has open
/// (`None`: none; `Some(None)`: the new-comment box), and whether it may
/// read comments there (`text`): a receiver that may not is sent no
/// sender's typing or draft, only their pointer and selection.
#[derive(Clone, Debug)]
pub struct Receiver {
    pub key: u64,
    pub person: String,
    pub open: Option<Option<String>>,
    pub text: bool,
}

/// What a receiver was last sent: each detailed sender's revisions, the
/// count of the others, and how many other people were present.
#[derive(Default, Debug)]
struct Sent {
    revs: HashMap<String, (u64, u64)>,
    others: usize,
    present: usize,
}

/// One sender in a batch, with the parts that changed for its receiver.
#[derive(Serialize)]
struct SenderOut<'a> {
    person_id: &'a str,
    color: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a Option<LiveCursor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    typing: Option<&'a Option<InThread>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft: Option<&'a Option<LiveDraft>>,
}

/// The live layers of one artifact. See the module docs.
#[derive(Debug)]
pub struct Relay {
    senders: BTreeMap<String, SenderState>,
    sent: HashMap<u64, Sent>,
    dirty: bool,
    cap: usize,
}

impl Default for Relay {
    fn default() -> Relay {
        Relay::new(LIVE_CAP)
    }
}

impl Relay {
    /// A relay giving each receiver full detail for at most `cap` senders.
    pub fn new(cap: usize) -> Relay {
        Relay {
            senders: BTreeMap::new(),
            sent: HashMap::new(),
            dirty: false,
            cap,
        }
    }

    /// Whether something changed since the last [`Relay::batches`].
    pub fn pending(&self) -> bool {
        self.dirty
    }

    /// Applies `m` from `person`'s socket `socket` at `now` (ms).
    pub fn apply(&mut self, person: &str, socket: u64, m: ClientMessage, now: u64) {
        let s = self
            .senders
            .entry(person.to_string())
            .or_insert_with(|| SenderState {
                typing: None,
                draft: None,
                cursor: None,
                open: None,
                c_rev: 0,
                t_rev: 0,
                active: now,
                socket,
            });
        s.socket = socket;
        self.dirty = true;
        match m {
            ClientMessage::Open(t) => {
                // What a sender has open ranks it for others; it is not shown.
                s.open = Some(t);
                return;
            }
            ClientMessage::Typing(t) => {
                s.typing = Some(InThread { thread_id: t });
                s.t_rev += 1;
            }
            ClientMessage::Draft(d) => {
                if d.text.is_empty() {
                    s.draft = None;
                    s.typing = None;
                } else {
                    s.typing = Some(InThread {
                        thread_id: d.thread_id.clone(),
                    });
                    s.draft = Some(d);
                }
                s.t_rev += 1;
            }
            ClientMessage::Cursor(c) => {
                s.cursor = (c.anchor.is_some() || c.selection.is_some()).then_some(c);
                s.c_rev += 1;
            }
        }
        s.active = now;
    }

    /// `person`'s typing and drafts go (they may no longer comment).
    pub fn clear_text(&mut self, person: &str) {
        if let Some(s) = self.senders.get_mut(person)
            && (s.typing.is_some() || s.draft.is_some())
        {
            s.typing = None;
            s.draft = None;
            s.t_rev += 1;
            self.dirty = true;
        }
    }

    /// Socket `socket` of `person` closed: the person's state goes when that
    /// socket wrote it last. Who is present may have changed, so the relay
    /// has news either way.
    pub fn leave(&mut self, person: &str, socket: u64) {
        self.sent.remove(&socket);
        if self.senders.get(person).is_some_and(|s| s.socket == socket) {
            self.senders.remove(person);
        }
        self.dirty = true;
    }

    /// Forgets what receiver `socket` was sent (its batch was lost): its next
    /// batch carries every sender whole.
    pub fn resend(&mut self, socket: u64) {
        self.sent.remove(&socket);
        self.dirty = true;
    }

    /// Forgets what receiver `socket` was sent, as [`Relay::resend`], without
    /// marking the relay as having news: the next [`Relay::batches`] made for
    /// another reason carries every sender whole to it, and none is due
    /// until then or until the caller calls [`Relay::resend`].
    pub fn forget(&mut self, socket: u64) {
        self.sent.remove(&socket);
    }

    /// Each receiver's batch, serialised, for the receivers whose view
    /// changed since they were last sent one, and marks what it answers as
    /// sent. A sender new to a receiver comes whole; after that only its
    /// changed parts. `color(person)` gives each sender's colour.
    ///
    /// A batch is `{"t":"live","senders":[…],"drop":[person IDs],"others":n,"present":n}`:
    /// each sender `{person_id, color, cursor?, typing?, draft?}` with the
    /// parts that changed (a part left out is unchanged; `null` is none),
    /// `drop` the senders no longer detailed, `others` how many more show
    /// something without detail, and `present` how many other people are
    /// among `receivers` (whether or not they show anything), so a page
    /// alone on the artifact need not report its pointer.
    pub fn batches(
        &mut self,
        receivers: &[Receiver],
        color: &dyn Fn(&str) -> u8,
    ) -> Vec<(u64, String)> {
        self.dirty = false;
        let mut out = Vec::new();
        let showing: Vec<(&String, &SenderState)> =
            self.senders.iter().filter(|(_, s)| s.shows()).collect();
        // Each sender's parts, serialised once per kind of change.
        let mut parts: HashMap<(&str, bool, bool), String> = HashMap::new();
        let people: HashSet<&str> = receivers.iter().map(|r| r.person.as_str()).collect();
        for r in receivers {
            let present = people.len() - 1;
            let mut ranked: Vec<(&String, &SenderState, u8)> = showing
                .iter()
                .filter(|(id, _)| **id != r.person)
                .map(|(id, s)| {
                    let tier = match &r.open {
                        Some(t) if s.in_thread(t) => 0,
                        _ => 1,
                    };
                    (*id, *s, tier)
                })
                .collect();
            ranked.sort_by(|a, b| {
                a.2.cmp(&b.2)
                    .then(b.1.active.cmp(&a.1.active))
                    .then(a.0.cmp(b.0))
            });
            let others = ranked.len().saturating_sub(self.cap);
            ranked.truncate(self.cap);
            let prev = self.sent.remove(&r.key).unwrap_or_default();
            let mut revs = HashMap::with_capacity(ranked.len());
            let mut senders: Vec<String> = Vec::new();
            for (id, s, _) in &ranked {
                revs.insert((*id).clone(), (s.c_rev, s.t_rev));
                let was = prev.revs.get(*id);
                let c = was.is_none_or(|w| w.0 != s.c_rev);
                let t = r.text && was.is_none_or(|w| w.1 != s.t_rev);
                if c || t {
                    let p = parts.entry((id.as_str(), c, t)).or_insert_with(|| {
                        serde_json::to_string(&SenderOut {
                            person_id: id,
                            color: color(id),
                            cursor: c.then_some(&s.cursor),
                            typing: t.then_some(&s.typing),
                            draft: t.then_some(&s.draft),
                        })
                        .expect("a sender serialises")
                    });
                    senders.push(p.clone());
                }
            }
            let mut drop: Vec<&String> = prev
                .revs
                .keys()
                .filter(|id| !revs.contains_key(*id))
                .collect();
            drop.sort();
            let changed = !senders.is_empty()
                || !drop.is_empty()
                || others != prev.others
                || present != prev.present;
            if changed {
                out.push((
                    r.key,
                    format!(
                        r#"{{"t":"live","senders":[{}],"drop":{},"others":{others},"present":{present}}}"#,
                        senders.join(","),
                        serde_json::to_string(&drop).expect("IDs serialise"),
                    ),
                ));
            }
            self.sent.insert(
                r.key,
                Sent {
                    revs,
                    others,
                    present,
                },
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const T1: &str = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";

    fn parse(v: Value) -> Option<ClientMessage> {
        parse_message(&v.to_string())
    }

    fn point(path: &[u32], offset: i64) -> Value {
        json!({"path": path, "offset": offset, "x": 0.5, "y": 0.25})
    }

    fn cursor_msg(x: f64) -> ClientMessage {
        ClientMessage::Cursor(LiveCursor {
            version: 1,
            file: "index.html".into(),
            anchor: Some(LivePoint {
                path: vec![0, 1],
                offset: -1,
                x,
                y: 0.5,
            }),
            selection: None,
        })
    }

    fn rx(key: u64, person: &str) -> Receiver {
        Receiver {
            key,
            person: person.into(),
            open: None,
            text: true,
        }
    }

    fn batch(out: &[(u64, String)], key: u64) -> Option<Value> {
        out.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, s)| serde_json::from_str(s).unwrap())
    }

    #[test]
    fn well_formed_messages_parse() {
        assert_eq!(
            parse(json!({"t": "open", "thread_id": null})),
            Some(ClientMessage::Open(None))
        );
        assert_eq!(
            parse(json!({"t": "typing", "thread_id": T1})),
            Some(ClientMessage::Typing(Some(T1.into())))
        );
        let Some(ClientMessage::Cursor(c)) =
            parse(json!({"t": "cursor", "version": 2, "file": "index.html",
            "anchor": point(&[0, 3], 12),
            "selection": {"kind": "range", "start": point(&[0], 0), "end": point(&[1], 4)}}))
        else {
            panic!("a cursor")
        };
        assert_eq!(c.version, 2);
        assert!(matches!(c.selection, Some(LiveSelection::Range { .. })));
        assert!(matches!(
            parse(
                json!({"t": "cursor", "version": 1, "file": "a.html", "anchor": null,
                "selection": {"kind": "area", "path": [2], "x": 0.1, "y": 0.2, "w": 0.3, "h": 0.4}})
            ),
            Some(ClientMessage::Cursor(LiveCursor {
                selection: Some(LiveSelection::Area { .. }),
                ..
            }))
        ));
        assert!(matches!(
            parse(
                json!({"t": "cursor", "version": 1, "file": "a.html", "anchor": null,
                "selection": {"kind": "element", "path": [2, 0]}})
            ),
            Some(ClientMessage::Cursor(LiveCursor {
                selection: Some(LiveSelection::Element { .. }),
                ..
            }))
        ));
    }

    #[test]
    fn malformed_messages_are_refused() {
        for bad in [
            json!({"t": "shout"}),
            json!({"t": "typing", "thread_id": "not-a-ulid"}),
            json!({"t": "typing", "thread_id": T1, "extra": 1}),
            json!({"t": "draft", "thread_id": null}),
            json!({"t": "cursor", "version": 0, "file": "a", "anchor": null, "selection": null}),
            json!({"t": "cursor", "version": 1, "file": "", "anchor": null, "selection": null}),
            json!({"t": "cursor", "version": 1, "file": "a", "anchor": {"path": [0], "offset": -2, "x": 0, "y": 0}, "selection": null}),
            json!({"t": "cursor", "version": 1, "file": "a", "anchor": {"path": [0], "offset": 0, "x": 1.5, "y": 0}, "selection": null}),
            json!({"t": "cursor", "version": 1, "file": "a", "anchor": {"path": vec![0; 65], "offset": 0, "x": 0, "y": 0}, "selection": null}),
            json!({"t": "cursor", "version": 1, "file": "a", "anchor": null, "selection": {"kind": "range", "start": point(&[0], -1), "end": point(&[0], 2)}}),
            json!({"t": "cursor", "version": 1, "file": "a", "anchor": null, "selection": {"kind": "area", "path": [0], "x": 0, "y": 0, "w": 2, "h": 0}}),
            json!({"t": "cursor", "version": 1, "file": "a", "anchor": null, "selection": {"kind": "blob"}}),
        ] {
            assert_eq!(parse(bad.clone()), None, "{bad}");
        }
        assert_eq!(parse_message("{"), None);
        let long = format!(
            r#"{{"t":"draft","thread_id":null,"text":"{}"}}"#,
            "x".repeat(MAX_LIVE_MESSAGE_BYTES)
        );
        assert_eq!(parse_message(&long), None);
    }

    #[test]
    fn a_long_draft_is_cut_to_the_limit_in_characters() {
        let text = "é".repeat(MAX_DRAFT_CHARS + 5);
        let Some(ClientMessage::Draft(d)) =
            parse(json!({"t": "draft", "thread_id": null, "text": text}))
        else {
            panic!("a draft")
        };
        assert_eq!(d.text.chars().count(), MAX_DRAFT_CHARS);
    }

    #[test]
    fn colours_prefer_their_own_and_skip_taken_ones() {
        let c = pick_color("u_a", &[]);
        assert!(c < LIVE_COLORS);
        assert_eq!(pick_color("u_a", &[]), c, "stable");
        let next = pick_color("u_a", &[c]);
        assert_eq!(next, (c + 1) % LIVE_COLORS);
        let all: Vec<u8> = (0..LIVE_COLORS).collect();
        assert_eq!(pick_color("u_a", &all), c, "all taken: its own");
    }

    #[test]
    fn the_bucket_takes_a_burst_then_the_rate() {
        let mut b = TokenBucket::new(0);
        for _ in 0..60 {
            assert_eq!(b.take(0), Take::Ok);
        }
        assert_eq!(b.take(0), Take::Drop);
        // 30 a second: one token per 33.3 ms.
        assert_eq!(b.take(34), Take::Ok);
        assert_eq!(b.take(34), Take::Drop);
        assert_eq!(b.take(1034), Take::Ok);
    }

    #[test]
    fn the_bucket_closes_after_five_seconds_over_the_rate_in_a_row() {
        let mut b = TokenBucket::with(0, 1.0, 1.0, 5);
        let mut t = 0;
        let mut seen = Vec::new();
        // Ten messages a second, every second: one is taken, the rest dropped.
        for _ in 0..6 {
            for i in 0..10 {
                seen.push(b.take(t + i * 100));
            }
            t += 1000;
        }
        let first_close = seen.iter().position(|&x| x == Take::Close).expect("closes");
        assert!(
            first_close >= 40,
            "not before the fifth second: {first_close}"
        );
        // A quiet second ends a run.
        let mut b = TokenBucket::with(0, 1.0, 1.0, 5);
        for s in 0..4 {
            for i in 0..10 {
                assert_ne!(b.take(s * 1000 + i * 100), Take::Close);
            }
        }
        assert_eq!(b.take(6000), Take::Ok);
        for i in 1..10 {
            assert_ne!(b.take(6000 + i * 100), Take::Close);
        }
    }

    #[test]
    fn a_receiver_gets_others_but_never_itself() {
        let mut r = Relay::default();
        r.apply("u_a", 1, cursor_msg(0.5), 0);
        assert!(r.pending());
        let out = r.batches(&[rx(1, "u_a"), rx(2, "u_b")], &|_| 3);
        assert!(!r.pending());
        let own = batch(&out, 1).unwrap();
        assert_eq!(own["senders"], json!([]), "never its own state: {own}");
        assert_eq!(own["present"], 1);
        let b = batch(&out, 2).unwrap();
        assert_eq!(b["senders"][0]["person_id"], "u_a");
        assert_eq!(b["senders"][0]["color"], 3);
        assert_eq!(b["senders"][0]["cursor"]["anchor"]["x"], 0.5);
        assert_eq!(b["others"], 0);
    }

    #[test]
    fn updates_between_flushes_are_coalesced_to_the_latest() {
        let mut r = Relay::default();
        for i in 0..20u32 {
            r.apply("u_a", 1, cursor_msg(f64::from(i) / 20.0), u64::from(i));
        }
        let out = r.batches(&[rx(2, "u_b")], &|_| 0);
        assert_eq!(out.len(), 1);
        let b = batch(&out, 2).unwrap();
        assert_eq!(b["senders"].as_array().unwrap().len(), 1);
        assert_eq!(b["senders"][0]["cursor"]["anchor"]["x"], 0.95);
        // Nothing changed since: no batch.
        assert!(r.batches(&[rx(2, "u_b")], &|_| 0).is_empty());
    }

    #[test]
    fn only_the_parts_that_changed_are_sent_again() {
        let mut r = Relay::default();
        r.apply(
            "u_a",
            1,
            ClientMessage::Draft(LiveDraft {
                thread_id: None,
                text: "hel".into(),
            }),
            0,
        );
        r.apply("u_a", 1, cursor_msg(0.1), 0);
        let first = batch(&r.batches(&[rx(2, "u_b")], &|_| 0), 2).unwrap();
        assert_eq!(first["senders"][0]["draft"]["text"], "hel");
        assert!(first["senders"][0].get("cursor").is_some());
        r.apply("u_a", 1, cursor_msg(0.2), 10);
        let second = batch(&r.batches(&[rx(2, "u_b")], &|_| 0), 2).unwrap();
        let s = &second["senders"][0];
        assert!(s.get("draft").is_none() && s.get("typing").is_none(), "{s}");
        assert_eq!(s["cursor"]["anchor"]["x"], 0.2);
        // A cleared draft clears typing too.
        r.apply(
            "u_a",
            1,
            ClientMessage::Draft(LiveDraft {
                thread_id: None,
                text: String::new(),
            }),
            20,
        );
        let third = batch(&r.batches(&[rx(2, "u_b")], &|_| 0), 2).unwrap();
        assert_eq!(third["senders"][0]["draft"], Value::Null);
        assert_eq!(third["senders"][0]["typing"], Value::Null);
        assert!(third["senders"][0].get("cursor").is_none());
    }

    #[test]
    fn a_receiver_that_may_not_read_comments_gets_no_typing_or_draft() {
        let mut r = Relay::default();
        r.apply(
            "u_a",
            1,
            ClientMessage::Draft(LiveDraft {
                thread_id: None,
                text: "secret".into(),
            }),
            0,
        );
        r.apply("u_a", 1, cursor_msg(0.1), 0);
        let blind = Receiver {
            text: false,
            ..rx(3, "u_c")
        };
        let out = r.batches(&[rx(2, "u_b"), blind], &|_| 0);
        let reader = batch(&out, 2).unwrap();
        assert_eq!(reader["senders"][0]["draft"]["text"], "secret");
        let b = batch(&out, 3).unwrap();
        let s = &b["senders"][0];
        assert!(s.get("draft").is_none() && s.get("typing").is_none(), "{s}");
        assert_eq!(s["cursor"]["anchor"]["x"], 0.1);
        assert!(!b.to_string().contains("secret"), "{b}");
        r.apply("u_a", 1, ClientMessage::Typing(None), 5);
        let out = r.batches(
            &[
                rx(2, "u_b"),
                Receiver {
                    text: false,
                    ..rx(3, "u_c")
                },
            ],
            &|_| 0,
        );
        assert!(batch(&out, 3).is_none(), "nothing it may see changed");
    }

    #[test]
    fn at_most_the_cap_of_senders_are_detailed_and_the_rest_counted() {
        let mut r = Relay::default();
        for i in 0..40u64 {
            r.apply(&format!("u_{i:02}"), i, cursor_msg(0.5), i);
        }
        let b = batch(&r.batches(&[rx(100, "u_me")], &|_| 0), 100).unwrap();
        let senders = b["senders"].as_array().unwrap();
        assert_eq!(senders.len(), LIVE_CAP);
        assert_eq!(b["others"], 10);
        // The most recently active first.
        assert_eq!(senders[0]["person_id"], "u_39");
        assert!(
            senders
                .iter()
                .all(|s| s["person_id"].as_str().unwrap() >= "u_10")
        );
    }

    #[test]
    fn people_in_the_receivers_thread_rank_first_and_a_sender_leaving_detail_is_dropped() {
        let mut r = Relay::new(2);
        r.apply("u_a", 1, ClientMessage::Typing(Some(T1.into())), 0);
        r.apply("u_b", 2, cursor_msg(0.5), 10);
        r.apply("u_c", 3, cursor_msg(0.5), 20);
        let mut me = rx(9, "u_me");
        me.open = Some(Some(T1.into()));
        let b = batch(&r.batches(std::slice::from_ref(&me), &|_| 0), 9).unwrap();
        let ids: Vec<&str> = b["senders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["person_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["u_a", "u_c"],
            "the thread's writer, then the most recent"
        );
        assert_eq!(b["others"], 1);
        // u_b moves: it is now more recent than u_c, which leaves detail.
        r.apply("u_b", 2, cursor_msg(0.6), 30);
        let b = batch(&r.batches(std::slice::from_ref(&me), &|_| 0), 9).unwrap();
        assert_eq!(b["drop"], json!(["u_c"]));
        assert_eq!(b["senders"][0]["person_id"], "u_b");
    }

    #[test]
    fn a_closed_socket_takes_its_persons_state_only_when_it_wrote_last() {
        let mut r = Relay::default();
        r.apply("u_a", 1, cursor_msg(0.5), 0);
        r.apply("u_a", 2, cursor_msg(0.6), 1);
        let _ = r.batches(&[rx(9, "u_b")], &|_| 0);
        r.leave("u_a", 1);
        assert!(
            r.batches(&[rx(9, "u_b")], &|_| 0).is_empty(),
            "tab 2 still holds it"
        );
        r.leave("u_a", 2);
        let b = batch(&r.batches(&[rx(9, "u_b")], &|_| 0), 9).unwrap();
        assert_eq!(b["drop"], json!(["u_a"]));
    }

    #[test]
    fn a_lost_batch_is_answered_whole_next_time() {
        let mut r = Relay::default();
        r.apply(
            "u_a",
            1,
            ClientMessage::Draft(LiveDraft {
                thread_id: None,
                text: "hi".into(),
            }),
            0,
        );
        r.apply("u_a", 1, cursor_msg(0.5), 0);
        let _ = r.batches(&[rx(9, "u_b")], &|_| 0);
        r.resend(9);
        assert!(r.pending());
        let b = batch(&r.batches(&[rx(9, "u_b")], &|_| 0), 9).unwrap();
        let s = &b["senders"][0];
        assert!(s.get("cursor").is_some() && s.get("draft").is_some());
    }

    #[test]
    fn clearing_text_goes_out_once() {
        let mut r = Relay::default();
        r.apply("u_a", 1, ClientMessage::Typing(None), 0);
        let _ = r.batches(&[rx(9, "u_b")], &|_| 0);
        r.clear_text("u_a");
        let b = batch(&r.batches(&[rx(9, "u_b")], &|_| 0), 9).unwrap();
        // It shows nothing now, so it is dropped from detail.
        assert_eq!(b["drop"], json!(["u_a"]));
        r.clear_text("u_a");
        assert!(!r.pending());
    }

    #[test]
    fn a_receiver_is_told_how_many_other_people_are_present() {
        let mut r = Relay::default();
        // Alone: nothing to say.
        assert!(r.batches(&[rx(1, "u_a")], &|_| 0).is_empty());
        // Ben joins with two tabs and shows nothing: Ann hears one person,
        // each of Ben's tabs one (Ann), and no one hears themself.
        let all = [rx(1, "u_a"), rx(2, "u_b"), rx(3, "u_b")];
        for k in [2, 3] {
            r.resend(k);
        }
        let out = r.batches(&all, &|_| 0);
        for k in [1, 2, 3] {
            let b = batch(&out, k).unwrap();
            assert_eq!(b["present"], 1, "{k}: {b}");
            assert_eq!(b["senders"], json!([]));
        }
        assert!(
            r.batches(&all, &|_| 0).is_empty(),
            "unchanged: not sent again"
        );
        // One of Ben's tabs closes: Ben is still present.
        r.leave("u_b", 3);
        assert!(r.pending());
        assert!(r.batches(&all[..2], &|_| 0).is_empty());
        // The other closes too: Ann is alone again.
        r.leave("u_b", 2);
        let b = batch(&r.batches(&all[..1], &|_| 0), 1).unwrap();
        assert_eq!(b["present"], 0);
    }

    #[test]
    fn an_open_message_ranks_but_shows_nothing() {
        let mut r = Relay::default();
        r.apply("u_a", 1, ClientMessage::Open(Some(T1.into())), 0);
        assert!(r.batches(&[rx(9, "u_b")], &|_| 0).is_empty());
    }
}

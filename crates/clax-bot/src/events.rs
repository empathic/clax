//! A run's live events (spec §10, `GET /api/bot/runs/<id>/events`):
//! `step_started`, `turn_delta`, `script`, `call`, `step_done` and
//! `run_end`, each with an SSE ID `<step>.<attempt>:<seq>` that never
//! repeats within a run.
//!
//! - `attempt` is the step's [`clax_store::bot_runs::Step::attempt`]: one
//!   more each time the step is begun again (after a crash, a stall or a
//!   retried model call), so a new attempt's events never reuse an ID.
//! - `seq` counts the attempt's events: `step_started` is 0, the deltas,
//!   the script and its calls follow from 1, `step_done` is [`DONE_SEQ`]
//!   and `run_end` [`DONE_SEQ`] + 1 (fixed, so a step's live and replayed
//!   ends have one ID).
//!
//! IDs order by `(step, attempt, seq)`. A subscriber that reconnects with
//! a `Last-Event-ID` gets every event after it: the committed steps
//! rebuilt from the store (final content only, no deltas: `step_started`,
//! a script's `script` and `call`s, `step_done` with the step's content,
//! `run_end`), then the live tail of the open attempt, then live events;
//! duplicates are dropped by ID. A `step_started` for a step the client
//! holds replaces that step's partial state.
//!
//! The channel is tokio's `broadcast` (its `sync` feature builds for
//! wasm32), so the hub works on the Worker's one thread as natively.

use clax_bot_sandbox::MaybeSendSync;
use clax_core::Result;
use clax_store::Store;
use clax_store::bot_runs::{RunStatus, Step, StepKind};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// The `seq` of a step's `step_done`; `run_end` is one more.
pub const DONE_SEQ: u32 = 1_000_000;

/// How many live events a slow subscriber may fall behind before it
/// must reconnect.
const CHANNEL_CAPACITY: usize = 1024;

/// An event's SSE ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId {
    pub step: i64,
    pub attempt: i64,
    pub seq: u32,
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}:{}", self.step, self.attempt, self.seq)
    }
}

impl EventId {
    /// The ID `s` names (`<step>.<attempt>:<seq>`), or `None`.
    #[must_use]
    pub fn parse(s: &str) -> Option<EventId> {
        let (sa, seq) = s.trim().split_once(':')?;
        let (step, attempt) = sa.split_once('.')?;
        Some(EventId {
            step: step.parse().ok()?,
            attempt: attempt.parse().ok()?,
            seq: seq.parse().ok()?,
        })
    }
}

/// What happened, before the hub numbers it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    StepStarted {
        kind: StepKind,
    },
    /// A piece of a model turn as it streams: `{type: "text"|"thinking"|
    /// "tool_input", index, text}`.
    TurnDelta(Value),
    Script {
        title: String,
        code: String,
    },
    Call {
        op: String,
        target: Option<String>,
        ok: bool,
        code: Option<String>,
        /// How long the call took, in milliseconds (0 for one answered
        /// from the journal).
        ms: u64,
    },
    /// The step committed. `summary` is [`step_summary`] of its payload.
    StepDone {
        kind: StepKind,
        credits_milli: Option<i64>,
        run_credits_milli: i64,
        summary: Value,
    },
    RunEnd {
        outcome: RunStatus,
        reason: Option<String>,
        credits_milli: i64,
    },
}

impl Event {
    /// The SSE event name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Event::StepStarted { .. } => "step_started",
            Event::TurnDelta(_) => "turn_delta",
            Event::Script { .. } => "script",
            Event::Call { .. } => "call",
            Event::StepDone { .. } => "step_done",
            Event::RunEnd { .. } => "run_end",
        }
    }

    fn data(&self, id: EventId) -> Value {
        let mut v = match self {
            Event::StepStarted { kind } => json!({"kind": kind.as_str()}),
            Event::TurnDelta(d) => d.clone(),
            Event::Script { title, code } => json!({"title": title, "code": code}),
            Event::Call {
                op,
                target,
                ok,
                code,
                ms,
            } => json!({"op": op, "target": target, "ok": ok, "code": code, "ms": ms}),
            Event::StepDone {
                kind,
                credits_milli,
                run_credits_milli,
                summary,
            } => json!({"kind": kind.as_str(), "credits_milli": credits_milli,
                        "run_credits_milli": run_credits_milli, "summary": summary}),
            Event::RunEnd {
                outcome,
                reason,
                credits_milli,
            } => json!({"outcome": outcome.as_str(), "reason": reason,
                        "credits_milli": credits_milli}),
        };
        if let Some(o) = v.as_object_mut() {
            o.insert("step".into(), id.step.into());
            o.insert("attempt".into(), id.attempt.into());
        }
        v
    }
}

/// One numbered event, as sent.
#[derive(Clone, Debug, PartialEq)]
pub struct RunEvent {
    pub id: EventId,
    /// The SSE event name ([`Event::name`]).
    pub event: &'static str,
    pub data: Value,
}

impl RunEvent {
    /// The event as an SSE frame (`id`, `event`, `data` lines and a blank
    /// line).
    #[must_use]
    pub fn to_sse(&self) -> String {
        format!(
            "id: {}\nevent: {}\ndata: {}\n\n",
            self.id, self.event, self.data
        )
    }
}

/// Where a tick sends its events.
pub trait EventSink: MaybeSendSync {
    fn emit(&self, run: &str, step: i64, attempt: i64, event: Event);
}

/// A sink that drops every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _run: &str, _step: i64, _attempt: i64, _event: Event) {}
}

struct Channel {
    tx: broadcast::Sender<RunEvent>,
    /// The events of the attempt at `at`, until its step is done.
    tail: Vec<RunEvent>,
    at: (i64, i64),
    next_seq: u32,
    /// The run ended: the channel goes once no one listens.
    ended: bool,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            tx: broadcast::channel(CHANNEL_CAPACITY).0,
            tail: Vec::new(),
            at: (0, 0),
            next_seq: 1,
            ended: false,
        }
    }
}

/// Every run's live events: numbers them, keeps the open attempt's tail,
/// and fans them out to subscribers.
#[derive(Default)]
pub struct EventHub {
    runs: Mutex<HashMap<String, Channel>>,
}

impl std::fmt::Debug for EventHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHub").finish_non_exhaustive()
    }
}

impl EventSink for EventHub {
    fn emit(&self, run: &str, step: i64, attempt: i64, event: Event) {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let ch = runs.entry(run.to_string()).or_insert_with(Channel::new);
        if ch.at != (step, attempt) {
            ch.at = (step, attempt);
            ch.tail.clear();
            ch.next_seq = 1;
        }
        let seq = match &event {
            Event::StepStarted { .. } => 0,
            Event::StepDone { .. } => DONE_SEQ,
            Event::RunEnd { .. } => DONE_SEQ + 1,
            _ => {
                if ch.next_seq >= DONE_SEQ {
                    return;
                }
                ch.next_seq += 1;
                ch.next_seq - 1
            }
        };
        let id = EventId { step, attempt, seq };
        let ev = RunEvent {
            id,
            event: event.name(),
            data: event.data(id),
        };
        let ends = matches!(event, Event::RunEnd { .. });
        if matches!(event, Event::StepDone { .. } | Event::RunEnd { .. }) {
            // The store has the step now.
            ch.tail.clear();
        } else {
            ch.tail.push(ev.clone());
        }
        let _ = ch.tx.send(ev);
        if ends {
            ch.ended = true;
        }
        prune(&mut runs);
    }
}

/// Drops the channels of ended runs no one listens to.
fn prune(runs: &mut HashMap<String, Channel>) {
    runs.retain(|_, c| !(c.ended && c.tx.receiver_count() == 0));
}

/// A committed step's content, as `step_done` carries it: a turn's text
/// and tool call, a script's result, a terminal step's summary or reason.
#[must_use]
pub fn step_summary(kind: StepKind, payload: &Value) -> Value {
    match kind {
        StepKind::Turn => {
            let text: Vec<&str> = payload["assistant"]
                .as_array()
                .map(|bs| {
                    bs.iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            let tool = payload["assistant"].as_array().and_then(|bs| {
                bs.iter()
                    .find(|b| b["type"] == "tool_use")
                    .map(|b| json!({"name": b["name"], "input": b["input"]}))
            });
            json!({"text": text.join("\n\n"), "tool": tool, "stop_reason": payload["stop_reason"]})
        }
        StepKind::Script => json!({
            "title": payload["title"],
            "result": payload["result"]["content"],
            "is_error": payload["result"]["is_error"].as_bool().unwrap_or(false),
            "changed": payload["changed"],
            "published": payload["published"],
        }),
        StepKind::Finish => json!({"summary": payload["summary"]}),
        StepKind::GiveUp | StepKind::Stopped | StepKind::Failed => {
            json!({"reason": payload["reason"], "notice": payload["notice"]})
        }
    }
}

/// The events committed step `s` is replayed as.
fn canonical(
    s: &Step,
    run_status: RunStatus,
    run_reason: Option<&str>,
    run_credits: i64,
    out: &mut Vec<RunEvent>,
) {
    let Some(payload) = &s.payload else {
        return;
    };
    let mut push = |seq: u32, e: Event| {
        let id = EventId {
            step: s.n,
            attempt: s.attempt,
            seq,
        };
        out.push(RunEvent {
            id,
            event: e.name(),
            data: e.data(id),
        });
    };
    push(0, Event::StepStarted { kind: s.kind });
    if s.kind == StepKind::Script {
        push(
            1,
            Event::Script {
                title: payload["title"].as_str().unwrap_or("").into(),
                code: payload["code"].as_str().unwrap_or("").into(),
            },
        );
        for (i, c) in payload["calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            push(
                2 + i as u32,
                Event::Call {
                    op: c["op"].as_str().unwrap_or("").into(),
                    target: c["target"].as_str().map(str::to_string),
                    ok: c["ok"].as_bool().unwrap_or(false),
                    code: c["code"].as_str().map(str::to_string),
                    ms: c["ms"].as_u64().unwrap_or(0),
                },
            );
        }
    }
    push(
        DONE_SEQ,
        Event::StepDone {
            kind: s.kind,
            credits_milli: payload["credits_milli"].as_i64(),
            run_credits_milli: run_credits,
            summary: step_summary(s.kind, payload),
        },
    );
    if s.kind.ends_as().is_some() && run_status.is_terminal() {
        push(
            DONE_SEQ + 1,
            Event::RunEnd {
                outcome: run_status,
                reason: run_reason.map(str::to_string),
                credits_milli: run_credits,
            },
        );
    }
}

/// Why [`Subscription::next`] stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lagged {
    /// The subscriber fell more than the channel holds behind; reconnect
    /// with the last ID it got.
    Behind,
}

/// One subscriber's events: the replay, then live ones, each once, in ID
/// order.
#[derive(Debug)]
pub struct Subscription {
    replay: VecDeque<RunEvent>,
    /// `None` for a run that had ended: there is nothing live to wait for.
    rx: Option<broadcast::Receiver<RunEvent>>,
    last: Option<EventId>,
    ended: bool,
}

impl Subscription {
    /// The next event; `Ok(None)` once the run has ended and every event
    /// up to its `run_end` was given.
    ///
    /// # Errors
    /// [`Lagged::Behind`].
    pub async fn next(&mut self) -> std::result::Result<Option<RunEvent>, Lagged> {
        if self.ended {
            return Ok(None);
        }
        let ev = loop {
            let ev = match self.replay.pop_front() {
                Some(ev) => ev,
                None => match self.rx.as_mut() {
                    None => return Ok(None),
                    Some(rx) => match rx.recv().await {
                        Ok(ev) => ev,
                        Err(broadcast::error::RecvError::Lagged(_)) => return Err(Lagged::Behind),
                        Err(broadcast::error::RecvError::Closed) => return Ok(None),
                    },
                },
            };
            if self.last.is_none_or(|l| ev.id > l) {
                break ev;
            }
        };
        self.last = Some(ev.id);
        if ev.event == "run_end" {
            self.ended = true;
        }
        Ok(Some(ev))
    }

    /// The replayed events not yet given (for a caller that sends them in
    /// one write).
    #[must_use]
    pub fn pending_replay(&self) -> usize {
        self.replay.len()
    }
}

impl EventHub {
    #[must_use]
    pub fn new() -> EventHub {
        EventHub::default()
    }

    /// How many runs the hub keeps a channel for (live runs, and ended
    /// ones someone still listens to).
    #[must_use]
    pub fn channels(&self) -> usize {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Subscribes to run `run`'s events after `after` (all of them for
    /// `None`), replaying what the store and the live tail hold.
    ///
    /// # Errors
    /// `NotFound` for no such run; the store's.
    pub fn subscribe(
        &self,
        store: &Store,
        run: &str,
        after: Option<EventId>,
    ) -> Result<Subscription> {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let (r, steps) = store
            .run_with_steps(run)?
            .ok_or(clax_core::CoreError::NotFound)?;
        let mut replay = Vec::new();
        for s in steps.iter().filter(|s| s.committed_at.is_some()) {
            canonical(
                s,
                r.status,
                r.reason.as_deref(),
                r.credits_milli,
                &mut replay,
            );
        }
        prune(&mut runs);
        let rx = if r.status.is_terminal() {
            None
        } else {
            let ch = runs.entry(run.to_string()).or_insert_with(Channel::new);
            replay.extend(ch.tail.iter().cloned());
            Some(ch.tx.subscribe())
        };
        replay.sort_by_key(|e| e.id);
        replay.dedup_by_key(|e| e.id);
        let replay: VecDeque<RunEvent> = replay
            .into_iter()
            .filter(|e| after.is_none_or(|a| e.id > a))
            .collect();
        let ended = r.status.is_terminal()
            && after.is_some_and(|a| a.seq == DONE_SEQ + 1)
            && replay.is_empty();
        Ok(Subscription {
            replay,
            rx,
            last: after,
            ended,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_print_parse_and_order() {
        let a = EventId {
            step: 3,
            attempt: 2,
            seq: 7,
        };
        assert_eq!(a.to_string(), "3.2:7");
        assert_eq!(EventId::parse("3.2:7"), Some(a));
        assert_eq!(EventId::parse("3:7"), None);
        assert!(EventId::parse("3.1:9").unwrap() < a);
        assert!(EventId::parse("3.2:1000000").unwrap() > a);
        assert!(EventId::parse("4.1:0").unwrap() > a);
    }

    #[test]
    fn the_hub_numbers_events_by_attempt() {
        let hub = EventHub::new();
        let ids = std::sync::Arc::new(Mutex::new(Vec::new()));
        {
            let mut runs = hub.runs.lock().unwrap();
            runs.insert("r".into(), Channel::new());
        }
        let mut rx = hub.runs.lock().unwrap()["r"].tx.subscribe();
        hub.emit(
            "r",
            1,
            1,
            Event::StepStarted {
                kind: StepKind::Turn,
            },
        );
        hub.emit(
            "r",
            1,
            1,
            Event::TurnDelta(json!({"type": "text", "text": "a"})),
        );
        hub.emit(
            "r",
            1,
            2,
            Event::StepStarted {
                kind: StepKind::Turn,
            },
        );
        hub.emit(
            "r",
            1,
            2,
            Event::TurnDelta(json!({"type": "text", "text": "b"})),
        );
        hub.emit(
            "r",
            1,
            2,
            Event::StepDone {
                kind: StepKind::Turn,
                credits_milli: Some(5),
                run_credits_milli: 5,
                summary: json!({}),
            },
        );
        while let Ok(e) = rx.try_recv() {
            ids.lock().unwrap().push(e.id.to_string());
        }
        assert_eq!(
            *ids.lock().unwrap(),
            ["1.1:0", "1.1:1", "1.2:0", "1.2:1", "1.2:1000000"]
        );
        assert!(hub.runs.lock().unwrap()["r"].tail.is_empty());
    }
}

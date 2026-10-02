//! One `sample()` call's run and its frames. [`drive`] asks the provider
//! round by round, relays page tool calls and waits for their results, and
//! pushes [`Out`] frames into a [`Flight`]; each reader replays a flight's
//! frames from the start and then follows it live. When the last reader goes
//! away before the flight ends, the run is aborted: the provider request is
//! dropped (the key stops paying) and the call's tool waits are forgotten.

use super::provider::{Block, ProviderEvent, ProviderRequest, Role, Stop, Turn};
use super::request::{Prepared, Tier, Verb};
use super::{Sampler, ToolOutput, json_reply};
use futures::{Stream, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::AbortHandle;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Done {
    pub text: String,
    pub truncated: bool,
    pub model_tier_applied: Tier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// One frame of a call's stream (the SSE frames of `docs/contract.md` "Sample protocol").
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    Done(Done),
    Error {
        code: &'static str,
        message: String,
    },
}

impl Out {
    /// The SSE event name and data.
    pub fn sse(&self) -> (&'static str, Value) {
        match self {
            Out::Text(d) => ("text", json!({"delta": d})),
            Out::ToolCall { id, name, input } => {
                ("tool_call", json!({"id": id, "name": name, "input": input}))
            }
            Out::Done(d) => ("done", serde_json::to_value(d).expect("done serialises")),
            Out::Error { code, message } => ("error", json!({"code": code, "message": message})),
        }
    }

    fn ends(&self) -> bool {
        matches!(self, Out::Done(_) | Out::Error { .. })
    }
}

#[derive(Default)]
pub struct Flight {
    frames: Mutex<Vec<Out>>,
    finished: AtomicBool,
    notify: Notify,
    readers: AtomicUsize,
    abort: Mutex<Option<AbortHandle>>,
}

impl Flight {
    pub fn new() -> Arc<Flight> {
        Arc::default()
    }

    /// Appends a frame; a `Done` or `Error` ends the flight (later pushes are ignored).
    pub fn push(&self, o: Out) {
        if self.is_finished() {
            return;
        }
        let ends = o.ends();
        self.frames.lock().expect("frames lock").push(o);
        if ends {
            self.finished.store(true, Ordering::SeqCst);
        }
        self.notify.notify_waiters();
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// The run to abort when the last reader leaves an unfinished flight.
    pub fn set_abort(&self, h: AbortHandle) {
        *self.abort.lock().expect("abort lock") = Some(h);
    }

    /// Every frame so far, then each new one, until the flight ends.
    pub fn reader(self: &Arc<Self>) -> impl Stream<Item = Out> + Send + 'static {
        self.readers.fetch_add(1, Ordering::SeqCst);
        let guard = ReaderGuard(self.clone());
        futures::stream::unfold((guard, 0usize), |(guard, next)| async move {
            loop {
                let flight = guard.0.clone();
                let notified = flight.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(o) = flight
                    .frames
                    .lock()
                    .expect("frames lock")
                    .get(next)
                    .cloned()
                {
                    return Some((o, (guard, next + 1)));
                }
                if flight.is_finished() {
                    return None;
                }
                notified.await;
            }
        })
    }
}

struct ReaderGuard(Arc<Flight>);

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        if self.0.readers.fetch_sub(1, Ordering::SeqCst) == 1
            && !self.0.is_finished()
            && let Some(h) = self.0.abort.lock().expect("abort lock").take()
        {
            h.abort();
        }
    }
}

/// Calls `f(None)` if dropped before [`Finish::done`]: the run was aborted or failed.
struct Finish<F: FnOnce(Option<&Done>)>(Option<F>);

impl<F: FnOnce(Option<&Done>)> Finish<F> {
    fn done(&mut self, d: Option<&Done>) {
        if let Some(f) = self.0.take() {
            f(d);
        }
    }
}

impl<F: FnOnce(Option<&Done>)> Drop for Finish<F> {
    fn drop(&mut self) {
        self.done(None);
    }
}

struct Forget(Arc<Sampler>, String);

impl Drop for Forget {
    fn drop(&mut self) {
        self.0.forget(&self.1);
    }
}

fn fail(flight: &Flight, code: &'static str, message: impl Into<String>) {
    flight.push(Out::Error {
        code,
        message: message.into(),
    });
}

/// Runs call `call_id` (already registered with [`Sampler::open_call`]) and
/// pushes its frames into `flight`. `on_finish` runs once: with the answer
/// just before a successful `Done` is pushed, with `None` on any failure or
/// when the run is aborted.
pub async fn drive(
    sampler: Arc<Sampler>,
    call_id: String,
    p: Prepared,
    flight: Arc<Flight>,
    on_finish: impl FnOnce(Option<&Done>) + Send + 'static,
) {
    let _forget = Forget(sampler.clone(), call_id.clone());
    let mut finish = Finish(Some(on_finish));
    let Some(provider) = sampler.provider().cloned() else {
        fail(
            &flight,
            "sampling_disabled",
            "no sample provider is configured on this machine",
        );
        return;
    };
    let rounds = if p.tools.is_empty() {
        1
    } else {
        sampler.settings.max_rounds.max(1)
    };
    let mut turns = p.turns.clone();
    let mut text = String::new();
    let mut last_round_text = String::new();
    let mut truncated = false;
    for round in 0..rounds {
        let req = ProviderRequest {
            model: p.model.clone(),
            max_tokens: p.max_tokens,
            system: p.system.clone(),
            messages: turns.clone(),
            tools: p.tools.clone(),
            final_round: round + 1 == rounds,
        };
        let mut stream = provider.stream(req);
        let mut calls = Vec::new();
        let mut end = None;
        last_round_text.clear();
        while let Some(ev) = stream.next().await {
            match ev {
                ProviderEvent::Text(d) if d.is_empty() => {}
                ProviderEvent::Text(d) => {
                    if last_round_text.is_empty() && !text.is_empty() {
                        text.push_str("\n\n");
                        flight.push(Out::Text("\n\n".into()));
                    }
                    text.push_str(&d);
                    last_round_text.push_str(&d);
                    flight.push(Out::Text(d));
                }
                ProviderEvent::ToolUse { id, name, input } => calls.push((id, name, input)),
                ProviderEvent::End { stop, assistant } => {
                    end = Some((stop, assistant));
                    break;
                }
                ProviderEvent::Error { code, message } => {
                    fail(&flight, code.as_str(), message);
                    return;
                }
            }
        }
        let Some((stop, assistant)) = end else {
            fail(
                &flight,
                "upstream_error",
                "the provider's answer ended early",
            );
            return;
        };
        match stop {
            Stop::Refusal => {
                fail(&flight, "refused", "Claude declined this request");
                return;
            }
            Stop::MaxTokens => {
                truncated = true;
                break;
            }
            Stop::ToolUse if !calls.is_empty() && round + 1 < rounds => {
                turns.push(Turn {
                    role: Role::Assistant,
                    content: assistant.into_iter().map(Block::Raw).collect(),
                });
                let mut waits = Vec::with_capacity(calls.len());
                for (id, ..) in &calls {
                    let Some(rx) = sampler.expect_tool(&call_id, id) else {
                        fail(
                            &flight,
                            "upstream_error",
                            "the call was closed while it waited for a tool",
                        );
                        return;
                    };
                    waits.push(rx);
                }
                for (id, name, input) in &calls {
                    flight.push(Out::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                let limit = sampler.settings.tool_timeout;
                let outputs = futures::future::join_all(waits.into_iter().map(|rx| async move {
                    match tokio::time::timeout(limit, rx).await {
                        Ok(Ok(o)) => o,
                        _ => ToolOutput {
                            content: format!(
                                "Error: the tool did not answer within {} s",
                                limit.as_secs()
                            ),
                            is_error: true,
                        },
                    }
                }))
                .await;
                let results = calls
                    .iter()
                    .zip(outputs)
                    .map(|((id, ..), o)| Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: o.content,
                        is_error: o.is_error,
                    })
                    .collect();
                turns.push(Turn {
                    role: Role::User,
                    content: results,
                });
            }
            Stop::ToolUse | Stop::EndTurn => break,
        }
    }
    if text.trim().is_empty() {
        fail(&flight, "empty_completion", "Claude produced no text");
        return;
    }
    let value = match p.verb {
        Verb::Text => None,
        Verb::Json if truncated => {
            fail(
                &flight,
                "invalid_json",
                "the answer was cut short by the length limit before its JSON was complete",
            );
            return;
        }
        Verb::Json => match json_reply::parse(&last_round_text) {
            Some(v) => Some(v),
            None => {
                fail(
                    &flight,
                    "invalid_json",
                    "the final message holds no parseable JSON value",
                );
                return;
            }
        },
    };
    let done = Done {
        text,
        truncated,
        model_tier_applied: p.tier,
        value,
    };
    // Forget the call before anyone can read `done`, so its tool waits are gone by then.
    sampler.forget(&call_id);
    finish.done(Some(&done));
    flight.push(Out::Done(done));
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn readers_replay_from_the_start_and_follow_until_the_end() {
        let f = Flight::new();
        f.push(Out::Text("a".into()));
        let early = f.reader();
        f.push(Out::Text("b".into()));
        let late = f.reader();
        let pusher = f.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            pusher.push(Out::Done(Done {
                text: "ab".into(),
                truncated: false,
                model_tier_applied: Tier::Default,
                value: None,
            }));
        });
        let (x, y): (Vec<Out>, Vec<Out>) = tokio::join!(early.collect(), late.collect());
        assert_eq!(x, y);
        assert_eq!(x.len(), 3);
        assert!(f.is_finished());
    }

    #[tokio::test]
    async fn the_last_reader_leaving_aborts_an_unfinished_flight() {
        let f = Flight::new();
        let task = tokio::spawn(std::future::pending::<()>());
        f.set_abort(task.abort_handle());
        let (a, b) = (f.reader(), f.reader());
        drop(a);
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        drop(b);
        let r = task.await;
        assert!(r.unwrap_err().is_cancelled());
    }

    #[test]
    fn frames_serialise_as_the_sse_contract() {
        assert_eq!(
            Out::Text("hi".into()).sse(),
            ("text", serde_json::json!({"delta": "hi"}))
        );
        let d = Done {
            text: "t".into(),
            truncated: true,
            model_tier_applied: Tier::Quick,
            value: None,
        };
        assert_eq!(
            Out::Done(d).sse(),
            (
                "done",
                serde_json::json!({"text": "t", "truncated": true, "model_tier_applied": "quick"})
            )
        );
        assert_eq!(
            Out::Error {
                code: "refused",
                message: "no".into()
            }
            .sse(),
            (
                "error",
                serde_json::json!({"code": "refused", "message": "no"})
            )
        );
    }
}

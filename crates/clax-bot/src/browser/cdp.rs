//! One Chrome DevTools Protocol connection over a [`CdpTransport`]
//! (spec §4.3): commands with their IDs, answers matched to them, the
//! events that arrive meanwhile, flattened sessions, and the bounds every
//! wait is held to.
//!
//! # Commands and answers
//!
//! Each command gets the next ID, from 1. [`Cdp::send`] sends one and
//! [`Cdp::wait`] waits for its answer; [`Cdp::call`] does both. An answer
//! to a command sent but not yet waited for is kept until it is: answers
//! may come in any order. [`Cdp::send_only`] sends a command whose answer
//! is dropped when it comes (a paused request's fulfilment, the closing
//! commands). An answer with `error` is [`CdpFail::Protocol`] with its
//! message.
//!
//! # Events
//!
//! Every frame without an `id` is an event. One that arrives while a
//! command waits is queued, in arrival order, and handed out by
//! [`Cdp::drain_events`], [`Cdp::next_event`] or
//! [`Cdp::answer_or_event`] before any later frame: none is dropped.
//!
//! # Sessions
//!
//! A command for an attached page (`Target.attachToTarget` with `flatten`)
//! carries its `sessionId`; an event of that page carries it back
//! ([`CdpEvent::session`]).
//!
//! # Bounds
//!
//! Every wait ends at the deadline ([`CdpFail::Deadline`]), at the run's
//! cancellation ([`CdpFail::Cancelled`]) or when the socket closes
//! ([`CdpFail::Closed`]), whichever is first; once one of them has
//! happened every later wait answers it at once. Sends are not bounded:
//! a transport's `send` hands a frame to its socket. Closing the
//! connection ([`Cdp::close`]) closes the transport once.

use super::{BrowserError, CdpTransport, Deadline};
use clax_bot_sandbox::CancelToken;
use futures::FutureExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::future::Future;
use std::task::Poll;

/// One event of the browser: its method, its parameters, and the session
/// (page) it is of, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

/// Why a command or a wait failed.
#[derive(Clone, Debug, PartialEq)]
pub enum CdpFail {
    /// The test's deadline passed.
    Deadline,
    /// The run was cancelled.
    Cancelled,
    /// The socket closed.
    Closed,
    /// The browser answered an error, or sent a frame that is not CDP.
    Protocol(String),
    /// The transport failed.
    Transport(BrowserError),
}

impl fmt::Display for CdpFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CdpFail::Deadline => f.write_str("the test's time ran out"),
            CdpFail::Cancelled => f.write_str("the run was cancelled"),
            CdpFail::Closed => f.write_str("the browser's connection closed"),
            CdpFail::Protocol(m) => write!(f, "the browser answered: {m}"),
            CdpFail::Transport(e) => write!(f, "{e}"),
        }
    }
}

/// What [`Cdp::answer_or_event`] got first.
#[derive(Clone, Debug, PartialEq)]
pub enum Got {
    /// The command's answer: its `result`, or the error it answered.
    Answer(Result<Value, CdpFail>),
    /// An event, before the answer.
    Event(CdpEvent),
}

/// What ended the connection's waits for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    Deadline,
    Cancelled,
    Closed,
}

impl Ended {
    fn fail(self) -> CdpFail {
        match self {
            Ended::Deadline => CdpFail::Deadline,
            Ended::Cancelled => CdpFail::Cancelled,
            Ended::Closed => CdpFail::Closed,
        }
    }
}

/// One CDP connection (see the module docs).
pub struct Cdp {
    transport: Box<dyn CdpTransport>,
    deadline: Deadline,
    cancel: CancelToken,
    next_id: u64,
    /// Commands sent whose answers someone will wait for.
    awaited: BTreeSet<u64>,
    /// Answers that came before their wait.
    answers: BTreeMap<u64, Result<Value, CdpFail>>,
    events: VecDeque<CdpEvent>,
    ended: Option<Ended>,
    transport_closed: bool,
}

impl fmt::Debug for Cdp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cdp")
            .field("next_id", &self.next_id)
            .field("events", &self.events.len())
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

/// What one frame read from the socket is.
enum Frame {
    Answer(u64, Result<Value, CdpFail>),
    Event(CdpEvent),
}

/// `frame` with each `\u` escape of a lone UTF-16 surrogate replaced by
/// U+FFFD, or `None` when it has none. A page's strings may hold lone
/// surrogates, which Chrome writes as escapes and JSON parsers refuse.
fn mend_surrogates(frame: &str) -> Option<String> {
    fn escape_at(b: &[u8], at: usize) -> Option<u16> {
        if b.get(at) != Some(&b'\\') || b.get(at + 1) != Some(&b'u') {
            return None;
        }
        let hex = std::str::from_utf8(b.get(at + 2..at + 6)?).ok()?;
        u16::from_str_radix(hex, 16).ok()
    }
    let b = frame.as_bytes();
    let mut out = String::with_capacity(frame.len());
    let (mut copied, mut i, mut mended) = (0, 0, false);
    while i < b.len() {
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        let Some(unit) = escape_at(b, i) else {
            // Another escape (`\\` among them): skip it whole, so the `u`
            // after an escaped backslash is not read as an escape.
            i += 2;
            continue;
        };
        let high = (0xD800..0xDC00).contains(&unit);
        let paired = high && escape_at(b, i + 6).is_some_and(|n| (0xDC00..0xE000).contains(&n));
        if paired {
            i += 12;
        } else if (0xD800..0xE000).contains(&unit) {
            out.push_str(&frame[copied..i]);
            out.push_str("\\ufffd");
            i += 6;
            copied = i;
            mended = true;
        } else {
            i += 6;
        }
    }
    mended.then(|| {
        out.push_str(&frame[copied..]);
        out
    })
}

fn parse(frame: &str) -> Result<Frame, CdpFail> {
    let v: Value = match serde_json::from_str(frame) {
        Ok(v) => v,
        Err(e) => mend_surrogates(frame)
            .and_then(|m| serde_json::from_str(&m).ok())
            .ok_or_else(|| CdpFail::Protocol(format!("a frame that is not JSON ({e})")))?,
    };
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        let answer = match v.get("error") {
            Some(err) => Err(CdpFail::Protocol(
                err.get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| err.to_string(), str::to_string),
            )),
            None => Ok(v.get("result").cloned().unwrap_or_else(|| json!({}))),
        };
        return Ok(Frame::Answer(id, answer));
    }
    let Some(method) = v.get("method").and_then(Value::as_str) else {
        return Err(CdpFail::Protocol(
            "a frame with neither an id nor a method".into(),
        ));
    };
    Ok(Frame::Event(CdpEvent {
        method: method.to_string(),
        params: v.get("params").cloned().unwrap_or_else(|| json!({})),
        session: v
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

impl Cdp {
    /// A connection over `transport`, its waits bounded by `deadline` and
    /// `cancel`.
    #[must_use]
    pub fn new(transport: Box<dyn CdpTransport>, deadline: Deadline, cancel: CancelToken) -> Cdp {
        Cdp {
            transport,
            deadline,
            cancel,
            next_id: 1,
            awaited: BTreeSet::new(),
            answers: BTreeMap::new(),
            events: VecDeque::new(),
            ended: None,
            transport_closed: false,
        }
    }

    /// Whether the deadline has passed (as a wait saw it).
    #[must_use]
    pub fn expired(&self) -> bool {
        self.ended == Some(Ended::Deadline)
    }

    /// Whether the deadline passed, the run was cancelled or the socket
    /// closed: every later wait fails at once.
    #[must_use]
    pub fn ended(&self) -> Option<CdpFail> {
        if self.ended.is_none() && self.cancel.is_cancelled() {
            return Some(CdpFail::Cancelled);
        }
        self.ended.map(Ended::fail)
    }

    async fn send_frame(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<u64, CdpFail> {
        if self.ended == Some(Ended::Closed) || self.transport_closed {
            return Err(CdpFail::Closed);
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut frame = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            frame["sessionId"] = json!(s);
        }
        self.transport
            .send(frame.to_string())
            .await
            .map_err(CdpFail::Transport)?;
        Ok(id)
    }

    /// Sends `method` with `params`, on `session` when given; its answer
    /// is waited for with [`Cdp::wait`] or [`Cdp::answer_or_event`].
    ///
    /// # Errors
    /// [`CdpFail::Closed`] once the socket closed; the transport's.
    pub async fn send(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<u64, CdpFail> {
        let id = self.send_frame(session, method, params).await?;
        self.awaited.insert(id);
        Ok(id)
    }

    /// Sends `method` with `params`, on `session` when given, and drops
    /// its answer when it comes.
    ///
    /// # Errors
    /// As [`Cdp::send`].
    pub async fn send_only(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<(), CdpFail> {
        self.send_frame(session, method, params).await.map(|_| ())
    }

    /// Reads the next frame, bounded (see the module docs).
    async fn read(&mut self) -> Result<Frame, CdpFail> {
        loop {
            if let Some(fail) = self.ended() {
                return Err(fail);
            }
            let Cdp {
                transport,
                deadline,
                cancel,
                ended,
                ..
            } = self;
            let recv = transport.recv();
            futures::pin_mut!(recv);
            let cancelled = cancel.cancelled();
            futures::pin_mut!(cancelled);
            let got = std::future::poll_fn(|cx| {
                if cancelled.as_mut().poll(cx).is_ready() {
                    *ended = Some(Ended::Cancelled);
                    return Poll::Ready(Err(CdpFail::Cancelled));
                }
                if deadline.poll_unpin(cx).is_ready() {
                    *ended = Some(Ended::Deadline);
                    return Poll::Ready(Err(CdpFail::Deadline));
                }
                recv.as_mut()
                    .poll(cx)
                    .map(|r| r.map_err(CdpFail::Transport))
            })
            .await;
            let frame = match got {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    self.ended = Some(Ended::Closed);
                    return Err(CdpFail::Closed);
                }
                Err(e) => return Err(e),
            };
            match parse(&frame)? {
                Frame::Answer(id, answer) => {
                    if self.awaited.contains(&id) {
                        return Ok(Frame::Answer(id, answer));
                    }
                    // An answer to a command sent with `send_only`, or to
                    // none: dropped.
                }
                event @ Frame::Event(_) => return Ok(event),
            }
        }
    }

    /// The answer to command `id` (from [`Cdp::send`]), or the next event
    /// if one comes first: events already queued come before anything
    /// read.
    ///
    /// # Errors
    /// The bounds' (see the module docs), and a frame that is not CDP.
    pub async fn answer_or_event(&mut self, id: u64) -> Result<Got, CdpFail> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Got::Event(event));
        }
        if let Some(answer) = self.answers.remove(&id) {
            self.awaited.remove(&id);
            return Ok(Got::Answer(answer));
        }
        loop {
            match self.read().await? {
                Frame::Answer(got, answer) if got == id => {
                    self.awaited.remove(&id);
                    return Ok(Got::Answer(answer));
                }
                Frame::Answer(other, answer) => {
                    self.answers.insert(other, answer);
                }
                Frame::Event(event) => return Ok(Got::Event(event)),
            }
        }
    }

    /// The answer to command `id` (from [`Cdp::send`]), queueing every
    /// event that arrives meanwhile.
    ///
    /// # Errors
    /// The answer's error ([`CdpFail::Protocol`]), the bounds' (see the
    /// module docs), and a frame that is not CDP.
    pub async fn wait(&mut self, id: u64) -> Result<Value, CdpFail> {
        if let Some(answer) = self.answers.remove(&id) {
            self.awaited.remove(&id);
            return answer;
        }
        loop {
            match self.read().await? {
                Frame::Answer(got, answer) if got == id => {
                    self.awaited.remove(&id);
                    return answer;
                }
                Frame::Answer(other, answer) => {
                    self.answers.insert(other, answer);
                }
                Frame::Event(event) => self.events.push_back(event),
            }
        }
    }

    /// Sends `method` with `params`, on `session` when given, and waits for
    /// its answer, queueing every event that arrives meanwhile.
    ///
    /// # Errors
    /// As [`Cdp::send`] and [`Cdp::wait`].
    pub async fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpFail> {
        let id = self.send(session, method, params).await?;
        self.wait(id).await
    }

    /// The events queued so far, in arrival order, emptying the queue.
    pub fn drain_events(&mut self) -> Vec<CdpEvent> {
        self.events.drain(..).collect()
    }

    /// The next event: the first queued, else the next to arrive.
    ///
    /// # Errors
    /// The bounds' (see the module docs), and a frame that is not CDP.
    pub async fn next_event(&mut self) -> Result<CdpEvent, CdpFail> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        loop {
            match self.read().await? {
                Frame::Answer(id, answer) => {
                    self.answers.insert(id, answer);
                }
                Frame::Event(event) => return Ok(event),
            }
        }
    }

    /// `work`'s output, unless the deadline passes or the run is
    /// cancelled first: work done between reads (a paused request's
    /// answer from the host) is held to the same bounds as a read.
    ///
    /// # Errors
    /// [`CdpFail::Deadline`] or [`CdpFail::Cancelled`], each ending every
    /// later wait as a read's does; at once when one already has.
    pub async fn bounded<T>(&mut self, work: impl Future<Output = T>) -> Result<T, CdpFail> {
        if let Some(fail) = self.ended() {
            return Err(fail);
        }
        let Cdp {
            deadline,
            cancel,
            ended,
            ..
        } = self;
        futures::pin_mut!(work);
        let cancelled = cancel.cancelled();
        futures::pin_mut!(cancelled);
        std::future::poll_fn(|cx| {
            if cancelled.as_mut().poll(cx).is_ready() {
                *ended = Some(Ended::Cancelled);
                return Poll::Ready(Err(CdpFail::Cancelled));
            }
            if deadline.poll_unpin(cx).is_ready() {
                *ended = Some(Ended::Deadline);
                return Poll::Ready(Err(CdpFail::Deadline));
            }
            work.as_mut().poll(cx).map(Ok)
        })
        .await
    }

    /// Closes the transport; a second close does nothing.
    pub async fn close(&mut self) {
        if !self.transport_closed {
            self.transport_closed = true;
            self.transport.close().await;
        }
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::browser::fake::{FakeChrome, answer, event};
    use futures::channel::oneshot;

    /// A deadline that never fires.
    fn never() -> Deadline {
        futures::future::pending().boxed()
    }

    #[tokio::test]
    async fn answers_are_matched_by_id_out_of_order() {
        // The fake answers the first command only with the second's
        // answer: the second's answer, then the first's.
        let mut first = None;
        let chrome = FakeChrome::new(move |cmd: &Value| {
            let id = cmd["id"].as_u64().unwrap();
            if id == 1 {
                first = Some(cmd.clone());
                return vec![];
            }
            let first = first.take().unwrap();
            vec![
                answer(cmd, json!({"n": "second"})),
                answer(&first, json!({"n": "first"})),
            ]
        });
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        let one = cdp.send(None, "A.one", json!({})).await.unwrap();
        let two = cdp.send(None, "A.two", json!({})).await.unwrap();
        assert_eq!((one, two), (1, 2));
        assert_eq!(cdp.wait(one).await.unwrap(), json!({"n": "first"}));
        assert_eq!(cdp.wait(two).await.unwrap(), json!({"n": "second"}));
        assert_eq!(log.methods(), ["A.one", "A.two"]);
    }

    #[test]
    fn a_lone_surrogate_from_the_page_is_read_as_a_replacement() {
        // A console message and an evaluate answer, each holding a lone
        // surrogate escape, as Chrome writes a page's broken string.
        let event =
            r#"{"method":"Runtime.consoleAPICalled","params":{"args":[{"value":"a\ud83d"}]}}"#;
        let Ok(Frame::Event(e)) = parse(event) else {
            panic!("an event")
        };
        assert_eq!(e.params["args"][0]["value"], "a\u{fffd}");
        let answer = r#"{"id":4,"result":{"result":{"value":"\udc00b😀"}}}"#;
        let Ok(Frame::Answer(4, Ok(v))) = parse(answer) else {
            panic!("an answer")
        };
        assert_eq!(v["result"]["value"], "\u{fffd}b\u{1f600}");
        // An escaped backslash before `u` is text, and a frame that is
        // not JSON still fails.
        assert_eq!(mend_surrogates(r#"{"a":"\\ud83d"}"#), None);
        assert!(matches!(parse("{nope"), Err(CdpFail::Protocol(_))));
    }

    #[tokio::test]
    async fn events_between_a_command_and_its_answer_are_kept() {
        let chrome = FakeChrome::new(|cmd: &Value| {
            vec![
                event(
                    "Runtime.consoleAPICalled",
                    json!({"type": "log", "args": [{"value": "one"}]}),
                    Some("S"),
                ),
                event(
                    "Runtime.consoleAPICalled",
                    json!({"type": "log", "args": [{"value": "two"}]}),
                    Some("S"),
                ),
                answer(cmd, json!({"result": {"type": "number", "value": 2}})),
                event("Log.entryAdded", json!({"entry": {"text": "after"}}), None),
            ]
        });
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        let got = cdp
            .call(Some("S"), "Runtime.evaluate", json!({"expression": "1+1"}))
            .await
            .unwrap();
        assert_eq!(got["result"]["value"], 2);
        let events = cdp.drain_events();
        let texts: Vec<&str> = events
            .iter()
            .map(|e| e.params["args"][0]["value"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["one", "two"]);
        assert_eq!(events[0].session.as_deref(), Some("S"));
        assert!(cdp.drain_events().is_empty());
        // The frame after the answer is the next event.
        let after = cdp.next_event().await.unwrap();
        assert_eq!(after.method, "Log.entryAdded");
        assert_eq!(after.session, None);
    }

    #[tokio::test]
    async fn the_session_id_goes_on_page_commands() {
        let chrome = FakeChrome::new(|cmd: &Value| vec![answer(cmd, json!({}))]);
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        cdp.call(None, "Target.createTarget", json!({"url": "about:blank"}))
            .await
            .unwrap();
        cdp.call(Some("PAGE-1"), "Page.enable", json!({}))
            .await
            .unwrap();
        let sent = log.commands();
        assert_eq!(sent[0].get("sessionId"), None);
        assert_eq!(sent[0]["params"]["url"], "about:blank");
        assert_eq!(sent[1]["sessionId"], "PAGE-1");
        assert_eq!(sent[1]["method"], "Page.enable");
    }

    #[tokio::test]
    async fn an_error_answer_is_a_protocol_failure() {
        let chrome = FakeChrome::new(|cmd: &Value| {
            vec![json!({"id": cmd["id"], "error": {"code": -32000, "message": "No node"}})]
        });
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        assert_eq!(
            cdp.call(None, "DOM.focus", json!({})).await,
            Err(CdpFail::Protocol("No node".into()))
        );
    }

    #[tokio::test]
    async fn the_deadline_ends_a_waiting_call() {
        // A command the browser never answers.
        let chrome = FakeChrome::new(|_: &Value| vec![]);
        let (fire, fired) = oneshot::channel::<()>();
        let deadline = async move {
            let _ = fired.await;
        }
        .boxed();
        let mut cdp = Cdp::new(Box::new(chrome), deadline, CancelToken::new());
        let id = cdp.send(None, "Page.navigate", json!({})).await.unwrap();
        {
            let wait = cdp.wait(id);
            futures::pin_mut!(wait);
            assert!(futures::poll!(wait.as_mut()).is_pending());
            fire.send(()).unwrap();
            assert_eq!(wait.await, Err(CdpFail::Deadline));
        }
        assert!(cdp.expired());
        // Every later wait answers at once; sending still works, so the
        // browser can be closed.
        assert_eq!(cdp.next_event().await, Err(CdpFail::Deadline));
        cdp.send_only(None, "Browser.close", json!({}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_ends_a_waiting_call() {
        let chrome = FakeChrome::new(|_: &Value| vec![]);
        let cancel = CancelToken::new();
        let mut cdp = Cdp::new(Box::new(chrome), never(), cancel.clone());
        let id = cdp.send(None, "Page.navigate", json!({})).await.unwrap();
        {
            let wait = cdp.wait(id);
            futures::pin_mut!(wait);
            assert!(futures::poll!(wait.as_mut()).is_pending());
            cancel.cancel();
            assert_eq!(wait.await, Err(CdpFail::Cancelled));
        }
        assert_eq!(cdp.ended(), Some(CdpFail::Cancelled));
        assert!(!cdp.expired());
    }

    #[tokio::test]
    async fn a_closed_socket_is_closed_not_a_hang() {
        // The browser closes its socket instead of answering.
        let chrome = FakeChrome::new(|_: &Value| vec![]);
        let log = chrome.log();
        let mut cdp = Cdp::new(Box::new(chrome), never(), CancelToken::new());
        let id = cdp.send(None, "Page.navigate", json!({})).await.unwrap();
        log.close_socket();
        assert_eq!(cdp.wait(id).await, Err(CdpFail::Closed));
        assert_eq!(
            cdp.call(None, "Page.enable", json!({})).await,
            Err(CdpFail::Closed)
        );
        cdp.close().await;
        cdp.close().await;
        assert_eq!(log.closes(), 1);
    }
}

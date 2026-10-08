//! Mirroring Claude Code's `AskUserQuestion` into Clax (spec
//! 2026-10-06-agent-questions-and-inbox-design §4.4). `PreToolUse` mirrors the
//! call and, while a Clax surface of the owner's is open, waits for the
//! person's answer there: answered → allow with the answers as the tool's
//! input; skipped → deny; moved to the terminal, timed out, or any failure →
//! no output, so the terminal dialog appears. `PostToolUse` records the
//! terminal's answer on a question that was moved there.

use crate::events::{Daemon, live_session};
use crate::input::HookInput;
use crate::output::HookOutput;
use clax_core::questions::{Answer, Question, to_claude};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The reason a skipped question's call is denied with; Claude reads it.
pub const DECLINED: &str = "The person chose not to answer this question in Clax. Continue without the answer, or ask again differently.";

/// The longest wait the hook asks of the daemon, in seconds (the daemon's
/// `terminal_after_s` is clamped to this too).
const MAX_WAIT_S: u64 = 3300;

/// How a `PreToolUse` run ended, as logged to hooks.log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Answered in Clax: the call was allowed with the answers.
    Answered,
    /// Skipped in Clax: the call was denied.
    Declined,
    /// Moved to the terminal in Clax, or withdrawn (its session ended, or
    /// the daemon shut down), while the hook waited.
    Released,
    /// The daemon chose `terminal` mode: no Clax surface was open.
    Terminal,
    /// `terminal_after_s` passed with no answer; the hook released it.
    Timeout,
    /// A request failed, or the session is not known to the daemon. Also a
    /// malformed `answered` result: the poll has then marked the answer
    /// taken, so Clax shows it answered although Claude never got it (the
    /// daemon validates answers when they are given, so this is defensive).
    Error,
    /// The input is not an `AskUserQuestion` call.
    Skipped,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Answered => "answered",
            Outcome::Declined => "declined",
            Outcome::Released => "released",
            Outcome::Terminal => "terminal",
            Outcome::Timeout => "timeout",
            Outcome::Error => "error",
            Outcome::Skipped => "skipped",
        }
    }
}

/// How long the hook may take: `extra` beyond the daemon's
/// `terminal_after_s` for the long poll.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub extra: Duration,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            extra: Duration::from_secs(10),
        }
    }
}

/// One `PreToolUse` run, with what hooks.log records about it.
#[derive(Debug)]
pub struct Asked {
    pub out: HookOutput,
    pub outcome: Outcome,
    /// The daemon's mode (`wait` or `terminal`), None when it never chose one.
    pub mode: Option<&'static str>,
    /// How long the hook held its poll.
    pub waited: Duration,
}

fn is_ask(input: &HookInput) -> bool {
    input.rest.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion")
}

/// `PreToolUse` for `AskUserQuestion`; see the module comment. Never fails:
/// every error is `(none, Error)`.
pub fn ask(input: &HookInput, daemon: &dyn Daemon, budget: Budget) -> (HookOutput, Outcome) {
    let a = ask_logged(input, daemon, budget);
    (a.out, a.outcome)
}

/// [`ask`], with the mode and the time waited for hooks.log.
pub fn ask_logged(input: &HookInput, daemon: &dyn Daemon, budget: Budget) -> Asked {
    let mut a = Asked {
        out: HookOutput::none(),
        outcome: Outcome::Skipped,
        mode: None,
        waited: Duration::ZERO,
    };
    if !is_ask(input) {
        return a;
    }
    if try_ask(input, daemon, budget, &mut a).is_err() {
        a.out = HookOutput::none();
        a.outcome = Outcome::Error;
    }
    a
}

fn try_ask(
    input: &HookInput,
    daemon: &dyn Daemon,
    budget: Budget,
    a: &mut Asked,
) -> anyhow::Result<()> {
    a.outcome = Outcome::Error;
    let tool_input = input.rest.get("tool_input").cloned().unwrap_or(Value::Null);
    let Some(sid) = live_session("claude", input, daemon)? else {
        return Ok(());
    };
    let mut body = json!({"source": "hook", "questions": tool_input["questions"]});
    if let Some(t) = input.rest.get("tool_use_id").and_then(Value::as_str) {
        body["tool_use_id"] = json!(t);
    }
    let made = daemon.post(&format!("/api/sessions/{sid}/questions"), &body)?;
    if made["mode"] != "wait" {
        a.mode = Some("terminal");
        a.outcome = Outcome::Terminal;
        return Ok(());
    }
    a.mode = Some("wait");
    let qid = made["question"]["id"]
        .as_str()
        .filter(|q| clax_core::is_ulid(q))
        .ok_or_else(|| anyhow::anyhow!("no question ID"))?
        .to_string();
    let after = made["terminal_after_s"]
        .as_u64()
        .unwrap_or(600)
        .min(MAX_WAIT_S);
    let path = format!("/api/sessions/{sid}/questions/{qid}");
    let started = Instant::now();
    let got = daemon.get_with_timeout(
        &format!("{path}?wait={after}"),
        Duration::from_secs(after) + budget.extra,
    );
    a.waited = started.elapsed();
    let got = got?;
    (a.out, a.outcome) = match settle(&tool_input, &got["question"])? {
        Some(done) => done,
        None => {
            // Still open: the wait ran out. Hand it to the terminal; when the
            // release loses to an answer given in that instant, take the
            // answer instead. A release that failed with the question still
            // open is tried once more; failing again, it is an error.
            let release = || daemon.post(&format!("{path}/release"), &json!({}));
            if release().is_ok() {
                (HookOutput::none(), Outcome::Timeout)
            } else {
                let now = daemon.get(&format!("{path}?wait=0"))?;
                match settle(&tool_input, &now["question"])? {
                    Some(done) => done,
                    None => {
                        release()?;
                        (HookOutput::none(), Outcome::Timeout)
                    }
                }
            }
        }
    };
    Ok(())
}

/// The output for a question that is no longer open, or None while it is.
fn settle(tool_input: &Value, q: &Value) -> anyhow::Result<Option<(HookOutput, Outcome)>> {
    Ok(Some(match q["status"].as_str() {
        Some("answered") => {
            let qs: Vec<Question> = serde_json::from_value(q["questions"].clone())?;
            let given: Vec<Answer> = serde_json::from_value(q["answers"].clone())?;
            anyhow::ensure!(qs.len() == given.len(), "one answer per question");
            let (answers, annotations) = to_claude(&qs, &given);
            let mut updated = tool_input.clone();
            updated["answers"] = Value::Object(answers);
            if !annotations.is_empty() {
                updated["annotations"] = Value::Object(annotations);
            }
            (HookOutput::allow_with_input(updated), Outcome::Answered)
        }
        Some("declined") => (HookOutput::deny(DECLINED), Outcome::Declined),
        Some("open") => return Ok(None),
        _ => (HookOutput::none(), Outcome::Released),
    }))
}

/// `PostToolUse` for `AskUserQuestion`: records the terminal's answers on
/// the question that was moved to the terminal, if any.
///
/// # Errors
/// When the session lookup or the request fails (the caller exits 0).
pub fn asked(input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    if !is_ask(input) {
        return Ok(HookOutput::none());
    }
    let (Some(t), Some(answers)) = (
        input.rest.get("tool_use_id").and_then(Value::as_str),
        input
            .rest
            .get("tool_response")
            .and_then(|r| r.get("answers"))
            .filter(|a| a.is_object()),
    ) else {
        return Ok(HookOutput::none());
    };
    if let Some(sid) = live_session("claude", input, daemon)? {
        daemon.post(
            &format!("/api/sessions/{sid}/questions:terminal"),
            &json!({"tool_use_id": t, "answers": answers}),
        )?;
    }
    Ok(HookOutput::none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The question's ID in every scripted answer.
    const Q: &str = "01J9ZQ3V5W6X7Y8Z9A0B1C2D3E";

    type Step = (&'static str, String, anyhow::Result<Value>);

    /// Answers each request from a script: (method, path prefix) → result.
    struct Fake {
        script: RefCell<Vec<Step>>,
        seen: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(s: Vec<Step>) -> Fake {
            Fake {
                script: RefCell::new(s),
                seen: RefCell::default(),
            }
        }
        fn take(&self, m: &str, p: &str) -> anyhow::Result<Value> {
            self.seen.borrow_mut().push(format!("{m} {p}"));
            let mut s = self.script.borrow_mut();
            let i = s
                .iter()
                .position(|(mm, pp, _)| *mm == m && p.starts_with(pp.as_str()))
                .unwrap_or_else(|| panic!("unexpected {m} {p}"));
            s.remove(i).2
        }
    }

    impl Daemon for Fake {
        fn browser_url(&self, p: &str) -> String {
            format!("http://localhost:7480{p}")
        }
        fn get(&self, p: &str) -> anyhow::Result<Value> {
            self.take("GET", p)
        }
        fn get_with_timeout(&self, p: &str, _t: Duration) -> anyhow::Result<Value> {
            self.take("GET", p)
        }
        fn post(&self, p: &str, _b: &Value) -> anyhow::Result<Value> {
            self.take("POST", p)
        }
        fn patch(&self, p: &str, _b: &Value) -> anyhow::Result<Value> {
            self.take("PATCH", p)
        }
    }

    fn input() -> HookInput {
        HookInput::parse(
            &json!({"session_id": "hs1", "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
            "tool_use_id": "toolu_1", "tool_input": {"questions": [{"question": "Which?", "header": "Pick",
            "options": [{"label": "A", "description": "a", "preview": "A!"}, {"label": "B", "description": "b"}], "multiSelect": false}]}})
            .to_string(),
        )
    }
    fn sessions() -> Step {
        (
            "GET",
            "/api/sessions?live=true".into(),
            Ok(
                json!({"sessions": [{"id": "S1", "harness": "claude", "harness_session_id": "hs1"}]}),
            ),
        )
    }
    fn created(mode: &str) -> Step {
        let status = if mode == "wait" { "open" } else { "released" };
        (
            "POST",
            "/api/sessions/S1/questions".into(),
            Ok(
                json!({"question": {"id": Q, "status": status}, "mode": mode, "terminal_after_s": 600}),
            ),
        )
    }
    fn polled(wait: &str, status: &str, answers: Value) -> Step {
        (
            "GET",
            format!("/api/sessions/S1/questions/{Q}?wait={wait}"),
            Ok(
                json!({"question": {"id": Q, "status": status, "answers": answers,
            "questions": [{"question": "Which?", "header": "Pick", "options": [{"label": "A", "preview": "A!"}, {"label": "B"}], "multi_select": false, "other": true}]}}),
            ),
        )
    }
    fn released(ok: bool) -> Step {
        (
            "POST",
            format!("/api/sessions/S1/questions/{Q}/release"),
            if ok {
                Ok(json!({}))
            } else {
                Err(anyhow::anyhow!("question_closed: closed"))
            },
        )
    }

    #[test]
    fn answered_in_clax_allows_with_the_answers() {
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled(
                "600",
                "answered",
                json!([{"selected": ["A"], "text": null}]),
            ),
        ]);
        let (out, o) = ask(&input(), &d, Budget::default());
        assert_eq!(o, Outcome::Answered);
        let v = out.value().unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
        let ui = &v["hookSpecificOutput"]["updatedInput"];
        assert_eq!(
            ui["questions"],
            input().rest["tool_input"]["questions"],
            "the original questions are echoed"
        );
        assert_eq!(ui["answers"]["Which?"], "A");
        assert_eq!(ui["annotations"]["Which?"]["preview"], "A!");
    }

    #[test]
    fn an_answer_without_a_preview_has_no_annotations() {
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled(
                "600",
                "answered",
                json!([{"selected": ["B"], "text": "and C"}]),
            ),
        ]);
        let (out, _) = ask(&input(), &d, Budget::default());
        let ui = &out.value().unwrap()["hookSpecificOutput"]["updatedInput"];
        assert_eq!(ui["answers"]["Which?"], "B, and C");
        assert!(ui.get("annotations").is_none());
    }

    #[test]
    fn declined_denies_with_a_reason() {
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "declined", Value::Null),
        ]);
        let (out, o) = ask(&input(), &d, Budget::default());
        assert_eq!(o, Outcome::Declined);
        let v = out.value().unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            v["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("chose not to answer")
        );
    }

    #[test]
    fn released_or_still_open_prints_nothing() {
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "released", Value::Null),
        ]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Released)
        );
        // Withdrawn while the hook waited: its session ended, or the daemon
        // shut down. Nothing is released.
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "withdrawn", Value::Null),
        ]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Released)
        );
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "open", Value::Null),
            released(true),
        ]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Timeout)
        );
        assert!(
            d.seen.borrow().iter().any(|s| s.ends_with("/release")),
            "the timer releases it"
        );
    }

    #[test]
    fn an_answer_that_beats_the_timers_release_is_still_taken() {
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "open", Value::Null),
            released(false),
            polled("0", "answered", json!([{"selected": ["A"], "text": null}])),
        ]);
        let (out, o) = ask(&input(), &d, Budget::default());
        assert_eq!(o, Outcome::Answered);
        assert_eq!(
            out.value().unwrap()["hookSpecificOutput"]["updatedInput"]["answers"]["Which?"],
            "A"
        );
    }

    #[test]
    fn a_release_that_fails_with_the_question_open_is_retried_once() {
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "open", Value::Null),
            released(false),
            polled("0", "open", Value::Null),
            released(true),
        ]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Timeout)
        );
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "open", Value::Null),
            released(false),
            polled("0", "open", Value::Null),
            released(false),
        ]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Error)
        );
    }

    #[test]
    fn terminal_mode_prints_nothing_and_returns_at_once() {
        let d = Fake::new(vec![sessions(), created("terminal")]);
        let a = ask_logged(&input(), &d, Budget::default());
        assert_eq!(
            (a.out, a.outcome, a.mode),
            (HookOutput::none(), Outcome::Terminal, Some("terminal"))
        );
        assert_eq!(d.seen.borrow().len(), 2, "no poll");
    }

    #[test]
    fn fails_open() {
        let d = Fake::new(vec![(
            "GET",
            "/api/sessions".into(),
            Err(anyhow::anyhow!("down")),
        )]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Error)
        );
        let d = Fake::new(vec![]);
        let other = HookInput::parse(
            &json!({"session_id": "hs1", "tool_name": "Bash", "tool_input": {}}).to_string(),
        );
        assert_eq!(
            ask(&other, &d, Budget::default()),
            (HookOutput::none(), Outcome::Skipped)
        );
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            (
                "GET",
                format!("/api/sessions/S1/questions/{Q}"),
                Err(anyhow::anyhow!("reset")),
            ),
        ]);
        let a = ask_logged(&input(), &d, Budget::default());
        assert_eq!((a.outcome, a.mode), (Outcome::Error, Some("wait")));
        let d = Fake::new(vec![(
            "GET",
            "/api/sessions".into(),
            Ok(json!({"sessions": []})),
        )]);
        assert_eq!(
            ask(&input(), &d, Budget::default()),
            (HookOutput::none(), Outcome::Error),
            "no live session"
        );
        // A daemon that answers with the wrong answer count or no question ID.
        let d = Fake::new(vec![
            sessions(),
            created("wait"),
            polled("600", "answered", json!([])),
        ]);
        assert_eq!(ask(&input(), &d, Budget::default()).1, Outcome::Error);
        let d = Fake::new(vec![
            sessions(),
            (
                "POST",
                "/api/sessions/S1/questions".into(),
                Ok(json!({"question": {"id": "../x"}, "mode": "wait"})),
            ),
        ]);
        assert_eq!(ask(&input(), &d, Budget::default()).1, Outcome::Error);
    }

    #[test]
    fn asked_records_the_terminal_answer() {
        let post = HookInput::parse(
            &json!({"session_id": "hs1", "tool_name": "AskUserQuestion", "tool_use_id": "toolu_1",
            "tool_response": {"answers": {"Which?": "B"}}})
            .to_string(),
        );
        let d = Fake::new(vec![
            sessions(),
            (
                "POST",
                "/api/sessions/S1/questions:terminal".into(),
                Ok(json!({})),
            ),
        ]);
        assert_eq!(asked(&post, &d).unwrap(), HookOutput::none());
        assert_eq!(d.seen.borrow().len(), 2);
        // Another tool, or no answers: nothing is asked of the daemon.
        let d = Fake::new(vec![]);
        let bash = HookInput::parse(&json!({"session_id": "hs1", "tool_name": "Bash"}).to_string());
        assert_eq!(asked(&bash, &d).unwrap(), HookOutput::none());
        let none = HookInput::parse(&json!({"session_id": "hs1", "tool_name": "AskUserQuestion", "tool_use_id": "t", "tool_response": {}}).to_string());
        assert_eq!(asked(&none, &d).unwrap(), HookOutput::none());
    }
}

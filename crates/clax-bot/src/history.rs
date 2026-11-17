//! What the run loop itself reads of a run's committed steps: two folds
//! over `&[Step]`, each one pass, each reading only the payload keys it
//! names.
//!
//! - [`Control`] is the state the tick decides by: which step is next
//!   and with what, whether the model was already nudged, how many steps
//!   are committed, what the last turn used.
//! - [`transcript`] is what a model is shown of the run so far: the
//!   system text and the messages, as the steps stored them.
//!
//! - [`check`] says whether a transcript keeps what a turn relies on,
//!   for a rule that stands in for [`transcript`]; [`check_shortened`]
//!   is the same for one shortened after a full context window, and
//!   reads a turn's `next.overflow`.
//! - [`committed_and_open`] splits the steps a tick read into the
//!   committed ones and the open one.
//!
//! Neither fold reads what a surface keeps in a step's payload. What
//! Clax's own surface folds from the same steps is
//! [`crate::clax::Tally`], which this module does not import. Two tests
//! hold that: `the_run_loops_folds_read_nothing_clax_keeps_in_a_payload`
//! (`src/history_fixtures.rs`) gives both folds histories with and
//! without the payload fields of Clax's surface (`changed`, `replied`,
//! `published`, `created`, `base`) and fails when a result differs; and
//! `this_file_imports_a_step_and_messages_and_nothing_of_the_crate`
//! here reads this file's source and fails on any `use` but the three
//! below, and on a `crate::` or `super::` path in its code.
//!
//! # Payload keys read
//!
//! | Step | [`Control`] | [`transcript`] |
//! |---|---|---|
//! | `turn` | `next` (whole), `next.nudge`, `usage` | `system`, `user`, `assistant`, `next.kind`, `next.user` |
//! | `script` | none: the next step is a turn | `result` |
//! | `finish`, `give_up`, `stopped`, `failed` | none: the run has ended | none |
//!
//! A step that is not committed (`committed_at` is `None`) is read by
//! neither. A step's attempt number is read by neither: a step committed
//! on its third attempt adds what one committed on its first does.
//!
//! # Cost
//!
//! A tick reads the run's steps from the store once
//! (`Store::run_with_steps`) and keeps them (`Tick::steps`). [`Control`]
//! is folded on every tick and clones one `next` object, the last
//! turn's. [`transcript`] is folded only by a tick that takes a turn,
//! and clones every stored block once, into the request.

use clax_sample::messages::{Block, Message, Role, Usage};
use clax_store::bot_runs::{Step, StepKind};
use serde_json::{Value, json};

/// The payload of `s`, `null` for a step without one.
fn payload(s: &Step) -> &Value {
    s.payload.as_ref().unwrap_or(&Value::Null)
}

/// The committed steps of `steps`, in order.
fn committed(steps: &[Step]) -> impl Iterator<Item = &Step> {
    steps.iter().filter(|s| s.committed_at.is_some())
}

/// `steps` as a tick read them, split: the committed steps, in order,
/// and the first step that is not committed (the one begun and left
/// open), when there is one. Any other step that is not committed is in
/// neither.
pub(crate) fn committed_and_open(mut steps: Vec<Step>) -> (Vec<Step>, Option<Step>) {
    let open = steps
        .iter()
        .position(|s| s.committed_at.is_none())
        .map(|i| steps.remove(i));
    steps.retain(|s| s.committed_at.is_some());
    (steps, open)
}

/// The blocks a payload keeps as the JSON array `v`; none for anything
/// else.
fn blocks(v: &Value) -> Vec<Block> {
    v.as_array()
        .map(|a| a.iter().cloned().map(Block::from_json).collect())
        .unwrap_or_default()
}

/// Adds `content` to `messages` as `role`'s: to the last message when it
/// is that role's, else as a new one. Empty content adds nothing.
pub(crate) fn push(messages: &mut Vec<Message>, role: Role, content: Vec<Block>) {
    if content.is_empty() {
        return;
    }
    match messages.last_mut() {
        Some(m) if m.role == role => m.content.extend(content),
        _ => messages.push(Message { role, content }),
    }
}

/// The token counts the JSON object `v` keeps (a turn's `usage`, an
/// observed entry); 0 for a count it lacks.
pub(crate) fn usage_of(v: &Value) -> Usage {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    Usage {
        input_tokens: n("input_tokens"),
        cache_creation_input_tokens: n("cache_creation_input_tokens"),
        cache_read_input_tokens: n("cache_read_input_tokens"),
        output_tokens: n("output_tokens"),
        billed_micro_usd: v["billed_micro_usd"].as_u64(),
    }
}

/// What the tick decides by, folded from the committed steps.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Control {
    /// The step to take now, as the last committed step left it: the
    /// last turn's `next` (`{kind: "turn" | "script" | "finish" |
    /// "give_up", …}`), `{kind: "turn"}` before any step and after a
    /// script, `{kind: "end"}` after a terminal step.
    pub next: Value,
    /// The last turn's next step was the nudge: a turn that again calls
    /// no tool ends the run.
    pub nudged: bool,
    /// How many steps are committed.
    pub committed: usize,
    /// The last turn's usage.
    pub last_usage: Option<Usage>,
}

impl Control {
    /// The control state `steps` add up to, in one pass.
    pub(crate) fn of(steps: &[Step]) -> Control {
        let mut c = Control {
            next: Value::Null,
            nudged: false,
            committed: 0,
            last_usage: None,
        };
        // The last turn's `next` as stored, or the kind of step that
        // follows a step that stores none.
        let mut next: Result<&Value, &str> = Err("turn");
        for s in committed(steps) {
            c.committed += 1;
            match s.kind {
                StepKind::Turn => {
                    let p = payload(s);
                    c.nudged = p["next"]["nudge"].as_bool().unwrap_or(false);
                    c.last_usage = Some(usage_of(&p["usage"]));
                    next = Ok(&p["next"]);
                }
                StepKind::Script => next = Err("turn"),
                _ => next = Err("end"),
            }
        }
        c.next = match next {
            Ok(stored) => stored.clone(),
            Err(kind) => json!({"kind": kind}),
        };
        c
    }
}

/// What a model is shown of a run so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// The system text the last committed turn that stored one stored:
    /// the first turn's, on every run a tick wrote, since only a first
    /// turn stores one. `None` until such a turn is committed, and the
    /// tick then writes the system text and the request message itself.
    pub system: Option<String>,
    /// The messages, oldest first, roles alternating.
    pub messages: Vec<Message>,
}

/// Every committed step of `steps` as the model is shown it, in one pass:
/// the first turn's `system` and `user`, each turn's `assistant` blocks
/// as stored, what the run loop answered a turn with (`next.user`, when
/// the next step is a turn), and each script's stored `result`.
/// Consecutive contents of one role are one message. `system` is the
/// string the last committed turn that stored one stored, which is step
/// 1's on every run a tick wrote.
///
/// It is a pure function of the steps, with the signature a policy's
/// history rule has to fill (`fn(&[Step]) -> Transcript`): a rule that
/// sends fewer steps or a summary stands where this is called, in the
/// turn. Whatever stands there must keep what the turn relies on. With
/// S the committed steps and T what the rule returns:
///
/// 1. **The system text.** `T.system` is `Some` exactly when a committed
///    turn of S stored a string `system`, and is then that text, byte
///    for byte. The turn takes `None` to mean the run's first request,
///    and then writes the system text and the request message again and
///    stores both in the step it commits.
/// 2. **The request.** When `T.system` is `Some`, `T.messages` is not
///    empty, its first message is the user's, and that message opens
///    with step 1's stored `user` blocks, unchanged and in order: with
///    the system text they are the cached prefix.
/// 3. **Roles.** Roles alternate, starting with the user's, and no
///    message is empty.
/// 4. **The last message.** When the step to take is a turn, the last
///    message is the user's; except after a paused turn (its `next` is
///    a turn with no `user` blocks) that stored `assistant` blocks,
///    where it is the assistant's and ends with those blocks, which the
///    model continues. After a turn that stored none and asked for a
///    turn with no `user` blocks (a request that was refused), it is
///    the user's.
/// 5. **Pairs.** Every `tool_use` block of an assistant message has a
///    `tool_result` with its ID in the user message right after it,
///    and every `tool_result` answers a `tool_use` of the assistant
///    message right before it. A `tool_use` in the last message is one
///    the next step answers.
/// 6. **Stored blocks as stored.** Every block is one a committed step
///    stores (in `user`, `assistant`, `next.user` when `next.kind` is
///    `turn`, or `result`), compared as JSON, in step order and in
///    stored order within a step; or a block the rule adds for the
///    steps it left out. A step's payload, the journal and the audit
///    are what happened and are not written.
/// 7. **No breakpoint.** No block carries `cache_control`: the turn
///    sets the one cache breakpoint, on the last block.
///
/// [`check`] says whether a transcript keeps them. This function does,
/// on every run a tick wrote.
///
/// The turn adds to what a rule returns: the request message on a first
/// turn, and the cache breakpoint.
#[must_use]
pub fn transcript(steps: &[Step]) -> Transcript {
    let mut t = Transcript::default();
    for s in committed(steps) {
        let p = payload(s);
        match s.kind {
            StepKind::Turn => {
                if let Some(sys) = p["system"].as_str() {
                    t.system = Some(sys.to_string());
                }
                push(&mut t.messages, Role::User, blocks(&p["user"]));
                push(&mut t.messages, Role::Assistant, blocks(&p["assistant"]));
                if p["next"]["kind"] == "turn" {
                    push(&mut t.messages, Role::User, blocks(&p["next"]["user"]));
                }
            }
            StepKind::Script => push(
                &mut t.messages,
                Role::User,
                vec![Block::from_json(p["result"].clone())],
            ),
            _ => {}
        }
    }
    t
}

/// The ID a block of kind `kind` (`tool_use`, `tool_result`) keeps
/// under `key`, when the JSON `block` is one.
fn id_of<'a>(block: &'a Value, kind: &str, key: &str) -> Option<&'a Value> {
    (block["type"] == kind).then(|| &block[key])
}

/// Whether `shown` keeps, for the steps `steps`, what a turn relies on:
/// the invariants [`transcript`]'s documentation numbers, for a rule
/// that stands in its place. `Err` names the first one broken and where.
///
/// Invariants 1 to 5 and 7 are checked whole. Of 6, what can be told
/// without knowing which blocks the rule added: a block that a committed
/// step stores is not shown before one stored earlier that is shown too.
/// A block no step stores is taken for one the rule added, so a stored
/// block that was rewritten is not found.
///
/// 4 and 5 are of a run a tick wrote: on steps that are not one (a
/// script whose turn was left out, say) they can fail for the steps'
/// sake and not the rule's.
///
/// # Errors
/// The first invariant `shown` breaks.
pub fn check(steps: &[Step], shown: &Transcript) -> Result<(), String> {
    check_with(steps, shown, false)
}

/// [`check`], for a transcript that leaves steps out because the model's
/// context window was full. It differs in 4 alone: when the last
/// committed turn's `next` records an `overflow` (the turn's answer was
/// cut off by a full window, or its request was refused as too long),
/// the last message may be the user's, with that turn's stored
/// `assistant` blocks left out. An answer the window cut off is the one
/// thing such a transcript may drop from the end; everything else holds
/// as [`check`] holds it, the request at the front (2) included.
///
/// # Errors
/// The first invariant `shown` breaks.
pub fn check_shortened(steps: &[Step], shown: &Transcript) -> Result<(), String> {
    check_with(steps, shown, true)
}

/// [`check`]; with `shortened`, [`check_shortened`].
fn check_with(steps: &[Step], shown: &Transcript, shortened: bool) -> Result<(), String> {
    let done: Vec<&Step> = committed(steps).collect();
    let array = |v: &'_ Value| -> Vec<Value> { v.as_array().cloned().unwrap_or_default() };

    // 1: the system text.
    let stored = done
        .iter()
        .filter(|s| s.kind == StepKind::Turn)
        .filter_map(|s| payload(s)["system"].as_str())
        .next_back();
    if shown.system.as_deref() != stored {
        return Err(format!(
            "1: the system text is {}, and the steps store {}",
            shown
                .system
                .as_ref()
                .map_or("none".into(), |s| format!("{} bytes", s.len())),
            stored.map_or("none".into(), |s| format!("{} bytes", s.len())),
        ));
    }

    let messages: Vec<(Role, Vec<Value>)> = shown
        .messages
        .iter()
        .map(|m| (m.role, m.content.iter().map(Block::to_json).collect()))
        .collect();

    // 3: roles alternate from the user's, and no message is empty.
    for (i, (role, content)) in messages.iter().enumerate() {
        if content.is_empty() {
            return Err(format!("3: message {i} is empty"));
        }
        let want = if i % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        };
        if *role != want {
            return Err(format!("3: message {i} is {role:?}'s, not {want:?}'s"));
        }
    }

    // 2: the request opens the first message.
    if shown.system.is_some() {
        let Some((_, first)) = messages.first() else {
            return Err("2: a system text and no message".into());
        };
        let request = done
            .first()
            .filter(|s| s.kind == StepKind::Turn)
            .map(|s| array(&payload(s)["user"]))
            .unwrap_or_default();
        if !first.starts_with(&request) {
            return Err("2: the first message does not open with step 1's `user` blocks".into());
        }
    }

    // 4: the last message, when the step to take is a turn.
    if let (Some(last), Some((role, content))) = (done.last(), messages.last()) {
        let p = payload(last);
        let answered = match last.kind {
            StepKind::Script => Some(true),
            StepKind::Turn if p["next"]["kind"] == "turn" => {
                Some(!array(&p["next"]["user"]).is_empty())
            }
            _ => None,
        };
        match answered {
            Some(true) if *role != Role::User => {
                return Err("4: the last message is not the user's".into());
            }
            Some(false) => {
                let said = array(&p["assistant"]);
                let ends_with_it = *role == Role::Assistant && content.ends_with(&said);
                let left_out =
                    shortened && p["next"]["overflow"].is_object() && *role == Role::User;
                if said.is_empty() {
                    // Nothing was said to go on from: the turn answers
                    // the user.
                    if *role != Role::User {
                        return Err(
                            "4: after a turn that said nothing, the last message is not the user's"
                                .into(),
                        );
                    }
                } else if !ends_with_it && !left_out {
                    return Err(
                        "4: after a paused turn, the last message does not end with its blocks"
                            .into(),
                    );
                }
            }
            _ => {}
        }
    }

    // 5: a tool_use and its tool_result are kept together.
    let ids = |content: &'_ [Value], kind: &str, key: &str| -> Vec<Value> {
        content
            .iter()
            .filter_map(|b| id_of(b, kind, key).cloned())
            .collect()
    };
    for (i, (role, content)) in messages.iter().enumerate() {
        match role {
            Role::Assistant => {
                let Some((_, after)) = messages.get(i + 1) else {
                    continue;
                };
                let results = ids(after, "tool_result", "tool_use_id");
                if let Some(id) = ids(content, "tool_use", "id")
                    .iter()
                    .find(|id| !results.contains(id))
                {
                    return Err(format!(
                        "5: the tool_use {id} of message {i} has no tool_result in message {}",
                        i + 1
                    ));
                }
            }
            // Refused by 3.
            Role::System => {}
            Role::User => {
                let uses = match i.checked_sub(1).and_then(|j| messages.get(j)) {
                    Some((_, before)) => ids(before, "tool_use", "id"),
                    None => Vec::new(),
                };
                if let Some(id) = ids(content, "tool_result", "tool_use_id")
                    .iter()
                    .find(|id| !uses.contains(id))
                {
                    return Err(format!(
                        "5: the tool_result for {id} in message {i} answers no tool_use before it"
                    ));
                }
            }
        }
    }

    // 6: stored blocks in the order stored.
    let mut kept: Vec<Value> = Vec::new();
    for s in &done {
        let p = payload(s);
        match s.kind {
            StepKind::Turn => {
                kept.extend(array(&p["user"]));
                kept.extend(array(&p["assistant"]));
                if p["next"]["kind"] == "turn" {
                    kept.extend(array(&p["next"]["user"]));
                }
            }
            StepKind::Script => kept.push(p["result"].clone()),
            _ => {}
        }
    }
    let mut at = 0;
    for (i, (_, content)) in messages.iter().enumerate() {
        for block in content {
            match kept[at..].iter().position(|k| k == block) {
                Some(ahead) => at += ahead + 1,
                None if kept[..at].contains(block) => {
                    return Err(format!(
                        "6: a block of message {i} is shown after one its step stores it before"
                    ));
                }
                None => {}
            }
        }
    }

    // 7: no cache breakpoint.
    for (i, (_, content)) in messages.iter().enumerate() {
        if content.iter().any(|b| !b["cache_control"].is_null()) {
            return Err(format!("7: a block of message {i} carries cache_control"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_fixtures::{a_run, open, step};

    fn text(s: &str) -> Value {
        Block::text(s).to_json()
    }

    /// The file's code, without its tests: each `use` item as written,
    /// with its white space squeezed, and every other line that is not a
    /// comment.
    fn uses_and_code(code: &str) -> (Vec<String>, Vec<&str>) {
        let (mut uses, mut rest) = (Vec::new(), Vec::new());
        let mut open: Option<String> = None;
        for line in code.lines() {
            let t = line.trim();
            if t.starts_with("//") || t.is_empty() {
                continue;
            }
            let begins = ["use ", "pub use ", "pub(crate) use ", "extern crate "]
                .iter()
                .any(|k| t.starts_with(k));
            match open.take() {
                Some(mut item) => {
                    item.push(' ');
                    item.push_str(t);
                    if t.ends_with(';') {
                        uses.push(item);
                    } else {
                        open = Some(item);
                    }
                }
                None if begins && t.ends_with(';') => uses.push(t.to_string()),
                None if begins => open = Some(t.to_string()),
                None => rest.push(line),
            }
        }
        assert!(open.is_none(), "a `use` that does not end");
        (uses, rest)
    }

    #[test]
    fn this_file_imports_a_step_and_messages_and_nothing_of_the_crate() {
        let source = include_str!("history.rs");
        let (code, _) = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("this file's tests follow its code");
        let (uses, rest) = uses_and_code(code);
        assert_eq!(
            uses,
            [
                "use clax_sample::messages::{Block, Message, Role, Usage};",
                "use clax_store::bot_runs::{Step, StepKind};",
                "use serde_json::{Value, json};",
            ],
            "src/history.rs imports only these: nothing of this crate, and of the store a step"
        );
        for line in rest {
            for path in ["crate::", "super::", "clax_store", "mod ", "include!"] {
                assert!(
                    !line.contains(path),
                    "src/history.rs reaches outside its imports with `{path}`: {line}"
                );
            }
        }
    }

    #[test]
    fn the_scan_of_imports_sees_grouped_and_relative_paths() {
        let (uses, rest) = uses_and_code(
            "use crate::{clax as x};\n// use crate::run;\nuse super::clax::*;\npub(crate) use a::{\n    b,\n    c,\n};\nfn f() { crate::clax::Tally::of(&[]); }\n",
        );
        assert_eq!(
            uses,
            [
                "use crate::{clax as x};",
                "use super::clax::*;",
                "pub(crate) use a::{ b, c, };",
            ]
        );
        assert_eq!(rest, ["fn f() { crate::clax::Tally::of(&[]); }"]);
    }

    #[test]
    fn the_open_step_is_taken_out_and_only_committed_steps_are_kept() {
        let numbers = |steps: &[Step]| steps.iter().map(|s| s.n).collect::<Vec<_>>();
        // No open step.
        let (steps, none) = committed_and_open(a_run());
        assert_eq!(numbers(&steps), [1, 2, 3, 4, 5]);
        assert_eq!(none, None);
        // The open step last, as a run has it.
        let mut run = a_run()[..3].to_vec();
        run.push(open(4, StepKind::Turn, 2));
        let (steps, taken) = committed_and_open(run);
        assert_eq!(numbers(&steps), [1, 2, 3]);
        assert_eq!(taken, Some(open(4, StepKind::Turn, 2)));
        // An open step that is not last: the committed steps keep their
        // order around it.
        let mut run = a_run();
        run.insert(1, open(9, StepKind::Script, 1));
        let (steps, taken) = committed_and_open(run);
        assert_eq!(numbers(&steps), [1, 2, 3, 4, 5]);
        assert_eq!(taken.map(|s| s.n), Some(9));
        // Two open steps: the first is the open one, the second is in
        // neither.
        let mut run = a_run()[..2].to_vec();
        run.insert(1, open(7, StepKind::Turn, 1));
        run.push(open(8, StepKind::Script, 3));
        let (steps, taken) = committed_and_open(run);
        assert_eq!(numbers(&steps), [1, 2]);
        assert!(steps.iter().all(|s| s.committed_at.is_some()));
        assert_eq!(taken, Some(open(7, StepKind::Turn, 1)));
        // Nothing at all.
        assert_eq!(committed_and_open(Vec::new()), (Vec::new(), None));
    }

    #[test]
    fn before_any_step_the_next_is_a_turn() {
        let c = Control::of(&[]);
        assert_eq!(
            c,
            Control {
                next: json!({"kind": "turn"}),
                nudged: false,
                committed: 0,
                last_usage: None,
            }
        );
        assert_eq!(transcript(&[]), Transcript::default());
        assert_eq!(check(&[], &Transcript::default()), Ok(()));
    }

    #[test]
    fn control_follows_every_kind_of_step() {
        let run = a_run();
        let next = |n: usize| Control::of(&run[..n]);
        // Step 1, a turn that asked for a script.
        let c = next(1);
        assert_eq!(c.next["kind"], "script");
        assert_eq!(c.next["tool_use_id"], "tu_1");
        assert_eq!(c.committed, 1);
        assert!(!c.nudged);
        assert_eq!(
            c.last_usage,
            Some(Usage {
                input_tokens: 1000,
                cache_creation_input_tokens: 500,
                cache_read_input_tokens: 2000,
                output_tokens: 300,
                billed_micro_usd: None,
            })
        );
        // Step 2, the script: a turn is next, and the turn's facts stand.
        let c = next(2);
        assert_eq!(c.next, json!({"kind": "turn"}));
        assert_eq!(c.committed, 2);
        assert_eq!(c.last_usage.unwrap().input_tokens, 1000);
        // Step 3, a turn that called no tool: nudged, with what to say.
        let c = next(3);
        assert!(c.nudged);
        assert_eq!(c.next["kind"], "turn");
        assert_eq!(c.next["user"], json!([text("nudge")]));
        assert_eq!(
            c.last_usage,
            Some(Usage {
                input_tokens: 7,
                billed_micro_usd: Some(42),
                ..Usage::default()
            })
        );
        // Step 4, a turn committed on its third attempt, whose context
        // window was full: the nudge is over and a give-up is next.
        let c = next(4);
        assert!(!c.nudged);
        assert_eq!(
            c.next,
            json!({"kind": "give_up", "code": "context_window", "reason": "full"})
        );
        assert_eq!(c.committed, 4);
        assert_eq!(c.last_usage, Some(Usage::default()), "a turn without usage");
        // Step 5, the give-up: the run has ended.
        let c = next(5);
        assert_eq!(c.next, json!({"kind": "end"}));
        assert_eq!(c.committed, 5);
        assert!(!c.nudged);
    }

    #[test]
    fn committed_counts_the_committed_steps_and_not_their_numbers() {
        let run = a_run();
        let mut numbered = run.clone();
        for (s, n) in numbered.iter_mut().zip([3, 9, 10, 20, 41]) {
            s.n = n;
        }
        assert_eq!(Control::of(&numbered), Control::of(&run));
        assert_eq!(Control::of(&numbered[..2]).committed, 2);
    }

    #[test]
    fn only_a_turn_says_whether_the_model_was_nudged() {
        // A nudged turn, then a script: the script leaves the nudge as
        // the turn left it.
        let nudged = a_run()[2].clone();
        let script = step(4, StepKind::Script, json!({"result": text("r")}));
        assert!(Control::of(&[nudged.clone(), script.clone()]).nudged);
        // A turn that was not nudged, then the same script: not nudged.
        let plain = a_run()[0].clone();
        assert!(!Control::of(&[plain, script]).nudged);
    }

    #[test]
    fn every_terminal_step_ends_the_run() {
        for (kind, payload) in [
            (StepKind::Finish, json!({"summary": "done", "posted": true})),
            (StepKind::GiveUp, json!({"code": "model", "reason": "no"})),
            (
                StepKind::Stopped,
                json!({"reason": "run_cap", "notice": "n"}),
            ),
            (StepKind::Failed, json!({"reason": "internal_error"})),
            // A cancelled script's stopped step, with the script's parts.
            (
                StepKind::Stopped,
                json!({"reason": "cancelled", "notice": null, "title": "t", "code": "c",
                       "next": {"kind": "turn", "nudge": true}, "usage": {"input_tokens": 9}}),
            ),
        ] {
            let mut run = a_run()[..2].to_vec();
            run.push(step(3, kind, payload));
            let c = Control::of(&run);
            assert_eq!(c.next, json!({"kind": "end"}), "{kind:?}");
            assert_eq!(c.committed, 3);
            assert!(!c.nudged, "{kind:?}: only a turn's `next` is read");
            assert_eq!(c.last_usage.unwrap().input_tokens, 1000, "{kind:?}");
            assert_eq!(
                transcript(&run),
                transcript(&run[..2]),
                "{kind:?} adds no message"
            );
        }
    }

    #[test]
    fn a_step_that_is_not_committed_is_not_read() {
        let mut run = a_run()[..2].to_vec();
        let before = (Control::of(&run), transcript(&run));
        run.push(open(3, StepKind::Turn, 2));
        assert_eq!((Control::of(&run), transcript(&run)), before);
        // Nor is one that somehow kept a payload.
        let mut kept = open(3, StepKind::Turn, 1);
        kept.payload = Some(json!({"next": {"kind": "finish"}, "assistant": [text("x")]}));
        run[2] = kept;
        assert_eq!((Control::of(&run), transcript(&run)), before);
    }

    #[test]
    fn an_attempt_number_changes_nothing() {
        let run = a_run();
        let mut again = run.clone();
        for s in &mut again {
            s.attempt += 2;
        }
        assert_eq!(Control::of(&again), Control::of(&run));
        assert_eq!(transcript(&again), transcript(&run));
    }

    #[test]
    fn a_turn_without_a_payload_leaves_no_next_step() {
        let mut bare = step(1, StepKind::Turn, Value::Null);
        bare.payload = None;
        let c = Control::of(&[bare.clone()]);
        assert_eq!(c.next, Value::Null);
        assert_eq!(c.last_usage, Some(Usage::default()));
        assert_eq!(transcript(&[bare]), Transcript::default());
    }

    #[test]
    fn the_transcript_is_every_step_as_stored_in_order() {
        let run = a_run();
        let t = transcript(&run);
        assert_eq!(t.system.as_deref(), Some("the system text"));
        let roles: Vec<Role> = t.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant
            ]
        );
        let json = |m: &Message| m.content.iter().map(Block::to_json).collect::<Vec<_>>();
        // Step 1: the request, then the assistant's blocks as stored.
        assert_eq!(json(&t.messages[0]), [text("the request")]);
        let stored = run[0].payload.as_ref().unwrap()["assistant"].clone();
        assert_eq!(Value::Array(json(&t.messages[1])), stored);
        // Step 2: the script's result answers the turn's tool call.
        let result = run[1].payload.as_ref().unwrap()["result"].clone();
        assert_eq!(result["tool_use_id"], stored[1]["id"]);
        assert_eq!(json(&t.messages[2]), [result]);
        // Step 3: the turn that called no tool, and the nudge it is
        // answered with.
        assert_eq!(json(&t.messages[3]), [text("I am done.")]);
        assert_eq!(json(&t.messages[4]), [text("nudge")]);
        // Step 4: its `next` is a give-up, so nothing answers it; step 5
        // adds nothing.
        assert_eq!(json(&t.messages[5]), [text("too long")]);
    }

    #[test]
    fn a_turns_answer_is_shown_only_when_a_turn_is_next() {
        let turn = |next: Value| {
            step(
                1,
                StepKind::Turn,
                json!({"system": "s", "user": [text("u")], "assistant": [text("a")], "next": next}),
            )
        };
        let shown = |next: Value| transcript(&[turn(next)]).messages.len();
        assert_eq!(shown(json!({"kind": "turn", "user": [text("more")]})), 3);
        assert_eq!(shown(json!({"kind": "turn", "user": []})), 2, "pause_turn");
        assert_eq!(shown(json!({"kind": "script", "user": [text("no")]})), 2);
        assert_eq!(shown(json!({"kind": "finish", "user": [text("no")]})), 2);
    }

    #[test]
    fn consecutive_contents_of_one_role_are_one_message() {
        // A paused turn is answered with nothing, so the next turn's
        // blocks join its own.
        let run = [
            step(
                1,
                StepKind::Turn,
                json!({"system": "s", "user": [text("u")], "assistant": [text("a")],
                       "next": {"kind": "turn", "user": []}}),
            ),
            step(
                2,
                StepKind::Turn,
                json!({"assistant": [text("b")], "next": {"kind": "finish", "summary": ""}}),
            ),
        ];
        let t = transcript(&run);
        assert_eq!(t.messages.len(), 2);
        assert_eq!(t.messages[1].content.len(), 2);
    }

    #[test]
    fn a_later_turns_system_text_replaces_the_first() {
        let run = [
            step(1, StepKind::Turn, json!({"system": "one"})),
            step(2, StepKind::Turn, json!({"system": 7})),
            step(3, StepKind::Turn, json!({"system": "three"})),
        ];
        assert_eq!(transcript(&run[..2]).system.as_deref(), Some("one"));
        assert_eq!(transcript(&run).system.as_deref(), Some("three"));
    }

    #[test]
    fn a_script_without_a_result_still_answers() {
        let run = [step(1, StepKind::Script, json!({"title": "t"}))];
        let t = transcript(&run);
        assert_eq!(t.messages.len(), 1);
        assert_eq!(t.messages[0].role, Role::User);
        assert_eq!(t.messages[0].content[0].to_json(), Value::Null);
    }

    #[test]
    fn usage_reads_each_count_and_the_billed_cost() {
        let u = usage_of(&json!({
            "input_tokens": 1, "cache_creation_input_tokens": 2,
            "cache_read_input_tokens": 3, "output_tokens": 4, "billed_micro_usd": 5,
        }));
        assert_eq!(
            u,
            Usage {
                input_tokens: 1,
                cache_creation_input_tokens: 2,
                cache_read_input_tokens: 3,
                output_tokens: 4,
                billed_micro_usd: Some(5),
            }
        );
        // A billed cost of 0 is a cost, not the lack of one.
        let free = usage_of(&json!({"input_tokens": 1, "billed_micro_usd": 0}));
        assert_eq!(free.billed_micro_usd, Some(0));
        assert_eq!(usage_of(&json!({"input_tokens": 1})).billed_micro_usd, None);
        // A count that is not a number is 0, and so is all of a value
        // that is no object.
        let odd = usage_of(&json!({"input_tokens": "9", "output_tokens": -1,
                                   "cache_read_input_tokens": 2.5, "billed_micro_usd": "0"}));
        assert_eq!(odd, Usage::default());
        assert_eq!(usage_of(&json!([1, 2])), Usage::default());
    }

    /// The first three steps of [`a_run`] (a turn, its script, a turn
    /// that is nudged) and their transcript, which keeps every invariant.
    fn shown() -> (Vec<Step>, Transcript) {
        let run = a_run()[..3].to_vec();
        let t = transcript(&run);
        assert_eq!(check(&run, &t), Ok(()));
        assert_eq!(t.messages.len(), 5);
        (run, t)
    }

    /// What [`check`] says of `t` for `run`: the number of the invariant
    /// broken, or 0.
    fn broken(run: &[Step], t: &Transcript) -> u32 {
        match check(run, t) {
            Ok(()) => 0,
            Err(e) => e.split(':').next().unwrap().parse().expect("a number"),
        }
    }

    #[test]
    fn check_holds_the_system_text() {
        let (run, t) = shown();
        let none = Transcript {
            system: None,
            ..t.clone()
        };
        assert_eq!(broken(&run, &none), 1);
        let other = Transcript {
            system: Some("the system text ".into()),
            ..t.clone()
        };
        assert_eq!(broken(&run, &other), 1);
        // A system text no step stored.
        let mut bare = run.clone();
        bare[0].payload.as_mut().unwrap()["system"] = Value::Null;
        assert_eq!(broken(&bare, &t), 1);
        // The last one stored is the one.
        let mut two = run.clone();
        two[2].payload.as_mut().unwrap()["system"] = json!("later");
        assert_eq!(broken(&two, &t), 1);
        assert_eq!(broken(&two, &transcript(&two)), 0);
    }

    #[test]
    fn check_holds_the_request_at_the_front() {
        let (run, t) = shown();
        let mut led = t.clone();
        led.messages[0].content.insert(0, Block::text("a summary"));
        assert_eq!(broken(&run, &led), 2);
        let mut reworded = t.clone();
        reworded.messages[0].content[0] = Block::text("the request, reworded");
        assert_eq!(broken(&run, &reworded), 2);
        let empty = Transcript {
            messages: Vec::new(),
            ..t.clone()
        };
        assert_eq!(broken(&run, &empty), 2);
        // Blocks after the request are the rule's to add.
        let mut more = t;
        more.messages[0].content.push(Block::text("a summary"));
        assert_eq!(broken(&run, &more), 0);
    }

    #[test]
    fn check_holds_alternating_roles_and_no_empty_message() {
        let (run, t) = shown();
        let mut twice = t.clone();
        let again = twice.messages[4].clone();
        twice.messages.push(again);
        assert_eq!(broken(&run, &twice), 3);
        let mut hollow = t.clone();
        hollow.messages[3].content.clear();
        assert_eq!(broken(&run, &hollow), 3);
        let mut assistant_first = t;
        assistant_first.messages[0].role = Role::Assistant;
        assert_eq!(broken(&run, &assistant_first), 3);
    }

    #[test]
    fn check_holds_the_last_message() {
        let (run, t) = shown();
        // The nudge left out: the assistant's words are last.
        let mut cut = t.clone();
        cut.messages.pop();
        assert_eq!(broken(&run, &cut), 4);
        // After a script, its result is last.
        let mut after_script = transcript(&run[..2]);
        assert_eq!(broken(&run[..2], &after_script), 0);
        after_script.messages.pop();
        assert_eq!(broken(&run[..2], &after_script), 4);
        // After a paused turn the assistant's blocks are last, as stored.
        let mut paused = run.clone();
        paused[2].payload.as_mut().unwrap()["next"] = json!({"kind": "turn", "user": []});
        let t = transcript(&paused);
        assert_eq!(t.messages.last().unwrap().role, Role::Assistant);
        assert_eq!(broken(&paused, &t), 0);
        let mut answered = t.clone();
        answered.messages.push(Message {
            role: Role::User,
            content: vec![Block::text("go on")],
        });
        assert_eq!(broken(&paused, &answered), 4);
        let mut trailed = t;
        trailed
            .messages
            .last_mut()
            .unwrap()
            .content
            .push(Block::text("and more"));
        assert_eq!(broken(&paused, &trailed), 4);
        // An answer a full window cut off may be left out of a shortened
        // transcript, and only there, and only that answer.
        let mut cut_off = paused.clone();
        cut_off[2].payload.as_mut().unwrap()["next"]["overflow"] = json!({"level": 1});
        let whole = transcript(&cut_off);
        assert_eq!(broken(&cut_off, &whole), 0);
        assert_eq!(check_shortened(&cut_off, &whole), Ok(()));
        let mut dropped = whole.clone();
        dropped.messages.pop();
        assert_eq!(dropped.messages.last().unwrap().role, Role::User);
        assert_eq!(broken(&cut_off, &dropped), 4);
        assert_eq!(check_shortened(&cut_off, &dropped), Ok(()));
        assert!(check_shortened(&paused, &dropped).is_err());
        let mut trailed = whole;
        trailed
            .messages
            .last_mut()
            .unwrap()
            .content
            .push(Block::text("and more"));
        assert!(check_shortened(&cut_off, &trailed).is_err());
        // After a turn that stored no blocks and asked for a turn with
        // none (a refused request), the last message is the user's, in
        // either check.
        let mut silent = run.clone();
        silent[2].payload.as_mut().unwrap()["assistant"] = json!([]);
        silent[2].payload.as_mut().unwrap()["next"] =
            json!({"kind": "turn", "user": [], "overflow": {"level": 1}});
        let t = transcript(&silent);
        assert_eq!(t.messages.last().unwrap().role, Role::User);
        assert_eq!(broken(&silent, &t), 0);
        let mut unanswered = t;
        unanswered.messages.push(Message {
            role: Role::Assistant,
            content: vec![Block::text("I will look.")],
        });
        assert_eq!(broken(&silent, &unanswered), 4);
        assert!(check_shortened(&silent, &unanswered).is_err());
        // A step to take that is no turn claims nothing of the last
        // message.
        let t = transcript(&run[..1]);
        assert_eq!(t.messages.last().unwrap().role, Role::Assistant);
        assert_eq!(broken(&run[..1], &t), 0);
    }

    #[test]
    fn check_holds_tool_calls_with_their_results() {
        let (run, t) = shown();
        // The result left out, something else in its place.
        let mut unanswered = t.clone();
        unanswered.messages[2].content = vec![Block::text("a summary")];
        assert_eq!(broken(&run, &unanswered), 5);
        // The call left out, its result kept.
        let mut orphan = t.clone();
        orphan.messages[1].content.pop();
        assert_eq!(broken(&run, &orphan), 5);
        // A result for another call beside the right one.
        let mut stray = t.clone();
        stray.messages[2]
            .content
            .push(Block::tool_result("tu_9", "x", false));
        assert_eq!(broken(&run, &stray), 5);
        // A result in the first message answers nothing.
        let mut first = t;
        first.messages[0]
            .content
            .push(Block::tool_result("tu_1", "x", false));
        assert_eq!(broken(&run, &first), 5);
    }

    #[test]
    fn check_holds_the_order_of_stored_blocks() {
        let (run, t) = shown();
        // The nudge and the request change places: both are the user's
        // text blocks, so only their order is wrong.
        let mut swapped = t.clone();
        let nudge = swapped.messages[4].content[0].clone();
        swapped.messages[0].content.push(nudge);
        assert_eq!(broken(&run, &swapped), 6);
        // A block no step stores is taken for the rule's.
        let mut added = t;
        added.messages[4].content.push(Block::text("a summary"));
        assert_eq!(broken(&run, &added), 0);
    }

    #[test]
    fn check_holds_that_no_block_has_a_breakpoint() {
        let (run, t) = shown();
        let mut marked = t.clone();
        marked.messages[4].content.push(Block::Text {
            text: "a summary".into(),
            cache_control: Some(clax_sample::messages::CacheControl::default()),
        });
        assert_eq!(broken(&run, &marked), 7);
        let mut raw = t;
        raw.messages[4].content.push(Block::Raw(
            json!({"type": "text", "text": "x", "cache_control": {"type": "ephemeral"}}),
        ));
        assert_eq!(broken(&run, &raw), 7);
    }
}

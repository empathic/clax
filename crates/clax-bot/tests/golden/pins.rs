//! What every golden test shares: one run's pins ([`Pins`]), which read
//! the run back from the store, the fake host and the model API, make a
//! golden's text of it and compare it; what is normalised ([`scrub`]);
//! and the scripts and turns several sections use.

use super::names::Golden;
use crate::common::*;
use clax_bot::host::{FakeWrite, NOTICE_SEQ};
use clax_bot::web::{FetchedPage, WebError};
use clax_bot::{Next, Subscription, TickOutcome};
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, Step, StepKind};
use clax_store::credits::{CostLinePatch, LINE_WEB_FETCH};
use clax_store::roles::OWNER;
use serde_json::{Value, json};

#[path = "../../src/golden.rs"]
mod support;

/// A script that edits the draft and publishes nothing.
pub const DRAFT_ONLY: &str =
    r#"await clax.files.write("extra.css", "h1 { color: blue; }"); return "written";"#;

/// A script that edits the draft and publishes it, without a reply.
pub const PUBLISH: &str = r#"await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
const v = await clax.draft.publish({note: "Blue heading", addresses: []});
return v.version;"#;

/// What a script records of each call: what it returned, or what it threw
/// as a script sees it.
pub const ATTEMPT: &str = r#"const out = {};
const attempt = async (name, f) => {
  try { out[name] = {returned: await f()}; }
  catch (e) { out[name] = {threw: String(e), code: e.code, message: e.message, retryable: e.retryable}; }
};
"#;

/// The page the sender's comment names in the runs that fetch one.
pub const GIVEN: &str = "https://example.com/a";

/// A page at `url`, as a web source answers a fetch of it.
pub fn fetched(url: &str) -> Result<FetchedPage, WebError> {
    Ok(FetchedPage {
        final_url: url.into(),
        status: 200,
        content_type: "text/markdown".into(),
        text: "# Doc title\nbody".into(),
        links: Vec::new(),
        request_id: None,
    })
}

/// Makes the sender's comment `text`: the URLs in it may be fetched.
pub fn sender_says(env: &Env, text: &str) {
    env.host.lock().context.threads[0].comments[0].text = text.into();
}

/// What a search and a fetch cost in the goldens, in millicredits: set
/// here, so that a golden holds no web price the store was seeded with.
pub const SEARCH_MILLI: i64 = 50;
pub const FETCH_MILLI: i64 = 100;

/// Prices a search at [`SEARCH_MILLI`] and a fetch at [`FETCH_MILLI`].
pub fn price_the_web(env: &Env) {
    for (line, milli) in [
        ("web.search.ceramic", SEARCH_MILLI),
        (LINE_WEB_FETCH, FETCH_MILLI),
    ] {
        price(env, line, milli);
    }
}

/// Charges `milli` millicredits for one request of cost line `line`
/// (a line's fixed charge is for a thousand).
pub fn price(env: &Env, line: &str, milli: i64) {
    env.store
        .put_cost_line(
            &AuditCtx::DAEMON,
            line,
            CostLinePatch {
                fixed_credits_milli: Some(Some(milli * 1000)),
                ..CostLinePatch::default()
            },
        )
        .unwrap();
}

/// What the owner spent this week, in millicredits.
pub fn spent(env: &Env) -> i64 {
    env.store
        .week_usage(OWNER, env.clock_now())
        .unwrap()
        .spent_milli
}

/// A turn that calls `give_up` for `reason`.
pub fn give_up_turn(reason: &str) -> Reply {
    stream(
        &[B::Tool(
            "tu_g",
            "give_up",
            json!({"reason": reason}).to_string(),
        )],
        "tool_use",
        USAGE,
    )
}

pub fn kinds(out: &[TickOutcome]) -> Vec<StepKind> {
    out.iter().filter_map(|o| o.kind).collect()
}

/// Replaces, wherever they are in `v`, the values the header of
/// `tests/golden.rs` lists by key, where the key has a value: a script's
/// measurements, the hash of its code, a comment's ID and the build's
/// version and commit.
fn scrub(v: &mut Value) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::Object(map) => {
            for (k, x) in map.iter_mut() {
                let name = match k.as_str() {
                    "ms" | "us" | "busy_us" | "cpu_us" | "wall_us" | "cpu_ms" | "mem_peak"
                    | "mem_peak_bytes" => "<measured>",
                    "code_sha256" => "<sha256 of the code>",
                    "comment_id" => "<comment>",
                    "clax_version" => "<version>",
                    "clax_commit" => "<commit>",
                    _ => {
                        scrub(x);
                        continue;
                    }
                };
                if !x.is_null() {
                    *x = json!(name);
                }
            }
        }
        _ => {}
    }
}

/// What stands for credits charged from what a script measured.
const MEASURED_CREDITS: &str = "<charged from what the scripts measured>";

/// Replaces `v[key]`, when it has a value, by [`MEASURED_CREDITS`].
fn measured_credits(v: &mut Value, key: &str) {
    if let Some(credits) = v.get_mut(key)
        && !credits.is_null()
    {
        *credits = json!(MEASURED_CREDITS);
    }
}

/// `v` as a golden's text: pretty-printed, with a final newline.
fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("serialisable");
    s.push('\n');
    s
}

/// Compares `v`, the JSON of `what`, which holds several runs' values,
/// each normalised by its own run's pins ([`Pins::normalised`]), with
/// `golden`.
pub fn pin_normalised(what: &str, golden: Golden, v: &Value) {
    support::golden(what, &golden.path(), &pretty(v));
}

/// A request body with its system prompt's text and its tools, which this
/// checks are the first request's, replaced by a note.
pub fn after_the_first(first: &Value, body: &Value) -> Value {
    assert_eq!(body["system"], first["system"], "the system prompt");
    assert_eq!(body["tools"], first["tools"], "the tools");
    let mut b = body.clone();
    b["system"] = json!("(the first request's, compared by the test)");
    b["tools"] = json!("(the first request's, compared by the test)");
    b
}

/// One comparison a test makes among several ([`Pins::pin_all`]): what is
/// compared, its golden, and its text now.
pub struct Pinned {
    what: String,
    golden: Golden,
    text: String,
}

/// One run's pins: its environment and what stands for its IDs.
pub struct Pins {
    pub env: Env,
    pub run: String,
    /// The run's live events, when the test subscribed before driving it
    /// ([`Pins::subscribed`]).
    live: Option<Subscription>,
}

impl Pins {
    pub fn new(env: Env, run: String) -> Pins {
        Pins {
            env,
            run,
            live: None,
        }
    }

    /// The pins of a run not yet driven, listening to its live events
    /// from the first ([`Pins::events`]).
    pub fn subscribed(env: Env, run: String) -> Pins {
        let live = env
            .hub
            .subscribe(&env.store, &run, None)
            .expect("the run's events");
        Pins {
            env,
            run,
            live: Some(live),
        }
    }

    /// Drives the run to its end with a new driver, and answers every
    /// tick's outcome.
    pub async fn drive(&self) -> Vec<TickOutcome> {
        let out = self
            .env
            .drive(&self.env.deps(), &self.run, &DriverId::mint())
            .await;
        assert!(
            matches!(out.last().unwrap().next, Next::Done(_)),
            "the run ended: {out:?}"
        );
        out
    }

    /// `v` as a golden's text: scrubbed ([`scrub`]) and pretty-printed,
    /// with a final newline and this run's IDs replaced by their names.
    fn text(&self, v: &Value) -> String {
        let mut v = v.clone();
        scrub(&mut v);
        self.named(pretty(&v))
    }

    /// `s` with this run's minted IDs replaced by their names.
    fn named(&self, mut s: String) -> String {
        for (id, name) in [
            (&self.env.aid, "<artifact>"),
            (&self.env.t1, "<thread-1>"),
            (&self.env.t2, "<thread-2>"),
            (&self.run, "<run>"),
        ] {
            s = s.replace(id.as_str(), name);
        }
        s
    }

    /// `v` as [`Pins::pin`] writes it, parsed again: a part of a golden
    /// that holds several runs' values ([`pin_normalised`]), or a value
    /// to compare with another run's.
    pub fn normalised(&self, v: &Value) -> Value {
        serde_json::from_str(&self.text(v)).expect("the JSON it wrote")
    }

    /// Compares `v`, the JSON of `what`, with `golden`.
    pub fn pin(&self, what: &str, golden: Golden, v: &Value) {
        self.pin_all([self.json(what, golden, v)]);
    }

    /// Makes every comparison of `pinned`, and fails with every one that
    /// differs.
    pub fn pin_all(&self, pinned: impl IntoIterator<Item = Pinned>) {
        support::expect_none(
            pinned
                .into_iter()
                .map(|p| support::mismatch(&p.what, &p.golden.path(), &p.text)),
        );
    }

    /// The comparison of `v`, the JSON of `what`, with `golden`.
    pub fn json(&self, what: &str, golden: Golden, v: &Value) -> Pinned {
        Pinned {
            what: what.into(),
            golden,
            text: self.text(v),
        }
    }

    /// The comparison with `golden` of the body of `request` as it
    /// arrived at the model API: its bytes, with this run's IDs replaced
    /// by their names and nothing else changed, so the order of every
    /// object's keys, the spacing and the way numbers and strings are
    /// written are the wire's.
    pub fn wire(&self, what: &str, golden: Golden, request: &Seen) -> Pinned {
        Pinned {
            what: format!("the bytes of {what}, as sent"),
            golden,
            text: self.named(request.raw.clone()),
        }
    }

    fn stored(&self) -> (clax_store::bot_runs::Run, Vec<Step>) {
        self.env.store.run_with_steps(&self.run).unwrap().unwrap()
    }

    /// Every step of the run as stored: its number, kind, attempt and
    /// payload. A payload's `system` (the run's first turn keeps it),
    /// which this checks is the text of the system prompt of `first` (the
    /// run's first request body), is replaced by a note.
    pub fn steps(&self, first: &Value) -> Value {
        self.stored()
            .1
            .iter()
            .map(|s| {
                let mut payload = s.payload.clone();
                if let Some(system) = payload.as_mut().and_then(|p| p.get_mut("system")) {
                    assert_eq!(
                        *system, first["system"][0]["text"],
                        "the system prompt of step {}",
                        s.n
                    );
                    *system = json!("(the first request's, compared by the test)");
                }
                json!({"n": s.n, "kind": s.kind.as_str(), "attempt": s.attempt,
                       "started_at": s.started_at, "committed_at": s.committed_at,
                       "payload": payload})
            })
            .collect()
    }

    /// The journal of step `n`: each call's place (`seq`), attempt,
    /// operation and outcome.
    fn journal_of(&self, n: i64) -> Vec<Value> {
        self.env
            .store
            .bot_calls(&self.run, n)
            .unwrap()
            .iter()
            .map(|c| {
                json!({"step": c.step_n(), "attempt": c.attempt(), "seq": c.seq(),
                       "op": c.op(), "ok": c.ok(), "result": c.result()})
            })
            .collect()
    }

    /// The run's journal: every call of every step ([`Pins::journal_of`]).
    /// A notice the run loop posts is at [`NOTICE_SEQ`] and after.
    pub fn journal(&self) -> Value {
        self.stored()
            .1
            .iter()
            .flat_map(|s| self.journal_of(s.n))
            .collect()
    }

    /// The calls of every script step of the run: the step's `calls` as
    /// stored (each call's operation, target, outcome and code, which the
    /// run view and the `bot.call` records are made from) and its journal
    /// rows (what a script run again is answered from).
    pub fn script_calls(&self) -> Value {
        self.stored()
            .1
            .iter()
            .filter(|s| s.kind == StepKind::Script)
            .map(|s| {
                json!({
                    "step": s.n,
                    "calls": s.payload.as_ref().map(|p| p["calls"].clone()),
                    "journal": self.journal_of(s.n),
                })
            })
            .collect()
    }

    /// The writes the fake host committed, in the order it made them:
    /// each one's key (`<run>:<step>:<seq>`, its journal place), its
    /// operation, and the thread of a reply (a script's and the run
    /// loop's notices alike).
    pub fn writes(&self) -> Value {
        self.env
            .host
            .lock()
            .writes
            .iter()
            .map(|FakeWrite { key, op, args }| {
                let mut w = json!({"key": key, "op": op});
                if op == "threads.reply" {
                    w["thread"] = args[0].clone();
                }
                w
            })
            .collect()
    }

    /// The drafts the fake host was given to publish, in order: the
    /// version each was to be published over (`if_version`), its note,
    /// the threads it addresses and its files' paths.
    pub fn drafts_published(&self) -> Value {
        self.env
            .host
            .lock()
            .versions
            .iter()
            .map(|d| {
                json!({
                    "if_version": d.if_version,
                    "note": d.note,
                    "addresses": d.addresses,
                    "files": d.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
                })
            })
            .collect()
    }

    /// The usage rows the run's steps charged, in the order they were
    /// written: the step, the cost line, the quantity, the credits and
    /// whether the instance bore them. The quantity and the credits of
    /// script compute (`script.compute`) are what a script measured,
    /// and are replaced by a name.
    pub fn usage(&self) -> Value {
        self.env
            .usage()
            .iter()
            .map(|r| {
                let exact = |v: i64| {
                    if r.line == clax_store::credits::LINE_SCRIPT_COMPUTE {
                        json!("<measured>")
                    } else {
                        json!(v)
                    }
                };
                json!({
                    "step": r.step_n,
                    "line": r.line,
                    "qty": exact(r.qty),
                    "credits_milli": exact(r.credits_milli),
                    "absorbed": r.absorbed,
                })
            })
            .collect()
    }

    /// The run's `bot.*` audit records as stored, under `records`: kind,
    /// time, actor, the IDs kept in their own columns, and the body. Their
    /// kinds alone come first, under `kinds`, so that a record added,
    /// dropped or renamed is the first difference and reads as that.
    ///
    /// `credits_milli` is replaced where it is charged from what a script
    /// measured and is present with a value: in a `bot.script` record, and
    /// in `bot.run_end` when the run ran a script. Elsewhere it is exact.
    pub fn audit(&self) -> Value {
        let events: Vec<_> = self
            .env
            .store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("bot."))
            .collect();
        let scripted = events.iter().any(|e| e.kind == "bot.script");
        let records: Vec<Value> = events
            .iter()
            .map(|e| {
                let i = &e.ids;
                let mut body = serde_json::from_str::<Value>(&e.body).unwrap();
                if e.kind == "bot.script" || (e.kind == "bot.run_end" && scripted) {
                    measured_credits(&mut body, "credits_milli");
                }
                json!({
                    "kind": e.kind,
                    "at": e.at,
                    "actor": serde_json::from_str::<Value>(&e.actor).unwrap(),
                    "ids": {"artifact": i.artifact, "artifact2": i.artifact2, "thread": i.thread,
                            "session": i.session, "question": i.question, "call": i.call,
                            "origin": i.origin},
                    "body": body,
                })
            })
            .collect();
        json!({
            "kinds": events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            "records": records,
        })
    }

    /// The run's live events, as a subscriber that listened from before
    /// its first tick got them ([`Pins::subscribed`]), in order: each
    /// one's ID (`<step>.<attempt>:<seq>`), name and data. The run has
    /// ended.
    ///
    /// The credits charged from what a script measured are replaced where
    /// they are present with a value: `credits_milli` of a script's
    /// `step_done`, and from then on `run_credits_milli` of every
    /// `step_done` and `credits_milli` of `run_end`.
    pub async fn events(&mut self) -> Value {
        let live = self
            .live
            .as_mut()
            .expect("the pins of a run subscribed to before it was driven");
        let mut out = Vec::new();
        let mut scripted = false;
        while let Some(e) = live.next().await.expect("no event was missed") {
            let mut data = e.data.clone();
            if e.event == "step_done" {
                if data["kind"] == "script" {
                    scripted = true;
                    measured_credits(&mut data, "credits_milli");
                }
                if scripted {
                    measured_credits(&mut data, "run_credits_milli");
                }
            }
            if e.event == "run_end" && scripted {
                measured_credits(&mut data, "credits_milli");
            }
            out.push(json!({"id": e.id.to_string(), "event": e.event, "data": data}));
        }
        Value::Array(out)
    }

    /// What the run showed of itself on its threads' roster, from the
    /// [`clax_bot::host::BotHost::working`] calls the fake host recorded
    /// with the run's status and step in the store at each:
    ///
    /// - `shown_working_before`: for each step of the run, whether
    ///   ClaxBot was shown as working at least once while the run was at
    ///   that step and not ended. How often is not pinned: each call
    ///   renews the same record.
    /// - `shown_working_at_no_step`: the calls that showed it working at
    ///   a step the run does not have, or after the run ended.
    /// - `stopped_showing`: each call that stopped showing it, in order,
    ///   with the run's status in the store when it was made (the server
    ///   starts the threads' follow-ups from this call only when the run
    ///   has ended) and how many calls of either kind came after it.
    fn roster(&self, steps: &[Step]) -> Value {
        let calls = self.env.host.lock().roster.clone();
        let shown = |n: i64| {
            calls
                .iter()
                .any(|c| c.on && c.step == n && c.status.is_some_and(|s| !s.is_terminal()))
        };
        let stray = calls
            .iter()
            .filter(|c| {
                c.on && (c.status.is_none_or(|s| s.is_terminal())
                    || !steps.iter().any(|s| s.n == c.step))
            })
            .count();
        json!({
            "shown_working_before": steps.iter()
                .map(|s| json!({"step": s.n, "kind": s.kind.as_str(), "shown": shown(s.n)}))
                .collect::<Vec<_>>(),
            "shown_working_at_no_step": stray,
            "stopped_showing": calls.iter().enumerate()
                .filter(|(_, c)| !c.on)
                .map(|(i, c)| json!({
                    "run_status": c.status.map(|s| s.as_str()),
                    "calls_after_it": calls.len() - i - 1,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// How the run ended: its status and reason, its steps' kinds, its
    /// last step as stored, every reply ClaxBot left on the artifact's
    /// two threads as the store holds them (a script's and the run loop's
    /// notices alike, oldest first by thread), the writes the fake host
    /// committed in the order it made them, each reply with its thread
    /// ([`Pins::writes`]), the journal places of the last step's notices
    /// (at [`NOTICE_SEQ`] and after), and the roster ([`Pins::roster`]).
    pub fn ending(&self) -> Value {
        let (run, steps) = self.stored();
        let last = steps.last().expect("a step");
        let notices: Vec<Value> = self
            .env
            .store
            .bot_calls(&self.run, last.n)
            .unwrap()
            .iter()
            .filter(|c| c.seq() >= NOTICE_SEQ)
            .map(|c| json!({"step": c.step_n(), "seq": c.seq(), "op": c.op(), "ok": c.ok()}))
            .collect();
        json!({
            "status": run.status.as_str(),
            "reason": run.reason,
            "steps": steps.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(),
            "last_step": {"kind": last.kind.as_str(), "payload": last.payload},
            "replies": self.env.replies().iter()
                .map(|(thread, text)| json!({"thread": thread, "text": text}))
                .collect::<Vec<_>>(),
            "writes_in_order": self.writes(),
            "notices_in_the_journal": notices,
            "roster": self.roster(&steps),
        })
    }

    /// What the run left beside [`Pins::ending`]: its `bot.*` audit
    /// records ([`Pins::audit`]), the usage rows its steps charged
    /// ([`Pins::usage`]), its whole journal, a notice's rows with what
    /// they hold ([`Pins::journal`]), and every
    /// [`clax_bot::host::BotHost::working`] call the fake host received,
    /// in order: whether it showed ClaxBot working or stopped showing
    /// it, and the run's status and step in the store when it was made.
    pub fn ending_net(&self) -> Value {
        let roster: Vec<Value> = self
            .env
            .host
            .lock()
            .roster
            .iter()
            .map(
                |c| json!({"on": c.on, "run_status": c.status.map(|s| s.as_str()), "step": c.step}),
            )
            .collect();
        json!({
            "audit": self.audit(),
            "usage": self.usage(),
            "journal": self.journal(),
            "roster_calls": roster,
        })
    }

    /// The comparison of [`Pins::ending_net`] with the net of `golden`
    /// ([`Golden::net`]).
    pub fn ending_net_pinned(&self, golden: Golden) -> Pinned {
        self.json(
            &format!(
                "what else a run that ends leaves ({}): its audit records, usage, journal and roster calls",
                golden.net().file()
            ),
            golden.net(),
            &self.ending_net(),
        )
    }

    /// The comparison of [`Pins::ending`] with `golden`.
    pub fn ending_pinned(&self, golden: Golden) -> Pinned {
        self.json(
            &format!(
                "how a run ends ({}): its last step, replies, writes, notices and roster",
                golden.file()
            ),
            golden,
            &self.ending(),
        )
    }

    /// Pins [`Pins::ending`] as `golden` and [`Pins::ending_net`] as its
    /// net.
    pub fn pin_ending(&self, golden: Golden) {
        self.pin_all([self.ending_pinned(golden), self.ending_net_pinned(golden)]);
    }
}

/// Drives `run` to its end with a new driver, and gives its pins.
pub async fn driven(env: Env, run: String) -> Pins {
    let p = Pins::new(env, run);
    p.drive().await;
    p
}

//! The one Spend sequence every charged call of Clax's own surface goes
//! through, whatever its capability (spec 2026-11-19-claxbot-browser
//! §4.6): a search, a fetch, a browser test or browse. What differs per
//! operation is a [`Performer`], which answers each step's question; the
//! order of the steps, the refusals they make and what is written are
//! this module's alone. The first step that fails answers the script:
//!
//! 1. **Replay.** A call journaled at its place answers from the journal
//!    ([`crate::surface::Call::replay`]): what it answered, charged and
//!    showed. Nothing goes out; no browser opens.
//! 2. **Declared.** The run's binding has the operation
//!    ([`crate::surface::Binding::has`]) and the host has a source for its
//!    family ([`Performer::source`]), or `<family>_disabled`. An
//!    operation the binding lacks is refused by the run loop before the
//!    session sees it ([`crate::surface::SurfaceProvider::undeclared`],
//!    [`disabled`]), and refused the same here by a session handed one
//!    all the same.
//! 3. **Live.** The capability the call needs
//!    ([`Performer::capability`]) is live for the run's person now
//!    ([`Capabilities::now`], with what the host can perform,
//!    [`Performer::configured`]), or `<family>_disabled` naming it; a
//!    source whose host found it cannot run here
//!    ([`Performer::unusable`]) is `<family>_unavailable`, not retryable.
//! 4. **Arguments** ([`Performer::check_args`]), or `invalid_args`.
//! 5. **Admission** ([`Performer::admit`]): a URL passes its capability's
//!    rule, a clax target exists.
//! 6. **Free answers** ([`Performer::free_answer`]): a fetch of a URL
//!    and format the page store keeps answers its window from the store.
//! 7. **Cap.** The capability's per-run cap has room
//!    ([`Performer::count_key`]), or `<family>_cap`. The count is of the
//!    run's journaled calls of the family marked `counted`
//!    ([`clax_store::Store::bot_spend_counts`]).
//! 8. **Price.** The line exists (else `<family>_unavailable`: pricing
//!    not configured), and the person can pay the call's price cap
//!    ([`Performer::price_cap`]: one request, or a browser call's time
//!    cap) at its rate, at most their credits left this week unless their
//!    credits are not enforced, or `credits_exhausted`.
//! 9. **Perform** ([`Performer::perform`]), with the run's cancel token.
//!    What it answers is the call's answer: a failure on the page
//!    (`passed: false`, `ok: false`) is a value, journaled and charged as
//!    any (spec §4.7).
//! 10. **Write.** The journal entry, the usage row, a fetch's page and
//!     the `bot.spend` record, in one [`Store::spend_journaled`]
//!     transaction, before the script sees the answer.
//!
//! # What is written
//!
//! Every call past step 2 is journaled with its `bot.spend` record
//! (`capability`, `op`, `ok` or `code`, and the performer's summary of
//! the call, beside what the store adds: `run`, `step`,
//! `credits_milli`), refusals too, uncharged. A call whose transaction
//! fails, or whose entry cannot be made, has written nothing and is
//! answered [`Answer::unwritten`]: the step run again makes it again. A
//! refusal at step 2 is the run loop's to journal ([`Answer::refused`]),
//! with no `bot.spend` record.
//!
//! Each entry keeps, beside the script's value (`value`) or the error
//! (`code`, `message`, `capability`, and `retryable` when it is):
//! `cost_milli`, the charge; the family's summary of the call under its
//! name (`web` or `browser`), what the run panel shows; `counted`, true
//! when the call reached its source; and the performer's own fields (a
//! fetch's `source` and `format`). Every error a call of a capability is
//! answered carries that capability ([`HostError::capability`]).
//!
//! [`Store::spend_journaled`]: clax_store::Store::spend_journaled

use super::session::ClaxSession;
use crate::capability::{
    BrowserTerms, CODE_CREDITS_EXHAUSTED, Capabilities, Capability, Family, Off, Terms,
};
use crate::host::host_error;
use crate::surface::{Answer, Call, Fields, err_json, recorded};
use clax_bot_sandbox::HostError;
use clax_core::audit::{AuditKind, AuditRecord};
use clax_store::bot_runs::CallEntry;
use clax_store::bot_web::{SessionSpend, Spend, WebPageRow};
use clax_store::credits::{Applies, KeySource, Metered, Rate, row_credits_milli};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The family's name: its namespace, the prefix of its codes and the
/// field of a journal entry that keeps what the run shows of a call.
pub(super) fn family_name(f: Family) -> &'static str {
    match f {
        Family::Web => "web",
        Family::Browser => "browser",
    }
}

/// `<family>_<condition>`.
fn code(f: Family, condition: &str) -> String {
    format!("{}_{condition}", family_name(f))
}

/// How a call past step 2 ends, before it is written.
pub(super) struct Finish {
    pub res: Result<Value, HostError>,
    /// What to charge, at the rate step 8 read, and its credits.
    pub usage: Option<(Metered, i64)>,
    /// The page a fetch received.
    pub page: Option<WebPageRow>,
    /// What a browser session's call does to its session's row: the
    /// segment it charges (spec §10.3).
    pub session: Option<SessionSpend>,
    /// Whether the call reached its source, and so counts against the
    /// run's cap (step 7).
    pub counted: bool,
    /// Fields the `bot.spend` record adds.
    pub audit: Vec<(&'static str, Value)>,
    /// Fields the journal entry adds.
    pub entry: Vec<(&'static str, Value)>,
}

impl Finish {
    /// A call that answered `res`, uncharged and not counted.
    pub(super) fn new(res: Result<Value, HostError>) -> Finish {
        Finish {
            res,
            usage: None,
            page: None,
            session: None,
            counted: false,
            audit: Vec::new(),
            entry: Vec::new(),
        }
    }

    /// A refusal past step 2.
    pub(super) fn refused(e: HostError) -> Finish {
        Finish::new(Err(e))
    }

    /// This finish with `audit` before its own audit fields and `entry`
    /// before its own entry fields.
    pub(super) fn with(
        mut self,
        audit: &[(&'static str, Value)],
        entry: &[(&'static str, Value)],
    ) -> Finish {
        self.audit = audit.iter().cloned().chain(self.audit).collect();
        self.entry = entry.iter().cloned().chain(self.entry).collect();
        self
    }
}

/// What a call may cost at most (step 8): a quantity of a cost line,
/// and how a refusal names it.
pub(super) struct PriceCap {
    pub line: String,
    /// In the line's unit: 1 request, or a browser call's time cap in
    /// milliseconds.
    pub qty: i64,
    /// What the refusal says the call costs: `clax.web.fetch costs`, `a
    /// browser call of 60 s can cost`.
    pub costs: String,
}

/// What step 8 read: the line's rate, the key the call is on, and the
/// person's credits left this week before the call.
pub(super) struct Priced {
    pub rate: Rate,
    pub ks: KeySource,
    pub line: String,
    pub credits: i64,
}

impl Priced {
    /// The usage of `qty` of the line at the rate read, and its credits.
    ///
    /// # Errors
    /// The price cannot be computed.
    pub(super) fn usage(&self, qty: i64) -> clax_core::Result<(Metered, i64)> {
        let credits = if self.ks == KeySource::Own && self.rate.applies == Applies::InstanceKey {
            0
        } else {
            row_credits_milli(
                qty,
                self.rate.cost_micro_usd,
                self.rate.margin_bp,
                self.rate.fixed_credits_milli,
                self.rate.credit_price_micro_usd,
                self.rate.unit.base(),
            )?
        };
        let usage = Metered {
            rate: Some(self.rate),
            ..Metered::new(&self.line, qty, self.ks)
        };
        Ok((usage, credits))
    }
}

/// What the checks read of the run before step 6: its person's
/// capabilities now (step 3), and its counted calls of the family by op
/// (step 7).
pub(super) struct Seen {
    pub caps: Capabilities,
    pub counts: BTreeMap<String, u32>,
}

impl Seen {
    /// The run's counted calls of `op`.
    pub(super) fn count(&self, op: &str) -> u32 {
        self.counts.get(op).copied().unwrap_or(0)
    }

    /// What is left under capability `c`'s per-run cap once the run made
    /// `n` counted calls of it: 0 when it is not live.
    pub(super) fn left_of(&self, c: Capability, n: u32) -> u32 {
        let max = match self.caps.get(c) {
            Ok(Terms::Search(g)) => g.max_per_run,
            Ok(Terms::Fetch(g)) => g.max_per_run,
            Ok(Terms::Open(g)) => g.max_per_run,
            Err(_) => 0,
        };
        max.saturating_sub(n)
    }
}

/// One operation's answers to the steps of [`run`].
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub(super) trait Performer: Sync {
    /// The call's arguments, checked (step 4) and admitted (step 5).
    type Args: Send + Sync;

    /// The capability a call on `args` needs.
    fn capability(&self, args: &[Value]) -> Capability;

    /// What the run shows of a call on `args` before they are read.
    fn shown(&self, args: &[Value]) -> Value;

    /// Whether the record of a call adds what the run shows of it.
    fn audit_shown(&self) -> bool {
        false
    }

    /// Whether calls are performed one after another (the session's
    /// `browsing` lock), each written before the next is checked.
    fn serial(&self) -> bool {
        false
    }

    /// Step 3: why the source cannot run on this host, when it cannot.
    fn unusable(&self, _s: &ClaxSession<'_>) -> Option<HostError> {
        None
    }

    /// Step 4: `call`'s arguments under `terms` (its capability's) and
    /// `browser` (the browser family's settings, when one is live).
    ///
    /// # Errors
    /// `invalid_args`, or what the arguments cannot be read for.
    fn check_args(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        terms: &Terms,
        browser: Option<BrowserTerms>,
        shown: &mut Value,
    ) -> Result<Self::Args, HostError>;

    /// The fields every record and entry of a call with `args` adds once
    /// they are read: its audit fields, then its entry fields.
    fn noted(&self, _args: &Self::Args) -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
        (Vec::new(), Vec::new())
    }

    /// Step 5.
    ///
    /// # Errors
    /// The refusal.
    async fn admit(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &mut Self::Args,
        shown: &mut Value,
    ) -> Result<(), HostError>;

    /// Step 6: the call's answer without its source, when it has one.
    async fn free_answer(
        &self,
        _s: &ClaxSession<'_>,
        _call: &Call<'_>,
        _args: &Self::Args,
        _seen: &Seen,
    ) -> Option<Finish> {
        None
    }

    /// Step 7: the op the call is counted under, the capability's
    /// per-run cap and what the refusal calls its calls (`fetches`).
    fn count_key(&self, call: &Call<'_>, args: &Self::Args) -> (&'static str, u32, &'static str);

    /// Step 8.
    fn price_cap(&self, call: &Call<'_>, args: &Self::Args) -> PriceCap;

    /// Step 9.
    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &Self::Args,
        seen: &Seen,
        priced: &Priced,
        shown: &mut Value,
    ) -> Finish;

    /// What follows a refusal `credits_exhausted` of `call`, before it is
    /// written: a browser session's call closes its session (spec §6.3).
    async fn out_of_credits(&self, _s: &ClaxSession<'_>, _call: &Call<'_>) {}
}

/// What the run shows of a call of `family` beside its answer: its
/// summary under the family's name.
fn showing(family: Family, shown: Value) -> Fields {
    Fields::from_iter([(family_name(family).to_string(), shown)])
}

/// Step 2: what a script is answered for a charged operation `op` its
/// run's binding does not have, and for one it has on a host with no
/// source for it. Refused, charged nothing, with what the run shows of
/// the call (`shown`), for the run loop to journal.
pub(super) fn disabled(op: &str, c: Capability, shown: Value) -> Answer {
    let refusal = HostError::new(
        code(c.family(), "disabled"),
        format!("clax.{op} is not available to this run"),
    )
    .with_capability(c.id());
    Answer::refused(refusal)
        .uncharged()
        .showing(showing(c.family(), shown))
}

/// A call of `family` as its journal entry `e` answers it: its value or
/// its failure, what it charged (0 when the entry keeps no cost) and what
/// the run shows of it (`shown` when the entry keeps none).
fn from_entry(family: Family, e: &CallEntry, shown: Value) -> Answer {
    let r = e.result();
    let result = if e.ok() {
        Ok(r.get("value").cloned().unwrap_or(Value::Null))
    } else {
        recorded(e)
    };
    let name = family_name(family);
    Answer::written(result)
        .charged(r["cost_milli"].as_i64().unwrap_or(0))
        .showing(showing(family, r.get(name).cloned().unwrap_or(shown)))
}

/// A call that failed with `e` and wrote nothing: no entry, no charge, no
/// cost shown. Nothing journals it, and the step run again makes the call
/// again.
fn unwritten(c: Capability, e: HostError, shown: Value) -> Answer {
    Answer::unwritten(e.with_capability(c.id())).showing(showing(c.family(), shown))
}

/// Why a call of `op` needing `c` is refused at step 3: `c` is no longer
/// live for the run's person.
fn off(op: &str, c: Capability, why: Off) -> HostError {
    HostError::new(
        code(c.family(), "disabled"),
        format!(
            "clax.{op} was turned off ({}) since this run started; do without it",
            why.as_str()
        ),
    )
}

/// Performs `call` through the Spend sequence with `p`, or answers it
/// from its entry; see the module documentation.
pub(super) async fn run<P: Performer>(s: &ClaxSession<'_>, call: &Call<'_>, p: &P) -> Answer {
    let c = p.capability(call.args);
    let mut shown = p.shown(call.args);
    // Step 1, before any other: a call given its entry is answered from
    // it, whatever the binding and the host have by now.
    if let Some(e) = call.replay {
        return from_entry(c.family(), e, shown);
    }
    // Step 2.
    if !(s.has_source(c.family()) && s.cx.binding.has(call.op)) {
        return disabled(call.op, c, shown);
    }
    let _one_at_a_time = if p.serial() {
        Some(s.browsing.lock().await)
    } else {
        None
    };
    let f = fresh(s, call, p, c, &mut shown).await;
    if f.res.as_ref().is_err_and(|e| e.code == CODE_CREDITS_EXHAUSTED) {
        p.out_of_credits(s, call).await;
    }
    write(s, call, c, p.audit_shown(), shown, f)
}

/// Steps 3 to 9 of a call not in the journal.
async fn fresh<P: Performer>(
    s: &ClaxSession<'_>,
    call: &Call<'_>,
    p: &P,
    c: Capability,
    shown: &mut Value,
) -> Finish {
    let store = s.store();
    let run = &s.run;
    let family = c.family();
    // Step 3.
    let caps = match Capabilities::now(store, &run.person, &|k| s.configured(k)) {
        Ok(caps) => caps,
        Err(e) => return Finish::refused(host_error(&e)),
    };
    let terms = match caps.get(c) {
        Ok(t) => t.clone(),
        Err(why) => return Finish::refused(off(call.op, c, why)),
    };
    if let Some(e) = p.unusable(s) {
        return Finish::refused(e);
    }
    // Step 4.
    let mut args = match p.check_args(s, call, &terms, caps.browser(), shown) {
        Ok(a) => a,
        Err(e) => return Finish::refused(e),
    };
    let noted = |p: &P, args: &P::Args, f: Finish| {
        let (audit, entry) = p.noted(args);
        f.with(&audit, &entry)
    };
    // Step 5.
    if let Err(e) = p.admit(s, call, &mut args, shown).await {
        return noted(p, &args, Finish::refused(e));
    }
    let counts = match store.bot_spend_counts(&run.id, family_name(family)) {
        Ok(n) => n,
        Err(e) => return noted(p, &args, Finish::refused(host_error(&e))),
    };
    let seen = Seen { caps, counts };
    // Step 6.
    if let Some(f) = p.free_answer(s, call, &args, &seen).await {
        return noted(p, &args, f);
    }
    // Step 7.
    let (key, max, what) = p.count_key(call, &args);
    if seen.count(key) >= max {
        let e = HostError::new(
            code(family, "cap"),
            format!("this run has used its {max} {what}; work with what you have"),
        );
        return noted(p, &args, Finish::refused(e));
    }
    // Step 8.
    let priced = match priced(s, call.op, family, &p.price_cap(call, &args)) {
        Ok(x) => x,
        Err(e) => return noted(p, &args, Finish::refused(e)),
    };
    // Step 9.
    let f = p.perform(s, call, &args, &seen, &priced, shown).await;
    noted(p, &args, f)
}

/// Step 8: the line of `cap` at its rate now; `<family>_unavailable`
/// when the line has no rate (pricing is not configured, and a call
/// made anyway could not be charged), `credits_exhausted` when the
/// person cannot pay `cap`.
fn priced(
    s: &ClaxSession<'_>,
    op: &str,
    family: Family,
    cap: &PriceCap,
) -> Result<Priced, HostError> {
    let store = s.store();
    let line = &cap.line;
    let rate = store
        .rate_of(line)
        .map_err(|e| host_error(&e))?
        .ok_or_else(|| {
            HostError::new(
                code(family, "unavailable"),
                format!(
                    "{} pricing is not configured on this instance (it has no cost line \
                     {line}); do without clax.{op}",
                    family_name(family)
                ),
            )
        })?;
    let priced = Priced {
        rate,
        ks: s.key_source(),
        line: line.clone(),
        credits: s.week_left()?,
    };
    let (_, most) = priced.usage(cap.qty).map_err(|e| host_error(&e))?;
    let unenforced = store
        .flags_of(&s.run.person)
        .map_err(|e| host_error(&e))?
        .claxbot_unenforced;
    if !unenforced && most > priced.credits {
        return Err(HostError::new(
            CODE_CREDITS_EXHAUSTED,
            format!(
                "the person you act for has {} credits left this week and {} {}; finish with \
                 what you have",
                priced.credits as f64 / 1000.0,
                cap.costs,
                most as f64 / 1000.0
            ),
        ));
    }
    Ok(priced)
}

/// The journal entry's result of a call (see the module docs).
fn entry_result(family: Family, f: &Finish, cost: i64, shown: &Value) -> Value {
    let mut v = match &f.res {
        Ok(v) => json!({"value": v}),
        Err(e) => err_json(e),
    };
    v["cost_milli"] = json!(cost);
    v[family_name(family)] = shown.clone();
    if f.counted {
        v["counted"] = json!(true);
    }
    for (k, x) in &f.entry {
        v[*k] = x.clone();
    }
    v
}

/// Step 10: journals `f`, a call of `c`, at the call's place, with its
/// `bot.spend` record (adding what the run shows of the call, `shown`,
/// when `audit_shown`) and its charge, and answers what the journal
/// holds.
pub(super) fn write(
    s: &ClaxSession<'_>,
    call: &Call<'_>,
    c: Capability,
    audit_shown: bool,
    shown: Value,
    mut f: Finish,
) -> Answer {
    let store = s.store();
    let family = c.family();
    f.res = f.res.map_err(|e| e.with_capability(c.id()));
    let cost = f.usage.as_ref().map_or(0, |(_, credits)| *credits);
    let entry = match s.cx.claim.entry(call.place, call.op, call.raw) {
        Ok(e) => e.answered(f.res.is_ok(), entry_result(family, &f, cost, &shown)),
        Err(e) => return unwritten(c, e, shown),
    };
    let mut audit = AuditRecord::new(AuditKind::BotSpend, store.now())
        .with("capability", c.id())
        .with("op", call.op);
    if audit_shown {
        for (k, v) in shown.as_object().into_iter().flatten() {
            audit = audit.with(k, v.clone());
        }
    }
    audit = match &f.res {
        Ok(_) => audit.with("ok", true),
        Err(e) => audit.with("ok", false).with("code", e.code.as_str()),
    };
    for (k, v) in f.audit {
        audit = audit.with(k, v);
    }
    let spend = Spend {
        entry,
        usage: f.usage.map(|(m, _)| m),
        page: f.page,
        session: f.session,
        audit,
    };
    match store.spend_journaled(&s.run, s.cx.claim.driver(), spend) {
        Ok(spent) => {
            // Charged at the rate checked, the charge is its price.
            debug_assert!(spent.replayed || spent.credits_milli == cost);
            from_entry(family, &spent.entry, shown)
        }
        Err(e) => unwritten(c, host_error(&e), shown),
    }
}

/// Performs the charged call `call` (`web.*`, `browser.*`) with its
/// performer; `None` for an operation that is not charged.
pub(super) async fn perform(s: &ClaxSession<'_>, call: &Call<'_>) -> Option<Answer> {
    use crate::{browser, web};
    Some(match call.op {
        web::OP_SEARCH => run(s, call, &super::web::Search).await,
        web::OP_FETCH => run(s, call, &super::web::Fetch).await,
        browser::OP_TEST => run(s, call, &super::browser::Test).await,
        browser::OP_OPEN => run(s, call, &super::browser::Open).await,
        browser::OP_PAGE | browser::OP_ACT | browser::OP_EXPECT | browser::OP_CLOSE => {
            run(s, call, &super::browser::SessionOp::of(s, call)).await
        }
        _ => return None,
    })
}

/// Step 2 for the run loop: what a script is answered for a charged
/// operation `op` its run's binding does not have ([`disabled`]); `None`
/// for an operation that is not charged.
pub(super) fn undeclared(op: &str, args: &[Value]) -> Option<Answer> {
    use crate::{browser, web};
    let (c, shown) = match op {
        web::OP_SEARCH => (super::web::Search.capability(args), super::web::Search.shown(args)),
        web::OP_FETCH => (super::web::Fetch.capability(args), super::web::Fetch.shown(args)),
        browser::OP_TEST => (
            super::browser::Test.capability(args),
            super::browser::Test.shown(args),
        ),
        browser::OP_OPEN => (
            super::browser::Open.capability(args),
            super::browser::Open.shown(args),
        ),
        browser::OP_PAGE | browser::OP_ACT | browser::OP_EXPECT | browser::OP_CLOSE => {
            let p = super::browser::SessionOp::unbound(op);
            (p.capability(args), p.shown(args))
        }
        _ => return None,
    };
    Some(disabled(op, c, shown))
}

impl ClaxSession<'_> {
    /// The store the session's rows are in.
    pub(super) fn store(&self) -> &clax_store::Store {
        &self.provider.store
    }

    /// Whether the host has a source for `family` (step 2).
    fn has_source(&self, family: Family) -> bool {
        match family {
            Family::Web => self.provider.web.is_some(),
            Family::Browser => self.provider.browser.is_some(),
        }
    }

    /// Whether the host can perform `c` now (step 3): its source is there
    /// and has what `c` needs (the web source's credentials for search
    /// and fetch, [`crate::web::WebSource::configured`]; the browser
    /// source's, [`crate::browser::BrowserSource::configured`]).
    fn configured(&self, c: Capability) -> bool {
        match c {
            Capability::WebSearch => self.provider.web.as_ref().is_some_and(|w| w.configured().search),
            Capability::WebFetch => self.provider.web.as_ref().is_some_and(|w| w.configured().fetch),
            Capability::OpenUrl | Capability::OpenArtifact | Capability::OpenContent => {
                self.provider.browser.as_ref().is_some_and(|b| b.configured())
            }
        }
    }

    /// The person's credits left this week.
    pub(super) fn week_left(&self) -> Result<i64, HostError> {
        Ok(self
            .store()
            .week_usage(&self.run.person, self.provider.clock.now())
            .map_err(|e| host_error(&e))?
            .remaining_milli())
    }

    /// The key the run's person's calls are on now: their own when they
    /// turned it on, else the one the run was made on.
    pub(super) fn key_source(&self) -> KeySource {
        self.store()
            .key_source_of(&self.run.person)
            .unwrap_or(self.run.key_source)
    }
}

//! `browser.test` and `browser.browse` in a session of Clax's own
//! surface (the session's browser part, spec 2026-11-19-claxbot-browser
//! §5, §8). A browser call made by a script goes through these checks
//! in order, those of the web part ([`super::web`]) with the browser's
//! grant, codes and line, and the first that fails answers the script:
//!
//! 1. A call journaled at its place answers from the journal
//!    ([`crate::surface::Call::replay`]), before any other check: what
//!    it answered, what it charged and what the run shows of it. No
//!    browser opens.
//! 2. The run's binding has the operation and the host has a source
//!    (the provider's), or `browser_disabled`. An operation the binding
//!    lacks is refused by the run loop before the session sees it
//!    ([`crate::surface::SurfaceProvider::undeclared`], [`disabled`]), and
//!    refused the same here by a session handed one all the same.
//! 3. The operation is still granted now ([`BrowserAccess::now`]: the
//!    admin or the person may have turned it off since the run started,
//!    or the source lost its credentials,
//!    [`crate::browser::BrowserSource::configured`]), or
//!    `browser_disabled`. Browsing is granted only while the person's web
//!    choice is on too.
//! 4. Its arguments hold ([`crate::browser::opts`]: the target or the
//!    URL, the options, every cap), or `invalid_args`.
//! 5. `test`: the target exists: the version the call names, and the
//!    page in the draft or that version, or `browser_target_not_found`.
//!    `browse`: the URL and every `goto` step's URL may be opened under
//!    the admin's URL rule ([`crate::browser::browse::BrowseScope::allowed`],
//!    the sender's URLs read from the request now), or `url_not_allowed`.
//! 6. The run's tests (or browses) have room, or `browser_cap`. The count is of the
//!    run's journaled browser calls marked `counted`
//!    ([`clax_store::Store::bot_spend_counts`]): those whose browser
//!    opened, and those Browser Run refused, so a script cannot retry a
//!    failing Browser Run without end; not those the host refused before
//!    asking it (`browser_busy`, a cancelled run).
//! 7. The browser line exists (else `browser_unavailable`: browser
//!    pricing is not configured), and the person can pay for the call's
//!    time cap at its rate (at most their credits left this week, unless
//!    their credits are not enforced), or `credits_exhausted`.
//! 8. The source opens a browser ([`crate::browser::BrowserSource::open`])
//!    and the test runs ([`run_test`]) on the run's draft as the session
//!    holds it, edits not yet committed included, or on the version, with
//!    the source's deadline at the call's time cap from before the open,
//!    which it bounds too. A browse runs on the URL, its page's every
//!    request held to the rule's hosts
//!    ([`crate::browser::browse::BrowsePolicy`]).
//!
//! Calls of one step are performed one after the other (the session's
//! `browsing` lock), so two tests a script starts together never hold
//! two browsers of the instance's at once, and each is counted and
//! charged before the next is checked.
//!
//! # What is written
//!
//! Every call past check 2 is journaled by the session with its
//! `bot.browser` record in one transaction
//! ([`clax_store::Store::spend_journaled`], with no page), refusals too,
//! uncharged. A call whose browser opened is charged its measured time
//! (from the open to the close, by the provider's clock, rounded up to
//! the whole second) at the rate check 7 read, whatever it answered: a
//! result, a timeout, a cancellation, or none, its future dropped by the
//! sandbox on a cancel (`Opened`). Each screenshot is kept just
//! before as an item of the run's content (surface `clax`, then noted
//! with the call's operation and key); one that cannot be kept fails its
//! step. A failed transaction leaves them to the run's prune, writes
//! nothing else, and is answered [`Answer::unwritten`], so the step run
//! again makes the call again.
//!
//! Each entry keeps, beside the script's value or the error: `cost_milli`,
//! the charge; `browser`, what the run panel shows (`target`, `path` and
//! `viewport` once the arguments hold, or a browse's `url` and
//! `viewport`; then `ms`, `passed`, `steps` as
//! `[ok, failed]`, `tests` when saved tests ran, the counts of `errors`,
//! `console` and `blocked`, and the screenshots' IDs); and `counted`. The
//! `bot.browser` record names the operation, the version (none for the
//! draft) and the path, or a browse's URL, the viewport, `ms` and
//! `passed`, beside what the
//! store adds (`run`, `step`, `credits_milli`) and `ok` or `code`.
//!
//! The script's value is the test's result (spec §5.1) with each
//! screenshot step's value and `screenshots` the items' `Image` handles,
//! `cost` in credits, and `left`: the run's tests and browses left under
//! the caps after the call, and the person's credits left this week. A
//! script's error carries the browser error's `retryable` flag, and a
//! retryable error's message also says to try again (both from the flag,
//! in [`from_browser`] alone).

use super::session::ClaxSession;
use crate::browser::browse::{BrowsePolicy, BrowseRule, BrowseScope, sender_urls};
use crate::browser::opts::{self, BrowserOpts, BrowserTarget, Mode, Step};
use crate::browser::serve::{Pages, Site};
use crate::browser::session::{RequestPolicy, Start, TestPolicy, TestRun, run_test};
use crate::browser::{
    BrowserAccess, BrowserError, BrowserGrant, BrowserSource, CODE_BROWSER_BUSY, CODE_BROWSER_CAP,
    CODE_BROWSER_DISABLED, CODE_BROWSER_TARGET_NOT_FOUND, CODE_BROWSER_TIMEOUT,
    CODE_BROWSER_UNAVAILABLE, CODE_CREDITS_EXHAUSTED, Cdp, OP_BROWSE, OP_TEST,
};
use crate::host::host_error;
use crate::surface::{Answer, Call, Fields, err_json, recorded};
use clax_bot_sandbox::{CODE_CANCELLED, HostError};
use clax_core::ArtifactId;
use clax_core::audit::{AuditKind, AuditRecord};
use clax_store::bot_content::NewContent;
use clax_store::bot_runs::CallEntry;
use clax_store::bot_web::Spend;
use clax_store::credits::{
    Applies, KeySource, LINE_BROWSER_TIME, Metered, Rate, row_credits_milli,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// What the run shows of a browser call beside its answer: `browser`,
/// its summary.
fn showing(browser: Value) -> Fields {
    Fields::from_iter([("browser".to_string(), browser)])
}

/// What the run shows of a call to `op` on `args` before its arguments
/// are read: the target as the script gave it, or the URL. `None` for
/// an operation that is not the browser's.
fn shown(op: &str, args: &[Value]) -> Option<Value> {
    match op {
        OP_TEST => Some(json!({"target": args.first().cloned().unwrap_or(Value::Null)})),
        OP_BROWSE => Some(json!({"url": args.first().cloned().unwrap_or(Value::Null)})),
        _ => None,
    }
}

/// Check 2: what a script is answered for a browser operation its run's
/// binding does not have, and for one it has on a host with no browser
/// source. Refused, charged nothing, with what the run shows of the
/// call, for the run loop to journal; `None` for an operation that is
/// not the browser's.
pub(super) fn disabled(op: &str, args: &[Value]) -> Option<Answer> {
    let browser = shown(op, args)?;
    let refusal = HostError::new(
        CODE_BROWSER_DISABLED,
        format!("clax.{op} is not available to this run"),
    );
    Some(
        Answer::refused(refusal)
            .uncharged()
            .showing(showing(browser)),
    )
}

/// `e` as the script sees it: a retryable error is
/// [`HostError::retryable`], and its message says to try again. The only
/// place a browser error's flag and its words are made, so they cannot
/// disagree.
fn from_browser(e: &BrowserError) -> HostError {
    if e.retryable {
        HostError::new(e.code, format!("{} (retryable: try again)", e.message)).retryable()
    } else {
        HostError::new(e.code, e.message.clone())
    }
}

/// How a browser call ends, before it is written.
struct Finish {
    res: Result<Value, HostError>,
    /// What to charge, at the rate checked, and its credits.
    usage: Option<(Metered, i64)>,
    /// Whether the call counts against the run's cap (check 6).
    counted: bool,
    /// Fields the `bot.browser` record adds.
    audit: Vec<(&'static str, Value)>,
}

impl Finish {
    /// A refusal past check 2: uncharged, not counted.
    fn refused(e: HostError) -> Finish {
        Finish {
            res: Err(e),
            usage: None,
            counted: false,
            audit: Vec::new(),
        }
    }
}

/// The journal entry's result of a browser call (see the module docs).
fn entry_result(f: &Finish, cost: i64, shown: &Value) -> Value {
    let mut v = match &f.res {
        Ok(v) => json!({"value": v}),
        Err(e) => err_json(e),
    };
    v["cost_milli"] = json!(cost);
    v["browser"] = shown.clone();
    if f.counted {
        v["counted"] = json!(true);
    }
    v
}

/// A browser call as its journal entry `e` answers it: its value or its
/// failure, what it charged (0 when the entry keeps no cost) and what
/// the run shows of it (`shown` when the entry keeps no `browser`).
fn from_entry(e: &CallEntry, shown: Value) -> Answer {
    let r = e.result();
    let result = if e.ok() {
        Ok(r.get("value").cloned().unwrap_or(Value::Null))
    } else {
        recorded(e)
    };
    Answer::written(result)
        .charged(r["cost_milli"].as_i64().unwrap_or(0))
        .showing(showing(r.get("browser").cloned().unwrap_or(shown)))
}

/// A browser call that failed with `e` and wrote nothing: no entry, no
/// charge, no cost shown. Nothing journals it, and the step run again
/// makes the call again.
fn unwritten(e: HostError, shown: Value) -> Answer {
    Answer::unwritten(e).showing(showing(shown))
}

/// The credits of `ms` milliseconds of browser time at `rate` on a `ks`
/// key, as the store charges a usage of that quantity at that rate.
fn price(rate: &Rate, ks: KeySource, ms: i64) -> clax_core::Result<i64> {
    if ks == KeySource::Own && rate.applies == Applies::InstanceKey {
        return Ok(0);
    }
    row_credits_milli(
        ms,
        rate.cost_micro_usd,
        rate.margin_bp,
        rate.fixed_credits_milli,
        rate.credit_price_micro_usd,
        rate.unit.base(),
    )
}

/// `ms` rounded up to the whole second, in milliseconds.
fn whole_seconds(ms: i64) -> i64 {
    (ms.max(0) + 999) / 1000 * 1000
}

/// What the checks after the arguments read, and what the call's answer
/// is made with.
struct Ctx {
    grant: BrowserGrant,
    /// The call's operation, `browser.test` or `browser.browse`.
    op: &'static str,
    /// The run's counted tests and browses before the call.
    tests: u32,
    browses: u32,
    rate: Rate,
    ks: KeySource,
    /// The person's credits left this week before the call.
    credits: i64,
}

impl Ctx {
    /// `left` once the run made `tests` and `browses` counted calls,
    /// with `credits_milli` left this week.
    fn left(&self, tests: u32, browses: u32, credits_milli: i64) -> Value {
        let under = |max: Option<u32>, n: u32| max.map_or(0, |max| max.saturating_sub(n));
        json!({
            "tests": under(self.grant.test.as_ref().map(|g| g.max_per_run), tests),
            "browses": under(self.grant.browse.as_ref().map(|g| g.max_per_run), browses),
            "credits": credits_milli.max(0) as f64 / 1000.0,
        })
    }
}

/// A call whose browser opened and has not ended. The sandbox drops a
/// host call's future when the run is cancelled or its claim is lost
/// (`clax_bot_js`), so the call's own cancellation path, which closes the
/// browser and charges it, may never run: dropped while armed, this
/// journals, charges and audits the time so far
/// ([`ClaxSession::abandoned`]). `Browser.close` cannot be sent from a
/// drop; the socket's close ends the session, and Browser Run ends the
/// browser after its keep-alive. After a lost claim the journal write is
/// refused, as any other write of that claim's.
struct Opened<'a, 's, 'c> {
    session: &'a ClaxSession<'s>,
    call: &'a Call<'c>,
    cx: &'a Ctx,
    options: &'a BrowserOpts,
    audit: &'a [(&'static str, Value)],
    shown: Value,
    opened_at: chrono::DateTime<chrono::Utc>,
    armed: bool,
}

impl Drop for Opened<'_, '_, '_> {
    fn drop(&mut self) {
        if self.armed {
            self.session.abandoned(
                self.call,
                self.cx,
                self.options,
                self.audit,
                std::mem::take(&mut self.shown),
                self.opened_at,
            );
        }
    }
}

/// The handle of one kept screenshot, or why it could not be kept.
type Kept = Result<Value, String>;

impl ClaxSession<'_> {
    /// Performs the browser call `call`, or answers it from its entry;
    /// see the module documentation.
    pub(super) async fn browser(&self, call: &Call<'_>) -> Answer {
        let Some(mut shown) = shown(call.op, call.args) else {
            return Answer::refused(HostError::new(
                crate::api::CODE_UNKNOWN_OP,
                format!("clax.{} is not a browser operation", call.op),
            ));
        };
        // Check 1, before any other: a call given its entry is answered
        // from it, whatever the binding and the host have by now.
        if let Some(e) = call.replay {
            return from_entry(e, shown);
        }
        // Check 2.
        let source = match &self.provider.browser {
            Some(s) if self.cx.binding.has(call.op) => s.clone(),
            _ => {
                return disabled(call.op, call.args)
                    .expect("a browser operation has a refusal for a run without it");
            }
        };
        let _one_at_a_time = self.browsing.lock().await;
        let f = self.browser_fresh(&source, call, &mut shown).await;
        self.write_browser(call, shown, f)
    }

    /// Journals `f` at the call's place, with its `bot.browser` record
    /// and its charge, and answers what the journal holds.
    fn write_browser(&self, call: &Call<'_>, shown: Value, f: Finish) -> Answer {
        let store = self.store();
        let cost = f.usage.as_ref().map_or(0, |(_, c)| *c);
        let entry = match self.cx.claim.entry(call.place, call.op, call.raw) {
            Ok(e) => e.answered(f.res.is_ok(), entry_result(&f, cost, &shown)),
            Err(e) => return unwritten(e, shown),
        };
        let mut audit = AuditRecord::new(AuditKind::BotBrowser, store.now()).with("op", call.op);
        for (k, v) in f.audit {
            audit = audit.with(k, v);
        }
        audit = match &f.res {
            Ok(_) => audit.with("ok", true),
            Err(e) => audit.with("ok", false).with("code", e.code.as_str()),
        };
        let spend = Spend {
            entry,
            usage: f.usage.map(|(m, _)| m),
            page: None,
            audit,
        };
        match store.spend_journaled(&self.run, self.cx.claim.driver(), spend) {
            Ok(spent) => {
                // Charged at the rate checked, the charge is its price.
                debug_assert!(spent.replayed || spent.credits_milli == cost);
                from_entry(&spent.entry, shown)
            }
            Err(e) => unwritten(host_error(&e), shown),
        }
    }

    /// Checks 3 to 8 of a call not in the journal, on `source`.
    async fn browser_fresh(
        &self,
        source: &Arc<dyn BrowserSource>,
        call: &Call<'_>,
        shown: &mut Value,
    ) -> Finish {
        let (op, args) = (call.op, call.args);
        let store = self.store();
        let run = &self.run;
        // Check 3.
        let grant = match BrowserAccess::now(store, &run.person, source.configured()) {
            Ok(BrowserAccess::On(g)) => g,
            Ok(BrowserAccess::Off(why)) => {
                return Finish::refused(HostError::new(
                    CODE_BROWSER_DISABLED,
                    format!(
                        "browser use was turned off ({}) since this run started; do without it",
                        why.as_str()
                    ),
                ));
            }
            Err(e) => return Finish::refused(host_error(&e)),
        };
        let (op, max_per_run, seconds, mode) = if op == OP_BROWSE {
            let Some(b) = &grant.browse else {
                return Finish::refused(HostError::new(
                    CODE_BROWSER_DISABLED,
                    "browsing was turned off (by the admin, or the person's web choice) since \
                     this run started; do without clax.browser.browse",
                ));
            };
            (OP_BROWSE, b.max_per_run, b.seconds, Mode::Browse)
        } else {
            let Some(t) = &grant.test else {
                return Finish::refused(HostError::new(
                    CODE_BROWSER_DISABLED,
                    "the admin turned clax.browser.test off since this run started; do without it",
                ));
            };
            (OP_TEST, t.max_per_run, t.seconds, Mode::Test)
        };
        // Check 4.
        let options = match opts::parse_opts(&args[1], mode, seconds) {
            Ok(o) => o,
            Err(e) => return Finish::refused(from_browser(&e)),
        };
        if mode == Mode::Browse {
            return self
                .browse(source, call, grant, max_per_run, options, shown)
                .await;
        }
        let target = match opts::parse_target(&args[0]) {
            Ok(t) => t,
            Err(e) => return Finish::refused(from_browser(&e)),
        };
        *shown = json!({
            "target": target.version.map_or_else(|| "draft".to_string(), |n| format!("v{n}")),
            "path": target.path,
            "viewport": options.viewport.label(),
        });
        let mut audit = vec![
            ("path", json!(target.path)),
            ("viewport", json!(options.viewport.label())),
        ];
        if let Some(n) = target.version {
            audit.insert(0, ("version", json!(n)));
        }
        let with_audit = |f: Finish, more: Vec<(&'static str, Value)>| Finish {
            audit: audit.iter().cloned().chain(more).collect(),
            ..f
        };
        // Check 5.
        let artifact = match ArtifactId::parse(&run.artifact_id) {
            Ok(a) => a,
            Err(e) => return with_audit(Finish::refused(host_error(&e)), vec![]),
        };
        let pages = match self.pages(&artifact, &target).await {
            Ok(p) => p,
            Err(e) => return with_audit(Finish::refused(e), vec![]),
        };
        // Checks 6 and 7.
        let cx = match self.browser_ctx(grant, op, max_per_run, &options) {
            Ok(cx) => cx,
            Err(e) => return with_audit(Finish::refused(e), vec![]),
        };
        let site = Site::new(&artifact, pages);
        let policy = TestPolicy {
            site: &site,
            source: source.as_ref(),
            store,
            person: &run.person,
        };
        let start = Start::Test {
            origin: site.origin.clone(),
            path: target.path.clone(),
        };
        // Check 8.
        let (f, more) = self
            .open_and_run(source, call, &cx, &options, &start, &policy, &audit, shown)
            .await;
        with_audit(f, more)
    }

    /// Checks 5 to 8 of a browse, its options `options` read.
    async fn browse(
        &self,
        source: &Arc<dyn BrowserSource>,
        call: &Call<'_>,
        grant: BrowserGrant,
        max_per_run: u32,
        options: BrowserOpts,
        shown: &mut Value,
    ) -> Finish {
        let Some(url) = call.args[0].as_str() else {
            return Finish::refused(HostError::new(
                crate::api::CODE_BAD_ARGS,
                "url: must be a string, the public URL to open",
            ));
        };
        *shown = json!({"url": url, "viewport": options.viewport.label()});
        let audit = [
            ("url", json!(url)),
            ("viewport", json!(options.viewport.label())),
        ];
        let with_audit = |f: Finish, more: Vec<(&'static str, Value)>| Finish {
            audit: audit.iter().cloned().chain(more).collect(),
            ..f
        };
        // Check 5.
        let scope = match self.browse_scope(source.as_ref(), &grant).await {
            Ok(s) => s,
            Err(e) => return with_audit(Finish::refused(e), vec![]),
        };
        let url = match scope.allowed(url) {
            Ok(u) => u,
            Err(e) => return with_audit(Finish::refused(from_browser(&e)), vec![]),
        };
        for (i, step) in options.steps.iter().enumerate() {
            if let Step::Goto(to) = step
                && let Err(e) = scope.allowed(to)
            {
                let e = BrowserError::new(
                    e.code,
                    format!("opts.steps[{i}].goto: {}", e.message),
                    false,
                );
                return with_audit(Finish::refused(from_browser(&e)), vec![]);
            }
        }
        // Checks 6 and 7.
        let cx = match self.browser_ctx(grant, OP_BROWSE, max_per_run, &options) {
            Ok(cx) => cx,
            Err(e) => return with_audit(Finish::refused(e), vec![]),
        };
        let policy = BrowsePolicy::new(&scope, source.as_ref());
        let start = Start::Browse {
            url: url.as_str().to_string(),
        };
        // Check 8.
        let (f, more) = self
            .open_and_run(source, call, &cx, &options, &start, &policy, &audit, shown)
            .await;
        with_audit(f, more)
    }

    /// What a browse under `grant`'s rule may open: the instance's own
    /// hosts from `source`, and under the sender rule the URLs of the
    /// request this run answers, read now.
    async fn browse_scope(
        &self,
        source: &dyn BrowserSource,
        grant: &BrowserGrant,
    ) -> Result<BrowseScope, HostError> {
        let rule = grant
            .browse
            .as_ref()
            .map_or(BrowseRule::Sender, |b| b.rule.clone());
        let own = source.own_hosts().map_err(|why| {
            HostError::new(
                CODE_BROWSER_UNAVAILABLE,
                format!("this instance cannot name its own hosts ({why}), so it browses nothing"),
            )
        })?;
        let sender = if rule == BrowseRule::Sender {
            let cx = self.provider.host.context(&self.run).await.map_err(|e| {
                tracing::warn!(
                    "clax-bot: run {} has no request context for a browse: {e}",
                    self.run.id
                );
                HostError::new(
                    CODE_BROWSER_UNAVAILABLE,
                    "the request this run answers could not be read, so no URL can be checked \
                     against it; do without this page",
                )
            })?;
            sender_urls(&cx)
        } else {
            Default::default()
        };
        Ok(BrowseScope::new(rule, own, sender))
    }

    /// Check 8: opens a browser on `source` and runs `start` under
    /// `policy`, then ends the call ([`ClaxSession::tested`]). `audit` is
    /// what the call's `bot.browser` record holds so far, for the record
    /// written should this future be dropped once the browser opened
    /// ([`Opened`]).
    #[allow(clippy::too_many_arguments)]
    async fn open_and_run(
        &self,
        source: &Arc<dyn BrowserSource>,
        call: &Call<'_>,
        cx: &Ctx,
        options: &BrowserOpts,
        start: &Start,
        policy: &dyn RequestPolicy,
        audit: &[(&'static str, Value)],
        shown: &mut Value,
    ) -> (Finish, Vec<(&'static str, Value)>) {
        // The call's time cap bounds the open too: an upgrade that stalls
        // ends at the deadline, counted (Browser Run was asked) and not
        // charged (no browser opened).
        let mut deadline = source.deadline(
            self.provider.clock.now() + chrono::Duration::seconds(i64::from(options.timeout_s)),
        );
        let opening = futures::future::select(source.open(self.cx.cancel), &mut deadline).await;
        let transport = match opening {
            futures::future::Either::Right(((), _)) => {
                let e = BrowserError::new(
                    CODE_BROWSER_TIMEOUT,
                    format!(
                        "Browser Run did not open a browser within the call's {} s",
                        options.timeout_s
                    ),
                    true,
                );
                return (
                    Finish {
                        counted: true,
                        ..Finish::refused(from_browser(&e))
                    },
                    vec![],
                );
            }
            futures::future::Either::Left((Ok(t), _)) => t,
            futures::future::Either::Left((Err(e), _)) => {
                // Refused before Browser Run was asked: not counted.
                let counted = e.code != CODE_BROWSER_BUSY && e.code != CODE_CANCELLED;
                return (
                    Finish {
                        counted,
                        ..Finish::refused(from_browser(&e))
                    },
                    vec![],
                );
            }
        };
        // Declared before the connection, so a dropped future closes the
        // socket before the guard writes the charge.
        let mut opened = Opened {
            session: self,
            call,
            cx,
            options,
            audit,
            shown: shown.clone(),
            opened_at: self.provider.clock.now(),
            armed: true,
        };
        let mut cdp = Cdp::new(transport, deadline, self.cx.cancel.clone());
        let tested = run_test(
            &mut cdp,
            start,
            options,
            policy,
            self.provider.clock.as_ref(),
            self.provider.browser_limits,
        )
        .await;
        opened.armed = false;
        self.tested(call, cx, options, tested, shown)
    }

    /// Journals, charges and audits a call whose browser opened at
    /// `opened_at` and whose future was dropped before it ended: its time
    /// so far, counted, answered `cancelled` ([`Opened`]).
    fn abandoned(
        &self,
        call: &Call<'_>,
        cx: &Ctx,
        options: &BrowserOpts,
        audit: &[(&'static str, Value)],
        mut shown: Value,
        opened_at: chrono::DateTime<chrono::Utc>,
    ) {
        let ms = (self.provider.clock.now() - opened_at)
            .num_milliseconds()
            .max(0);
        let qty = whole_seconds(ms).min(i64::from(options.timeout_s) * 1000);
        let usage = match price(&cx.rate, cx.ks, qty) {
            Ok(cost) => Some((
                Metered {
                    rate: Some(cx.rate),
                    ..Metered::new(LINE_BROWSER_TIME, qty, cx.ks)
                },
                cost,
            )),
            Err(e) => {
                tracing::warn!(
                    "clax-bot: run {}: an abandoned browser call could not be priced: {e}",
                    self.run.id
                );
                None
            }
        };
        shown["ms"] = json!(ms);
        let f = Finish {
            res: Err(HostError::new(
                CODE_CANCELLED,
                "the run stopped while the browser was open",
            )),
            usage,
            counted: true,
            audit: audit.iter().cloned().chain([("ms", json!(ms))]).collect(),
        };
        let _ = self.write_browser(call, shown, f);
    }

    /// Check 5: the files of `target`, or `browser_target_not_found`.
    async fn pages(
        &self,
        artifact: &ArtifactId,
        target: &BrowserTarget,
    ) -> Result<Pages, HostError> {
        let missing = |m: String| HostError::new(CODE_BROWSER_TARGET_NOT_FOUND, m);
        let Some(n) = target.version else {
            let (files, base) = {
                let st = self.lock();
                (st.draft.files().clone(), st.draft.base_version)
            };
            if !files.contains_key(&target.path) {
                return Err(missing(format!(
                    "the draft has no file {}; list them with clax.files.list()",
                    target.path
                )));
            }
            return Ok(Pages::Draft(files, base));
        };
        let store = self.store();
        if store
            .get_version(artifact, n)
            .map_err(|e| host_error(&e))?
            .is_none()
        {
            return Err(missing(format!("the artifact has no version {n}")));
        }
        if store
            .read_file_async(artifact, n, &target.path)
            .await
            .map_err(|e| host_error(&e))?
            .is_none()
        {
            return Err(missing(format!(
                "version {n} of the artifact has no file {}",
                target.path
            )));
        }
        Ok(Pages::Version(artifact.clone(), n))
    }

    /// Checks 6 and 7: the run's counted calls of `op` under its cap
    /// `max_per_run`, the line's rate, and the person's credits for the
    /// call's time cap.
    fn browser_ctx(
        &self,
        grant: BrowserGrant,
        op: &'static str,
        max_per_run: u32,
        options: &BrowserOpts,
    ) -> Result<Ctx, HostError> {
        let store = self.store();
        let counted = store
            .bot_spend_counts(&self.run.id, "browser")
            .map_err(|e| host_error(&e))?;
        let count = |op: &str| counted.get(op).copied().unwrap_or(0);
        let (tests, browses) = (count(OP_TEST), count(OP_BROWSE));
        let (used, what) = if op == OP_BROWSE {
            (browses, "browses")
        } else {
            (tests, "browser tests")
        };
        if used >= max_per_run {
            return Err(HostError::new(
                CODE_BROWSER_CAP,
                format!("this run has used its {max_per_run} {what}; work with what you have"),
            ));
        }
        let rate = store
            .rate_of(LINE_BROWSER_TIME)
            .map_err(|e| host_error(&e))?
            .ok_or_else(|| {
                HostError::new(
                    CODE_BROWSER_UNAVAILABLE,
                    format!(
                        "browser pricing is not configured on this instance (it has no cost \
                         line {LINE_BROWSER_TIME}); do without clax.{op}"
                    ),
                )
            })?;
        let ks = self.key_source();
        let most =
            price(&rate, ks, i64::from(options.timeout_s) * 1000).map_err(|e| host_error(&e))?;
        let credits = self.week_left()?;
        let unenforced = store
            .flags_of(&self.run.person)
            .map_err(|e| host_error(&e))?
            .claxbot_unenforced;
        if !unenforced && most > credits {
            return Err(HostError::new(
                CODE_CREDITS_EXHAUSTED,
                format!(
                    "the person you act for has {} credits left this week and a browser call of \
                     {} s can cost {}; finish with what you have",
                    credits as f64 / 1000.0,
                    options.timeout_s,
                    most as f64 / 1000.0
                ),
            ));
        }
        Ok(Ctx {
            grant,
            op,
            tests,
            browses,
            rate,
            ks,
            credits,
        })
    }

    /// How a test whose browser opened ends: charged its time, rounded
    /// up to the second and at most the call's time cap (what the credits
    /// check priced), and counted; its screenshots kept, its result
    /// completed; and the `bot.browser` record's `ms` and `passed`.
    fn tested(
        &self,
        call: &Call<'_>,
        cx: &Ctx,
        options: &BrowserOpts,
        tested: TestRun,
        shown: &mut Value,
    ) -> (Finish, Vec<(&'static str, Value)>) {
        let ms = (tested.closed_at - tested.opened_at)
            .num_milliseconds()
            .max(0);
        let qty = whole_seconds(ms).min(i64::from(options.timeout_s) * 1000);
        let cost = match price(&cx.rate, cx.ks, qty) {
            Ok(c) => c,
            Err(e) => return (Finish::refused(host_error(&e)), vec![]),
        };
        let usage = Metered {
            rate: Some(cx.rate),
            ..Metered::new(LINE_BROWSER_TIME, qty, cx.ks)
        };
        shown["ms"] = json!(ms);
        let mut more = vec![("ms", json!(ms))];
        let res = match tested.outcome {
            Ok(mut v) => {
                let kept: Vec<Kept> = tested
                    .shots
                    .iter()
                    .map(|png| self.keep_screenshot(call, png))
                    .collect();
                let mut handles = Vec::new();
                let mut ids = Vec::new();
                for k in kept.iter().flatten() {
                    handles.push(k.clone());
                    ids.push(k["id"].clone());
                }
                let mut lost = false;
                // Only a screenshot step's value names a screenshot: an
                // eval step's is whatever the page answered.
                let steps = v["steps"].as_array_mut().into_iter().flatten();
                for (step, asked) in steps.zip(&options.steps) {
                    if !matches!(asked, Step::Screenshot { .. }) {
                        continue;
                    }
                    let Some(i) = step["value"]["screenshot"].as_u64() else {
                        continue;
                    };
                    match kept.get(usize::try_from(i).unwrap_or(usize::MAX)) {
                        Some(Ok(handle)) => step["value"] = handle.clone(),
                        Some(Err(why)) => {
                            *step = json!({"ok": false, "error": why});
                            lost = true;
                        }
                        None => {}
                    }
                }
                if lost {
                    v["passed"] = json!(false);
                }
                v["screenshots"] = Value::Array(handles);
                v["cost"] = json!(cost as f64 / 1000.0);
                let (tests, browses) = if cx.op == OP_BROWSE {
                    (cx.tests, cx.browses + 1)
                } else {
                    (cx.tests + 1, cx.browses)
                };
                v["left"] = cx.left(tests, browses, cx.credits - cost);
                summarize(shown, &v, ids);
                more.push(("passed", v["passed"].clone()));
                Ok(v)
            }
            Err(e) => Err(from_browser(&e)),
        };
        (
            Finish {
                res,
                usage: Some((usage, cost)),
                counted: true,
                audit: Vec::new(),
            },
            more,
        )
    }

    /// Keeps one screenshot as an item of the run's content, noted as
    /// `call`'s: its handle, or why it could not be kept.
    fn keep_screenshot(&self, call: &Call<'_>, png: &[u8]) -> Kept {
        let store = self.store();
        let size = clax_core::media::sniff(png).ok();
        let id: String = clax_core::secret::random_bytes::<16>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let new = NewContent {
            id: &id,
            run_id: &self.run.id,
            surface: clax_store::bot_runs::CLAX_SURFACE,
            registration_id: None,
            name: None,
            media_type: "image/png",
            width: size.as_ref().map(|s| s.width),
            height: size.as_ref().map(|s| s.height),
            bytes: png,
        };
        let row = store
            .put_bot_content(&new, self.provider.content_limits)
            .map_err(|e| format!("the screenshot could not be kept: {e}"))?;
        store
            .note_bot_content(&row.id, call.op, &call.place.key, call.place.step, None)
            .map_err(|e| format!("the screenshot could not be kept: {e}"))?;
        Ok(row.handle())
    }
}

/// What the run shows of a test's result `v`, into `shown` (see the
/// module docs), with its screenshots' IDs `ids`.
fn summarize(shown: &mut Value, v: &Value, ids: Vec<Value>) {
    let steps = v["steps"].as_array().map_or(&[][..], Vec::as_slice);
    let ok = steps.iter().filter(|s| s["ok"] == true).count();
    let len = |x: &Value| x.as_array().map_or(0, Vec::len);
    shown["passed"] = v["passed"].clone();
    shown["steps"] = json!([ok, steps.len() - ok]);
    if let Some(t) = v.get("tests").filter(|t| !t.is_null()) {
        shown["tests"] = json!({"passed": t["passed"], "failed": len(&t["failed"])});
    }
    shown["errors"] = json!(len(&v["errors"]));
    shown["console"] = json!(len(&v["console"]));
    shown["blocked"] = json!(len(&v["requests"]["blocked"]));
    shown["screenshots"] = Value::Array(ids);
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn time_is_charged_by_the_whole_second() {
        assert_eq!(whole_seconds(0), 0);
        assert_eq!(whole_seconds(1), 1_000);
        assert_eq!(whole_seconds(4_200), 5_000);
        assert_eq!(whole_seconds(5_000), 5_000);
        assert_eq!(whole_seconds(-3), 0);
    }

    #[test]
    fn a_summary_counts_what_the_panel_shows() {
        let mut shown = json!({"target": "draft"});
        let v = json!({
            "passed": false,
            "steps": [{"ok": true}, {"ok": false, "error": "x"}, {"ok": false, "skipped": true}],
            "tests": {"passed": 2, "failed": [{"name": "a", "error": "b"}], "skipped": 0},
            "errors": [{"message": "m"}],
            "console": [],
            "requests": {"failed": [], "blocked": [{"url": "u", "why": "origin"}]},
        });
        summarize(&mut shown, &v, vec![json!("c1")]);
        assert_eq!(
            shown,
            json!({"target": "draft", "passed": false, "steps": [1, 2],
                   "tests": {"passed": 2, "failed": 1}, "errors": 1, "console": 0,
                   "blocked": 1, "screenshots": ["c1"]})
        );
    }
}

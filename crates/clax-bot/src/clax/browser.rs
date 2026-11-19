//! `browser.test` and `browser.browse` in a session of Clax's own
//! surface: the browser's [`Performer`]s on the one Spend sequence
//! ([`super::spend`]; spec 2026-11-19-claxbot-browser §4.6, §6.8). What
//! each answers to the sequence's steps:
//!
//! - **Live** (step 3): `browser.open.artifact` for a test,
//!   `browser.open.url` for a browse.
//! - **Arguments** (step 4, [`crate::browser::opts`]): the target or the
//!   URL, the options, every cap, the call's time at most
//!   `browser.call_seconds`.
//! - **Admission** (step 5). A test: the target exists, the version the
//!   call names and the page in the draft or that version, or
//!   `browser_target_not_found`. A browse: the URL and every `goto`
//!   step's URL pass the admin's URL rule
//!   ([`crate::browser::browse::BrowseScope::allowed`], the run's sources
//!   read now), or `url_not_allowed` or `url_not_in_prior_context`.
//! - **Cap** (step 7): the run's tests or browses counted: those whose
//!   browser opened, and those Browser Run refused, so a script cannot
//!   retry a failing Browser Run without end; not those the host refused
//!   before asking it (`browser_busy`, a cancelled run).
//! - **Price** (step 8): the call's time cap at the browser line.
//! - **Perform** (step 9): the source opens a browser
//!   ([`crate::browser::BrowserSource::open`]) and the test runs
//!   ([`run_test`]) on the run's draft as the session holds it, edits not
//!   yet committed included, or on the version, with the source's
//!   deadline at the call's time cap from before the open, which it
//!   bounds too. A browse runs on the URL, its page's every request held
//!   to the rule's hosts ([`crate::browser::browse::BrowsePolicy`]).
//!
//! Calls of one step are performed one after the other (the session's
//! `browsing` lock, [`Performer::serial`]), so two tests a script starts
//! together never hold two browsers of the instance's at once, and each
//! is counted and charged before the next is checked.
//!
//! # What is charged and shown
//!
//! A call whose browser opened is charged its measured time (from the
//! open to the close, by the provider's clock, rounded up to the whole
//! second) at the rate step 8 read, whatever it answered: a result, a
//! timeout, a cancellation, or none, its future dropped by the sandbox on
//! a cancel (`Opened`). A page that throws or an assertion that fails is
//! an answer (`passed: false`), journaled and charged as any. Each
//! screenshot is kept just before as an item of the run's content
//! (surface `clax`, then noted with the call's operation and key); one
//! that cannot be kept fails its step. A failed transaction leaves them
//! to the run's prune.
//!
//! Each entry keeps under `browser` what the run panel shows (`target`,
//! `path` and `viewport` once the arguments hold, or a browse's `url` and
//! `viewport`; then `ms`, `passed`, `steps` as `[ok, failed]`, `tests`
//! when saved tests ran, the counts of `errors`, `console` and `blocked`,
//! and the screenshots' IDs). Each `bot.spend` record of a browser call
//! names the version (none for the draft) and the path, or a browse's
//! URL, the viewport, `ms` and `passed`.
//!
//! The script's value is the test's result (spec §5.1) with each
//! screenshot step's value and `screenshots` the items' `Image` handles,
//! `cost` in credits, and `left`: the run's tests and browses left under
//! the caps after the call, and the person's credits left this week. A
//! script's error carries the browser error's `retryable` flag, and a
//! retryable error's message also says to try again (both from the flag,
//! in [`from_browser`] alone).

use super::session::ClaxSession;
use super::spend::{self, Finish, Performer, PriceCap, Priced, Seen};
use crate::browser::browse::{BrowsePolicy, BrowseScope};
use crate::browser::opts::{self, BrowserOpts, BrowserTarget, Mode, Step};
use crate::browser::serve::{Pages, Site};
use crate::browser::session::{RequestPolicy, Start, TestPolicy, TestRun, run_test};
use crate::browser::{
    BrowserError, BrowserSource, CODE_BROWSER_BUSY, CODE_BROWSER_TARGET_NOT_FOUND,
    CODE_BROWSER_TIMEOUT, CODE_BROWSER_UNAVAILABLE, Cdp, OP_BROWSE, OP_TEST,
};
use crate::capability::{BrowserTerms, Capability, OpenGrant, Terms, UrlRule};
use crate::host::host_error;
use crate::surface::Call;
use crate::web::PublicUrl;
use clax_bot_sandbox::{CODE_CANCELLED, HostError};
use clax_core::ArtifactId;
use clax_store::bot_content::NewContent;
use clax_store::credits::LINE_BROWSER_TIME;
use serde_json::{Value, json};
use std::sync::Arc;

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

/// `ms` rounded up to the whole second, in milliseconds.
fn whole_seconds(ms: i64) -> i64 {
    (ms.max(0) + 999) / 1000 * 1000
}

/// What a performed call's answer is made with: its operation, what the
/// Spend sequence read before it ([`Seen`]) and the price it checked
/// ([`Priced`]).
struct Ctx<'a> {
    /// The call's operation, `browser.test` or `browser.browse`.
    op: &'static str,
    seen: &'a Seen,
    priced: &'a Priced,
}

impl Ctx<'_> {
    /// `left` once the run made one more counted call of [`Ctx::op`],
    /// with `credits_milli` left this week.
    fn left(&self, credits_milli: i64) -> Value {
        let (mut tests, mut browses) = (self.seen.count(OP_TEST), self.seen.count(OP_BROWSE));
        if self.op == OP_BROWSE {
            browses += 1;
        } else {
            tests += 1;
        }
        json!({
            "tests": self.seen.left_of(Capability::OpenArtifact, tests),
            "browses": self.seen.left_of(Capability::OpenUrl, browses),
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
    cx: &'a Ctx<'a>,
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

/// `clax.browser.test`.
pub(super) struct Test;

/// `clax.browser.browse`.
pub(super) struct Browse;

/// A test's arguments, and what its admission found.
pub(super) struct TestArgs {
    max_per_run: u32,
    options: BrowserOpts,
    target: BrowserTarget,
    /// What the call's records add once its arguments are read.
    audit: Vec<(&'static str, Value)>,
    /// The artifact and the files of the target, found at admission.
    found: Option<(ArtifactId, Pages)>,
}

/// A browse's arguments, and what its admission found.
pub(super) struct BrowseArgs {
    max_per_run: u32,
    rule: UrlRule,
    options: BrowserOpts,
    url: String,
    audit: Vec<(&'static str, Value)>,
    /// What the browse may open, and its URL, found at admission.
    found: Option<(BrowseScope, PublicUrl)>,
}

/// The browser source; [`super::spend`]'s step 2 has made sure there is
/// one.
fn browser_source(s: &ClaxSession<'_>) -> Arc<dyn BrowserSource> {
    s.provider
        .browser
        .clone()
        .expect("the Spend sequence performs a browser call only on a host with a browser source")
}

/// The open grant of `terms`.
fn open_grant(terms: &Terms) -> &OpenGrant {
    let Terms::Open(g) = terms else {
        unreachable!("a browser capability's terms are an open grant")
    };
    g
}

/// What a browser call may cost at most: its time cap.
fn time_cap(options: &BrowserOpts) -> PriceCap {
    PriceCap {
        line: LINE_BROWSER_TIME.to_string(),
        qty: i64::from(options.timeout_s) * 1000,
        costs: format!("a browser call of {} s can cost", options.timeout_s),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Performer for Test {
    type Args = TestArgs;

    fn capability(&self, _args: &[Value]) -> Capability {
        Capability::OpenArtifact
    }

    fn shown(&self, args: &[Value]) -> Value {
        json!({"target": args.first().cloned().unwrap_or(Value::Null)})
    }

    fn serial(&self) -> bool {
        true
    }

    fn check_args(
        &self,
        _s: &ClaxSession<'_>,
        call: &Call<'_>,
        terms: &Terms,
        browser: Option<BrowserTerms>,
        shown: &mut Value,
    ) -> Result<TestArgs, HostError> {
        let seconds = browser.map_or(0, |b| b.call_seconds);
        let options =
            opts::parse_opts(&call.args[1], Mode::Test, seconds).map_err(|e| from_browser(&e))?;
        let target = opts::parse_target(&call.args[0]).map_err(|e| from_browser(&e))?;
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
        Ok(TestArgs {
            max_per_run: open_grant(terms).max_per_run,
            options,
            target,
            audit,
            found: None,
        })
    }

    fn noted(&self, args: &TestArgs) -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
        (args.audit.clone(), Vec::new())
    }

    async fn admit(
        &self,
        s: &ClaxSession<'_>,
        _call: &Call<'_>,
        args: &mut TestArgs,
        _shown: &mut Value,
    ) -> Result<(), HostError> {
        let artifact = ArtifactId::parse(&s.run.artifact_id).map_err(|e| host_error(&e))?;
        let pages = s.pages(&artifact, &args.target).await?;
        args.found = Some((artifact, pages));
        Ok(())
    }

    fn count_key(&self, _call: &Call<'_>, args: &TestArgs) -> (&'static str, u32, &'static str) {
        (OP_TEST, args.max_per_run, "browser tests")
    }

    fn price_cap(&self, _call: &Call<'_>, args: &TestArgs) -> PriceCap {
        time_cap(&args.options)
    }

    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &TestArgs,
        seen: &Seen,
        priced: &Priced,
        shown: &mut Value,
    ) -> Finish {
        let Some((artifact, pages)) = &args.found else {
            unreachable!("a test is performed only once admitted")
        };
        let source = browser_source(s);
        let site = Site::new(artifact, pages.clone());
        let policy = TestPolicy {
            site: &site,
            source: source.as_ref(),
            store: s.store(),
            person: &s.run.person,
        };
        let start = Start::Test {
            origin: site.origin.clone(),
            path: args.target.path.clone(),
        };
        let cx = Ctx { op: OP_TEST, seen, priced };
        s.open_and_run(&source, call, &cx, &args.options, &start, &policy, &args.audit, shown)
            .await
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Performer for Browse {
    type Args = BrowseArgs;

    fn capability(&self, _args: &[Value]) -> Capability {
        Capability::OpenUrl
    }

    fn shown(&self, args: &[Value]) -> Value {
        json!({"url": args.first().cloned().unwrap_or(Value::Null)})
    }

    fn serial(&self) -> bool {
        true
    }

    fn check_args(
        &self,
        _s: &ClaxSession<'_>,
        call: &Call<'_>,
        terms: &Terms,
        browser: Option<BrowserTerms>,
        shown: &mut Value,
    ) -> Result<BrowseArgs, HostError> {
        let g = open_grant(terms);
        let seconds = browser.map_or(0, |b| b.call_seconds);
        let options =
            opts::parse_opts(&call.args[1], Mode::Browse, seconds).map_err(|e| from_browser(&e))?;
        let Some(url) = call.args[0].as_str() else {
            return Err(HostError::new(
                crate::api::CODE_BAD_ARGS,
                "url: must be a string, the public URL to open",
            ));
        };
        *shown = json!({"url": url, "viewport": options.viewport.label()});
        let audit = vec![
            ("url", json!(url)),
            ("viewport", json!(options.viewport.label())),
        ];
        Ok(BrowseArgs {
            max_per_run: g.max_per_run,
            rule: g.url_rule.clone().unwrap_or(UrlRule::Sender),
            options,
            url: url.to_string(),
            audit,
            found: None,
        })
    }

    fn noted(&self, args: &BrowseArgs) -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
        (args.audit.clone(), Vec::new())
    }

    async fn admit(
        &self,
        s: &ClaxSession<'_>,
        _call: &Call<'_>,
        args: &mut BrowseArgs,
        _shown: &mut Value,
    ) -> Result<(), HostError> {
        let scope = s
            .browse_scope(browser_source(s).as_ref(), &args.rule)
            .await?;
        let url = scope.allowed(&args.url).map_err(|e| from_browser(&e))?;
        for (i, step) in args.options.steps.iter().enumerate() {
            if let Step::Goto(to) = step
                && let Err(e) = scope.allowed(to)
            {
                let e = BrowserError::new(
                    e.code,
                    format!("opts.steps[{i}].goto: {}", e.message),
                    false,
                );
                return Err(from_browser(&e));
            }
        }
        args.found = Some((scope, url));
        Ok(())
    }

    fn count_key(&self, _call: &Call<'_>, args: &BrowseArgs) -> (&'static str, u32, &'static str) {
        (OP_BROWSE, args.max_per_run, "browses")
    }

    fn price_cap(&self, _call: &Call<'_>, args: &BrowseArgs) -> PriceCap {
        time_cap(&args.options)
    }

    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &BrowseArgs,
        seen: &Seen,
        priced: &Priced,
        shown: &mut Value,
    ) -> Finish {
        let Some((scope, url)) = &args.found else {
            unreachable!("a browse is performed only once admitted")
        };
        let source = browser_source(s);
        let policy = BrowsePolicy::new(scope, source.as_ref());
        let start = Start::Browse {
            url: url.as_str().to_string(),
        };
        let cx = Ctx { op: OP_BROWSE, seen, priced };
        s.open_and_run(&source, call, &cx, &args.options, &start, &policy, &args.audit, shown)
            .await
    }
}

impl ClaxSession<'_> {
    /// What a browse under `rule` may open: the instance's own hosts from
    /// `source`, and the run's sources
    /// ([`crate::web::access::sources`]: the request this run answers, its
    /// journaled searches and its page store), read now; under
    /// `any_public` none are read.
    async fn browse_scope(
        &self,
        source: &dyn BrowserSource,
        rule: &UrlRule,
    ) -> Result<BrowseScope, HostError> {
        let rule = rule.clone();
        let own = source.own_hosts().map_err(|why| {
            HostError::new(
                CODE_BROWSER_UNAVAILABLE,
                format!("this instance cannot name its own hosts ({why}), so it browses nothing"),
            )
        })?;
        let found = if rule == UrlRule::AnyPublic {
            Default::default()
        } else {
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
            let store = self.store();
            let journal = store
                .bot_web_calls(&self.run.id)
                .map_err(|e| host_error(&e))?;
            let pages = store.web_pages(&self.run.id).map_err(|e| host_error(&e))?;
            crate::web::access::sources(&cx, &journal, &pages)
        };
        Ok(BrowseScope::new(rule, own, found))
    }

    /// Step 9: opens a browser on `source` and runs `start` under
    /// `policy`, then ends the call ([`ClaxSession::tested`]). `audit` is
    /// what the call's `bot.spend` record holds so far, for the record
    /// written should this future be dropped once the browser opened
    /// ([`Opened`]).
    #[allow(clippy::too_many_arguments)]
    async fn open_and_run(
        &self,
        source: &Arc<dyn BrowserSource>,
        call: &Call<'_>,
        cx: &Ctx<'_>,
        options: &BrowserOpts,
        start: &Start,
        policy: &dyn RequestPolicy,
        audit: &[(&'static str, Value)],
        shown: &mut Value,
    ) -> Finish {
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
                return Finish {
                    counted: true,
                    ..Finish::refused(from_browser(&e))
                };
            }
            futures::future::Either::Left((Ok(t), _)) => t,
            futures::future::Either::Left((Err(e), _)) => {
                // Refused before Browser Run was asked: not counted.
                let counted = e.code != CODE_BROWSER_BUSY && e.code != CODE_CANCELLED;
                return Finish {
                    counted,
                    ..Finish::refused(from_browser(&e))
                };
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
        cx: &Ctx<'_>,
        options: &BrowserOpts,
        audit: &[(&'static str, Value)],
        mut shown: Value,
        opened_at: chrono::DateTime<chrono::Utc>,
    ) {
        let ms = (self.provider.clock.now() - opened_at)
            .num_milliseconds()
            .max(0);
        let qty = whole_seconds(ms).min(i64::from(options.timeout_s) * 1000);
        let usage = match cx.priced.usage(qty) {
            Ok(usage) => Some(usage),
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
            usage,
            counted: true,
            audit: audit.iter().cloned().chain([("ms", json!(ms))]).collect(),
            ..Finish::refused(HostError::new(
                CODE_CANCELLED,
                "the run stopped while the browser was open",
            ))
        };
        let c = if cx.op == OP_BROWSE {
            Capability::OpenUrl
        } else {
            Capability::OpenArtifact
        };
        let _ = spend::write(self, call, c, false, shown, f);
    }

    /// Step 5: the files of `target`, or `browser_target_not_found`.
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

    /// How a test whose browser opened ends: charged its time, rounded
    /// up to the second and at most the call's time cap (what the credits
    /// check priced), and counted; its screenshots kept, its result
    /// completed; and the `bot.spend` record's `ms` and `passed`.
    fn tested(
        &self,
        call: &Call<'_>,
        cx: &Ctx<'_>,
        options: &BrowserOpts,
        tested: TestRun,
        shown: &mut Value,
    ) -> Finish {
        let ms = (tested.closed_at - tested.opened_at)
            .num_milliseconds()
            .max(0);
        let qty = whole_seconds(ms).min(i64::from(options.timeout_s) * 1000);
        let (usage, cost) = match cx.priced.usage(qty) {
            Ok(x) => x,
            Err(e) => return Finish::refused(host_error(&e)),
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
                v["left"] = cx.left(cx.priced.credits - cost);
                summarize(shown, &v, ids);
                more.push(("passed", v["passed"].clone()));
                Ok(v)
            }
            Err(e) => Err(from_browser(&e)),
        };
        Finish {
            usage: Some((usage, cost)),
            counted: true,
            audit: more,
            ..Finish::new(res)
        }
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

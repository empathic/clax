//! The browser's operations in a session of Clax's own surface: its
//! [`Performer`]s on the one Spend sequence ([`super::spend`]; spec
//! 2026-11-19-claxbot-browser §4.6, §6.2 to §6.4, §6.8, §6.9): [`Open`]
//! for `browser.open`, [`SessionOp`] for `browser.page`, `browser.act`,
//! `browser.expect` and `browser.close`, [`Test`] for `browser.test`.
//! What each answers to the sequence's steps:
//!
//! - **Live** (step 3): an open needs the capability of its source's
//!   kind (`clax-browser:open-url`, `.artifact`, `.content`); a call on a
//!   session needs its session's kind's, as the registry holds it (a
//!   `goto` out of its origin made it a URL session), else as the store
//!   does; a test needs `clax-browser:open-artifact`.
//! - **Arguments** (step 4): an open's source and viewport
//!   ([`crate::browser::opts::parse_source`], with spec §6.2's caps); a
//!   session call's action, locator and `args`, by its row of the layer's
//!   table ([`crate::browser::layer`]): an action the table does not have
//!   is `invalid_args`, as is an option its row does not take.
//! - **Admission** (step 5). An open: the run has fewer sessions open
//!   than `browser.max_open_per_run`, else `browser_cap`; a URL passes the
//!   admin's URL rule; a clax target exists; each `Image` of supplied
//!   files is an item of the run's content. A session call: its handle,
//!   the capability its `Page` holds (spec §17.3), names a session of this
//!   run that is open in this process (else `browser_session_closed` or
//!   `browser_session_lost`, uncharged); `runTests` only on a session of
//!   the run's clax.
//! - **Cap** (step 7): an open, and a test, are counted against their
//!   kind's `max_per_run`, under the capability's ID (the entry's
//!   `count_as`); calls on a session are not counted, but for a `goto` out
//!   of an artifact or content session, which makes a session on a URL:
//!   it is counted under `clax-browser:open-url`, and refused `browser_cap`
//!   once the run has used that capability's `max_per_run`.
//! - **Price** (step 8): one call's time cap, `browser.call_seconds`.
//! - **Perform** (step 9): the host's session registry
//!   ([`crate::browser::live::BrowserSessions`]) opens the session or
//!   performs the call; each `goto` of a URL session passes
//!   `clax-browser:open-url`'s URL rule, as its open did, else the rule's
//!   refusal; a `goto` out of an artifact or content session is
//!   given the URL session's policy when the run declared
//!   `clax-browser:open-url`, it is live and its rule admits the URL, and is refused `browser_disabled` by the
//!   session otherwise.
//!
//! # What is charged
//!
//! A session's time is charged in segments, each millisecond once (spec
//! §10.3): an open from the WebSocket's upgrade to the open's end, each
//! call from the session's charged point (the store's row) to its own
//! end, written with the call's entry ([`Finish::session`]). A failure
//! on the page is an answer (`ok: false`), journaled and charged as any;
//! a session that ended answers `browser_session_closed` (or
//! `browser_session_lost`) uncharged, its tail charged by the store when
//! the host ended it ([`ClaxSession::charge_ended`], at each call and at
//! the run's end). A close whose Spend is not written is charged its last
//! segment all the same, as an end the host made
//! ([`clax_store::Store::end_bot_browser_session`]); an open whose Spend
//! is not written closes its session, `dropped`, and charges its time
//! ([`ClaxSession::end_unwritten`]). A refusal
//! `credits_exhausted` closes the session,
//! `credits`. A test is charged its measured time in milliseconds, at
//! most its time cap, as one segment of no session; so is an open whose
//! browser opened and failed before its page loaded, from the
//! WebSocket's upgrade. Nothing is rounded up per call.
//!
//! Each screenshot is kept just before as an item of the run's content
//! (surface `clax`, then noted with the call's operation and key) and
//! answered as its `Image` handle; one that cannot be kept fails the
//! call's answer (`ok: false`), or a test's (`passed: false`).
//!
//! Each entry keeps under `browser` what the run panel shows of the call,
//! naming a session by its ID and never by its handle (an open's `kind`,
//! `viewport`, `session` and `ms`; a session call's
//! `session`, `action` and `locator`, then `ok` and `ms`; a test's
//! `target`, `path`, `viewport`, then `ms`, `passed`, `tests` and the
//! counts of `errors`, `console` and `blocked`). A script's error carries
//! the browser error's `retryable` flag, and a retryable error's message
//! also says to try again (both from the flag, in [`from_browser`]
//! alone).

use super::session::ClaxSession;
use super::spend::{self, Finish, Performer, PriceCap, Priced, Seen};
use crate::browser::browse::{BrowsePolicy, BrowseScope};
use crate::browser::cap::{Cap, Caveat, Class, Refusal, origin_of};
use crate::browser::layer::{self, Op};
use crate::browser::live::{
    BrowserSessions, CallCtx, Nav, SessionCall, SessionTimes, new_session_id,
};
use crate::browser::opts::{self, BrowserOpts, BrowserTarget, SourceSpec, Viewport};
use crate::browser::serve::{ContentFile, ContentSite, Pages, Site};
use crate::browser::session::{
    ArtifactPolicy, ContentPolicy, RequestPolicy, Start, TestPolicy, TestRun, run_test,
};
use crate::browser::{
    BrowserError, BrowserSource, CODE_BROWSER_BUSY, CODE_BROWSER_CAP, CODE_BROWSER_DISABLED,
    CODE_BROWSER_FORBIDDEN, CODE_BROWSER_TARGET_NOT_FOUND, CODE_BROWSER_TIMEOUT,
    CODE_BROWSER_UNAVAILABLE, Cdp,
};
use crate::capability::{BrowserTerms, Capability, OpenGrant, Terms, UrlRule};
use crate::host::host_error;
use crate::surface::Call;
use crate::web::PublicUrl;
use clax_bot_sandbox::{CODE_CANCELLED, HostError};
use clax_core::ArtifactId;
use clax_store::bot_content::NewContent;
use clax_store::bot_grants::{GrantWrite, ShareGrant};
use clax_store::bot_web::{SessionEnd, SessionKind, SessionSpend};
use clax_store::credits::LINE_BROWSER_TIME;
use serde_json::{Value, json};
use std::collections::BTreeMap;
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

/// `invalid_args` saying `what`.
fn bad(what: impl Into<String>) -> HostError {
    HostError::new(crate::api::CODE_BAD_ARGS, what)
}

/// The capability that opens a session of `kind`.
fn cap_of(kind: SessionKind) -> Capability {
    match kind {
        SessionKind::Url => Capability::OpenUrl,
        SessionKind::Artifact => Capability::OpenArtifact,
        SessionKind::Content => Capability::OpenContent,
    }
}

/// What a refusal at the cap calls a kind's sessions.
fn sessions_of(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Url => "browser sessions on a URL",
        SessionKind::Artifact => "browser sessions on the clax",
        SessionKind::Content => "browser sessions on supplied content",
    }
}

/// `left` once the run made one more counted open of `this`'s kind
/// (none for a call that is not counted), with `credits_milli` left this
/// week: each source kind's sessions left under its cap.
fn left(seen: &Seen, this: Option<Capability>, credits_milli: i64) -> Value {
    let n = |c: Capability| seen.count(c.id()) + u32::from(this == Some(c));
    json!({
        "url": seen.left_of(Capability::OpenUrl, n(Capability::OpenUrl)),
        "artifact": seen.left_of(Capability::OpenArtifact, n(Capability::OpenArtifact)),
        "content": seen.left_of(Capability::OpenContent, n(Capability::OpenContent)),
        "credits": credits_milli.max(0) as f64 / 1000.0,
    })
}

/// What a performed test's answer is made with: what the Spend sequence
/// read before it ([`Seen`]) and the price it checked ([`Priced`]).
struct Ctx<'a> {
    seen: &'a Seen,
    priced: &'a Priced,
}

/// A test whose browser opened and has not ended. The sandbox drops a
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

/// `clax.browser.open`.
pub(super) struct Open;

/// `browser.page`, `browser.act`, `browser.expect` and `browser.close`:
/// a call on an open session, of the kind it has (see the module docs).
pub(super) struct SessionOp {
    op: Op,
    /// The capability the call needs: its session's kind's.
    cap: Capability,
    kind: Option<SessionKind>,
    /// The ID of the session the call's handle is the capability of, when
    /// this process holds one for the run (spec §17.3).
    session: Option<String>,
}

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

/// What an open's admission found.
pub(super) enum Found {
    Url(BrowseScope, PublicUrl),
    Artifact(ArtifactId, Pages, String),
    Content(Vec<u8>, BTreeMap<String, ContentFile>),
}

/// An open's arguments, and what its admission found.
pub(super) struct OpenArgs {
    spec: SourceSpec,
    viewport: Viewport,
    max_per_run: u32,
    rule: UrlRule,
    terms: BrowserTerms,
    audit: Vec<(&'static str, Value)>,
    found: Option<Found>,
}

/// A session call's arguments.
pub(super) struct CallArgs {
    session: String,
    call: Option<SessionCall>,
    row: Option<&'static layer::Row>,
    terms: BrowserTerms,
    /// A `share` or `revoke`: the registry's, not the browser's.
    cap_call: Option<CapCall>,
    /// Where a `navigate` call goes, for its capability's `origin` caveat.
    nav: Nav,
    /// The capability that authorized the call, once checked: its
    /// session's task holds a navigation to its `origin` caveat again.
    held: Option<Cap>,
    /// The session's charged point when the call was admitted, and its
    /// time since then not yet charged (the model's turns included): what
    /// the price check adds to the call's time cap (spec §10.3).
    charged_ms: i64,
    uncharged_ms: i64,
}

/// What a `share` or `revoke` asks of the page's capability (spec §18.4).
enum CapCall {
    /// A share: its session actions, its lifetime in seconds, its
    /// origins.
    Share {
        actions: Option<Vec<Class>>,
        expires: Option<u32>,
        origins: Option<Vec<String>>,
    },
    Revoke,
}

/// `args` of a `share`, checked (spec §18.4): `actions` a list of
/// names, `expires` seconds (1 or more), `origins` a non-empty list of
/// origins.
fn share_of(args: &Value) -> Result<CapCall, HostError> {
    let strings = |k: &str| -> Result<Option<Vec<String>>, HostError> {
        match args.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .map(Some)
                .ok_or_else(|| bad(format!("args.{k}: must be a list of strings"))),
            Some(_) => Err(bad(format!("args.{k}: must be a list of strings"))),
        }
    };
    let actions = strings("actions")?
        .map(|names| {
            names
                .iter()
                .map(|n| {
                    Class::parse(n).ok_or_else(|| {
                        bad(format!(
                            "args.actions: `{n}` is not one of read, act, navigate, record, close"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let expires = match args.get("expires").and_then(Value::as_u64) {
        None => None,
        Some(n) if n >= 1 => Some(u32::try_from(n).unwrap_or(u32::MAX)),
        Some(_) => return Err(bad("args.expires: must be 1 second or more")),
    };
    let origins = strings("origins")?
        .map(|os| {
            if os.is_empty() {
                return Err(bad("args.origins: must name at least one origin"));
            }
            os.iter()
                .map(|o| {
                    origin_of(o).ok_or_else(|| {
                        bad(format!(
                            "args.origins: `{o}` is not an origin like https://example.com"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(CapCall::Share {
        actions,
        expires,
        origins,
    })
}

/// Whether `call` is a `share` or `revoke`, which never reach the browser.
fn is_cap_call(call: &Call<'_>) -> bool {
    call.op == crate::browser::OP_PAGE
        && matches!(
            call.args.get(1).and_then(Value::as_str),
            Some("share" | "revoke")
        )
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

/// The browser family's settings, live whenever a browser capability is.
fn terms_of(browser: Option<BrowserTerms>) -> Result<BrowserTerms, HostError> {
    browser.ok_or_else(|| {
        HostError::new(
            CODE_BROWSER_DISABLED,
            "the browser is not available to this run",
        )
    })
}

/// What a call of `seconds` may cost at most: its time cap.
fn time_cap(seconds: u32) -> PriceCap {
    PriceCap {
        line: LINE_BROWSER_TIME.to_string(),
        qty: i64::from(seconds) * 1000,
        after: 0,
        costs: format!("a browser call of {seconds} s can cost"),
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
        let (target, options) = opts::parse_test(&call.args[0], &call.args[1], seconds)
            .map_err(|e| from_browser(&e))?;
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
        // All the saved tests, when the target has none, is none to run.
        if args.options.tests == opts::Tests::All
            && pages
                .test_files(s.store())
                .await
                .is_ok_and(|files| files.is_empty())
        {
            args.options.tests = opts::Tests::None;
        }
        args.found = Some((artifact, pages));
        Ok(())
    }

    fn count_key(&self, _call: &Call<'_>, args: &TestArgs) -> (&'static str, u32, &'static str) {
        (
            Capability::OpenArtifact.id(),
            args.max_per_run,
            sessions_of(SessionKind::Artifact),
        )
    }

    fn price_cap(&self, _call: &Call<'_>, args: &TestArgs) -> PriceCap {
        time_cap(args.options.timeout_s)
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
        let cx = Ctx { seen, priced };
        let f = s
            .open_and_run(
                &source,
                call,
                &cx,
                &args.options,
                &start,
                &policy,
                &args.audit,
                shown,
            )
            .await;
        counted_as(f, Capability::OpenArtifact)
    }
}

/// `f` with the entry field that counts it under `c`, when it counts.
fn counted_as(f: Finish, c: Capability) -> Finish {
    if f.counted {
        f.with(&[], &[("count_as", json!(c.id()))])
    } else {
        f
    }
}

/// The kind a source the script wrote opens, read loosely for step 3:
/// the field it holds of `url`, `artifact` and `html` (a URL when it
/// holds none of them, which step 4 then refuses).
fn kind_of_source(v: &Value) -> SessionKind {
    if v.get("artifact").is_some() {
        SessionKind::Artifact
    } else if v.get("html").is_some() {
        SessionKind::Content
    } else {
        SessionKind::Url
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Performer for Open {
    type Args = OpenArgs;

    fn capability(&self, args: &[Value]) -> Capability {
        cap_of(kind_of_source(args.first().unwrap_or(&Value::Null)))
    }

    fn shown(&self, args: &[Value]) -> Value {
        json!({"kind": kind_of_source(args.first().unwrap_or(&Value::Null)).as_str()})
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
    ) -> Result<OpenArgs, HostError> {
        let terms_b = terms_of(browser)?;
        let spec = opts::parse_source(&call.args[0]).map_err(|e| from_browser(&e))?;
        let viewport = opts::parse_open_opts(&call.args[1]).map_err(|e| from_browser(&e))?;
        let g = open_grant(terms);
        let kind = spec.kind();
        *shown = json!({"kind": kind.as_str(), "viewport": viewport.label()});
        let mut audit = vec![
            ("kind", json!(kind.as_str())),
            ("viewport", json!(viewport.label())),
        ];
        match &spec {
            SourceSpec::Url(u) => {
                shown["url"] = json!(u);
                audit.push(("url", json!(u)));
            }
            SourceSpec::Artifact { version, path } => {
                let target = version.map_or_else(|| "draft".to_string(), |n| format!("v{n}"));
                shown["target"] = json!(target);
                shown["path"] = json!(path);
                if let Some(n) = version {
                    audit.push(("version", json!(n)));
                }
                audit.push(("path", json!(path)));
            }
            SourceSpec::Content { files, .. } => {
                audit.push(("files", json!(files.len())));
            }
        }
        Ok(OpenArgs {
            spec,
            viewport,
            max_per_run: g.max_per_run,
            rule: g.url_rule.clone().unwrap_or(UrlRule::Sender),
            terms: terms_b,
            audit,
            found: None,
        })
    }

    fn noted(&self, args: &OpenArgs) -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
        (args.audit.clone(), Vec::new())
    }

    async fn admit(
        &self,
        s: &ClaxSession<'_>,
        _call: &Call<'_>,
        args: &mut OpenArgs,
        _shown: &mut Value,
    ) -> Result<(), HostError> {
        let sessions = s.sessions()?;
        s.charge_ended();
        let open = sessions.open_sessions(&s.run.id).len();
        let most = args.terms.max_open_per_run as usize;
        if open >= most {
            return Err(HostError::new(
                CODE_BROWSER_CAP,
                format!(
                    "this run has {open} browser session(s) open, the most it may hold at once; \
                     close one with page.close() first"
                ),
            ));
        }
        args.found = Some(match &args.spec {
            SourceSpec::Url(u) => {
                let scope = s
                    .browse_scope(browser_source(s).as_ref(), &args.rule)
                    .await?;
                let url = scope.allowed(u).map_err(|e| from_browser(&e))?;
                Found::Url(scope, url)
            }
            SourceSpec::Artifact { version, path } => {
                let artifact = ArtifactId::parse(&s.run.artifact_id).map_err(|e| host_error(&e))?;
                let target = BrowserTarget {
                    version: *version,
                    path: path.clone(),
                };
                let pages = s.pages(&artifact, &target).await?;
                Found::Artifact(artifact, pages, path.clone())
            }
            SourceSpec::Content { html, files } => {
                let store = s.store();
                let run = &s.run.id;
                let files = crate::browser::serve::content_files(files.clone(), |id| {
                    let row = store.bot_content_of(run, id).ok().flatten()?;
                    store.bot_content_bytes(&row).ok().flatten()
                })
                .map_err(|why| bad(format!("source.{why}")))?;
                Found::Content(html.as_bytes().to_vec(), files)
            }
        });
        Ok(())
    }

    fn count_key(&self, _call: &Call<'_>, args: &OpenArgs) -> (&'static str, u32, &'static str) {
        let kind = args.spec.kind();
        (cap_of(kind).id(), args.max_per_run, sessions_of(kind))
    }

    fn price_cap(&self, _call: &Call<'_>, args: &OpenArgs) -> PriceCap {
        time_cap(args.terms.call_seconds)
    }

    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        _call: &Call<'_>,
        args: &OpenArgs,
        seen: &Seen,
        priced: &Priced,
        shown: &mut Value,
    ) -> Finish {
        let Some(found) = &args.found else {
            unreachable!("an open is performed only once admitted")
        };
        let Ok(sessions) = s.sessions() else {
            unreachable!("an open is admitted only on a host with a session registry")
        };
        let source = browser_source(s);
        let id = new_session_id();
        let kind = args.spec.kind();
        let (start, policy): (Start, Arc<dyn RequestPolicy>) = match found {
            Found::Url(scope, url) => (
                Start::Browse {
                    url: url.as_str().to_string(),
                },
                Arc::new(BrowsePolicy::new(scope.clone(), source.clone())),
            ),
            Found::Artifact(artifact, pages, path) => {
                let site = Site::new(artifact, pages.clone());
                let start = Start::Test {
                    origin: site.origin.clone(),
                    path: path.clone(),
                };
                let policy = ArtifactPolicy::new(
                    site,
                    source.clone(),
                    s.provider.store.clone(),
                    &s.run.person,
                );
                (start, Arc::new(policy))
            }
            Found::Content(html, files) => {
                let site = ContentSite::new(&id, html.clone(), files.clone());
                (
                    Start::Content {
                        host: site.host.clone(),
                    },
                    Arc::new(ContentPolicy(site)),
                )
            }
        };
        let deadline =
            s.provider.clock.now() + chrono::Duration::seconds(i64::from(args.terms.call_seconds));
        let (opened, browser_at) = sessions
            .open_timed(
                &s.run.id,
                id,
                kind,
                start,
                policy,
                args.viewport.clone(),
                SessionTimes::from(args.terms),
                deadline,
                s.cx.cancel,
            )
            .await;
        let c = cap_of(kind);
        let f = match opened {
            Ok(o) => {
                let ms = o.at_ms.max(0);
                let (usage, cost) = match priced.usage(ms) {
                    Ok(x) => x,
                    Err(e) => {
                        s.end_unwritten(&o.session, kind, o.opened_at).await;
                        return Finish::refused(host_error(&e));
                    }
                };
                shown["session"] = json!(o.session);
                shown["ms"] = json!(ms);
                // The page's capability goes to the prelude alone (spec
                // §17.3): what the run shows and records names the session.
                let mut v = json!({
                    "page": o.handle,
                    "kind": kind.as_str(),
                    "url": o.url,
                    "title": o.title,
                    "status": o.status,
                    "viewport": args.viewport.label(),
                    "cost": cost as f64 / 1000.0,
                    "left": left(seen, Some(c), priced.credits - cost),
                });
                if let Some(e) = &o.error {
                    v["error"] = json!(e);
                }
                Finish {
                    usage: Some((usage, cost)),
                    session: Some(SessionSpend::Open {
                        session: o.session.clone(),
                        kind,
                        opened_at: o.opened_at,
                        through_ms: ms,
                    }),
                    counted: true,
                    audit: vec![("session", json!(o.session)), ("ms", json!(ms))],
                    ..Finish::new(Ok(v))
                }
            }
            Err(e) => {
                // Refused before Browser Run was asked: not counted. A
                // browser that opened and failed before its page loaded
                // is charged its time, as no session's.
                let counted = e.code != CODE_BROWSER_BUSY && e.code != CODE_CANCELLED;
                // From the WebSocket's upgrade, in milliseconds (spec
                // §10.3).
                let usage = match browser_at {
                    Some(at) if e.opened => {
                        let ms = (s.provider.clock.now() - at).num_milliseconds().max(0);
                        let cap = i64::from(args.terms.call_seconds) * 1000;
                        priced.usage(ms.min(cap)).ok()
                    }
                    _ => None,
                };
                Finish {
                    usage,
                    counted,
                    ..Finish::refused(from_browser(&e))
                }
            }
        };
        s.charge_ended();
        counted_as(f, c)
    }

    async fn after_write(
        &self,
        s: &ClaxSession<'_>,
        session: Option<&SessionSpend>,
        written: bool,
    ) {
        if let (
            false,
            Some(SessionSpend::Open {
                session,
                kind,
                opened_at,
                ..
            }),
        ) = (written, session)
        {
            s.end_unwritten(session, *kind, *opened_at).await;
        }
    }
}

impl SessionOp {
    /// The performer of `call`, a call on the session its first argument
    /// names, of that session's kind (see the module docs).
    pub(super) fn of(s: &ClaxSession<'_>, call: &Call<'_>) -> SessionOp {
        let mut p = SessionOp::unbound(call.op);
        p.session = call
            .args
            .first()
            .and_then(Value::as_str)
            .zip(s.provider.sessions.as_ref())
            .and_then(|(handle, sessions)| sessions.resolve(&s.run.id, handle).ok());
        let kind = p.session.as_deref().and_then(|id| s.session_kind(id));
        match kind {
            Some(k) => {
                p.kind = Some(k);
                p.cap = cap_of(k);
            }
            None => {
                // A session the run never had: the first source
                // capability the run holds names the refusal.
                let held = crate::capability::CapSet::parse(&s.run.caps);
                if let Some(c) = [
                    Capability::OpenUrl,
                    Capability::OpenArtifact,
                    Capability::OpenContent,
                ]
                .into_iter()
                .find(|c| held.has(*c))
                {
                    p.cap = c;
                }
            }
        }
        p
    }

    /// The performer of a call of `op` whose session is not known.
    pub(super) fn unbound(op: &str) -> SessionOp {
        let op = match op {
            crate::browser::OP_PAGE => Op::Page,
            crate::browser::OP_ACT => Op::Act,
            crate::browser::OP_EXPECT => Op::Expect,
            _ => Op::Close,
        };
        SessionOp {
            op,
            cap: Capability::OpenUrl,
            kind: None,
            session: None,
        }
    }
}

/// The action argument at `i`, or `invalid_args` naming `field`.
fn action_at<'a>(args: &'a [Value], i: usize, field: &str) -> Result<&'a str, HostError> {
    args.get(i)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("{field}: must be a string")))
}

/// `v`, an action's `args`, as sent on: an object (`{}` for none).
fn args_of(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Object(_)) => v.cloned().unwrap_or_default(),
        _ => json!({}),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Performer for SessionOp {
    type Args = CallArgs;

    fn capability(&self, _args: &[Value]) -> Capability {
        self.cap
    }

    fn shown(&self, args: &[Value]) -> Value {
        let mut v = json!({"session": self.session, "op": self.op.name()});
        if let Some(kind) = self.kind {
            v["kind"] = json!(kind.as_str());
        }
        match self.op {
            Op::Page => v["action"] = args.get(1).cloned().unwrap_or(Value::Null),
            Op::Act | Op::Expect => {
                v["locator"] = args.get(1).cloned().unwrap_or(Value::Null);
                v["action"] = args.get(2).cloned().unwrap_or(Value::Null);
            }
            Op::Close => {}
        }
        v
    }

    fn serial(&self) -> bool {
        true
    }

    fn check_args(
        &self,
        _s: &ClaxSession<'_>,
        call: &Call<'_>,
        _terms: &Terms,
        browser: Option<BrowserTerms>,
        _shown: &mut Value,
    ) -> Result<CallArgs, HostError> {
        let terms = terms_of(browser)?;
        let a = call.args;
        if !a.first().is_some_and(Value::is_string) {
            return Err(bad("page: must be a string"));
        }
        // The session the handle names, by its ID; a handle no session of
        // this run has is refused at admission.
        let session = self.session.clone().unwrap_or_default();
        let checked = |row: &'static layer::Row, v: Option<&Value>| -> Result<Value, HostError> {
            let args = args_of(v);
            if v.is_some_and(|v| !v.is_null() && !v.is_object()) {
                return Err(bad("args: must be an object"));
            }
            layer::check(row, &args).map_err(bad)?;
            Ok(args)
        };
        let locator = |v: Option<&Value>| -> Result<Value, HostError> {
            let v = v.cloned().unwrap_or(Value::Null);
            crate::browser::locate::parse_locator(&v).map_err(|e| bad(format!("locator: {e}")))?;
            Ok(v)
        };
        let (call, row) = match self.op {
            Op::Page => {
                let action = action_at(a, 1, "action")?;
                let row = layer::row(Op::Page, action, false)
                    .ok_or_else(|| bad(format!("action: `{action}` is not a page action")))?;
                let args = checked(row, a.get(2))?;
                if action == "share" || action == "revoke" {
                    let cap_call = if action == "share" {
                        share_of(&args)?
                    } else {
                        CapCall::Revoke
                    };
                    return Ok(CallArgs {
                        session,
                        call: None,
                        row: Some(row),
                        terms,
                        cap_call: Some(cap_call),
                        nav: Nav::None,
                        held: None,
                        charged_ms: 0,
                        uncharged_ms: 0,
                    });
                }
                if action == "record"
                    && !args["seconds"].as_i64().is_some_and(|n| {
                        (crate::browser::record::MIN_SECONDS..=crate::browser::record::MAX_SECONDS)
                            .contains(&n)
                    })
                {
                    return Err(bad("args.seconds: must be 1 to 10"));
                }
                (
                    Some(SessionCall::Page {
                        action: action.to_string(),
                        args,
                    }),
                    Some(row),
                )
            }
            Op::Act => {
                let locator = locator(a.get(1))?;
                let action = action_at(a, 2, "action")?;
                let row = layer::row(Op::Act, action, false)
                    .ok_or_else(|| bad(format!("action: `{action}` is not a locator action")))?;
                let args = checked(row, a.get(3))?;
                (
                    Some(SessionCall::Act {
                        locator,
                        action: action.to_string(),
                        args,
                    }),
                    Some(row),
                )
            }
            Op::Expect => {
                let on_page = a
                    .get(1)
                    .is_none_or(|l| l.is_null() || l.as_array().is_some_and(Vec::is_empty));
                let locator = if on_page {
                    None
                } else {
                    Some(locator(a.get(1))?)
                };
                let matcher = action_at(a, 2, "matcher")?;
                let row = layer::row(Op::Expect, matcher, on_page).ok_or_else(|| {
                    bad(format!(
                        "matcher: `{matcher}` is not a matcher of {}",
                        if on_page { "the page" } else { "a locator" }
                    ))
                })?;
                let args = checked(row, a.get(3))?;
                (
                    Some(SessionCall::Expect {
                        locator,
                        matcher: matcher.to_string(),
                        args,
                    }),
                    Some(row),
                )
            }
            Op::Close => (None, None),
        };
        let nav = match &call {
            Some(SessionCall::Page { action, args }) => match action.as_str() {
                "goto" => Nav::Goto(args["url"].as_str().unwrap_or_default().to_string()),
                "reload" => Nav::Reload,
                "goBack" | "goForward" => Nav::History,
                _ => Nav::None,
            },
            _ => Nav::None,
        };
        Ok(CallArgs {
            session,
            call,
            row,
            terms,
            cap_call: None,
            nav,
            held: None,
            charged_ms: 0,
            uncharged_ms: 0,
        })
    }

    fn noted(&self, args: &CallArgs) -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
        let mut audit = vec![("session", json!(self.session))];
        if let Some(row) = args.row
            && let Some(action) = row.action
        {
            audit.push(("action", json!(action)));
        }
        (audit, Vec::new())
    }

    async fn admit(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &mut CallArgs,
        _shown: &mut Value,
    ) -> Result<(), HostError> {
        // A session that ended, or that this process does not hold, is
        // refused here, before its price is checked; the ends the host
        // made are charged first.
        s.charge_ended();
        let sessions = s.sessions()?;
        let handle = call
            .args
            .first()
            .and_then(Value::as_str)
            .unwrap_or_default();
        // The page's capability allows the call (spec §18.3): its class,
        // its expiry, its origins, not revoked.
        let class = match (self.op, args.row) {
            (Op::Close, _) => Some(Class::Close),
            (_, Some(row)) => layer::class(row),
            (_, None) => None,
        };
        let (session, cap) = sessions
            .authorize(&s.run.id, handle, class, &args.nav)
            .map_err(|e| from_browser(&e))?;
        // A shared page's grant is live in the store and conveys the call's
        // action (spec §18.4), as well as its macaroon allowing it.
        if !cap.is_root() {
            let location = match &args.nav {
                Nav::Goto(url) => sessions
                    .goto_target(&s.run.id, &session, url)
                    .ok()
                    .and_then(|t| origin_of(&t)),
                _ => None,
            };
            let now = chrono::DateTime::from_timestamp_millis(sessions.now_ms())
                .unwrap_or_else(|| s.provider.clock.now());
            let conveys = s
                .store()
                .share_conveys(
                    &s.run.id,
                    cap.id(),
                    class.map(Class::as_str),
                    location.as_deref(),
                    now,
                )
                .map_err(|e| host_error(&e))?;
            if !conveys {
                return Err(HostError::new(
                    CODE_BROWSER_FORBIDDEN,
                    Refusal::Revoked.message(),
                ));
            }
        }
        args.held = Some(cap);
        // The session's time not yet charged, to its point now: what the
        // price check adds to the call's time cap (spec §10.3).
        args.charged_ms = s
            .store()
            .bot_browser_session(&s.run.id, &session)
            .map_err(|e| host_error(&e))?
            .map_or(0, |r| r.charged_ms);
        args.uncharged_ms = sessions
            .at_ms(&s.run.id, &session)
            .map_or(0, |at| (at - args.charged_ms).max(0));
        if let Some(c) = args.row.and_then(|r| r.needs)
            && self.kind.is_some_and(|k| cap_of(k) != c)
        {
            return Err(HostError::new(
                CODE_BROWSER_DISABLED,
                "page.runTests runs the clax's saved tests: only in a session opened on the run's clax",
            ));
        }
        Ok(())
    }

    fn count_key(&self, _call: &Call<'_>, _args: &CallArgs) -> (&'static str, u32, &'static str) {
        ("browser.session.call", u32::MAX, "browser session calls")
    }

    fn price_cap(&self, _call: &Call<'_>, args: &CallArgs) -> PriceCap {
        // A share or revoke reaches no browser and costs nothing (spec
        // §18.4).
        if args.cap_call.is_some() {
            return time_cap(0);
        }
        // Any other call charges the session's time since its charged
        // point through its end: the person can pay the time not yet
        // charged and the call's time cap, priced after the charged point
        // as the segment will be (spec §10.3).
        let seconds = args.terms.call_seconds;
        PriceCap {
            qty: args.uncharged_ms + i64::from(seconds) * 1000,
            after: args.charged_ms,
            costs: format!(
                "the browser session's {} s not yet charged and a call of {seconds} s can cost",
                args.uncharged_ms as f64 / 1000.0
            ),
            ..time_cap(seconds)
        }
    }

    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &CallArgs,
        seen: &Seen,
        priced: &Priced,
        shown: &mut Value,
    ) -> Finish {
        let Ok(sessions) = s.sessions() else {
            unreachable!("a session call is admitted only on a host with a session registry")
        };
        s.charge_ended();
        let run = s.run.id.as_str();
        let session = args.session.as_str();
        if let Some(cc) = &args.cap_call {
            return cap_call(s, &sessions, self.kind, call, cc, shown);
        }
        let answered = match &args.call {
            None => sessions
                .close(run, session, SessionEnd::Closed)
                .await
                .map(|at| (at, None)),
            Some(c) => {
                let deadline = s.provider.clock.now()
                    + chrono::Duration::seconds(i64::from(args.terms.call_seconds));
                let mut ctx = CallCtx::new(deadline, s.cx.cancel.clone());
                ctx.held = args
                    .held
                    .clone()
                    .map(|c| Arc::new((c, crate::browser::cap::Holder::Run(run.to_string()))));
                if self.kind == Some(SessionKind::Artifact) {
                    ctx.pages = Some(s.draft_pages());
                }
                if let SessionCall::Page { action, args: a } = c
                    && action == "goto"
                {
                    match s
                        .leave_policy(
                            &sessions,
                            session,
                            self.kind,
                            a["url"].as_str().unwrap_or_default(),
                            seen,
                        )
                        .await
                    {
                        Ok(leave) => ctx.leave = leave,
                        Err(e) => return Finish::refused(e),
                    }
                }
                sessions
                    .call(run, session, c.clone(), ctx)
                    .await
                    .map(|a| (a.at_ms, Some(a)))
            }
        };
        s.charge_ended();
        let (at_ms, answered) = match answered {
            Ok(x) => x,
            Err(e) => return Finish::refused(from_browser(&e)),
        };
        let charged = s
            .store()
            .bot_browser_session(run, session)
            .ok()
            .flatten()
            .map_or(0, |r| r.charged_ms);
        let through = at_ms.max(charged);
        let ms = through - charged;
        // Rounded once over the session: the segment costs what the
        // session's time through it costs less what it cost before it.
        let (usage, cost) = match priced.usage_after(charged, ms) {
            Ok(x) => x,
            Err(e) => return Finish::refused(host_error(&e)),
        };
        shown["ms"] = json!(ms);
        let Some(a) = answered else {
            shown["ok"] = json!(true);
            return Finish {
                usage: Some((usage, cost)),
                session: Some(SessionSpend::Close {
                    session: session.to_string(),
                    through_ms: through,
                    reason: SessionEnd::Closed,
                }),
                audit: vec![("ms", json!(through))],
                ..Finish::new(Ok(json!({"ms": through, "cost": cost as f64 / 1000.0})))
            };
        };
        let mut ok = a.ok;
        let mut error = a.error.clone();
        let mut value = a.value.clone();
        if let Some(kind) = a.kind {
            shown["kind"] = json!(kind.as_str());
        }
        if value.get("record").is_some() {
            match s.keep_clip(call, &value["record"], &a.shots) {
                Ok(clip) => {
                    shown["clip"] = clip["clip"].clone();
                    value = clip;
                }
                Err(why) => {
                    ok = false;
                    error = Some(why);
                    value = Value::Null;
                }
            }
        } else if !a.shots.is_empty() {
            let kept: Vec<Kept> = a
                .shots
                .iter()
                .map(|png| s.keep_screenshot(call, png))
                .collect();
            shown["screenshots"] = json!(
                kept.iter()
                    .filter_map(|k| k.as_ref().ok().map(|h| h["id"].clone()))
                    .collect::<Vec<_>>()
            );
            if let Some(i) = value["screenshot"].as_u64() {
                match kept.get(usize::try_from(i).unwrap_or(usize::MAX)) {
                    Some(Ok(handle)) => value = handle.clone(),
                    Some(Err(why)) => {
                        ok = false;
                        error = Some(why.clone());
                        value = Value::Null;
                    }
                    None => {}
                }
            }
        }
        shown["ok"] = json!(ok);
        let mut v = json!({"ok": ok, "url": a.url, "ms": ms, "cost": cost as f64 / 1000.0});
        if ok {
            v["value"] = value;
        } else {
            v["error"] = json!(error.unwrap_or_default());
            if a.timeout {
                v["timeout"] = json!(true);
            }
        }
        let f = Finish {
            usage: Some((usage, cost)),
            session: Some(SessionSpend::Call {
                session: session.to_string(),
                through_ms: through,
                kind: a.kind,
            }),
            // A goto out made the session one on a URL: counted as an
            // open on one.
            counted: a.kind == Some(SessionKind::Url),
            audit: vec![("ok", json!(ok)), ("ms", json!(ms))],
            ..Finish::new(Ok(v))
        };
        counted_as(f, Capability::OpenUrl)
    }

    async fn out_of_credits(&self, s: &ClaxSession<'_>, call: &Call<'_>) {
        let (Ok(sessions), Some(session)) = (s.sessions(), self.session.as_deref()) else {
            return;
        };
        if is_cap_call(call) {
            return;
        }
        let _ = sessions
            .close(&s.run.id, session, SessionEnd::Credits)
            .await;
        s.charge_ended();
    }

    async fn after_write(
        &self,
        s: &ClaxSession<'_>,
        session: Option<&SessionSpend>,
        written: bool,
    ) {
        // A close whose Spend was not written has ended the session in the
        // registry, which charges a script's close to the close's own
        // Spend: its last segment is charged here, once (the store charges
        // only while the row is open).
        if let (
            false,
            Some(SessionSpend::Close {
                session,
                through_ms,
                reason,
            }),
        ) = (written, session)
            && let Err(e) =
                s.store()
                    .end_bot_browser_session(&s.run.id, session, *through_ms, *reason)
        {
            tracing::warn!(
                "clax-bot: run {}: browser session {session} closed and could not be charged: {e}",
                s.run.id
            );
        }
    }
}

/// A `share` or `revoke` of the capability `call` names (spec §18.4):
/// minted by the registry, its grant row written (a share) or revoked (a
/// revoke) in the call's Spend transaction, uncharged, journaled and
/// audited with the capabilities' IDs and never the capabilities (§18.9).
/// A share of a session's root has for parent the person's grant to their
/// runs of the session's open action (`kind`'s); a share of a share, that
/// share's grant.
fn cap_call(
    s: &ClaxSession<'_>,
    sessions: &BrowserSessions,
    kind: Option<SessionKind>,
    call: &Call<'_>,
    cc: &CapCall,
    shown: &mut Value,
) -> Finish {
    let run = s.run.id.as_str();
    let handle = call
        .args
        .first()
        .and_then(Value::as_str)
        .unwrap_or_default();
    let answer = |value: Value| json!({"ok": true, "url": "", "ms": 0, "cost": 0, "value": value});
    match cc {
        CapCall::Share {
            actions,
            expires,
            origins,
        } => {
            let mut more = Vec::new();
            if let Some(actions) = actions {
                more.push(Caveat::Actions(actions.iter().copied().collect()));
            }
            let expires_ms =
                expires.map(|secs| sessions.now_ms().saturating_add(i64::from(secs) * 1000));
            if let Some(ms) = expires_ms {
                more.push(Caveat::Expires(ms));
            }
            if let Some(os) = origins {
                more.push(Caveat::Origin(os.iter().cloned().collect()));
            }
            let child = match sessions.share(run, handle, &more) {
                Ok(c) => c,
                Err(e) => return Finish::refused(from_browser(&e)),
            };
            let parent = child.chain()[child.chain().len() - 2].clone();
            let parent_grant = if child.chain().len() == 2 {
                let held = kind.map(cap_of).and_then(|c| {
                    s.store()
                        .person_caps(&s.run.person)
                        .ok()
                        .and_then(|h| h.get(c.id()).and_then(|p| p.runs_grant.clone()))
                });
                match held {
                    Some(g) => g,
                    None => {
                        return Finish::refused(HostError::new(
                            CODE_BROWSER_FORBIDDEN,
                            Refusal::Revoked.message(),
                        ));
                    }
                }
            } else {
                parent.clone()
            };
            let actions_held: Vec<String> = match actions {
                Some(a) => a.iter().map(|c| c.as_str().to_string()).collect(),
                None => Class::ALL.iter().map(|c| c.as_str().to_string()).collect(),
            };
            let grant = GrantWrite::Share(ShareGrant {
                id: child.id().to_string(),
                run: run.to_string(),
                parent: parent_grant,
                actions: actions_held,
                origins: origins.clone(),
                expires_at: expires_ms
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            });
            shown["capability"] = json!(child.id());
            shown["ok"] = json!(true);
            Finish {
                grant: Some(grant),
                audit: vec![
                    ("capability", json!(child.id())),
                    ("parent", json!(parent)),
                    (
                        "actions",
                        json!(
                            actions
                                .as_ref()
                                .map(|o| o.iter().map(|c| c.as_str()).collect::<Vec<_>>())
                        ),
                    ),
                    ("expires", json!(expires_ms)),
                    ("origins", json!(origins)),
                ],
                ..Finish::new(Ok(answer(
                    json!({"page": child.encode(), "shown": child.shown()}),
                )))
            }
        }
        CapCall::Revoke => match sessions.revoke(run, handle) {
            Ok(cap) => {
                shown["capability"] = json!(cap.id());
                shown["ok"] = json!(true);
                Finish {
                    grant: Some(GrantWrite::Revoke(cap.id().to_string())),
                    audit: vec![("capability", json!(cap.id()))],
                    ..Finish::new(Ok(answer(Value::Null)))
                }
            }
            Err(e) => Finish::refused(from_browser(&e)),
        },
    }
}

impl ClaxSession<'_> {
    /// The host's session registry, or `browser_unavailable` on a host
    /// that holds none.
    fn sessions(&self) -> Result<Arc<BrowserSessions>, HostError> {
        self.provider.sessions.clone().ok_or_else(|| {
            HostError::new(
                CODE_BROWSER_UNAVAILABLE,
                "this host holds no browser sessions; use clax.browser.test",
            )
        })
    }

    /// Ends session `session`, of `kind`, opened at `opened_at`, whose
    /// open was not written: no script holds its handle, so it is closed
    /// `dropped` and its whole time charged once, its row made closed
    /// ([`clax_store::Store::end_unwritten_bot_browser_session`]); else it
    /// would run uncharged and hold one of the run's open sessions.
    async fn end_unwritten(
        &self,
        session: &str,
        kind: SessionKind,
        opened_at: chrono::DateTime<chrono::Utc>,
    ) {
        let Ok(sessions) = self.sessions() else {
            return;
        };
        // The registry's end finds no row to charge; the store's makes it.
        let at_ms = sessions
            .close(&self.run.id, session, SessionEnd::Dropped)
            .await
            .unwrap_or(0);
        self.charge_ended();
        if let Err(e) = self.store().end_unwritten_bot_browser_session(
            &self.run.id,
            session,
            kind,
            opened_at,
            at_ms,
            SessionEnd::Dropped,
        ) {
            tracing::warn!(
                "clax-bot: run {}: browser session {session}, its open not written, could not be charged: {e}",
                self.run.id
            );
        }
    }

    /// Charges the sessions the host ended since the last look (idle,
    /// lifetime, a revoke, credits, a run's end, a lost browser): each
    /// one's tail, once ([`clax_store::Store::end_bot_browser_session`]).
    pub(super) fn charge_ended(&self) {
        if let Some(sessions) = &self.provider.sessions {
            charge_ended(&self.provider.store, sessions);
        }
    }

    /// The kind of the run's session `id`: the registry's while it holds
    /// it, else the store's row's.
    fn session_kind(&self, id: &str) -> Option<SessionKind> {
        if let Some(sessions) = &self.provider.sessions
            && let Some((_, k)) = sessions
                .open_sessions(&self.run.id)
                .into_iter()
                .find(|(s, _)| s == id)
        {
            return Some(k);
        }
        self.store()
            .bot_browser_session(&self.run.id, id)
            .ok()
            .flatten()
            .map(|r| r.kind)
    }

    /// A recording's answer (spec §6.6, [`crate::browser::record`]): each
    /// frame of `frames` (JPEG) kept as an item of the run's content, the
    /// clip's manifest as one more, and `{frames, clip, ms, count}`, at
    /// most [`crate::browser::record::SHOWN`] frames evenly spaced.
    /// `record` is the driver's `{at, ms, width, height}`.
    fn keep_clip(
        &self,
        call: &Call<'_>,
        record: &Value,
        frames: &[Vec<u8>],
    ) -> Result<Value, String> {
        use crate::browser::record::{MANIFEST_MEDIA_TYPE, manifest, shown};
        let mut handles = Vec::with_capacity(frames.len());
        let mut items = Vec::with_capacity(frames.len());
        for (i, jpeg) in frames.iter().enumerate() {
            let h = self.keep_item(call, jpeg, "image/jpeg", "recording")?;
            let at = record["at"][i].as_i64().unwrap_or(0);
            items.push((h["id"].as_str().unwrap_or_default().to_string(), at));
            handles.push(h);
        }
        let side = |k: &str| u32::try_from(record[k].as_u64().unwrap_or(0)).unwrap_or(0);
        let m = manifest(side("width"), side("height"), &items);
        let clip = self.keep_item(
            call,
            m.to_string().as_bytes(),
            MANIFEST_MEDIA_TYPE,
            "recording",
        )?;
        let picked: Vec<Value> = shown(handles.len())
            .into_iter()
            .map(|i| handles[i].clone())
            .collect();
        Ok(
            json!({"frames": picked, "clip": clip["id"], "ms": record["ms"], "count": handles.len()}),
        )
    }

    /// The run's draft as this step holds it now.
    fn draft_pages(&self) -> Pages {
        let st = self.lock();
        Pages::Draft(st.draft.files().clone(), st.draft.base_version)
    }

    /// What a `goto` of `url` in session `session`, of kind `kind`, is
    /// given (spec §6.3). In a URL session the URL, resolved against the
    /// page's, passes `clax-browser:open-url`'s rule as an open's does (else the
    /// rule's refusal) and the session keeps its policy (none). Out of an
    /// artifact or content session's origin, the URL session's policy when
    /// the run declared `clax-browser:open-url`, it is live and its rule admits
    /// the URL (else the rule's refusal) and the run has a session on a URL
    /// left under its `max_per_run` (else `browser_cap`); none when the run
    /// did not declare it or it is not live (the session then refuses it
    /// `browser_disabled`), and none for a `goto` within the origin.
    async fn leave_policy(
        &self,
        sessions: &BrowserSessions,
        session: &str,
        kind: Option<SessionKind>,
        url: &str,
        seen: &Seen,
    ) -> Result<Option<Arc<dyn RequestPolicy>>, HostError> {
        let run = &self.run.id;
        let in_url_session = kind == Some(SessionKind::Url);
        if !in_url_session
            && !sessions
                .leaves(run, session, url)
                .map_err(|e| from_browser(&e))?
        {
            return Ok(None);
        }
        // Declared by the run, then live: a grant made since the run
        // started is not added (spec §4.4).
        if !crate::capability::CapSet::parse(&self.run.caps).has(Capability::OpenUrl) {
            return Ok(None);
        }
        let Ok(terms) = seen.caps.get(Capability::OpenUrl) else {
            return Ok(None);
        };
        let target = sessions
            .goto_target(run, session, url)
            .map_err(|e| from_browser(&e))?;
        let rule = open_grant(terms)
            .url_rule
            .clone()
            .unwrap_or(UrlRule::Sender);
        let source = browser_source(self);
        let scope = self.browse_scope(source.as_ref(), &rule).await?;
        scope.allowed(&target).map_err(|e| from_browser(&e))?;
        if in_url_session {
            return Ok(None);
        }
        // A goto out makes a session on a URL: counted as an open on one
        // (spec §11.1).
        let max = open_grant(terms).max_per_run;
        if seen.count(Capability::OpenUrl.id()) >= max {
            return Err(HostError::new(
                CODE_BROWSER_CAP,
                format!(
                    "this run has used its {max} {}; work with what you have",
                    sessions_of(SessionKind::Url)
                ),
            ));
        }
        Ok(Some(Arc::new(BrowsePolicy::new(scope, source))))
    }
}

/// Charges, in `store`, the ends `sessions` made since the last look
/// ([`charge`]).
pub fn charge_ended(store: &clax_store::Store, sessions: &BrowserSessions) {
    charge(store, sessions.ended());
}

/// Charges, in `store`, each of `ended`, ends a host's registry made: its
/// tail on its run's open step or on none, once
/// (`clax_store::Store::end_bot_browser_session`). A host hands it to its
/// registry ([`BrowserSessions::charge_with`]).
pub fn charge(store: &clax_store::Store, ended: Vec<crate::browser::live::Ended>) {
    for e in ended {
        if let Err(err) = store.end_bot_browser_session(&e.run, &e.session, e.at_ms, e.reason) {
            tracing::warn!(
                "clax-bot: run {}: browser session {} ended ({}) and could not be charged: {err}",
                e.run,
                e.session,
                e.reason.as_str()
            );
        }
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
        let qty = ms.min(i64::from(options.timeout_s) * 1000);
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
        let f = counted_as(f, Capability::OpenArtifact);
        let _ = spend::write(self, call, Capability::OpenArtifact, false, shown, f);
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

    /// How a test whose browser opened ends: charged its time in
    /// milliseconds, at most the call's time cap (what the credits
    /// check priced), and counted; its result completed (spec §6.8: no
    /// steps, screenshots, `html` or `text`; `cost` and `left` added); and
    /// the `bot.spend` record's `ms` and `passed`.
    fn tested(
        &self,
        _call: &Call<'_>,
        cx: &Ctx<'_>,
        options: &BrowserOpts,
        tested: TestRun,
        shown: &mut Value,
    ) -> Finish {
        let ms = (tested.closed_at - tested.opened_at)
            .num_milliseconds()
            .max(0);
        let qty = ms.min(i64::from(options.timeout_s) * 1000);
        let (usage, cost) = match cx.priced.usage(qty) {
            Ok(x) => x,
            Err(e) => return Finish::refused(host_error(&e)),
        };
        shown["ms"] = json!(ms);
        let mut more = vec![("ms", json!(ms))];
        let res = match tested.outcome {
            Ok(mut v) => {
                if let Some(o) = v.as_object_mut() {
                    for k in ["steps", "screenshots", "html", "text"] {
                        o.remove(k);
                    }
                }
                v["cost"] = json!(cost as f64 / 1000.0);
                v["left"] = left(
                    cx.seen,
                    Some(Capability::OpenArtifact),
                    cx.priced.credits - cost,
                );
                summarize(shown, &v);
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
        self.keep_item(call, png, "image/png", "screenshot")
    }

    /// Keeps `bytes` of `media_type` as an item of the run's content,
    /// noted as `call`'s: its handle, or why the `what` could not be kept.
    fn keep_item(&self, call: &Call<'_>, bytes: &[u8], media_type: &str, what: &str) -> Kept {
        let store = self.store();
        let size = clax_core::media::sniff(bytes).ok();
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
            media_type,
            width: size.as_ref().map(|s| s.width),
            height: size.as_ref().map(|s| s.height),
            bytes,
        };
        let row = store
            .put_bot_content(&new, self.provider.content_limits)
            .map_err(|e| format!("the {what} could not be kept: {e}"))?;
        store
            .note_bot_content(&row.id, call.op, &call.place.key, call.place.step, None)
            .map_err(|e| format!("the {what} could not be kept: {e}"))?;
        Ok(row.handle())
    }
}

/// What the run shows of a test's result `v`, into `shown` (see the
/// module docs).
fn summarize(shown: &mut Value, v: &Value) {
    let len = |x: &Value| x.as_array().map_or(0, Vec::len);
    shown["passed"] = v["passed"].clone();
    if let Some(t) = v.get("tests").filter(|t| !t.is_null()) {
        shown["tests"] = json!({"passed": t["passed"], "failed": len(&t["failed"])});
    }
    shown["errors"] = json!(len(&v["errors"]));
    shown["console"] = json!(len(&v["console"]));
    shown["blocked"] = json!(len(&v["requests"]["blocked"]));
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_counts_what_the_panel_shows() {
        let mut shown = json!({"target": "draft"});
        let v = json!({
            "passed": false,
            "tests": {"passed": 2, "failed": [{"name": "a", "error": "b"}], "skipped": 0},
            "errors": [{"message": "m"}],
            "console": [],
            "requests": {"failed": [], "blocked": [{"url": "u", "why": "origin"}]},
        });
        summarize(&mut shown, &v);
        assert_eq!(
            shown,
            json!({"target": "draft", "passed": false,
                   "tests": {"passed": 2, "failed": 1}, "errors": 1, "console": 0,
                   "blocked": 1})
        );
    }
}

//! The `clax.web.*` performer ([`crate::api::Performer::Web`], spec
//! 2026-11-02-claxbot-web-tools §4.2, §5, §6, §7.2, §8). A web call made by
//! a script goes through these checks in order, and the first that fails
//! answers the script:
//!
//! 1. A call journaled at its place answers from the journal.
//! 2. The run declares the operation ([`crate::web::WebShape`]) and the
//!    host has a source ([`crate::run::Deps::web`]), or `web_disabled`.
//! 3. The operation is still granted now ([`WebAccess::now`]: the admin
//!    or the person may have turned it off since the run started, or the
//!    source lost what it needs, [`crate::web::WebSource::configured`]),
//!    or `web_disabled`.
//! 4. Its arguments hold (a query of 1 to 1,024 characters, a `limit` and
//!    an `offset` not below 0, a URL), or `invalid_args`.
//! 5. A fetch: the URL is a [`PublicUrl`], its host is not one of the
//!    instance's own ([`crate::web::WebSource::own_hosts`]), the admin's
//!    [`UrlRule`] lets its host be reached, a source holds it ([`sources`]) unless the rule
//!    is [`UrlRule::AnyPublic`], and a URL that looks credentialed
//!    ([`looks_credentialed`]) is in the sender's own words; else
//!    `url_not_allowed` or `url_not_in_prior_context`. When the request
//!    cannot be read, `web_unavailable` (retryable).
//! 6. A fetch of a URL the run's page store keeps answers its window from
//!    the store: free, and not counted against the cap. It must ask for
//!    the format the page was fetched in, or `invalid_args`.
//! 7. The run's cap for the operation has room, or `web_cap`. The count
//!    is of the run's calls that reached the source (its journaled web
//!    calls marked `counted`), charged or not: a search that failed and a
//!    fetch that never went out count, though neither is charged.
//! 8. The operation's cost line exists (else `web_unavailable`: web
//!    pricing is not configured), and the person can pay for one request
//!    at its rate (at most their credits left this week, unless their
//!    credits are not enforced), or `credits_exhausted`.
//! 9. The source is called, with the run's cancel token and, for a fetch,
//!    the host check of every redirect target (the instance's own hosts
//!    and the rule's host part, as in check 5, spec §5.3; the source
//!    refuses an `https` to `http` hop itself).
//!
//! # What is written
//!
//! Every call past check 2 is journaled with its `bot.web` record in one
//! transaction ([`Store::spend_journaled`]): refusals and windows from the
//! page store too, uncharged, so every URL a script tried is in the audit
//! trail. A call the source answered is charged there (when it succeeded,
//! or when [`charged`] says its failure is), at the rate check 8 read,
//! with the page a fetch received (its text cut at [`STORE_BYTES`]). A
//! refusal at check 2 is journaled alone.
//!
//! Each entry keeps, beside the script's value (`value`) or the error
//! (`code`, `message`, and `retryable` when it is): `cost_milli`, the
//! charge; `web`, what the run panel shows (`{query}`, or `{url, source}`
//! once the URL parsed); `counted`, true when the call reached the source
//! (check 9), which check 7 counts; and, for a fetch whose URL had a
//! source, `source` (the [`UrlSource`] that allowed it,
//! [`UrlSource::to_json`]) and `format`. A call answered from the journal
//! shows the same and is not counted again.
//!
//! Every answer carries `left`: the run's searches and fetches left under
//! the caps after the call, and the person's credits left this week as a
//! number of credits. A script's error carries the [`WebError`]'s
//! `retryable` flag ([`HostError::retryable`]), and a retryable error's
//! message also says to try again, for a model that reads only the message.
//!
//! [`Store::spend_journaled`]: clax_store::Store::spend_journaled

use super::{RunHost, recorded};
use crate::api::CODE_BAD_ARGS;
use crate::host::host_error;
use crate::web::page::{PAGE_CHARS, SNIPPET_CHARS, STORE_BYTES, snippet, window};
use crate::web::url::{OwnHosts, looks_credentialed};
use crate::web::{
    CODE_CREDITS_EXHAUSTED, CODE_URL_NOT_ALLOWED, CODE_URL_NOT_IN_PRIOR_CONTEXT, CODE_WEB_CAP,
    CODE_WEB_DISABLED, CODE_WEB_UNAVAILABLE, FetchGrant, Format, OP_FETCH, OP_SEARCH, PublicUrl,
    SearchAnswer, SearchGrant, SearchQuery, UrlRule, UrlSource, WebAccess, WebError, WebGrant,
    WebShape, WebSource, access::sources, charged,
};
use clax_bot_sandbox::HostError;
use clax_core::audit::{AuditKind, AuditRecord};
use clax_store::bot_runs::CallEntry;
use clax_store::bot_web::{Spend, WebPageRow};
use clax_store::credits::{
    Applies, KeySource, LINE_WEB_FETCH, Metered, Rate, row_credits_milli, web_search_line,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// The longest query, in characters.
const MAX_QUERY_CHARS: usize = 1_024;

/// What a web call answered, what it charged (`None` when the journal
/// could not be written) and what the run panel shows of it.
pub(super) struct WebDone {
    pub res: Result<Value, HostError>,
    pub cost_milli: Option<i64>,
    pub web: Value,
}

/// How a web call ends, before it is written.
struct Finish {
    res: Result<Value, HostError>,
    /// What to charge, at the rate checked, and its credits.
    usage: Option<(Metered, i64)>,
    page: Option<WebPageRow>,
    /// Written with a `bot.web` record: every call past check 2.
    audited: bool,
    /// Fields the `bot.web` record adds (`provider`, `request_id`, …).
    extra: Vec<(&'static str, Value)>,
    source: Option<UrlSource>,
    format: Option<Format>,
    /// Whether the call reached the source (check 9), and so counts
    /// against the run's cap.
    counted: bool,
}

impl Finish {
    /// A call past check 2 that answered `res`, uncharged.
    fn new(res: Result<Value, HostError>) -> Finish {
        Finish {
            res,
            usage: None,
            page: None,
            audited: true,
            extra: Vec::new(),
            source: None,
            format: None,
            counted: false,
        }
    }

    /// A refusal past check 2.
    fn refused(e: HostError) -> Finish {
        Finish::new(Err(e))
    }

    /// A refusal at check 2: journaled, not audited.
    fn undeclared(e: HostError) -> Finish {
        Finish {
            audited: false,
            ..Finish::refused(e)
        }
    }

    /// This finish, for a fetch of `format` that `src` allowed.
    fn of_fetch(mut self, src: &UrlSource, format: Format) -> Finish {
        self.source = Some(src.clone());
        self.format = Some(format);
        self
    }
}

fn format_str(f: Format) -> &'static str {
    match f {
        Format::Auto => "auto",
        Format::Raw => "raw",
    }
}

/// `text` cut to at most `max` bytes at a character boundary, and its whole
/// length in characters.
fn cut(text: &str, max: usize) -> (String, i64) {
    let total = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
    if text.len() <= max {
        return (text.to_string(), total);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), total)
}

/// Why a fetch may not reach `host`, on the URL asked for (check 5) or a
/// redirect target (the `hop` check): it is one of the instance's own
/// hosts (`own`, spec §5.3), or the rule does not let a fetch reach it
/// ([`UrlRule::allows_host`]). `None` when it may.
fn host_refused(rule: &UrlRule, own: &OwnHosts, host: &str) -> Option<&'static str> {
    if own.holds(host) {
        Some("this instance's own host, which a fetch never reaches; use another source")
    } else if !rule.allows_host(host) {
        Some("a host this instance does not fetch from")
    } else {
        None
    }
}

/// The most bytes of a provider's search response a `bot.web` audit record
/// keeps whole.
const AUDIT_RESPONSE_BYTES: usize = 64 << 10;

/// What a search's `bot.web` audit record keeps of the provider's
/// response, so a row stays small whatever a provider answers: the whole
/// response when its JSON is at most [`AUDIT_RESPONSE_BYTES`], else only
/// each hit's `url`, `title` and `age` and the `requestId`, marked
/// `truncated`.
fn audit_response(a: &SearchAnswer) -> Value {
    if a.raw.to_string().len() <= AUDIT_RESPONSE_BYTES {
        return a.raw.clone();
    }
    let items: Vec<Value> = a
        .hits
        .iter()
        .map(|h| json!({"url": h.url, "title": h.title, "age": h.age}))
        .collect();
    json!({"items": items, "requestId": a.request_id, "truncated": true})
}

/// What the script sees of a search answer: at most `limit` results with
/// a URL, each with a title (the URL when missing) and a snippet cut at a
/// word boundary.
fn results_of(a: &SearchAnswer, limit: u8) -> Vec<Value> {
    a.hits
        .iter()
        .filter(|h| !h.url.trim().is_empty())
        .take(usize::from(limit))
        .map(|h| {
            let title = h
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(&h.url);
            let mut r = json!({
                "url": h.url,
                "title": title,
                "snippet": snippet(h.description.as_deref().unwrap_or(""), SNIPPET_CHARS),
            });
            if let Some(age) = &h.age {
                r["age"] = json!(age);
            }
            r
        })
        .collect()
}

/// A page's window from character `offset`, as `web.fetch` answers it.
fn page_value(p: &WebPageRow, offset: usize, left: Value) -> Value {
    let w = window(&p.text, offset, PAGE_CHARS);
    json!({
        "url": p.url,
        "finalUrl": p.final_url,
        "status": p.status,
        "contentType": p.content_type,
        "text": w.text,
        "offset": w.offset,
        "nextOffset": w.next_offset,
        "totalChars": p.total_chars,
        "left": left,
    })
}

/// `e` as the script sees it: a retryable error is
/// [`HostError::retryable`], and its message says to try again.
fn from_web(e: &WebError) -> HostError {
    if e.retryable {
        HostError::new(e.code, format!("{} (retryable: try again)", e.message)).retryable()
    } else {
        HostError::new(e.code, e.message.clone())
    }
}

/// The journal entry's result of a web call (see the module docs).
fn entry_result(f: &Finish, cost: i64, shown: &Value) -> Value {
    let mut v = match &f.res {
        Ok(v) => json!({"value": v}),
        Err(e) => super::err_json(e),
    };
    v["cost_milli"] = json!(cost);
    v["web"] = shown.clone();
    if f.counted {
        v["counted"] = json!(true);
    }
    if let Some(src) = &f.source {
        v["source"] = src.to_json();
    }
    if let Some(fmt) = f.format {
        v["format"] = json!(format_str(fmt));
    }
    v
}

/// A web call as its journal entry `e` answers it; `shown` when the entry
/// keeps no `web`.
fn from_entry(e: &CallEntry, shown: Value) -> WebDone {
    let r = e.result();
    WebDone {
        res: if e.ok() {
            Ok(r.get("value").cloned().unwrap_or(Value::Null))
        } else {
            recorded(e)
        },
        cost_milli: Some(r["cost_milli"].as_i64().unwrap_or(0)),
        web: r.get("web").cloned().unwrap_or(shown),
    }
}

/// The credits of one request at `rate` on a `ks` key, as
/// [`clax_store::Store::one_request_milli`] prices it and the charge of a
/// usage at that rate comes to.
fn one_request(rate: &Rate, ks: KeySource) -> clax_core::Result<i64> {
    if ks == KeySource::Own && rate.applies == Applies::InstanceKey {
        return Ok(0);
    }
    row_credits_milli(
        1,
        rate.cost_micro_usd,
        rate.margin_bp,
        rate.fixed_credits_milli,
        rate.credit_price_micro_usd,
        rate.unit.base(),
    )
}

fn cap(max: u32, what: &str) -> HostError {
    HostError::new(
        CODE_WEB_CAP,
        format!("this run has used its {max} {what}; work with what you have"),
    )
}

/// The arguments of a call, checked against the grant.
enum Call {
    Search {
        grant: SearchGrant,
        query: SearchQuery,
    },
    Fetch {
        grant: FetchGrant,
        url: PublicUrl,
        offset: usize,
        format: Format,
    },
}

/// The arguments of `op`, checked and bounded by `grant`.
fn call_of(op: &str, args: &[Value], grant: &WebGrant) -> Result<Call, HostError> {
    let bad = |m: String| HostError::new(CODE_BAD_ARGS, format!("clax.{op}: {m}"));
    let off = || {
        HostError::new(
            CODE_WEB_DISABLED,
            format!("the admin turned clax.{op} off since this run started; do without it"),
        )
    };
    if op == OP_SEARCH {
        let g = grant.search.clone().ok_or_else(off)?;
        let query = args[0].as_str().unwrap_or("").to_string();
        let n = query.chars().count();
        if n == 0 || n > MAX_QUERY_CHARS {
            return Err(bad(format!(
                "query must be 1 to {MAX_QUERY_CHARS} characters, got {n}"
            )));
        }
        let limit = match args.get(1).and_then(Value::as_i64) {
            None => g.limit,
            Some(l) if l < 1 => return Err(bad(format!("limit must be at least 1, got {l}"))),
            Some(l) => u8::try_from(l).unwrap_or(u8::MAX).min(g.limit),
        };
        let query = SearchQuery {
            query,
            limit,
            provider: g.provider,
            gateway: g.gateway.clone(),
            byok_alias: g.byok_alias.clone(),
        };
        return Ok(Call::Search { grant: g, query });
    }
    let g = grant.fetch.clone().ok_or_else(off)?;
    let url = PublicUrl::parse(args[0].as_str().unwrap_or("")).map_err(|e| from_web(&e))?;
    let opts = &args[1];
    let offset = match opts["offset"].as_i64() {
        None => 0,
        Some(o) if o < 0 => return Err(bad(format!("offset must be at least 0, got {o}"))),
        Some(o) => usize::try_from(o).unwrap_or(usize::MAX),
    };
    let format = match opts["format"].as_str() {
        Some("raw") => Format::Raw,
        _ => Format::Auto,
    };
    Ok(Call::Fetch {
        grant: g,
        url,
        offset,
        format,
    })
}

/// What the checks after the arguments read: the call's place, the grant
/// now, the run's journaled web calls, its counted calls ([`counts`]) and
/// its source.
struct Ctx<'g> {
    op: &'g str,
    seq: i64,
    grant: WebGrant,
    journal: Vec<CallEntry>,
    searches: u32,
    fetches: u32,
    source: &'g Arc<dyn WebSource>,
}

/// The searches and fetches in `journal` (a run's web calls) that reached
/// the source, charged or not: the calls the run's caps count.
fn counts(journal: &[CallEntry]) -> (u32, u32) {
    let count = |op: &str| {
        let n = journal
            .iter()
            .filter(|e| e.op() == op && e.result()["counted"] == json!(true))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    };
    (count(OP_SEARCH), count(OP_FETCH))
}

impl Ctx<'_> {
    /// `left` once the run made `searches` and `fetches` counted calls,
    /// with `credits_milli` left this week.
    fn left(&self, searches: u32, fetches: u32, credits_milli: i64) -> Value {
        let under = |g: Option<u32>, n: u32| g.map_or(0, |max| max.saturating_sub(n));
        json!({
            "searches": under(self.grant.search.as_ref().map(|g| g.max_per_run), searches),
            "fetches": under(self.grant.fetch.as_ref().map(|g| g.max_per_run), fetches),
            "credits": credits_milli.max(0) as f64 / 1000.0,
        })
    }
}

/// What a charged call costs: the usage at the rate checked, its credits
/// and the person's credits left before it.
struct Priced {
    usage: Metered,
    price: i64,
    credits: i64,
}

impl RunHost<'_> {
    /// Performs web call `op` (`raw`, its arguments as the script passed
    /// them, already checked as `args`) at place `seq`; see the module
    /// documentation.
    pub(super) async fn web(
        &self,
        op: &str,
        args: &[Value],
        raw: &Value,
        seq: i64,
        replay: Option<&CallEntry>,
    ) -> WebDone {
        let mut shown = if op == OP_SEARCH {
            json!({"query": args[0]})
        } else {
            json!({"url": args[0]})
        };
        if let Some(e) = replay {
            return from_entry(e, shown);
        }
        let f = self.web_fresh(op, args, seq, &mut shown).await;
        self.write(op, raw, seq, shown, f)
    }

    /// Journals `f` at place `seq`, with its `bot.web` record and charge
    /// when audited, and answers what the journal holds.
    fn write(&self, op: &str, raw: &Value, seq: i64, shown: Value, f: Finish) -> WebDone {
        let tick = self.tick;
        let cost = f.usage.as_ref().map_or(0, |(_, c)| *c);
        let entry = match tick.entry(self.attempt, seq, op, raw) {
            Ok(e) => e.answered(f.res.is_ok(), entry_result(&f, cost, &shown)),
            Err(e) => {
                return WebDone {
                    res: Err(e),
                    cost_milli: None,
                    web: shown,
                };
            }
        };
        if !f.audited {
            if let Err(e) = tick.store.record_bot_call(&entry) {
                tracing::warn!(
                    "clax-bot: journaling call {seq} of run {}: {e}",
                    tick.run.id
                );
            }
            return from_entry(&entry, shown);
        }
        let mut audit = AuditRecord::new(AuditKind::BotWeb, tick.store.now()).with("op", op);
        for (k, v) in shown.as_object().into_iter().flatten() {
            audit = audit.with(k, v.clone());
        }
        audit = match &f.res {
            Ok(_) => audit.with("ok", true),
            Err(e) => audit.with("ok", false).with("code", e.code.as_str()),
        };
        for (k, v) in f.extra {
            audit = audit.with(k, v);
        }
        let spend = Spend {
            entry,
            usage: f.usage.map(|(m, _)| m),
            page: f.page,
            audit,
        };
        match tick.store.spend_journaled(&tick.run, tick.driver, spend) {
            Ok(spent) => {
                // Charged at the rate checked, the charge is its price.
                debug_assert!(spent.replayed || spent.credits_milli == cost);
                from_entry(&spent.entry, shown)
            }
            Err(e) => WebDone {
                res: Err(host_error(&e)),
                cost_milli: None,
                web: shown,
            },
        }
    }

    /// Checks 2 to 9 of a call not in the journal.
    async fn web_fresh(&self, op: &str, args: &[Value], seq: i64, shown: &mut Value) -> Finish {
        let tick = self.tick;
        let run = &tick.run;
        let shape = WebShape::parse(&run.web);
        let declared = if op == OP_SEARCH {
            shape.search
        } else {
            shape.fetch
        };
        let source = match &tick.deps.web {
            Some(s) if declared => s,
            _ => {
                return Finish::undeclared(HostError::new(
                    CODE_WEB_DISABLED,
                    format!("clax.{op} is not available to this run"),
                ));
            }
        };
        let grant = match WebAccess::now(tick.store, &run.person, source.configured()) {
            Ok(WebAccess::On(g)) => g,
            Ok(WebAccess::Off(why)) => {
                return Finish::refused(HostError::new(
                    CODE_WEB_DISABLED,
                    format!(
                        "web use was turned off ({}) since this run started; do without it",
                        why.as_str()
                    ),
                ));
            }
            Err(e) => return Finish::refused(host_error(&e)),
        };
        let call = match call_of(op, args, &grant) {
            Ok(c) => c,
            Err(e) => return Finish::refused(e),
        };
        let journal = match tick.store.bot_web_calls(&run.id) {
            Ok(j) => j,
            Err(e) => return Finish::refused(host_error(&e)),
        };
        let (searches, fetches) = counts(&journal);
        let cx = Ctx {
            op,
            seq,
            grant,
            journal,
            searches,
            fetches,
            source,
        };
        match call {
            Call::Search { grant, query } => self.search(&cx, &grant, query).await,
            Call::Fetch {
                grant,
                url,
                offset,
                format,
            } => self.fetch(&cx, &grant, &url, offset, format, shown).await,
        }
    }

    /// Checks 7 to 9 of a search.
    async fn search(&self, cx: &Ctx<'_>, g: &SearchGrant, query: SearchQuery) -> Finish {
        if cx.searches >= g.max_per_run {
            return Finish::refused(cap(g.max_per_run, "searches"));
        }
        let p = match self.priced(cx.op, &web_search_line(g.provider.as_str())) {
            Ok(p) => p,
            Err(e) => return Finish::refused(e),
        };
        let mut extra = vec![("provider", json!(g.provider.as_str()))];
        match cx.source.search(&query, &self.tick.opts.cancel).await {
            Ok(a) => {
                let v = json!({
                    "results": results_of(&a, query.limit),
                    "left": cx.left(cx.searches + 1, cx.fetches, p.credits - p.price),
                });
                extra.push(("request_id", json!(a.request_id)));
                // Trimmed past AUDIT_RESPONSE_BYTES ([`audit_response`]).
                extra.push(("response", audit_response(&a)));
                Finish {
                    usage: Some((p.usage, p.price)),
                    extra,
                    counted: true,
                    ..Finish::new(Ok(v))
                }
            }
            Err(e) => {
                extra.push(("request_id", json!(e.request_id)));
                Finish {
                    // A failed search is never charged, but it counts
                    // (spec §7.2).
                    usage: charged(cx.op, &e).then_some((p.usage, p.price)),
                    extra,
                    counted: true,
                    ..Finish::refused(from_web(&e))
                }
            }
        }
    }

    /// Checks 5 to 9 of a fetch.
    async fn fetch(
        &self,
        cx: &Ctx<'_>,
        g: &FetchGrant,
        url: &PublicUrl,
        offset: usize,
        format: Format,
        shown: &mut Value,
    ) -> Finish {
        let tick = self.tick;
        shown["url"] = json!(url.normalized());
        let own = cx.source.own_hosts();
        let (src, pages) = match self.url_source(&g.rule, &own, url, &cx.journal).await {
            Ok(x) => x,
            Err(e) => return Finish::refused(e),
        };
        shown["source"] = json!(src.label());
        let refused = |e: HostError| Finish::refused(e).of_fetch(&src, format);
        if let Some(p) = pages.iter().find(|p| p.url == url.normalized()) {
            let kept_as = cx
                .journal
                .iter()
                .find(|e| e.step_n() == p.step_n && e.seq() == p.seq)
                .and_then(|e| e.result()["format"].as_str())
                .unwrap_or("auto");
            if kept_as != format_str(format) {
                return refused(HostError::new(
                    CODE_BAD_ARGS,
                    format!(
                        "this run already read {} as {kept_as}; fetch it with that format",
                        url.normalized()
                    ),
                ));
            }
            let credits = match self.week_left() {
                Ok(c) => c,
                Err(e) => return refused(e),
            };
            let v = page_value(p, offset, cx.left(cx.searches, cx.fetches, credits));
            return Finish {
                extra: vec![("stored", json!(true))],
                ..Finish::new(Ok(v)).of_fetch(&src, format)
            };
        }
        if cx.fetches >= g.max_per_run {
            return refused(cap(g.max_per_run, "fetches"));
        }
        let p = match self.priced(cx.op, LINE_WEB_FETCH) {
            Ok(p) => p,
            Err(e) => return refused(e),
        };
        let rule = g.rule.clone();
        let own = cx.source.own_hosts();
        let hop = move |to: &PublicUrl| -> Result<(), WebError> {
            let to = PublicUrl::parse(to.as_str())?;
            match host_refused(&rule, &own, to.host()) {
                None => Ok(()),
                Some(why) => Err(WebError::new(
                    CODE_URL_NOT_ALLOWED,
                    format!("the page redirected to {}, {why}", to.host()),
                    false,
                )),
            }
        };
        let got = cx
            .source
            .fetch(url, format, &g.limits, &hop, &tick.opts.cancel)
            .await;
        match got {
            Ok(page) => {
                let (text, total_chars) = cut(&page.text, STORE_BYTES);
                let row = WebPageRow {
                    url: url.normalized().to_string(),
                    step_n: tick.n,
                    seq: cx.seq,
                    final_url: page.final_url,
                    status: page.status,
                    content_type: page.content_type,
                    text,
                    total_chars,
                    links: page.links,
                };
                let left = cx.left(cx.searches, cx.fetches + 1, p.credits - p.price);
                let v = page_value(&row, offset, left);
                Finish {
                    usage: Some((p.usage, p.price)),
                    page: Some(row),
                    extra: vec![("request_id", json!(page.request_id))],
                    counted: true,
                    ..Finish::new(Ok(v)).of_fetch(&src, format)
                }
            }
            // Charged when it went out; counted whether or not (spec §7.2).
            Err(e) => Finish {
                usage: charged(cx.op, &e).then_some((p.usage, p.price)),
                extra: vec![("request_id", json!(e.request_id))],
                counted: true,
                ..refused(from_web(&e))
            },
        }
    }

    /// One request on `line` at its rate now; `web_unavailable` when the
    /// line has no rate (web pricing is not configured, and a call made
    /// anyway could not be charged), `credits_exhausted` when the person
    /// cannot pay for it.
    fn priced(&self, op: &str, line: &str) -> Result<Priced, HostError> {
        let tick = self.tick;
        let store = tick.store;
        let rate = store
            .rate_of(line)
            .map_err(|e| host_error(&e))?
            .ok_or_else(|| {
                HostError::new(
                    CODE_WEB_UNAVAILABLE,
                    format!(
                        "web pricing is not configured on this instance (it has no cost line \
                         {line}); do without clax.{op}"
                    ),
                )
            })?;
        let ks = tick.key_source();
        let price = one_request(&rate, ks).map_err(|e| host_error(&e))?;
        let credits = self.week_left()?;
        let unenforced = store
            .flags_of(&tick.run.person)
            .map_err(|e| host_error(&e))?
            .claxbot_unenforced;
        if !unenforced && price > credits {
            return Err(HostError::new(
                CODE_CREDITS_EXHAUSTED,
                format!(
                    "the person you act for has {} credits left this week and clax.{op} costs {}; \
                     finish with what you have",
                    credits as f64 / 1000.0,
                    price as f64 / 1000.0
                ),
            ));
        }
        Ok(Priced {
            usage: Metered {
                rate: Some(rate),
                ..Metered::new(line, 1, ks)
            },
            price,
            credits,
        })
    }

    /// The person's credits left this week.
    fn week_left(&self) -> Result<i64, HostError> {
        let tick = self.tick;
        Ok(tick
            .store
            .week_usage(&tick.run.person, tick.deps.clock.now())
            .map_err(|e| host_error(&e))?
            .remaining_milli())
    }

    /// Where `url` came from, as `rule` requires (check 5), with the run's
    /// page store it was derived from beside `journal` (the run's web
    /// calls); `url`'s host is neither one of `own` nor off the rule's
    /// hosts ([`host_refused`]).
    async fn url_source(
        &self,
        rule: &UrlRule,
        own: &OwnHosts,
        url: &PublicUrl,
        journal: &[CallEntry],
    ) -> Result<(UrlSource, Vec<WebPageRow>), HostError> {
        if let Some(why) = host_refused(rule, own, url.host()) {
            return Err(HostError::new(
                CODE_URL_NOT_ALLOWED,
                format!("{} is {why}", url.host()),
            ));
        }
        let tick = self.tick;
        let cx = tick.deps.host.context(&tick.run).await.map_err(|e| {
            tracing::warn!(
                "clax-bot: run {} has no request context for a fetch: {e}",
                tick.run.id
            );
            HostError::new(
                CODE_WEB_UNAVAILABLE,
                "the request this run answers could not be read (retryable: try again)",
            )
            .retryable()
        })?;
        let pages = tick
            .store
            .web_pages(&tick.run.id)
            .map_err(|e| host_error(&e))?;
        let found = sources(&cx, journal, &pages).remove(url.normalized());
        if looks_credentialed(url) && !matches!(found, Some(UrlSource::Request { .. })) {
            return Err(HostError::new(
                CODE_URL_NOT_ALLOWED,
                "the URL looks like it holds a credential; it is fetched only when the \
                 person you act for wrote it themselves",
            ));
        }
        let src = match (rule, found) {
            (_, Some(src)) => src,
            (UrlRule::AnyPublic, None) => UrlSource::AnyPublic,
            (_, None) => {
                return Err(HostError::new(
                    CODE_URL_NOT_IN_PRIOR_CONTEXT,
                    format!(
                        "{} is not in the sender's words, a search result or a page you \
                         fetched; fetch only URLs from those",
                        url.normalized()
                    ),
                ));
            }
        };
        Ok((src, pages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::SearchHit;

    #[test]
    fn a_large_search_response_is_trimmed_for_the_audit() {
        let hit = SearchHit {
            url: "https://a.example/".into(),
            title: Some("A".into()),
            description: Some("about A".into()),
            age: Some("2d".into()),
        };
        let small = SearchAnswer {
            hits: vec![hit.clone()],
            request_id: Some("req-1".into()),
            raw: json!({"items": [{"url": "https://a.example/"}], "extra": "kept"}),
        };
        assert_eq!(audit_response(&small), small.raw);
        let large = SearchAnswer {
            raw: json!({"items": [{"url": "https://a.example/",
                                    "description": "x".repeat(AUDIT_RESPONSE_BYTES)}]}),
            ..small
        };
        let kept = audit_response(&large);
        assert_eq!(
            kept,
            json!({"items": [{"url": "https://a.example/", "title": "A", "age": "2d"}],
                   "requestId": "req-1", "truncated": true})
        );
        assert!(kept.to_string().len() < 1_000);
    }
}

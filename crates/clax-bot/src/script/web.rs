//! The `clax.web.*` performer ([`crate::api::Performer::Web`], spec
//! 2026-11-02-claxbot-web-tools §4.2, §5, §6, §7.2): a web call made by a
//! script goes through these checks in order, and the first that fails
//! answers the script. A refusal before the source is called (checks 2 to
//! 8) is uncharged and journaled like any other refusal:
//!
//! 1. A call journaled at its place answers from the journal.
//! 2. The run declares the operation ([`crate::web::WebShape`]) and the
//!    host has a source ([`crate::run::Deps::web`]), or `web_disabled`.
//! 3. The operation is still granted now ([`WebAccess::now`]: the admin
//!    or the person may have turned it off since the run started), or
//!    `web_disabled`.
//! 4. Its arguments hold (a query of 1 to 1,024 characters, a `limit` and
//!    an `offset` not below 0, a URL), or `invalid_args`.
//! 5. A fetch: the URL is a [`PublicUrl`], the admin's [`UrlRule`] lets
//!    its host be reached, a source holds it ([`sources`]) unless the rule
//!    is [`UrlRule::AnyPublic`], and a URL that looks credentialed
//!    ([`looks_credentialed`]) is in the sender's own words; else
//!    `url_not_allowed` or `url_not_in_prior_context`.
//! 6. A fetch of a URL the run's page store keeps answers its window from
//!    the store: free, and not counted against the cap. Its journal entry
//!    is written like a read's.
//! 7. The run's cap for the operation has room, or `web_cap`. The count
//!    is of the run's charged calls (its `web.*` usage rows), so a search
//!    that failed, or a fetch that never went out, does not count.
//! 8. The person can pay for one request on the operation's line
//!    ([`Store::one_request_milli`] at most their credits left this
//!    week, unless their credits are not enforced), or
//!    `credits_exhausted`.
//! 9. The source is called, with the run's cancel token and, for a fetch,
//!    the host check of every redirect target (the rule's host part, spec
//!    §5.3; the source refuses an `https` to `http` hop itself).
//! 10. The answer, its charge (when it succeeded, or when [`charged`]
//!     says the failure is), the fetched page (its text cut at
//!     [`STORE_BYTES`]) and its `bot.web` record are written in one
//!     transaction ([`Store::spend_journaled`]) before the script sees
//!     the answer.
//!
//! Every answer carries `left`: the run's searches and fetches left under
//! the caps after the call, and the person's credits left this week as a
//! number of credits.
//!
//! [`Store::one_request_milli`]: clax_store::Store::one_request_milli
//! [`Store::spend_journaled`]: clax_store::Store::spend_journaled

use super::{RunHost, recorded};
use crate::api::CODE_BAD_ARGS;
use crate::host::{RequestContext, host_error};
use crate::web::page::{PAGE_CHARS, SNIPPET_CHARS, STORE_BYTES, snippet, window};
use crate::web::url::looks_credentialed;
use crate::web::{
    CODE_CREDITS_EXHAUSTED, CODE_URL_NOT_ALLOWED, CODE_URL_NOT_IN_PRIOR_CONTEXT, CODE_WEB_CAP,
    CODE_WEB_DISABLED, FetchGrant, Format, OP_SEARCH, PublicUrl, SearchAnswer, SearchGrant,
    SearchQuery, UrlRule, UrlSource, WebAccess, WebError, WebGrant, WebShape, WebSource,
    access::sources, charged,
};
use clax_bot_sandbox::HostError;
use clax_core::audit::{AuditKind, AuditRecord};
use clax_store::bot_runs::CallEntry;
use clax_store::bot_web::{Spend, WebPageRow};
use clax_store::credits::{LINE_WEB_FETCH, Metered, web_search_line};
use serde_json::{Value, json};
use std::sync::Arc;

/// The longest query, in characters.
const MAX_QUERY_CHARS: usize = 1_024;

/// What a web call answered, what it charged (`None` when it was answered
/// from the journal) and what the run panel shows of it (`{query}` or
/// `{url, source}`).
pub(super) struct WebDone {
    pub res: Result<Value, HostError>,
    pub cost_milli: Option<i64>,
    pub web: Value,
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

fn refusal(code: &str, message: impl Into<String>) -> HostError {
    HostError::new(code, message)
}

fn from_web(e: &WebError) -> HostError {
    HostError::new(e.code, e.message.clone())
}

/// A web call that failed: its error, whether it is still to be journaled
/// (a refusal before the source was called) and what it charged.
struct Refused {
    error: HostError,
    unjournaled: bool,
    cost_milli: i64,
}

impl Refused {
    /// Refused before anything was written.
    fn before(error: HostError) -> Refused {
        Refused {
            error,
            unjournaled: true,
            cost_milli: 0,
        }
    }

    /// Failed after its entry was written, or where it cannot be.
    fn written(error: HostError, cost_milli: i64) -> Refused {
        Refused {
            error,
            unjournaled: false,
            cost_milli,
        }
    }
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
            return WebDone {
                res: recorded(e),
                cost_milli: None,
                web: shown,
            };
        }
        let (res, cost) = match self.web_fresh(op, args, raw, seq, &mut shown).await {
            Ok((v, cost)) => (Ok(v), cost),
            Err(r) => {
                let res = Err(r.error);
                if r.unjournaled {
                    self.journal(seq, op, raw, &res, false);
                }
                (res, r.cost_milli)
            }
        };
        WebDone {
            res,
            cost_milli: Some(cost),
            web: shown,
        }
    }

    /// [`RunHost::web`] for a call not in the journal: its value and
    /// charge, or its error and whether it is still to be journaled.
    async fn web_fresh(
        &self,
        op: &str,
        args: &[Value],
        raw: &Value,
        seq: i64,
        shown: &mut Value,
    ) -> Result<(Value, i64), Refused> {
        let refuse = |code: &str, m: String| Refused::before(refusal(code, m));
        let tick = self.tick;
        let store = tick.store;
        let run = &tick.run;
        let shape = WebShape::parse(&run.web);
        let declared = if op == OP_SEARCH {
            shape.search
        } else {
            shape.fetch
        };
        let source: &Arc<dyn WebSource> = match &tick.deps.web {
            Some(s) if declared => s,
            _ => {
                return Err(refuse(
                    CODE_WEB_DISABLED,
                    format!("clax.{op} is not available to this run"),
                ));
            }
        };
        let grant = match WebAccess::now(store, &run.person, true) {
            Ok(WebAccess::On(g)) => g,
            Ok(WebAccess::Off(why)) => {
                return Err(refuse(
                    CODE_WEB_DISABLED,
                    format!(
                        "web use was turned off ({}) since this run started; do without it",
                        why.as_str()
                    ),
                ));
            }
            Err(e) => return Err(Refused::before(host_error(&e))),
        };
        let call = self.call_of(op, args, &grant).map_err(Refused::before)?;
        let (searches, fetches) = self.spent_counts().map_err(Refused::before)?;
        let left = |searches: u32, fetches: u32, credits_milli: i64| {
            let under = |g: Option<u32>, n: u32| g.map_or(0, |max| max.saturating_sub(n));
            json!({
                "searches": under(grant.search.as_ref().map(|g| g.max_per_run), searches),
                "fetches": under(grant.fetch.as_ref().map(|g| g.max_per_run), fetches),
                "credits": credits_milli.max(0) as f64 / 1000.0,
            })
        };
        let week_left = || -> Result<i64, HostError> {
            Ok(store
                .week_usage(&run.person, tick.deps.clock.now())
                .map_err(|e| host_error(&e))?
                .remaining_milli())
        };
        // The URL rule and the page store, for a fetch.
        let mut fetch_source = None;
        if let Call::Fetch {
            grant: g,
            url,
            offset,
            ..
        } = &call
        {
            let src = self
                .url_source(&g.rule, url)
                .await
                .map_err(Refused::before)?;
            shown["url"] = json!(url.normalized());
            shown["source"] = json!(src.label());
            let kept = store
                .web_page(&run.id, url.normalized())
                .map_err(|e| Refused::before(host_error(&e)))?;
            if let Some(p) = kept {
                let credits = week_left().map_err(Refused::before)?;
                let v = page_value(&p, *offset, left(searches, fetches, credits));
                self.journal(seq, op, raw, &Ok(v.clone()), true);
                return Ok((v, 0));
            }
            fetch_source = Some(src);
        }
        let (count, max, what) = match &call {
            Call::Search { grant: g, .. } => (searches, g.max_per_run, "searches"),
            Call::Fetch { grant: g, .. } => (fetches, g.max_per_run, "fetches"),
        };
        if count >= max {
            return Err(refuse(
                CODE_WEB_CAP,
                format!("this run has used its {max} {what}; work with what you have"),
            ));
        }
        let line = match &call {
            Call::Search { grant: g, .. } => web_search_line(g.provider.as_str()),
            Call::Fetch { .. } => LINE_WEB_FETCH.to_string(),
        };
        let ks = tick.key_source();
        let price = store
            .one_request_milli(&line, ks)
            .map_err(|e| Refused::before(host_error(&e)))?;
        let credits = week_left().map_err(Refused::before)?;
        let unenforced = store
            .flags_of(&run.person)
            .map_err(|e| Refused::before(host_error(&e)))?
            .claxbot_unenforced;
        if !unenforced && price > credits {
            return Err(refuse(
                CODE_CREDITS_EXHAUSTED,
                format!(
                    "the person you act for has {} credits left this week and clax.{op} costs {}; \
                     finish with what you have",
                    credits as f64 / 1000.0,
                    price as f64 / 1000.0
                ),
            ));
        }
        let cancel = &tick.opts.cancel;
        let at = store.now();
        let (got, usage_on, page, audit): (Result<Value, WebError>, bool, _, _) = match call {
            Call::Search { grant: g, query } => {
                let mut audit = AuditRecord::new(AuditKind::BotWeb, at)
                    .with("op", op)
                    .with("query", query.query.as_str())
                    .with("provider", g.provider.as_str());
                match source.search(&query, cancel).await {
                    Ok(a) => {
                        audit = audit
                            .with("request_id", a.request_id.clone())
                            .with("response", a.raw.clone());
                        let v = json!({
                            "results": results_of(&a, query.limit),
                            "left": left(searches + 1, fetches, credits - price),
                        });
                        (Ok(v), true, None, audit)
                    }
                    Err(e) => {
                        let on = charged(op, &e);
                        audit = audit.with("request_id", e.request_id.clone());
                        (Err(e), on, None, audit)
                    }
                }
            }
            Call::Fetch {
                grant: g,
                url,
                offset,
                format,
            } => {
                let src = fetch_source.unwrap_or(UrlSource::AnyPublic);
                let mut audit = AuditRecord::new(AuditKind::BotWeb, at)
                    .with("op", op)
                    .with("url", url.normalized())
                    .with("source", src.label());
                let rule = g.rule.clone();
                let hop = move |to: &PublicUrl| -> Result<(), WebError> {
                    let to = PublicUrl::parse(to.as_str())?;
                    if rule.allows_host(to.host()) {
                        Ok(())
                    } else {
                        Err(WebError::new(
                            CODE_URL_NOT_ALLOWED,
                            format!(
                                "the page redirected to {}, a host this instance does not fetch",
                                to.host()
                            ),
                            false,
                        ))
                    }
                };
                match source.fetch(&url, format, &g.limits, &hop, cancel).await {
                    Ok(p) => {
                        audit = audit.with("request_id", p.request_id.clone());
                        let (text, total_chars) = cut(&p.text, STORE_BYTES);
                        let row = WebPageRow {
                            url: url.normalized().to_string(),
                            step_n: tick.n,
                            seq,
                            final_url: p.final_url,
                            status: p.status,
                            content_type: p.content_type,
                            text,
                            total_chars,
                            links: p.links,
                        };
                        let v =
                            page_value(&row, offset, left(searches, fetches + 1, credits - price));
                        (Ok(v), true, Some(row), audit)
                    }
                    Err(e) => {
                        let on = charged(op, &e);
                        audit = audit.with("request_id", e.request_id.clone());
                        (Err(e), on, None, audit)
                    }
                }
            }
        };
        let audit = match &got {
            Ok(_) => audit.with("ok", true),
            Err(e) => audit.with("ok", false).with("code", e.code),
        };
        let entry = tick
            .entry(self.attempt, seq, op, raw)
            .map_err(|e| Refused::written(e, 0))?
            .answered(
                got.is_ok(),
                match &got {
                    Ok(v) => v.clone(),
                    Err(e) => super::err_json(&from_web(e)),
                },
            );
        let spent = store
            .spend_journaled(
                run,
                tick.driver,
                Spend {
                    entry,
                    usage: usage_on.then(|| Metered::new(&line, 1, ks)),
                    page,
                    audit,
                },
            )
            .map_err(|e| Refused::written(host_error(&e), 0))?;
        recorded(&spent.entry)
            .map(|v| (v, spent.credits_milli))
            .map_err(|e| Refused::written(e, spent.credits_milli))
    }

    /// The arguments of `op`, checked and bounded by `grant`.
    fn call_of(&self, op: &str, args: &[Value], grant: &WebGrant) -> Result<Call, HostError> {
        let bad = |m: String| HostError::new(CODE_BAD_ARGS, format!("clax.{op}: {m}"));
        let off = || {
            refusal(
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

    /// Where `url` came from, as `rule` requires (see the module
    /// documentation, check 5).
    async fn url_source(&self, rule: &UrlRule, url: &PublicUrl) -> Result<UrlSource, HostError> {
        if !rule.allows_host(url.host()) {
            return Err(refusal(
                CODE_URL_NOT_ALLOWED,
                format!(
                    "{} is not on the hosts this instance fetches from",
                    url.host()
                ),
            ));
        }
        let tick = self.tick;
        let cx = match tick.deps.host.context(&tick.run).await {
            Ok(cx) => cx,
            Err(e) => {
                // Without the request, only the run's own records are
                // sources.
                tracing::warn!(
                    "clax-bot: run {} has no request context for a fetch: {e}",
                    tick.run.id
                );
                RequestContext::default()
            }
        };
        let mut journal = Vec::new();
        for n in 1..=tick.n {
            journal.extend(
                tick.store
                    .bot_calls(&tick.run.id, n)
                    .map_err(|e| host_error(&e))?,
            );
        }
        let pages = tick
            .store
            .web_pages(&tick.run.id)
            .map_err(|e| host_error(&e))?;
        let found = sources(&cx, &journal, &pages).remove(url.normalized());
        if looks_credentialed(url) && !matches!(found, Some(UrlSource::Request { .. })) {
            return Err(refusal(
                CODE_URL_NOT_ALLOWED,
                "the URL looks like it holds a credential; it is fetched only when the \
                 person you act for wrote it themselves",
            ));
        }
        match (rule, found) {
            (_, Some(src)) => Ok(src),
            (UrlRule::AnyPublic, None) => Ok(UrlSource::AnyPublic),
            (_, None) => Err(refusal(
                CODE_URL_NOT_IN_PRIOR_CONTEXT,
                format!(
                    "{} is not in the sender's words, a search result or a page you fetched; \
                     fetch only URLs from those",
                    url.normalized()
                ),
            )),
        }
    }

    /// The run's charged searches and fetches so far: its `web.*` usage
    /// rows.
    fn spent_counts(&self) -> Result<(u32, u32), HostError> {
        let rows = self
            .tick
            .store
            .usage_of_run(&self.tick.run.id)
            .map_err(|e| host_error(&e))?;
        let count = |f: &dyn Fn(&str) -> bool| {
            u32::try_from(rows.iter().filter(|r| f(&r.line)).count()).unwrap_or(u32::MAX)
        };
        Ok((
            count(&|l| l.starts_with("web.search.")),
            count(&|l| l == LINE_WEB_FETCH),
        ))
    }
}

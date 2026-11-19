//! `web.search` and `web.fetch` in a session of Clax's own surface: the
//! web's [`Performer`]s on the one Spend sequence ([`super::spend`];
//! spec 2026-11-02-claxbot-web-tools §4.2, §5, §6, §7.2, §8, and spec
//! 2026-11-19-claxbot-browser §4.6). What each answers to the sequence's
//! steps:
//!
//! - **Live** (step 3): `web.search` or `web.fetch`, each its own
//!   capability; a source still configured whose host found it cannot
//!   be used here ([`crate::web::WebSource::unusable`]: the Worker's AI
//!   binding under `wrangler dev --local`) is refused `web_unavailable`,
//!   not retryable: the web was not turned off, it cannot run here.
//! - **Arguments** (step 4): a query of 1 to 1,024 characters and a
//!   `limit` not below 0 (at most the grant's), or a URL, an `offset`
//!   not below 0 and a format.
//! - **Admission** (step 5), a fetch: the URL is a [`PublicUrl`], its
//!   host is not one of the instance's own
//!   ([`crate::web::WebSource::own_hosts`]), the admin's [`UrlRule`] lets
//!   its host be reached, a source holds it ([`sources`]) unless the rule
//!   is [`UrlRule::AnyPublic`], and a URL that looks credentialed
//!   ([`looks_credentialed`](crate::web::url::looks_credentialed)) is in
//!   the sender's own words; else `url_not_allowed` or
//!   `url_not_in_prior_context`. When the request cannot be read,
//!   `web_unavailable`, not retryable: it is refused before the source
//!   and so not counted, and a flag inviting a retry that nothing counts
//!   would invite a loop bounded only by the script's own limits.
//! - **Free answers** (step 6), a fetch of a URL the run's page store
//!   keeps: its window from the store, free and not counted. It must ask
//!   for the format the page was fetched in, or `invalid_args`.
//! - **Cap** (step 7): the run's searches or fetches that reached the
//!   source, charged or not: a search that failed and a fetch that never
//!   went out count, though neither is charged.
//! - **Price** (step 8): one request at the operation's line.
//! - **Perform** (step 9): the source is called, with the run's cancel
//!   token and, for a fetch, the host check of every redirect target (the
//!   instance's own hosts and the rule's host part, as at admission,
//!   spec §5.3; the source refuses an `https` to `http` hop itself). A
//!   call the source answered is charged when it succeeded, or when
//!   [`charged`] says its failure is, with the page a fetch received
//!   (its text cut at [`STORE_BYTES`]).
//!
//! Each entry keeps what the run shows under `web` (`{query}`, or `{url,
//! source}` once the URL parsed) and, for a fetch whose URL had a source,
//! `source` (the [`UrlSource`] that allowed it, [`UrlSource::to_json`])
//! and `format`. Each `bot.spend` record of a web call adds what the run
//! shows, and the provider and request ID, a search's response
//! ([`audit_response`]) or that a fetch came from the page store.
//!
//! Every answer carries `left`: the run's searches and fetches left under
//! the caps after the call, and the person's credits left this week as a
//! number of credits. A script's error carries the [`WebError`]'s
//! `retryable` flag ([`HostError::retryable`]), and a retryable error's
//! message also says to try again, for a model that reads only the message
//! (both from the flag, in [`from_web`] alone).

use super::session::ClaxSession;
use super::spend::{Finish, Performer, PriceCap, Priced, Seen};
use crate::api::CODE_BAD_ARGS;
use crate::capability::{BrowserTerms, Capability, Terms};
use crate::host::host_error;
use crate::surface::Call;
use crate::web::page::{PAGE_CHARS, SNIPPET_CHARS, STORE_BYTES, snippet, window};
use crate::web::url::OwnHosts;
use crate::web::{
    CODE_URL_NOT_ALLOWED, CODE_WEB_UNAVAILABLE, FetchGrant, Format, OP_FETCH, OP_SEARCH,
    PublicUrl, SearchAnswer, SearchGrant, SearchQuery, UrlRule, UrlSource, WebError, WebSource,
    access::{Refused, admit, sources},
    charged,
};
use clax_bot_sandbox::HostError;
use clax_store::bot_runs::CallEntry;
use clax_store::bot_web::WebPageRow;
use clax_store::credits::{LINE_WEB_FETCH, web_search_line};
use serde_json::{Value, json};
use std::sync::Arc;

/// The longest query, in characters.
const MAX_QUERY_CHARS: usize = 1_024;

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

/// Why a fetch may not reach `host`, on the URL asked for (step 5) or a
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

/// The most bytes of a provider's search response a `bot.spend` record
/// keeps whole.
const AUDIT_RESPONSE_BYTES: usize = 64 << 10;

/// What a search's `bot.spend` record keeps of the provider's
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
/// [`HostError::retryable`], and its message says to try again. The only
/// place a web error's flag and its words are made, so they cannot
/// disagree.
fn from_web(e: &WebError) -> HostError {
    if e.retryable {
        HostError::new(e.code, format!("{} (retryable: try again)", e.message)).retryable()
    } else {
        HostError::new(e.code, e.message.clone())
    }
}


/// `clax.web.search`.
pub(super) struct Search;

/// `clax.web.fetch`.
pub(super) struct Fetch;

/// A search's arguments, checked against its grant.
pub(super) struct SearchArgs {
    grant: SearchGrant,
    query: SearchQuery,
}

/// A fetch's arguments, checked against its grant, and what its
/// admission found.
pub(super) struct FetchArgs {
    grant: FetchGrant,
    url: PublicUrl,
    offset: usize,
    format: Format,
    /// The source that admitted the URL.
    src: Option<UrlSource>,
    /// The run's page store, read at admission.
    pages: Vec<WebPageRow>,
    /// The run's web calls, read at admission.
    journal: Vec<CallEntry>,
}

/// `invalid_args` for `op`, saying `m`.
fn bad(op: &str, m: String) -> HostError {
    HostError::new(CODE_BAD_ARGS, format!("clax.{op}: {m}"))
}

/// The web source; [`super::spend`]'s step 2 has made sure there is one.
fn web_source(s: &ClaxSession<'_>) -> Arc<dyn WebSource> {
    s.provider
        .web
        .clone()
        .expect("the Spend sequence performs a web call only on a host with a web source")
}

/// Step 3: the web source's own reason it cannot run here.
fn web_unusable(s: &ClaxSession<'_>) -> Option<HostError> {
    let why = s.provider.web.as_ref()?.unusable()?;
    Some(HostError::new(
        CODE_WEB_UNAVAILABLE,
        format!("{why} Continue without the web."),
    ))
}

/// `left` once the run made `searches` and `fetches` counted calls, with
/// `credits_milli` left this week.
fn left(seen: &Seen, searches: u32, fetches: u32, credits_milli: i64) -> Value {
    json!({
        "searches": seen.left_of(Capability::WebSearch, searches),
        "fetches": seen.left_of(Capability::WebFetch, fetches),
        "credits": credits_milli.max(0) as f64 / 1000.0,
    })
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Performer for Search {
    type Args = SearchArgs;

    fn capability(&self, _args: &[Value]) -> Capability {
        Capability::WebSearch
    }

    fn shown(&self, args: &[Value]) -> Value {
        json!({"query": args.first()})
    }

    fn audit_shown(&self) -> bool {
        true
    }

    fn unusable(&self, s: &ClaxSession<'_>) -> Option<HostError> {
        web_unusable(s)
    }

    fn check_args(
        &self,
        _s: &ClaxSession<'_>,
        call: &Call<'_>,
        terms: &Terms,
        _browser: Option<BrowserTerms>,
        _shown: &mut Value,
    ) -> Result<SearchArgs, HostError> {
        let Terms::Search(g) = terms else {
            unreachable!("web.search's terms are a search grant")
        };
        let (op, args) = (call.op, call.args);
        let query = args[0].as_str().unwrap_or("").to_string();
        let n = query.chars().count();
        if n == 0 || n > MAX_QUERY_CHARS {
            return Err(bad(
                op,
                format!("query must be 1 to {MAX_QUERY_CHARS} characters, got {n}"),
            ));
        }
        let limit = match args.get(1).and_then(Value::as_i64) {
            None => g.limit,
            Some(l) if l < 1 => return Err(bad(op, format!("limit must be at least 1, got {l}"))),
            Some(l) => u8::try_from(l).unwrap_or(u8::MAX).min(g.limit),
        };
        let query = SearchQuery {
            query,
            limit,
            provider: g.provider,
            gateway: g.gateway.clone(),
            byok_alias: g.byok_alias.clone(),
        };
        Ok(SearchArgs {
            grant: g.clone(),
            query,
        })
    }

    async fn admit(
        &self,
        _s: &ClaxSession<'_>,
        _call: &Call<'_>,
        _args: &mut SearchArgs,
        _shown: &mut Value,
    ) -> Result<(), HostError> {
        Ok(())
    }

    fn count_key(&self, _call: &Call<'_>, args: &SearchArgs) -> (&'static str, u32, &'static str) {
        (OP_SEARCH, args.grant.max_per_run, "searches")
    }

    fn price_cap(&self, call: &Call<'_>, args: &SearchArgs) -> PriceCap {
        PriceCap {
            line: web_search_line(args.grant.provider.as_str()),
            qty: 1,
            costs: format!("clax.{} costs", call.op),
        }
    }

    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &SearchArgs,
        seen: &Seen,
        priced: &Priced,
        _shown: &mut Value,
    ) -> Finish {
        let (usage, price) = match priced.usage(1) {
            Ok(x) => x,
            Err(e) => return Finish::refused(host_error(&e)),
        };
        let (searches, fetches) = (seen.count(OP_SEARCH), seen.count(OP_FETCH));
        let mut audit = vec![("provider", json!(args.grant.provider.as_str()))];
        match web_source(s).search(&args.query, s.cx.cancel).await {
            Ok(a) => {
                let v = json!({
                    "results": results_of(&a, args.query.limit),
                    "left": left(seen, searches + 1, fetches, priced.credits - price),
                });
                audit.push(("request_id", json!(a.request_id)));
                // Trimmed past AUDIT_RESPONSE_BYTES ([`audit_response`]).
                audit.push(("response", audit_response(&a)));
                Finish {
                    usage: Some((usage, price)),
                    audit,
                    counted: true,
                    ..Finish::new(Ok(v))
                }
            }
            Err(e) => {
                audit.push(("request_id", json!(e.request_id)));
                Finish {
                    // A failed search is never charged, but it counts
                    // (spec §7.2).
                    usage: charged(call.op, &e).then_some((usage, price)),
                    audit,
                    counted: true,
                    ..Finish::refused(from_web(&e))
                }
            }
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Performer for Fetch {
    type Args = FetchArgs;

    fn capability(&self, _args: &[Value]) -> Capability {
        Capability::WebFetch
    }

    fn shown(&self, args: &[Value]) -> Value {
        json!({"url": args.first()})
    }

    fn audit_shown(&self) -> bool {
        true
    }

    fn unusable(&self, s: &ClaxSession<'_>) -> Option<HostError> {
        web_unusable(s)
    }

    fn check_args(
        &self,
        _s: &ClaxSession<'_>,
        call: &Call<'_>,
        terms: &Terms,
        _browser: Option<BrowserTerms>,
        _shown: &mut Value,
    ) -> Result<FetchArgs, HostError> {
        let Terms::Fetch(g) = terms else {
            unreachable!("web.fetch's terms are a fetch grant")
        };
        let (op, args) = (call.op, call.args);
        let url = PublicUrl::parse(args[0].as_str().unwrap_or("")).map_err(|e| from_web(&e))?;
        let opts = &args[1];
        let offset = match opts["offset"].as_i64() {
            None => 0,
            Some(o) if o < 0 => return Err(bad(op, format!("offset must be at least 0, got {o}"))),
            Some(o) => usize::try_from(o).unwrap_or(usize::MAX),
        };
        let format = match opts["format"].as_str() {
            Some("raw") => Format::Raw,
            _ => Format::Auto,
        };
        Ok(FetchArgs {
            grant: g.clone(),
            url,
            offset,
            format,
            src: None,
            pages: Vec::new(),
            journal: Vec::new(),
        })
    }

    fn noted(&self, args: &FetchArgs) -> (Vec<(&'static str, Value)>, Vec<(&'static str, Value)>) {
        let entry = match &args.src {
            Some(src) => vec![
                ("source", src.to_json()),
                ("format", json!(format_str(args.format))),
            ],
            None => Vec::new(),
        };
        (Vec::new(), entry)
    }

    async fn admit(
        &self,
        s: &ClaxSession<'_>,
        _call: &Call<'_>,
        args: &mut FetchArgs,
        shown: &mut Value,
    ) -> Result<(), HostError> {
        shown["url"] = json!(args.url.normalized());
        let own = web_source(s).own_hosts();
        args.journal = s
            .store()
            .bot_web_calls(&s.run.id)
            .map_err(|e| host_error(&e))?;
        let (src, pages) = s
            .url_source(&args.grant.rule, &own, &args.url, &args.journal)
            .await?;
        shown["source"] = json!(src.label());
        args.src = Some(src);
        args.pages = pages;
        Ok(())
    }

    async fn free_answer(
        &self,
        s: &ClaxSession<'_>,
        _call: &Call<'_>,
        args: &FetchArgs,
        seen: &Seen,
    ) -> Option<Finish> {
        let url = args.url.normalized();
        let p = args.pages.iter().find(|p| p.url == url)?;
        let kept_as = args
            .journal
            .iter()
            .find(|e| e.step_n() == p.step_n && e.seq() == p.seq)
            .and_then(|e| e.result()["format"].as_str())
            .unwrap_or("auto");
        if kept_as != format_str(args.format) {
            return Some(Finish::refused(HostError::new(
                CODE_BAD_ARGS,
                format!("this run already read {url} as {kept_as}; fetch it with that format"),
            )));
        }
        let credits = match s.week_left() {
            Ok(c) => c,
            Err(e) => return Some(Finish::refused(e)),
        };
        let left = left(seen, seen.count(OP_SEARCH), seen.count(OP_FETCH), credits);
        Some(Finish {
            audit: vec![("stored", json!(true))],
            ..Finish::new(Ok(page_value(p, args.offset, left)))
        })
    }

    fn count_key(&self, _call: &Call<'_>, args: &FetchArgs) -> (&'static str, u32, &'static str) {
        (OP_FETCH, args.grant.max_per_run, "fetches")
    }

    fn price_cap(&self, call: &Call<'_>, _args: &FetchArgs) -> PriceCap {
        PriceCap {
            line: LINE_WEB_FETCH.to_string(),
            qty: 1,
            costs: format!("clax.{} costs", call.op),
        }
    }

    async fn perform(
        &self,
        s: &ClaxSession<'_>,
        call: &Call<'_>,
        args: &FetchArgs,
        seen: &Seen,
        priced: &Priced,
        _shown: &mut Value,
    ) -> Finish {
        let (usage, price) = match priced.usage(1) {
            Ok(x) => x,
            Err(e) => return Finish::refused(host_error(&e)),
        };
        let source = web_source(s);
        let rule = args.grant.rule.clone();
        let own = source.own_hosts();
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
        let got = source
            .fetch(&args.url, args.format, &args.grant.limits, &hop, s.cx.cancel)
            .await;
        let (searches, fetches) = (seen.count(OP_SEARCH), seen.count(OP_FETCH));
        match got {
            Ok(page) => {
                let (text, total_chars) = cut(&page.text, STORE_BYTES);
                let row = WebPageRow {
                    url: args.url.normalized().to_string(),
                    step_n: s.cx.step,
                    seq: call.place.seq,
                    final_url: page.final_url,
                    status: page.status,
                    content_type: page.content_type,
                    text,
                    total_chars,
                    links: page.links,
                };
                let left = left(seen, searches, fetches + 1, priced.credits - price);
                let v = page_value(&row, args.offset, left);
                Finish {
                    usage: Some((usage, price)),
                    page: Some(row),
                    audit: vec![("request_id", json!(page.request_id))],
                    counted: true,
                    ..Finish::new(Ok(v))
                }
            }
            // Charged when it went out; counted whether or not (spec §7.2).
            Err(e) => Finish {
                usage: charged(call.op, &e).then_some((usage, price)),
                audit: vec![("request_id", json!(e.request_id))],
                counted: true,
                ..Finish::refused(from_web(&e))
            },
        }
    }
}

impl ClaxSession<'_> {
    /// Where `url` came from, as `rule` requires (step 5), with the run's
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
        let cx = self.provider.host.context(&self.run).await.map_err(|e| {
            tracing::warn!(
                "clax-bot: run {} has no request context for a fetch: {e}",
                self.run.id
            );
            HostError::new(
                CODE_WEB_UNAVAILABLE,
                "the request this run answers could not be read, so no URL can be checked \
                 against it; do without this page",
            )
        })?;
        let pages = self
            .store()
            .web_pages(&self.run.id)
            .map_err(|e| host_error(&e))?;
        let found = sources(&cx, journal, &pages);
        let src = match admit(url, rule, &found, own) {
            Ok(src) => src,
            Err(why) => {
                let message = match (why, rule) {
                    (Refused::OwnHost | Refused::NotListed, _) => format!(
                        "{} is {}",
                        url.host(),
                        host_refused(rule, own, url.host()).unwrap_or("not a host to fetch from")
                    ),
                    (Refused::Credentialed, _) => {
                        "the URL looks like it holds a credential; it is fetched only when the \
                         person you act for wrote it themselves"
                            .to_string()
                    }
                    (Refused::NotInPriorContext, UrlRule::Sender) => format!(
                        "{} is not in the words of the person you act for; fetch only URLs \
                         they wrote",
                        url.normalized()
                    ),
                    (Refused::NotInPriorContext, _) => format!(
                        "{} is not in the sender's words, a search result or a page you \
                         fetched; fetch only URLs from those",
                        url.normalized()
                    ),
                };
                return Err(HostError::new(why.code(), message));
            }
        };
        Ok((src, pages))
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
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

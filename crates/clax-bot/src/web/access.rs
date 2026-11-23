//! What a run may do on the web (spec §4.4): [`WebAccess`], computed from
//! the instance's `web.*` settings, the person's own choice
//! ([`Store::web_choice`]) and whether the host has a source at all.
//!
//! # The shape and the grant
//!
//! A run's [`WebShape`] (which of `clax.web.search` and `clax.web.fetch`
//! exist) is fixed when the run is made and kept on it ([`Run::web`],
//! written from [`WebAccess::shape`]): the system prompt and its API
//! declaration are generated from it ([`crate::api::dts`],
//! [`crate::prompt::system_prompt`]) and stay the same for every turn, so
//! the prompt cache holds. What can change mid-run (the admin turns an
//! operation off, the person opts out, a cap is reached) is read again
//! with [`WebAccess::now`] at each call and answered with an error; a run
//! made without an operation never gains it.
//!
//!
//! # Where a URL may come from
//!
//! [`sources`] derives the URLs a fetch may ask for (spec §5.2) from
//! durable records of what the run received: the sender's own words, the
//! run's journaled search answers and the links of the pages it keeps. It
//! is computed at each fetch, never kept as a set of its own, so a replay
//! derives the same set.
//!
//! [`Run::web`]: clax_store::bot_runs::Run::web

use super::{FetchLimits, OP_SEARCH, Provider, PublicUrl};
use crate::host::RequestContext;
use clax_core::Result;
use clax_store::Store;
use clax_store::bot_runs::CallEntry;
use clax_store::bot_web::WebPageRow;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Which web operations: those a run declares, kept on the run as `""`,
/// `"search"`, `"fetch"` or `"search,fetch"` ([`WebShape::as_str`]), or
/// those a host's source can perform now
/// ([`WebSource::configured`](super::WebSource::configured)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct WebShape {
    pub search: bool,
    pub fetch: bool,
}

impl WebShape {
    /// Both operations.
    pub const BOTH: WebShape = WebShape {
        search: true,
        fetch: true,
    };

    /// The shape `s` names: its comma-separated parts `search` and
    /// `fetch`; anything else names neither, so an unknown value declares
    /// nothing.
    #[must_use]
    pub fn parse(s: &str) -> WebShape {
        let mut shape = WebShape::default();
        for part in s.split(',') {
            match part.trim() {
                "search" => shape.search = true,
                "fetch" => shape.fetch = true,
                _ => {}
            }
        }
        shape
    }

    /// `""`, `"search"`, `"fetch"` or `"search,fetch"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match (self.search, self.fetch) {
            (false, false) => "",
            (true, false) => "search",
            (false, true) => "fetch",
            (true, true) => "search,fetch",
        }
    }

    /// Whether it declares either operation.
    #[must_use]
    pub fn any(self) -> bool {
        self.search || self.fetch
    }
}

/// Why web use is off for a run (the run panel says so, spec §9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebOff {
    /// The admin turned both search and fetch off ("off on this
    /// instance").
    AdminOff,
    /// No live admin grant gives the person either capability ("not
    /// granted to you").
    NotGranted,
    /// The person turned web use off for themselves ("off by your
    /// choice").
    PersonOff,
    /// The host's source can perform none of the operations the admin
    /// turned on: search alone on, on a daemon without a Cloudflare
    /// account ID and token ("not configured").
    NotConfigured,
}

impl WebOff {
    /// `admin_off`, `not_granted`, `person_off` or `not_configured`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WebOff::AdminOff => "admin_off",
            WebOff::NotGranted => "not_granted",
            WebOff::PersonOff => "person_off",
            WebOff::NotConfigured => "not_configured",
        }
    }

    /// The reason [`Off`](crate::capability::Off) gives, in this enum's
    /// words.
    #[must_use]
    pub fn of(off: crate::capability::Off) -> WebOff {
        use crate::capability::Off;
        match off {
            Off::NotOffered => WebOff::AdminOff,
            Off::NotConfigured => WebOff::NotConfigured,
            Off::NotGranted => WebOff::NotGranted,
            Off::PersonOff => WebOff::PersonOff,
        }
    }
}

pub use crate::capability::UrlRule;

/// What a run may search, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchGrant {
    pub provider: Provider,
    /// The AI Gateway ID.
    pub gateway: String,
    /// The gateway's stored provider key; `None` bills Cloudflare credits.
    pub byok_alias: Option<String>,
    /// Results per search, 1 to 10: a script's `limit` defaults to it and
    /// is capped at it.
    pub limit: u8,
    pub max_per_run: u32,
}

/// What a run may fetch, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchGrant {
    pub rule: UrlRule,
    pub max_per_run: u32,
    pub limits: FetchLimits,
}

/// The operations web use is on for: `None` for one the admin turned off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebGrant {
    pub search: Option<SearchGrant>,
    pub fetch: Option<FetchGrant>,
}

/// What a run may do on the web now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebAccess {
    Off(WebOff),
    /// At least one of the grant's operations is `Some`.
    On(WebGrant),
}

/// Where a URL a fetch asks for came from (spec §5.2), as the run panel
/// and the `bot.web` record name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlSource {
    /// The request: one of the sender's comments (its ID), or the brief
    /// (`None`).
    Request { comment_id: Option<String> },
    /// A result of the search journaled at `(step, seq)`, the run's
    /// `n`th answered search (from 1, in journal order).
    Search { step: i64, seq: i64, n: i64 },
    /// A link of the page fetched at `(step, seq)`, the `n`th page of the
    /// run's page store (from 1, in fetch order).
    Fetched { step: i64, seq: i64, n: i64 },
    /// No source: the rule is [`UrlRule::AnyPublic`].
    AnyPublic,
}

impl UrlSource {
    /// How the run panel and the `bot.web` record name it: `comment`,
    /// `brief`, `search #<n>`, `page #<n>` or `any public URL`, where `n`
    /// numbers the run's searches or pages from 1 across all its steps.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            UrlSource::Request {
                comment_id: Some(_),
            } => "comment".into(),
            UrlSource::Request { comment_id: None } => "brief".into(),
            UrlSource::Search { n, .. } => format!("search #{n}"),
            UrlSource::Fetched { n, .. } => format!("page #{n}"),
            UrlSource::AnyPublic => "any public URL".into(),
        }
    }

    /// It as a fetch's journal entry keeps it: `{label, kind}` and, by
    /// kind, `comment_id` (`comment`), or `step`, `seq` and `n`
    /// (`search`, `page`); `kind` is `comment`, `brief`, `search`, `page`
    /// or `any_public`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut v = match self {
            UrlSource::Request {
                comment_id: Some(id),
            } => json!({"kind": "comment", "comment_id": id}),
            UrlSource::Request { comment_id: None } => json!({"kind": "brief"}),
            UrlSource::Search { step, seq, n } => {
                json!({"kind": "search", "step": step, "seq": seq, "n": n})
            }
            UrlSource::Fetched { step, seq, n } => {
                json!({"kind": "page", "step": step, "seq": seq, "n": n})
            }
            UrlSource::AnyPublic => json!({"kind": "any_public"}),
        };
        v["label"] = json!(self.label());
        v
    }
}

/// The `http(s)://` URLs written in `text`, normalised: each runs to the
/// first space, quote, angle or square bracket, brace or backtick, less
/// the sentence punctuation (`.,;:!?`) and unbalanced `)` it ends with.
/// What does not parse as a [`PublicUrl`] is left out.
fn urls_in(text: &str) -> Vec<String> {
    let stop = |c: char| c.is_whitespace() || "\"'<>[]{}`".contains(c);
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = ["http://", "https://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        let tail = &rest[i..];
        let end = tail.find(stop).unwrap_or(tail.len());
        let mut url = &tail[..end];
        loop {
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            let trimmed = match trimmed.strip_suffix(')') {
                Some(t) if t.matches('(').count() < t.matches(')').count() + 1 => t,
                _ => trimmed,
            };
            if trimmed.len() == url.len() {
                break;
            }
            url = trimmed;
        }
        if let Ok(u) = PublicUrl::parse(url) {
            out.push(u.normalized().to_string());
        }
        rest = &tail[end.max(1)..];
    }
    out
}

/// The URLs a fetch of the run may ask for, by normalised URL, each with
/// the first source that holds it (spec §5.2): the sender's own comments
/// (`from_sender`) and the brief in `cx`, then the results of each
/// answered `web.search` in `journal` (the run's calls, in order; a
/// search's entry keeps the script's value under `value`), then
/// the links of each page in `pages` (the run's page store, in fetch
/// order). Nothing else is a source: not the draft, the published
/// artifact, a script's output, other people's comments or anything the
/// run wrote.
#[must_use]
pub fn sources(
    cx: &RequestContext,
    journal: &[CallEntry],
    pages: &[WebPageRow],
) -> BTreeMap<String, UrlSource> {
    let mut out = BTreeMap::new();
    for c in cx.threads.iter().flat_map(|t| &t.comments) {
        if c.from_sender {
            for u in urls_in(&c.text) {
                out.entry(u).or_insert_with(|| UrlSource::Request {
                    comment_id: Some(c.id.clone()),
                });
            }
        }
    }
    for u in cx.brief.as_deref().map(urls_in).unwrap_or_default() {
        out.entry(u)
            .or_insert(UrlSource::Request { comment_id: None });
    }
    let searches = journal.iter().filter(|e| e.op() == OP_SEARCH && e.ok());
    for (n, e) in (1..).zip(searches) {
        let results = &e.result()["value"]["results"];
        for r in results.as_array().into_iter().flatten() {
            if let Some(u) = r["url"].as_str().and_then(|u| PublicUrl::parse(u).ok()) {
                out.entry(u.normalized().to_string())
                    .or_insert(UrlSource::Search {
                        step: e.step_n(),
                        seq: e.seq(),
                        n,
                    });
            }
        }
    }
    for (n, p) in (1..).zip(pages) {
        for l in &p.links {
            if let Ok(u) = PublicUrl::parse(l) {
                out.entry(u.normalized().to_string())
                    .or_insert(UrlSource::Fetched {
                        step: p.step_n,
                        seq: p.seq,
                        n,
                    });
            }
        }
    }
    out
}

/// Why [`admit`] refused a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// One of the instance's own hosts: `url_not_allowed`.
    OwnHost,
    /// Under [`UrlRule::Allowlist`], a host not on the list:
    /// `url_not_allowed`.
    NotListed,
    /// It looks like it holds a credential, and the sender did not write
    /// it: `url_not_allowed`.
    Credentialed,
    /// No source the rule takes holds it (`sender`: not in the sender's
    /// words; `sourced`, `allowlist`: in no source of the run):
    /// `url_not_in_prior_context`.
    NotInPriorContext,
}

impl Refused {
    /// Its code: [`crate::web::CODE_URL_NOT_ALLOWED`] or
    /// [`crate::web::CODE_URL_NOT_IN_PRIOR_CONTEXT`].
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Refused::NotInPriorContext => super::CODE_URL_NOT_IN_PRIOR_CONTEXT,
            Refused::OwnHost | Refused::NotListed | Refused::Credentialed => {
                super::CODE_URL_NOT_ALLOWED
            }
        }
    }
}

/// Whether `url` passes `rule` (spec 2026-11-19-claxbot-browser-design
/// §4.5), the one URL check of `web.fetch` and of a browser session on a
/// URL: not one of the instance's own hosts (`own`), on the allowlist
/// under [`UrlRule::Allowlist`], a credential-looking URL only in the
/// sender's own words, and held by a source the rule takes, of `found`
/// (the run's sources, [`sources`]): under [`UrlRule::Sender`] the
/// sender's words alone, under [`UrlRule::Sourced`] and
/// [`UrlRule::Allowlist`] any, under [`UrlRule::AnyPublic`] none needed
/// ([`UrlSource::AnyPublic`] when none holds it). Checked in that order;
/// the caller has parsed `url` as a [`PublicUrl`] (no credentials, no
/// private literal, `localhost` or other scheme) and words each refusal.
///
/// # Errors
/// Why it is refused.
pub fn admit(
    url: &PublicUrl,
    rule: &UrlRule,
    found: &BTreeMap<String, UrlSource>,
    own: &super::OwnHosts,
) -> std::result::Result<UrlSource, Refused> {
    if own.holds(url.host()) {
        return Err(Refused::OwnHost);
    }
    if !rule.allows_host(url.host()) {
        return Err(Refused::NotListed);
    }
    let src = found.get(url.normalized()).cloned();
    if super::url::looks_credentialed(url) && !matches!(src, Some(UrlSource::Request { .. })) {
        return Err(Refused::Credentialed);
    }
    match (rule, src) {
        (UrlRule::Sender, Some(s @ UrlSource::Request { .. })) => Ok(s),
        (UrlRule::Sender, _) => Err(Refused::NotInPriorContext),
        (_, Some(s)) => Ok(s),
        (UrlRule::AnyPublic, None) => Ok(UrlSource::AnyPublic),
        (_, None) => Err(Refused::NotInPriorContext),
    }
}

impl WebAccess {
    /// Now, for `person`: a view of [`Capabilities::now`] over the two
    /// web capabilities, the host able to perform those `configured`
    /// names. Off when neither is live, for the reason closest to on
    /// ([`Capabilities::why_off`]): the admin turned both off
    /// ([`WebOff::AdminOff`]), the source can perform none of those
    /// offered ([`WebOff::NotConfigured`]), no admin grant gives them
    /// ([`WebOff::NotGranted`]), or the person's grants are revoked
    /// ([`WebOff::PersonOff`]). Else the live ones, with their settings.
    ///
    /// [`Capabilities::now`]: crate::capability::Capabilities::now
    /// [`Capabilities::why_off`]: crate::capability::Capabilities::why_off
    ///
    /// # Errors
    /// The store's.
    pub fn now(store: &Store, person: &str, configured: WebShape) -> Result<WebAccess> {
        use crate::capability::{Capabilities, Capability, Terms};
        let caps = Capabilities::now(store, person, &|c| match c {
            Capability::WebSearch => configured.search,
            Capability::WebFetch => configured.fetch,
            _ => false,
        })?;
        if let Some(off) = caps.why_off(&[Capability::WebSearch, Capability::WebFetch]) {
            return Ok(WebAccess::Off(WebOff::of(off)));
        }
        let search = match caps.get(Capability::WebSearch) {
            Ok(Terms::Search(g)) => Some(g.clone()),
            _ => None,
        };
        let fetch = match caps.get(Capability::WebFetch) {
            Ok(Terms::Fetch(g)) => Some(g.clone()),
            _ => None,
        };
        Ok(WebAccess::On(WebGrant { search, fetch }))
    }

    /// Which operations it grants.
    #[must_use]
    pub fn shape(&self) -> WebShape {
        match self {
            WebAccess::Off(_) => WebShape::default(),
            WebAccess::On(g) => WebShape {
                search: g.search.is_some(),
                fetch: g.fetch.is_some(),
            },
        }
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::audit::AuditCtx;
    use clax_store::bot_runs::{NewRun, RequestKind};
    use clax_store::bot_settings::Setting;
    use clax_store::credits::KeySource;
    use clax_store::roles::OWNER;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }

    fn set(st: &Store, changes: &[(Setting, Value)]) {
        let changes: Vec<(Setting, Option<Value>)> =
            changes.iter().map(|(s, v)| (*s, Some(v.clone()))).collect();
        st.set_bot_settings(&AuditCtx::DAEMON, &changes).unwrap();
        grant_everyone(st);
    }

    /// The instance-wide grant of every capability, once: no capability
    /// reaches a run without one (migration 69).
    fn grant_everyone(st: &Store) {
        use clax_store::bot_grants::{CAPABILITIES, EVERYONE, GrantFilter};
        let live = st
            .cap_grants(&GrantFilter {
                grantee: Some(EVERYONE.into()),
                live: true,
                ..GrantFilter::default()
            })
            .unwrap();
        for cap in CAPABILITIES {
            if !live.iter().any(|g| g.lists(cap)) {
                st.issue_cap_grant(&AuditCtx::DAEMON, cap, clax_store::roles::OWNER, EVERYONE)
                    .unwrap();
            }
        }
    }

    #[test]
    fn shapes_round_trip_through_their_text() {
        for s in ["", "search", "fetch", "search,fetch"] {
            assert_eq!(WebShape::parse(s).as_str(), s);
        }
        assert_eq!(WebShape::parse("fetch,search").as_str(), "search,fetch");
        assert_eq!(WebShape::parse("browse"), WebShape::default());
        assert_eq!(WebOff::AdminOff.as_str(), "admin_off");
        assert_eq!(WebOff::PersonOff.as_str(), "person_off");
        assert_eq!(WebOff::NotConfigured.as_str(), "not_configured");
    }

    #[test]
    fn web_is_off_until_the_admin_turns_it_on() {
        let (_d, st) = store();
        assert_eq!(
            WebAccess::now(&st, OWNER, WebShape::BOTH).unwrap(),
            WebAccess::Off(WebOff::AdminOff)
        );
        set(&st, &[(Setting::WebSearchEnabled, json!(true))]);
        let a = WebAccess::now(&st, OWNER, WebShape::BOTH).unwrap();
        assert_eq!(
            a,
            WebAccess::On(WebGrant {
                search: Some(SearchGrant {
                    provider: Provider::Ceramic,
                    gateway: "default".into(),
                    byok_alias: None,
                    limit: 5,
                    max_per_run: 10,
                }),
                fetch: None,
            })
        );
        assert_eq!(a.shape().as_str(), "search");
        set(
            &st,
            &[
                (Setting::WebFetchEnabled, json!(true)),
                (Setting::WebSearchProvider, json!("linkup")),
                (Setting::WebSearchByokAlias, json!("mykey")),
                (Setting::WebSearchLimit, json!(8)),
                (Setting::WebMaxFetchesPerRun, json!(3)),
                (Setting::WebFetchUrlRule, json!("allowlist")),
                (Setting::WebFetchAllowlist, json!(["example.com"])),
            ],
        );
        let WebAccess::On(g) = WebAccess::now(&st, OWNER, WebShape::BOTH).unwrap() else {
            panic!("on");
        };
        let s = g.search.unwrap();
        assert_eq!(
            (s.provider, s.byok_alias.as_deref(), s.limit),
            (Provider::Linkup, Some("mykey"), 8)
        );
        assert_eq!(
            g.fetch,
            Some(FetchGrant {
                rule: UrlRule::Allowlist(vec!["example.com".into()]),
                max_per_run: 3,
                limits: FetchLimits::default(),
            })
        );
        set(&st, &[(Setting::WebSearchEnabled, json!(false))]);
        assert_eq!(
            WebAccess::now(&st, OWNER, WebShape::BOTH)
                .unwrap()
                .shape()
                .as_str(),
            "fetch"
        );
    }

    #[test]
    fn a_person_who_chose_off_gets_off() {
        let (_d, st) = store();
        set(
            &st,
            &[
                (Setting::WebSearchEnabled, json!(true)),
                (Setting::WebFetchEnabled, json!(true)),
            ],
        );
        assert_eq!(
            WebAccess::now(&st, OWNER, WebShape::BOTH)
                .unwrap()
                .shape()
                .as_str(),
            "search,fetch"
        );
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, false).unwrap();
        let a = WebAccess::now(&st, OWNER, WebShape::BOTH).unwrap();
        assert_eq!(a, WebAccess::Off(WebOff::PersonOff));
        assert_eq!(a.shape(), WebShape::default());
    }

    #[test]
    fn unconfigured_hosts_get_off() {
        let (_d, st) = store();
        set(&st, &[(Setting::WebFetchEnabled, json!(true))]);
        assert_eq!(
            WebAccess::now(&st, OWNER, WebShape::default()).unwrap(),
            WebAccess::Off(WebOff::NotConfigured)
        );
        // The instance's reason comes before the person's.
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, false).unwrap();
        assert_eq!(
            WebAccess::now(&st, OWNER, WebShape::default()).unwrap(),
            WebAccess::Off(WebOff::NotConfigured)
        );
    }

    #[test]
    fn only_what_the_source_can_perform_is_granted() {
        let (_d, st) = store();
        set(
            &st,
            &[
                (Setting::WebSearchEnabled, json!(true)),
                (Setting::WebFetchEnabled, json!(true)),
            ],
        );
        let fetch_only = WebShape {
            search: false,
            fetch: true,
        };
        let WebAccess::On(g) = WebAccess::now(&st, OWNER, fetch_only).unwrap() else {
            panic!("fetch is on and the source can fetch");
        };
        assert!(g.search.is_none() && g.fetch.is_some());
        // Search alone on, and the source cannot search: not configured.
        set(&st, &[(Setting::WebFetchEnabled, json!(false))]);
        assert_eq!(
            WebAccess::now(&st, OWNER, fetch_only).unwrap(),
            WebAccess::Off(WebOff::NotConfigured)
        );
    }

    #[test]
    fn a_run_keeps_the_shape_it_started_with() {
        let (_d, st) = store();
        set(&st, &[(Setting::WebSearchEnabled, json!(true))]);
        let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
            "title": "Report",
            "files": {"index.html": {"content": "<!doctype html><title>R</title>", "encoding": "utf8"}}
        }))
        .unwrap();
        let aid = st
            .create_artifact(
                &AuditCtx::DAEMON,
                clax_core::publish::validate(req).unwrap(),
                None,
            )
            .unwrap()
            .0
            .id;
        let shape = WebAccess::now(&st, OWNER, WebShape::BOTH).unwrap().shape();
        let run = st
            .create_run(
                &crate::run::bot_ctx(OWNER),
                NewRun {
                    artifact_id: aid,
                    person: OWNER.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec!["t1".into()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                    caps: clax_store::bot_runs::caps_of_web(shape.as_str()),
                },
            )
            .unwrap();
        set(&st, &[(Setting::WebSearchEnabled, json!(false))]);
        assert_eq!(
            WebAccess::now(&st, OWNER, WebShape::BOTH).unwrap(),
            WebAccess::Off(WebOff::AdminOff)
        );
        let kept = st.run(&run.id).unwrap().unwrap();
        assert_eq!(
            WebShape::parse(&kept.web),
            WebShape {
                search: true,
                fetch: false
            }
        );
    }

    #[test]
    fn urls_are_read_from_text_without_their_punctuation() {
        assert_eq!(
            urls_in(
                "See https://Example.com:443/a, (and http://b.example/x_(y)) \
                 or <https://c.example/?q=1#frag>. Not ftp://d.example or https://localhost/."
            ),
            [
                "https://example.com/a",
                "http://b.example/x_(y)",
                "https://c.example/?q=1"
            ]
        );
        assert_eq!(
            urls_in("[docs](https://e.example/d) and `https://f.example`"),
            ["https://e.example/d", "https://f.example/"]
        );
        assert!(urls_in("no links, just https:// alone").is_empty());
    }

    /// The one URL check, as a fetch asks it: each rule against the
    /// sender's words, a search answer and a fetched link, and the checks
    /// every rule makes.
    #[test]
    fn sender_admits_only_the_senders_urls() {
        let own = super::super::OwnHosts::new(Some("https://clax.example"), None).unwrap();
        let request = UrlSource::Request {
            comment_id: Some("c1".into()),
        };
        let search = UrlSource::Search {
            step: 1,
            seq: 0,
            n: 1,
        };
        let page = UrlSource::Fetched {
            step: 1,
            seq: 1,
            n: 1,
        };
        let found: BTreeMap<String, UrlSource> = [
            ("https://said.example/a", request.clone()),
            ("https://said.example/?token=1", request.clone()),
            ("https://found.example/b", search.clone()),
            ("https://linked.example/c", page.clone()),
            ("https://found.example/?api_key=2", search.clone()),
            ("https://clax.example/x", request.clone()),
        ]
        .into_iter()
        .map(|(u, s)| (u.to_string(), s))
        .collect();
        let check =
            |url: &str, rule: &UrlRule| admit(&PublicUrl::parse(url).unwrap(), rule, &found, &own);
        let allow = UrlRule::Allowlist(vec!["said.example".into(), "found.example".into()]);
        for (url, rule, want) in [
            (
                "https://said.example/a",
                UrlRule::Sender,
                Ok(request.clone()),
            ),
            (
                "https://found.example/b",
                UrlRule::Sender,
                Err(Refused::NotInPriorContext),
            ),
            (
                "https://linked.example/c",
                UrlRule::Sender,
                Err(Refused::NotInPriorContext),
            ),
            (
                "https://found.example/b",
                UrlRule::Sourced,
                Ok(search.clone()),
            ),
            (
                "https://linked.example/c",
                UrlRule::Sourced,
                Ok(page.clone()),
            ),
            (
                "https://else.example/",
                UrlRule::Sourced,
                Err(Refused::NotInPriorContext),
            ),
            ("https://found.example/b", allow.clone(), Ok(search.clone())),
            (
                "https://linked.example/c",
                allow.clone(),
                Err(Refused::NotListed),
            ),
            (
                "https://said.example/z",
                allow.clone(),
                Err(Refused::NotInPriorContext),
            ),
            (
                "https://else.example/",
                UrlRule::AnyPublic,
                Ok(UrlSource::AnyPublic),
            ),
            (
                "https://found.example/b",
                UrlRule::AnyPublic,
                Ok(search.clone()),
            ),
        ] {
            assert_eq!(check(url, &rule), want, "{url} under {rule:?}");
        }
        for rule in [
            UrlRule::Sender,
            UrlRule::Sourced,
            allow.clone(),
            UrlRule::AnyPublic,
        ] {
            assert_eq!(
                check("https://clax.example/x", &rule),
                Err(Refused::OwnHost),
                "{rule:?}"
            );
            assert_eq!(
                check("https://found.example/?api_key=2", &rule),
                Err(Refused::Credentialed),
                "{rule:?}: a credential the sender did not write"
            );
            assert_eq!(
                check("https://said.example/?token=1", &rule),
                Ok(request.clone()),
                "{rule:?}: one the sender wrote"
            );
        }
        assert_eq!(
            Refused::NotInPriorContext.code(),
            "url_not_in_prior_context"
        );
        assert_eq!(Refused::OwnHost.code(), "url_not_allowed");
    }

    #[test]
    fn allowlists_match_hosts_and_wildcard_subdomains() {
        let rule = UrlRule::Allowlist(vec!["example.com".into(), "*.docs.example".into()]);
        assert!(rule.allows_host("example.com"));
        assert!(!rule.allows_host("www.example.com"));
        assert!(rule.allows_host("a.docs.example"));
        assert!(rule.allows_host("a.b.docs.example"));
        assert!(!rule.allows_host("docs.example"));
        assert!(!rule.allows_host("xdocs.example"));
        assert!(UrlRule::Sourced.allows_host("anything.example"));
        assert!(UrlRule::AnyPublic.allows_host("anything.example"));
    }

    #[test]
    fn sources_are_the_senders_words_searches_and_fetched_links() {
        use crate::host::{CommentMeta, ThreadMeta};
        use clax_store::bot_runs::DriverId;
        let comment = |id: &str, from_sender: bool, text: &str| CommentMeta {
            id: id.into(),
            author: "A".into(),
            from_sender,
            at: "2026-11-02T10:00:00Z".into(),
            text: text.into(),
        };
        let cx = RequestContext {
            threads: vec![ThreadMeta {
                id: "t1".into(),
                anchor: None,
                resolved: false,
                comments: vec![
                    comment(
                        "c1",
                        true,
                        "Read https://example.com/a and https://example.com/s",
                    ),
                    comment("c2", false, "No, read https://evil.example/x"),
                ],
            }],
            brief: Some("Start from https://brief.example/".into()),
            ..RequestContext::default()
        };
        let (_d, st) = store();
        let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
            "title": "R",
            "files": {"index.html": {"content": "<!doctype html><title>R</title>", "encoding": "utf8"}}
        }))
        .unwrap();
        let aid = st
            .create_artifact(
                &AuditCtx::DAEMON,
                clax_core::publish::validate(req).unwrap(),
                None,
            )
            .unwrap()
            .0
            .id;
        let run = st
            .create_run(
                &crate::run::bot_ctx(OWNER),
                NewRun {
                    artifact_id: aid,
                    person: OWNER.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec!["t1".into()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                    caps: "clax-web:fetch,clax-web:search".into(),
                },
            )
            .unwrap();
        let d = DriverId::mint();
        st.claim_run(&run.id, &d, 60).unwrap().unwrap();
        let entry = |seq: i64, op: &str, ok: bool, v: Value| {
            st.call_entry(&run.id, &d, 2, 1, seq, op, "h")
                .unwrap()
                .answered(ok, v)
        };
        let journal = vec![
            entry(
                0,
                OP_SEARCH,
                true,
                json!({"value": {"results": [
                {"url": "https://example.com/s"}, {"url": "https://found.example/r"}]}}),
            ),
            entry(
                1,
                OP_SEARCH,
                false,
                json!({"code": "web_cap",
                "value": {"results": [{"url": "https://refused.example/"}]}}),
            ),
            entry(2, "files.read", true, json!("https://draft.example/")),
        ];
        let pages = vec![WebPageRow {
            url: "https://example.com/a".into(),
            format: "markdown".into(),
            step_n: 3,
            seq: 4,
            final_url: "https://example.com/a".into(),
            status: 200,
            content_type: "text/html".into(),
            text: "https://text-only.example/".into(),
            total_chars: 26,
            links: vec![
                "https://linked.example/l".into(),
                "https://found.example/r".into(),
            ],
        }];
        let got = sources(&cx, &journal, &pages);
        let c1 = UrlSource::Request {
            comment_id: Some("c1".into()),
        };
        assert_eq!(
            got.into_iter().collect::<Vec<_>>(),
            [
                (
                    "https://brief.example/".to_string(),
                    UrlSource::Request { comment_id: None }
                ),
                ("https://example.com/a".to_string(), c1.clone()),
                // The sender's words come before a search that found it too.
                ("https://example.com/s".to_string(), c1),
                (
                    "https://found.example/r".to_string(),
                    UrlSource::Search {
                        step: 2,
                        seq: 0,
                        n: 1
                    }
                ),
                (
                    "https://linked.example/l".to_string(),
                    UrlSource::Fetched {
                        step: 3,
                        seq: 4,
                        n: 1
                    }
                ),
            ]
        );
        let s = UrlSource::Search {
            step: 2,
            seq: 0,
            n: 3,
        };
        assert_eq!(s.label(), "search #3");
        assert_eq!(
            s.to_json(),
            json!({"kind": "search", "step": 2, "seq": 0, "n": 3, "label": "search #3"})
        );
        assert_eq!(
            UrlSource::Fetched {
                step: 3,
                seq: 4,
                n: 2
            }
            .label(),
            "page #2"
        );
        assert_eq!(UrlSource::Request { comment_id: None }.label(), "brief");
    }
}

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
use clax_store::bot_settings::Setting;
use clax_store::bot_web::WebPageRow;
use serde_json::Value;
use std::collections::BTreeMap;

/// Which web operations a run declares: kept on the run as `""`,
/// `"search"`, `"fetch"` or `"search,fetch"` ([`WebShape::as_str`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct WebShape {
    pub search: bool,
    pub fetch: bool,
}

impl WebShape {
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
    /// The person turned web use off for themselves ("off by your
    /// choice").
    PersonOff,
    /// The host has no source: the daemon without a Cloudflare account ID
    /// and token ("not configured").
    NotConfigured,
}

impl WebOff {
    /// `admin_off`, `person_off` or `not_configured`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WebOff::AdminOff => "admin_off",
            WebOff::PersonOff => "person_off",
            WebOff::NotConfigured => "not_configured",
        }
    }
}

/// Which URLs a fetch may ask for (`web.fetch.url_rule`, spec §5.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlRule {
    /// Only URLs a source of the run holds (spec §5.2): the default.
    Sourced,
    /// Any public URL.
    AnyPublic,
    /// URLs a source of the run holds, on these hosts (lowercase, as
    /// `web.fetch.allowlist` keeps them): `example.com` is that host
    /// alone, `*.example.com` its subdomains and not `example.com`.
    Allowlist(Vec<String>),
}

impl UrlRule {
    /// Whether the rule lets a fetch reach `host` (lowercase, as
    /// [`PublicUrl::host`] gives it): always, but under
    /// [`UrlRule::Allowlist`], where the host must be on the list. It is
    /// the host check of a redirect target too (spec §5.3).
    #[must_use]
    pub fn allows_host(&self, host: &str) -> bool {
        let UrlRule::Allowlist(hosts) = self else {
            return true;
        };
        let host = host.strip_suffix('.').unwrap_or(host);
        hosts.iter().any(|h| match h.strip_prefix("*.") {
            Some(base) => host
                .strip_suffix(base)
                .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
            None => host == h,
        })
    }
}

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
    /// A result of the search journaled at `(step, seq)`.
    Search { step: i64, seq: i64 },
    /// A link of the page fetched at `(step, seq)`.
    Fetched { step: i64, seq: i64 },
    /// No source: the rule is [`UrlRule::AnyPublic`].
    AnyPublic,
}

impl UrlSource {
    /// How the run panel and the `bot.web` record name it: `comment`,
    /// `brief`, `search #<seq>`, `page #<seq>` or `any public URL`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            UrlSource::Request {
                comment_id: Some(_),
            } => "comment".into(),
            UrlSource::Request { comment_id: None } => "brief".into(),
            UrlSource::Search { seq, .. } => format!("search #{seq}"),
            UrlSource::Fetched { seq, .. } => format!("page #{seq}"),
            UrlSource::AnyPublic => "any public URL".into(),
        }
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
/// answered `web.search` in `journal` (the run's calls, in order), then
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
    for e in journal.iter().filter(|e| e.op() == OP_SEARCH && e.ok()) {
        for r in e.result()["results"].as_array().into_iter().flatten() {
            if let Some(u) = r["url"].as_str().and_then(|u| PublicUrl::parse(u).ok()) {
                out.entry(u.normalized().to_string())
                    .or_insert(UrlSource::Search {
                        step: e.step_n(),
                        seq: e.seq(),
                    });
            }
        }
    }
    for p in pages {
        for l in &p.links {
            if let Ok(u) = PublicUrl::parse(l) {
                out.entry(u.normalized().to_string())
                    .or_insert(UrlSource::Fetched {
                        step: p.step_n,
                        seq: p.seq,
                    });
            }
        }
    }
    out
}

impl WebAccess {
    /// Now, for `person`: off when the admin turned both operations off
    /// ([`WebOff::AdminOff`]), when `configured` is false (the host has no
    /// source, [`WebOff::NotConfigured`]), or when the person chose off
    /// ([`WebOff::PersonOff`]), in that order: the instance's reasons come
    /// first, since the person's switch would not turn web use on. Else
    /// the operations the admin turned on, with their settings.
    ///
    /// # Errors
    /// The store's.
    pub fn now(store: &Store, person: &str, configured: bool) -> Result<WebAccess> {
        let on = |s: Setting| -> Result<bool> { Ok(store.bot_setting(s)?.as_bool() == Some(true)) };
        let search_on = on(Setting::WebSearchEnabled)?;
        let fetch_on = on(Setting::WebFetchEnabled)?;
        if !search_on && !fetch_on {
            return Ok(WebAccess::Off(WebOff::AdminOff));
        }
        if !configured {
            return Ok(WebAccess::Off(WebOff::NotConfigured));
        }
        if !store.web_choice(person)? {
            return Ok(WebAccess::Off(WebOff::PersonOff));
        }
        let text = |s: Setting| -> Result<String> {
            Ok(store.bot_setting(s)?.as_str().unwrap_or("").to_string())
        };
        let count = |s: Setting| -> Result<u32> {
            let v = store.bot_setting(s)?;
            Ok(v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .or_else(|| {
                    s.default_value()
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                })
                .unwrap_or(0))
        };
        let search = if search_on {
            // The setting is checked on write (`Setting::check`), so the
            // fallback is never reached in practice.
            let provider = match text(Setting::WebSearchProvider)?.as_str() {
                "exa" => Provider::Exa,
                "linkup" => Provider::Linkup,
                _ => Provider::Ceramic,
            };
            let alias = text(Setting::WebSearchByokAlias)?;
            Some(SearchGrant {
                provider,
                gateway: text(Setting::WebSearchGateway)?,
                byok_alias: (!alias.is_empty()).then_some(alias),
                limit: u8::try_from(count(Setting::WebSearchLimit)?.clamp(1, 10)).unwrap_or(5),
                max_per_run: count(Setting::WebMaxSearchesPerRun)?,
            })
        } else {
            None
        };
        let fetch = if fetch_on {
            let rule = match text(Setting::WebFetchUrlRule)?.as_str() {
                "any_public" => UrlRule::AnyPublic,
                "allowlist" => UrlRule::Allowlist(
                    store
                        .bot_setting(Setting::WebFetchAllowlist)?
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                _ => UrlRule::Sourced,
            };
            Some(FetchGrant {
                rule,
                max_per_run: count(Setting::WebMaxFetchesPerRun)?,
                limits: FetchLimits::default(),
            })
        } else {
            None
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
mod tests {
    use super::*;
    use clax_core::audit::AuditCtx;
    use clax_store::bot_runs::{NewRun, RequestKind};
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
            WebAccess::now(&st, OWNER, true).unwrap(),
            WebAccess::Off(WebOff::AdminOff)
        );
        set(&st, &[(Setting::WebSearchEnabled, json!(true))]);
        let a = WebAccess::now(&st, OWNER, true).unwrap();
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
        let WebAccess::On(g) = WebAccess::now(&st, OWNER, true).unwrap() else {
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
            WebAccess::now(&st, OWNER, true).unwrap().shape().as_str(),
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
            WebAccess::now(&st, OWNER, true).unwrap().shape().as_str(),
            "search,fetch"
        );
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, false).unwrap();
        let a = WebAccess::now(&st, OWNER, true).unwrap();
        assert_eq!(a, WebAccess::Off(WebOff::PersonOff));
        assert_eq!(a.shape(), WebShape::default());
    }

    #[test]
    fn unconfigured_hosts_get_off() {
        let (_d, st) = store();
        set(&st, &[(Setting::WebFetchEnabled, json!(true))]);
        assert_eq!(
            WebAccess::now(&st, OWNER, false).unwrap(),
            WebAccess::Off(WebOff::NotConfigured)
        );
        // The instance's reason comes before the person's.
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, false).unwrap();
        assert_eq!(
            WebAccess::now(&st, OWNER, false).unwrap(),
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
        let shape = WebAccess::now(&st, OWNER, true).unwrap().shape();
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
                    web: shape.as_str().into(),
                },
            )
            .unwrap();
        set(&st, &[(Setting::WebSearchEnabled, json!(false))]);
        assert_eq!(
            WebAccess::now(&st, OWNER, true).unwrap(),
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
                    web: "search,fetch".into(),
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
                json!({"results": [
                {"url": "https://example.com/s"}, {"url": "https://found.example/r"}]}),
            ),
            entry(
                1,
                OP_SEARCH,
                false,
                json!({"code": "web_cap",
                "results": [{"url": "https://refused.example/"}]}),
            ),
            entry(2, "files.read", true, json!("https://draft.example/")),
        ];
        let pages = vec![WebPageRow {
            url: "https://example.com/a".into(),
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
                    UrlSource::Search { step: 2, seq: 0 }
                ),
                (
                    "https://linked.example/l".to_string(),
                    UrlSource::Fetched { step: 3, seq: 4 }
                ),
            ]
        );
        assert_eq!(UrlSource::Search { step: 2, seq: 0 }.label(), "search #0");
        assert_eq!(UrlSource::Fetched { step: 3, seq: 4 }.label(), "page #4");
        assert_eq!(UrlSource::Request { comment_id: None }.label(), "brief");
    }
}

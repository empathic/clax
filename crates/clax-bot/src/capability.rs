//! A run's capabilities, computed one way (spec
//! 2026-11-19-claxbot-browser-design §4.2, §4.3).
//!
//! Five capabilities, each offered, granted, capped and revoked on its own
//! ([`Capability`]). For a person, [`Capabilities::now`] answers each with
//! its terms, or why it is off ([`Off`]): a capability is live when the
//! admin's switch offers it, the host can perform it, an admin grant gives
//! it to the person and the person's grant of it to their runs is live
//! ([`Store::person_caps`]). Nothing is implied: with no grant, nothing is
//! live.
//!
//! A run keeps the capabilities live when it was made ([`CapSet`],
//! `bot_runs.caps`); its declaration is generated from them. Each call
//! computes [`Capabilities::now`] again and refuses one whose capability is
//! no longer live.
//!
//! [`Store::person_caps`]: clax_store::Store::person_caps

use crate::web::{FetchGrant, FetchLimits, Provider, SearchGrant};
use clax_core::Result;
use clax_store::Store;
use clax_store::bot_settings::Setting;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The code shared by every family (spec §4.7): the person cannot pay
/// the call's price cap.
pub const CODE_CREDITS_EXHAUSTED: &str = "credits_exhausted";
/// The code shared by every family: no source of the run holds the URL
/// (`sender`, `sourced`, `allowlist`, §4.5).
pub const CODE_URL_NOT_IN_PRIOR_CONTEXT: &str = "url_not_in_prior_context";
/// The code shared by every family: the host check or the allowlist
/// refuses the URL, or a redirect target.
pub const CODE_URL_NOT_ALLOWED: &str = "url_not_allowed";

/// One capability (spec §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// `clax.web.search`.
    WebSearch,
    /// `clax.web.fetch`.
    WebFetch,
    /// `clax.browser.open({url})`, and a `goto` out of a session's origin.
    OpenUrl,
    /// `clax.browser.open({artifact})` and `clax.browser.test`.
    OpenArtifact,
    /// `clax.browser.open({html})`.
    OpenContent,
}

/// The family a capability is billed in (spec §4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Retrieval, `clax.web`, per request.
    Web,
    /// A running browser, `clax.browser`, by time.
    Browser,
}

impl Capability {
    /// Every capability, in the order they are shown.
    pub const ALL: [Capability; 5] = [
        Capability::WebSearch,
        Capability::WebFetch,
        Capability::OpenUrl,
        Capability::OpenArtifact,
        Capability::OpenContent,
    ];

    /// Its ID, as grants and `bot_runs.caps` name it.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Capability::WebSearch => "clax-web:search",
            Capability::WebFetch => "clax-web:fetch",
            Capability::OpenUrl => "clax-browser:open-url",
            Capability::OpenArtifact => "clax-browser:open-artifact",
            Capability::OpenContent => "clax-browser:open-content",
        }
    }

    /// The capability `id` names.
    #[must_use]
    pub fn parse(id: &str) -> Option<Capability> {
        Capability::ALL.into_iter().find(|c| c.id() == id)
    }

    /// Its family.
    #[must_use]
    pub fn family(self) -> Family {
        match self {
            Capability::WebSearch | Capability::WebFetch => Family::Web,
            Capability::OpenUrl | Capability::OpenArtifact | Capability::OpenContent => {
                Family::Browser
            }
        }
    }

    /// The admin's switch that offers it.
    #[must_use]
    pub fn switch(self) -> Setting {
        match self {
            Capability::WebSearch => Setting::WebSearchEnabled,
            Capability::WebFetch => Setting::WebFetchEnabled,
            Capability::OpenUrl => Setting::BrowserOpenUrlEnabled,
            Capability::OpenArtifact => Setting::BrowserOpenArtifactEnabled,
            Capability::OpenContent => Setting::BrowserOpenContentEnabled,
        }
    }
}

/// A set of capabilities: a run's, kept as `bot_runs.caps`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapSet(BTreeSet<Capability>);

impl CapSet {
    /// The set `text` names: its comma-separated capability IDs; a part
    /// that names none is ignored.
    #[must_use]
    pub fn parse(text: &str) -> CapSet {
        CapSet(text.split(',').filter_map(Capability::parse).collect())
    }

    /// Its text: the IDs sorted and comma-separated, as
    /// [`clax_store::bot_runs::caps_text`] keeps them.
    #[must_use]
    pub fn as_str(&self) -> String {
        clax_store::bot_runs::caps_text(self.0.iter().map(|c| c.id()))
    }

    /// Whether it holds `c`.
    #[must_use]
    pub fn has(&self, c: Capability) -> bool {
        self.0.contains(&c)
    }

    /// It with `c`.
    pub fn insert(&mut self, c: Capability) {
        self.0.insert(c);
    }

    /// Whether it holds any capability of `family`.
    #[must_use]
    pub fn any_of(&self, family: Family) -> bool {
        self.0.iter().any(|c| c.family() == family)
    }

    /// Its capabilities, in [`Capability::ALL`]'s order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }
}

impl FromIterator<Capability> for CapSet {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        CapSet(iter.into_iter().collect())
    }
}

/// Why a capability is off for a person (spec §4.3), in the order they
/// are told: the instance's reasons first, since no grant would turn the
/// capability on, then the admin's grant, then the person's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Off {
    /// The admin's switch for it is off ("off on this instance").
    NotOffered,
    /// The host cannot perform it now: no credentials, or no source ("not
    /// configured").
    NotConfigured,
    /// No live admin grant gives it to the person ("not granted to you").
    NotGranted,
    /// The person's grant of it to their runs is revoked ("off by your
    /// choice").
    PersonOff,
}

impl Off {
    /// `not_offered`, `not_configured`, `not_granted` or `person_off`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Off::NotOffered => "not_offered",
            Off::NotConfigured => "not_configured",
            Off::NotGranted => "not_granted",
            Off::PersonOff => "person_off",
        }
    }
}

/// What a run may open a browser on, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenGrant {
    /// [`Capability::OpenUrl`], [`Capability::OpenArtifact`] or
    /// [`Capability::OpenContent`].
    pub kind: Capability,
    /// Sessions of this kind one run opens (for the artifact kind, tests
    /// too).
    pub max_per_run: u32,
    /// For [`Capability::OpenUrl`], which URLs a session may open and go
    /// to (`browser.open-url.url_rule`).
    pub url_rule: Option<UrlRule>,
}

/// A live capability's terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Terms {
    Search(SearchGrant),
    Fetch(FetchGrant),
    Open(OpenGrant),
}

/// The browser family's settings, shared by every `browser.open` kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrowserTerms {
    /// One call's time cap, 10 to 120 seconds (`browser.call_seconds`).
    pub call_seconds: u32,
    /// A session with no call for this long ends, 60 to 1,200 seconds.
    pub idle_seconds: u32,
    /// A session open this long ends, 60 to 7,200 seconds.
    pub max_seconds: u32,
    /// A run's sessions open at once, 1 to 4.
    pub max_open_per_run: u32,
}

/// Each capability's terms, or why it is off, for one person now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    each: BTreeMap<Capability, std::result::Result<Terms, Off>>,
    browser: Option<BrowserTerms>,
}

/// Which URLs a capability that takes one admits (spec §4.5), granted
/// separately to `web.fetch` (`web.fetch.url_rule`) and
/// `clax-browser:open-url` (`browser.open-url.url_rule`, which also governs
/// every `goto` out of a session's origin). [`crate::web::access::admit`]
/// applies it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlRule {
    /// Only URLs in the sender's own words: a `from_sender` comment of
    /// the request, or the brief.
    Sender,
    /// Only URLs a source of the run holds (web tools spec §5.2): the
    /// sender's words, a search answer, a fetched page's link.
    /// `web.fetch`'s default.
    Sourced,
    /// Any public URL.
    AnyPublic,
    /// URLs a source of the run holds, on these hosts (lowercase, as the
    /// capability's allowlist keeps them): `example.com` is that host
    /// alone, `*.example.com` its subdomains and not `example.com`.
    Allowlist(Vec<String>),
}

impl UrlRule {
    /// Whether the rule lets a fetch reach `host` (lowercase, as
    /// [`PublicUrl::host`](crate::web::PublicUrl::host) gives it): always, but under
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

/// A whole-number setting as `u32`, its default where the stored value
/// is not one (never, as settings are checked on write).
fn count(store: &Store, s: Setting) -> Result<u32> {
    let as_u32 = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
    Ok(as_u32(&store.bot_setting(s)?)
        .or_else(|| as_u32(&s.default_value()))
        .unwrap_or(0))
}

fn text(store: &Store, s: Setting) -> Result<String> {
    Ok(store.bot_setting(s)?.as_str().unwrap_or("").to_string())
}

fn hosts(store: &Store, s: Setting) -> Result<Vec<String>> {
    Ok(store
        .bot_setting(s)?
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// The URL rule `rule` (a setting's value) names, with `allowlist`'s
/// hosts; one it does not name is `sourced`, never reached, as settings
/// are checked on write.
fn url_rule(store: &Store, rule: Setting, allowlist: Setting) -> Result<UrlRule> {
    Ok(match text(store, rule)?.as_str() {
        "sender" => UrlRule::Sender,
        "any_public" => UrlRule::AnyPublic,
        "allowlist" => UrlRule::Allowlist(hosts(store, allowlist)?),
        _ => UrlRule::Sourced,
    })
}

fn terms(store: &Store, c: Capability) -> Result<Terms> {
    Ok(match c {
        Capability::WebSearch => {
            let provider = match text(store, Setting::WebSearchProvider)?.as_str() {
                "exa" => Provider::Exa,
                "linkup" => Provider::Linkup,
                _ => Provider::Ceramic,
            };
            let alias = text(store, Setting::WebSearchByokAlias)?;
            Terms::Search(SearchGrant {
                provider,
                gateway: text(store, Setting::WebSearchGateway)?,
                byok_alias: (!alias.is_empty()).then_some(alias),
                limit: u8::try_from(count(store, Setting::WebSearchLimit)?.clamp(1, 10))
                    .unwrap_or(5),
                max_per_run: count(store, Setting::WebMaxSearchesPerRun)?,
            })
        }
        Capability::WebFetch => Terms::Fetch(FetchGrant {
            rule: url_rule(store, Setting::WebFetchUrlRule, Setting::WebFetchAllowlist)?,
            max_per_run: count(store, Setting::WebMaxFetchesPerRun)?,
            limits: FetchLimits::default(),
        }),
        Capability::OpenUrl => Terms::Open(OpenGrant {
            kind: c,
            max_per_run: count(store, Setting::BrowserOpenUrlMaxPerRun)?,
            url_rule: Some(url_rule(
                store,
                Setting::BrowserOpenUrlUrlRule,
                Setting::BrowserOpenUrlAllowlist,
            )?),
        }),
        Capability::OpenArtifact => Terms::Open(OpenGrant {
            kind: c,
            max_per_run: count(store, Setting::BrowserOpenArtifactMaxPerRun)?,
            url_rule: None,
        }),
        Capability::OpenContent => Terms::Open(OpenGrant {
            kind: c,
            max_per_run: count(store, Setting::BrowserOpenContentMaxPerRun)?,
            url_rule: None,
        }),
    })
}

impl Capabilities {
    /// Each capability for `person` now: off when the admin's switch does
    /// not offer it ([`Off::NotOffered`]), when the host cannot perform it
    /// (`configured`, [`Off::NotConfigured`]), when no live admin grant
    /// gives it to the person ([`Off::NotGranted`]), or when the person's
    /// grant of it to their runs is not live ([`Off::PersonOff`]), the
    /// first of these that holds; else its terms.
    ///
    /// # Errors
    /// The store's.
    pub fn now(
        store: &Store,
        person: &str,
        configured: &dyn Fn(Capability) -> bool,
    ) -> Result<Capabilities> {
        let held = store.person_caps(person)?;
        let mut each = BTreeMap::new();
        for c in Capability::ALL {
            let cap = held.get(c.id()).cloned().unwrap_or_default();
            let v = if store.bot_setting(c.switch())?.as_bool() != Some(true) {
                Err(Off::NotOffered)
            } else if !configured(c) {
                Err(Off::NotConfigured)
            } else if cap.granted_by.is_none() {
                Err(Off::NotGranted)
            } else if cap.runs_grant.is_none() {
                Err(Off::PersonOff)
            } else {
                Ok(terms(store, c)?)
            };
            each.insert(c, v);
        }
        let browser = if each
            .iter()
            .any(|(c, v)| c.family() == Family::Browser && v.is_ok())
        {
            Some(BrowserTerms {
                call_seconds: count(store, Setting::BrowserCallSeconds)?,
                idle_seconds: count(store, Setting::BrowserSessionIdleSeconds)?,
                max_seconds: count(store, Setting::BrowserSessionMaxSeconds)?,
                max_open_per_run: count(store, Setting::BrowserMaxOpenPerRun)?,
            })
        } else {
            None
        };
        Ok(Capabilities { each, browser })
    }

    /// `c`'s terms, or why it is off.
    ///
    /// # Errors
    /// Why `c` is off.
    pub fn get(&self, c: Capability) -> std::result::Result<&Terms, Off> {
        match self.each.get(&c) {
            Some(Ok(t)) => Ok(t),
            Some(Err(off)) => Err(*off),
            None => Err(Off::NotOffered),
        }
    }

    /// The capabilities live now: what a run made now keeps.
    #[must_use]
    pub fn live(&self) -> CapSet {
        self.each
            .iter()
            .filter(|(_, v)| v.is_ok())
            .map(|(c, _)| *c)
            .collect()
    }

    /// The browser family's settings, when any `browser.open` kind is
    /// live.
    #[must_use]
    pub fn browser(&self) -> Option<BrowserTerms> {
        self.browser
    }

    /// Of `caps`, none live: the reason closest to on, the last in
    /// [`Off`]'s order (one capability not offered and another off by the
    /// person's choice is off by their choice); `None` when one is live.
    #[must_use]
    pub fn why_off(&self, caps: &[Capability]) -> Option<Off> {
        let mut why = None;
        for c in caps {
            match self.get(*c) {
                Ok(_) => return None,
                Err(off) => why = why.max(Some(off)),
            }
        }
        why
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::audit::AuditCtx;
    use clax_store::bot_grants::{EVERYONE, GrantFilter, Grantee, RevokeReason};
    use clax_store::roles::OWNER;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }

    fn offer(st: &Store, caps: &[Capability], on: bool) {
        let changes: Vec<_> = caps.iter().map(|c| (c.switch(), Some(json!(on)))).collect();
        st.set_bot_settings(&AuditCtx::DAEMON, &changes).unwrap();
    }

    fn grant(st: &Store, c: Capability, grantee: &str) -> String {
        st.issue_cap_grant(&AuditCtx::DAEMON, c.id(), OWNER, grantee)
            .unwrap()
            .id
    }

    fn all(_: Capability) -> bool {
        true
    }

    fn now(st: &Store, person: &str) -> Capabilities {
        Capabilities::now(st, person, &all).unwrap()
    }

    fn runs_grant(st: &Store, person: &str, c: Capability) -> String {
        st.person_caps(person).unwrap()[c.id()]
            .runs_grant
            .clone()
            .unwrap()
    }

    #[test]
    fn each_capability_is_on_and_off_on_its_own() {
        let (_d, st) = store();
        offer(&st, &Capability::ALL, true);
        for c in Capability::ALL {
            grant(&st, c, EVERYONE);
        }
        assert_eq!(now(&st, OWNER).live().iter().count(), 5);
        for c in Capability::ALL {
            // The admin's switch, alone.
            offer(&st, &[c], false);
            let caps = now(&st, OWNER);
            assert_eq!(caps.get(c), Err(Off::NotOffered), "{}", c.id());
            assert_eq!(caps.live().iter().count(), 4, "{}", c.id());
            offer(&st, &[c], true);
            // The person's grant, alone.
            let mine = runs_grant(&st, OWNER, c);
            st.revoke_cap_grant(&AuditCtx::DAEMON, &mine, RevokeReason::Controller)
                .unwrap();
            let caps = now(&st, OWNER);
            assert_eq!(caps.get(c), Err(Off::PersonOff), "{}", c.id());
            assert_eq!(caps.live().iter().count(), 4, "{}", c.id());
            assert!(
                st.set_cap_choice(&AuditCtx::DAEMON, OWNER, c.id(), true)
                    .unwrap()
            );
            assert_eq!(now(&st, OWNER).live().iter().count(), 5, "{}", c.id());
        }
    }

    #[test]
    fn the_admin_reason_comes_before_the_persons() {
        let (_d, st) = store();
        let c = Capability::OpenContent;
        grant(&st, c, EVERYONE);
        let mine = runs_grant(&st, OWNER, c);
        st.revoke_cap_grant(&AuditCtx::DAEMON, &mine, RevokeReason::Controller)
            .unwrap();
        assert_eq!(now(&st, OWNER).get(c), Err(Off::NotOffered));
        let none = |_: Capability| false;
        offer(&st, &[c], true);
        assert_eq!(
            Capabilities::now(&st, OWNER, &none).unwrap().get(c),
            Err(Off::NotConfigured)
        );
        assert_eq!(now(&st, OWNER).get(c), Err(Off::PersonOff));
        let caps = now(&st, OWNER);
        assert_eq!(
            caps.why_off(&[Capability::OpenUrl, c]),
            Some(Off::PersonOff),
            "the reason closest to on"
        );
        assert_eq!(Off::NotGranted.as_str(), "not_granted");
    }

    #[test]
    fn not_configured_per_capability() {
        let (_d, st) = store();
        offer(&st, &Capability::ALL, true);
        for c in Capability::ALL {
            grant(&st, c, EVERYONE);
        }
        let fetch_only = |c: Capability| c == Capability::WebFetch;
        let caps = Capabilities::now(&st, OWNER, &fetch_only).unwrap();
        assert_eq!(caps.live(), CapSet::parse("clax-web:fetch"));
        for c in Capability::ALL
            .into_iter()
            .filter(|c| *c != Capability::WebFetch)
        {
            assert_eq!(caps.get(c), Err(Off::NotConfigured), "{}", c.id());
        }
        assert_eq!(caps.browser(), None, "no browser kind is live");
    }

    #[test]
    fn an_offered_capability_with_no_grant_is_not_granted() {
        let (_d, st) = store();
        offer(&st, &Capability::ALL, true);
        let caps = now(&st, OWNER);
        for c in Capability::ALL {
            assert_eq!(caps.get(c), Err(Off::NotGranted), "{}", c.id());
        }
        assert_eq!(caps.live(), CapSet::default(), "nothing is implied");
        // A grant to one person gives it to them alone.
        let sam = "u_sam";
        grant(
            &st,
            Capability::OpenUrl,
            &Grantee::Person(sam.into()).as_text(),
        );
        assert!(now(&st, sam).live().has(Capability::OpenUrl));
        assert_eq!(
            now(&st, OWNER).get(Capability::OpenUrl),
            Err(Off::NotGranted)
        );
    }

    #[test]
    fn a_revoked_parent_leaves_the_run_nothing() {
        let (_d, st) = store();
        offer(&st, &[Capability::WebSearch], true);
        let root = grant(&st, Capability::WebSearch, EVERYONE);
        assert!(now(&st, OWNER).live().has(Capability::WebSearch));
        st.revoke_cap_grant(&AuditCtx::DAEMON, &root, RevokeReason::Granter)
            .unwrap();
        assert_eq!(
            now(&st, OWNER).get(Capability::WebSearch),
            Err(Off::NotGranted)
        );
        let live: Vec<String> = st
            .cap_grants(&GrantFilter {
                live: true,
                ..GrantFilter::default()
            })
            .unwrap()
            .into_iter()
            .map(|g| g.grantee)
            .collect();
        assert_eq!(
            live,
            [Grantee::RunsOf(OWNER.into()).as_text()],
            "the person's child ended with it; their runs grant conveys nothing"
        );
        assert_eq!(now(&st, OWNER).live(), CapSet::default());
    }

    #[test]
    fn caps_round_trip_through_their_text() {
        let all: CapSet = Capability::ALL.into_iter().collect();
        assert_eq!(
            all.as_str(),
            "clax-browser:open-artifact,clax-browser:open-content,clax-browser:open-url,clax-web:fetch,clax-web:search"
        );
        assert_eq!(CapSet::parse(&all.as_str()), all);
        assert_eq!(
            CapSet::parse("clax-web:search,web.crawl,,clax-browser:open-url,clax-web:search")
                .as_str(),
            "clax-browser:open-url,clax-web:search",
            "unknown parts ignored, each once"
        );
        assert_eq!(CapSet::parse("").as_str(), "");
        for c in Capability::ALL {
            assert_eq!(Capability::parse(c.id()), Some(c));
        }
        assert!(CapSet::parse("clax-browser:open-content").any_of(Family::Browser));
        assert!(!CapSet::parse("clax-browser:open-content").any_of(Family::Web));
    }

    #[test]
    fn open_artifact_needs_no_web_capability() {
        let (_d, st) = store();
        offer(
            &st,
            &[Capability::OpenArtifact, Capability::OpenContent],
            true,
        );
        grant(&st, Capability::OpenArtifact, EVERYONE);
        grant(&st, Capability::OpenContent, EVERYONE);
        let caps = now(&st, OWNER);
        assert_eq!(
            caps.live(),
            CapSet::parse("clax-browser:open-artifact,clax-browser:open-content")
        );
        assert_eq!(
            caps.get(Capability::OpenArtifact),
            Ok(&Terms::Open(OpenGrant {
                kind: Capability::OpenArtifact,
                max_per_run: 10,
                url_rule: None,
            }))
        );
        assert_eq!(
            caps.browser(),
            Some(BrowserTerms {
                call_seconds: 60,
                idle_seconds: 300,
                max_seconds: 1_800,
                max_open_per_run: 1,
            })
        );
        assert_eq!(caps.get(Capability::WebFetch), Err(Off::NotOffered));
        offer(&st, &[Capability::OpenUrl], true);
        grant(&st, Capability::OpenUrl, EVERYONE);
        assert_eq!(
            now(&st, OWNER).get(Capability::OpenUrl),
            Ok(&Terms::Open(OpenGrant {
                kind: Capability::OpenUrl,
                max_per_run: 5,
                url_rule: Some(UrlRule::Sender),
            })),
            "a URL session's rule defaults to sender"
        );
    }
}

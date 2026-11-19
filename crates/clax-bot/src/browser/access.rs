//! What a run may do in a browser (spec §4.5): [`BrowserAccess`], a view
//! of [`Capabilities::now`] over `browser.open.artifact` (testing) and
//! `browser.open.url` (browsing), each on its own: neither needs the
//! other, nor a web capability.
//!
//! [`Capabilities::now`]: crate::capability::Capabilities::now
//!
//! A run's [`BrowserShape`] (which of `clax.browser.test` and
//! `clax.browser.browse` exist) is fixed when the run is made and kept on
//! it ([`Run::browser`], written from [`BrowserAccess::shape`]): the
//! system prompt and its API declaration are generated from it, so they
//! stay the same for every turn. What can change mid-run (the admin turns
//! an operation off, the person opts out, the credentials are removed) is
//! read again with [`BrowserAccess::now`] at each call and answered with
//! an error; a run made without an operation never gains it.
//!
//! [`Run::browser`]: clax_store::bot_runs::Run::browser

use crate::capability::UrlRule;
use clax_core::Result;
use clax_store::Store;

/// Which browser operations: those a run declares, kept on the run as
/// `""`, `"test"`, `"browse"` or `"test,browse"` ([`BrowserShape::as_str`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BrowserShape {
    pub test: bool,
    pub browse: bool,
}

impl BrowserShape {
    /// The shape `s` names: its comma-separated parts `test` and `browse`;
    /// anything else names neither, so an unknown value declares nothing.
    #[must_use]
    pub fn parse(s: &str) -> BrowserShape {
        let mut shape = BrowserShape::default();
        for part in s.split(',') {
            match part.trim() {
                "test" => shape.test = true,
                "browse" => shape.browse = true,
                _ => {}
            }
        }
        shape
    }

    /// `""`, `"test"`, `"browse"` or `"test,browse"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match (self.test, self.browse) {
            (false, false) => "",
            (true, false) => "test",
            (false, true) => "browse",
            (true, true) => "test,browse",
        }
    }

    /// Whether it declares either operation.
    #[must_use]
    pub fn any(self) -> bool {
        self.test || self.browse
    }
}

/// Why browser use is off for a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserOff {
    /// The admin turned both testing and browsing off ("off on this
    /// instance").
    AdminOff,
    /// No live admin grant gives the person either ("not granted to
    /// you").
    NotGranted,
    /// The person revoked their grants to their runs ("off by your
    /// choice").
    PersonOff,
    /// The host's source cannot open browsers: no Cloudflare account ID
    /// and token on a daemon, or a host with no source ("not
    /// configured").
    NotConfigured,
}

impl BrowserOff {
    /// `admin_off`, `not_granted`, `person_off` or `not_configured`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BrowserOff::AdminOff => "admin_off",
            BrowserOff::NotGranted => "not_granted",
            BrowserOff::PersonOff => "person_off",
            BrowserOff::NotConfigured => "not_configured",
        }
    }

    /// The reason [`Off`](crate::capability::Off) gives, in this enum's
    /// words.
    #[must_use]
    pub fn of(off: crate::capability::Off) -> BrowserOff {
        use crate::capability::Off;
        match off {
            Off::NotOffered => BrowserOff::AdminOff,
            Off::NotConfigured => BrowserOff::NotConfigured,
            Off::NotGranted => BrowserOff::NotGranted,
            Off::PersonOff => BrowserOff::PersonOff,
        }
    }
}

/// What a run may test, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestGrant {
    /// `browser.open.artifact.max_per_run`.
    pub max_per_run: u32,
    /// One call's time cap, 10 to 120 seconds (`browser.call_seconds`): a
    /// call's `timeout_s` is capped at it.
    pub seconds: u32,
}

/// What a run may browse, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseGrant {
    /// `browser.open.url.url_rule`.
    pub rule: UrlRule,
    /// `browser.open.url.max_per_run`.
    pub max_per_run: u32,
    /// One call's time cap (`browser.call_seconds`), the one cap of every
    /// browser call.
    pub seconds: u32,
}

/// The operations browser use is on for: `None` for one that is off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserGrant {
    pub test: Option<TestGrant>,
    pub browse: Option<BrowseGrant>,
}

/// What a run may do in a browser now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserAccess {
    Off(BrowserOff),
    /// At least one of the grant's operations is `Some`.
    On(BrowserGrant),
}

impl BrowserAccess {
    /// Now, for `person`: a view of [`Capabilities::now`] over
    /// `browser.open.artifact` (testing) and `browser.open.url`
    /// (browsing), the host able to open browsers when `configured`. Off
    /// when neither is live, for the reason closest to on
    /// ([`Capabilities::why_off`]); else the live ones, each capped at
    /// `browser.call_seconds`.
    ///
    /// [`Capabilities::now`]: crate::capability::Capabilities::now
    /// [`Capabilities::why_off`]: crate::capability::Capabilities::why_off
    ///
    /// # Errors
    /// The store's.
    pub fn now(store: &Store, person: &str, configured: bool) -> Result<BrowserAccess> {
        use crate::capability::{Capabilities, Capability, Family, Terms};
        let caps = Capabilities::now(store, person, &|c| {
            c.family() == Family::Browser && configured
        })?;
        if let Some(off) = caps.why_off(&[Capability::OpenArtifact, Capability::OpenUrl]) {
            return Ok(BrowserAccess::Off(BrowserOff::of(off)));
        }
        let seconds = caps.browser().map_or(0, |b| b.call_seconds);
        let test = match caps.get(Capability::OpenArtifact) {
            Ok(Terms::Open(g)) => Some(TestGrant {
                max_per_run: g.max_per_run,
                seconds,
            }),
            _ => None,
        };
        let browse = match caps.get(Capability::OpenUrl) {
            Ok(Terms::Open(g)) => Some(BrowseGrant {
                rule: g.url_rule.clone().unwrap_or(UrlRule::Sender),
                max_per_run: g.max_per_run,
                seconds,
            }),
            _ => None,
        };
        Ok(BrowserAccess::On(BrowserGrant { test, browse }))
    }

    /// Which operations it grants.
    #[must_use]
    pub fn shape(&self) -> BrowserShape {
        match self {
            BrowserAccess::Off(_) => BrowserShape::default(),
            BrowserAccess::On(g) => BrowserShape {
                test: g.test.is_some(),
                browse: g.browse.is_some(),
            },
        }
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::audit::AuditCtx;
    use clax_store::bot_settings::Setting;
    use clax_store::roles::OWNER;
    use serde_json::Value;
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
    /// reaches a run without one (migration 67).
    fn grant_everyone(st: &Store) {
        use clax_store::bot_grants::{CAPABILITIES, EVERYONE, GRANTER_INSTANCE, GrantFilter};
        let live = st
            .cap_grants(&GrantFilter {
                grantee: Some(EVERYONE.into()),
                live: true,
                ..GrantFilter::default()
            })
            .unwrap();
        for cap in CAPABILITIES {
            if !live.iter().any(|g| g.capability == cap) {
                st.issue_cap_grant(&AuditCtx::DAEMON, cap, GRANTER_INSTANCE, EVERYONE, None)
                    .unwrap();
            }
        }
    }

    fn shape(test: bool, browse: bool) -> BrowserShape {
        BrowserShape { test, browse }
    }

    #[test]
    fn shapes_round_trip_through_their_text() {
        for s in ["", "test", "browse", "test,browse"] {
            assert_eq!(BrowserShape::parse(s).as_str(), s);
        }
        assert_eq!(BrowserShape::parse("browse,test").as_str(), "test,browse");
        assert_eq!(
            BrowserShape::parse(" test , browse ").as_str(),
            "test,browse"
        );
        assert_eq!(BrowserShape::parse("search,Test"), BrowserShape::default());
        assert!(!BrowserShape::default().any());
        assert!(shape(false, true).any() && shape(true, false).any());
        assert_eq!(BrowserOff::AdminOff.as_str(), "admin_off");
        assert_eq!(BrowserOff::PersonOff.as_str(), "person_off");
        assert_eq!(BrowserOff::NotConfigured.as_str(), "not_configured");
    }

    #[test]
    fn access_is_off_when_the_admin_or_the_person_says() {
        let (_d, st) = store();
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap(),
            BrowserAccess::Off(BrowserOff::AdminOff),
            "off by default"
        );
        set(&st, &[(Setting::BrowserOpenArtifactEnabled, json!(true))]);
        let access = BrowserAccess::now(&st, OWNER, true).unwrap();
        assert_eq!(
            access,
            BrowserAccess::On(BrowserGrant {
                test: Some(TestGrant {
                    max_per_run: 10,
                    seconds: 60
                }),
                browse: None,
            })
        );
        assert_eq!(access.shape(), shape(true, false));
        set(
            &st,
            &[
                (Setting::BrowserOpenArtifactMaxPerRun, json!(3)),
                (Setting::BrowserCallSeconds, json!(30)),
            ],
        );
        let BrowserAccess::On(g) = BrowserAccess::now(&st, OWNER, true).unwrap() else {
            panic!("testing is on");
        };
        assert_eq!(
            g.test,
            Some(TestGrant {
                max_per_run: 3,
                seconds: 30
            })
        );
        // The person opts out.
        st.set_browser_choice(&AuditCtx::DAEMON, OWNER, false)
            .unwrap();
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap(),
            BrowserAccess::Off(BrowserOff::PersonOff)
        );
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap().shape(),
            BrowserShape::default()
        );
        assert!(
            !BrowserAccess::now(&st, "someone-else", true)
                .unwrap()
                .shape()
                .test,
            "no grant reaches a key that names no person"
        );
        // The admin's reason comes before the person's.
        set(&st, &[(Setting::BrowserOpenArtifactEnabled, json!(false))]);
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap(),
            BrowserAccess::Off(BrowserOff::AdminOff)
        );
    }

    #[test]
    fn browse_is_its_own_capability_and_needs_no_web_choice() {
        let (_d, st) = store();
        set(
            &st,
            &[
                (Setting::BrowserOpenUrlEnabled, json!(true)),
                (Setting::BrowserOpenUrlMaxPerRun, json!(4)),
            ],
        );
        let BrowserAccess::On(g) = BrowserAccess::now(&st, OWNER, true).unwrap() else {
            panic!("browsing is on");
        };
        assert_eq!(
            g,
            BrowserGrant {
                test: None,
                browse: Some(BrowseGrant {
                    rule: UrlRule::Sender,
                    max_per_run: 4,
                    seconds: 60
                }),
            }
        );
        set(
            &st,
            &[
                (Setting::BrowserOpenUrlUrlRule, json!("allowlist")),
                (Setting::BrowserOpenUrlAllowlist, json!(["*.example.com"])),
            ],
        );
        let BrowserAccess::On(g) = BrowserAccess::now(&st, OWNER, true).unwrap() else {
            panic!("browsing is on");
        };
        assert_eq!(
            g.browse.unwrap().rule,
            UrlRule::Allowlist(vec!["*.example.com".into()])
        );
        set(
            &st,
            &[(Setting::BrowserOpenUrlUrlRule, json!("any_public"))],
        );
        // The person's web choice off leaves browsing on: none needs
        // another.
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, false).unwrap();
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap().shape(),
            shape(false, true)
        );
        set(&st, &[(Setting::BrowserOpenArtifactEnabled, json!(true))]);
        let access = BrowserAccess::now(&st, OWNER, true).unwrap();
        assert_eq!(access.shape(), shape(true, true));
        let BrowserAccess::On(g) = access else {
            panic!("both are on");
        };
        assert_eq!(g.browse.unwrap().rule, UrlRule::AnyPublic);
        // The browser choice off turns both off.
        st.set_browser_choice(&AuditCtx::DAEMON, OWNER, false)
            .unwrap();
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap(),
            BrowserAccess::Off(BrowserOff::PersonOff)
        );
    }

    #[test]
    fn not_configured_without_a_source() {
        let (_d, st) = store();
        // Nothing on: the admin's reason, whatever the source.
        assert_eq!(
            BrowserAccess::now(&st, OWNER, false).unwrap(),
            BrowserAccess::Off(BrowserOff::AdminOff)
        );
        for setting in [
            Setting::BrowserOpenArtifactEnabled,
            Setting::BrowserOpenUrlEnabled,
        ] {
            set(&st, &[(setting, json!(true))]);
            assert_eq!(
                BrowserAccess::now(&st, OWNER, false).unwrap(),
                BrowserAccess::Off(BrowserOff::NotConfigured),
                "{}",
                setting.key()
            );
        }
        // The instance's reason comes before the person's.
        st.set_browser_choice(&AuditCtx::DAEMON, OWNER, false)
            .unwrap();
        assert_eq!(
            BrowserAccess::now(&st, OWNER, false).unwrap(),
            BrowserAccess::Off(BrowserOff::NotConfigured)
        );
        assert_eq!(
            BrowserAccess::now(&st, OWNER, false).unwrap().shape(),
            BrowserShape::default()
        );
    }
}

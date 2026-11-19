//! What a run may do in a browser (spec §4.5): [`BrowserAccess`],
//! computed from the instance's `browser.*` settings, the person's own
//! choices ([`Store::browser_choice`], and [`Store::web_choice`] for
//! browsing) and whether the host's source can open browsers.
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

use super::browse::BrowseRule;
use clax_core::Result;
use clax_store::Store;
use clax_store::bot_settings::Setting;
use serde_json::Value;

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
    /// The person turned browser use off for themselves, or, with only
    /// browsing on, turned their web use off ("off by your choice").
    PersonOff,
    /// The host's source cannot open browsers: no Cloudflare account ID
    /// and token on a daemon, or a host with no source ("not
    /// configured").
    NotConfigured,
}

impl BrowserOff {
    /// `admin_off`, `person_off` or `not_configured`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BrowserOff::AdminOff => "admin_off",
            BrowserOff::PersonOff => "person_off",
            BrowserOff::NotConfigured => "not_configured",
        }
    }
}

/// What a run may test, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestGrant {
    pub max_per_run: u32,
    /// The per-test time cap, 10 to 120 seconds: a call's `timeout_s` is
    /// capped at it.
    pub seconds: u32,
}

/// What a run may browse, as the admin set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseGrant {
    pub rule: BrowseRule,
    pub max_per_run: u32,
    /// The time cap of one browse: the per-test time cap
    /// (`browser.test_seconds`), which bounds every browser session.
    pub seconds: u32,
}

/// The operations browser use is on for: `None` for one the admin turned
/// off (or, for browsing, the person's web choice).
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
    /// Now, for `person`: off when the admin turned both testing and
    /// browsing off ([`BrowserOff::AdminOff`]), when the host's source
    /// cannot open browsers (`configured`, [`BrowserOff::NotConfigured`]),
    /// or when the person chose off ([`BrowserOff::PersonOff`]), in that
    /// order: the instance's reasons come first, since the person's switch
    /// would not turn browser use on. Browsing needs the person's web
    /// choice too (spec §10.2): with it off, the grant has no browse, and
    /// with only browsing on, browser use is off by their choice. Else the
    /// operations the admin turned on, with their settings.
    ///
    /// # Errors
    /// The store's.
    pub fn now(store: &Store, person: &str, configured: bool) -> Result<BrowserAccess> {
        let on = |s: Setting| -> Result<bool> { Ok(store.bot_setting(s)?.as_bool() == Some(true)) };
        let test_on = on(Setting::BrowserEnabled)?;
        let browse_on = on(Setting::BrowserBrowseEnabled)?;
        if !test_on && !browse_on {
            return Ok(BrowserAccess::Off(BrowserOff::AdminOff));
        }
        if !configured {
            return Ok(BrowserAccess::Off(BrowserOff::NotConfigured));
        }
        if !store.browser_choice(person)? {
            return Ok(BrowserAccess::Off(BrowserOff::PersonOff));
        }
        let browse_on = browse_on && store.web_choice(person)?;
        if !test_on && !browse_on {
            return Ok(BrowserAccess::Off(BrowserOff::PersonOff));
        }
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
        let seconds = count(Setting::BrowserTestSeconds)?;
        let test = test_on
            .then(|| -> Result<TestGrant> {
                Ok(TestGrant {
                    max_per_run: count(Setting::BrowserMaxTestsPerRun)?,
                    seconds,
                })
            })
            .transpose()?;
        let browse = if browse_on {
            // The setting is checked on write (`Setting::check`), so the
            // fallback is never reached in practice.
            let rule = match store.bot_setting(Setting::BrowserBrowseUrlRule)?.as_str() {
                Some("any_public") => BrowseRule::AnyPublic,
                Some("allowlist") => BrowseRule::Allowlist(
                    store
                        .bot_setting(Setting::BrowserBrowseAllowlist)?
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                _ => BrowseRule::Sender,
            };
            Some(BrowseGrant {
                rule,
                max_per_run: count(Setting::BrowserMaxBrowsesPerRun)?,
                seconds,
            })
        } else {
            None
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
        set(&st, &[(Setting::BrowserEnabled, json!(true))]);
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
                (Setting::BrowserMaxTestsPerRun, json!(3)),
                (Setting::BrowserTestSeconds, json!(30)),
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
        // The person opts out; their web choice does not turn it back on.
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
            BrowserAccess::now(&st, "someone-else", true)
                .unwrap()
                .shape()
                .test
        );
        // The admin's reason comes before the person's.
        set(&st, &[(Setting::BrowserEnabled, json!(false))]);
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap(),
            BrowserAccess::Off(BrowserOff::AdminOff)
        );
    }

    #[test]
    fn browse_needs_the_web_choice_too() {
        let (_d, st) = store();
        set(
            &st,
            &[
                (Setting::BrowserBrowseEnabled, json!(true)),
                (Setting::BrowserMaxBrowsesPerRun, json!(4)),
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
                    rule: BrowseRule::Sender,
                    max_per_run: 4,
                    seconds: 60
                }),
            }
        );
        set(
            &st,
            &[
                (Setting::BrowserBrowseUrlRule, json!("allowlist")),
                (Setting::BrowserBrowseAllowlist, json!(["*.example.com"])),
            ],
        );
        let BrowserAccess::On(g) = BrowserAccess::now(&st, OWNER, true).unwrap() else {
            panic!("browsing is on");
        };
        assert_eq!(
            g.browse.unwrap().rule,
            BrowseRule::Allowlist(vec!["*.example.com".into()])
        );
        set(&st, &[(Setting::BrowserBrowseUrlRule, json!("any_public"))]);
        // The person's web choice off: browsing alone is off by their
        // choice, though their browser choice is on.
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, false).unwrap();
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap(),
            BrowserAccess::Off(BrowserOff::PersonOff)
        );
        // With testing on too, the run tests and does not browse.
        set(&st, &[(Setting::BrowserEnabled, json!(true))]);
        assert_eq!(
            BrowserAccess::now(&st, OWNER, true).unwrap().shape(),
            shape(true, false)
        );
        st.set_web_choice(&AuditCtx::DAEMON, OWNER, true).unwrap();
        let access = BrowserAccess::now(&st, OWNER, true).unwrap();
        assert_eq!(access.shape(), shape(true, true));
        let BrowserAccess::On(g) = access else {
            panic!("both are on");
        };
        assert_eq!(g.browse.unwrap().rule, BrowseRule::AnyPublic);
        // The browser choice off turns browsing off too.
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
        for setting in [Setting::BrowserEnabled, Setting::BrowserBrowseEnabled] {
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

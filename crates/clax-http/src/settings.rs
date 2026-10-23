//! What the settings routes (`clax_server::routes::settings`) hold beside
//! the request: the configuration the daemon read when it started, which
//! is the one in effect, and the hook that records who viewed or changed
//! the settings ([`SettingsAudit`]).
//!
//! The hook records the audit kinds of spec 2026-10-21-claxbot-design
//! §9.1: `view.admin {page: "settings"}` for a view and `admin.settings
//! {fields}` for a change ([`crate::bot::SettingsRecorded`], what
//! [`Settings::at_start`] installs); [`NoAudit`] records nothing.

use crate::auth::Caller;
use clax_core::config::HomeConfig;
use clax_core::config::settings::Change;
use clax_store::Store;
use std::sync::Arc;

/// Records views and changes of the settings. Called by `GET
/// /api/settings` after it read them, and by `PUT /api/settings` after a
/// write succeeded; never for a refused request. Both run on a store
/// thread (inside the route's `store_call`), so an implementation may
/// write to `st` without blocking the runtime. `who` is the caller the
/// route admitted ([`Caller`], as `may_edit_settings` saw it). The values
/// carry no secret (a `*_env` field names a variable, never its value).
pub trait SettingsAudit: Send + Sync {
    /// `who` read the settings: `fields` are the keys shown.
    fn viewed(&self, st: &Store, who: &Caller, fields: &[String]);
    /// `who` changed `changes` (each key's value and source, before and
    /// after).
    fn changed(&self, st: &Store, who: &Caller, changes: &[Change]);
}

/// Records nothing: the default.
pub struct NoAudit;

impl SettingsAudit for NoAudit {
    fn viewed(&self, _st: &Store, _who: &Caller, _fields: &[String]) {}
    fn changed(&self, _st: &Store, _who: &Caller, _changes: &[Change]) {}
}

/// The settings routes' state.
pub struct Settings {
    /// The configuration in effect: what the daemon read when it started
    /// (the defaults and the environment when its file did not read).
    /// `None` on a hosted daemon, or when a test leaves it out (the routes
    /// then show what reads now).
    pub running: Option<HomeConfig>,
    /// Why the file did not read when the daemon started, when it did not:
    /// the daemon then runs on the defaults.
    pub start_error: Option<String>,
    pub audit: Arc<dyn SettingsAudit>,
}

impl Settings {
    /// The settings of a daemon starting for `home` (hosted: none),
    /// recorded by [`crate::bot::SettingsRecorded`].
    pub fn at_start(home: &clax_core::Home, hosted: bool) -> Settings {
        let (running, start_error) = if hosted {
            (None, None)
        } else {
            match HomeConfig::load(home.root()) {
                Ok(c) => (Some(c), None),
                Err(e) => (
                    HomeConfig::from_text(
                        home.root().join(clax_core::config::FILE),
                        "",
                        clax_core::config::process_env(),
                    )
                    .ok(),
                    Some(e.to_string()),
                ),
            }
        };
        Settings {
            running,
            start_error,
            audit: Arc::new(crate::bot::SettingsRecorded {
                views: Arc::default(),
            }),
        }
    }
}

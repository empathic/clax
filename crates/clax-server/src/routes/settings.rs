//! The local daemon's settings (spec 2026-10-21-claxbot-design §8, §10):
//! every field of the home's configuration with its value and where it
//! came from, and changing the fields `config.toml` holds
//! ([`clax_core::config::settings`]).
//!
//! - `GET /api/settings` → `{file, fields: [{key, value, source, pending?,
//!   editable, restart_required, env}], problems: [{table, message}]}`.
//!   `value` and `source` are what the running daemon read when it started
//!   (in effect; `serve.port` is the port it listens on, with a `note`
//!   when `--port`, `CLAX_PORT` or the next free port chose it);
//!   `pending` is present when the file or environment now gives another
//!   value, which applies at the next start. When the file did not read
//!   at the start, `started_with_invalid_config` is true and
//!   `start_error` says why: the values in effect are the defaults. `source` is
//!   `default`, `file` or `env:<NAME>`; a field the environment sets is
//!   not editable here, nor is `bin` (`clax bin set` writes it). `env` is
//!   the variable that would set the field. `problems` names each table
//!   whose values do not check, which the daemon runs without.
//! - `PUT /api/settings {set: {<key>: <value> | null}}` → the same, plus
//!   `{written: [key], restart_required}` (`written` in key order, as `set`
//!   is read into a sorted map): writes the values into
//!   `config.toml` (null removes a key, so its default applies), keeping
//!   its comments and order. Nothing is written when one change is refused:
//!   400 `unknown_setting`, `not_editable`, `set_by_env` or
//!   `invalid_value` (a value its table's check refuses, with the check's
//!   message), and 400 `bad_config` when the file does not parse.
//!   `restart_required` is true when a written field is read only when the
//!   daemon starts (every field today): the daemon goes on with the values
//!   it started with.
//!
//! Each view and each successful write is handed, on the store thread
//! with the admitted caller, to the state's
//! [`SettingsAudit`](clax_http::settings::SettingsAudit) hook (a no-op until
//! the audit kinds of spec 2026-10-21-claxbot-design §9.1 are wired to it).
//!
//! Who may: [`may_edit_settings`]; an admin who is not the owner changes
//! only the fields [`owner_only_setting`] leaves them (the rest of `[bot]`
//! and `[questions]`; 403 `owner_only` for any other, which their view
//! shows not editable). A write that changes `bot.base_url` clears the
//! stored instance key (`admin.key {op: clear}`), which is sent only to
//! the URL it was set under.
//! Both routes are refused to another
//! machine's agents (`DeniedToGrants`) and answer 404 on a hosted daemon,
//! which has no `config.toml` of its own to show (the Worker has none at
//! all).

use super::artifacts::body;
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use clax_core::config::{HomeConfig, settings};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use serde::Deserialize;
use serde_json::{Value, json};

/// Whether `caller` may read and change the settings: the owner (a
/// browser of theirs, or the CLI by hand with the token) and the people
/// the owner made admins ([`clax_http::bot::may_admin`]). Agents get 403
/// `not_for_agents`; anyone else 403 `forbidden`, and no credential 401.
///
/// # Errors
/// As above; the store's.
pub fn may_edit_settings(st: &clax_store::Store, caller: &Caller) -> Result<(), ApiError> {
    clax_http::bot::may_admin(st, caller)
        .map(|_| ())
        .map_err(|e| match e.code {
            "forbidden" => ApiError::forbidden(
                "forbidden",
                "only the owner and admins read and change this Clax's settings",
            ),
            _ => e,
        })
}

/// Whether setting `key` is the owner's alone to change: every variable
/// name (`*_env`, which makes the daemon read that variable and send it
/// on, as `[bot] api_key_env` does to `base_url`), `[serve]`, `[sample]`,
/// `[hosted]`, `[toolpath]`, `bin`, and `bot.base_url`: the environment's
/// instance key goes to a base URL from the configuration alone, so only
/// the owner writes one there (admins set theirs on the provider page,
/// in the database, which the environment's key is never sent to; spec
/// §8.1). Admins change the rest of `[bot]` and `[questions]`.
pub fn owner_only_setting(key: &str) -> bool {
    key == "bin"
        || key == "bot.base_url"
        || key.ends_with("_env")
        || ["serve.", "sample.", "hosted.", "toolpath."]
            .iter()
            .any(|t| key.starts_with(t))
        || !(key.starts_with("bot.") || key.starts_with("questions."))
}

/// `v` (the settings' view) as `caller` may change it: an admin who is
/// not the owner sees the owner's fields not editable.
fn for_caller(mut v: Value, caller: &Caller) -> Value {
    if !caller.is_owner()
        && let Some(fs) = v["fields"].as_array_mut()
    {
        for f in fs {
            if f["key"].as_str().is_some_and(owner_only_setting) {
                f["editable"] = json!(false);
            }
        }
    }
    v
}

/// [`may_edit_settings`] on a store thread.
async fn admitted(s: &AppState, caller: &Caller) -> Result<(), ApiError> {
    let c = caller.clone();
    s.store_call(move |st| Ok(may_edit_settings(st, &c)))
        .await?
}

/// 404 on a hosted daemon, which has no `config.toml` to show.
fn local_only(s: &AppState) -> Result<(), ApiError> {
    if s.hosted.is_some() {
        Err(ApiError::not_found())
    } else {
        Ok(())
    }
}

/// The settings as the routes show them: each value as the daemon read it
/// when it started (`st.running`, else as it reads now), with `pending`
/// the value it reads now when that differs, and `serve.port` the port it
/// listens on (`listening`), with a note when that is not the configured
/// one; problems as it reads now, and `started_with_invalid_config` with
/// `start_error` when the file did not read at the start, so the daemon
/// runs on the defaults.
fn view(
    root: &std::path::Path,
    st: &clax_http::settings::Settings,
    listening: u16,
) -> clax_core::Result<Value> {
    let cfg = HomeConfig::load(root)?;
    let running = st.running.as_ref().unwrap_or(&cfg);
    let mut fields = settings::fields_in_effect(running, &cfg);
    if let Some(f) = fields.iter_mut().find(|f| f.key == "serve.port") {
        let configured = f.value.clone();
        if configured != json!(listening) {
            f.note = Some(format!(
                "the daemon listens on {listening}, not the configured {configured}: --port, CLAX_PORT or the next free port chose it"
            ));
            if f.pending.is_none() {
                f.pending = Some(configured);
            }
            f.value = json!(listening);
        }
    }
    Ok(json!({
        "file": cfg.path().display().to_string(),
        "fields": fields,
        "problems": settings::problems(&cfg),
        "started_with_invalid_config": st.start_error.is_some(),
        "start_error": st.start_error,
    }))
}

/// The keys `v`'s fields show.
fn shown_keys(v: &Value) -> Vec<String> {
    v["fields"]
        .as_array()
        .map(|fs| {
            fs.iter()
                .filter_map(|f| f["key"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/settings` (see the module documentation).
pub async fn get(State(s): State<AppState>, d: DeniedToGrants) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    let caller = d.caller(&s).await?;
    admitted(&s, &caller).await?;
    let root = s.home.root().to_path_buf();
    let (sets, port) = (s.settings.clone(), s.port);
    // File I/O, and the audit hook, on a store thread, never a runtime worker.
    let v = s
        .store_call(move |store| {
            let v = for_caller(view(&root, &sets, port)?, &caller);
            sets.audit.viewed(store, &caller, &shown_keys(&v));
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBody {
    /// Each key to set, with its value; null removes it.
    set: serde_json::Map<String, Value>,
}

/// `PUT /api/settings {set}` (see the module documentation).
pub async fn put(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    req: Result<Json<PutBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    let caller = d.caller(&s).await?;
    admitted(&s, &caller).await?;
    let b = body(req)?;
    if !caller.is_owner()
        && let Some(k) = b.set.keys().find(|k| owner_only_setting(k))
    {
        return Err(ApiError::forbidden(
            "owner_only",
            format!("only the owner changes {k}"),
        ));
    }
    let root = s.home.root().to_path_buf();
    let changes: Vec<(String, Value)> = b.set.into_iter().collect();
    let (sets, port) = (s.settings.clone(), s.port);
    let v = s
        .store_call(move |store| {
            let env = clax_core::config::process_env();
            let w = settings::write(&root, &env, &changes)?;
            sets.audit.changed(store, &caller, &w.changes);
            // The stored instance key goes only to the URL it was set under.
            if w.changes
                .iter()
                .any(|c| c.key == "bot.base_url" && c.old != c.new)
            {
                let ctx = clax_http::bot::caller_ctx(store, &caller)?;
                store.clear_secret(&ctx, &clax_store::secrets::SecretSlot::Instance)?;
            }
            let mut v = view(&root, &sets, port)?;
            v["written"] = json!(w.keys);
            v["restart_required"] = json!(w.restart_required);
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

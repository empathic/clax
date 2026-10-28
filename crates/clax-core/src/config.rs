//! A home's configuration: defaults, then `<home>/config.toml`, then the
//! environment, layered with [figment](https://docs.rs/figment).
//!
//! Each value comes from the last layer that sets it:
//!
//! 1. the defaults below ([`Source::Default`]);
//! 2. `<home>/config.toml` ([`Source::File`]);
//! 3. an environment variable `CLAX_<TABLE>__<KEY>`, double underscores
//!    nesting (`CLAX_SERVE__PORT`, `CLAX_BOT__MODELS__DEFAULT`;
//!    [`Source::Env`]). Only names in capitals with at least one `__` are
//!    read, so the variables Clax already has (`CLAX_HOME`, `CLAX_PORT`,
//!    `CLAX_BIN`, `CLAX_TOKEN`, …) are never settings, and no variable sets
//!    a top-level key such as `bin`, nor `[hosted]` ([`FILE_ONLY_TABLES`]:
//!    such a variable is ignored with a warning). A value is taken as the
//!    setting's type: verbatim for a string setting, else as a TOML value
//!    (`7481`, `true`); an empty value counts as unset. A variable that is
//!    not UTF-8 is never read ([`utf8_env`]).
//!
//! Each table is read on its own, so one that is invalid turns off only what
//! it configures. `[serve] port` is the port a daemon started for the home
//! listens on (7480 when absent; `--port` and `CLAX_PORT` come ahead of this
//! whole stack, see `clax_commands::Cli::port_for`); `just dev` and `just
//! watch` write `port = 7481` into `~/.clax-dev/config.toml`. `[serve]
//! trusted_hosts` names the DNS names, beside this machine's own, that the
//! API answers to (a reverse proxy's, such as `tailscale serve`'s;
//! [`HomeConfig::serve_trusted_hosts`]); a daemon whose list is invalid does
//! not start. `[serve] tailscale_identity` names a viewer that came through
//! `tailscale serve` by its tailnet login
//! ([`HomeConfig::serve_tailscale_identity`]), and `[serve]
//! owner_tailscale_login` names the one tailnet login whose browser is the
//! owner's ([`HomeConfig::serve_owner_tailscale_login`]), and `[serve]
//! trusted_host_frames` frames artifacts from a path of a trusted host
//! ([`HomeConfig::serve_trusted_host_frames`]). Any other key in `[serve]`
//! is logged and ignored. `[sample]`
//! configures the
//! `sample` capability ([`HomeConfig::sample`]); a daemon whose `[sample]` is
//! invalid starts with sample off. `[toolpath]` configures the audit
//! journal ([`HomeConfig::toolpath`]); a daemon whose `[toolpath]` is
//! invalid keeps recording audit events but writes no journal, and says
//! why in `clax toolpath status`. `[bot]` configures ClaxBot
//! ([`HomeConfig::bot`]). The top-level `bin` key names the clax the
//! plugins run ([`HomeConfig::bin`]); `clax bin set` writes it as one line,
//! [`bin_line`], which the plugins' wrapper reads without a TOML parser,
//! and nothing else here writes it. `[questions] terminal_after_s` is how
//! long a mirrored AskUserQuestion waits in Clax before the terminal dialog
//! ([`HomeConfig::questions_terminal_after_s`]). `[hosted]` turns on hosted
//! mode, in which the daemon serves the public internet at one origin
//! ([`HomeConfig::hosted`]). Other tables are reserved (spec §5) and
//! ignored.
//!
//! [`settings`] lists every field with its value and source, and writes
//! file-backed fields back into `config.toml` with `toml_edit`, keeping its
//! comments and order.

pub mod settings;

use crate::{CoreError, Result};
use figment::value::{Dict, Map, Value};
use figment::{Figment, Metadata, Profile, Provider};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name inside a home.
pub const FILE: &str = "config.toml";

/// The prefix of the environment variables that set configuration.
pub const ENV_PREFIX: &str = "CLAX_";

/// The port a daemon listens on when nothing sets `[serve] port`
/// (`crate::daemon::DEFAULT_PORT` re-exports it for native code; this one
/// builds for wasm32 too).
pub const DEFAULT_PORT: u16 = 7480;

/// The tables only `config.toml` sets: a variable naming one is ignored,
/// with a warning. Hosted mode decides who can reach the daemon, too much
/// to switch on, or break, from an inherited environment.
pub const FILE_ONLY_TABLES: &[&str] = &["hosted"];

/// `vars` with every pair that is not UTF-8 dropped: what the
/// configuration reads from an environment, which may hold any bytes
/// (`std::env::vars` would panic on them).
pub fn utf8_env(
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(String, String)> {
    vars.into_iter()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// The process's environment, as [`utf8_env`] keeps it; empty on wasm32,
/// where a Worker has no process environment (`std::env` panics there).
pub fn process_env() -> Vec<(String, String)> {
    #[cfg(target_arch = "wasm32")]
    {
        Vec::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        utf8_env(std::env::vars_os())
    }
}

/// Where a configuration value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The built-in default.
    Default,
    /// The home's `config.toml`.
    File,
    /// The environment variable it names.
    Env(String),
}

impl Source {
    /// `default`, `file` or `env:<NAME>`: how the settings API shows it.
    pub fn label(&self) -> String {
        match self {
            Source::Default => "default".into(),
            Source::File => "file".into(),
            Source::Env(name) => format!("env:{name}"),
        }
    }

    fn of(meta: &Metadata) -> Source {
        match meta.name.as_ref() {
            "file" => Source::File,
            n => match n.strip_prefix("env:") {
                Some(var) => Source::Env(var.to_string()),
                None => Source::Default,
            },
        }
    }
}

impl Serialize for Source {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.label())
    }
}

/// The configuration of a home, layered (see the module documentation).
#[derive(Clone, Debug)]
pub struct HomeConfig {
    /// Where the file is, for error messages.
    path: PathBuf,
    figment: Figment,
    /// The variables ignored ([`HomeConfig::ignored_env`]).
    ignored: Vec<String>,
}

impl Default for HomeConfig {
    fn default() -> Self {
        HomeConfig::layered(PathBuf::from(FILE), Dict::new(), std::iter::empty())
    }
}

/// One layer of the stack: its values and the metadata naming its source.
struct Layer {
    meta: Metadata,
    dict: Dict,
}

impl Provider for Layer {
    fn metadata(&self) -> Metadata {
        self.meta.clone()
    }

    fn data(&self) -> std::result::Result<Map<Profile, Dict>, figment::Error> {
        Ok(Profile::Default.collect(self.dict.clone()))
    }
}

/// The defaults of every table that has them, as one dictionary.
fn defaults() -> Dict {
    #[derive(Serialize)]
    struct Serve {
        port: u16,
    }
    #[derive(Serialize)]
    struct Questions {
        terminal_after_s: u64,
    }
    #[derive(Serialize)]
    struct Defaults {
        serve: Serve,
        sample: SampleConfig,
        toolpath: ToolpathConfig,
        gallery: GalleryConfig,
        questions: Questions,
        bot: BotConfig,
    }
    let d = Defaults {
        serve: Serve { port: DEFAULT_PORT },
        sample: SampleConfig::default(),
        toolpath: ToolpathConfig::default(),
        gallery: GalleryConfig::default(),
        questions: Questions {
            terminal_after_s: TERMINAL_AFTER_S,
        },
        bot: BotConfig::default(),
    };
    Value::serialize(d)
        .ok()
        .and_then(|v| v.into_dict())
        .unwrap_or_default()
}

/// The key path an environment variable sets (`CLAX_BOT__MODELS__DEFAULT` →
/// `["bot", "models", "default"]`), or `None` for one that sets no setting:
/// it must start with [`ENV_PREFIX`], be in capitals, digits and
/// underscores, and nest at least once (`__`) with no empty part.
pub fn env_key(name: &str) -> Option<Vec<String>> {
    let rest = name.strip_prefix(ENV_PREFIX)?;
    if !rest.contains("__")
        || !rest
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let parts: Vec<String> = rest.split("__").map(str::to_ascii_lowercase).collect();
    parts
        .iter()
        .all(|p| !p.is_empty() && !p.starts_with('_') && !p.ends_with('_'))
        .then_some(parts)
}

/// The variable that sets the setting at `key` (`bot.models.default` →
/// `CLAX_BOT__MODELS__DEFAULT`).
pub fn env_name(key: &str) -> String {
    format!(
        "{ENV_PREFIX}{}",
        key.split('.')
            .map(str::to_ascii_uppercase)
            .collect::<Vec<_>>()
            .join("__")
    )
}

/// An environment variable's value as the setting at `key` takes it: the
/// text itself when the setting's default is a string (so
/// `CLAX_BOT__MODELS__DEFAULT=123` is the model ID `"123"`), else a TOML
/// value when the text parses as one (`7481`, `true`), else the text as a
/// string (which the table then refuses, naming the variable).
fn env_value(key: &[String], raw: &str) -> Value {
    let mut at = Value::from(defaults());
    for part in key {
        match at.into_dict().and_then(|mut d| d.remove(part)) {
            Some(v) => at = v,
            None => return toml_value(raw),
        }
    }
    match at {
        Value::String(..) | Value::Char(..) => Value::from(raw.to_string()),
        _ => toml_value(raw),
    }
}

/// `raw` as a TOML value when it parses as one, else as a string.
fn toml_value(raw: &str) -> Value {
    format!("v = {raw}")
        .parse::<toml::Table>()
        .ok()
        .and_then(|mut t| t.remove("v"))
        .and_then(|v| Value::serialize(v).ok())
        .unwrap_or_else(|| Value::from(raw.to_string()))
}

/// `dict` with `value` at `path`, making the dictionaries on the way.
fn nest(path: &[String], value: Value) -> Dict {
    let mut dict = Dict::new();
    match path {
        [] => {}
        [last] => {
            dict.insert(last.clone(), value);
        }
        [first, rest @ ..] => {
            dict.insert(first.clone(), Value::from(nest(rest, value)));
        }
    }
    dict
}

/// What kind of value `v` is, as an error message names it.
fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::String(..) | Value::Char(..) => "string",
        Value::Bool(..) => "boolean",
        Value::Num(_, n) if n.to_f64().is_some() => "float",
        Value::Num(..) => "integer",
        Value::Empty(..) => "empty",
        Value::Dict(..) => "table",
        Value::Array(..) => "array",
    }
}

/// `v` as an error message shows it: its JSON form.
fn shown(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| kind_of(v).to_string())
}

/// `v` as an integer, when it is one (never a float).
fn as_int(v: &Value) -> Option<i128> {
    match v {
        Value::Num(_, n) => n
            .to_i128()
            .or_else(|| n.to_u128().and_then(|u| i128::try_from(u).ok())),
        _ => None,
    }
}

impl HomeConfig {
    /// `<home_root>/config.toml` over the defaults, under the process's
    /// environment; a missing file is an empty one.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file when it exists but
    /// cannot be read or does not parse.
    pub fn load(home_root: &Path) -> Result<HomeConfig> {
        HomeConfig::load_with_env(home_root, process_env())
    }

    /// As [`HomeConfig::load`], with `env` as the environment.
    ///
    /// # Errors
    /// As [`HomeConfig::load`].
    pub fn load_with_env(
        home_root: &Path,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<HomeConfig> {
        let path = home_root.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(bad_file(&path, &e)),
        };
        HomeConfig::from_text(path, text.as_deref().unwrap_or(""), env)
    }

    /// The configuration `text` would give, read as the file at `path`.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming `path` when `text` does not
    /// parse.
    pub fn from_text(
        path: PathBuf,
        text: &str,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<HomeConfig> {
        let table = text
            .parse::<toml::Table>()
            .map_err(|e| bad_file(&path, &e))?;
        if let Some(serve) = table.get("serve").and_then(|v| v.as_table()) {
            for key in serve.keys().filter(|k| !SERVE_KEYS.contains(&k.as_str())) {
                warn_once(format!(
                    "{}: unknown key [serve] {key} ignored",
                    path.display()
                ));
            }
        }
        let file = Value::serialize(table)
            .ok()
            .and_then(|v| v.into_dict())
            .unwrap_or_default();
        Ok(HomeConfig::layered(path, file, env))
    }

    fn layered(
        path: PathBuf,
        file: Dict,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> HomeConfig {
        let mut figment = Figment::new()
            .merge(Layer {
                meta: Metadata::named("default"),
                dict: defaults(),
            })
            .merge(Layer {
                meta: Metadata::named("file").source(path.as_path()),
                dict: file,
            });
        let mut vars: Vec<(String, String)> = env
            .into_iter()
            .filter(|(n, v)| !v.is_empty() && env_key(n).is_some())
            .collect();
        vars.sort();
        let mut ignored = Vec::new();
        for (name, raw) in vars {
            let Some(key) = env_key(&name) else { continue };
            if FILE_ONLY_TABLES.contains(&key[0].as_str()) {
                warn_once(format!(
                    "{name} ignored: [{}] is read from config.toml only",
                    key[0]
                ));
                ignored.push(name);
                continue;
            }
            figment = figment.merge(Layer {
                meta: Metadata::named(format!("env:{name}")),
                dict: nest(&key, env_value(&key, &raw)),
            });
        }
        HomeConfig {
            path,
            figment,
            ignored,
        }
    }

    /// The variables that name a setting of a file-only table
    /// ([`FILE_ONLY_TABLES`]) and were ignored.
    pub fn ignored_env(&self) -> &[String] {
        &self.ignored
    }

    /// Where the file is (whether or not it exists).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value at `key` (`sample.max_tokens`), when any layer sets it.
    pub fn value(&self, key: &str) -> Option<Value> {
        self.figment.find_value(key).ok()
    }

    /// Where the value at `key` came from: [`Source::Default`] when no
    /// layer sets it.
    pub fn source(&self, key: &str) -> Source {
        self.figment
            .find_metadata(key)
            .map(Source::of)
            .unwrap_or(Source::Default)
    }

    /// What an error about `key` names: the variable that set it, else the
    /// file.
    fn origin(&self, key: &str) -> String {
        match self.source(key) {
            Source::Env(name) => name,
            _ => self.path.display().to_string(),
        }
    }

    /// `bad_config` about `key` (whose source names the origin), with
    /// `what` after the table's label.
    fn bad(&self, key: &str, label: &str, what: impl std::fmt::Display) -> CoreError {
        let sep = if label.is_empty() { "" } else { " " };
        CoreError::invalid(
            "bad_config",
            format!("{}: {label}{sep}{what}", self.origin(key)),
        )
    }

    /// The table at `table`, checked to be one, deserialized; a figment
    /// error names its key and the variable that set it, if one did.
    fn table<T: for<'de> Deserialize<'de>>(&self, table: &str) -> Result<T> {
        let label = format!("[{table}]");
        if let Some(v) = self.value(table)
            && v.as_dict().is_none()
        {
            return Err(self.bad(
                table,
                &label,
                format!("must be a table, not {}", kind_of(&v)),
            ));
        }
        self.figment.extract_inner::<T>(table).map_err(|e| {
            let key = if e.path.is_empty() {
                table.to_string()
            } else {
                e.path.join(".")
            };
            let at = e
                .metadata
                .as_ref()
                .map(Source::of)
                .and_then(|s| match s {
                    Source::Env(n) => Some(n),
                    _ => None,
                })
                .unwrap_or_else(|| self.path.display().to_string());
            let field = key
                .strip_prefix(table)
                .and_then(|k| k.strip_prefix('.'))
                .map(|k| format!(" (at {k})"))
                .unwrap_or_default();
            CoreError::invalid("bad_config", format!("{at}: {label} {}{field}", e.kind))
        })
    }

    /// The top-level `bin` setting, the clax the plugins run (spec §13):
    /// `None` when absent. Only the file sets it.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and the key when it
    /// is not a string that [`bin_path_ok`] accepts.
    pub fn bin(&self) -> Result<Option<PathBuf>> {
        let Some(v) = self.value("bin") else {
            return Ok(None);
        };
        match v.as_str() {
            Some(s) if bin_path_ok(s) => Ok(Some(s.into())),
            _ => Err(self.bad(
                "bin",
                "",
                format!(
                    "bin must be an absolute path with no quote, backslash or control character, not {}",
                    shown(&v)
                ),
            )),
        }
    }

    /// `[serve] port`: `None` when neither the file nor the environment
    /// sets it (the daemon then listens on 7480).
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file (or the variable)
    /// and the key when `[serve]` is not a table, or the port is not an
    /// integer in 1..=65535.
    pub fn serve_port(&self) -> Result<Option<u16>> {
        if let Some(serve) = self.value("serve")
            && serve.as_dict().is_none()
        {
            return Err(self.bad(
                "serve",
                "[serve]",
                format!("must be a table, not {}", kind_of(&serve)),
            ));
        }
        if self.source("serve.port") == Source::Default {
            return Ok(None);
        }
        let Some(port) = self.value("serve.port") else {
            return Ok(None);
        };
        as_int(&port)
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .map(Some)
            .ok_or_else(|| {
                self.bad(
                    "serve.port",
                    "[serve]",
                    format!(
                        "port must be an integer from 1 to 65535, not {}",
                        shown(&port)
                    ),
                )
            })
    }

    /// `[serve] trusted_hosts`: the host names, lowercased, that the API
    /// answers to as it does to `localhost` (a reverse proxy in front of the
    /// daemon that keeps its own name in `Host`, such as `tailscale serve`);
    /// empty when absent. Each is matched exactly, never as a suffix or a
    /// pattern ([`trusted_host_ok`] says which names may be listed).
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file (or the variable)
    /// and the key when `[serve]` is not a table, the key is not an array of
    /// strings, or an entry is not a name [`trusted_host_ok`] accepts. The
    /// daemon does not start on it: no entry of a list that is wrong is
    /// trusted.
    pub fn serve_trusted_hosts(&self) -> Result<Vec<String>> {
        if let Some(serve) = self.value("serve")
            && serve.as_dict().is_none()
        {
            return Err(self.bad(
                "serve",
                "[serve]",
                format!("must be a table, not {}", kind_of(&serve)),
            ));
        }
        let Some(list) = self.value("serve.trusted_hosts") else {
            return Ok(Vec::new());
        };
        let bad = |what: String| self.bad("serve.trusted_hosts", "[serve]", what);
        let Some(list) = list.as_array() else {
            return Err(bad(format!(
                "trusted_hosts must be an array of host names, not {}",
                kind_of(&list)
            )));
        };
        let mut out: Vec<String> = Vec::with_capacity(list.len());
        for v in list {
            let name = v.as_str().filter(|n| trusted_host_ok(n)).ok_or_else(|| {
                bad(format!(
                    "trusted_hosts entries are bare DNS names (no scheme, port, path or wildcard; not localhost or an IP address), not {}",
                    shown(v)
                ))
            })?;
            let name = name.to_ascii_lowercase();
            if !out.contains(&name) {
                out.push(name);
            }
        }
        Ok(out)
    }

    /// `[serve] tailscale_identity`: whether a request through `tailscale
    /// serve` (a loopback peer naming one of [`HomeConfig::serve_trusted_hosts`])
    /// is the viewer its `Tailscale-User-Login` header names; `false` when
    /// absent. It names viewers only: it grants no rights.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file (or the variable)
    /// and the key when `[serve]` is not a table, the key is not a boolean,
    /// or it is `true` with no `trusted_hosts` (there is then no request it
    /// could trust, and the daemon does not start rather than read the
    /// headers from anyone); [`HomeConfig::serve_trusted_hosts`]'s errors
    /// when it is `true`.
    pub fn serve_tailscale_identity(&self) -> Result<bool> {
        if let Some(serve) = self.value("serve")
            && serve.as_dict().is_none()
        {
            return Err(self.bad(
                "serve",
                "[serve]",
                format!("must be a table, not {}", kind_of(&serve)),
            ));
        }
        let Some(v) = self.value("serve.tailscale_identity") else {
            return Ok(false);
        };
        let bad = |what: String| self.bad("serve.tailscale_identity", "[serve]", what);
        let Some(on) = v.to_bool() else {
            return Err(bad(format!(
                "tailscale_identity must be true or false, not {}",
                shown(&v)
            )));
        };
        if on && self.serve_trusted_hosts()?.is_empty() {
            return Err(bad(
                "tailscale_identity = true needs trusted_hosts to name the tailscale serve host"
                    .to_string(),
            ));
        }
        Ok(on)
    }

    /// `[serve] owner_tailscale_login`: the one tailnet login (such as
    /// `bdelanghe@github`, matched exactly) whose browser, on a request
    /// through `tailscale serve` that [`HomeConfig::serve_tailscale_identity`]
    /// trusts, is the owner's, as this machine's own browser is; `None` when
    /// absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file (or the variable)
    /// and the key when `[serve]` is not a table, the key is not a string of
    /// 1 to 256 bytes with no whitespace, control character or `*`, or it is
    /// set without `tailscale_identity = true` (whose errors it also
    /// returns). The daemon does not start on it.
    pub fn serve_owner_tailscale_login(&self) -> Result<Option<String>> {
        if let Some(serve) = self.value("serve")
            && serve.as_dict().is_none()
        {
            return Err(self.bad(
                "serve",
                "[serve]",
                format!("must be a table, not {}", kind_of(&serve)),
            ));
        }
        let Some(v) = self.value("serve.owner_tailscale_login") else {
            return Ok(None);
        };
        let bad = |what: String| self.bad("serve.owner_tailscale_login", "[serve]", what);
        let Some(login) = v.as_str().filter(|l| {
            (1..=256).contains(&l.len())
                && !l
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '*')
        }) else {
            return Err(bad(format!(
                "owner_tailscale_login must be one tailnet login such as \"you@github\" (no space, control character or wildcard), not {}",
                shown(&v)
            )));
        };
        let login = login.to_string();
        if !self.serve_tailscale_identity()? {
            return Err(bad(
                "owner_tailscale_login needs tailscale_identity = true (and trusted_hosts)"
                    .to_string(),
            ));
        }
        Ok(Some(login))
    }

    /// `[serve] trusted_host_frames`: whether a shell on one of
    /// [`HomeConfig::serve_trusted_hosts`] frames each artifact from a path
    /// of that same host (`/_clax/frame/<id>/…`) in a sandbox without
    /// `allow-same-origin` (an opaque origin), its runtime calls relayed by
    /// the shell, rather than from the artifact's `<id>.localhost` site,
    /// which a browser on another machine cannot open; `false` when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file (or the variable)
    /// and the key when `[serve]` is not a table, the key is not a boolean,
    /// or it is `true` with no `trusted_hosts`;
    /// [`HomeConfig::serve_trusted_hosts`]'s errors when it is `true`. The
    /// daemon does not start on it.
    pub fn serve_trusted_host_frames(&self) -> Result<bool> {
        if let Some(serve) = self.value("serve")
            && serve.as_dict().is_none()
        {
            return Err(self.bad(
                "serve",
                "[serve]",
                format!("must be a table, not {}", kind_of(&serve)),
            ));
        }
        let Some(v) = self.value("serve.trusted_host_frames") else {
            return Ok(false);
        };
        let bad = |what: String| self.bad("serve.trusted_host_frames", "[serve]", what);
        let Some(on) = v.to_bool() else {
            return Err(bad(format!(
                "trusted_host_frames must be true or false, not {}",
                shown(&v)
            )));
        };
        if on && self.serve_trusted_hosts()?.is_empty() {
            return Err(bad(
                "trusted_host_frames = true needs trusted_hosts to name the host the shell is served on"
                    .to_string(),
            ));
        }
        Ok(on)
    }

    /// `[sample]`: the default when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and `[sample]` when it
    /// is not a table, has an unknown key or a mistyped value, names a
    /// provider other than `anthropic` or `stub`, sets `max_tokens = 0`,
    /// leaves a model ID empty, or sets a `base_url` that [`base_url_ok`]
    /// refuses. The caller turns sampling off; it never stops the daemon.
    pub fn sample(&self) -> Result<SampleConfig> {
        let s: SampleConfig = self.table("sample")?;
        let bad = |key: &str, what: String| self.bad(&format!("sample.{key}"), "[sample]", what);
        if !matches!(s.provider.as_str(), "anthropic" | "stub") {
            return Err(bad(
                "provider",
                format!(
                    "provider is \"anthropic\" or \"stub\", not \"{}\"",
                    s.provider
                ),
            ));
        }
        if s.max_tokens == 0 {
            return Err(bad("max_tokens", "max_tokens must be at least 1".into()));
        }
        if let Some(c) = s.daily_cost_cap_usd
            && !(c.is_finite() && c >= 0.0)
        {
            return Err(bad(
                "daily_cost_cap_usd",
                format!("daily_cost_cap_usd must be a number of dollars, 0 or more, not {c}"),
            ));
        }
        for (tier, id) in [
            ("quick", &s.models.quick),
            ("default", &s.models.default),
            ("complex", &s.models.complex),
        ] {
            if id.trim().is_empty() {
                return Err(bad(
                    &format!("models.{tier}"),
                    format!("models.{tier} is empty"),
                ));
            }
        }
        if !base_url_ok(&s.base_url) {
            return Err(bad(
                "base_url",
                format!(
                    "base_url must be https://, or http:// on localhost, 127.0.0.1 or [::1], not \"{}\"",
                    s.base_url
                ),
            ));
        }
        Ok(s)
    }

    /// `[bot]`: ClaxBot's provider, models, caps and credits (spec
    /// 2026-10-21-claxbot-design §8); the default when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file (or the variable)
    /// and `[bot]` when it is not a table, has an unknown key or a
    /// mistyped value, leaves `api_key_env` or a model ID empty, sets a
    /// `base_url` that [`base_url_ok`] refuses, or sets a count, cap or
    /// price outside its range ([`BotConfig`]). The caller turns ClaxBot
    /// off; it never stops the daemon.
    pub fn bot(&self) -> Result<BotConfig> {
        let b: BotConfig = self.table("bot")?;
        let bad = |key: &str, what: String| self.bad(&format!("bot.{key}"), "[bot]", what);
        if b.api_key_env.trim().is_empty() {
            return Err(bad("api_key_env", "api_key_env is empty".into()));
        }
        if !base_url_ok(&b.base_url) {
            return Err(bad(
                "base_url",
                format!(
                    "base_url must be https://, or http:// on localhost, 127.0.0.1 or [::1], not \"{}\"",
                    b.base_url
                ),
            ));
        }
        for (tier, id) in [
            ("quick", &b.models.quick),
            ("default", &b.models.default),
            ("complex", &b.models.complex),
        ] {
            if id.trim().is_empty() {
                return Err(bad(
                    &format!("models.{tier}"),
                    format!("models.{tier} is empty"),
                ));
            }
        }
        let ranges: [(&str, u64, u64, u64); 11] = [
            ("max_tokens", b.max_tokens.into(), 1, 1_000_000),
            ("max_steps", b.max_steps.into(), 1, 10_000),
            ("max_run_credits", b.max_run_credits, 1, u64::MAX),
            ("max_run_minutes", b.max_run_minutes.into(), 1, 24 * 60),
            ("concurrency", b.concurrency.into(), 1, 64),
            (
                "per_person_concurrency",
                b.per_person_concurrency.into(),
                1,
                64,
            ),
            (
                "credit_price_micro_usd",
                b.credit_price_micro_usd,
                1,
                u64::MAX,
            ),
            ("script.cpu_ms", b.script.cpu_ms, 1, 600_000),
            ("script.memory_mib", b.script.memory_mib.into(), 1, 4096),
            ("script.max_calls", b.script.max_calls.into(), 1, 100_000),
            ("free_weekly_credits", b.free_weekly_credits, 0, u64::MAX),
        ];
        for (key, v, lo, hi) in ranges {
            if !(lo..=hi).contains(&v) {
                let range = if hi == u64::MAX {
                    format!("at least {lo}")
                } else {
                    format!("from {lo} to {hi}")
                };
                return Err(bad(key, format!("{key} must be {range}, not {v}")));
            }
        }
        Ok(b)
    }

    /// `[questions] terminal_after_s` (spec 2026-10-06-agent-questions-and-inbox
    /// §6.7): how many seconds a mirrored AskUserQuestion waits in Clax
    /// before the terminal dialog appears; 0 never holds it. Default
    /// [`TERMINAL_AFTER_S`], clamped to `0..=`[`MAX_TERMINAL_AFTER_S`]; an
    /// out-of-range or mistyped value is logged.
    pub fn questions_terminal_after_s(&self) -> u64 {
        if self
            .value("questions")
            .is_none_or(|q| q.as_dict().is_none())
        {
            return TERMINAL_AFTER_S;
        }
        let Some(v) = self.value("questions.terminal_after_s") else {
            return TERMINAL_AFTER_S;
        };
        match as_int(&v) {
            Some(n) => {
                let clamped = n.clamp(0, MAX_TERMINAL_AFTER_S as i128);
                if clamped != n {
                    tracing::warn!(
                        value = %n,
                        used = %clamped,
                        "questions.terminal_after_s is out of range"
                    );
                }
                clamped as u64
            }
            None => {
                tracing::warn!(
                    kind = kind_of(&v),
                    used = TERMINAL_AFTER_S,
                    "questions.terminal_after_s is not an integer"
                );
                TERMINAL_AFTER_S
            }
        }
    }

    /// `[hosted]`: `None` when absent (the local daemon).
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and `[hosted]` when
    /// it is not a table, has an unknown key or a mistyped value, lacks
    /// `public_origin`, names an origin [`hosted_origin`] refuses, or names
    /// an empty `setup_secret_env`. A daemon refuses to start on it: falling
    /// back to the local daemon would serve the internet as a LAN.
    pub fn hosted(&self) -> Result<Option<HostedConfig>> {
        if self.value("hosted").is_none() {
            return Ok(None);
        }
        let t: HostedTable = self.table("hosted")?;
        let bad = |key: &str, what: String| self.bad(&format!("hosted.{key}"), "[hosted]", what);
        let public_origin = hosted_origin(&t.public_origin).ok_or_else(|| {
            bad(
                "public_origin",
                format!(
                    "public_origin must be an https:// origin with no path, or http:// on localhost, 127.0.0.1 or [::1], not \"{}\"",
                    t.public_origin
                ),
            )
        })?;
        let setup_secret_env = t
            .setup_secret_env
            .unwrap_or_else(|| SETUP_SECRET_ENV.into());
        if setup_secret_env.trim().is_empty() {
            return Err(bad("setup_secret_env", "setup_secret_env is empty".into()));
        }
        let github = t
            .github_client_id
            .filter(|c| !c.trim().is_empty())
            .map(|client_id| GitHubConfig {
                client_id: client_id.trim().to_string(),
                client_secret_env: t
                    .github_client_secret_env
                    .unwrap_or_else(|| GITHUB_CLIENT_SECRET_ENV.into()),
                owner_login: t.owner_github.filter(|o| !o.trim().is_empty()),
                api_base: t.github_api_base,
                oauth_base: t.github_oauth_base,
            });
        let legal = Legal::from_settings(
            t.legal.as_deref(),
            t.terms_url.as_deref(),
            t.privacy_url.as_deref(),
            t.legal_version.as_deref(),
        )
        .map_err(|e| bad("legal", e))?;
        let links = t.app_links.as_ref();
        let stores = t.stores.as_ref();
        let clax_app = ClaxApp::from_settings(
            links.and_then(|l| l.apple_team_id.as_deref()),
            links.and_then(|l| l.android_package.as_deref()),
            links.and_then(|l| l.android_sha256.as_deref()),
            stores.and_then(|s| s.ios.as_deref()),
            stores.and_then(|s| s.android.as_deref()),
        )
        .map_err(|e| bad("app_links", e))?;
        let site_domain = match t.site_domain.as_deref() {
            None => None,
            Some(d) => Some(hosted_site_domain(d, &public_origin).map_err(|e| match e {
                SiteDomainError::NotBare => bad(
                    "site_domain",
                    format!("site_domain must be a bare domain name such as clax.run, not \"{d}\""),
                ),
                SiteDomainError::SharesPublicHost {
                    domain,
                    public_host,
                } => bad(
                    "site_domain",
                    format!(
                        "site_domain \"{domain}\" must not be public_origin's host {public_host}, a domain above it or one under it"
                    ),
                ),
            })?),
        };
        Ok(Some(HostedConfig {
            public_origin,
            setup_secret_env,
            behind_cloudflare: t.behind_cloudflare.unwrap_or(false),
            github,
            member_max_artifacts: t.member_max_artifacts.unwrap_or(DEFAULT_MAX_ARTIFACTS),
            public_gallery: t.public_gallery.unwrap_or(false),
            legal,
            site_domain,
            clax_app,
        }))
    }

    /// `[gallery]`: the default when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and `[gallery]`
    /// when it is not a table, has an unknown key or a mistyped value.
    pub fn gallery(&self) -> Result<GalleryConfig> {
        self.table("gallery")
    }

    /// `[gallery] quickstart`: whether a hosted site seeds a new person's
    /// gallery with the Clax Quickstart page, and whether a quickstart is
    /// moved to newer releases (a local home is never seeded). Default
    /// true; a `[gallery]` that does not check is logged and leaves it
    /// true.
    pub fn gallery_quickstart(&self) -> bool {
        match self.gallery() {
            Ok(g) => g.quickstart,
            Err(e) => {
                warn_once(format!("{e}; [gallery] quickstart stays on"));
                GalleryConfig::default().quickstart
            }
        }
    }

    /// `[toolpath]`: the default when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and `[toolpath]`
    /// when it is not a table, has an unknown key or a mistyped value, or
    /// sets `segment_max_mb` below 1 or above 4096.
    pub fn toolpath(&self) -> Result<ToolpathConfig> {
        let t: ToolpathConfig = self.table("toolpath")?;
        if !(1..=4096).contains(&t.segment_max_mb) {
            return Err(self.bad(
                "toolpath.segment_max_mb",
                "[toolpath]",
                format!(
                    "segment_max_mb must be from 1 to 4096, not {}",
                    t.segment_max_mb
                ),
            ));
        }
        Ok(t)
    }
}

/// Logs `message` as a warning the first time this process sees it: the
/// configuration is read again on each settings request, and a warning
/// about the same key or variable says nothing new the second time.
fn warn_once(message: String) {
    static SEEN: std::sync::Mutex<std::collections::BTreeSet<String>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let first = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(message.clone());
    if first {
        tracing::warn!("{message}");
    }
}

/// `bad_config` naming the file `path`.
fn bad_file(path: &Path, e: &dyn std::fmt::Display) -> CoreError {
    CoreError::invalid("bad_config", format!("{}: {e}", path.display()))
}

/// Keys `[serve]` understands; any other key there is logged and ignored.
const SERVE_KEYS: &[&str] = &[
    "port",
    "trusted_hosts",
    "tailscale_identity",
    "owner_tailscale_login",
    "trusted_host_frames",
];

/// `[sample]`: the `sample` capability's provider, key variable, models, and cap.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SampleConfig {
    /// `anthropic` (default) or `stub` (a deterministic provider for tests and demos).
    pub provider: String,
    /// The environment variable the daemon reads the API key from, at start.
    pub api_key_env: String,
    /// Where the Messages API is; the key is sent nowhere else. `https://`,
    /// or `http://` on a loopback host only ([`base_url_ok`]).
    pub base_url: String,
    pub models: SampleModels,
    /// Most calls per artifact per local day that reach the provider; none when absent.
    pub daily_call_cap: Option<u32>,
    /// Most US dollars of model use per artifact per local day, priced from
    /// each round's reported usage; default 0.50. A call that starts under
    /// the cap runs to its end, so the day's spend can pass it by one call.
    pub daily_cost_cap_usd: Option<f64>,
    /// `max_tokens` of each provider request.
    pub max_tokens: u32,
    /// The stub reports image support.
    pub stub_images: bool,
    /// The stub's pause before each streamed piece, in milliseconds.
    pub stub_delay_ms: u64,
}

impl Default for SampleConfig {
    fn default() -> Self {
        SampleConfig {
            provider: "anthropic".into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            base_url: "https://api.anthropic.com".into(),
            models: SampleModels::default(),
            daily_call_cap: None,
            daily_cost_cap_usd: Some(DAILY_COST_CAP_USD),
            max_tokens: 16000,
            stub_images: false,
            stub_delay_ms: 40,
        }
    }
}

/// The default of [`SampleConfig::daily_cost_cap_usd`].
pub const DAILY_COST_CAP_USD: f64 = 0.50;

/// `[sample.models]`: the model ID each `modelTier` maps to.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SampleModels {
    pub quick: String,
    pub default: String,
    pub complex: String,
}

impl Default for SampleModels {
    fn default() -> Self {
        SampleModels {
            quick: "claude-haiku-4-5".into(),
            default: "claude-sonnet-5-5".into(),
            complex: "claude-opus-5-5".into(),
        }
    }
}

/// `[gallery]`: what a gallery starts with.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GalleryConfig {
    /// `quickstart`: seed each new hosted gallery with the Clax Quickstart
    /// page, and move an untouched one to newer releases (default true).
    pub quickstart: bool,
}

impl Default for GalleryConfig {
    fn default() -> Self {
        GalleryConfig { quickstart: true }
    }
}

/// `[toolpath]`: the audit journal (spec 2026-10-06-toolpath-audit-design
/// §7).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ToolpathConfig {
    /// `journal`: write the journal (default true). Audit events are
    /// recorded either way, and the journal catches up when turned back on.
    pub journal: bool,
    /// `journal_text`: render free text into the journal (default true);
    /// false renders it as `--no-text` does (§7.7).
    pub journal_text: bool,
    /// `segment_max_mb`: the size a segment stays within, in MiB (default
    /// 64, at least 1).
    pub segment_max_mb: u64,
    /// `journal_retain_days`: days a closed segment is kept; 0 keeps every
    /// segment (the default). Never touches the audit table.
    pub journal_retain_days: u32,
}

impl Default for ToolpathConfig {
    fn default() -> Self {
        ToolpathConfig {
            journal: true,
            journal_text: true,
            segment_max_mb: 64,
            journal_retain_days: 0,
        }
    }
}

/// `[bot]`: ClaxBot (spec 2026-10-21-claxbot-design §8): whether the
/// instance runs it, its provider, models and caps, and the credit
/// arithmetic's instance settings. [`HomeConfig::bot`] checks the ranges
/// each field's documentation names.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BotConfig {
    /// The instance may run ClaxBot at all (default true).
    pub enabled: bool,
    /// The environment variable the daemon reads the instance's API key
    /// from (`ANTHROPIC_API_KEY`); not empty. A key there takes precedence
    /// over a stored instance key (spec §8.1).
    pub api_key_env: String,
    /// Where the Messages API is; the key is sent nowhere else
    /// ([`base_url_ok`]).
    pub base_url: String,
    /// `max_tokens` of each model turn: 1 to 1,000,000 (default 32,000).
    pub max_tokens: u32,
    /// The most steps one run takes: 1 to 10,000 (default 40).
    pub max_steps: u32,
    /// The most credits one run spends: at least 1 (default 200).
    pub max_run_credits: u64,
    /// The longest one run lasts, in minutes: 1 to 1,440 (default 30).
    pub max_run_minutes: u32,
    /// How many runs the daemon drives at once: 1 to 64 (default 4).
    pub concurrency: u32,
    /// How many of one person's runs the daemon drives at once, within
    /// `concurrency`: 1 to 64 (default 1), so one person's runs cannot take
    /// every slot (or every sandbox thread) from everyone else.
    pub per_person_concurrency: u32,
    /// Each person's free credits a week (default 2,000).
    pub free_weekly_credits: u64,
    /// What one credit costs, in micro-USD: at least 1 (default 10,000,
    /// $0.01).
    pub credit_price_micro_usd: u64,
    pub models: BotModels,
    pub script: BotScript,
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            enabled: true,
            api_key_env: "ANTHROPIC_API_KEY".into(),
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 32000,
            max_steps: 40,
            max_run_credits: 200,
            max_run_minutes: 30,
            concurrency: 4,
            per_person_concurrency: 1,
            free_weekly_credits: 2000,
            credit_price_micro_usd: 10_000,
            models: BotModels::default(),
            script: BotScript::default(),
        }
    }
}

/// `[bot.models]`: the model ID of each tier; none empty.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BotModels {
    pub default: String,
    pub quick: String,
    pub complex: String,
}

impl Default for BotModels {
    fn default() -> Self {
        BotModels {
            default: "claude-sonnet-5-5".into(),
            quick: "claude-haiku-5-5".into(),
            complex: "claude-opus-5-5".into(),
        }
    }
}

/// `[bot.script]`: the limits of one sandboxed script (spec §6.1).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BotScript {
    /// CPU time, in milliseconds: 1 to 600,000 (default 2,000).
    pub cpu_ms: u64,
    /// Memory, in MiB: 1 to 4,096 (default 64).
    pub memory_mib: u32,
    /// Host calls: 1 to 100,000 (default 200).
    pub max_calls: u32,
}

impl Default for BotScript {
    fn default() -> Self {
        BotScript {
            cpu_ms: 2000,
            memory_mib: 64,
            max_calls: 200,
        }
    }
}

/// How many live artifacts one GitHub member may keep by default
/// (`[hosted] member_max_artifacts`; the store's `MemberCaps`).
pub const DEFAULT_MAX_ARTIFACTS: u64 = 200;

/// `[hosted]`: hosted mode, in which the daemon serves the public internet
/// at `public_origin` (behind a proxy that terminates TLS and keeps the
/// `Host` header): people sign in through the browser with session cookies,
/// and nothing answers a request as "from this machine".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedConfig {
    /// The origin people reach the daemon at, normalized (`https://host[:port]`;
    /// `http://` only on `localhost`, `127.0.0.1` or `[::1]`, for development).
    pub public_origin: String,
    /// The environment variable the daemon reads the setup secret from, at
    /// start (`CLAX_SETUP_SECRET` when absent).
    pub setup_secret_env: String,
    /// Cloudflare is in front of the daemon (false when absent): a
    /// request's client is the address its `CF-Connecting-IP` names, which
    /// the abuse limits count by. Without Cloudflare that header is the
    /// client's own to forge, so the connection's peer counts.
    pub behind_cloudflare: bool,
    /// GitHub sign-in (`github_client_id`); `None` keeps it off.
    pub github: Option<GitHubConfig>,
    /// The most live artifacts a GitHub member keeps
    /// (`member_max_artifacts`, [`DEFAULT_MAX_ARTIFACTS`]
    /// when absent).
    pub member_max_artifacts: u64,
    /// Whether pages may be listed in the public gallery that signed-out
    /// visitors see (`public_gallery`, false when absent).
    pub public_gallery: bool,
    /// The Terms of Service and Privacy Policy the instance shows and
    /// people agree to (`legal`, or `terms_url`, `privacy_url` and
    /// `legal_version`); `None` shows none.
    pub legal: Option<Legal>,
    /// The domain each artifact's site is served under, as
    /// `<id>.<site_domain>` (`site_domain`, normalized by [`site_domain`]);
    /// `None` serves sites at `<id>.localhost` alone.
    pub site_domain: Option<String>,
    /// What the instance says about the Clax app (`app_links` and
    /// `stores`): none of it when absent.
    pub clax_app: ClaxApp,
}

/// The Clax app's bundle ID on iOS and macOS, which the Apple app site
/// association names after the team ID.
pub const APPLE_BUNDLE_ID: &str = "dev.empathic.clax";

/// What a hosted Clax says about the Clax app: the app links its
/// `/.well-known` files declare, and the store pages its Get in Clax page
/// offers (`[hosted] app_links = {apple_team_id, android_package,
/// android_sha256}` and `stores = {ios, android}`, every key optional).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaxApp {
    /// The Apple developer team ID that signs the app (10 uppercase
    /// letters and digits): `/.well-known/apple-app-site-association`
    /// names `<team>.`[`APPLE_BUNDLE_ID`]. `None` serves no such file.
    pub apple_team_id: Option<String>,
    /// The Android app's package and its signing certificate's SHA-256
    /// fingerprint (32 uppercase hex bytes joined by `:`):
    /// `/.well-known/assetlinks.json` names them. `None` serves no such file.
    pub android: Option<AndroidApp>,
    /// The App Store page of the app (an `https://` URL).
    pub ios_store: Option<String>,
    /// The Play Store page of the app (an `https://` URL).
    pub android_store: Option<String>,
}

/// [`ClaxApp::android`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidApp {
    pub package: String,
    pub sha256: String,
}

impl ClaxApp {
    /// The settings as given (empty counts as absent): a team ID of 10
    /// uppercase letters and digits; a package (dot-separated Java
    /// identifiers) and a fingerprint (32 hex bytes joined by `:`, read
    /// in any case and kept in uppercase) together or neither; store pages
    /// as `https://` URLs.
    pub fn from_settings(
        apple_team_id: Option<&str>,
        android_package: Option<&str>,
        android_sha256: Option<&str>,
        ios_store: Option<&str>,
        android_store: Option<&str>,
    ) -> std::result::Result<ClaxApp, String> {
        fn set(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|v| !v.is_empty())
        }
        let team = set(apple_team_id);
        if let Some(t) = team {
            let ok = t.len() == 10
                && t.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
            if !ok {
                return Err(format!(
                    "apple_team_id is 10 uppercase letters and digits, not \"{t}\""
                ));
            }
        }
        let android = match (set(android_package), set(android_sha256)) {
            (None, None) => None,
            (Some(p), Some(f)) => {
                let ident = |s: &str| {
                    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                };
                if !(p.contains('.') && p.split('.').all(ident)) {
                    return Err(format!(
                        "android_package is a package name such as dev.empathic.clax, not \"{p}\""
                    ));
                }
                let f = f.to_ascii_uppercase();
                let parts: Vec<&str> = f.split(':').collect();
                let ok = parts.len() == 32
                    && parts
                        .iter()
                        .all(|h| h.len() == 2 && h.bytes().all(|b| b.is_ascii_hexdigit()));
                if !ok {
                    return Err(
                        "android_sha256 is a SHA-256 fingerprint: 32 hex bytes joined by ':'"
                            .into(),
                    );
                }
                Some(AndroidApp {
                    package: p.into(),
                    sha256: f,
                })
            }
            _ => return Err("android_package and android_sha256 go together".into()),
        };
        let store = |name: &str, v: Option<&str>| match set(v) {
            None => Ok(None),
            Some(u) if u.to_ascii_lowercase().starts_with("https://") && base_url_ok(u) => {
                Ok(Some(u.to_string()))
            }
            Some(u) => Err(format!(
                "stores.{name} must be an https:// URL, not \"{u}\""
            )),
        };
        Ok(ClaxApp {
            apple_team_id: team.map(str::to_string),
            android,
            ios_store: store("ios", ios_store)?,
            android_store: store("android", android_store)?,
        })
    }
}

/// The Terms of Service and Privacy Policy a hosted Clax shows: linked
/// from its sign-in pages and footers, and agreed to by each person who
/// signs in under their notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Legal {
    /// The documents in this repository's `docs/legal`, served at
    /// `/terms` and `/privacy` (`legal = "bundled"`): the hosted service
    /// Empathic operates.
    Bundled,
    /// The deployment's own documents at these `http(s)` URLs; `version`
    /// names the text a person agrees to, and changing it asks every
    /// person to agree again.
    Own {
        terms_url: String,
        privacy_url: String,
        version: String,
    },
}

/// The version of the bundled documents a person agrees to. Change it
/// only for a material change to `docs/legal`: every person is asked to
/// agree again. Editorial fixes change the documents' "Last Updated" date
/// alone.
pub const BUNDLED_LEGAL_VERSION: &str = "2026-10-19";

impl Legal {
    /// The settings as given (`legal`, `terms_url`, `privacy_url`,
    /// `version`; empty counts as absent): `legal = "bundled"` alone, or
    /// all three of the others, or none of them.
    pub fn from_settings(
        legal: Option<&str>,
        terms_url: Option<&str>,
        privacy_url: Option<&str>,
        version: Option<&str>,
    ) -> std::result::Result<Option<Legal>, String> {
        fn set(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|v| !v.is_empty())
        }
        let own = [set(terms_url), set(privacy_url), set(version)];
        match (set(legal), own) {
            (Some("bundled"), [None, None, None]) => Ok(Some(Legal::Bundled)),
            (Some("bundled"), _) => {
                Err("legal = \"bundled\" takes no terms_url, privacy_url or legal_version".into())
            }
            (Some(other), _) => Err(format!("legal must be \"bundled\", not \"{other}\"")),
            (None, [None, None, None]) => Ok(None),
            (None, [Some(t), Some(p), Some(v)]) => {
                for u in [t, p] {
                    if !base_url_ok(u) {
                        return Err(format!(
                            "terms_url and privacy_url must be https:// URLs (http:// only on localhost), not \"{u}\""
                        ));
                    }
                }
                Ok(Some(Legal::Own {
                    terms_url: t.into(),
                    privacy_url: p.into(),
                    version: v.into(),
                }))
            }
            (None, _) => Err("terms_url, privacy_url and legal_version go together".into()),
        }
    }

    /// Where the Terms of Service are read.
    pub fn terms_url(&self) -> &str {
        match self {
            Legal::Bundled => "/terms",
            Legal::Own { terms_url, .. } => terms_url,
        }
    }

    /// Where the Privacy Policy is read.
    pub fn privacy_url(&self) -> &str {
        match self {
            Legal::Bundled => "/privacy",
            Legal::Own { privacy_url, .. } => privacy_url,
        }
    }

    /// The version a person agrees to.
    pub fn version(&self) -> &str {
        match self {
            Legal::Bundled => BUNDLED_LEGAL_VERSION,
            Legal::Own { version, .. } => version,
        }
    }
}

/// `[hosted]`'s GitHub sign-in: a GitHub OAuth App's client ID, the
/// environment variable its client secret is read from at start
/// (`github_client_secret_env`, `GITHUB_CLIENT_SECRET` when absent), the
/// owner's GitHub login (`owner_github`, compared case-insensitively), and,
/// for tests, the bases of GitHub's API and OAuth endpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubConfig {
    pub client_id: String,
    pub client_secret_env: String,
    pub owner_login: Option<String>,
    pub api_base: Option<String>,
    pub oauth_base: Option<String>,
}

/// [`GitHubConfig::client_secret_env`] when absent.
pub const GITHUB_CLIENT_SECRET_ENV: &str = "GITHUB_CLIENT_SECRET";

/// [`HostedConfig::setup_secret_env`] when absent.
pub const SETUP_SECRET_ENV: &str = "CLAX_SETUP_SECRET";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedTable {
    public_origin: String,
    setup_secret_env: Option<String>,
    behind_cloudflare: Option<bool>,
    github_client_id: Option<String>,
    github_client_secret_env: Option<String>,
    owner_github: Option<String>,
    github_api_base: Option<String>,
    github_oauth_base: Option<String>,
    member_max_artifacts: Option<u64>,
    public_gallery: Option<bool>,
    legal: Option<String>,
    terms_url: Option<String>,
    privacy_url: Option<String>,
    legal_version: Option<String>,
    site_domain: Option<String>,
    app_links: Option<AppLinksTable>,
    stores: Option<StoresTable>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppLinksTable {
    apple_team_id: Option<String>,
    android_package: Option<String>,
    android_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoresTable {
    ios: Option<String>,
    android: Option<String>,
}

/// `domain` as a site domain ([`HostedConfig::site_domain`]): lowercased,
/// or `None` unless it is a bare host name: dot-separated labels of 1 to 63
/// ASCII letters, digits and `-`, none starting or ending with `-` (so no
/// scheme, port, path or leading dot), whose last label is not all digits
/// (so no IP literal).
pub fn site_domain(domain: &str) -> Option<String> {
    let label_ok = |label: &str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let last = domain.rsplit('.').next().unwrap_or("");
    let ok = domain.split('.').all(label_ok) && !last.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| domain.to_ascii_lowercase())
}

/// Whether domain names `a` and `b` (lowercase) are the same name or one is
/// under the other (`example.com` and `clax.example.com`).
fn nested_names(a: &str, b: &str) -> bool {
    let under = |child: &str, parent: &str| {
        child
            .strip_suffix(parent)
            .is_some_and(|rest| rest.ends_with('.'))
    };
    a == b || under(a, b) || under(b, a)
}

/// Why [`hosted_site_domain`] refused a site domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteDomainError {
    /// Not a bare host name ([`site_domain`]).
    NotBare,
    /// `domain` is the public origin's host `public_host`, a domain above
    /// it or one under it.
    SharesPublicHost { domain: String, public_host: String },
}

/// `domain` as the site domain of a hosted instance whose public origin is
/// `public_origin` (`[hosted] site_domain`, the Worker's `SITE_DOMAIN`):
/// [`site_domain`]'s form, sharing nothing with the public origin: not its
/// host, nor a name above or below it. `localhost` under a `localhost`
/// public origin is the exception: the names reach this machine alone, so
/// no registrable domain is shared.
pub fn hosted_site_domain(
    domain: &str,
    public_origin: &str,
) -> std::result::Result<String, SiteDomainError> {
    let domain = site_domain(domain).ok_or(SiteDomainError::NotBare)?;
    let public_host = url::Url::parse(public_origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    let loopback = domain == "localhost" && public_host == "localhost";
    if !loopback && nested_names(&domain, &public_host) {
        return Err(SiteDomainError::SharesPublicHost {
            domain,
            public_host,
        });
    }
    Ok(domain)
}

/// `origin` as hosted mode takes it ([`HostedConfig::public_origin`]): its
/// normalized serialization, or `None` when it is not a bare `https://`
/// origin or an `http://` one on a loopback name.
pub fn hosted_origin(origin: &str) -> Option<String> {
    let u = url::Url::parse(origin).ok()?;
    let bare = u.username().is_empty()
        && u.password().is_none()
        && matches!(u.path(), "" | "/")
        && u.query().is_none()
        && u.fragment().is_none();
    let local = matches!(
        u.host(),
        Some(url::Host::Domain("localhost"))
            | Some(url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST))
            | Some(url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST))
    );
    let ok =
        bare && u.host().is_some() && (u.scheme() == "https" || (u.scheme() == "http" && local));
    ok.then(|| u.origin().ascii_serialization())
}

/// [`HomeConfig::questions_terminal_after_s`] when the key is absent.
pub const TERMINAL_AFTER_S: u64 = 600;
/// The largest [`HomeConfig::questions_terminal_after_s`]: under the hook's
/// 3,600 s timeout with room for the terminal dialog.
pub const MAX_TERMINAL_AFTER_S: u64 = 3300;

/// Whether `path` can be the `bin` setting: absolute, and free of `"`, `\`
/// and control characters, so that `bin = "<path>"` is valid TOML whose
/// value is `path` verbatim, and the plugins' wrapper can read it without
/// a TOML parser.
pub fn bin_path_ok(path: &str) -> bool {
    path.starts_with('/')
        && !path
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control())
}

/// The line `clax bin set` writes for `path`, which must pass
/// [`bin_path_ok`]: `bin = "<path>"`.
pub fn bin_line(path: &str) -> String {
    format!("bin = \"{path}\"")
}

/// Whether `name` may be listed in `[serve] trusted_hosts`: a DNS name of
/// dot-separated labels (letters, digits and inner hyphens, 1 to 63 bytes
/// each, 253 in all) whose last label is not all digits, with no scheme,
/// port, path, trailing dot or wildcard. `localhost`, `*.localhost` (the
/// artifact origins) and IP addresses are refused: the API's own rule
/// already decides those.
pub fn trusted_host_ok(name: &str) -> bool {
    let label_ok = |l: &str| {
        (1..=63).contains(&l.len())
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    let lower = name.to_ascii_lowercase();
    name.len() <= 253
        && name.split('.').all(label_ok)
        && !name
            .rsplit('.')
            .next()
            .is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
        && lower != "localhost"
        && !lower.ends_with(".localhost")
}

/// Whether `url` may receive the API key: `https://` with a host, or
/// `http://` whose host is `localhost`, an IPv4 loopback address, or `[::1]`.
/// A URL with user information (`user@host`) is refused.
pub fn base_url_ok(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let (https, rest) = if let Some(r) = lower.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = lower.strip_prefix("http://") {
        (false, r)
    } else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        match v6.split_once(']') {
            Some((h, tail)) if tail.is_empty() || tail.starts_with(':') => h,
            _ => return false,
        }
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return false;
    }
    if https {
        return true;
    }
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The host of `url` as [`base_url_ok`] reads it (lowercase, an IPv6
/// literal without its brackets), or `None` for a URL it refuses.
fn base_url_host(url: &str) -> Option<String> {
    if !base_url_ok(url) {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => authority.split(':').next().unwrap_or(""),
    };
    Some(host.to_string())
}

/// The origin a model key is bound to for `url` (spec §8.1): its scheme,
/// host and port, lowercase, the port left out when it is the scheme's
/// default; `None` for a URL [`base_url_ok`] refuses.
pub fn key_host(url: &str) -> Option<String> {
    let host = base_url_host(url)?;
    let lower = url.to_ascii_lowercase();
    let (scheme, rest, default) = match lower.strip_prefix("https://") {
        Some(r) => ("https", r, "443"),
        None => ("http", lower.strip_prefix("http://")?, "80"),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let port = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once("]:").map(|(_, p)| p),
        None => authority.split_once(':').map(|(_, p)| p),
    }
    .filter(|p| !p.is_empty() && *p != default);
    let host = host.trim_end_matches('.');
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    Some(match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Name suffixes that never name a public host: a hosted Clax refuses to
/// send a model key to them.
const PRIVATE_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".internal",
    ".lan",
    ".home.arpa",
    ".localdomain",
    ".intranet",
    ".corp",
];

/// Whether a hosted Clax (anyone's server, or the Worker) may send a model
/// key to `url`: [`base_url_ok`], `https://` only, and its host a public
/// DNS name: no IP literal of any kind (loopback, private, link-local and
/// unique-local addresses among them, and the instance's own metadata
/// endpoints), not `localhost`, not a single label, and not under a suffix
/// that names a private network ([`PRIVATE_SUFFIXES`]). A name can still
/// resolve to a private address; the platform's own egress rules are the
/// last word there. With `loopback_for_tests`, `http://` on a loopback
/// host is taken as on a local daemon, for a test's scripted provider; it
/// is never a setting an admin can change.
pub fn hosted_base_url_ok(url: &str, loopback_for_tests: bool) -> bool {
    let Some(host) = base_url_host(url) else {
        return false;
    };
    let https = url
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"));
    if !https {
        return loopback_for_tests;
    }
    let host = host.trim_end_matches('.');
    if host.parse::<std::net::IpAddr>().is_ok() || host.contains(':') {
        return false;
    }
    // Every label a name may have, and at least two of them.
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2
        || labels
            .iter()
            .any(|l| l.is_empty() || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    {
        return false;
    }
    // A last label of digits, or a hex label, is an IPv4 address in
    // another spelling (`127.1`, `0x7f.0x0.0x0.0x1`).
    if labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
        || labels.iter().any(|l| l.starts_with("0x"))
    {
        return false;
    }
    host != "localhost" && !PRIVATE_SUFFIXES.iter().any(|s| host.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_bound_to_its_urls_scheme_host_and_port() {
        for (url, want) in [
            ("https://api.anthropic.com", "https://api.anthropic.com"),
            (
                "https://API.Anthropic.com:443/v1/",
                "https://api.anthropic.com",
            ),
            (
                "https://proxy.example.com:8443/x",
                "https://proxy.example.com:8443",
            ),
            ("http://127.0.0.1:9000", "http://127.0.0.1:9000"),
            ("http://localhost:80", "http://localhost"),
            ("http://[::1]:8080/", "http://[::1]:8080"),
            ("https://api.anthropic.com./", "https://api.anthropic.com"),
        ] {
            assert_eq!(key_host(url).as_deref(), Some(want), "{url}");
        }
        assert_ne!(
            key_host("https://a.example.com"),
            key_host("https://b.example.com")
        );
        assert_ne!(
            key_host("https://a.example.com"),
            key_host("https://a.example.com:8443")
        );
        assert_eq!(key_host("ftp://x"), None);
        assert_eq!(key_host("http://evil.example"), None);
    }

    #[test]
    fn a_hosted_clax_sends_a_key_only_to_a_public_https_host() {
        for ok in [
            "https://api.anthropic.com",
            "https://API.Anthropic.com/v1",
            "https://proxy.example.com:8443/x",
            "https://gateway.ai.cloudflare.com/v1/acct/gw/anthropic",
        ] {
            assert!(hosted_base_url_ok(ok, false), "{ok}");
        }
        for bad in [
            "http://api.anthropic.com",
            "http://127.0.0.1:9000",
            "http://localhost:1234",
            "https://127.0.0.1",
            "https://10.0.0.5",
            "https://192.168.1.1:8443",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]",
            "https://[fd00::1]",
            "https://[fe80::1]",
            "https://[::ffff:127.0.0.1]",
            "https://localhost",
            "https://localhost.",
            "https://api.localhost",
            "https://printer.local",
            "https://metadata.google.internal",
            "https://nas.lan",
            "https://router.home.arpa",
            "https://intranet",
            "https://127.1",
            "https://0x7f.0x0.0x0.0x1",
            "https://a..b",
            "https://user@api.anthropic.com",
            "",
        ] {
            assert!(!hosted_base_url_ok(bad, false), "{bad}");
        }
        // The test escape takes loopback http, as a local daemon does, and
        // nothing more.
        assert!(hosted_base_url_ok("http://127.0.0.1:9000", true));
        assert!(!hosted_base_url_ok("http://10.0.0.5:9000", true));
        assert!(!hosted_base_url_ok("https://10.0.0.5", true));
    }

    #[test]
    fn terminal_after_s_defaults_clamps_and_ignores_mistyped_values() {
        assert_eq!(with("").questions_terminal_after_s(), 600);
        assert_eq!(with("[questions]\n").questions_terminal_after_s(), 600);
        assert_eq!(
            with("[questions]\nterminal_after_s = 0\n").questions_terminal_after_s(),
            0
        );
        assert_eq!(
            with("[questions]\nterminal_after_s = 90\n").questions_terminal_after_s(),
            90
        );
        assert_eq!(
            with("[questions]\nterminal_after_s = -5\n").questions_terminal_after_s(),
            0
        );
        assert_eq!(
            with("[questions]\nterminal_after_s = 9999\n").questions_terminal_after_s(),
            3300
        );
        assert_eq!(
            with("[questions]\nterminal_after_s = \"x\"\n").questions_terminal_after_s(),
            600
        );
        assert_eq!(with("questions = 3\n").questions_terminal_after_s(), 600);
    }

    fn with(text: &str) -> HomeConfig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), text).unwrap();
        HomeConfig::load_with_env(dir.path(), []).unwrap()
    }

    /// `text` as the file at `/h/config.toml`, under `env`.
    fn under(text: &str, env: &[(&str, &str)]) -> HomeConfig {
        HomeConfig::from_text(
            PathBuf::from("/h/config.toml"),
            text,
            env.iter().map(|(k, v)| (k.to_string(), v.to_string())),
        )
        .unwrap()
    }

    #[test]
    fn the_environment_overrides_the_file_which_overrides_the_defaults() {
        let file = "[serve]\nport = 7481\n[sample]\nmax_tokens = 9\n";
        let c = under(file, &[]);
        assert_eq!(c.serve_port().unwrap(), Some(7481));
        assert_eq!(c.source("serve.port"), Source::File);
        assert_eq!(c.source("sample.provider"), Source::Default);
        let c = under(
            file,
            &[
                ("CLAX_SERVE__PORT", "7490"),
                ("CLAX_SAMPLE__MODELS__QUICK", "q-1"),
                ("CLAX_BOT__ENABLED", "false"),
            ],
        );
        assert_eq!(c.serve_port().unwrap(), Some(7490));
        assert_eq!(
            c.source("serve.port"),
            Source::Env("CLAX_SERVE__PORT".into())
        );
        assert_eq!(c.source("serve.port").label(), "env:CLAX_SERVE__PORT");
        let s = c.sample().unwrap();
        assert_eq!((s.max_tokens, s.models.quick.as_str()), (9, "q-1"));
        assert_eq!(s.models.default, "claude-sonnet-5-5");
        assert!(!c.bot().unwrap().enabled);
        // The default port is no setting: the daemon falls back to 7480.
        let c = under("", &[]);
        assert_eq!(c.serve_port().unwrap(), None);
        assert_eq!(c.value("serve.port").as_ref().and_then(as_int), Some(7480));
        assert_eq!(c.source("serve.port"), Source::Default);
    }

    #[test]
    fn only_nested_capital_clax_variables_are_settings() {
        for name in [
            "CLAX_PORT",
            "CLAX_HOME",
            "CLAX_BIN",
            "CLAX_TOKEN",
            "CLAX_SETUP_SECRET",
            "clax_serve__port",
            "CLAX_serve__port",
            "CLAX_SERVE___PORT",
            "CLAX__SERVE",
            "CLAX_SERVE__",
            "OTHER_SERVE__PORT",
        ] {
            assert_eq!(env_key(name), None, "{name}");
        }
        assert_eq!(
            env_key("CLAX_BOT__MODELS__DEFAULT"),
            Some(vec!["bot".into(), "models".into(), "default".into()])
        );
        assert_eq!(env_name("bot.script.cpu_ms"), "CLAX_BOT__SCRIPT__CPU_MS");
        // CLAX_PORT and CLAX_BIN keep their own meaning, and an empty value is unset.
        let c = under(
            "[serve]\nport = 7481\n",
            &[
                ("CLAX_PORT", "1"),
                ("CLAX_BIN", "/x"),
                ("CLAX_SERVE__PORT", ""),
            ],
        );
        assert_eq!(c.serve_port().unwrap(), Some(7481));
        assert_eq!(c.bin().unwrap(), None);
    }

    #[test]
    fn an_environment_value_takes_its_settings_type() {
        let c = under(
            "",
            &[
                ("CLAX_SAMPLE__STUB_IMAGES", "true"),
                ("CLAX_SAMPLE__DAILY_CALL_CAP", "3"),
                ("CLAX_SAMPLE__MODELS__QUICK", "claude-haiku-4-5"),
                ("CLAX_SAMPLE__MODELS__DEFAULT", "123"),
                ("CLAX_BOT__MODELS__COMPLEX", "1.0"),
                ("CLAX_BOT__API_KEY_ENV", "true"),
                ("CLAX_BOT__MAX_STEPS", "5"),
            ],
        );
        let s = c.sample().unwrap();
        assert!(s.stub_images);
        assert_eq!(s.daily_call_cap, Some(3));
        assert_eq!(s.models.quick, "claude-haiku-4-5");
        // A string setting takes the text verbatim, however numeric it looks.
        assert_eq!(s.models.default, "123");
        let b = c.bot().unwrap();
        assert_eq!(
            (
                b.models.complex.as_str(),
                b.api_key_env.as_str(),
                b.max_steps
            ),
            ("1.0", "true", 5)
        );
        // An integer setting given text that is no integer is refused, naming the variable.
        let e = under("", &[("CLAX_BOT__MAX_STEPS", "five")])
            .bot()
            .unwrap_err()
            .to_string();
        assert!(e.contains("CLAX_BOT__MAX_STEPS"), "{e}");
    }

    #[test]
    fn hosted_is_never_set_by_the_environment() {
        let c = under(
            "",
            &[
                ("CLAX_HOSTED__PUBLIC_ORIGIN", "https://clax.example"),
                ("CLAX_HOSTED__LEGAL_VERSION", "1.0"),
            ],
        );
        assert_eq!(c.hosted().unwrap(), None);
        assert_eq!(
            c.ignored_env(),
            ["CLAX_HOSTED__LEGAL_VERSION", "CLAX_HOSTED__PUBLIC_ORIGIN"]
        );
        // Nor broken by it: the file's table stands.
        let c = under(
            "[hosted]\npublic_origin = \"https://clax.example\"\n",
            &[("CLAX_HOSTED__NOPE", "1")],
        );
        assert_eq!(
            c.hosted().unwrap().unwrap().public_origin,
            "https://clax.example"
        );
    }

    #[test]
    fn an_environment_that_is_not_utf8_is_skipped_not_a_panic() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let vars = vec![
            (OsString::from("FOO"), OsString::from_vec(vec![0xff])),
            (OsString::from_vec(vec![b'C', 0xfe]), OsString::from("x")),
            (OsString::from("CLAX_SERVE__PORT"), OsString::from("7490")),
        ];
        let kept = utf8_env(vars);
        assert_eq!(kept, [("CLAX_SERVE__PORT".to_string(), "7490".to_string())]);
        let dir = tempfile::tempdir().unwrap();
        let c = HomeConfig::load_with_env(dir.path(), kept).unwrap();
        assert_eq!(c.serve_port().unwrap(), Some(7490));
        // The process's own environment reads without a panic, whatever it holds.
        let _ = process_env();
    }

    #[test]
    fn a_bad_value_from_the_environment_names_the_variable_and_turns_off_only_its_table() {
        for (var, val, table) in [
            ("CLAX_SAMPLE__MAX_TOKENS", "0", "[sample]"),
            ("CLAX_SAMPLE__MAX_TOKENS", "many", "[sample]"),
            ("CLAX_BOT__BASE_URL", "http://example.com", "[bot]"),
            ("CLAX_TOOLPATH__SEGMENT_MAX_MB", "0", "[toolpath]"),
            ("CLAX_SERVE__PORT", "0", "[serve]"),
        ] {
            let c = under("[serve]\nport = 7481\n", &[(var, val)]);
            let e = match table {
                "[sample]" => c.sample().map(drop),
                "[bot]" => c.bot().map(drop),
                "[toolpath]" => c.toolpath().map(drop),
                _ => c.serve_port().map(drop),
            }
            .unwrap_err()
            .to_string();
            assert!(e.contains(var) && e.contains(table), "{var}={val}: {e}");
            if table != "[serve]" {
                assert_eq!(c.serve_port().unwrap(), Some(7481), "{var}");
            }
        }
    }

    #[test]
    fn bot_defaults_and_overrides() {
        let b = under("", &[]).bot().unwrap();
        assert_eq!(b, BotConfig::default());
        assert!(b.enabled);
        assert_eq!(
            (b.api_key_env.as_str(), b.base_url.as_str()),
            ("ANTHROPIC_API_KEY", "https://api.anthropic.com")
        );
        assert_eq!(
            (
                b.max_tokens,
                b.max_steps,
                b.max_run_credits,
                b.max_run_minutes,
                b.concurrency,
                b.per_person_concurrency
            ),
            (32000, 40, 200, 30, 4, 1)
        );
        assert_eq!(
            (b.free_weekly_credits, b.credit_price_micro_usd),
            (2000, 10_000)
        );
        assert_eq!(
            (
                b.models.default.as_str(),
                b.models.quick.as_str(),
                b.models.complex.as_str()
            ),
            ("claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-5-5")
        );
        assert_eq!(
            (b.script.cpu_ms, b.script.memory_mib, b.script.max_calls),
            (2000, 64, 200)
        );
        let b = under(
            "[bot]\nmax_steps = 5\nbase_url = \"http://127.0.0.1:9\"\n[bot.script]\ncpu_ms = 10\n",
            &[("CLAX_BOT__MODELS__DEFAULT", "m")],
        )
        .bot()
        .unwrap();
        assert_eq!(
            (b.max_steps, b.script.cpu_ms, b.models.default.as_str()),
            (5, 10, "m")
        );
        assert_eq!(b.base_url, "http://127.0.0.1:9");
    }

    #[test]
    fn a_bad_bot_table_is_bad_config_naming_the_file_and_table() {
        for t in [
            "bot = 1\n",
            "[bot]\nunknown = 1\n",
            "[bot]\nenabled = \"yes\"\n",
            "[bot]\napi_key_env = \" \"\n",
            "[bot]\nbase_url = \"http://api.anthropic.com\"\n",
            "[bot]\nmax_tokens = 0\n",
            "[bot]\nmax_steps = 0\n",
            "[bot]\nconcurrency = 65\n",
            "[bot]\nper_person_concurrency = 0\n",
            "[bot]\ncredit_price_micro_usd = 0\n",
            "[bot]\nmax_run_minutes = -1\n",
            "[bot.models]\nquick = \"\"\n",
            "[bot.models]\nhuge = \"x\"\n",
            "[bot.script]\nmemory_mib = 0\n",
        ] {
            let c = with(&format!("{t}[serve]\nport = 7481\n"));
            let e = c.bot().unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "bad_config",
                        ..
                    }
                ),
                "{t}: {e}"
            );
            let m = e.to_string();
            assert!(m.contains("config.toml") && m.contains("[bot]"), "{t}: {m}");
            assert_eq!(c.serve_port().unwrap(), Some(7481), "{t}");
            assert!(c.sample().is_ok(), "{t}");
        }
    }

    #[test]
    fn hosted_legal_is_bundled_or_a_deployments_own_or_none() {
        let legal = |extra: &str| {
            with(&format!(
                "[hosted]\npublic_origin = \"https://clax.example\"\n{extra}"
            ))
            .hosted()
            .map(|h| h.unwrap().legal)
        };
        assert_eq!(legal("").unwrap(), None);
        let b = legal("legal = \"bundled\"\n").unwrap().unwrap();
        assert_eq!(
            (b.terms_url(), b.privacy_url(), b.version()),
            ("/terms", "/privacy", BUNDLED_LEGAL_VERSION)
        );
        let own = legal(
            "terms_url = \"https://example.org/terms\"\nprivacy_url = \"https://example.org/privacy\"\nlegal_version = \"3\"\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (own.terms_url(), own.privacy_url(), own.version()),
            (
                "https://example.org/terms",
                "https://example.org/privacy",
                "3"
            )
        );
        for bad in [
            "legal = \"yes\"\n",
            "legal = \"bundled\"\nlegal_version = \"2\"\n",
            "terms_url = \"https://example.org/terms\"\n",
            "terms_url = \"http://example.org/t\"\nprivacy_url = \"https://example.org/p\"\nlegal_version = \"1\"\n",
            "terms_url = \"/terms\"\nprivacy_url = \"https://example.org/p\"\nlegal_version = \"1\"\n",
        ] {
            let e = legal(bad).unwrap_err().to_string();
            assert!(e.contains("hosted"), "{bad}: {e}");
        }
    }

    #[test]
    fn hosted_app_links_and_stores_are_checked_and_optional() {
        let app = |extra: &str| {
            with(&format!(
                "[hosted]\npublic_origin = \"https://clax.example\"\n{extra}"
            ))
            .hosted()
            .map(|h| h.unwrap().clax_app)
        };
        assert_eq!(app("").unwrap(), ClaxApp::default());
        let fp = ["ab"; 32].join(":");
        let a = app(&format!(
            "app_links = {{apple_team_id = \"ABCDE12345\", android_package = \"dev.empathic.clax\", android_sha256 = \"{fp}\"}}\nstores = {{ios = \"https://apps.apple.com/app/id1\", android = \"https://play.google.com/store/apps/details?id=dev.empathic.clax\"}}\n"
        ))
        .unwrap();
        assert_eq!(a.apple_team_id.as_deref(), Some("ABCDE12345"));
        assert_eq!(
            a.android,
            Some(AndroidApp {
                package: "dev.empathic.clax".into(),
                sha256: fp.to_ascii_uppercase(),
            })
        );
        assert_eq!(
            a.ios_store.as_deref(),
            Some("https://apps.apple.com/app/id1")
        );
        let team_only = app("app_links = {apple_team_id = \"ABCDE12345\"}\n").unwrap();
        assert_eq!(team_only.android, None);
        for bad in [
            "app_links = {apple_team_id = \"abcde12345\"}\n".to_string(),
            "app_links = {apple_team_id = \"ABCDE1234\"}\n".to_string(),
            "app_links = {android_package = \"dev.empathic.clax\"}\n".to_string(),
            format!("app_links = {{android_sha256 = \"{fp}\"}}\n"),
            format!("app_links = {{android_package = \"clax\", android_sha256 = \"{fp}\"}}\n"),
            "app_links = {android_package = \"dev.empathic.clax\", android_sha256 = \"AB:CD\"}\n"
                .to_string(),
            "app_links = {team = \"ABCDE12345\"}\n".to_string(),
            "stores = {ios = \"http://apps.apple.com/x\"}\n".to_string(),
            "stores = {web = \"https://example.org\"}\n".to_string(),
        ] {
            let e = app(&bad).unwrap_err().to_string();
            assert!(e.contains("hosted"), "{bad}: {e}");
        }
    }

    #[test]
    fn hosted_is_off_when_absent_and_its_origin_is_checked() {
        assert_eq!(with("").hosted().unwrap(), None);
        let h = with("[hosted]\npublic_origin = \"https://Clax.Example:8443/\"\n")
            .hosted()
            .unwrap()
            .unwrap();
        assert_eq!(h.public_origin, "https://clax.example:8443");
        assert_eq!(h.setup_secret_env, SETUP_SECRET_ENV);
        assert!(!h.behind_cloudflare);
        let h =
            with("[hosted]\npublic_origin = \"https://clax.example\"\nbehind_cloudflare = true\n")
                .hosted()
                .unwrap()
                .unwrap();
        assert!(h.behind_cloudflare);
        assert!(
            with(
                "[hosted]\npublic_origin = \"https://clax.example\"\nbehind_cloudflare = \"yes\"\n"
            )
            .hosted()
            .is_err()
        );
        let h = with(
            "[hosted]\npublic_origin = \"http://localhost:7490\"\nsetup_secret_env = \"MY_SECRET\"\n",
        )
        .hosted()
        .unwrap()
        .unwrap();
        assert_eq!(
            (h.public_origin.as_str(), h.setup_secret_env.as_str()),
            ("http://localhost:7490", "MY_SECRET")
        );
        for bad in [
            "hosted = 1\n",
            "[hosted]\n",
            "[hosted]\npublic_origin = \"http://clax.example\"\n",
            "[hosted]\npublic_origin = \"https://clax.example/app\"\n",
            "[hosted]\npublic_origin = \"https://u:p@clax.example\"\n",
            "[hosted]\npublic_origin = \"https://clax.example?x=1\"\n",
            "[hosted]\npublic_origin = \"clax.example\"\n",
            "[hosted]\npublic_origin = \"https://clax.example\"\nsetup_secret_env = \" \"\n",
            "[hosted]\npublic_origin = \"https://clax.example\"\nport = 1\n",
        ] {
            let e = with(bad).hosted().unwrap_err().to_string();
            assert!(e.contains("[hosted]") || e.contains("hosted"), "{bad}: {e}");
        }
        assert_eq!(
            hosted_origin("http://127.0.0.1:9000").as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert_eq!(
            hosted_origin("http://[::1]:9000").as_deref(),
            Some("http://[::1]:9000")
        );
    }

    #[test]
    fn hosted_site_domain_is_a_bare_domain_name() {
        let site = |extra: &str| {
            with(&format!(
                "[hosted]\npublic_origin = \"https://clax.example\"\n{extra}"
            ))
            .hosted()
        };
        assert_eq!(site("").unwrap().unwrap().site_domain, None);
        assert_eq!(
            site("site_domain = \"Clax.Run\"\n")
                .unwrap()
                .unwrap()
                .site_domain
                .as_deref(),
            Some("clax.run")
        );
        for bad in [
            "",
            "https://clax.run",
            "clax.run:443",
            ".clax.run",
            "clax.run/",
            "clax..run",
            &format!("{}.run", "a".repeat(64)),
            "-clax.run",
            "clax-.run",
            "clax.run-",
            "127.0.0.1",
            "10.0.0.1",
        ] {
            let e = site(&format!("site_domain = \"{bad}\"\n"))
                .unwrap_err()
                .to_string();
            assert!(e.contains("site_domain"), "{bad}: {e}");
        }
        let longest = format!("{}.run", "a".repeat(63));
        assert_eq!(
            site(&format!("site_domain = \"{longest}\"\n"))
                .unwrap()
                .unwrap()
                .site_domain,
            Some(longest)
        );
        assert!(site("site_domain = \"my-clax.run\"\n").is_ok());
    }

    #[test]
    fn the_site_domain_rule_is_one_function_for_every_host() {
        assert_eq!(
            super::hosted_site_domain("Clax.Run", "https://clax.dev"),
            Ok("clax.run".to_string())
        );
        assert_eq!(
            super::hosted_site_domain("localhost", "http://localhost:8787"),
            Ok("localhost".to_string())
        );
        assert_eq!(
            super::hosted_site_domain("https://clax.run", "https://clax.dev"),
            Err(super::SiteDomainError::NotBare)
        );
        for shared in ["clax.dev", "dev", "sites.clax.dev"] {
            assert_eq!(
                super::hosted_site_domain(shared, "https://clax.dev"),
                Err(super::SiteDomainError::SharesPublicHost {
                    domain: shared.to_string(),
                    public_host: "clax.dev".to_string(),
                }),
                "{shared}"
            );
        }
    }

    #[test]
    fn hosted_site_domain_shares_nothing_with_the_public_host() {
        let site = |domain: &str| {
            with(&format!(
                "[hosted]\npublic_origin = \"https://clax.example.com:8443\"\nsite_domain = \"{domain}\"\n"
            ))
            .hosted()
        };
        for bad in [
            "clax.example.com",
            "CLAX.example.com",
            "example.com",
            "x.clax.example.com",
        ] {
            let e = site(bad).unwrap_err().to_string();
            assert!(
                e.contains("site_domain") && e.contains("public_origin"),
                "{bad}: {e}"
            );
        }
        for good in ["clax.run", "ample.com", "example.co"] {
            assert!(site(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn a_loopback_public_origin_may_put_its_sites_under_localhost() {
        let site = |origin: &str, domain: &str| {
            with(&format!(
                "[hosted]\npublic_origin = \"{origin}\"\nsite_domain = \"{domain}\"\n"
            ))
            .hosted()
        };
        // `<id>.localhost` names this machine alone: no registrable domain
        // is shared with anyone.
        assert_eq!(
            site("http://localhost:7490", "localhost")
                .unwrap()
                .unwrap()
                .site_domain
                .as_deref(),
            Some("localhost")
        );
        assert!(site("http://127.0.0.1:7490", "localhost").is_ok());
        // Only `localhost` itself: a name under it still nests with the
        // public host.
        let e = site("http://localhost:7490", "x.localhost")
            .unwrap_err()
            .to_string();
        assert!(e.contains("site_domain"), "{e}");
    }

    #[test]
    fn a_missing_file_has_no_port() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            HomeConfig::load(dir.path()).unwrap().serve_port().unwrap(),
            None
        );
    }

    #[test]
    fn serve_port_is_read_and_other_tables_are_ignored() {
        assert_eq!(
            with("[sample]\napi_key_env = \"K\"\n\n[serve]\nport = 7481\n")
                .serve_port()
                .unwrap(),
            Some(7481)
        );
        assert_eq!(with("[sample]\nx = 1\n").serve_port().unwrap(), None);
        assert_eq!(with("[serve]\nother = 1\n").serve_port().unwrap(), None);
    }

    #[test]
    fn out_of_range_or_mistyped_ports_are_errors_naming_the_file_and_key() {
        for t in [
            "[serve]\nport = 0\n",
            "[serve]\nport = 74810\n",
            "[serve]\nport = -1\n",
            "[serve]\nport = \"7481\"\n",
            "[serve]\nport = 7481.0\n",
            "serve = 7481\n",
        ] {
            let e = with(t).serve_port().unwrap_err().to_string();
            assert!(
                e.contains("config.toml") && e.contains("[serve]"),
                "{t}: {e}"
            );
        }
    }

    #[test]
    fn trusted_hosts_are_empty_by_default_and_read_lowercased_once_each() {
        for t in [
            "",
            "[serve]\nport = 7481\n",
            "[serve]\ntrusted_hosts = []\n",
        ] {
            assert!(with(t).serve_trusted_hosts().unwrap().is_empty(), "{t}");
        }
        let c = with(
            "[serve]\nport = 7480\ntrusted_hosts = [\"Phobos.tail556f5b.ts.net\", \"phobos\", \"phobos.tail556f5b.ts.net\"]\n",
        );
        assert_eq!(
            c.serve_trusted_hosts().unwrap(),
            ["phobos.tail556f5b.ts.net", "phobos"]
        );
        assert_eq!(c.serve_port().unwrap(), Some(7480));
        // The environment sets it as it does any other key.
        let c = under(
            "",
            &[(
                "CLAX_SERVE__TRUSTED_HOSTS",
                "[\"phobos.tail556f5b.ts.net\"]",
            )],
        );
        assert_eq!(
            c.serve_trusted_hosts().unwrap(),
            ["phobos.tail556f5b.ts.net"]
        );
    }

    #[test]
    fn the_owner_tailscale_login_is_exact_and_needs_tailscale_identity() {
        const ON: &str =
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\ntailscale_identity = true\n";
        for t in ["", "[serve]\nport = 7481\n", ON] {
            assert_eq!(with(t).serve_owner_tailscale_login().unwrap(), None, "{t}");
        }
        assert_eq!(
            with(&format!(
                "{ON}owner_tailscale_login = \"bdelanghe@github\"\n"
            ))
            .serve_owner_tailscale_login()
            .unwrap()
            .as_deref(),
            Some("bdelanghe@github")
        );
        for t in [
            "[serve]\nowner_tailscale_login = \"bdelanghe@github\"\n".to_string(),
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\nowner_tailscale_login = \"bdelanghe@github\"\n".to_string(),
            format!("{ON}owner_tailscale_login = \"\"\n"),
            format!("{ON}owner_tailscale_login = \"*\"\n"),
            format!("{ON}owner_tailscale_login = \"*@github\"\n"),
            format!("{ON}owner_tailscale_login = \"a b\"\n"),
            format!("{ON}owner_tailscale_login = 1\n"),
            format!("{ON}owner_tailscale_login = [\"bdelanghe@github\"]\n"),
        ] {
            let e = with(&t).serve_owner_tailscale_login().unwrap_err().to_string();
            assert!(e.contains("config.toml") && e.contains("[serve]"), "{t}: {e}");
        }
    }

    #[test]
    fn trusted_host_frames_are_off_by_default_and_need_trusted_hosts() {
        for t in [
            "",
            "[serve]\nport = 7481\n",
            "[serve]\ntrusted_host_frames = false\n",
        ] {
            assert!(!with(t).serve_trusted_host_frames().unwrap(), "{t}");
        }
        assert!(
            with("[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\ntrusted_host_frames = true\n")
                .serve_trusted_host_frames()
                .unwrap()
        );
        for t in [
            "[serve]\ntrusted_host_frames = true\n",
            "[serve]\ntrusted_hosts = []\ntrusted_host_frames = true\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\ntrusted_host_frames = \"yes\"\n",
        ] {
            let e = with(t).serve_trusted_host_frames().unwrap_err().to_string();
            assert!(
                e.contains("config.toml") && e.contains("[serve]"),
                "{t}: {e}"
            );
        }
    }

    #[test]
    fn tailscale_identity_is_off_by_default_and_needs_trusted_hosts() {
        for t in [
            "",
            "[serve]\nport = 7481\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\n",
            "[serve]\ntailscale_identity = false\n",
        ] {
            assert!(!with(t).serve_tailscale_identity().unwrap(), "{t}");
        }
        let on = with(
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\ntailscale_identity = true\n",
        );
        assert!(on.serve_tailscale_identity().unwrap());
        for t in [
            "[serve]\ntailscale_identity = true\n",
            "[serve]\ntrusted_hosts = []\ntailscale_identity = true\n",
            "[serve]\ntrusted_hosts = [\"*\"]\ntailscale_identity = true\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\ntailscale_identity = \"yes\"\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net\"]\ntailscale_identity = 1\n",
            "serve = 1\n",
        ] {
            let e = with(t).serve_tailscale_identity().unwrap_err().to_string();
            assert!(
                e.contains("config.toml") && e.contains("[serve]"),
                "{t}: {e}"
            );
        }
    }

    #[test]
    fn a_bad_trusted_host_is_an_error_naming_the_file_and_key() {
        for t in [
            "[serve]\ntrusted_hosts = \"phobos.tail556f5b.ts.net\"\n",
            "[serve]\ntrusted_hosts = [1]\n",
            "[serve]\ntrusted_hosts = [\"\"]\n",
            "[serve]\ntrusted_hosts = [\"*\"]\n",
            "[serve]\ntrusted_hosts = [\"*.tail556f5b.ts.net\"]\n",
            "[serve]\ntrusted_hosts = [\"https://phobos.tail556f5b.ts.net\"]\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net:443\"]\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net/\"]\n",
            "[serve]\ntrusted_hosts = [\"phobos.tail556f5b.ts.net.\"]\n",
            "[serve]\ntrusted_hosts = [\"ok.example\", \"bad host\"]\n",
            "[serve]\ntrusted_hosts = [\"localhost\"]\n",
            "[serve]\ntrusted_hosts = [\"7q3k9mzx2b4t.LOCALHOST\"]\n",
            "[serve]\ntrusted_hosts = [\"100.64.0.1\"]\n",
            "[serve]\ntrusted_hosts = [\"[::1]\"]\n",
            "[serve]\ntrusted_hosts = [\"-a.example\"]\n",
            "serve = 1\n",
        ] {
            let e = with(t).serve_trusted_hosts().unwrap_err().to_string();
            assert!(
                e.contains("config.toml") && e.contains("[serve]"),
                "{t}: {e}"
            );
        }
    }

    #[test]
    fn a_config_that_does_not_parse_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "[serve\n").unwrap();
        let e = HomeConfig::load(dir.path()).unwrap_err().to_string();
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn a_config_that_cannot_be_read_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(FILE)).unwrap();
        let e = HomeConfig::load(dir.path()).unwrap_err().to_string();
        assert!(e.contains("config.toml"), "{e}");
    }
    #[test]
    fn an_absent_sample_table_is_the_default() {
        let s = with("[serve]\nport = 7481\n").sample().unwrap();
        assert_eq!(s, SampleConfig::default());
        assert_eq!(
            (
                s.provider.as_str(),
                s.api_key_env.as_str(),
                s.base_url.as_str()
            ),
            (
                "anthropic",
                "ANTHROPIC_API_KEY",
                "https://api.anthropic.com"
            )
        );
        assert_eq!(
            (
                s.models.quick.as_str(),
                s.models.default.as_str(),
                s.models.complex.as_str()
            ),
            ("claude-haiku-4-5", "claude-sonnet-5-5", "claude-opus-5-5")
        );
        assert_eq!(
            (
                s.max_tokens,
                s.daily_call_cap,
                s.stub_images,
                s.stub_delay_ms
            ),
            (16000, None, false, 40)
        );
        assert_eq!(s.daily_cost_cap_usd, Some(0.5));
    }

    #[test]
    fn sample_keys_override_the_defaults() {
        let s = with(
            "[sample]\nprovider = \"stub\"\ndaily_call_cap = 3\n[sample.models]\nquick = \"q\"\n",
        )
        .sample()
        .unwrap();
        assert_eq!(s.provider, "stub");
        assert_eq!(s.daily_call_cap, Some(3));
        assert_eq!(s.models.quick, "q");
        assert_eq!(s.models.default, "claude-sonnet-5-5");
        let c = with("[sample]\ndaily_cost_cap_usd = 2\n").sample().unwrap();
        assert_eq!(c.daily_cost_cap_usd, Some(2.0));
    }

    #[test]
    fn a_bad_sample_table_is_bad_config_naming_the_file_and_table_and_the_port_still_reads() {
        for t in [
            "[sample]\nprovider = \"openai\"\n",
            "[sample]\nunknown = 1\n",
            "[sample]\nmax_tokens = 0\n",
            "[sample]\ndaily_call_cap = -1\n",
            "[sample]\ndaily_cost_cap_usd = -0.5\n",
            "[sample.models]\nquick = \"\"\n",
            "[sample.models]\nhuge = \"x\"\n",
            "[sample]\nbase_url = \"http://api.anthropic.com\"\n",
            "sample = 3\n",
        ] {
            let c = with(&format!("{t}[serve]\nport = 7481\n"));
            let e = c.sample().unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "bad_config",
                        ..
                    }
                ),
                "{t}: {e}"
            );
            let m = e.to_string();
            assert!(
                m.contains("config.toml") && m.contains("[sample]"),
                "{t}: {m}"
            );
            assert_eq!(c.serve_port().unwrap(), Some(7481), "{t}");
        }
    }

    #[test]
    fn bin_is_read_beside_the_tables_and_checked() {
        let c = with("bin = \"/opt/clax/bin/clax\"\n\n[serve]\nport = 7481\n");
        assert_eq!(c.bin().unwrap(), Some("/opt/clax/bin/clax".into()));
        assert_eq!(c.serve_port().unwrap(), Some(7481));
        assert_eq!(with("[serve]\nport = 7481\n").bin().unwrap(), None);
        for t in [
            "bin = \"relative/clax\"\n",
            "bin = 3\n",
            "bin = \"/a\\\\b\"\n",
            "bin = \"/a\\\"b\"\n",
            "bin = \"/a\\tb\"\n",
        ] {
            let e = with(t).bin().unwrap_err().to_string();
            assert!(e.contains("config.toml") && e.contains("bin"), "{t}: {e}");
        }
    }

    #[test]
    fn a_bin_line_is_toml_whose_value_is_the_path() {
        for p in [
            "/usr/local/bin/clax",
            "/Users/a b/é/clax",
            "/x/'quoted'/clax",
        ] {
            assert!(bin_path_ok(p), "{p}");
            let t: toml::Table = bin_line(p).parse().unwrap();
            assert_eq!(t["bin"].as_str(), Some(p));
        }
        for p in ["clax", "", "/a\"b", "/a\\b", "/a\nb", "/a\tb"] {
            assert!(!bin_path_ok(p), "{p:?}");
        }
    }

    #[test]
    fn base_url_is_https_or_loopback_http() {
        for ok in [
            "https://api.anthropic.com",
            "https://proxy.example:8443/x",
            "http://127.0.0.1:9000",
            "http://localhost:1234/",
            "HTTP://LOCALHOST",
            "http://[::1]:8080",
        ] {
            assert!(base_url_ok(ok), "{ok}");
        }
        for bad in [
            "http://api.anthropic.com",
            "http://192.168.1.5:8080",
            "http://localhost.evil.example",
            "http://127.0.0.1@evil.example",
            "https://",
            "ftp://x",
            "api.anthropic.com",
            "http://[::2]",
            "",
        ] {
            assert!(!base_url_ok(bad), "{bad}");
        }
        assert!(
            with("[sample]\nbase_url = \"http://127.0.0.1:9\"\n")
                .sample()
                .is_ok()
        );
    }

    #[test]
    fn gallery_quickstart_defaults_on_and_reads_env() {
        let dir = tempfile::tempdir().unwrap();
        // The process environment is not read: the test runs under
        // CLAX_GALLERY__QUICKSTART=false.
        assert!(
            HomeConfig::load_with_env(dir.path(), [])
                .unwrap()
                .gallery_quickstart()
        );
        assert!(!with("[gallery]\nquickstart = false\n").gallery_quickstart());
        assert!(with("[gallery]\nquickstart = \"no\"\n").gallery_quickstart());
        let c = HomeConfig::load_with_env(
            dir.path(),
            [("CLAX_GALLERY__QUICKSTART".to_string(), "false".to_string())],
        )
        .unwrap();
        assert!(!c.gallery_quickstart());
    }

    #[test]
    fn toolpath_keys_default_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            HomeConfig::load(dir.path()).unwrap().toolpath().unwrap(),
            ToolpathConfig::default()
        );
        let d = ToolpathConfig::default();
        assert!(d.journal && d.journal_text);
        assert_eq!((d.segment_max_mb, d.journal_retain_days), (64, 0));
        assert_eq!(
            with("[toolpath]\njournal = false\njournal_text = false\nsegment_max_mb = 8\njournal_retain_days = 30\n")
                .toolpath()
                .unwrap(),
            ToolpathConfig {
                journal: false,
                journal_text: false,
                segment_max_mb: 8,
                journal_retain_days: 30,
            }
        );
        for bad in [
            "toolpath = 1\n",
            "[toolpath]\njournal = \"no\"\n",
            "[toolpath]\nother = 1\n",
            "[toolpath]\nsegment_max_mb = 0\n",
            "[toolpath]\njournal_retain_days = -1\n",
        ] {
            let e = with(bad).toolpath().unwrap_err().to_string();
            assert!(e.contains("[toolpath]"), "{bad}: {e}");
        }
    }
}

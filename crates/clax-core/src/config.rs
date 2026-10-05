//! A home's `config.toml`.
//!
//! Read-only here. `[serve] port` is the port a daemon started for the home
//! listens on (7480 when absent); `just dev` and `just watch` write
//! `port = 7481` into `~/.clax-dev/config.toml`. `[sample]` configures the
//! `sample` capability ([`HomeConfig::sample`]); a daemon whose `[sample]` is
//! invalid starts with sample off. The top-level `bin` key names the clax the
//! plugins run ([`HomeConfig::bin`]); `clax bin set` writes it as one line,
//! [`bin_line`], which the plugins' wrapper reads without a TOML parser, and
//! nothing else here reads it. Other tables are reserved (spec §5) and
//! ignored.

use crate::{CoreError, Result};
use serde::Deserialize;
use std::path::Path;

/// The file name inside a home.
pub const FILE: &str = "config.toml";

/// A parsed `config.toml`.
#[derive(Clone, Debug, Default)]
pub struct HomeConfig {
    /// Where it was read from, for error messages.
    path: std::path::PathBuf,
    table: toml::Table,
}

/// Keys `[serve]` understands; any other key there is logged and ignored.
const SERVE_KEYS: &[&str] = &["port"];

/// `[sample]`: the `sample` capability's provider, key variable, models, and cap.
#[derive(Clone, Debug, Deserialize, PartialEq)]
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
            max_tokens: 16000,
            stub_images: false,
            stub_delay_ms: 40,
        }
    }
}

/// `[sample.models]`: the model ID each `modelTier` maps to.
#[derive(Clone, Debug, Deserialize, PartialEq)]
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

impl HomeConfig {
    /// `<home_root>/config.toml`; a missing file is an empty config.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file when it exists but
    /// cannot be read or does not parse.
    pub fn load(home_root: &Path) -> Result<HomeConfig> {
        let path = home_root.join(FILE);
        let bad = |e: &dyn std::fmt::Display| {
            CoreError::invalid("bad_config", format!("{}: {e}", path.display()))
        };
        let table = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse::<toml::Table>().map_err(|e| bad(&e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(e) => return Err(bad(&e)),
        };
        if let Some(serve) = table.get("serve").and_then(|v| v.as_table()) {
            for key in serve.keys().filter(|k| !SERVE_KEYS.contains(&k.as_str())) {
                tracing::warn!("{}: unknown key [serve] {key} ignored", path.display());
            }
        }
        Ok(HomeConfig { path, table })
    }

    /// The top-level `bin` setting, the clax the plugins run (spec §13):
    /// `None` when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and the key when it
    /// is not a string that [`bin_path_ok`] accepts.
    pub fn bin(&self) -> Result<Option<std::path::PathBuf>> {
        let Some(v) = self.table.get("bin") else {
            return Ok(None);
        };
        match v.as_str() {
            Some(s) if bin_path_ok(s) => Ok(Some(s.into())),
            _ => Err(CoreError::invalid(
                "bad_config",
                format!(
                    "{}: bin must be an absolute path with no quote, backslash or control character, not {v}",
                    self.path.display()
                ),
            )),
        }
    }

    /// `[serve] port`: `None` when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and the key when
    /// `[serve]` is not a table, or the port is not an integer in 1..=65535.
    pub fn serve_port(&self) -> Result<Option<u16>> {
        let bad = |what: String| {
            CoreError::invalid("bad_config", format!("{}: {what}", self.path.display()))
        };
        let Some(serve) = self.table.get("serve") else {
            return Ok(None);
        };
        let Some(serve) = serve.as_table() else {
            return Err(bad(format!(
                "[serve] must be a table, not {}",
                serve.type_str()
            )));
        };
        let Some(port) = serve.get("port") else {
            return Ok(None);
        };
        port.as_integer()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .map(Some)
            .ok_or_else(|| {
                bad(format!(
                    "[serve] port must be an integer from 1 to 65535, not {port}"
                ))
            })
    }

    /// `[sample]`: the default when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and `[sample]` when it
    /// is not a table, has an unknown key or a mistyped value, names a
    /// provider other than `anthropic` or `stub`, sets `max_tokens = 0`,
    /// leaves a model ID empty, or sets a `base_url` that [`base_url_ok`] refuses. The caller turns sampling off; it never stops
    /// the daemon.
    pub fn sample(&self) -> Result<SampleConfig> {
        let bad = |what: String| {
            CoreError::invalid(
                "bad_config",
                format!("{}: [sample] {what}", self.path.display()),
            )
        };
        let Some(v) = self.table.get("sample") else {
            return Ok(SampleConfig::default());
        };
        if !v.is_table() {
            return Err(bad(format!("must be a table, not {}", v.type_str())));
        }
        let s: SampleConfig = v
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| bad(e.message().to_string()))?;
        if !matches!(s.provider.as_str(), "anthropic" | "stub") {
            return Err(bad(format!(
                "provider is \"anthropic\" or \"stub\", not \"{}\"",
                s.provider
            )));
        }
        if s.max_tokens == 0 {
            return Err(bad("max_tokens must be at least 1".into()));
        }
        for (tier, id) in [
            ("quick", &s.models.quick),
            ("default", &s.models.default),
            ("complex", &s.models.complex),
        ] {
            if id.trim().is_empty() {
                return Err(bad(format!("models.{tier} is empty")));
            }
        }
        if !base_url_ok(&s.base_url) {
            return Err(bad(format!(
                "base_url must be https://, or http:// on localhost, 127.0.0.1 or [::1], not \"{}\"",
                s.base_url
            )));
        }
        Ok(s)
    }
}

/// Whether `path` can be the `bin` setting: absolute, and free of `"`, `\`
/// and control characters, so that `bin = "<path>"` is valid TOML whose
/// value is `path` verbatim, and the plugins' wrapper can read it without
/// a TOML parser.
pub fn bin_path_ok(path: &str) -> bool {
    path.starts_with('/') && !path.chars().any(|c| c == '"' || c == '\\' || c.is_control())
}

/// The line `clax bin set` writes for `path`, which must pass
/// [`bin_path_ok`]: `bin = "<path>"`.
pub fn bin_line(path: &str) -> String {
    format!("bin = \"{path}\"")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn with(text: &str) -> HomeConfig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), text).unwrap();
        HomeConfig::load(dir.path()).unwrap()
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
    }

    #[test]
    fn a_bad_sample_table_is_bad_config_naming_the_file_and_table_and_the_port_still_reads() {
        for t in [
            "[sample]\nprovider = \"openai\"\n",
            "[sample]\nunknown = 1\n",
            "[sample]\nmax_tokens = 0\n",
            "[sample]\ndaily_call_cap = -1\n",
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
        for p in ["/usr/local/bin/clax", "/Users/a b/é/clax", "/x/'quoted'/clax"] {
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
}

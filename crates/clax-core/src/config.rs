//! A home's `config.toml`.
//!
//! Read-only here. `[serve] port` is the port a daemon started for the home
//! listens on (7480 when absent); `just dev` and `just watch` write
//! `port = 7481` into `~/.clax-dev/config.toml`. Other tables are reserved
//! (spec §5) and ignored.

use crate::{CoreError, Result};
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
}

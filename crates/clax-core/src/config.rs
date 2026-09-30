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
    table: toml::Table,
}

impl HomeConfig {
    /// `<home_root>/config.toml`; a missing file is an empty config.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file when it does not
    /// parse; `Io` when it cannot be read.
    pub fn load(home_root: &Path) -> Result<HomeConfig> {
        let path = home_root.join(FILE);
        let table = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse::<toml::Table>().map_err(|e| {
                CoreError::invalid("bad_config", format!("{}: {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(HomeConfig { table })
    }

    /// `[serve] port`, when it is an integer in 1..=65535.
    pub fn serve_port(&self) -> Option<u16> {
        let p = self.table.get("serve")?.get("port")?.as_integer()?;
        u16::try_from(p).ok().filter(|p| *p > 0)
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
        assert_eq!(HomeConfig::load(dir.path()).unwrap().serve_port(), None);
    }

    #[test]
    fn serve_port_is_read_and_other_tables_are_ignored() {
        assert_eq!(
            with("[sample]\napi_key_env = \"K\"\n\n[serve]\nport = 7481\n").serve_port(),
            Some(7481)
        );
    }

    #[test]
    fn out_of_range_or_mistyped_ports_are_ignored() {
        for t in [
            "[serve]\nport = 0\n",
            "[serve]\nport = 70000\n",
            "[serve]\nport = \"7481\"\n",
        ] {
            assert_eq!(with(t).serve_port(), None, "{t}");
        }
    }

    #[test]
    fn a_config_that_does_not_parse_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "[serve\n").unwrap();
        let e = HomeConfig::load(dir.path()).unwrap_err().to_string();
        assert!(e.contains("config.toml"), "{e}");
    }
}

//! What the binary reads from its environment. The runtime's key and a
//! GitHub App's are read where `workspace.toml` names them
//! ([`clax_bot_workspace::config`]), and go through nothing here.

use crate::routes::is_id;
use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Holds the bearer secret a host sends.
pub const ENV_TOKEN: &str = "CLAX_WORKSPACE_SERVICE_TOKEN";
/// The directory holding `workspace.toml`, the run ledger and the told file.
pub const ENV_HOME: &str = "CLAX_WORKSPACE_SERVICE_HOME";
/// The one instance this service serves: a host names it on every request,
/// so a second host with the same secret reaps and acknowledges nothing.
pub const ENV_INSTANCE: &str = "CLAX_WORKSPACE_SERVICE_INSTANCE";
/// Where to listen, when not [`DEFAULT_ADDR`].
pub const ENV_ADDR: &str = "CLAX_WORKSPACE_SERVICE_ADDR";
/// Loopback: a tunnel fronts the service, and it has no public port.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7490";
/// The fewest bytes of a bearer secret, which `openssl rand -hex 32` exceeds.
pub const MIN_TOKEN_BYTES: usize = 32;

/// The bearer secret. It has no `Debug` or `Display`, so that no log can
/// carry it, and compares in time that does not depend on its content.
pub struct BearerToken(String);

impl BearerToken {
    /// # Errors
    /// When `token` is too short to be a secret.
    pub fn new(token: String) -> Result<BearerToken> {
        if token.len() < MIN_TOKEN_BYTES {
            bail!("the bearer secret must be at least {MIN_TOKEN_BYTES} bytes");
        }
        Ok(BearerToken(token))
    }

    /// Whether `given` is this secret.
    #[must_use]
    pub fn matches(&self, given: &str) -> bool {
        clax_core::secret::constant_time_eq(self.0.as_bytes(), given.as_bytes())
    }
}

/// The binary's settings.
pub struct Settings {
    pub token: BearerToken,
    pub home: PathBuf,
    pub addr: SocketAddr,
    /// The one instance it serves.
    pub instance: String,
}

impl Settings {
    /// From `var`, which reads the environment. No message says what a
    /// variable held.
    ///
    /// # Errors
    /// When the secret, the home or the instance is missing, or a value
    /// cannot be read.
    pub fn read(var: impl Fn(&str) -> Option<String>) -> Result<Settings> {
        let token = var(ENV_TOKEN).with_context(|| format!("{ENV_TOKEN} is not set"))?;
        let home = var(ENV_HOME).with_context(|| format!("{ENV_HOME} is not set"))?;
        let instance = var(ENV_INSTANCE).with_context(|| format!("{ENV_INSTANCE} is not set"))?;
        if !is_id(&instance) {
            bail!("{ENV_INSTANCE} holds letters, digits, underscores and hyphens");
        }
        let addr = var(ENV_ADDR).unwrap_or_else(|| DEFAULT_ADDR.to_string());
        Ok(Settings {
            token: BearerToken::new(token).with_context(|| format!("reading {ENV_TOKEN}"))?,
            home: PathBuf::from(home),
            instance,
            addr: addr
                .parse()
                .with_context(|| format!("{ENV_ADDR} is not an address and port"))?,
        })
    }
}

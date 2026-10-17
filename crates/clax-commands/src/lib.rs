//! Every `clax` subcommand, and the daemon clients they share: the command
//! line ([`Cli`]) and its dispatch, `commands/*`, the blocking daemon client
//! (`client`), the agent channel, the hook log and the plugin and extension
//! installers.
//!
//! The binary (`clax-cli`) hands [`run`] what only it has ([`Env`]): the
//! files it embeds, behind `clax_core::embedded::Bundle`, and the daemon it
//! runs in process. So this crate depends on no server crate and on no
//! embedded file: a daemon edit or a web UI edit rebuilds the binary, not
//! the commands. It reaches the store directly only for `doctor` and
//! `init`'s credential revoke.
//!
//! Every command reaches the daemon through the clients that carry the
//! agent-channel marker (`clippy.toml`).

mod channel;
mod cli;
mod client;
mod codex_approvals;
mod commands;
mod extension_files;
mod hooklog;
mod host;
mod plugin_bin;
mod plugins;
mod remote;
mod term;

pub use cli::{Cli, Cmd, PORT_ENV, env_port};

use clax_core::Home;
use clax_core::embedded::Bundle;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::OnceLock;

/// What the binary provides to the commands.
pub struct Env {
    /// The files the binary carries.
    pub embedded: &'static dyn Bundle,
    /// Runs the daemon in this process (`clax serve --foreground`), until
    /// it stops.
    pub serve: fn(&Home, ServeArgs) -> anyhow::Result<()>,
    /// The commit the binary was built from.
    pub build_commit: &'static str,
}

/// What `clax serve --foreground` asks of the daemon [`Env::serve`] runs.
pub struct ServeArgs {
    /// The address to listen on.
    pub bind: IpAddr,
    /// The first port to try (0: any free port).
    pub port: u16,
    /// The version the daemon reports.
    pub version: &'static str,
    /// Debug builds only: serve the web UI from this directory, laid out
    /// like `web/dist`, in place of the binary's.
    pub web_dist: Option<PathBuf>,
}

static ENV: OnceLock<Env> = OnceLock::new();

/// Parses the command line and runs the command it names, exiting the
/// process with the command's status.
pub fn run(env: Env) -> std::process::ExitCode {
    clax_core::set_build_commit(env.build_commit);
    if ENV.set(env).is_err() {
        panic!("clax_commands::run runs once per process");
    }
    cli::main();
    std::process::ExitCode::SUCCESS
}

/// The files the binary carries. A unit test, which runs no binary, gets
/// the checkout's launcher and empty trees.
pub(crate) fn embedded() -> &'static dyn Bundle {
    match ENV.get() {
        Some(env) => env.embedded,
        #[cfg(test)]
        None => &test_bundle::CHECKOUT,
        #[cfg(not(test))]
        None => panic!("clax_commands::run sets the binary's files first"),
    }
}

/// Runs the daemon in this process, as the binary does.
pub(crate) fn serve(home: &Home, args: ServeArgs) -> anyhow::Result<()> {
    let env = ENV
        .get()
        .expect("clax_commands::run sets the binary's daemon first");
    (env.serve)(home, args)
}

#[cfg(test)]
mod test_bundle {
    use clax_core::embedded::{Bundle, FileTree, MemTree};

    /// The launcher from the checkout, and no extension or plugin files.
    pub struct Checkout;

    pub static CHECKOUT: Checkout = Checkout;
    static EMPTY: MemTree = MemTree(std::collections::BTreeMap::new());

    impl Bundle for Checkout {
        fn extension(&self) -> &dyn FileTree {
            &EMPTY
        }
        fn plugins(&self) -> &dyn FileTree {
            &EMPTY
        }
        fn launcher(&self) -> &'static str {
            include_str!("../../../scripts/ensure-clax.sh")
        }
    }
}

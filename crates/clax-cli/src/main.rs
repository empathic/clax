//! The `clax` command line: run the daemon and manage artifacts.

mod client;
mod commands;
mod hooklog;
mod plugins;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "clax",
    version,
    about = "Local artifacts with comment-driven development"
)]
pub struct Cli {
    /// Emit one JSON object on stdout instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Port to use when starting a daemon (0 = any free port). Default: the
    /// home's `[serve] port` in config.toml, else 7480.
    #[arg(long, global = true)]
    pub port: Option<u16>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Start the daemon in the background (or run it here with --foreground).
    Serve(commands::serve::Args),
    /// Stop the running daemon.
    Stop,
    /// Show whether a daemon is running.
    Status(commands::status::Args),
    /// Publish a page (and files) as a new artifact or a new version.
    Publish(Box<commands::publish::Args>),
    /// List artifacts, or the files of one.
    List(commands::list::Args),
    /// Read a published file of an artifact.
    Read(commands::read::Args),
    /// Manage an artifact's assets.
    #[command(subcommand)]
    Asset(commands::asset::Cmd),
    /// Open an artifact in the browser.
    Open(commands::open::Args),
    /// Delete an artifact.
    Delete(commands::delete::Args),
    /// Pin an artifact to the top of the gallery.
    Pin(commands::pin::Args),
    /// Unpin an artifact.
    Unpin(commands::pin::Args),
    /// Check the installation and storage.
    Doctor(commands::doctor::Args),
    /// Serve the MCP tools over stdin/stdout for one harness session.
    Mcp(commands::mcp::Args),
    /// Handle a harness lifecycle hook (reads the hook input from stdin).
    Hook(commands::hook::Args),
    /// Register the Clax plugins built into this binary with each harness
    /// whose CLI is on PATH, replacing stale registrations.
    ///
    /// Re-running reinstalls the plugin, which enables it again where it was
    /// disabled. A Pi package is removed only when `init` recorded it or its
    /// package.json names Clax's Pi package; one whose directory is missing
    /// is left and named, with the command that removes it. Known miss:
    /// `~user/` paths are not expanded. A Pi entry written that way is left
    /// registered, and a CODEX_HOME, CLAUDE_CONFIG_DIR or PI_CODING_AGENT_DIR
    /// written that way is taken relative to HOME.
    Init(commands::init::Args),
    /// Remove the Clax plugin registrations from each harness whose CLI is
    /// on PATH, and the plugins' copy once no harness refers to it.
    ///
    /// The copy is kept while any harness's registry still names it or
    /// cannot be read. The same known miss as `init` applies.
    Uninit(commands::init::Args),
    /// Print a haiku about Clax, one of ten, chosen at random.
    Haiku,
}

impl Cli {
    /// `--port` when given, else the home's `[serve] port`, else 7480.
    ///
    /// # Errors
    /// When `--port` is absent and the home's `config.toml` exists but cannot
    /// be read or parsed, or holds a `[serve] port` that is not a port; the
    /// message names the file. A missing file or key means the default.
    pub fn port_for(&self, home: &clax_core::Home) -> anyhow::Result<u16> {
        if let Some(p) = self.port {
            return Ok(p);
        }
        let port = clax_core::config::HomeConfig::load(home.root())
            .and_then(|c| c.serve_port())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(port.unwrap_or(clax_server::daemon::DEFAULT_PORT))
    }
}

/// True when the command line names the `hook` subcommand: a hook must never
/// fail its harness, so its startup failures exit 0.
fn is_hook_invocation() -> bool {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "hook" => return true,
            "--port" => {
                args.next();
            }
            s if s.starts_with('-') => {}
            _ => return false,
        }
    }
    false
}

/// The value after `--agent` on the command line, or `-`.
fn agent_arg() -> String {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--agent")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "-".into())
}

/// The Clax home, as an absolute path, so commands that run elsewhere (the
/// daemon, the harness CLIs `init` runs in HOME) see the same directory.
fn home_from_env() -> Result<clax_core::Home, String> {
    let home = clax_core::Home::from_env().map_err(|e| e.to_string())?;
    let root = std::path::absolute(home.root()).map_err(|e| format!("CLAX_HOME: {e}"))?;
    Ok(clax_core::Home::at(root))
}

fn main() {
    let started = std::time::Instant::now();
    let hook = is_hook_invocation();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if hook && let Ok(home) = home_from_env() {
                let text = e.to_string();
                commands::hook::log_run(&home, &agent_arg(), "-", started, Some(text.trim()));
            }
            let code = match e.kind() {
                _ if hook => 0,
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 1,
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let home = match home_from_env() {
        Ok(home) => home,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(if hook { 0 } else { 1 });
        }
    };
    let result = match &cli.cmd {
        Cmd::Serve(a) => commands::serve::run(&cli, &home, a),
        Cmd::Stop => commands::stop::run(&cli, &home),
        Cmd::Status(a) => commands::status::run(&cli, &home, a),
        Cmd::Publish(a) => commands::publish::run(&cli, &home, a),
        Cmd::List(a) => commands::list::run(&cli, &home, a),
        Cmd::Read(a) => commands::read::run(&cli, &home, a),
        Cmd::Asset(c) => commands::asset::run(&cli, &home, c),
        Cmd::Open(a) => commands::open::run(&cli, &home, a),
        Cmd::Delete(a) => commands::delete::run(&cli, &home, a),
        Cmd::Pin(a) => commands::pin::run(&cli, &home, a, true),
        Cmd::Unpin(a) => commands::pin::run(&cli, &home, a, false),
        Cmd::Doctor(a) => commands::doctor::run(&cli, &home, a),
        Cmd::Mcp(a) => commands::mcp::run(&cli, &home, a),
        Cmd::Hook(a) => commands::hook::run(&cli, &home, a),
        Cmd::Init(a) => commands::init::init(&cli, &home, a),
        Cmd::Uninit(a) => commands::init::uninit(&cli, &home, a),
        Cmd::Haiku => commands::haiku::run(&cli),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn port_for_uses_the_flag_then_the_config_then_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        assert_eq!(
            cli(&["clax", "list"]).port_for(&home).unwrap(),
            clax_server::daemon::DEFAULT_PORT
        );
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 7481\n").unwrap();
        assert_eq!(cli(&["clax", "list"]).port_for(&home).unwrap(), 7481);
        assert_eq!(
            cli(&["clax", "--port", "0", "list"])
                .port_for(&home)
                .unwrap(),
            0
        );
    }

    #[test]
    fn port_for_rejects_a_config_that_does_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        std::fs::write(dir.path().join("config.toml"), "[serve\nport = 7481\n").unwrap();
        let e = cli(&["clax", "list"])
            .port_for(&home)
            .unwrap_err()
            .to_string();
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn port_for_rejects_a_config_that_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        // A directory where the file should be: present, but unreadable as text.
        std::fs::create_dir(dir.path().join("config.toml")).unwrap();
        let e = cli(&["clax", "list"])
            .port_for(&home)
            .unwrap_err()
            .to_string();
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn port_for_rejects_a_serve_port_that_is_not_a_port() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 74810\n").unwrap();
        let e = cli(&["clax", "list"])
            .port_for(&home)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("config.toml") && e.contains("[serve] port"),
            "{e}"
        );
    }
}

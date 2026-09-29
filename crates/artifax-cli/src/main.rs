//! The `artifax` command line: run the daemon and manage artifacts.

mod client;
mod commands;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "artifax",
    version,
    about = "Local artifacts with comment-driven development"
)]
pub struct Cli {
    /// Emit one JSON object on stdout instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Port to use when starting a daemon (0 = any free port).
    #[arg(long, global = true, default_value_t = artifax_server::daemon::DEFAULT_PORT)]
    pub port: u16,
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
    Publish(commands::publish::Args),
    /// List artifacts, or the files of one.
    List(commands::list::Args),
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

fn main() {
    let hook = is_hook_invocation();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = match e.kind() {
                _ if hook => 0,
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 1,
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let home = match artifax_core::Home::from_env() {
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
        Cmd::Open(a) => commands::open::run(&cli, &home, a),
        Cmd::Delete(a) => commands::delete::run(&cli, &home, a),
        Cmd::Pin(a) => commands::pin::run(&cli, &home, a, true),
        Cmd::Unpin(a) => commands::pin::run(&cli, &home, a, false),
        Cmd::Doctor(a) => commands::doctor::run(&cli, &home, a),
        Cmd::Mcp(a) => commands::mcp::run(&cli, &home, a),
        Cmd::Hook(a) => commands::hook::run(&cli, &home, a),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

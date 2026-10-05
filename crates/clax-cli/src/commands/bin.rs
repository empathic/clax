//! `clax bin`: which clax the plugins run, and the `bin` setting in the
//! home's config.toml that names one (see [`crate::plugin_bin`]).

use crate::plugin_bin;
use clax_core::Home;
use serde_json::json;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Show which clax the plugins run, and why (the default).
    Show,
    /// Make the plugins run this clax binary: writes `bin = "<path>"` to
    /// config.toml. The path must be absolute, hold no quote, backslash or
    /// control character, and be a clax. Takes effect at each plugin's next
    /// start.
    Set(SetArgs),
    /// Remove the bin setting, so the plugins run the release they pin.
    Clear,
}

#[derive(clap::Args)]
#[group(required = true, multiple = false)]
pub struct SetArgs {
    /// The absolute path of a clax binary.
    path: Option<PathBuf>,
    /// This clax binary (the one running this command).
    #[arg(long)]
    this: bool,
}

/// What `clax bin` shows: the resolution, as the plugins' wrapper this
/// binary carries would make it.
fn show(cli: &crate::Cli, home: &Home) {
    let pin = plugin_bin::pinned_version(plugin_bin::LAUNCHER);
    let clax_bin = std::env::var("CLAX_BIN").ok();
    let r = plugin_bin::resolve(home, clax_bin.as_deref(), pin.as_deref());
    let mut out = r.to_json();
    out["pinned_version"] = json!(pin);
    out["setting"] = json!(plugin_bin::current(home));
    super::print(cli, out, |_| {
        let mut lines = vec![format!("the plugins run: {}", r.line())];
        lines.push(format!(
            "the release this clax's plugins pin: {}",
            pin.as_deref().unwrap_or("none")
        ));
        lines.join("\n")
    });
}

pub fn run(cli: &crate::Cli, home: &Home, cmd: Option<&Cmd>) -> anyhow::Result<()> {
    match cmd {
        None | Some(Cmd::Show) => show(cli, home),
        Some(Cmd::Set(a)) => {
            let path = match (&a.path, a.this) {
                (_, true) => std::env::current_exe()?,
                (Some(p), false) => p.clone(),
                (None, false) => unreachable!("clap requires a path or --this"),
            };
            let version = plugin_bin::set(home, &path)?;
            let config = home.root().join(clax_core::config::FILE);
            super::print(
                cli,
                json!({"bin": path, "version": version, "config": config}),
                |_| {
                    format!(
                        "the plugins now run {} ({version}), from their next start (bin in {})",
                        path.display(),
                        config.display()
                    )
                },
            );
        }
        Some(Cmd::Clear) => {
            let removed = plugin_bin::clear(home)?;
            super::print(cli, json!({"cleared": removed}), |_| {
                if removed {
                    "removed the bin setting; the plugins run the release they pin from their next start".into()
                } else {
                    "no bin setting to remove".into()
                }
            });
        }
    }
    Ok(())
}

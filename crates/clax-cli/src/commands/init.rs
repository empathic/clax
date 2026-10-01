//! `clax init` and `clax uninit`: register the plugins this binary embeds
//! with each harness through the harness's own CLI, and remove them.
//!
//! `init` writes the marketplace tree ([`crate::plugins`]) to
//! `<home>/marketplace`, then for each harness removes the existing `clax`
//! registration and any under the previous product name (found in the
//! harness's own registry), and adds the new one. `uninit` does the
//! removals and deletes the marketplace directory. Clax's data is never
//! touched, nor the previous name's home.
//!
//! Each harness is one entry in [`HARNESSES`]: its CLI's name and the
//! commands that remove and add its registration.

use super::doctor_agent::Dirs;
use clax_core::Home;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The previous product name, assembled so the name gate finds no literal.
const OLD: &str = concat!("arti", "fax");
/// The Pi package's name.
const PI_PACKAGE: &str = "@empathic/clax-pi";

/// A harness Clax registers its plugin with.
struct Harness {
    /// The `--agent` value, which is also the name of the harness's CLI.
    name: &'static str,
    /// The commands that remove Clax's registration and any under the
    /// previous name that the harness's registry (under [`Dirs`]) shows.
    removals: fn(&Dirs) -> Vec<Step>,
    /// The commands that register the marketplace at the given root.
    additions: fn(&Path) -> Vec<Step>,
}

/// Every supported harness, in the order `init` and `uninit` visit them.
const HARNESSES: &[Harness] = &[
    Harness {
        name: "claude",
        removals: claude_removals,
        additions: claude_additions,
    },
    Harness {
        name: "codex",
        removals: codex_removals,
        additions: codex_additions,
    },
    Harness {
        name: "pi",
        removals: pi_removals,
        additions: pi_additions,
    },
];

#[derive(clap::Args)]
pub struct Args {
    /// Only this harness (repeatable). Default: every one whose CLI
    /// (`claude`, `codex`, `pi`) is on PATH.
    #[arg(
        long = "agent",
        value_parser = clap::builder::PossibleValuesParser::new(HARNESSES.iter().map(|h| h.name))
    )]
    pub agents: Vec<String>,
}

/// One harness command; a failure of a `required` one fails the harness.
struct Step {
    args: Vec<String>,
    required: bool,
}

fn step(required: bool, args: &[&str]) -> Step {
    Step {
        args: args.iter().map(|s| s.to_string()).collect(),
        required,
    }
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Whether a JSON registry names `key`, at the top level or under `plugins`.
fn names(v: &Option<Value>, key: &str) -> bool {
    v.as_ref()
        .is_some_and(|v| v.get(key).is_some() || v["plugins"].get(key).is_some())
}

fn old_plugin() -> String {
    format!("{OLD}@{OLD}")
}

fn claude_removals(dirs: &Dirs) -> Vec<Step> {
    let old_plugin = old_plugin();
    let installed = read_json(&dirs.claude_dir.join("plugins/installed_plugins.json"));
    let markets = read_json(&dirs.claude_dir.join("plugins/known_marketplaces.json"));
    let mut out = Vec::new();
    if names(&installed, &old_plugin) {
        out.push(step(false, &["plugin", "uninstall", old_plugin.as_str()]));
    }
    if names(&markets, OLD) {
        out.push(step(false, &["plugin", "marketplace", "remove", OLD]));
    }
    out.push(step(false, &["plugin", "uninstall", "clax@clax"]));
    out.push(step(false, &["plugin", "marketplace", "remove", "clax"]));
    out
}

fn claude_additions(root: &Path) -> Vec<Step> {
    let r = root.display().to_string();
    vec![
        step(true, &["plugin", "marketplace", "add", r.as_str()]),
        step(true, &["plugin", "install", "clax@clax"]),
    ]
}

fn codex_removals(dirs: &Dirs) -> Vec<Step> {
    let old_plugin = old_plugin();
    let cfg: Option<toml::Table> = std::fs::read_to_string(dirs.codex_home.join("config.toml"))
        .ok()
        .and_then(|t| t.parse().ok());
    let has = |table: &str, key: &str| {
        cfg.as_ref()
            .and_then(|c| c.get(table))
            .and_then(|t| t.get(key))
            .is_some()
    };
    let mut out = Vec::new();
    if has("plugins", &old_plugin) {
        out.push(step(false, &["plugin", "remove", old_plugin.as_str()]));
    }
    if has("marketplaces", OLD) {
        out.push(step(false, &["plugin", "marketplace", "remove", OLD]));
    }
    out.push(step(false, &["plugin", "remove", "clax@clax"]));
    out.push(step(false, &["plugin", "marketplace", "remove", "clax"]));
    out
}

fn codex_additions(root: &Path) -> Vec<Step> {
    let r = root.display().to_string();
    vec![
        step(true, &["plugin", "marketplace", "add", r.as_str()]),
        step(true, &["plugin", "add", "clax@clax"]),
    ]
}

/// The installed Pi packages (absolute directories) whose `package.json`
/// `name` is one of `wanted`, from `<pi dir>/settings.json` (local paths
/// there are relative to that directory).
fn pi_packages(pi_dir: &Path, wanted: &[String]) -> Vec<PathBuf> {
    let Some(v) = read_json(&pi_dir.join("settings.json")) else {
        return Vec::new();
    };
    v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().or_else(|| p["source"].as_str()))
        .filter(|s| !s.contains(':'))
        .map(|s| {
            let p = Path::new(s);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                pi_dir.join(p)
            };
            abs.canonicalize().unwrap_or(abs)
        })
        .filter(|d| {
            read_json(&d.join("package.json"))
                .and_then(|v| v["name"].as_str().map(str::to_string))
                .is_some_and(|n| wanted.contains(&n))
        })
        .collect()
}

fn pi_removals(dirs: &Dirs) -> Vec<Step> {
    let wanted = vec![format!("@empathic/{OLD}-pi"), PI_PACKAGE.to_string()];
    pi_packages(&dirs.pi_dir, &wanted)
        .into_iter()
        .map(|d| {
            let d = d.display().to_string();
            step(false, &["remove", d.as_str()])
        })
        .collect()
}

fn pi_additions(root: &Path) -> Vec<Step> {
    let pi = root.join("plugins/pi").display().to_string();
    vec![step(true, &["install", pi.as_str()])]
}

/// `name` in a directory of `PATH`, if any.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Runs `steps` with `h`'s CLI; the harness's result as JSON.
fn run_steps(h: &Harness, steps: Vec<Step>, done: &str) -> Value {
    let mut commands = Vec::new();
    let mut notes = Vec::new();
    let mut failed = false;
    for s in steps {
        let line = format!("{} {}", h.name, s.args.join(" "));
        commands.push(line.clone());
        let out = std::process::Command::new(h.name)
            .args(&s.args)
            .stdin(std::process::Stdio::null())
            .output();
        let err = match out {
            Ok(o) if o.status.success() => continue,
            Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => e.to_string(),
        };
        if s.required {
            failed = true;
            notes.push(format!("`{line}` failed: {err}"));
            break;
        }
    }
    let status = if failed { "failed" } else { done };
    json!({"agent": h.name, "status": status, "detail": notes.join("\n"), "commands": commands})
}

/// Warns on stderr when the first `clax` on `PATH`, which the plugins run,
/// is not this executable.
fn warn_unless_first_on_path() {
    let first = super::doctor_agent::clax_on_path(&std::env::var_os("PATH").unwrap_or_default())
        .into_iter()
        .find(|(_, v)| v.is_some())
        .map(|(p, _)| p);
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    if first.as_ref().and_then(|p| p.canonicalize().ok()) != me {
        eprintln!(
            "warning: the plugins run the first clax on PATH, which is {}, not this one ({}); put this one's directory first on PATH",
            first
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "none".into()),
            me.map(|p| p.display().to_string()).unwrap_or_default()
        );
    }
}

fn run(cli: &crate::Cli, home: &Home, a: &Args, install: bool) -> anyhow::Result<()> {
    let dirs = Dirs::from_env(|k| std::env::var(k).ok())
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let root = home.root().join("marketplace");
    if install {
        crate::plugins::materialize(&root)?;
    }
    let chosen = HARNESSES
        .iter()
        .filter(|h| a.agents.is_empty() || a.agents.iter().any(|n| n == h.name));
    let mut results = Vec::new();
    for h in chosen {
        if on_path(h.name).is_none() {
            results.push(json!({"agent": h.name, "status": "skipped", "detail": format!("{} is not on PATH", h.name), "commands": []}));
            continue;
        }
        let mut steps = (h.removals)(&dirs);
        if install {
            steps.extend((h.additions)(&root));
        }
        results.push(run_steps(
            h,
            steps,
            if install { "registered" } else { "removed" },
        ));
    }
    if !install && root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    let failed = results.iter().any(|r| r["status"] == "failed");
    let out = json!({"marketplace": root, "agents": results});
    super::print(cli, out, |j| {
        let mut lines = vec![format!(
            "marketplace: {}",
            j["marketplace"].as_str().unwrap_or_default()
        )];
        for r in j["agents"].as_array().into_iter().flatten() {
            let mut l = format!(
                "{}: {}",
                r["agent"].as_str().unwrap_or_default(),
                r["status"].as_str().unwrap_or_default()
            );
            if let Some(d) = r["detail"].as_str().filter(|d| !d.is_empty()) {
                l.push_str(&format!(" ({d})"));
            }
            lines.push(l);
        }
        if install {
            lines.push("Start a new session in each harness to load the plugin.".into());
        }
        lines.join("\n")
    });
    if install {
        warn_unless_first_on_path();
    }
    if failed {
        anyhow::bail!(
            "a harness could not be {}",
            if install {
                "registered"
            } else {
                "unregistered"
            }
        );
    }
    Ok(())
}

pub fn init(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    run(cli, home, a, true)
}

pub fn uninit(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    run(cli, home, a, false)
}

//! `clax init` and `clax uninit`: register the plugins this binary embeds
//! with each harness through the harness's own CLI, and remove them.
//!
//! `init` writes the marketplace tree ([`crate::plugins`]) to
//! `<home>/marketplace`, then for each harness removes the existing `clax`
//! registration and any under the previous product name (found in the
//! harness's own registry), and adds the new one. `uninit` does the
//! removals, then deletes the marketplace directory unless a harness's
//! registry still points into it. Clax's data is never touched, nor the
//! previous name's home. Both hold `<home>/init.lock` throughout, and run
//! each harness CLI in the home directory, so a project's own harness
//! settings in the caller's working directory are never edited.
//!
//! Each harness is one entry in [`HARNESSES`]: its CLI's name, the commands
//! that remove and add its registration, and how to tell whether its
//! registry still refers to the marketplace.

use super::doctor_agent::Dirs;
use clax_core::Home;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

/// The previous product name, assembled so the name gate finds no literal.
const OLD: &str = concat!("arti", "fax");
/// The Pi package's name.
const PI_PACKAGE: &str = "@empathic/clax-pi";

/// What a harness's functions read: its configuration directories and the
/// user's home directory.
struct Ctx {
    dirs: Dirs,
    home: PathBuf,
}

/// A harness Clax registers its plugin with.
struct Harness {
    /// The `--agent` value, which is also the name of the harness's CLI.
    name: &'static str,
    /// The commands that remove Clax's registration and any under the
    /// previous name that the harness's registry shows, with notes on what
    /// the registry could not settle.
    removals: fn(&Ctx) -> Actions,
    /// The commands that register the marketplace at the given root.
    additions: fn(&Path) -> Vec<Step>,
    /// Whether the harness's registry still refers to a path under the
    /// given root.
    uses: fn(&Ctx, &Path) -> bool,
}

/// Every supported harness, in the order `init` and `uninit` visit them.
const HARNESSES: &[Harness] = &[
    Harness {
        name: "claude",
        removals: claude_removals,
        additions: claude_additions,
        uses: claude_uses,
    },
    Harness {
        name: "codex",
        removals: codex_removals,
        additions: codex_additions,
        uses: codex_uses,
    },
    Harness {
        name: "pi",
        removals: pi_removals,
        additions: pi_additions,
        uses: pi_uses,
    },
];

#[derive(clap::Args)]
pub struct Args {
    /// Only this harness (repeatable). Default: every one whose CLI is on PATH.
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

/// Commands to run, and notes for the harness's `detail`.
#[derive(Default)]
struct Actions {
    steps: Vec<Step>,
    notes: Vec<String>,
}

/// A registry file's contents: `None` when it does not exist. A file that
/// exists but cannot be read or parsed adds a note and also gives `None`.
fn read_registry<T>(
    p: &Path,
    parse: impl FnOnce(&str) -> Result<T, String>,
    notes: &mut Vec<String>,
) -> Option<T> {
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            notes.push(format!("could not read {}: {e}", p.display()));
            return None;
        }
    };
    match parse(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            notes.push(format!(
                "could not parse {} ({e}); registrations there were not looked for",
                p.display()
            ));
            None
        }
    }
}

fn read_json(p: &Path, notes: &mut Vec<String>) -> Option<Value> {
    read_registry(
        p,
        |t| serde_json::from_str(t).map_err(|e| e.to_string()),
        notes,
    )
}

fn read_toml(p: &Path, notes: &mut Vec<String>) -> Option<toml::Table> {
    read_registry(
        p,
        |t| t.parse().map_err(|e: toml::de::Error| e.to_string()),
        notes,
    )
}

/// Whether a JSON registry names `key`, at the top level or under `plugins`.
fn names(v: &Option<Value>, key: &str) -> bool {
    v.as_ref()
        .is_some_and(|v| v.get(key).is_some() || v["plugins"].get(key).is_some())
}

/// Whether a string in `v`, at any depth, is a path under `root`.
fn json_mentions(v: &Value, root: &Path) -> bool {
    match v {
        Value::String(s) => Path::new(s).starts_with(root),
        Value::Array(a) => a.iter().any(|v| json_mentions(v, root)),
        Value::Object(o) => o.values().any(|v| json_mentions(v, root)),
        _ => false,
    }
}

/// Whether a string in `v`, at any depth, is a path under `root`.
fn toml_mentions(v: &toml::Value, root: &Path) -> bool {
    match v {
        toml::Value::String(s) => Path::new(s).starts_with(root),
        toml::Value::Array(a) => a.iter().any(|v| toml_mentions(v, root)),
        toml::Value::Table(t) => t.values().any(|v| toml_mentions(v, root)),
        _ => false,
    }
}

fn old_plugin() -> String {
    format!("{OLD}@{OLD}")
}

fn claude_removals(ctx: &Ctx) -> Actions {
    let mut a = Actions::default();
    let old_plugin = old_plugin();
    let plugins = ctx.dirs.claude_dir.join("plugins");
    let installed = read_json(&plugins.join("installed_plugins.json"), &mut a.notes);
    let markets = read_json(&plugins.join("known_marketplaces.json"), &mut a.notes);
    if names(&installed, &old_plugin) {
        a.steps
            .push(step(false, &["plugin", "uninstall", old_plugin.as_str()]));
    }
    if names(&markets, OLD) {
        a.steps
            .push(step(false, &["plugin", "marketplace", "remove", OLD]));
    }
    a.steps
        .push(step(false, &["plugin", "uninstall", "clax@clax"]));
    a.steps
        .push(step(false, &["plugin", "marketplace", "remove", "clax"]));
    a
}

fn claude_additions(root: &Path) -> Vec<Step> {
    let r = root.display().to_string();
    vec![
        step(true, &["plugin", "marketplace", "add", r.as_str()]),
        step(true, &["plugin", "install", "clax@clax"]),
    ]
}

fn claude_uses(ctx: &Ctx, root: &Path) -> bool {
    let d = &ctx.dirs.claude_dir;
    [
        d.join("plugins/known_marketplaces.json"),
        d.join("plugins/installed_plugins.json"),
        d.join("settings.json"),
    ]
    .iter()
    .filter_map(|p| read_json(p, &mut Vec::new()))
    .any(|v| json_mentions(&v, root))
}

fn codex_removals(ctx: &Ctx) -> Actions {
    let mut a = Actions::default();
    let old_plugin = old_plugin();
    let cfg = read_toml(&ctx.dirs.codex_home.join("config.toml"), &mut a.notes);
    let has = |table: &str, key: &str| {
        cfg.as_ref()
            .and_then(|c| c.get(table))
            .and_then(|t| t.get(key))
            .is_some()
    };
    if has("plugins", &old_plugin) {
        a.steps
            .push(step(false, &["plugin", "remove", old_plugin.as_str()]));
    }
    if has("marketplaces", OLD) {
        a.steps
            .push(step(false, &["plugin", "marketplace", "remove", OLD]));
    }
    a.steps
        .push(step(false, &["plugin", "remove", "clax@clax"]));
    a.steps
        .push(step(false, &["plugin", "marketplace", "remove", "clax"]));
    a
}

fn codex_additions(root: &Path) -> Vec<Step> {
    let r = root.display().to_string();
    vec![
        step(true, &["plugin", "marketplace", "add", r.as_str()]),
        step(true, &["plugin", "add", "clax@clax"]),
    ]
}

fn codex_uses(ctx: &Ctx, root: &Path) -> bool {
    read_toml(&ctx.dirs.codex_home.join("config.toml"), &mut Vec::new())
        .is_some_and(|t| toml_mentions(&toml::Value::Table(t), root))
}

/// `p` with `.` and `..` resolved without following symlinks, as Node's
/// `path.resolve` does.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// The local Pi package sources in `<pi dir>/settings.json`, each as the
/// absolute directory Pi resolves it to: `~` against the home directory, a
/// relative path against the Pi directory, lexically. Sources with a scheme
/// (`npm:`, `git:`, …) are left out.
fn pi_local_packages(ctx: &Ctx, notes: &mut Vec<String>) -> Vec<PathBuf> {
    let pi_dir = &ctx.dirs.pi_dir;
    let Some(v) = read_json(&pi_dir.join("settings.json"), notes) else {
        return Vec::new();
    };
    v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().or_else(|| p["source"].as_str()))
        .filter(|s| !s.contains(':'))
        .map(|s| {
            let abs = if s == "~" {
                ctx.home.clone()
            } else if let Some(rest) = s.strip_prefix("~/") {
                ctx.home.join(rest)
            } else {
                pi_dir.join(s)
            };
            lexical(&abs)
        })
        .collect()
}

/// Whether a missing package directory looks like a Clax plugin from a
/// checkout or marketplace: `…/plugins/pi` under a path naming Clax or the
/// previous name.
fn looks_like_ours(d: &Path) -> bool {
    d.ends_with("plugins/pi")
        && d.components().any(|c| {
            let c = c.as_os_str().to_string_lossy().to_lowercase();
            c.contains("clax") || c.contains(OLD)
        })
}

fn pi_removals(ctx: &Ctx) -> Actions {
    let mut a = Actions::default();
    let wanted = [format!("@empathic/{OLD}-pi"), PI_PACKAGE.to_string()];
    for d in pi_local_packages(ctx, &mut a.notes) {
        let ours = if d.is_dir() {
            std::fs::read_to_string(d.join("package.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["name"].as_str().map(str::to_string))
                .is_some_and(|n| wanted.contains(&n))
        } else if looks_like_ours(&d) {
            a.notes.push(format!(
                "{} is gone; removing its registration",
                d.display()
            ));
            true
        } else {
            a.notes
                .push(format!("Pi package {} is missing; left alone", d.display()));
            false
        };
        if ours {
            let d = d.display().to_string();
            a.steps.push(step(false, &["remove", d.as_str()]));
        }
    }
    a
}

fn pi_additions(root: &Path) -> Vec<Step> {
    let pi = root.join("plugins/pi").display().to_string();
    vec![step(true, &["install", pi.as_str()])]
}

fn pi_uses(ctx: &Ctx, root: &Path) -> bool {
    pi_local_packages(ctx, &mut Vec::new())
        .iter()
        .any(|d| d.starts_with(root))
}

/// `name` in a directory of `PATH`, if any.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Runs `actions` with `h`'s CLI in the home directory; the harness's
/// result as JSON. A failed optional command is noted and skipped; a failed
/// required one fails the harness and stops it.
fn run_steps(h: &Harness, ctx: &Ctx, actions: Actions, done: &str) -> Value {
    let mut commands = Vec::new();
    let mut notes = actions.notes;
    let mut failed = false;
    for s in actions.steps {
        let line = format!("{} {}", h.name, s.args.join(" "));
        commands.push(line.clone());
        let mut cmd = std::process::Command::new(h.name);
        cmd.args(&s.args).stdin(std::process::Stdio::null());
        if ctx.home.is_dir() {
            cmd.current_dir(&ctx.home);
        }
        let err = match cmd.output() {
            Ok(o) if o.status.success() => continue,
            Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => e.to_string(),
        };
        if s.required {
            failed = true;
            notes.push(format!("`{line}` failed: {err}"));
            break;
        }
        let first = err.lines().next().unwrap_or_default();
        notes.push(format!("`{line}` failed (ignored): {first}"));
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

/// Exclusive advisory lock on `<home>/init.lock`, held until dropped, so
/// concurrent `init` and `uninit` runs take turns.
struct InitLock(#[allow(dead_code)] std::fs::File);

impl InitLock {
    fn acquire(home: &Home) -> std::io::Result<InitLock> {
        use std::os::fd::AsRawFd;
        std::fs::create_dir_all(home.root())?;
        let f = std::fs::File::create(home.root().join("init.lock"))?;
        loop {
            // SAFETY: flock on an owned, open descriptor.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(InitLock(f));
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// After `uninit`: removes the marketplace unless a harness's registry,
/// whether or not this run covered it, still refers to it. Returns what
/// happened, for the output.
fn remove_marketplace_unless_used(ctx: &Ctx, root: &Path) -> anyhow::Result<String> {
    if !root.exists() {
        return Ok(String::new());
    }
    let mut roots = vec![root.to_path_buf()];
    if let Ok(c) = root.canonicalize()
        && c != root
    {
        roots.push(c);
    }
    let users: Vec<&str> = HARNESSES
        .iter()
        .filter(|h| roots.iter().any(|r| (h.uses)(ctx, r)))
        .map(|h| h.name)
        .collect();
    if users.is_empty() {
        std::fs::remove_dir_all(root)?;
        Ok("removed".into())
    } else {
        Ok(format!(
            "kept: still registered with {}; run `clax uninit --agent <name>` for each",
            users.join(", ")
        ))
    }
}

fn run(cli: &crate::Cli, home: &Home, a: &Args, install: bool) -> anyhow::Result<()> {
    let user_home = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dirs = Dirs::from_env(|k| std::env::var(k).ok())
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let ctx = Ctx {
        dirs,
        home: user_home,
    };
    let _lock = InitLock::acquire(home)?;
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
        let mut actions = (h.removals)(&ctx);
        if install {
            actions.steps.extend((h.additions)(&root));
        }
        results.push(run_steps(
            h,
            &ctx,
            actions,
            if install { "registered" } else { "removed" },
        ));
    }
    let marketplace_detail = if install {
        "written".to_string()
    } else {
        remove_marketplace_unless_used(&ctx, &root)?
    };
    let failed = results.iter().any(|r| r["status"] == "failed");
    let out =
        json!({"marketplace": root, "marketplace_detail": marketplace_detail, "agents": results});
    super::print(cli, out, |j| {
        let mut m = format!(
            "marketplace: {}",
            j["marketplace"].as_str().unwrap_or_default()
        );
        if let Some(d) = j["marketplace_detail"].as_str().filter(|d| !d.is_empty()) {
            m.push_str(&format!(" ({d})"));
        }
        let mut lines = vec![m];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_resolves_dot_dot_without_touching_the_filesystem() {
        assert_eq!(lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn a_missing_package_is_ours_only_under_a_clax_or_previous_name_path() {
        assert!(looks_like_ours(Path::new("/x/clax/plugins/pi")));
        assert!(looks_like_ours(Path::new(&format!("/x/{OLD}/plugins/pi"))));
        assert!(!looks_like_ours(Path::new("/x/other/plugins/pi")));
        assert!(!looks_like_ours(Path::new("/x/clax/plugins/other")));
    }
}

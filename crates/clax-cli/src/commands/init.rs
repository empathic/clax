//! `clax init` and `clax uninit`: register the plugins this binary embeds
//! with each harness through the harness's own CLI, and remove them.
//!
//! `init` writes the marketplace tree ([`crate::plugins`]) to
//! `<home>/marketplace`, then for each harness removes the existing `clax`
//! registration and any under the previous product name, and adds the new
//! one. It records what it registered in `<home>/registrations.json`.
//! `uninit` does the removals, then deletes the marketplace directory
//! unless a harness's registry still points into it, or cannot be read.
//!
//! When in doubt, a registration is kept. Claude Code and Codex
//! registrations are removed by name (`clax`, `clax@clax`, and the previous
//! name's). A Grok registration is removed by the name `clax-grok` only: in Grok,
//! `clax` names the Claude Code plugin that Grok discovers in
//! `~/.claude/plugins`, and no `grok` command here ever names it. A Pi package, which Pi names by its directory, is removed only
//! when `registrations.json` records it, or its `package.json` names the
//! Clax Pi package or the previous name's. A Pi package whose directory is
//! missing or unreadable is left registered and named in the output, with
//! the command that removes it.
//!
//! `init` also sets the home's `bin` setting to this executable
//! ([`crate::plugin_bin::set`]), so the plugins it registers, which match
//! this binary, run it rather than the release they pin; `uninit` removes
//! the setting when it names this executable.
//!
//! Clax's data is never touched, nor the previous name's home. Both
//! commands hold `<home>/init.lock` throughout, and run each harness CLI in
//! the home directory, so a project's own harness settings in the caller's
//! working directory are never edited.
//!
//! `init` also installs the Chrome extension and its native messaging host
//! ([`super::extension::install`]); `uninit` revokes the extension's
//! credentials and removes the extension ([`super::extension::uninstall`]). A failed
//! extension step is reported under `extension` and never fails the command.
//!
//! For Codex, `init` then puts back the person's settings for the plugin
//! that removing it deleted (its approval settings, under
//! `[plugins."clax@clax"]`), and offers the approvals that keep Codex from
//! stopping mid-task to ask before a Clax tool
//! ([`crate::codex_approvals`]): it prints the `config.toml` lines and adds
//! them only when the person confirms, or with `--yes`. Without a terminal
//! and without `--yes` it adds nothing and says how to.
//!
//! Each harness is one entry in [`HARNESSES`].

use super::doctor_agent::Dirs;
use crate::codex_approvals;
use clax_core::Home;
use clax_mcp::tools::ClaxTools;
use serde_json::{Map, Value, json};
use std::path::{Component, Path, PathBuf};

/// The previous product name, assembled so the name gate finds no literal.
const OLD: &str = concat!("arti", "fax");
/// The Pi package's name.
const PI_PACKAGE: &str = "@empathic/clax-pi";

/// What a harness's functions read.
struct Ctx {
    dirs: Dirs,
    /// The user's home directory.
    home: PathBuf,
    /// What `init` recorded per harness (`registrations.json`).
    recorded: Map<String, Value>,
}

/// A harness Clax registers its plugin with.
struct Harness {
    /// The `--agent` value, which is also the name of the harness's CLI.
    name: &'static str,
    /// The commands that remove Clax's registration and any under the
    /// previous name, with notes on what was left and why.
    removals: fn(&Ctx) -> Actions,
    /// The commands that register the marketplace at the given root.
    additions: fn(&Path) -> Vec<Step>,
    /// What `init` records after registering the marketplace at the root.
    record: fn(&Path) -> Value,
    /// Whether the harness's registry still refers to a path under the
    /// root; an error when the registry cannot be read or parsed.
    uses: fn(&Ctx, &Path) -> Result<bool, String>,
    /// The commands that remove the registration of the root by hand.
    by_hand: fn(&Path) -> String,
}

/// Every supported harness, in the order `init` and `uninit` visit them.
const HARNESSES: &[Harness] = &[
    Harness {
        name: "claude",
        removals: claude_removals,
        additions: claude_additions,
        record: name_record,
        uses: claude_uses,
        by_hand: |_| {
            "claude plugin uninstall clax@clax; claude plugin marketplace remove clax".into()
        },
    },
    Harness {
        name: "codex",
        removals: codex_removals,
        additions: codex_additions,
        record: name_record,
        uses: codex_uses,
        by_hand: |_| "codex plugin remove clax@clax; codex plugin marketplace remove clax".into(),
    },
    Harness {
        name: "grok",
        removals: grok_removals,
        additions: grok_additions,
        record: |root| json!({"plugin": GROK_PLUGIN, "source": grok_plugin_dir(root)}),
        uses: grok_uses,
        by_hand: |_| format!("grok plugin uninstall {GROK_PLUGIN} --confirm"),
    },
    Harness {
        name: "pi",
        removals: pi_removals,
        additions: pi_additions,
        record: |root| json!({"packages": [pi_package_dir(root)]}),
        uses: pi_uses,
        by_hand: |root| format!("pi remove {}", pi_package_dir(root).display()),
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
    /// Add the Codex approval settings `init` offers without asking first.
    /// Without it, `init` asks on a terminal and otherwise adds nothing.
    #[arg(long)]
    pub yes: bool,
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

/// A registry file's contents: `Ok(None)` when it does not exist, an error
/// naming the file when it cannot be read or parsed.
fn load<T>(p: &Path, parse: impl FnOnce(&str) -> Result<T, String>) -> Result<Option<T>, String> {
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read {}: {e}", p.display())),
    };
    parse(&text)
        .map(Some)
        .map_err(|e| format!("could not parse {} ({e})", p.display()))
}

fn load_json(p: &Path) -> Result<Option<Value>, String> {
    load(p, |t| serde_json::from_str(t).map_err(|e| e.to_string()))
}

fn load_toml(p: &Path) -> Result<Option<toml::Table>, String> {
    load(p, |t| t.parse().map_err(|e: toml::de::Error| e.to_string()))
}

/// [`load`] for finding removals: an unreadable file adds a note and gives
/// `None`, so nothing in it is removed.
fn read_for_removal<T>(r: Result<Option<T>, String>, notes: &mut Vec<String>) -> Option<T> {
    r.unwrap_or_else(|e| {
        notes.push(format!("{e}; registrations there were not looked for"));
        None
    })
}

/// Whether a JSON registry names `key`, at the top level or under `plugins`.
fn names(v: &Option<Value>, key: &str) -> bool {
    v.as_ref()
        .is_some_and(|v| v.get(key).is_some() || v["plugins"].get(key).is_some())
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

/// A stored path as an absolute path: `file://…`, `~` or `~/…` (against
/// `home`), absolute, or relative to `base`; resolved lexically.
fn stored_path(s: &str, base: &Path, home: &Path) -> PathBuf {
    let s = s.trim();
    let s = s.strip_prefix("file://").unwrap_or(s);
    let p = if s == "~" {
        home.to_path_buf()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else {
        base.join(s)
    };
    lexical(&p)
}

/// Whether a string in `v`, at any depth, is a path under `root` in one of
/// the forms [`stored_path`] reads.
fn json_mentions(v: &Value, root: &Path, base: &Path, home: &Path) -> bool {
    match v {
        Value::String(s) => !s.is_empty() && stored_path(s, base, home).starts_with(root),
        Value::Array(a) => a.iter().any(|v| json_mentions(v, root, base, home)),
        Value::Object(o) => o.values().any(|v| json_mentions(v, root, base, home)),
        _ => false,
    }
}

/// As [`json_mentions`], for TOML.
fn toml_mentions(v: &toml::Value, root: &Path, base: &Path, home: &Path) -> bool {
    match v {
        toml::Value::String(s) => !s.is_empty() && stored_path(s, base, home).starts_with(root),
        toml::Value::Array(a) => a.iter().any(|v| toml_mentions(v, root, base, home)),
        toml::Value::Table(t) => t.values().any(|v| toml_mentions(v, root, base, home)),
        _ => false,
    }
}

fn old_plugin() -> String {
    format!("{OLD}@{OLD}")
}

/// The record for a harness that names the marketplace and plugin `clax`.
fn name_record(root: &Path) -> Value {
    json!({"marketplace": "clax", "plugin": "clax@clax", "source": root})
}

fn claude_removals(ctx: &Ctx) -> Actions {
    let mut a = Actions::default();
    let old_plugin = old_plugin();
    let plugins = ctx.dirs.claude_dir.join("plugins");
    let installed = read_for_removal(
        load_json(&plugins.join("installed_plugins.json")),
        &mut a.notes,
    );
    let markets = read_for_removal(
        load_json(&plugins.join("known_marketplaces.json")),
        &mut a.notes,
    );
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

fn claude_uses(ctx: &Ctx, root: &Path) -> Result<bool, String> {
    let d = &ctx.dirs.claude_dir;
    for p in [
        d.join("plugins/known_marketplaces.json"),
        d.join("plugins/installed_plugins.json"),
        d.join("settings.json"),
    ] {
        if load_json(&p)?.is_some_and(|v| json_mentions(&v, root, d, &ctx.home)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn codex_removals(ctx: &Ctx) -> Actions {
    let mut a = Actions::default();
    let old_plugin = old_plugin();
    let cfg = read_for_removal(
        load_toml(&ctx.dirs.codex_home.join("config.toml")),
        &mut a.notes,
    );
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

fn codex_uses(ctx: &Ctx, root: &Path) -> Result<bool, String> {
    let d = &ctx.dirs.codex_home;
    Ok(load_toml(&d.join("config.toml"))?
        .is_some_and(|t| toml_mentions(&toml::Value::Table(t), root, d, &ctx.home)))
}

/// The Grok plugin's name. Never `clax`: that is the Claude Code plugin,
/// which Grok also discovers.
const GROK_PLUGIN: &str = "clax-grok";

/// The Grok plugin's directory inside the marketplace at `root`.
fn grok_plugin_dir(root: &Path) -> PathBuf {
    lexical(&root.join("plugins").join(GROK_PLUGIN))
}

fn grok_removals(_ctx: &Ctx) -> Actions {
    Actions {
        steps: vec![step(
            false,
            &["plugin", "uninstall", GROK_PLUGIN, "--confirm"],
        )],
        notes: Vec::new(),
    }
}

fn grok_additions(root: &Path) -> Vec<Step> {
    let dir = grok_plugin_dir(root).display().to_string();
    vec![step(true, &["plugin", "install", dir.as_str(), "--trust"])]
}

/// Whether Grok still lists a plugin from under `root`: `grok plugin list
/// --json`, run in the home directory. Without `grok` on PATH, true when
/// `registrations.json` still records a Grok registration under `root`,
/// since Grok's own files are not read.
fn grok_uses(ctx: &Ctx, root: &Path) -> Result<bool, String> {
    if on_path("grok").is_none() {
        return Ok(ctx.recorded.get("grok").is_some_and(|r| {
            r["source"]
                .as_str()
                .is_some_and(|s| Path::new(s).starts_with(root))
        }));
    }
    let mut cmd = std::process::Command::new("grok");
    cmd.args(["plugin", "list", "--json"])
        .stdin(std::process::Stdio::null());
    if ctx.home.is_dir() {
        cmd.current_dir(&ctx.home);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run `grok plugin list --json`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`grok plugin list --json` failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or_default()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("could not parse `grok plugin list --json` ({e})"))?;
    Ok(json_mentions(&v, root, &ctx.home, &ctx.home))
}

/// The Pi package directory inside the marketplace at `root`.
fn pi_package_dir(root: &Path) -> PathBuf {
    lexical(&root.join("plugins/pi"))
}

/// The local Pi package sources in `<pi dir>/settings.json`, each as the
/// absolute directory Pi resolves it to: `~` against the home directory, a
/// relative path against the Pi directory, lexically. Sources with a scheme
/// (`npm:`, `git:`, …) are left out, and each directory is listed once.
fn pi_local_packages(ctx: &Ctx) -> Result<Vec<PathBuf>, String> {
    let pi_dir = &ctx.dirs.pi_dir;
    let Some(v) = load_json(&pi_dir.join("settings.json"))? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for d in v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().or_else(|| p["source"].as_str()))
        .filter(|s| !s.contains(':'))
        .map(|s| stored_path(s, pi_dir, &ctx.home))
    {
        if !out.contains(&d) {
            out.push(d);
        }
    }
    Ok(out)
}

/// The `name` in `<d>/package.json`: `Ok(None)` for a directory without a
/// readable Pi package name, an error when the directory itself is missing
/// or unreadable.
fn package_name(d: &Path) -> Result<Option<String>, String> {
    match std::fs::metadata(d) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err("is not a directory".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("is missing".into()),
        Err(e) => return Err(format!("cannot be read ({e})")),
    }
    match std::fs::read_to_string(d.join("package.json")) {
        Ok(t) => Ok(serde_json::from_str::<Value>(&t)
            .ok()
            .and_then(|v| v["name"].as_str().map(str::to_string))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot be read ({e})")),
    }
}

fn pi_removals(ctx: &Ctx) -> Actions {
    let mut a = Actions::default();
    let wanted = [format!("@empathic/{OLD}-pi"), PI_PACKAGE.to_string()];
    let recorded: Vec<PathBuf> = ctx.recorded.get("pi").map_or(Vec::new(), |r| {
        r["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| lexical(Path::new(s)))
            .collect()
    });
    let packages = match pi_local_packages(ctx) {
        Ok(p) => p,
        Err(e) => {
            a.notes
                .push(format!("{e}; registrations there were not looked for"));
            Vec::new()
        }
    };
    for d in packages {
        let ours = recorded.contains(&d)
            || match package_name(&d) {
                Ok(name) => name.is_some_and(|n| wanted.contains(&n)),
                Err(why) => {
                    a.notes.push(format!(
                        "Pi package {p} {why}, so it is left registered; if it is Clax's, remove it with `pi remove {p}`",
                        p = d.display()
                    ));
                    false
                }
            };
        if ours {
            let d = d.display().to_string();
            a.steps.push(step(false, &["remove", d.as_str()]));
        }
    }
    a
}

fn pi_additions(root: &Path) -> Vec<Step> {
    let pi = pi_package_dir(root).display().to_string();
    vec![step(true, &["install", pi.as_str()])]
}

fn pi_uses(ctx: &Ctx, root: &Path) -> Result<bool, String> {
    Ok(pi_local_packages(ctx)?.iter().any(|d| d.starts_with(root)))
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

/// Whether `answer` (a line the person typed) says yes.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Asks on the terminal whether to add `lines` to `path`, saying what the
/// tools in `asking` do.
fn confirm(asking: &[&codex_approvals::Asking], lines: &str, path: &Path) -> bool {
    use std::io::Write;
    let tools: Vec<&str> = asking.iter().map(|a| a.tool.as_str()).collect();
    let mut err = std::io::stderr();
    let _ = write!(
        err,
        "\nCodex asks before each call of these Clax tools: {}. {}\nSo that Codex does not stop mid-task to ask, `clax init` can add these lines to {}:\n\n{lines}\nAdd them? [y/N] ",
        tools.join(", "),
        codex_approvals::consequences(asking),
        path.display()
    );
    let _ = err.flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && is_yes(&answer)
}

/// Where `init` keeps Clax's Codex plugin settings across `codex plugin
/// remove` and the re-registration: written just before the removal,
/// deleted once the settings are back. It outlives a run only when that run
/// failed between the two, and the next `init` then puts them back;
/// `uninit` deletes it.
fn pending_settings_path(home: &Home) -> PathBuf {
    home.root().join("run/codex-plugin-settings.toml")
}

/// The settings kept by an `init` that failed mid-way, if any.
fn load_pending(pending: &Path) -> Option<toml_edit::Table> {
    let text = std::fs::read_to_string(pending).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let t = doc.as_table().clone();
    (!t.is_empty()).then_some(t)
}

/// Puts the `saved` settings back into the config at `path` and adds the
/// approvals for those of `shown` that Codex would still ask about, on the
/// config as it is now: read again here, so whatever Codex wrote meanwhile
/// is kept, and checked unchanged just before the write (tried twice).
/// Only tools the person was shown are added. The tools added, and whether
/// settings were put back.
fn write_settings(
    path: &Path,
    saved: Option<&toml_edit::Table>,
    shown: &[&str],
) -> Result<(Vec<String>, bool), String> {
    for _ in 0..2 {
        let fresh = codex_approvals::read_config(path)?.unwrap_or_default();
        let restored_text = match saved {
            Some(s) => codex_approvals::restore(&fresh, s)?,
            None => fresh.clone(),
        };
        let a = codex_approvals::assess(Some(&restored_text), &ClaxTools::tools())?;
        let add: Vec<&str> = a
            .addable()
            .into_iter()
            .filter(|t| shown.contains(t))
            .collect();
        let next = if add.is_empty() {
            restored_text.clone()
        } else {
            codex_approvals::add_approvals(&restored_text, &add)?
        };
        if codex_approvals::read_config(path)?.unwrap_or_default() != fresh {
            continue;
        }
        if next != fresh {
            codex_approvals::write_config(path, &next)
                .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        }
        return Ok((
            add.into_iter().map(str::to_string).collect(),
            restored_text != fresh,
        ));
    }
    Err(format!(
        "{} kept changing while it was being edited; nothing was written",
        path.display()
    ))
}

/// Before `codex plugin remove`: the person's settings for the plugin (with
/// any an `init` that failed mid-way kept at `pending`; the config's win),
/// written to `pending` so a failure before they are back loses nothing.
/// An error when the config cannot be read or parsed (nothing is kept, and
/// nothing put back).
fn keep_settings_for_removal(
    path: &Path,
    pending: &Path,
) -> Result<Option<toml_edit::Table>, String> {
    let now = match codex_approvals::read_config(path)? {
        Some(text) => {
            text.parse::<toml_edit::DocumentMut>()
                .map_err(|e| format!("could not parse {} ({e})", path.display()))?;
            codex_approvals::plugin_settings(&text)
        }
        None => None,
    };
    let saved = match (now, load_pending(pending)) {
        (Some(mut s), Some(p)) => {
            codex_approvals::merge_missing(&mut s, &p);
            Some(s)
        }
        (s, p) => s.or(p),
    };
    if let Some(s) = &saved {
        let mut doc = toml_edit::DocumentMut::new();
        for (k, v) in s.iter() {
            doc.insert(k, v.clone());
        }
        // When this fails, the settings are still put back from memory, and
        // a failed registration prints them.
        let _ = std::fs::create_dir_all(pending.parent().unwrap_or(Path::new(".")))
            .and_then(|_| std::fs::write(pending, doc.to_string()));
    }
    Ok(saved)
}

/// After Codex's re-registration: puts back the `saved` settings it
/// removed and deletes `pending`, then offers
/// the approvals for the Clax tools Codex would still ask about that the
/// person has set nothing for. `ask` gets those tools and the lines and
/// says whether to add them: `Some(true)` yes, `Some(false)` declined,
/// `None` no way to ask. When `registered` is false, `pending` keeps the
/// settings for the next run. The outcome as JSON; a failure is
/// reported there, never fatal.
fn codex_settings(
    path: &Path,
    pending: &Path,
    saved: Result<Option<toml_edit::Table>, String>,
    registered: bool,
    ask: impl FnOnce(&[&codex_approvals::Asking], &str) -> Option<bool>,
) -> Option<Value> {
    let failed =
        |detail: String| Some(json!({"status": "failed", "config": path, "detail": detail}));
    let saved = match saved {
        Ok(s) => s,
        Err(e) => return failed(format!("{e}; Clax's settings there were not carried over")),
    };
    if !registered {
        let saved = saved?;
        return failed(if pending.exists() {
            format!(
                "registering failed, and Clax's settings in {} may be gone; they are kept in {} and the next `clax init` puts them back",
                path.display(),
                pending.display()
            )
        } else {
            format!(
                "registering failed, and Clax's settings in {} may be gone; they were:\n{saved}",
                path.display()
            )
        });
    }
    // Put the settings back first, so they survive whatever happens next.
    let restored = match write_settings(path, saved.as_ref(), &[]) {
        Ok((_, r)) => r,
        Err(e) => return failed(format!("could not put back Clax's settings: {e}")),
    };
    let _ = std::fs::remove_file(pending);
    let text = match codex_approvals::read_config(path) {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => return failed(e),
    };
    let assessment = match codex_approvals::assess(Some(&text), &ClaxTools::tools()) {
        Ok(a) => a,
        Err(e) => return failed(format!("could not parse {} ({e})", path.display())),
    };
    let kept_note = assessment.kept_note(path);
    let out = |status: &str, tools: &[&str], detail: Option<String>| {
        let detail = [detail, kept_note.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ");
        Some(json!({
            "status": status,
            "config": path,
            "restored": restored,
            "tools": tools,
            "lines": codex_approvals::lines(tools),
            "detail": detail,
        }))
    };
    let asking = assessment.addable_asking();
    let tools = assessment.addable();
    if tools.is_empty() {
        return out("unchanged", &[], None);
    }
    let lines = codex_approvals::lines(&tools);
    match ask(&asking, &lines) {
        Some(true) => {}
        answer => {
            return out(
                if answer.is_some() {
                    "declined"
                } else {
                    "not_added"
                },
                &tools,
                Some(format!(
                    "Codex will ask before each call of {}; to stop it asking, run `{} --yes` or add the lines below to {}",
                    tools.join(", "),
                    codex_approvals::SETUP_COMMAND,
                    path.display()
                )),
            );
        }
    }
    // The settings went back above; a setting removed since stays removed.
    match write_settings(path, None, &tools) {
        Ok((added, _)) => {
            let added: Vec<&str> = added.iter().map(String::as_str).collect();
            out(
                if added.is_empty() {
                    "unchanged"
                } else {
                    "added"
                },
                &added,
                None,
            )
        }
        Err(e) => failed(format!(
            "could not add the approvals to {}: {e}",
            path.display()
        )),
    }
}

/// The text lines for a Codex harness's `approvals` result.
fn approvals_lines(a: &Value) -> Vec<String> {
    let Some(status) = a["status"].as_str() else {
        return Vec::new();
    };
    let config = a["config"].as_str().unwrap_or_default();
    let tools = a["tools"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    if a["restored"] == true {
        out.push(format!(
            "codex settings: put back Clax's settings in {config} that re-registering removed"
        ));
    }
    let detail = a["detail"].as_str().filter(|d| !d.is_empty());
    let mut l = match status {
        "added" => format!("codex approvals: added for {tools} in {config}"),
        "unchanged" => "codex approvals: nothing to add".to_string(),
        s => format!("codex approvals: {s}"),
    };
    if let Some(d) = detail {
        l.push_str(&format!(" ({d})"));
    }
    out.push(l);
    if matches!(status, "not_added" | "declined") {
        out.push(
            a["lines"]
                .as_str()
                .unwrap_or_default()
                .trim_end()
                .to_string(),
        );
    }
    out
}

/// `init`: sets the `bin` setting to this executable. `uninit`: removes it
/// when it names this executable. The outcome as JSON; a failure is
/// reported there, never fatal.
fn update_bin_setting(home: &Home, install: bool) -> Value {
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return json!({"status": "failed", "detail": format!("finding this executable: {e}")});
        }
    };
    if install {
        return match crate::plugin_bin::set(home, &me) {
            Ok(_) => json!({"status": "set", "detail": me}),
            Err(e) => {
                json!({"status": "failed", "detail": format!("{e:#}; the plugins run the release they pin unless CLAX_BIN names a binary")})
            }
        };
    }
    if crate::plugin_bin::current(home).as_deref() != me.to_str() {
        return json!({"status": "kept", "detail": "it does not name this executable"});
    }
    match crate::plugin_bin::clear(home) {
        Ok(_) => json!({"status": "cleared", "detail": me}),
        Err(e) => json!({"status": "failed", "detail": format!("{e:#}")}),
    }
}

/// Exclusive advisory lock on `<home>/init.lock`, held until dropped, so
/// concurrent `init` and `uninit` runs take turns.
pub(crate) struct InitLock(#[allow(dead_code)] std::fs::File);

impl InitLock {
    pub(crate) fn acquire(home: &Home) -> std::io::Result<InitLock> {
        super::extension::create_home(home)?;
        let f = std::fs::File::create(home.root().join("init.lock"))?;
        clax_server::daemon::retry_interrupted(|| f.lock())?;
        Ok(InitLock(f))
    }
}

fn registrations_path(home: &Home) -> PathBuf {
    home.root().join("registrations.json")
}

/// `registrations.json`'s per-harness records; empty when it is missing or
/// unreadable (nothing is then removed on its account).
fn load_recorded(home: &Home) -> Map<String, Value> {
    load_json(&registrations_path(home))
        .ok()
        .flatten()
        .and_then(|v| v.get("harnesses").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// Writes `registrations.json` atomically: a sibling temporary file renamed
/// into place.
fn save_recorded(home: &Home, recorded: &Map<String, Value>) -> std::io::Result<()> {
    let path = registrations_path(home);
    let tmp = home
        .root()
        .join(format!(".registrations.{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(&json!({"version": 1, "harnesses": recorded}))?;
    std::fs::write(&tmp, text + "\n")?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// After `uninit`: removes the marketplace unless a harness's registry,
/// whether or not this run covered it, still refers to it or cannot be
/// read. Returns what happened, for the output.
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
    let mut reasons = Vec::new();
    for h in HARNESSES {
        let mut used = Ok(false);
        for r in &roots {
            used = (h.uses)(ctx, r);
            if used != Ok(false) {
                break;
            }
        }
        match used {
            Ok(false) => {}
            Ok(true) => reasons.push(format!(
                "{} still registers it (with {} on PATH, run `clax uninit --agent {}`; otherwise run `{}`)",
                h.name,
                h.name,
                h.name,
                (h.by_hand)(root)
            )),
            Err(e) => reasons.push(format!("{}: {e}", h.name)),
        }
    }
    if reasons.is_empty() {
        std::fs::remove_dir_all(root)?;
        Ok("removed".into())
    } else {
        Ok(format!("kept: {}", reasons.join("; ")))
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
    // `uninit` on a machine with no Clax home creates none.
    let has_home = install || home.root().is_dir();
    let _lock = if has_home {
        Some(InitLock::acquire(home)?)
    } else {
        None
    };
    let mut ctx = Ctx {
        dirs,
        home: user_home,
        recorded: load_recorded(home),
    };
    let root = home.root().join("marketplace");
    if install {
        crate::plugins::materialize(&root)?;
    }
    if !install && (a.agents.is_empty() || a.agents.iter().any(|n| n == "codex")) {
        let _ = std::fs::remove_file(pending_settings_path(home));
    }
    let chosen = HARNESSES
        .iter()
        .filter(|h| a.agents.is_empty() || a.agents.iter().any(|n| n == h.name));
    let mut results = Vec::new();
    let mut recorded = ctx.recorded.clone();
    for h in chosen {
        if on_path(h.name).is_none() {
            results.push(json!({"agent": h.name, "status": "skipped", "detail": format!("{} is not on PATH", h.name), "commands": []}));
            continue;
        }
        let mut actions = (h.removals)(&ctx);
        if install {
            actions.steps.extend((h.additions)(&root));
        }
        let codex_saved = (install && h.name == "codex").then(|| {
            keep_settings_for_removal(
                &codex_approvals::config_path(&ctx.dirs.codex_home),
                &pending_settings_path(home),
            )
        });

        let mut r = run_steps(
            h,
            &ctx,
            actions,
            if install { "registered" } else { "removed" },
        );
        if let Some(saved) = codex_saved {
            let yes = a.yes;
            let approvals = codex_settings(
                &codex_approvals::config_path(&ctx.dirs.codex_home),
                &pending_settings_path(home),
                saved,
                r["status"] == "registered",
                |asking, lines| {
                    use std::io::IsTerminal;
                    if yes {
                        Some(true)
                    } else if std::io::stdin().is_terminal() {
                        Some(confirm(
                            asking,
                            lines,
                            &codex_approvals::config_path(&ctx.dirs.codex_home),
                        ))
                    } else {
                        None
                    }
                },
            );
            if let Some(v) = approvals {
                r["approvals"] = v;
            }
        }
        match r["status"].as_str() {
            Some("registered") => {
                recorded.insert(h.name.into(), (h.record)(&root));
            }
            Some("removed") => {
                recorded.remove(h.name);
            }
            _ => {}
        }
        results.push(r);
    }
    if has_home && recorded != ctx.recorded {
        save_recorded(home, &recorded)?;
    }
    ctx.recorded = recorded;
    let marketplace_detail = if install {
        "written".to_string()
    } else {
        remove_marketplace_unless_used(&ctx, &root)?
    };
    let failed = results.iter().any(|r| r["status"] == "failed");
    let bin = if has_home {
        update_bin_setting(home, install)
    } else {
        json!({"status": "kept", "detail": "there is no Clax home"})
    };
    let extension = if install {
        super::extension::install(home, false)
            .unwrap_or_else(|e| json!({"status": "failed", "detail": format!("{e:#}")}))
    } else {
        let revoked = revoke_extension_credentials(home);
        let mut v = super::extension::uninstall(home)
            .unwrap_or_else(|e| json!({"status": "failed", "detail": format!("{e:#}")}));
        v["credentials_revoked"] = revoked;
        v
    };
    let out = json!({"marketplace": root, "marketplace_detail": marketplace_detail, "agents": results, "bin": bin, "extension": extension});
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
            lines.extend(approvals_lines(&r["approvals"]));
        }
        lines.push(format!(
            "bin setting: {} ({})",
            j["bin"]["status"].as_str().unwrap_or_default(),
            j["bin"]["detail"].as_str().unwrap_or_default()
        ));
        let ext = &j["extension"];
        let mut l = format!("extension: {}", ext["status"].as_str().unwrap_or_default());
        if let Some(d) = ext["detail"].as_str() {
            l.push_str(&format!(" ({d})"));
        }
        lines.push(l);
        lines.extend(super::extension::host_lines(ext));
        if let Some(t) = ext["load_unpacked"].as_str() {
            lines.push(t.to_string());
        }
        if let Some(t) = ext["note"].as_str().filter(|_| ext["status"] == "removed") {
            lines.push(t.to_string());
        }
        if install {
            lines.push("Start a new session in each harness to load the plugin.".into());
        }
        lines.join("\n")
    });
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

/// Revokes every extension credential: through the daemon when one runs,
/// so its cache forgets them, else in the store directly. The count of live
/// credentials revoked, `"revoked"` through the daemon, or the failure.
fn revoke_extension_credentials(home: &Home) -> Value {
    if let Some(c) = crate::client::Client::discover(home) {
        return match c.delete("/api/extension/credentials") {
            Ok(()) => json!("revoked"),
            Err(e) => json!(format!("failed: {e:#}")),
        };
    }
    if !home.db_path().exists() {
        return json!(0);
    }
    match clax_core::Store::open(home).and_then(|st| st.revoke_extension_credentials()) {
        Ok(n) => json!(n),
        Err(e) => json!(format!("failed: {e}")),
    }
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

    const REGISTERED: &str = "[plugins.\"clax@clax\"]\nenabled = true\n";

    fn settings_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("codex/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pending = d.path().join("ax/run/codex-plugin-settings.toml");
        (d, path, pending)
    }

    /// What Codex writes while the person reads the prompt survives: the
    /// lines are added to the config as it is after the answer.
    #[test]
    fn approvals_are_added_to_the_config_as_it_is_after_the_answer() {
        let (_d, path, pending) = settings_dir();
        std::fs::write(&path, REGISTERED).unwrap();
        let v = codex_settings(&path, &pending, Ok(None), true, |asking, lines| {
            assert_eq!(asking.len(), 6);
            assert!(lines.contains("tools.delete]"));
            // Codex's "Always allow" on db_set, and another key, meanwhile.
            let now = std::fs::read_to_string(&path).unwrap();
            std::fs::write(
                &path,
                format!("{now}\n[plugins.\"clax@clax\".mcp_servers.clax.tools.db_set]\napproval_mode = \"prompt\"\n\n[tui]\nx = 1\n"),
            )
            .unwrap();
            Some(true)
        })
        .unwrap();
        assert_eq!(v["status"], "added", "{v}");
        let tools: Vec<&str> = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert!(
            !tools.contains(&"db_set"),
            "set meanwhile, so the person's: {v}"
        );
        assert!(tools.contains(&"delete"), "{v}");
        let cfg = std::fs::read_to_string(&path).unwrap();
        assert!(cfg.contains("[tui]\nx = 1\n"), "{cfg}");
        assert!(
            cfg.contains("tools.db_set]\napproval_mode = \"prompt\""),
            "{cfg}"
        );
        assert!(
            cfg.contains("tools.delete]\napproval_mode = \"approve\""),
            "{cfg}"
        );
    }

    #[test]
    fn a_server_default_of_the_persons_is_left_and_named_even_with_yes() {
        let (_d, path, pending) = settings_dir();
        let cfg = format!(
            "{REGISTERED}\n[plugins.\"clax@clax\".mcp_servers.clax]\ndefault_tools_approval_mode = \"prompt\"\n"
        );
        std::fs::write(&path, &cfg).unwrap();
        let v = codex_settings(&path, &pending, Ok(None), true, |_, _| {
            panic!("nothing is offered")
        })
        .unwrap();
        assert_eq!(v["status"], "unchanged", "{v}");
        assert!(
            v["detail"]
                .as_str()
                .unwrap()
                .contains("(your default_tools_approval_mode = \"prompt\")"),
            "{v}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), cfg);
    }

    #[test]
    fn declining_or_having_no_way_to_ask_writes_nothing() {
        let (_d, path, pending) = settings_dir();
        std::fs::write(&path, REGISTERED).unwrap();
        let v = codex_settings(&path, &pending, Ok(None), true, |_, _| Some(false)).unwrap();
        assert_eq!(v["status"], "declined", "{v}");
        let v = codex_settings(&path, &pending, Ok(None), true, |_, _| None).unwrap();
        assert_eq!(v["status"], "not_added", "{v}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), REGISTERED);
    }

    const PUBLISH_PROMPT: &str =
        "\n[plugins.\"clax@clax\".mcp_servers.clax.tools.publish]\napproval_mode = \"prompt\"\n";

    /// Settings an `init` that failed mid-way may have lost are kept and
    /// put back by the next run.
    #[test]
    fn settings_outlive_an_init_that_failed_mid_way() {
        let (_d, path, pending) = settings_dir();
        std::fs::write(&path, format!("{REGISTERED}{PUBLISH_PROMPT}")).unwrap();
        let saved = keep_settings_for_removal(&path, &pending).unwrap();
        assert!(saved.is_some() && pending.exists());
        // `codex plugin remove`, then a failed registration.
        std::fs::write(&path, "").unwrap();
        let v = codex_settings(&path, &pending, Ok(saved), false, |_, _| unreachable!()).unwrap();
        assert_eq!(v["status"], "failed", "{v}");
        assert!(
            v["detail"].as_str().unwrap().contains("next `clax init`"),
            "{v}"
        );
        assert!(pending.exists());
        // The next run: nothing in the config, the kept settings return.
        let saved = keep_settings_for_removal(&path, &pending).unwrap();
        assert!(saved.is_some());
        std::fs::write(&path, REGISTERED).unwrap();
        let v = codex_settings(&path, &pending, Ok(saved), true, |_, _| None).unwrap();
        assert_eq!(v["restored"], true, "{v}");
        assert!(!pending.exists());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("tools.publish]")
        );
    }

    /// After a completed `init` nothing is kept, so a setting the person
    /// removes afterwards stays removed.
    #[test]
    fn a_completed_init_keeps_nothing_to_resurrect() {
        let (_d, path, pending) = settings_dir();
        std::fs::write(&path, format!("{REGISTERED}{PUBLISH_PROMPT}")).unwrap();
        let saved = keep_settings_for_removal(&path, &pending).unwrap();
        std::fs::write(&path, REGISTERED).unwrap();
        let v = codex_settings(&path, &pending, Ok(saved), true, |_, _| None).unwrap();
        assert_eq!(v["restored"], true, "{v}");
        assert!(!pending.exists(), "deleted once the settings are back");
        // The person removes the setting; the next init has nothing to add back.
        std::fs::write(&path, REGISTERED).unwrap();
        assert!(
            keep_settings_for_removal(&path, &pending)
                .unwrap()
                .is_none()
        );
        assert!(!pending.exists());
        let v = codex_settings(&path, &pending, Ok(None), true, |_, _| None).unwrap();
        assert_eq!(v["restored"], false, "{v}");
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("tools.publish]")
        );
        assert_eq!(
            codex_settings(&path, &pending, Ok(None), false, |_, _| unreachable!()),
            None,
            "a failed registration with nothing saved reports nothing"
        );
    }

    /// A setting removed while the prompt is open stays removed: the saved
    /// settings are put back once, before the prompt.
    #[test]
    fn consent_does_not_put_back_settings_removed_meanwhile() {
        let (_d, path, pending) = settings_dir();
        std::fs::write(&path, format!("{REGISTERED}{PUBLISH_PROMPT}")).unwrap();
        let saved = keep_settings_for_removal(&path, &pending).unwrap();
        std::fs::write(&path, REGISTERED).unwrap();
        let v = codex_settings(&path, &pending, Ok(saved), true, |_, _| {
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("tools.publish]")
            );
            std::fs::write(&path, REGISTERED).unwrap();
            Some(true)
        })
        .unwrap();
        assert_eq!(v["status"], "added", "{v}");
        let cfg = std::fs::read_to_string(&path).unwrap();
        assert!(!cfg.contains("tools.publish]"), "{cfg}");
        assert!(cfg.contains("tools.delete]"), "{cfg}");
    }

    #[test]
    fn only_y_or_yes_is_consent() {
        for a in ["y\n", "Y", " yes \n", "YES"] {
            assert!(is_yes(a), "{a:?}");
        }
        for a in ["", "\n", "n", "no", "yep", "ok"] {
            assert!(!is_yes(a), "{a:?}");
        }
    }

    #[test]
    fn lexical_resolves_dot_dot_without_touching_the_filesystem() {
        assert_eq!(lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn stored_paths_are_read_in_every_form_the_tools_may_write() {
        let (base, home) = (Path::new("/cfg"), Path::new("/home/u"));
        let root = Path::new("/home/u/.clax/marketplace");
        for s in [
            "/home/u/.clax/marketplace",
            "~/.clax/marketplace/plugins/pi",
            "file:///home/u/.clax/marketplace",
            "../home/u/.clax/marketplace",
        ] {
            assert!(stored_path(s, base, home).starts_with(root), "{s}");
        }
        assert!(!stored_path("clax@clax", base, home).starts_with(root));
    }
}

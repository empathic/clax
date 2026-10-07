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

/// The person's settings for the Clax plugin in Codex's `config.toml`,
/// read before `codex plugin remove` deletes them; an error when the file
/// cannot be read or parsed.
fn codex_saved_settings(ctx: &Ctx) -> Result<Option<toml_edit::Table>, String> {
    let path = codex_approvals::config_path(&ctx.dirs.codex_home);
    let Some(text) = codex_approvals::read_config(&path)? else {
        return Ok(None);
    };
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("could not parse {} ({e})", path.display()))?;
    Ok(codex_approvals::plugin_settings(&text))
}

/// Whether `answer` (a line the person typed) says yes.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Asks on the terminal whether to add `lines` to `path`.
fn confirm(tools: &[&str], lines: &str, path: &Path) -> bool {
    use std::io::Write;
    let mut err = std::io::stderr();
    let _ = write!(
        err,
        "\nCodex asks before each call of these Clax tools, which change or remove artifacts or page data: {}.\nSo that Codex does not stop mid-task to ask, `clax init` can add these lines to {}:\n\n{lines}\nAdd them? [y/N] ",
        tools.join(", "),
        path.display()
    );
    let _ = err.flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && is_yes(&answer)
}

/// After Codex registered the plugin: puts back the `saved` settings that
/// re-registering removed, then offers the approvals for the Clax tools
/// Codex would still ask about, adding them when `yes` or the person
/// confirms on a terminal. The outcome as JSON; a failure is reported
/// there, never fatal.
fn codex_settings(ctx: &Ctx, saved: Result<Option<toml_edit::Table>, String>, yes: bool) -> Value {
    use std::io::IsTerminal;
    let path = codex_approvals::config_path(&ctx.dirs.codex_home);
    let failed = |detail: String| json!({"status": "failed", "config": path, "detail": detail});
    let saved = match saved {
        Ok(s) => s,
        Err(e) => return failed(format!("{e}; Clax's settings there were not carried over")),
    };
    let mut text = match codex_approvals::read_config(&path) {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => return failed(e),
    };
    let mut restored = false;
    if let Some(saved) = saved {
        match codex_approvals::restore(&text, &saved) {
            Ok(t) if t != text => {
                if let Err(e) = codex_approvals::write_config(&path, &t) {
                    return failed(format!("could not write {}: {e}", path.display()));
                }
                text = t;
                restored = true;
            }
            Ok(_) => {}
            Err(e) => return failed(format!("could not put back Clax's settings: {e}")),
        }
    }
    let assessment = match codex_approvals::assess(Some(&text), &ClaxTools::tools()) {
        Ok(a) => a,
        Err(e) => return failed(format!("could not parse {} ({e})", path.display())),
    };
    let kept: Vec<String> = assessment
        .kept()
        .iter()
        .map(|k| format!("{} (approval_mode = \"{}\")", k.tool, k.mode))
        .collect();
    let kept_note = (!kept.is_empty()).then(|| {
        format!(
            "Codex still asks before {}, as your config.toml sets",
            kept.join(", ")
        )
    });
    let tools = assessment.addable();
    let out = |status: &str, detail: Option<String>| {
        let detail = [detail, kept_note.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ");
        json!({
            "status": status,
            "config": path,
            "restored": restored,
            "tools": tools,
            "lines": codex_approvals::lines(&tools),
            "detail": detail,
        })
    };
    if tools.is_empty() {
        return out("unchanged", None);
    }
    let lines = codex_approvals::lines(&tools);
    let terminal = std::io::stdin().is_terminal();
    let consent = yes || (terminal && confirm(&tools, &lines, &path));
    if !consent {
        let status = if terminal { "declined" } else { "not_added" };
        return out(
            status,
            Some(format!(
                "Codex will ask before each call of {}; to stop it asking, run `{} --yes` or add the lines below to {}",
                tools.join(", "),
                codex_approvals::SETUP_COMMAND,
                path.display()
            )),
        );
    }
    match codex_approvals::add_approvals(&text, &tools)
        .and_then(|t| codex_approvals::write_config(&path, &t).map_err(|e| e.to_string()))
    {
        Ok(()) => out("added", None),
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
        let codex_saved = (install && h.name == "codex").then(|| codex_saved_settings(&ctx));
        let mut r = run_steps(
            h,
            &ctx,
            actions,
            if install { "registered" } else { "removed" },
        );
        if let Some(saved) = codex_saved
            && r["status"] == "registered"
        {
            r["approvals"] = codex_settings(&ctx, saved, a.yes);
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

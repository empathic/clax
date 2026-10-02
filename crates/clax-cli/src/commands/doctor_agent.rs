//! `clax doctor --agent <harness>`: one check per layer between a harness
//! and the daemon, so a partly working plugin says which layer failed.
//!
//! - `binary`: this `clax`, the one the plugins run (`$CLAX_BIN`, else the
//!   first on `PATH`), and every `clax` on `PATH`.
//! - `upgrade`: a failed upgrade that keeps the daemon at an older version.
//! - `plugin`: the harness's installed copy of the plugin, and whether its
//!   manifest version and launcher match this binary.
//! - `skill`: the installed skill's stated version and tool count, and whether
//!   it is the skill this binary was built with.
//! - `mcp`: whether the daemon has a live session of the harness.
//! - `hooks`: the harness's latest lines in `logs/hooks.log`.
//! - `feedback`: each live session's watches and push state.
//! - Grok Build only: `grok`, the installed Grok's version, and
//!   `claude_copy`, whether the Claude Code plugin has stood down in a Grok
//!   session.
//! - `channel` (Claude Code): whether the installed plugin declares the Clax
//!   channel, and how the latest session was launched.

use crate::client::Client;
use clax_core::Home;
use clax_mcp::{ClaxTools, plugin};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A harness whose integration `doctor --agent` checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DoctorAgent {
    Claude,
    Codex,
    Grok,
    Pi,
}

/// How many hooks.log lines the `hooks` check shows.
const HOOK_LINES: usize = 5;
/// The Pi package's name, which identifies it among Pi's installed packages.
const PI_PACKAGE: &str = "@empathic/clax-pi";
/// The launcher this binary was built with; the plugins carry copies.
const LAUNCHER: &str = include_str!("../../../../scripts/ensure-clax.sh");

impl DoctorAgent {
    /// The session `harness` name.
    pub fn harness(self) -> &'static str {
        match self {
            DoctorAgent::Claude => "claude",
            DoctorAgent::Codex => "codex",
            DoctorAgent::Grok => "grok",
            DoctorAgent::Pi => "pi",
        }
    }

    fn display(self) -> &'static str {
        match self {
            DoctorAgent::Claude => "Claude Code",
            DoctorAgent::Codex => "Codex",
            DoctorAgent::Grok => "Grok Build",
            DoctorAgent::Pi => "Pi",
        }
    }

    fn reinstall(self) -> &'static str {
        match self {
            DoctorAgent::Claude => {
                "reinstall with `/plugin uninstall clax@clax`, then `/plugin install clax@clax`"
            }
            DoctorAgent::Codex => "reinstall with `codex plugin add clax@clax`",
            DoctorAgent::Grok => "reinstall with `clax init --agent grok`",
            DoctorAgent::Pi => "reinstall with `pi install <checkout>/plugins/pi`",
        }
    }

    /// The skill this binary was built with, as the harness's plugin ships it.
    fn built_skill(self) -> &'static str {
        match self {
            DoctorAgent::Claude => {
                include_str!("../../../../plugins/claude-code/skills/clax/SKILL.md")
            }
            DoctorAgent::Codex => {
                include_str!("../../../../plugins/clax/skills/clax/SKILL.md")
            }
            DoctorAgent::Grok => {
                include_str!("../../../../plugins/clax-grok/skills/clax/SKILL.md")
            }
            DoctorAgent::Pi => include_str!("../../../../plugins/pi/skills/clax/SKILL.md"),
        }
    }
}

/// Where each harness keeps its configuration.
pub struct Dirs {
    /// `$CODEX_HOME`, else `~/.codex`.
    pub codex_home: PathBuf,
    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub claude_dir: PathBuf,
    /// `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
    pub pi_dir: PathBuf,
    /// `$GROK_HOME`, else `~/.grok`.
    pub grok_home: PathBuf,
}

impl Dirs {
    /// The directories from the environment lookup `env`; `None` without `HOME`.
    /// Empty variables count as unset. Each directory is absolute: a leading
    /// `~` or `~/` is expanded against `HOME`, and a relative value is taken
    /// relative to `HOME`, where `clax init` runs the harness CLIs.
    /// `~user/` is not supported: a value starting with it is taken relative
    /// to `HOME`.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> Option<Dirs> {
        let raw = |k: &str| env(k).filter(|v| !v.is_empty());
        let home = PathBuf::from(raw("HOME")?);
        let var = |k: &str| {
            raw(k).map(|v| {
                if v == "~" {
                    home.clone()
                } else if let Some(rest) = v.strip_prefix("~/") {
                    home.join(rest)
                } else {
                    home.join(v)
                }
            })
        };
        Some(Dirs {
            codex_home: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            pi_dir: var("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi/agent")),
            grok_home: var("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
        })
    }
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Value {
    json!({"name": name, "ok": ok, "detail": detail.into()})
}

/// The subdirectories of `dir`, or none when it cannot be read.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// `<base>/*/clax/*` holding `manifest`: a harness's plugin cache copies.
fn cached_copies(base: &Path, manifest: &str) -> Vec<PathBuf> {
    subdirs(base)
        .iter()
        .flat_map(|market| subdirs(&market.join("clax")))
        .filter(|root| root.join(manifest).is_file())
        .collect()
}

/// Every string value under a key named `installPath`, anywhere in `v`.
fn install_paths(v: &Value, out: &mut Vec<PathBuf>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                match (k.as_str(), v) {
                    ("installPath", Value::String(s)) => out.push(PathBuf::from(s)),
                    _ => install_paths(v, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|v| install_paths(v, out)),
        _ => {}
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The installed plugin roots for `agent`, newest version first.
///
/// - Codex: `<codex home>/plugins/cache/*/clax/<version>/`.
/// - Claude Code: the `installPath`s of `clax@*` entries in
///   `<claude dir>/plugins/installed_plugins.json`, then
///   `<claude dir>/plugins/cache/*/clax/<version>/`.
/// - Pi: local-path packages in `<pi dir>/settings.json` whose `package.json`
///   names the Clax Pi package.
/// - Grok Build: directories under `<grok home>` holding a
///   `.grok-plugin/plugin.json` that names clax-grok ([`grok_plugin_copies`]).
pub fn plugin_roots(agent: DoctorAgent, dirs: &Dirs) -> Vec<PathBuf> {
    let mut roots = match agent {
        DoctorAgent::Codex => cached_copies(
            &dirs.codex_home.join("plugins/cache"),
            ".codex-plugin/plugin.json",
        ),
        DoctorAgent::Claude => {
            let mut out = vec![];
            if let Some(Value::Object(plugins)) =
                read_json(&dirs.claude_dir.join("plugins/installed_plugins.json"))
                    .map(|v| v["plugins"].clone())
            {
                for (name, entry) in &plugins {
                    if name.starts_with("clax@") {
                        install_paths(entry, &mut out);
                    }
                }
            }
            out.retain(|p| p.join(".claude-plugin/plugin.json").is_file());
            for p in cached_copies(
                &dirs.claude_dir.join("plugins/cache"),
                ".claude-plugin/plugin.json",
            ) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            out
        }
        DoctorAgent::Pi => {
            let settings = read_json(&dirs.pi_dir.join("settings.json")).unwrap_or_default();
            let packages = settings["packages"].as_array().cloned().unwrap_or_default();
            packages
                .iter()
                .filter_map(|p| p.as_str().or_else(|| p["source"].as_str()))
                .filter(|s| s.starts_with('/') || s.starts_with('.'))
                .map(|s| dirs.pi_dir.join(s))
                .filter(|root| {
                    read_json(&root.join("package.json")).is_some_and(|m| m["name"] == PI_PACKAGE)
                })
                .collect()
        }
        DoctorAgent::Grok => grok_plugin_copies(&dirs.grok_home),
    };
    let version =
        |p: &PathBuf| plugin::manifest_version(p).and_then(|v| semver::Version::parse(&v).ok());
    roots.sort_by_key(|p| std::cmp::Reverse(version(p)));
    roots
}

/// Directories under `grok_home`, at most five levels down and never inside
/// `sessions` or `logs`, whose `.grok-plugin/plugin.json` names clax-grok.
/// Grok's install layout is not documented, so this does not assume one.
fn grok_plugin_copies(grok_home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(grok_home.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        if read_json(&d.join(".grok-plugin/plugin.json")).is_some_and(|m| m["name"] == "clax-grok")
        {
            out.push(d);
            continue;
        }
        if depth == 5 {
            continue;
        }
        for e in std::fs::read_dir(&d)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let name = e.file_name();
            if e.file_type().is_ok_and(|t| t.is_dir()) && name != "sessions" && name != "logs" {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    out
}

fn where_installed(agent: DoctorAgent, dirs: &Dirs) -> String {
    match agent {
        DoctorAgent::Codex => dirs.codex_home.join("plugins/cache").display().to_string(),
        DoctorAgent::Claude => dirs.claude_dir.join("plugins").display().to_string(),
        DoctorAgent::Grok => dirs.grok_home.display().to_string(),
        DoctorAgent::Pi => dirs.pi_dir.join("settings.json").display().to_string(),
    }
}

/// How long a `clax --version` on `PATH` may take.
const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Every file named `clax` in the `PATH` value `path`, in order, with the
/// first line of its `--version` when that names clax (and it answers
/// within [`VERSION_TIMEOUT`]).
pub fn clax_on_path(path: &std::ffi::OsStr) -> Vec<(PathBuf, Option<String>)> {
    std::env::split_paths(path)
        .map(|d| d.join("clax"))
        .filter(|p| p.is_file())
        .map(|p| {
            let v = version_line(&p).filter(|l| l.starts_with("clax "));
            (p, v)
        })
        .collect()
}

/// The first line `exe --version` prints, or None when it cannot run or
/// takes longer than [`VERSION_TIMEOUT`] (it is then killed).
fn version_line(exe: &Path) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    out.lines().next().map(str::to_string)
}

/// `binary`: this executable and its version, the binary the plugins'
/// wrapper runs (`$CLAX_BIN`, else the first clax on `PATH`), and every
/// clax on `PATH`; failed when the wrapper runs none, or another one.
pub fn binary_check(
    exe: &Path,
    version: &str,
    clax_bin: Option<&str>,
    on_path: &[(PathBuf, Option<String>)],
) -> Value {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let runs: Option<(PathBuf, String)> = match clax_bin.filter(|b| !b.is_empty()) {
        Some(b) => Some((PathBuf::from(b), "from CLAX_BIN".into())),
        None => on_path
            .iter()
            .find_map(|(p, v)| v.as_ref().map(|v| (p.clone(), v.clone()))),
    };
    let mut lines = vec![format!("this clax: {} (clax {version})", exe.display())];
    let ok = match &runs {
        Some((p, v)) => {
            lines.push(format!("the plugins run: {} ({v})", p.display()));
            canon(p) == canon(exe)
        }
        None => {
            lines.push("the plugins run: nothing (no clax on PATH)".into());
            false
        }
    };
    let listed: Vec<String> = on_path
        .iter()
        .map(|(p, v)| format!("{} ({})", p.display(), v.as_deref().unwrap_or("not clax")))
        .collect();
    lines.push(format!(
        "on PATH, in order: {}",
        if listed.is_empty() {
            "none".to_string()
        } else {
            listed.join("; ")
        }
    ));
    if !ok {
        lines.push(
            "the plugins run another clax than this one, or none: run `just install` in your Clax checkout (or install.sh), and put its directory first on the PATH your harness starts with".into(),
        );
    }
    check("binary", ok, lines.join("\n"))
}

/// `upgrade`: no failed upgrade keeps the running daemon (of version
/// `daemon_version`, when one runs) at an older version; failed, with the
/// failure, its reason, when the hold ends and what to do, when one does.
pub fn upgrade_check(home: &Home, daemon_version: Option<&str>) -> Value {
    let hold = daemon_version.and_then(|v| Some((v, crate::client::upgrade_hold_for(home, v)?)));
    match hold {
        None => check(
            "upgrade",
            true,
            "no failed upgrade is holding the daemon back",
        ),
        Some((kept, h)) => check(
            "upgrade",
            false,
            format!("{}\nwhy: {}", h.line(home, kept), h.reason),
        ),
    }
}

/// `plugin`: the newest installed copy of the harness's plugin; failed when
/// none is found, or when its manifest version or its launcher differs from
/// this binary's (a stale copy).
pub fn plugin_check(
    agent: DoctorAgent,
    dirs: &Dirs,
    binary_version: &str,
) -> (Value, Option<PathBuf>) {
    let Some(root) = plugin_roots(agent, dirs).into_iter().next() else {
        let install = match agent {
            DoctorAgent::Codex => {
                "`codex plugin marketplace add <checkout>`, then `codex plugin add clax@clax`"
            }
            DoctorAgent::Claude => {
                "`/plugin marketplace add <checkout>`, then `/plugin install clax@clax`"
            }
            DoctorAgent::Grok => "`clax init --agent grok`",
            DoctorAgent::Pi => "`pi install <checkout>/plugins/pi`",
        };
        return (
            check(
                "plugin",
                false,
                format!(
                    "no installed {} plugin found in {}; install it with {install}",
                    agent.display(),
                    where_installed(agent, dirs)
                ),
            ),
            None,
        );
    };
    let shown = root.display();
    let c = match plugin::manifest_version(&root) {
        None => check(
            "plugin",
            false,
            format!("{shown} has no manifest version; {}", agent.reinstall()),
        ),
        Some(v) if v != binary_version => check(
            "plugin",
            false,
            format!(
                "stale plugin: {} ({shown} is version {v}, this clax is {binary_version})",
                agent.reinstall()
            ),
        ),
        Some(v) => match std::fs::read_to_string(root.join("scripts/ensure-clax.sh")) {
            Ok(launcher) if launcher != LAUNCHER => check(
                "plugin",
                false,
                format!(
                    "stale plugin: {} ({shown} is version {v}, but its scripts/ensure-clax.sh differs from this clax's)",
                    agent.reinstall()
                ),
            ),
            _ => check("plugin", true, format!("{shown} (version {v})")),
        },
    };
    (c, Some(root))
}

/// `skill`: the installed skill states this binary's tool count and is the
/// skill this binary was built with.
pub fn skill_check(agent: DoctorAgent, root: Option<&Path>, tool_count: usize) -> Value {
    let Some(root) = root else {
        return check("skill", false, "no installed plugin, so no installed skill");
    };
    let path = root.join("skills/clax/SKILL.md");
    let shown = path.display();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return check(
            "skill",
            false,
            format!("{shown} is missing; {}", agent.reinstall()),
        );
    };
    match plugin::skill_block(&text) {
        None => check(
            "skill",
            false,
            format!(
                "stale skill: {shown} states no plugin version or tool count, so it predates this clax ({tool_count} tools); {}",
                agent.reinstall()
            ),
        ),
        Some((_, n)) if n != tool_count => check(
            "skill",
            false,
            format!(
                "stale skill: {shown} lists {n} tools, this clax has {tool_count}; {}",
                agent.reinstall()
            ),
        ),
        Some(_) if text != agent.built_skill() => check(
            "skill",
            false,
            format!(
                "stale skill: {shown} differs from the skill this clax was built with; {}",
                agent.reinstall()
            ),
        ),
        Some((v, n)) => check(
            "skill",
            true,
            format!("{shown}: plugin {v}, {n} tools, as this clax"),
        ),
    }
}

/// The daemon's live sessions of `agent`, or why they could not be listed.
pub fn live_sessions(client: Option<&Client>, agent: DoctorAgent) -> Result<Vec<Value>, String> {
    let c = client.ok_or("no daemon is running")?;
    let v = c
        .get("/api/sessions?live=true")
        .map_err(|e| format!("the daemon did not list sessions: {e:#}"))?;
    Ok(v["sessions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|s| s["harness"] == agent.harness())
                .cloned()
                .collect()
        })
        .unwrap_or_default())
}

/// `mcp`: the daemon has a live session of the harness, which its MCP server
/// (or, under Pi, its extension) registers.
pub fn mcp_check(agent: DoctorAgent, sessions: &Result<Vec<Value>, String>) -> Value {
    let mut c = mcp_sessions_check(agent, sessions);
    if agent == DoctorAgent::Grok {
        let detail = format!(
            "{}\n{GROK_APPROVAL}",
            c["detail"].as_str().unwrap_or_default()
        );
        c["detail"] = Value::String(detail);
    }
    c
}

/// How Grok's per-call tool approval can be lifted for the clax tools; doctor
/// prints it and never writes it.
const GROK_APPROVAL: &str = "Grok asks before each tool call; [permission] allow = [\"MCPTool(clax_grok__*)\"] in ~/.grok/config.toml approves them all, delete included";

fn mcp_sessions_check(agent: DoctorAgent, sessions: &Result<Vec<Value>, String>) -> Value {
    let registrar = match agent {
        DoctorAgent::Pi => "the Pi extension",
        _ => "the plugin's MCP server",
    };
    match sessions {
        Err(e) => check(
            "mcp",
            false,
            format!("{e}; {registrar} starts one on its first tool call"),
        ),
        Ok(s) if s.is_empty() => {
            let hint = match agent {
                DoctorAgent::Codex => " (`codex mcp list` shows whether Codex has the server)",
                DoctorAgent::Claude => " (`/mcp` in Claude Code shows the server's state)",
                DoctorAgent::Grok => {
                    " (`grok mcp list` shows whether Grok has the clax_grok server)"
                }
                DoctorAgent::Pi => "",
            };
            check(
                "mcp",
                false,
                format!(
                    "no live {} session in the daemon: start one; if one is running, {registrar} has not reached the daemon{hint}",
                    agent.display()
                ),
            )
        }
        Ok(s) => check(
            "mcp",
            true,
            format!(
                "{} live {} session(s): {}",
                s.len(),
                agent.display(),
                s.iter()
                    .map(|s| s["id"].as_str().unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    }
}

/// `hooks`: the harness's last lines in hooks.log. Failed when the latest
/// is the launcher finding no binary, or when no Claude Code or Grok Build
/// hook has run. Stand-down lines are not hook runs.
pub fn hooks_check(agent: DoctorAgent, home: &Home) -> Value {
    if agent == DoctorAgent::Pi {
        return check(
            "hooks",
            true,
            "Pi runs no hooks; its extension calls the daemon itself",
        );
    }
    let lines = crate::hooklog::tail_for(home, agent.harness(), HOOK_LINES);
    let log = home.hooks_log_path();
    match lines.last() {
        None => check(
            "hooks",
            agent == DoctorAgent::Codex,
            match agent {
                DoctorAgent::Codex => format!(
                    "no hook has run ({} has no codex line); Codex runs plugin hooks only with `features.hooks = true` and trusted hooks, which are optional",
                    log.display()
                ),
                DoctorAgent::Grok => format!(
                    "no hook has run ({} has no grok line); clax-grok's hooks run once the plugin is installed and trusted (clax init installs it with --trust)",
                    log.display()
                ),
                _ => format!("no hook has run ({} has no claude line)", log.display()),
            },
        ),
        Some(last) => check(
            "hooks",
            !last.contains(" launcher "),
            format!(
                "last {} line(s) of {}:\n{}",
                lines.len(),
                log.display(),
                lines.join("\n")
            ),
        ),
    }
}

/// The oldest Grok Build release Clax is checked against
/// (open-questions Q3).
const MIN_GROK: semver::Version = semver::Version::new(1, 0, 45);

/// `grok`: the first `X.Y.Z` in `grok --version`'s first line; failed when
/// there is none, or it is older than [`MIN_GROK`].
pub fn grok_version_check(version_line: Option<&str>) -> Value {
    let Some(line) = version_line else {
        return check(
            "grok",
            false,
            "grok is not on PATH, or `grok --version` failed",
        );
    };
    let found = line
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find_map(|w| semver::Version::parse(w).ok());
    match found {
        Some(v) if v >= MIN_GROK => check("grok", true, line.to_string()),
        Some(v) => check(
            "grok",
            false,
            format!(
                "{line}: Clax is checked against Grok Build {MIN_GROK} and later; update Grok (v{v} is older)"
            ),
        ),
        None => check("grok", false, format!("{line}: no version found")),
    }
}

/// `claude_copy`: never failed. Whether the Claude Code plugin has stood
/// down in a Grok session, from its `standdown` lines in hooks.log.
pub fn claude_copy_check(home: &Home) -> Value {
    let n = [
        home.hooks_log_path().with_extension("log.1"),
        home.hooks_log_path(),
    ]
    .iter()
    .filter_map(|p| std::fs::read_to_string(p).ok())
    .map(|t| {
        t.lines()
            .filter(|l| l.contains(" standdown ") && l.contains(" host=grok"))
            .count()
    })
    .sum::<usize>();
    if n == 0 {
        check(
            "claude_copy",
            true,
            "the Claude Code plugin has not run in a Grok session",
        )
    } else {
        check(
            "claude_copy",
            true,
            format!(
                "the Claude Code plugin is enabled in Grok and stood down {n} time(s): clax-grok acts instead. `grok plugin disable clax` removes its idle clax server"
            ),
        )
    }
}

/// The first `grok` on the `PATH` value `path`.
fn grok_on_path(path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|d| d.join("grok"))
        .find(|p| p.is_file())
}

/// One live session's feedback state: its watches (from
/// `/api/sessions/<id>/watches`) and push (from `/api/sessions/<id>`).
pub struct SessionFeedback {
    pub id: String,
    pub watches: Option<Value>,
    pub push: Option<Value>,
}

/// `feedback`: for each live session, how many artifacts it watches (and with
/// replies armed) and whether comments can be pushed to it.
pub fn feedback_check(sessions: &Result<Vec<SessionFeedback>, String>) -> Value {
    let s = match sessions {
        Err(e) => return check("feedback", false, e.clone()),
        Ok(s) if s.is_empty() => {
            return check("feedback", true, "no live session to deliver comments to");
        }
        Ok(s) => s,
    };
    let mut ok = true;
    let lines: Vec<String> = s
        .iter()
        .map(|f| {
            let watches = match f.watches.as_ref().and_then(|w| w["watches"].as_array()) {
                Some(w) => format!(
                    "{} watch(es), {} with replies armed",
                    w.len(),
                    w.iter().filter(|w| w["replies_armed"] == true).count()
                ),
                None => {
                    ok = false;
                    "watches unavailable".into()
                }
            };
            let push = match f.push.as_ref() {
                Some(p) if p["available"] == true => {
                    format!("push {}", p["tier"].as_str().unwrap_or("on"))
                }
                Some(p) => format!("push off: {}", p["reason"].as_str().unwrap_or("unknown")),
                None => {
                    ok = false;
                    "push unavailable".into()
                }
            };
            format!("session {}: {watches}; {push}", f.id)
        })
        .collect();
    check("feedback", ok, lines.join("\n"))
}

/// `channel` (Claude Code): whether the installed plugin declares the Clax
/// channel, and how the latest Claude Code session was launched (the
/// shim's `channel` line in hooks.log). The channel is opt-in, so only a
/// manifest without it fails.
pub fn channel_check(root: Option<&Path>, home: &Home) -> Value {
    let declared = root
        .and_then(|r| read_json(&r.join(".claude-plugin/plugin.json")))
        .is_some_and(|m| m["channels"] == json!([{"server": "clax"}]));
    if !declared {
        return check(
            "channel",
            false,
            "the installed plugin does not declare the clax channel; run `clax init` to install the current plugin",
        );
    }
    let launch = clax_mcp::channel::LAUNCH;
    let last = crate::hooklog::tail_for(home, "claude", 200)
        .into_iter()
        .rev()
        .find(|l| {
            l.split_once(' ')
                .is_some_and(|(_, rest)| rest.starts_with("channel "))
        });
    let detail = match last {
        None => format!(
            "no Claude Code session has started the shim yet. To wake idle sessions through the channel, launch `{launch}`; without it the skill runs `clax feedback follow --once` in the background"
        ),
        Some(l) if l.contains("launch_flag=present") => format!(
            "the latest session ({}) was launched with the channel: {l}. Claude Code does not tell Clax whether the channel registered; its startup screen says so. If it says the channel was blocked, relaunch without the flag to use the background fallback",
            l.split(' ').next().unwrap_or_default()
        ),
        Some(l) => format!(
            "the latest session ({}) was launched without the channel ({l}); idle sessions wake through the skill's background `clax feedback follow --once`. For the channel, launch `{launch}` (research preview, CLI only, claude.ai or Console login; on Team and Enterprise an Owner must turn channels on)",
            l.split(' ').next().unwrap_or_default()
        ),
    };
    check("channel", true, detail)
}

/// Every layered check for `agent`.
pub fn checks(agent: DoctorAgent, home: &Home, client: Option<&Client>) -> Vec<Value> {
    let version = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe().unwrap_or_default();
    let on_path = clax_on_path(&std::env::var_os("PATH").unwrap_or_default());
    let clax_bin = std::env::var("CLAX_BIN").ok();
    let mut out = vec![binary_check(&exe, version, clax_bin.as_deref(), &on_path)];
    out.push(upgrade_check(home, client.map(|c| c.info.version.as_str())));
    // The installed plugin's root; `None` when HOME is unset.
    let mut plugin_root: Option<PathBuf> = None;
    match Dirs::from_env(|k| std::env::var(k).ok()) {
        Some(dirs) => {
            let (plugin, root) = plugin_check(agent, &dirs, version);
            out.push(plugin);
            out.push(skill_check(agent, root.as_deref(), ClaxTools::tool_count()));
            plugin_root = root;
        }
        None => {
            out.push(check("plugin", false, "HOME is not set"));
            out.push(check("skill", false, "HOME is not set"));
        }
    }
    let sessions = live_sessions(client, agent);
    out.push(mcp_check(agent, &sessions));
    out.push(hooks_check(agent, home));
    if agent == DoctorAgent::Grok {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let line = grok_on_path(&path).and_then(|g| version_line(&g));
        out.push(grok_version_check(line.as_deref()));
        out.push(claude_copy_check(home));
    }
    let feedback = sessions.map(|list| {
        list.iter()
            .map(|s| {
                let id = s["id"].as_str().unwrap_or_default().to_string();
                let c = client.expect("sessions were listed through the client");
                SessionFeedback {
                    watches: c.get(&format!("/api/sessions/{id}/watches")).ok(),
                    push: c
                        .get(&format!("/api/sessions/{id}"))
                        .ok()
                        .map(|v| v["push"].clone()),
                    id,
                }
            })
            .collect()
    });
    out.push(feedback_check(&feedback));
    if agent == DoctorAgent::Claude {
        out.push(channel_check(plugin_root.as_deref(), home));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp HOME with each harness's directory under it.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture {
                dir: tempfile::tempdir().unwrap(),
            }
        }
        fn home(&self) -> &Path {
            self.dir.path()
        }
        fn dirs(&self) -> Dirs {
            let home = self.home().display().to_string();
            Dirs::from_env(|k| (k == "HOME").then(|| home.clone())).unwrap()
        }
        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let p = self.home().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        }
        /// A plugin copy at `rel` with `manifest` at `version`, the launcher and
        /// the skill as this binary has them.
        fn plugin(&self, rel: &str, manifest: &str, version: &str, agent: DoctorAgent) -> PathBuf {
            self.write(
                &format!("{rel}/{manifest}"),
                &json!({"name": "clax", "version": version}).to_string(),
            );
            self.write(&format!("{rel}/scripts/ensure-clax.sh"), LAUNCHER);
            self.write(&format!("{rel}/skills/clax/SKILL.md"), agent.built_skill());
            self.home().join(rel)
        }
        /// The Clax home under this HOME.
        fn clax_home(&self) -> Home {
            Home::at(self.home().join("ax"))
        }
        /// Adds `channels: [{"server": "clax"}]` to the Claude Code manifest
        /// of the plugin copy at `root`.
        fn write_manifest_channels(&self, root: &Path) {
            let path = root.join(".claude-plugin/plugin.json");
            let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            m["channels"] = json!([{"server": "clax"}]);
            std::fs::write(&path, m.to_string()).unwrap();
        }
    }

    const V: &str = env!("CARGO_PKG_VERSION");

    #[test]
    fn dirs_default_under_home_and_honour_overrides() {
        let d = Dirs::from_env(|k| match k {
            "HOME" => Some("/h".into()),
            "CODEX_HOME" => Some("/cx".into()),
            "CLAUDE_CONFIG_DIR" => Some(String::new()),
            _ => None,
        })
        .unwrap();
        assert_eq!(d.codex_home, PathBuf::from("/cx"));
        assert_eq!(d.claude_dir, PathBuf::from("/h/.claude"));
        assert_eq!(d.pi_dir, PathBuf::from("/h/.pi/agent"));
        assert_eq!(d.grok_home, PathBuf::from("/h/.grok"));
        assert!(Dirs::from_env(|_| None).is_none());
    }

    #[test]
    fn dirs_expand_a_tilde_and_resolve_relative_values_against_home() {
        let d = Dirs::from_env(|k| match k {
            "HOME" => Some("/h".into()),
            "CODEX_HOME" => Some("~".into()),
            "CLAUDE_CONFIG_DIR" => Some("cfg/claude".into()),
            "PI_CODING_AGENT_DIR" => Some("~/.pi/agent".into()),
            "GROK_HOME" => Some("g".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(d.codex_home, PathBuf::from("/h"));
        assert_eq!(d.claude_dir, PathBuf::from("/h/cfg/claude"));
        assert_eq!(d.pi_dir, PathBuf::from("/h/.pi/agent"));
        assert_eq!(d.grok_home, PathBuf::from("/h/g"));
    }

    #[test]
    fn a_current_codex_cache_copy_passes_plugin_and_skill() {
        let f = Fixture::new();
        let root = f.plugin(
            &format!(".codex/plugins/cache/clax/clax/{V}"),
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        let (p, found) = plugin_check(DoctorAgent::Codex, &f.dirs(), V);
        assert_eq!(p["ok"], true, "{p}");
        assert_eq!(found.as_deref(), Some(root.as_path()));
        let s = skill_check(
            DoctorAgent::Codex,
            found.as_deref(),
            ClaxTools::tool_count(),
        );
        assert_eq!(s["ok"], true, "{s}");
    }

    #[test]
    fn a_cache_copy_under_the_previous_name_is_not_this_plugin() {
        const OLD: &str = concat!("arti", "fax");
        let f = Fixture::new();
        f.plugin(
            &format!(".codex/plugins/cache/{OLD}/{OLD}/0.2.0"),
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        let (p, found) = plugin_check(DoctorAgent::Codex, &f.dirs(), V);
        assert_eq!(p["ok"], false, "{p}");
        assert_eq!(found, None);
    }

    #[test]
    fn a_claude_install_under_the_previous_name_is_not_this_plugin() {
        const OLD: &str = concat!("arti", "fax");
        let f = Fixture::new();
        let listed = f.plugin(
            &format!("elsewhere/{OLD}"),
            ".claude-plugin/plugin.json",
            V,
            DoctorAgent::Claude,
        );
        f.write(
            ".claude/plugins/installed_plugins.json",
            &json!({"version": 2, "plugins": {
                format!("{OLD}@{OLD}"): [{"scope": "user", "installPath": listed}]
            }})
            .to_string(),
        );
        f.plugin(
            &format!(".claude/plugins/cache/{OLD}/{OLD}/0.2.0"),
            ".claude-plugin/plugin.json",
            V,
            DoctorAgent::Claude,
        );
        assert_eq!(
            plugin_roots(DoctorAgent::Claude, &f.dirs()),
            Vec::<PathBuf>::new()
        );
        let (p, found) = plugin_check(DoctorAgent::Claude, &f.dirs(), V);
        assert_eq!(p["ok"], false, "{p}");
        assert_eq!(found, None);
    }

    #[test]
    fn a_codex_copy_of_another_version_is_a_stale_plugin() {
        let f = Fixture::new();
        f.plugin(
            ".codex/plugins/cache/clax/clax/0.1.0",
            ".codex-plugin/plugin.json",
            "0.1.0",
            DoctorAgent::Codex,
        );
        let (p, _) = plugin_check(DoctorAgent::Codex, &f.dirs(), V);
        assert_eq!(p["ok"], false);
        let detail = p["detail"].as_str().unwrap();
        assert!(
            detail.starts_with("stale plugin: reinstall with `codex plugin add clax@clax`")
                && detail.contains("version 0.1.0"),
            "{detail}"
        );
    }

    #[test]
    fn a_copy_with_an_old_launcher_is_a_stale_plugin() {
        let f = Fixture::new();
        f.plugin(
            &format!(".codex/plugins/cache/clax/clax/{V}"),
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        f.write(
            &format!(".codex/plugins/cache/clax/clax/{V}/scripts/ensure-clax.sh"),
            "#!/bin/sh\n",
        );
        let (p, _) = plugin_check(DoctorAgent::Codex, &f.dirs(), V);
        assert_eq!(p["ok"], false);
        assert!(
            p["detail"]
                .as_str()
                .unwrap()
                .contains("ensure-clax.sh differs"),
            "{p}"
        );
    }

    #[test]
    fn the_newest_codex_copy_is_checked() {
        let f = Fixture::new();
        f.plugin(
            ".codex/plugins/cache/clax/clax/0.1.0",
            ".codex-plugin/plugin.json",
            "0.1.0",
            DoctorAgent::Codex,
        );
        let new = f.plugin(
            &format!(".codex/plugins/cache/clax/clax/{V}"),
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        assert_eq!(plugin_roots(DoctorAgent::Codex, &f.dirs())[0], new);
    }

    #[test]
    fn no_plugin_says_where_it_looked_and_how_to_install() {
        let f = Fixture::new();
        for agent in [
            DoctorAgent::Claude,
            DoctorAgent::Codex,
            DoctorAgent::Grok,
            DoctorAgent::Pi,
        ] {
            let (p, root) = plugin_check(agent, &f.dirs(), V);
            assert_eq!(p["ok"], false);
            assert!(root.is_none());
            assert!(
                p["detail"].as_str().unwrap().contains("install it with"),
                "{p}"
            );
            let s = skill_check(agent, None, 22);
            assert_eq!(s["ok"], false);
        }
    }

    #[test]
    fn claude_plugins_come_from_installed_plugins_json_and_the_cache() {
        let f = Fixture::new();
        let listed = f.plugin(
            "elsewhere/clax",
            ".claude-plugin/plugin.json",
            V,
            DoctorAgent::Claude,
        );
        f.write(
            ".claude/plugins/installed_plugins.json",
            &json!({"version": 2, "plugins": {
                "clax@clax": [{"scope": "user", "installPath": listed}],
                "other@x": [{"installPath": "/nope"}]
            }})
            .to_string(),
        );
        let cached = f.plugin(
            ".claude/plugins/cache/clax/clax/0.1.0",
            ".claude-plugin/plugin.json",
            "0.1.0",
            DoctorAgent::Claude,
        );
        assert_eq!(
            plugin_roots(DoctorAgent::Claude, &f.dirs()),
            vec![listed, cached]
        );
        let (p, _) = plugin_check(DoctorAgent::Claude, &f.dirs(), V);
        assert_eq!(p["ok"], true, "{p}");
    }

    #[test]
    fn pi_packages_are_local_paths_in_settings_naming_the_clax_package() {
        let f = Fixture::new();
        f.write(
            "checkout/plugins/pi/package.json",
            &json!({"name": PI_PACKAGE, "version": V}).to_string(),
        );
        f.write(
            "checkout/plugins/pi/skills/clax/SKILL.md",
            DoctorAgent::Pi.built_skill(),
        );
        f.write("other/package.json", &json!({"name": "other"}).to_string());
        let pi = f.home().join("checkout/plugins/pi");
        f.write(
            ".pi/agent/settings.json",
            &json!({"packages": ["npm:x", f.home().join("other"), {"source": pi}]}).to_string(),
        );
        assert_eq!(plugin_roots(DoctorAgent::Pi, &f.dirs()), vec![pi.clone()]);
        let s = skill_check(DoctorAgent::Pi, Some(&pi), ClaxTools::tool_count());
        assert_eq!(s["ok"], true, "{s}");
    }

    #[test]
    fn a_skill_without_the_block_or_with_another_count_is_stale() {
        let f = Fixture::new();
        let root = f.home().join("p");
        f.write(
            "p/skills/clax/SKILL.md",
            "# Clax\n\nThe tools are exposed as the `clax` MCP server (`publish`).\n",
        );
        let s = skill_check(DoctorAgent::Codex, Some(&root), 22);
        assert_eq!(s["ok"], false);
        let d = s["detail"].as_str().unwrap();
        assert!(
            d.starts_with("stale skill:") && d.contains("codex plugin add"),
            "{d}"
        );
        f.write(
            "p/skills/clax/SKILL.md",
            "This is Clax plugin 0.2.0. It provides 14 tools as the `clax` MCP server",
        );
        let s = skill_check(DoctorAgent::Codex, Some(&root), 22);
        assert!(
            s["detail"]
                .as_str()
                .unwrap()
                .contains("lists 14 tools, this clax has 22"),
            "{s}"
        );
        f.write(
            "p/skills/clax/SKILL.md",
            "This is Clax plugin 0.2.0. It provides 22 tools, edited",
        );
        let s = skill_check(DoctorAgent::Codex, Some(&root), 22);
        assert!(
            s["detail"]
                .as_str()
                .unwrap()
                .contains("differs from the skill this clax was built with"),
            "{s}"
        );
    }

    #[test]
    fn the_built_skills_state_this_binarys_tool_count() {
        for agent in [
            DoctorAgent::Claude,
            DoctorAgent::Codex,
            DoctorAgent::Grok,
            DoctorAgent::Pi,
        ] {
            assert_eq!(
                plugin::skill_block(agent.built_skill()),
                Some((V.to_string(), ClaxTools::tool_count())),
                "{agent:?}"
            );
        }
    }

    #[test]
    fn mcp_needs_a_live_session_of_the_harness() {
        let none = mcp_check(DoctorAgent::Codex, &Ok(vec![]));
        assert_eq!(none["ok"], false);
        assert!(
            none["detail"]
                .as_str()
                .unwrap()
                .contains("no live Codex session"),
            "{none}"
        );
        let down = mcp_check(DoctorAgent::Claude, &Err("no daemon is running".into()));
        assert_eq!(down["ok"], false);
        let live = mcp_check(
            DoctorAgent::Codex,
            &Ok(vec![json!({"id": "s1", "harness": "codex"})]),
        );
        assert_eq!(live["ok"], true);
        assert!(live["detail"].as_str().unwrap().contains("s1"), "{live}");
    }

    #[test]
    fn hooks_shows_the_agents_last_lines_and_fails_on_a_missing_binary() {
        let f = Fixture::new();
        let home = Home::at(f.home().join("ax"));
        let none = hooks_check(DoctorAgent::Codex, &home);
        assert_eq!(none["ok"], true);
        assert!(
            none["detail"]
                .as_str()
                .unwrap()
                .starts_with("no hook has run"),
            "{none}"
        );
        assert_eq!(hooks_check(DoctorAgent::Claude, &home)["ok"], false);
        assert_eq!(hooks_check(DoctorAgent::Pi, &home)["ok"], true);
        crate::hooklog::append(&home, "t1 hook agent=codex event=stop exit=0 stderr=\"\"");
        crate::hooklog::append(&home, "t2 hook agent=claude event=stop exit=0 stderr=\"\"");
        let ok = hooks_check(DoctorAgent::Codex, &home);
        assert_eq!(ok["ok"], true);
        let d = ok["detail"].as_str().unwrap();
        assert!(d.contains("t1") && !d.contains("t2"), "{d}");
        crate::hooklog::append(
            &home,
            "t3 launcher mode=hook agent=codex exit=0 reason=\"no binary found\"",
        );
        assert_eq!(hooks_check(DoctorAgent::Codex, &home)["ok"], false);
    }

    #[test]
    fn the_grok_plugin_is_found_anywhere_under_grok_home() {
        let f = Fixture::new();
        let root = f.dirs().grok_home.join("plugins/installed/clax-grok-0.3.0");
        std::fs::create_dir_all(root.join(".grok-plugin")).unwrap();
        std::fs::write(
            root.join(".grok-plugin/plugin.json"),
            r#"{"name": "clax-grok", "version": "0.3.0"}"#,
        )
        .unwrap();
        // A plugin of another name is not ours, and session trees are never walked.
        let other = f.dirs().grok_home.join("plugins/x");
        std::fs::create_dir_all(other.join(".grok-plugin")).unwrap();
        std::fs::write(
            other.join(".grok-plugin/plugin.json"),
            r#"{"name": "x", "version": "9.9.9"}"#,
        )
        .unwrap();
        let session = f.dirs().grok_home.join("sessions/s1/clax-grok");
        std::fs::create_dir_all(session.join(".grok-plugin")).unwrap();
        std::fs::write(
            session.join(".grok-plugin/plugin.json"),
            r#"{"name": "clax-grok", "version": "0.3.0"}"#,
        )
        .unwrap();
        assert_eq!(plugin_roots(DoctorAgent::Grok, &f.dirs()), vec![root]);
    }

    #[test]
    fn grok_hooks_are_expected_and_stand_downs_are_not_grok_or_claude_hooks() {
        let f = Fixture::new();
        let home = Home::at(f.home().join("ax"));
        assert_eq!(
            hooks_check(DoctorAgent::Grok, &home)["ok"],
            false,
            "no grok hook has run"
        );
        assert!(
            claude_copy_check(&home)["detail"]
                .as_str()
                .unwrap()
                .contains("has not run"),
        );
        crate::hooklog::append(
            &home,
            "2026-10-01T10:00:00Z standdown mode=hook agent=claude host=grok",
        );
        assert_eq!(
            hooks_check(DoctorAgent::Claude, &home)["ok"],
            false,
            "a stand-down is not a Claude Code hook run"
        );
        let c = claude_copy_check(&home);
        assert_eq!(c["ok"], true);
        assert!(
            c["detail"]
                .as_str()
                .unwrap()
                .contains("grok plugin disable clax"),
            "{c}"
        );
    }

    #[test]
    fn the_grok_version_check_warns_below_the_minimum() {
        assert_eq!(grok_version_check(Some("grok 1.0.45"))["ok"], true);
        assert_eq!(
            grok_version_check(Some("grok-build 1.1.0 (abc)"))["ok"],
            true
        );
        assert_eq!(grok_version_check(Some("grok 1.0.44"))["ok"], false);
        assert_eq!(grok_version_check(Some("grok dev"))["ok"], false);
        assert_eq!(grok_version_check(None)["ok"], false);
    }

    #[test]
    fn grok_mcp_prints_the_approval_rule() {
        for sessions in [Ok(vec![]), Ok(vec![json!({"id": "s1", "harness": "grok"})])] {
            let c = mcp_check(DoctorAgent::Grok, &sessions);
            assert!(
                c["detail"]
                    .as_str()
                    .unwrap()
                    .contains(r#"allow = ["MCPTool(clax_grok__*)"]"#),
                "{c}"
            );
        }
        let none = mcp_check(DoctorAgent::Grok, &Ok(vec![]));
        assert!(none["detail"].as_str().unwrap().contains("grok mcp list"));
    }

    #[test]
    fn channel_needs_the_manifest_entry_and_reports_the_last_launch() {
        let f = Fixture::new();
        let root = f.plugin(
            "plugins/cache/clax/clax/0.3.0",
            ".claude-plugin/plugin.json",
            "0.3.0",
            DoctorAgent::Claude,
        );
        // Without `channels` in the manifest: failed, says to reinstall.
        let v = channel_check(Some(&root), &f.clax_home());
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["detail"].as_str().unwrap().contains("clax init"), "{v}");
        assert_eq!(channel_check(None, &f.clax_home())["ok"], false);
        // With it, and no channel line: ok, gives the launch command.
        f.write_manifest_channels(&root);
        let v = channel_check(Some(&root), &f.clax_home());
        assert_eq!(v["ok"], true);
        assert!(
            v["detail"]
                .as_str()
                .unwrap()
                .contains("--dangerously-load-development-channels plugin:clax@clax"),
            "{v}"
        );
        // The latest channel line decides the text.
        crate::hooklog::append(
            &f.clax_home(),
            "2026-10-01T10:00:00Z channel agent=claude launch_flag=absent flag=\"\" entry=\"\" parent_pid=1",
        );
        let d = channel_check(Some(&root), &f.clax_home())["detail"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            d.contains("2026-10-01T10:00:00Z") && d.contains("without the channel"),
            "{d}"
        );
        crate::hooklog::append(
            &f.clax_home(),
            "2026-10-01T10:05:00Z channel agent=claude launch_flag=present flag=\"--dangerously-load-development-channels\" entry=\"plugin:clax@clax\" parent_pid=2",
        );
        crate::hooklog::append(
            &f.clax_home(),
            "2026-10-01T10:05:01Z hook agent=claude event=stop bin=/b/clax duration_ms=3 exit=0 stderr=\"\"",
        );
        let d = channel_check(Some(&root), &f.clax_home())["detail"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            d.contains("2026-10-01T10:05:00Z")
                && d.contains("plugin:clax@clax")
                && d.contains("startup screen"),
            "{d}"
        );
    }

    #[test]
    fn a_latest_channel_line_leaves_hooks_ok() {
        let f = Fixture::new();
        let home = f.clax_home();
        crate::hooklog::append(
            &home,
            "t1 hook agent=claude event=stop bin=/b/clax duration_ms=3 exit=0 stderr=\"\"",
        );
        crate::hooklog::append(
            &home,
            "t2 channel agent=claude launch_flag=absent flag=\"\" entry=\"\" parent_pid=1",
        );
        let h = hooks_check(DoctorAgent::Claude, &home);
        assert_eq!(h["ok"], true, "{h}");
        assert!(h["detail"].as_str().unwrap().contains("t2 channel"), "{h}");
    }

    #[test]
    fn feedback_names_each_sessions_watches_and_push() {
        assert_eq!(feedback_check(&Ok(vec![]))["ok"], true);
        assert_eq!(
            feedback_check(&Err("no daemon is running".into()))["ok"],
            false
        );
        let c = feedback_check(&Ok(vec![
            SessionFeedback {
                id: "s1".into(),
                watches: Some(
                    json!({"watches": [{"replies_armed": true}, {"replies_armed": false}]}),
                ),
                push: Some(json!({"tier": "queue", "available": true, "reason": null})),
            },
            SessionFeedback {
                id: "s2".into(),
                watches: Some(json!({"watches": []})),
                push: Some(
                    json!({"tier": null, "available": false, "reason": "Codex session ID unknown, native push disabled"}),
                ),
            },
        ]));
        assert_eq!(c["ok"], true);
        assert_eq!(
            c["detail"],
            "session s1: 2 watch(es), 1 with replies armed; push queue\nsession s2: 0 watch(es), 0 with replies armed; push off: Codex session ID unknown, native push disabled"
        );
        let broken = feedback_check(&Ok(vec![SessionFeedback {
            id: "s3".into(),
            watches: None,
            push: None,
        }]));
        assert_eq!(broken["ok"], false);
    }

    /// A script named clax in `dir` whose --version prints `line`.
    fn fake_clax(dir: &Path, line: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("clax");
        std::fs::write(&p, format!("#!/bin/sh\necho '{line}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn clax_on_path_lists_every_clax_in_order() {
        let t = tempfile::tempdir().unwrap();
        let a = fake_clax(&t.path().join("a"), "other 1.0");
        let b = fake_clax(&t.path().join("b"), "clax 0.3.0");
        let path = std::env::join_paths([
            t.path().join("a"),
            t.path().join("none"),
            t.path().join("b"),
        ])
        .unwrap();
        let found = clax_on_path(&path);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].0.canonicalize().unwrap(), found[0].1.clone()),
            (a, None)
        );
        assert_eq!(
            (found[1].0.canonicalize().unwrap(), found[1].1.clone()),
            (b, Some("clax 0.3.0".into()))
        );
    }

    #[test]
    fn a_clax_on_path_that_hangs_is_listed_without_a_version() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("clax");
        std::fs::write(&p, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let found = clax_on_path(t.path().as_os_str());
        assert!(started.elapsed() < VERSION_TIMEOUT + std::time::Duration::from_secs(2));
        assert_eq!(found, vec![(p, None)]);
    }

    #[test]
    fn binary_passes_when_the_plugins_run_this_clax() {
        let t = tempfile::tempdir().unwrap();
        let me = fake_clax(&t.path().join("me"), "clax 0.3.0");
        let other = fake_clax(&t.path().join("other"), "clax 0.2.0");
        let v = binary_check(
            &me,
            "0.3.0",
            None,
            &[
                (me.clone(), Some("clax 0.3.0".into())),
                (other.clone(), Some("clax 0.2.0".into())),
            ],
        );
        assert_eq!(v["ok"], true, "{v}");
        let d = v["detail"].as_str().unwrap();
        assert!(
            d.contains(&format!("the plugins run: {} (clax 0.3.0)", me.display())),
            "{d}"
        );
        assert!(
            d.contains(&format!("{} (clax 0.2.0)", other.display())),
            "{d}"
        );
    }

    #[test]
    fn binary_fails_when_another_clax_comes_first_or_none_is_on_path() {
        let t = tempfile::tempdir().unwrap();
        let me = fake_clax(&t.path().join("me"), "clax 0.3.0");
        let first = fake_clax(&t.path().join("first"), "clax 0.2.0");
        let v = binary_check(
            &me,
            "0.3.0",
            None,
            &[
                (first.clone(), Some("clax 0.2.0".into())),
                (me.clone(), Some("clax 0.3.0".into())),
            ],
        );
        assert_eq!(v["ok"], false);
        assert!(
            v["detail"]
                .as_str()
                .unwrap()
                .contains("the plugins run another clax than this one"),
            "{v}"
        );
        let v = binary_check(&me, "0.3.0", None, &[]);
        assert_eq!(v["ok"], false);
        assert!(
            v["detail"]
                .as_str()
                .unwrap()
                .contains("the plugins run: nothing (no clax on PATH)"),
            "{v}"
        );
        let v = binary_check(&me, "0.3.0", Some(me.to_str().unwrap()), &[]);
        assert_eq!(v["ok"], true, "CLAX_BIN names this binary: {v}");
    }

    #[test]
    fn upgrade_fails_while_a_failed_upgrade_holds_the_running_daemon_back() {
        let t = tempfile::tempdir().unwrap();
        let home = Home::at(t.path().join("ax"));
        home.ensure_dirs().unwrap();
        let exe = fake_clax(&t.path().join("new"), "clax 0.3.0");
        crate::client::tests::write_hold(&home, "0.3.0", &exe, "it crashed");
        let v = upgrade_check(&home, Some("0.2.0"));
        assert_eq!(v["ok"], false, "{v}");
        let d = v["detail"].as_str().unwrap();
        for s in [
            "keeping clax daemon v0.2.0",
            "to v0.3.0",
            &exe.display().to_string(),
            "not tried again until",
            "why: it crashed",
            "`clax stop`",
        ] {
            assert!(d.contains(s), "{s} in {d}");
        }
        // No daemon, or one that is not older, is not held back.
        assert_eq!(upgrade_check(&home, None)["ok"], true);
        assert_eq!(upgrade_check(&home, Some("0.3.0"))["ok"], true);
    }
}

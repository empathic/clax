//! `clax doctor --agent <harness>`: one check per layer between a harness
//! and the daemon, so a partly working plugin says which layer failed.
//!
//! - `binary`: this `clax` and its version.
//! - `plugin`: the harness's installed copy of the plugin, and whether its
//!   manifest version and launcher match this binary.
//! - `skill`: the installed skill's stated version and tool count, and whether
//!   it is the skill this binary was built with.
//! - `mcp`: whether the daemon has a live session of the harness.
//! - `hooks`: the harness's latest lines in `logs/hooks.log`.
//! - `feedback`: each live session's watches and push state.

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
            DoctorAgent::Pi => "pi",
        }
    }

    fn display(self) -> &'static str {
        match self {
            DoctorAgent::Claude => "Claude Code",
            DoctorAgent::Codex => "Codex",
            DoctorAgent::Pi => "Pi",
        }
    }

    fn reinstall(self) -> &'static str {
        match self {
            DoctorAgent::Claude => {
                "reinstall with `/plugin uninstall clax@clax`, then `/plugin install clax@clax`"
            }
            DoctorAgent::Codex => "reinstall with `codex plugin add clax@clax`",
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
}

impl Dirs {
    /// The directories from the environment lookup `env`; `None` without `HOME`.
    /// Empty variables count as unset.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> Option<Dirs> {
        let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = var("HOME")?;
        Some(Dirs {
            codex_home: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            pi_dir: var("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi/agent")),
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
    };
    let version =
        |p: &PathBuf| plugin::manifest_version(p).and_then(|v| semver::Version::parse(&v).ok());
    roots.sort_by_key(|p| std::cmp::Reverse(version(p)));
    roots
}

fn where_installed(agent: DoctorAgent, dirs: &Dirs) -> String {
    match agent {
        DoctorAgent::Codex => dirs.codex_home.join("plugins/cache").display().to_string(),
        DoctorAgent::Claude => dirs.claude_dir.join("plugins").display().to_string(),
        DoctorAgent::Pi => dirs.pi_dir.join("settings.json").display().to_string(),
    }
}

/// `binary`: this executable and its version.
pub fn binary_check(exe: &Path, version: &str) -> Value {
    check(
        "binary",
        true,
        format!("{} (clax {version})", exe.display()),
    )
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
/// is the launcher finding no binary, or when no Claude Code hook has run.
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

/// Every layered check for `agent`.
pub fn checks(agent: DoctorAgent, home: &Home, client: Option<&Client>) -> Vec<Value> {
    let version = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe().unwrap_or_default();
    let mut out = vec![binary_check(&exe, version)];
    match Dirs::from_env(|k| std::env::var(k).ok()) {
        Some(dirs) => {
            let (plugin, root) = plugin_check(agent, &dirs, version);
            out.push(plugin);
            out.push(skill_check(agent, root.as_deref(), ClaxTools::tool_count()));
        }
        None => {
            out.push(check("plugin", false, "HOME is not set"));
            out.push(check("skill", false, "HOME is not set"));
        }
    }
    let sessions = live_sessions(client, agent);
    out.push(mcp_check(agent, &sessions));
    out.push(hooks_check(agent, home));
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
        assert!(Dirs::from_env(|_| None).is_none());
    }

    #[test]
    fn a_current_codex_cache_copy_passes_plugin_and_skill() {
        let f = Fixture::new();
        let root = f.plugin(
            ".codex/plugins/cache/clax/clax/0.2.0",
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
            ".codex/plugins/cache/clax/clax/0.2.0",
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        f.write(
            ".codex/plugins/cache/clax/clax/0.2.0/scripts/ensure-clax.sh",
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
            ".codex/plugins/cache/clax/clax/0.2.0",
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        assert_eq!(plugin_roots(DoctorAgent::Codex, &f.dirs())[0], new);
    }

    #[test]
    fn no_plugin_says_where_it_looked_and_how_to_install() {
        let f = Fixture::new();
        for agent in [DoctorAgent::Claude, DoctorAgent::Codex, DoctorAgent::Pi] {
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
        for agent in [DoctorAgent::Claude, DoctorAgent::Codex, DoctorAgent::Pi] {
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
}

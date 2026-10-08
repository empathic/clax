use super::doctor_agent::{self, DoctorAgent};
use crate::client::Client;
use anyhow::Context;
use clax_core::{ArtifactId, Home, Store};
use clax_server::daemon::DaemonLock;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// Repair what is safe to repair: remove stray staging and temp files and
    /// version directories no row accounts for, delete zero-version artifact
    /// rows, and delete assets and corrupt rows that belong to deleted
    /// artifacts. Live artifacts' rows are never deleted.
    #[arg(long)]
    pub fix: bool,
    /// Also check each layer of this harness's integration: the binary, the
    /// installed plugin and skill, its MCP sessions, its hooks, and feedback
    /// delivery (and, for Codex, native push).
    #[arg(long, value_enum)]
    pub agent: Option<DoctorAgent>,
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"name": name, "ok": ok, "detail": detail.into()})
}

/// A passing check that the person should still read: printed `warn`.
fn warn(name: &str, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"name": name, "ok": true, "warn": true, "detail": detail.into()})
}

/// `sample`: the daemon's `GET /api/sample` when one answers, else the home's
/// `[sample]` table read here (`local`: the provider, or why the table is
/// invalid). A bad table warns: the daemon starts with sample() off.
fn sample_check(
    daemon: Option<&serde_json::Value>,
    local: Option<Result<String, String>>,
) -> serde_json::Value {
    if let Some(d) = daemon {
        let key = d["key_env"].as_str().unwrap_or("its key variable");
        return match (d["provider"].as_str(), d["reason"].as_str()) {
            (Some(p), _) => {
                let mut detail = if p == "anthropic" {
                    format!("anthropic, key from {key}")
                } else {
                    p.to_string()
                };
                if let Some(cap) = d["daily_call_cap"].as_u64() {
                    detail.push_str(&format!(", at most {cap} calls per artifact a day"));
                }
                check("sample", true, detail)
            }
            (None, Some("bad_config")) => warn(
                "sample",
                format!(
                    "sample() is off: {}",
                    d["detail"].as_str().unwrap_or("config.toml is invalid")
                ),
            ),
            (None, Some("no_key")) => check(
                "sample",
                true,
                format!("sample() is off: {key} is not set in the daemon's environment"),
            ),
            _ => check("sample", true, "sample() is off"),
        };
    }
    match local {
        Some(Ok(p)) => check("sample", true, format!("{p} (no daemon is running)")),
        Some(Err(e)) => warn("sample", format!("sample() will be off: {e}")),
        None => check("sample", true, "no daemon is running"),
    }
}

/// `codex_push` ([`codex_push_check`]); `codex_sessions`: whether
/// every live Codex session has its Codex session ID (joined by the
/// SessionStart hook), without which push is off for it.
/// `codex_push` from the daemon's `GET /api/push` (`None` when it did not
/// answer): ok when the daemon has a `codex` (the detail names it and where it
/// came from) or push was turned off on purpose (`CLAX_CODEX_BIN` set
/// empty); failed when `codex` was not found, and when the daemon predates
/// `/api/push` (version skew).
fn codex_push_check(push: Option<&serde_json::Value>, daemon_version: &str) -> serde_json::Value {
    let Some(p) = push else {
        return check(
            "codex_push",
            false,
            format!(
                "the daemon (version {daemon_version}) does not report push; it is older than this clax ({}): run `clax stop`, then start it again",
                env!("CARGO_PKG_VERSION")
            ),
        );
    };
    let bin = p["codex"]["bin"].as_str();
    let source = p["codex"]["source"].as_str().unwrap_or_default();
    let reason = p["codex"]["reason"].as_str();
    match (bin, source) {
        (Some(b), "env") => check(
            "codex_push",
            true,
            format!("codex at {b} (from CLAX_CODEX_BIN)"),
        ),
        (Some(b), _) => check("codex_push", true, format!("codex at {b} (found on PATH)")),
        (None, "disabled") => check(
            "codex_push",
            true,
            "Codex push is off on purpose: the daemon was started with CLAX_CODEX_BIN set empty",
        ),
        (None, _) => check(
            "codex_push",
            false,
            format!(
                "{}; run `clax stop`, then start it again from a shell where `codex` is on PATH, or set CLAX_CODEX_BIN to its path",
                reason.unwrap_or("codex is not on the daemon's PATH; native push disabled")
            ),
        ),
    }
}

/// `extension`: ok when the extension's files and its native host launcher
/// and wrapper copy match this binary, and every installed browser has this
/// home's host registration; otherwise a warning
/// naming what is off and how to set it up.
fn extension_check(ext: &serde_json::Value) -> serde_json::Value {
    let hosts = ext["hosts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let bad: Vec<String> = hosts
        .iter()
        .filter(|h| h["status"] != "installed")
        .map(|h| {
            format!(
                "{} host {}",
                h["browser"].as_str().unwrap_or_default(),
                h["status"].as_str().unwrap_or_default()
            )
        })
        .collect();
    let dir = ext["dir"].as_str().unwrap_or_default();
    let fix = "run `clax extension install` (or /clax:extension in Claude Code)";
    if ext["files"] == "missing" {
        return warn(
            "extension",
            format!("the Chrome extension is not installed: {fix}"),
        );
    }
    if hosts.is_empty() {
        return warn(
            "extension",
            format!("{dir}: no supported browser (Chrome, Chromium, Brave, Edge) is installed"),
        );
    }
    let mut problems = bad;
    if ext["launcher"] != "current" {
        problems.insert(
            0,
            format!(
                "native host launcher {}",
                ext["launcher"].as_str().unwrap_or("unknown")
            ),
        );
    }
    if ext["files"] != "current" {
        problems.insert(0, "files differ from this clax's build".into());
    }
    if problems.is_empty() {
        check(
            "extension",
            true,
            format!(
                "{dir}, ID {}, host registered with {}",
                ext["extension_id"].as_str().unwrap_or_default(),
                hosts
                    .iter()
                    .filter_map(|h| h["browser"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    } else {
        warn("extension", format!("{}: {fix}", problems.join("; ")))
    }
}

/// `codex_approvals`: whether Codex would stop mid-task to ask before
/// calling a Clax tool, under its `config.toml` at `path` (`text`: its
/// contents, `None` when absent). Passes when no tool asks; warns naming
/// `clax init --agent codex` when tools ask that the person has set nothing
/// for, and names the tools the person's own settings make ask (on the tool
/// or the server), with the setting, as theirs. Profiles
/// (`codex -p`) and project config layers are not read.
fn codex_approvals_check(
    path: &Path,
    text: Result<Option<String>, String>,
    tools: &[rmcp::model::Tool],
) -> serde_json::Value {
    use crate::codex_approvals::{SETUP_COMMAND, assess};
    let a = match text.and_then(|t| {
        assess(t.as_deref(), tools).map_err(|e| format!("could not parse {} ({e})", path.display()))
    }) {
        Ok(a) => a,
        Err(e) => return check("codex_approvals", false, e),
    };
    let kept = a.kept_note(path);
    let addable = a.addable();
    if !addable.is_empty() {
        let mut d = format!(
            "Codex will stop to ask before each call of {}: run `{SETUP_COMMAND}`, which shows the lines it adds to {} and adds them once you confirm",
            addable.join(", "),
            path.display()
        );
        if let Some(k) = kept {
            d.push_str(&format!("; {k}"));
        }
        return warn("codex_approvals", d);
    }
    match kept {
        Some(k) => warn("codex_approvals", k),
        None => check(
            "codex_approvals",
            true,
            format!(
                "Codex runs every Clax tool without asking, by {} (profiles and project config layers not checked)",
                path.display()
            ),
        ),
    }
}

fn codex_checks(client: Option<&Client>) -> Vec<serde_json::Value> {
    let approvals = match doctor_agent::Dirs::from_env(|k| std::env::var(k).ok()) {
        Some(d) => {
            let path = crate::codex_approvals::config_path(&d.codex_home);
            let text = crate::codex_approvals::read_config(&path);
            codex_approvals_check(&path, text, &clax_mcp::tools::ClaxTools::tools())
        }
        None => check("codex_approvals", false, "HOME is not set"),
    };
    let mut out = vec![approvals];
    out.extend(codex_daemon_checks(client));
    out
}

fn codex_daemon_checks(client: Option<&Client>) -> Vec<serde_json::Value> {
    let Some(c) = client else {
        return vec![check(
            "codex_push",
            false,
            "no daemon is running; it finds codex on the PATH it starts with",
        )];
    };
    let mut out = vec![codex_push_check(
        c.get("/api/push").ok().as_ref(),
        &c.info.version,
    )];
    let sessions = c.get("/api/sessions?live=true").ok();
    let codex: Vec<&serde_json::Value> = sessions
        .as_ref()
        .and_then(|s| s["sessions"].as_array())
        .map(|a| a.iter().filter(|s| s["harness"] == "codex").collect())
        .unwrap_or_default();
    let missing = codex
        .iter()
        .filter(|s| s["harness_session_id"].is_null())
        .count();
    out.push(check(
        "codex_sessions",
        missing == 0,
        match (codex.len(), missing) {
            (0, _) => "no live Codex sessions".to_string(),
            (n, 0) => format!("{n} live Codex sessions, each with its Codex session ID"),
            (n, m) => format!("{m} of {n} live Codex sessions have no Codex session ID, so native push is off for them: install the clax plugin's hooks, set `features.hooks = true` in the Codex config, and trust the hooks when Codex asks"),
        },
    ));
    out
}

/// Every stored JSON column parses; the detail lists each failing row as
/// `<id>[:vN]:<column>`.
fn corrupt_rows(store: &Store) -> serde_json::Value {
    match store.corrupt_rows() {
        Err(e) => check("corrupt_rows", false, e.to_string()),
        Ok(rows) if rows.is_empty() => check("corrupt_rows", true, "no corrupt rows"),
        Ok(rows) => check(
            "corrupt_rows",
            false,
            rows.iter()
                .map(|r| match r.version {
                    Some(n) => format!("{}:v{n}:{}", r.artifact_id, r.column),
                    None => format!("{}:{}", r.artifact_id, r.column),
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
}

/// Every file recorded for each listed artifact's current version exists on
/// disk with the recorded size. Artifacts with corrupt rows are left out of the
/// listing and reported by the `corrupt_rows` check instead.
fn version_files(home: &Home, store: &Store) -> serde_json::Value {
    let artifacts = match store.list_artifacts() {
        Ok(a) => a,
        Err(e) => return check("version_files", false, e.to_string()),
    };
    let mut problems = vec![];
    for a in artifacts {
        let id = match ArtifactId::parse(&a.id) {
            Ok(id) => id,
            Err(e) => {
                problems.push(format!("{}: {e}", a.id));
                continue;
            }
        };
        let version = match store.get_version(&id, a.current_version) {
            Ok(Some(v)) => v,
            Ok(None) => {
                problems.push(format!("{}: version {} missing", a.id, a.current_version));
                continue;
            }
            Err(e) => {
                problems.push(format!("{}: {e}", a.id));
                continue;
            }
        };
        let dir = home.version_dir(&id, a.current_version);
        for (path, meta) in &version.files {
            let on_disk = if path == "index.html" {
                dir.join("index.html")
            } else {
                dir.join("files").join(path)
            };
            match std::fs::metadata(&on_disk) {
                Err(_) => problems.push(format!("missing {}:{path}", a.id)),
                Ok(m) if m.len() != meta.size => {
                    problems.push(format!("size mismatch {}:{path}", a.id))
                }
                Ok(_) => {}
            }
        }
    }
    check(
        "version_files",
        problems.is_empty(),
        if problems.is_empty() {
            "all current version files present".into()
        } else {
            problems.join(", ")
        },
    )
}

/// `<artifact>:<path under the artifact directory>` for a path inside `artifacts/`.
fn artifact_relative(home: &Home, path: &Path) -> String {
    let rel = path
        .strip_prefix(home.root().join("artifacts"))
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    rel.replacen('/', ":", 1)
}

/// Staging directories, version directories above `current_version`, and
/// zero-version artifact rows; nothing here is reachable through the API.
fn stale_files(home: &Home, store: &Store) -> serde_json::Value {
    let dirs = store.stray_version_dirs();
    let zero = store.zero_version_artifacts();
    let (dirs, zero) = match (dirs, zero) {
        (Ok(d), Ok(z)) => (d, z),
        (Err(e), _) | (_, Err(e)) => return check("stale_files", false, e.to_string()),
    };
    let problems: Vec<String> = dirs
        .iter()
        .map(|d| artifact_relative(home, d))
        .chain(
            zero.iter()
                .map(|id| format!("{id}: zero-version artifact row")),
        )
        .collect();
    check(
        "stale_files",
        problems.is_empty(),
        if problems.is_empty() {
            "no stale files".into()
        } else {
            problems.join(", ")
        },
    )
}

/// Every asset row of a live artifact has its file with the recorded size, and
/// no upload temp files linger. Rows of deleted artifacts are not reported: their
/// files are gone by design, and `--fix` deletes the rows.
fn assets(home: &Home, store: &Store) -> serde_json::Value {
    let (rows, tmps) = match (store.list_all_asset_rows(), store.stray_asset_temp_files()) {
        (Ok(r), Ok(t)) => (r, t),
        (Err(e), _) | (_, Err(e)) => return check("assets", false, e.to_string()),
    };
    let mut problems = vec![];
    for row in rows.iter().filter(|r| !r.artifact_deleted) {
        let name = format!("{}:{}", row.asset.artifact_id, row.asset.id);
        match std::fs::metadata(&row.path) {
            Err(_) => problems.push(format!("missing {name}")),
            Ok(m) if m.len() != row.asset.size => problems.push(format!("size mismatch {name}")),
            Ok(_) => {}
        }
    }
    problems.extend(
        tmps.iter()
            .map(|t| format!("temp file {}", artifact_relative(home, t))),
    );
    check(
        "assets",
        problems.is_empty(),
        if problems.is_empty() {
            "all asset files present".into()
        } else {
            problems.join(", ")
        },
    )
}

fn remove_dir_if_present(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Applies the repairs listed on [`Args::fix`]; returns one line per repair
/// made, naming the paths and IDs removed.
fn apply_fixes(home: &Home, store: &Store) -> anyhow::Result<Vec<String>> {
    let mut fixed = vec![];
    let dirs = store.stray_version_dirs()?;
    for d in &dirs {
        remove_dir_if_present(d)?;
    }
    if !dirs.is_empty() {
        let names: Vec<_> = dirs.iter().map(|d| artifact_relative(home, d)).collect();
        fixed.push(format!(
            "removed stale version directories: {}",
            names.join(", ")
        ));
    }
    let tmps = store.stray_asset_temp_files()?;
    for t in &tmps {
        match std::fs::remove_file(t) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if !tmps.is_empty() {
        let names: Vec<_> = tmps.iter().map(|t| artifact_relative(home, t)).collect();
        fixed.push(format!("removed stray temp files: {}", names.join(", ")));
    }
    let zero = store.zero_version_artifacts()?;
    store.delete_zero_version_artifacts()?;
    if !zero.is_empty() {
        fixed.push(format!(
            "deleted zero-version artifact rows: {}",
            zero.join(", ")
        ));
    }
    let assets: Vec<String> = store
        .list_all_asset_rows()?
        .into_iter()
        .filter(|r| r.artifact_deleted)
        .map(|r| format!("{}:{}", r.asset.artifact_id, r.asset.id))
        .collect();
    store.delete_assets_of_deleted_artifacts()?;
    if !assets.is_empty() {
        fixed.push(format!(
            "deleted asset rows of deleted artifacts: {}",
            assets.join(", ")
        ));
    }
    let before = store.corrupt_rows()?;
    store.delete_corrupt_deleted_rows()?;
    let after = store.corrupt_rows()?;
    let cleared: Vec<String> = before
        .iter()
        .filter(|r| !after.contains(r))
        .map(|r| match r.version {
            Some(n) => format!("{}:v{n}:{}", r.artifact_id, r.column),
            None => format!("{}:{}", r.artifact_id, r.column),
        })
        .collect();
    if !cleared.is_empty() {
        fixed.push(format!(
            "deleted corrupt rows of deleted artifacts: {}",
            cleared.join(", ")
        ));
    }
    Ok(fixed)
}

/// `journal`: the audit journal, from the daemon's `GET
/// /api/toolpath/status` (spec §3.5). It warns when the journal is behind
/// with an error, when it has trailed the table for more than 10 s, when it
/// is off because `[toolpath]` is invalid, and when retention could not
/// remove a segment; a journal turned off on purpose is fine. Without a
/// daemon (`daemon` false) there is nothing to read.
fn journal_check(daemon: bool, status: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(v) = status else {
        return if daemon {
            warn(
                "journal",
                "could not read the journal status from the daemon",
            )
        } else {
            check("journal", true, "not checked: the daemon is not running")
        };
    };
    let segment = v["segment"].as_str().unwrap_or("no segment yet");
    let error = v["last_error"].as_str();
    if v["journal"] != true {
        return match error {
            Some(e) => warn("journal", e),
            None => check("journal", true, "off ([toolpath] journal = false)"),
        };
    }
    let lag = v["lag_ms"].as_i64().unwrap_or(0);
    let (cursor, newest) = (&v["cursor"], &v["newest_seq"]);
    let at = format!("{segment}, event {cursor} of {newest}");
    match (error, v["warning"].as_str()) {
        (Some(e), _) => warn(
            "journal",
            format!("behind at event {cursor} of {newest}: {e}"),
        ),
        (None, _) if lag > 10_000 => warn(
            "journal",
            format!("behind for {} s at event {cursor} of {newest}", lag / 1000),
        ),
        (None, Some(w)) => warn("journal", format!("{at}; {w}")),
        (None, None) => check("journal", true, at),
    }
}

/// `config`: the home's `config.toml` reads and parses, its `[serve] port`,
/// when present, is a port, and so is `CLAX_PORT` (`env`), when set. The
/// detail names the port daemons for this home start on and where it comes
/// from, or the error.
fn config_check(home: &Home, env: Option<std::ffi::OsString>) -> serde_json::Value {
    let file = clax_core::config::HomeConfig::load(home.root()).and_then(|c| c.serve_port());
    match (crate::env_port(env), file) {
        (Some(Err(e)), _) => check("config", false, e.to_string()),
        (_, Err(e)) => check("config", false, e.to_string()),
        (Some(Ok(p)), Ok(Some(f))) => check(
            "config",
            true,
            format!("{}={p} (overrides [serve] port = {f})", crate::PORT_ENV),
        ),
        (Some(Ok(p)), Ok(None)) => check("config", true, format!("{}={p}", crate::PORT_ENV)),
        (None, Ok(Some(p))) => check("config", true, format!("[serve] port = {p}")),
        (None, Ok(None)) => check(
            "config",
            true,
            format!("default port {}", clax_server::daemon::DEFAULT_PORT),
        ),
    }
}

pub fn run(cli: &crate::Cli, home: &Home, args: &Args) -> anyhow::Result<()> {
    let mut checks = vec![];
    let mut fixed = vec![];
    let writable = home.ensure_dirs().is_ok()
        && std::fs::write(home.root().join(".doctor"), b"")
            .map(|_| std::fs::remove_file(home.root().join(".doctor")).is_ok())
            .unwrap_or(false);
    checks.push(check(
        "build",
        true,
        format!(
            "clax {} built from commit {}",
            env!("CARGO_PKG_VERSION"),
            clax_core::build_commit()
        ),
    ));
    checks.push(check("home", writable, home.root().display().to_string()));
    checks.push(config_check(home, std::env::var_os(crate::PORT_ENV)));
    // Held until the process exits, so an auto-start cannot race the repairs.
    // Taken before discovery: a daemon that is still starting holds it.
    let _lock = if args.fix && writable {
        Some(DaemonLock::acquire(home).context("acquiring daemon lock")?)
    } else {
        None
    };
    let client = Client::discover(home);
    if args.fix && client.is_some() {
        eprintln!("error: doctor --fix needs the daemon stopped; run `clax stop` first");
        std::process::exit(1);
    }
    checks.push(check(
        "daemon",
        client.is_some(),
        client
            .as_ref()
            .map(|c| c.base.clone())
            .unwrap_or("not running".into()),
    ));
    let mode = std::fs::metadata(home.daemon_json())
        .map(|m| m.permissions().mode() & 0o777)
        .ok();
    checks.push(check(
        "daemon_json_mode",
        mode.is_none_or(|m| m == 0o600),
        mode.map(|m| format!("{m:o}")).unwrap_or("absent".into()),
    ));
    match Store::open(home) {
        Ok(store) => {
            // Whole-home checks may read for longer than a request may.
            store.lift_read_limit();
            if args.fix {
                fixed = apply_fixes(home, &store)?;
            }
            let integrity = store.integrity_check();
            checks.push(check(
                "db_integrity",
                matches!(&integrity, Ok(s) if s == "ok"),
                integrity.unwrap_or_else(|e| e.to_string()),
            ));
            checks.push(corrupt_rows(&store));
            checks.push(version_files(home, &store));
            checks.push(stale_files(home, &store));
            checks.push(assets(home, &store));
        }
        Err(e) => {
            checks.push(check("db_integrity", false, e.to_string()));
            checks.push(check("corrupt_rows", false, "store unavailable"));
            checks.push(check("version_files", false, "store unavailable"));
            checks.push(check("stale_files", false, "store unavailable"));
            checks.push(check("assets", false, "store unavailable"));
        }
    }
    let ui = client
        .as_ref()
        .map(|c| c.http_status("/") == Some(200))
        .unwrap_or(false);
    checks.push(check(
        "ui",
        ui,
        if ui {
            "shell served"
        } else {
            "UI not built or daemon down; run `just web`"
        },
    ));
    let local = clax_core::config::HomeConfig::load(home.root())
        .and_then(|c| c.sample())
        .map(|s| s.provider)
        .map_err(|e| e.to_string());
    checks.push(sample_check(
        client
            .as_ref()
            .and_then(|c| c.get("/api/sample").ok())
            .as_ref(),
        Some(local),
    ));
    checks.push(extension_check(&super::extension::status(home)));
    checks.push(journal_check(
        client.is_some(),
        client
            .as_ref()
            .and_then(|c| c.get("/api/toolpath/status").ok())
            .as_ref(),
    ));
    if let Some(agent) = args.agent {
        checks.extend(doctor_agent::checks(agent, home, client.as_ref()));
    }
    if let Some(DoctorAgent::Codex) = args.agent {
        checks.extend(codex_checks(client.as_ref()));
    }
    let ok = checks.iter().all(|c| c["ok"].as_bool().unwrap());
    super::print(
        cli,
        serde_json::json!({"ok": ok, "checks": checks, "fixed": fixed}),
        |j| {
            let fixes = j["fixed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| format!("fix  {}", f.as_str().unwrap()));
            fixes
                .chain(j["checks"].as_array().unwrap().iter().map(|c| {
                    format!(
                        "{} {:<18} {}",
                        if c["warn"].as_bool() == Some(true) {
                            "warn"
                        } else if c["ok"].as_bool().unwrap() {
                            "ok  "
                        } else {
                            "FAIL"
                        },
                        c["name"].as_str().unwrap(),
                        c["detail"].as_str().unwrap()
                    )
                }))
                .collect::<Vec<_>>()
                .join("\n")
        },
    );
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        codex_approvals_check, codex_push_check, config_check, extension_check, journal_check,
    };
    use serde_json::{Value, json};

    #[test]
    fn the_config_check_names_a_clax_port_that_overrides_or_is_not_a_port() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        let env = |v: &str| Some(std::ffi::OsString::from(v));
        assert_eq!(config_check(&home, None)["detail"], "default port 7480");
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 7481\n").unwrap();
        assert_eq!(config_check(&home, None)["detail"], "[serve] port = 7481");
        assert_eq!(
            config_check(&home, env(""))["detail"],
            "[serve] port = 7481"
        );
        let over = config_check(&home, env("7490"));
        assert_eq!(over["ok"], true, "{over}");
        assert_eq!(
            over["detail"],
            "CLAX_PORT=7490 (overrides [serve] port = 7481)"
        );
        let bad = config_check(&home, env("x"));
        assert_eq!(bad["ok"], false, "{bad}");
        assert!(
            bad["detail"].as_str().unwrap().contains("CLAX_PORT"),
            "{bad}"
        );
    }

    #[test]
    fn the_codex_approvals_check_names_the_setup_command() {
        let tools = clax_mcp::tools::ClaxTools::tools();
        let p = std::path::Path::new("/cx/config.toml");
        let none = codex_approvals_check(p, Ok(None), &tools);
        assert_eq!(none["warn"], true, "{none}");
        let d = none["detail"].as_str().unwrap();
        assert!(d.contains("clax init --agent codex"), "{d}");
        assert!(d.contains("delete, ") || d.contains(", delete"), "{d}");
        assert!(!d.contains("publish"), "{d}");
        let approved = crate::codex_approvals::add_approvals(
            "",
            &crate::codex_approvals::assess(None, &tools)
                .unwrap()
                .addable(),
        )
        .unwrap();
        let ok = codex_approvals_check(p, Ok(Some(approved.clone())), &tools);
        assert_eq!(
            (ok["ok"].clone(), ok["warn"].clone()),
            (json!(true), Value::Null),
            "{ok}"
        );
        let own = approved.replace(
            "tools.delete]\napproval_mode = \"approve\"",
            "tools.delete]\napproval_mode = \"prompt\"",
        );
        let kept = codex_approvals_check(p, Ok(Some(own)), &tools);
        assert_eq!(kept["warn"], true, "{kept}");
        let d = kept["detail"].as_str().unwrap();
        assert!(
            d.contains("delete (your approval_mode = \"prompt\" on `delete`)"),
            "{d}"
        );
        assert!(!d.contains("clax init"), "{d}");
        let server = codex_approvals_check(
            p,
            Ok(Some(
                "[plugins.\"clax@clax\".mcp_servers.clax]\ndefault_tools_approval_mode = \"prompt\"\n"
                    .into(),
            )),
            &tools,
        );
        let d = server["detail"].as_str().unwrap();
        assert_eq!(server["warn"], true, "{server}");
        assert!(
            d.contains("(your default_tools_approval_mode = \"prompt\")"),
            "{d}"
        );
        assert!(
            !d.contains("clax init"),
            "the person's server default is theirs: {d}"
        );
        let bad = codex_approvals_check(p, Ok(Some("[x\n".into())), &tools);
        assert_eq!(bad["ok"], false, "{bad}");
        let unreadable = codex_approvals_check(p, Err("could not read".into()), &tools);
        assert_eq!(unreadable["ok"], false, "{unreadable}");
    }
    #[test]
    fn the_extension_check_warns_until_every_browser_is_registered() {
        let ok = extension_check(&serde_json::json!({
            "dir": "/h/extension", "extension_id": "abc", "files": "current", "launcher": "current",
            "hosts": [{"browser": "chrome", "status": "installed"}]
        }));
        assert_eq!(ok["ok"], true);
        assert!(ok["warn"].is_null(), "{ok}");
        let launcher = extension_check(&serde_json::json!({
            "dir": "/h/extension", "files": "current", "launcher": "stale",
            "hosts": [{"browser": "chrome", "status": "installed"}]
        }));
        assert_eq!(launcher["warn"], true);
        assert!(
            launcher["detail"]
                .as_str()
                .unwrap()
                .contains("launcher stale"),
            "{launcher}"
        );
        let missing = extension_check(
            &serde_json::json!({"dir": "/h/extension", "files": "missing", "hosts": []}),
        );
        assert_eq!(missing["warn"], true);
        assert!(
            missing["detail"]
                .as_str()
                .unwrap()
                .contains("clax extension install")
        );
        let stale = extension_check(&serde_json::json!({
            "dir": "/h/extension", "files": "stale",
            "hosts": [{"browser": "chrome", "status": "installed"}, {"browser": "brave", "status": "stale"}]
        }));
        assert_eq!(stale["warn"], true);
        let d = stale["detail"].as_str().unwrap();
        assert!(
            d.contains("files differ") && d.contains("brave host stale"),
            "{d}"
        );
    }

    #[test]
    fn the_sample_line_reports_the_daemon_or_the_file_and_warns_on_a_bad_table() {
        use super::sample_check;
        let on = sample_check(
            Some(
                &json!({"available": true, "provider": "anthropic", "reason": null, "detail": null, "key_env": "ANTHROPIC_API_KEY", "daily_call_cap": 200}),
            ),
            None,
        );
        assert_eq!(
            (on["ok"].clone(), on["warn"].clone()),
            (json!(true), Value::Null)
        );
        assert_eq!(
            on["detail"],
            "anthropic, key from ANTHROPIC_API_KEY, at most 200 calls per artifact a day"
        );
        let off = sample_check(
            Some(
                &json!({"available": false, "provider": null, "reason": "no_key", "detail": null, "key_env": "ANTHROPIC_API_KEY", "daily_call_cap": null}),
            ),
            None,
        );
        assert_eq!(off["ok"], true);
        assert!(
            off["detail"]
                .as_str()
                .unwrap()
                .contains("ANTHROPIC_API_KEY is not set in the daemon's environment")
        );
        let bad = sample_check(
            Some(
                &json!({"available": false, "provider": null, "reason": "bad_config", "detail": "config.toml: [sample] provider is \"anthropic\" or \"stub\", not \"openai\"", "key_env": null, "daily_call_cap": null}),
            ),
            None,
        );
        assert_eq!(
            (bad["ok"].clone(), bad["warn"].clone()),
            (json!(true), json!(true))
        );
        assert!(
            bad["detail"]
                .as_str()
                .unwrap()
                .starts_with("sample() is off: config.toml: [sample]")
        );
        let local = sample_check(
            None,
            Some(Err(
                "config.toml: [sample] max_tokens must be at least 1".into()
            )),
        );
        assert_eq!(local["warn"], true);
        let local = sample_check(None, Some(Ok("stub".into())));
        assert_eq!(local["detail"], "stub (no daemon is running)");
    }

    #[test]
    fn codex_push_is_ok_when_off_on_purpose_and_names_version_skew() {
        let off = codex_push_check(
            Some(
                &json!({"codex": {"available": false, "bin": null, "source": "disabled", "reason": "x"}}),
            ),
            "0.2.0",
        );
        assert_eq!(off["ok"], true);
        assert!(
            off["detail"].as_str().unwrap().contains("off on purpose"),
            "{off}"
        );
        let old = codex_push_check(None, "0.1.0");
        assert_eq!(old["ok"], false);
        let detail = old["detail"].as_str().unwrap();
        assert!(
            detail.contains("0.1.0") && detail.contains("older") && !detail.contains("PATH"),
            "{detail}"
        );
        let bad = codex_push_check(
            Some(
                &json!({"codex": {"available": false, "bin": null, "source": "not_found",
                "reason": "CLAX_CODEX_BIN names /x, which is not an executable file; native push disabled"}}),
            ),
            "0.2.0",
        );
        assert_eq!(bad["ok"], false);
        assert!(
            bad["detail"]
                .as_str()
                .unwrap()
                .starts_with("CLAX_CODEX_BIN names /x"),
            "{bad}"
        );
    }

    #[test]
    fn the_journal_line_warns_when_it_is_behind() {
        let status = |journal: bool, lag: Value, error: Value| {
            json!({"journal": journal, "dir": "/h/toolpath/journal",
                   "segment": "clax-6a1f0c3e-20261006-001.path.jsonl", "cursor": 40,
                   "newest_seq": 42, "lag_ms": lag, "last_error": error})
        };
        let line = |v: Option<&Value>| {
            let c = journal_check(v.is_some(), v);
            (
                c["ok"].as_bool().unwrap(),
                c["warn"].as_bool().unwrap_or(false),
                c["detail"].as_str().unwrap().to_string(),
            )
        };
        let (ok, warned, detail) = line(Some(&status(true, json!(12), Value::Null)));
        assert!(ok && !warned);
        assert_eq!(
            detail,
            "clax-6a1f0c3e-20261006-001.path.jsonl, event 40 of 42"
        );
        let (ok, warned, detail) = line(Some(&status(true, json!(12_500), Value::Null)));
        assert!(ok && warned);
        assert_eq!(detail, "behind for 12 s at event 40 of 42");
        let (_, warned, detail) = line(Some(&status(
            true,
            json!(300),
            json!("writing the journal: No space left on device (os error 28)"),
        )));
        assert!(warned && detail.contains("os error 28"), "{detail}");
        let (ok, warned, detail) = line(Some(&status(false, Value::Null, Value::Null)));
        assert!(ok && !warned && detail.contains("journal = false"));
        let (_, warned, _) = line(Some(&status(
            false,
            Value::Null,
            json!("journal off: [toolpath] ..."),
        )));
        assert!(warned);
        let (ok, warned, _) = line(None);
        assert!(ok && !warned);
        let c = journal_check(true, None);
        assert_eq!(c["warn"], true);
        assert!(c["detail"].as_str().unwrap().contains("could not read"));
        let mut v = status(true, json!(0), Value::Null);
        v["warning"] = json!("journal_retain_days: removing x: Permission denied (os error 13)");
        let (ok, warned, detail) = line(Some(&v));
        assert!(ok && warned && detail.contains("os error 13"), "{detail}");
    }
}

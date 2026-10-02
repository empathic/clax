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

fn codex_checks(client: Option<&Client>) -> Vec<serde_json::Value> {
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

/// `config`: the home's `config.toml` reads and parses, and its `[serve]
/// port`, when present, is a port. The detail names the port daemons for
/// this home start on, or the error.
fn config_check(home: &Home) -> serde_json::Value {
    match clax_core::config::HomeConfig::load(home.root()).and_then(|c| c.serve_port()) {
        Ok(Some(p)) => check("config", true, format!("[serve] port = {p}")),
        Ok(None) => check(
            "config",
            true,
            format!("default port {}", clax_server::daemon::DEFAULT_PORT),
        ),
        Err(e) => check("config", false, e.to_string()),
    }
}

pub fn run(cli: &crate::Cli, home: &Home, args: &Args) -> anyhow::Result<()> {
    let mut checks = vec![];
    let mut fixed = vec![];
    let writable = home.ensure_dirs().is_ok()
        && std::fs::write(home.root().join(".doctor"), b"")
            .map(|_| std::fs::remove_file(home.root().join(".doctor")).is_ok())
            .unwrap_or(false);
    checks.push(check("home", writable, home.root().display().to_string()));
    checks.push(config_check(home));
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
    use super::codex_push_check;
    use serde_json::{Value, json};
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
}

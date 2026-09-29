use crate::client::Client;
use anyhow::Context;
use artifax_core::{ArtifactId, Home, Store};
use artifax_server::daemon::DaemonLock;
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
    /// Also check native push for this harness's sessions.
    #[arg(long, value_enum)]
    pub agent: Option<DoctorAgent>,
}

/// A harness whose native push `doctor --agent` checks.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum DoctorAgent {
    Codex,
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"name": name, "ok": ok, "detail": detail.into()})
}

/// `codex_push`: whether the running daemon has a `codex` to run `codex queue`
/// with, naming the binary and where it came from; `codex_sessions`: whether
/// every live Codex session has its Codex session ID (joined by the
/// SessionStart hook), without which push is off for it.
fn codex_checks(client: Option<&Client>) -> Vec<serde_json::Value> {
    let Some(c) = client else {
        return vec![check(
            "codex_push",
            false,
            "no daemon is running; it finds codex on the PATH it starts with",
        )];
    };
    let push = c.get("/api/push").ok();
    let bin = push
        .as_ref()
        .and_then(|p| p["codex"]["bin"].as_str().map(str::to_string));
    let source = push
        .as_ref()
        .and_then(|p| p["codex"]["source"].as_str().map(str::to_string))
        .unwrap_or_default();
    let detail = match (bin.as_deref(), source.as_str()) {
        (Some(b), "env") => format!("codex at {b} (from ARTIFAX_CODEX_BIN)"),
        (Some(b), _) => format!("codex at {b} (found on PATH)"),
        (None, "disabled") => "Codex push is off: the daemon was started with ARTIFAX_CODEX_BIN set empty".to_string(),
        (None, _) => "codex is not on the daemon's PATH; run `artifax stop`, then start it again from a shell where `codex` is on PATH, or set ARTIFAX_CODEX_BIN".to_string(),
    };
    let mut out = vec![check("codex_push", bin.is_some(), detail)];
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
            (n, m) => format!("{m} of {n} live Codex sessions have no Codex session ID, so native push is off for them: install the artifax plugin's hooks, set `features.hooks = true` in the Codex config, and trust the hooks when Codex asks"),
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

pub fn run(cli: &crate::Cli, home: &Home, args: &Args) -> anyhow::Result<()> {
    let mut checks = vec![];
    let mut fixed = vec![];
    let writable = home.ensure_dirs().is_ok()
        && std::fs::write(home.root().join(".doctor"), b"")
            .map(|_| std::fs::remove_file(home.root().join(".doctor")).is_ok())
            .unwrap_or(false);
    checks.push(check("home", writable, home.root().display().to_string()));
    // Held until the process exits, so an auto-start cannot race the repairs.
    // Taken before discovery: a daemon that is still starting holds it.
    let _lock = if args.fix && writable {
        Some(DaemonLock::acquire(home).context("acquiring daemon lock")?)
    } else {
        None
    };
    let client = Client::discover(home);
    if args.fix && client.is_some() {
        eprintln!("error: doctor --fix needs the daemon stopped; run `artifax stop` first");
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
                        if c["ok"].as_bool().unwrap() {
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

use crate::client::Client;
use artifax_core::{ArtifactId, Home, Store};
use std::os::unix::fs::PermissionsExt;

fn check(name: &str, ok: bool, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"name": name, "ok": ok, "detail": detail.into()})
}

/// Every file recorded for each artifact's current version exists on disk
/// with the recorded size.
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

pub fn run(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let mut checks = vec![];
    let writable = home.ensure_dirs().is_ok()
        && std::fs::write(home.root().join(".doctor"), b"")
            .map(|_| std::fs::remove_file(home.root().join(".doctor")).is_ok())
            .unwrap_or(false);
    checks.push(check("home", writable, home.root().display().to_string()));
    let client = Client::discover(home);
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
            let integrity = store.integrity_check();
            checks.push(check(
                "db_integrity",
                matches!(&integrity, Ok(s) if s == "ok"),
                integrity.unwrap_or_else(|e| e.to_string()),
            ));
            checks.push(version_files(home, &store));
        }
        Err(e) => {
            checks.push(check("db_integrity", false, e.to_string()));
            checks.push(check("version_files", false, "store unavailable"));
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
    let ok = checks.iter().all(|c| c["ok"].as_bool().unwrap());
    super::print(cli, serde_json::json!({"ok": ok, "checks": checks}), |j| {
        j["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
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
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

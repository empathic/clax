//! `clax extension install | uninstall | status` (spec 2026-10-05 §6.6):
//! the unpacked extension under `<home>/extension`, its native host
//! launcher, and a host manifest for each installed browser.
//!
//! `install` writes the embedded build ([`crate::extension_files::files`])
//! to a staging directory beside `<home>/extension` and swaps it into place,
//! so files an older build had and this one lacks are gone. It adds
//! `host/ensure-clax.sh` (the plugins' wrapper this binary carries) and
//! `host/launch.sh`, which fixes `CLAX_HOME` to this home and execs the
//! wrapper's copy with `exec native-host`, since Chrome starts the host with
//! a minimal environment and no arguments. It then writes
//! `dev.empathic.clax.json` into each browser's hosts directory whose
//! profile directory exists, with `allowed_origins` exactly the extension's
//! origin under the ID in effect for this home, and records the manifests it
//! wrote in `installed.json`.
//!
//! `uninstall` removes exactly the manifests `installed.json` lists whose
//! `path` is this home's `launch.sh`, then `<home>/extension`. `status`
//! reports whether the files match this binary's build and, per installed
//! browser, whether its manifest is `installed`, `missing`, or `stale`
//! (another path or origin). Both `install` and `uninstall` hold the init
//! lock (`<home>/init.lock`), which `clax init` and `clax uninit` already
//! hold when they call them.

use crate::extension_files::{HostDir, files, host_dirs};
use clax_core::Home;
use clax_core::extension::{HOST_NAME, extension_id_in_effect, extension_origin};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Write the extension to <home>/extension and register its native
    /// messaging host with each installed browser (Chrome, Chromium, Brave,
    /// Edge).
    Install,
    /// Remove the host registrations install wrote, and <home>/extension.
    Uninstall,
    /// Report whether the extension's files match this binary, its ID, and
    /// each installed browser's host registration.
    Status,
}

/// The first part of the one-time step in Chrome; the directory follows.
const LOAD_UNPACKED: &str =
    "In Chrome, open chrome://extensions, turn on Developer mode, choose Load unpacked, and pick";
/// Prefix of the staging and retired directories beside `<home>/extension`.
const SCRATCH: &str = ".extension-";

fn ext_dir(home: &Home) -> PathBuf {
    home.root().join("extension")
}

fn launcher(home: &Home) -> PathBuf {
    ext_dir(home).join("host/launch.sh")
}

fn manifest_file(d: &HostDir) -> PathBuf {
    d.dir.join(format!("{HOST_NAME}.json"))
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// The origin the host manifests allow: `chrome-extension://<ID>/`.
fn allowed_origin(home: &Home) -> String {
    format!(
        "{}/",
        extension_origin(&extension_id_in_effect(home.root()))
    )
}

/// `s` quoted for a POSIX shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Each browser's hosts directory with whether the browser is installed:
/// its profile directory (the hosts directory's parent) exists, or, with
/// `CLAX_NATIVE_HOST_DIRS`, the listed directory itself does.
fn browsers() -> Vec<(HostDir, bool)> {
    let listed = env("CLAX_NATIVE_HOST_DIRS").is_some();
    host_dirs(&env)
        .into_iter()
        .map(|d| {
            let profile = if listed {
                Some(d.dir.as_path())
            } else {
                d.dir.parent()
            };
            let installed = profile.is_some_and(Path::is_dir);
            (d, installed)
        })
        .collect()
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

/// The manifests `installed.json` lists.
fn recorded_hosts(home: &Home) -> Vec<String> {
    read_json(&ext_dir(home).join("installed.json"))
        .and_then(|v| serde_json::from_value(v["hosts"].clone()).ok())
        .unwrap_or_default()
}

/// Whether the manifest at `path` names this home's launcher.
fn is_ours(path: &str, launch: &str) -> bool {
    read_json(Path::new(path)).is_some_and(|m| m["path"] == launch)
}

fn write_exec(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// Removes staging and retired directories an interrupted run left.
fn remove_scratch(home: &Home) {
    for e in std::fs::read_dir(home.root())
        .into_iter()
        .flatten()
        .flatten()
    {
        if e.file_name().to_string_lossy().starts_with(SCRATCH) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The launcher Chrome runs (spec L14).
fn launch_script(home: &Home) -> String {
    format!(
        "#!/bin/sh\n# Launches the Clax native messaging host for the Clax Chrome extension.\nCLAX_HOME={home}\nexport CLAX_HOME\nexec {wrapper} exec native-host \"$@\"\n",
        home = sh_quote(&home.root().display().to_string()),
        wrapper = sh_quote(
            &ext_dir(home)
                .join("host/ensure-clax.sh")
                .display()
                .to_string()
        ),
    )
}

/// Writes the extension into a staging directory, then swaps it in for
/// `<home>/extension`; the previous directory is put back if the swap fails.
fn write_files(home: &Home, fs: &[(String, Vec<u8>)]) -> anyhow::Result<()> {
    let dir = ext_dir(home);
    let pid = std::process::id();
    let staging = home.root().join(format!("{SCRATCH}new-{pid}"));
    let retired = home.root().join(format!("{SCRATCH}old-{pid}"));
    let build = || -> anyhow::Result<()> {
        for (p, bytes) in fs {
            let dest = staging.join(p);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(dest, bytes)?;
        }
        std::fs::create_dir_all(staging.join("host"))?;
        let wrapper = crate::plugins::files()
            .into_iter()
            .find(|(p, _)| p == "plugins/claude-code/scripts/ensure-clax.sh")
            .map(|(_, b)| b)
            .ok_or_else(|| anyhow::anyhow!("the plugins' wrapper is missing from this build"))?;
        write_exec(&staging.join("host/ensure-clax.sh"), &wrapper)?;
        write_exec(
            &staging.join("host/launch.sh"),
            launch_script(home).as_bytes(),
        )?;
        Ok(())
    };
    if let Err(e) = build() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let had = dir.exists();
    if had {
        std::fs::rename(&dir, &retired)?;
    }
    if let Err(e) = std::fs::rename(&staging, &dir) {
        if had {
            let _ = std::fs::rename(&retired, &dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.into());
    }
    if had {
        let _ = std::fs::remove_dir_all(&retired);
    }
    Ok(())
}

/// Writes the extension, its launcher and the host manifests (see the
/// module documentation). Fails when this binary carries no extension build.
pub fn install(home: &Home) -> anyhow::Result<Value> {
    let fs = files();
    if !fs.iter().any(|(p, _)| p == "manifest.json") {
        anyhow::bail!(
            "this clax was built without the extension (run scripts/build-web.sh, then rebuild clax)"
        );
    }
    std::fs::create_dir_all(home.root())?;
    remove_scratch(home);
    let dir = ext_dir(home);
    let launch = launcher(home).display().to_string();
    let previous = recorded_hosts(home);
    write_files(home, &fs)?;
    let manifest = json!({
        "name": HOST_NAME,
        "description": "Clax: pairs the Clax extension with the local Clax daemon",
        "path": launch,
        "type": "stdio",
        "allowed_origins": [allowed_origin(home)],
    });
    let body = serde_json::to_vec_pretty(&manifest)?;
    let mut hosts = Vec::new();
    let mut written: Vec<String> = Vec::new();
    for (d, installed) in browsers() {
        let file = manifest_file(&d);
        if !installed {
            hosts.push(
                json!({"browser": d.browser, "status": "skipped", "detail": "not installed"}),
            );
            continue;
        }
        let res = std::fs::create_dir_all(&d.dir).and_then(|_| std::fs::write(&file, &body));
        match res {
            Ok(()) => {
                written.push(file.display().to_string());
                hosts.push(json!({"browser": d.browser, "status": "installed", "path": file}));
            }
            Err(e) => hosts.push(
                json!({"browser": d.browser, "status": "failed", "path": file, "detail": e.to_string()}),
            ),
        }
    }
    // Manifests an earlier install wrote where this one did not (a browser
    // since removed, or another directory list) stay recorded while they
    // are still this home's, so uninstall removes them too.
    let mut recorded = written.clone();
    for p in previous {
        if !recorded.contains(&p) && is_ours(&p, &launch) {
            recorded.push(p);
        }
    }
    std::fs::write(
        dir.join("installed.json"),
        serde_json::to_vec_pretty(
            &json!({"version": env!("CARGO_PKG_VERSION"), "hosts": recorded}),
        )?,
    )?;
    Ok(json!({
        "status": "installed",
        "dir": dir,
        "extension_id": extension_id_in_effect(home.root()),
        "hosts": hosts,
        "load_unpacked": format!("{LOAD_UNPACKED} {} (once).", dir.display()),
    }))
}

/// Removes the manifests install recorded that still name this home's
/// launcher, then `<home>/extension`.
pub fn uninstall(home: &Home) -> anyhow::Result<Value> {
    let dir = ext_dir(home);
    let launch = launcher(home).display().to_string();
    let mut removed = Vec::new();
    for path in recorded_hosts(home) {
        if is_ours(&path, &launch) && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    let had = dir.exists();
    if had {
        std::fs::remove_dir_all(&dir)?;
    }
    if home.root().is_dir() {
        remove_scratch(home);
    }
    Ok(json!({
        "status": if had { "removed" } else { "absent" },
        "hosts_removed": removed,
        "note": "Remove the Clax extension at chrome://extensions too.",
    }))
}

/// The extension's state: `files` is `current` when every file of this
/// binary's build is on disk as built, `stale` when not, `missing` without
/// a manifest; `hosts` lists each installed browser's registration.
pub fn status(home: &Home) -> Value {
    let dir = ext_dir(home);
    let launch = launcher(home).display().to_string();
    let origin = allowed_origin(home);
    let files_state = if !dir.join("manifest.json").is_file() {
        "missing"
    } else if files()
        .iter()
        .all(|(p, b)| std::fs::read(dir.join(p)).ok().as_deref() == Some(b.as_slice()))
    {
        "current"
    } else {
        "stale"
    };
    let hosts: Vec<Value> = browsers()
        .into_iter()
        .filter(|(_, installed)| *installed)
        .map(|(d, _)| {
            let file = manifest_file(&d);
            let status = match read_json(&file) {
                None => "missing",
                Some(m)
                    if m["path"] == launch.as_str() && m["allowed_origins"] == json!([origin]) =>
                {
                    "installed"
                }
                Some(_) => "stale",
            };
            json!({"browser": d.browser, "status": status, "path": file})
        })
        .collect();
    json!({
        "dir": dir,
        "extension_id": extension_id_in_effect(home.root()),
        "files": files_state,
        "hosts": hosts,
    })
}

/// One line per browser: `  <browser>: <status> (<path or detail>)`.
pub fn host_lines(v: &Value) -> Vec<String> {
    v["hosts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| {
            let mut l = format!(
                "  {}: {}",
                h["browser"].as_str().unwrap_or_default(),
                h["status"].as_str().unwrap_or_default()
            );
            if let Some(d) = h["detail"].as_str().or_else(|| h["path"].as_str()) {
                l.push_str(&format!(" ({d})"));
            }
            l
        })
        .collect()
}

pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let out = match cmd {
        Cmd::Install => {
            let _lock = super::init::InitLock::acquire(home)?;
            install(home)?
        }
        Cmd::Uninstall => {
            let _lock = if home.root().is_dir() {
                Some(super::init::InitLock::acquire(home)?)
            } else {
                None
            };
            uninstall(home)?
        }
        Cmd::Status => status(home),
    };
    super::print(cli, out, |j| {
        let mut lines = Vec::new();
        match cmd {
            Cmd::Install => {
                lines.push(format!(
                    "extension: installed ({})",
                    j["dir"].as_str().unwrap_or_default()
                ));
                lines.extend(host_lines(j));
                lines.push(j["load_unpacked"].as_str().unwrap_or_default().to_string());
            }
            Cmd::Uninstall => {
                lines.push(format!(
                    "extension: {}",
                    j["status"].as_str().unwrap_or_default()
                ));
                for h in j["hosts_removed"].as_array().into_iter().flatten() {
                    lines.push(format!("  removed {}", h.as_str().unwrap_or_default()));
                }
                lines.push(j["note"].as_str().unwrap_or_default().to_string());
            }
            Cmd::Status => {
                lines.push(format!(
                    "extension files: {} ({})",
                    j["files"].as_str().unwrap_or_default(),
                    j["dir"].as_str().unwrap_or_default()
                ));
                lines.push(format!(
                    "extension ID: {}",
                    j["extension_id"].as_str().unwrap_or_default()
                ));
                if j["hosts"].as_array().is_none_or(Vec::is_empty) {
                    lines.push("  no supported browser is installed".into());
                }
                lines.extend(host_lines(j));
            }
        }
        lines.join("\n")
    });
    Ok(())
}

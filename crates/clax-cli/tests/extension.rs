//! `clax extension install|uninstall|status` against scratch browser
//! directories (`CLAX_NATIVE_HOST_DIRS`) and a fixture build
//! (`CLAX_EXTENSION_DIST`); real browser profiles are never touched.

use assert_cmd::Command;
use clax_core::extension::{extension_id_in_effect, extension_origin};
use serde_json::Value;
use std::path::{Path, PathBuf};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("icons")).unwrap();
        std::fs::write(dist.join("manifest.json"), clax_core::extension::MANIFEST).unwrap();
        std::fs::write(dist.join("sw.js"), "export {}").unwrap();
        std::fs::write(dist.join("icons/16.png"), [137u8, 80, 78, 71]).unwrap();
        for b in ["chrome", "brave"] {
            std::fs::create_dir_all(dir.path().join(b)).unwrap();
        }
        Env { dir }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn origin(&self) -> String {
        format!(
            "{}/",
            extension_origin(&extension_id_in_effect(&self.p("ax")))
        )
    }
    fn cmd(&self, args: &[&str]) -> Value {
        self.cmd_in(Command::cargo_bin("clax").unwrap(), args)
    }
    /// `clax <args>` under `umask 000`, so modes come from Clax alone.
    fn cmd_umask0(&self, args: &[&str]) -> Value {
        let mut c = Command::new("/bin/sh");
        c.arg("-c")
            .arg("umask 000; exec \"$0\" \"$@\"")
            .arg(assert_cmd::cargo::cargo_bin("clax"));
        self.cmd_in(c, args)
    }
    fn cmd_in(&self, mut c: Command, args: &[&str]) -> Value {
        let out = c
            .env("HOME", self.dir.path())
            .env("CLAX_HOME", self.p("ax"))
            .env("CLAX_EXTENSION_DIST", self.p("dist"))
            .env_remove("XDG_CONFIG_HOME")
            .env(
                "CLAX_NATIVE_HOST_DIRS",
                format!(
                    "chrome={}:brave={}:edge={}",
                    self.p("chrome").display(),
                    self.p("brave").display(),
                    self.p("edge").display()
                ),
            )
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn host_manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("dev.empathic.clax.json")).unwrap()).unwrap()
}

#[test]
fn install_writes_the_extension_the_launcher_and_a_manifest_per_existing_browser() {
    let e = Env::new();
    let out = e.cmd(&["extension", "install"]);
    assert!(e.p("ax/extension/manifest.json").exists());
    assert!(e.p("ax/extension/sw.js").exists());
    assert!(e.p("ax/extension/icons/16.png").exists());
    let launch = std::fs::read_to_string(e.p("ax/extension/host/launch.sh")).unwrap();
    assert!(
        launch.contains(&format!(
            "exec '{}' exec native-host \"$@\"",
            e.p("ax/extension/host/ensure-clax.sh").display()
        )),
        "{launch}"
    );
    assert!(launch.contains(&format!("CLAX_HOME='{}'", e.p("ax").display())));
    assert!(launch.contains("export CLAX_HOME"));
    use std::os::unix::fs::PermissionsExt;
    for f in ["launch.sh", "ensure-clax.sh"] {
        let mode = std::fs::metadata(e.p("ax/extension/host").join(f))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{f} is executable");
    }
    let wrapper = std::fs::read(e.p("ax/extension/host/ensure-clax.sh")).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/claude-code/scripts/ensure-clax.sh");
    assert_eq!(
        wrapper,
        std::fs::read(repo).unwrap(),
        "a copy of the plugins' wrapper"
    );
    for b in ["chrome", "brave"] {
        let m = host_manifest(&e.p(b));
        assert_eq!(m["name"], "dev.empathic.clax");
        assert_eq!(m["type"], "stdio");
        assert_eq!(
            m["path"],
            e.p("ax/extension/host/launch.sh").display().to_string()
        );
        assert_eq!(m["allowed_origins"], serde_json::json!([e.origin()]));
    }
    assert!(
        !e.p("edge").exists(),
        "a browser that is not installed is left alone"
    );
    assert!(
        out["load_unpacked"]
            .as_str()
            .unwrap()
            .contains("chrome://extensions")
    );
    assert_eq!(out["extension_id"], extension_id_in_effect(&e.p("ax")));
    let st = e.cmd(&["extension", "status"]);
    assert_eq!(st["files"], "current");
    assert_eq!(st["extension_id"], extension_id_in_effect(&e.p("ax")));
    assert_eq!(
        st["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["status"] == "installed")
            .count(),
        2,
        "{st}"
    );
}

#[test]
fn status_reports_stale_files_and_manifests() {
    let e = Env::new();
    let st = e.cmd(&["extension", "status"]);
    assert_eq!(st["files"], "missing");
    assert!(
        st["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["status"] == "missing"),
        "{st}"
    );
    e.cmd(&["extension", "install"]);
    std::fs::write(e.p("ax/extension/sw.js"), "changed").unwrap();
    std::fs::write(
        e.p("brave/dev.empathic.clax.json"),
        r#"{"name":"dev.empathic.clax","path":"/elsewhere/launch.sh"}"#,
    )
    .unwrap();
    let st = e.cmd(&["extension", "status"]);
    assert_eq!(st["files"], "stale");
    let by = |b: &str| {
        st["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["browser"] == b)
            .unwrap()["status"]
            .clone()
    };
    assert_eq!(by("chrome"), "installed");
    assert_eq!(by("brave"), "stale");
}

#[test]
fn uninstall_removes_only_what_install_wrote() {
    let e = Env::new();
    e.cmd(&["extension", "install"]);
    std::fs::create_dir_all(e.p("edge")).unwrap();
    std::fs::write(
        e.p("edge/dev.empathic.clax.json"),
        r#"{"name":"dev.empathic.clax","path":"/elsewhere/launch.sh"}"#,
    )
    .unwrap();
    let out = e.cmd(&["extension", "uninstall"]);
    assert_eq!(out["status"], "removed");
    assert!(!e.p("ax/extension").exists());
    assert!(!e.p("chrome/dev.empathic.clax.json").exists());
    assert!(!e.p("brave/dev.empathic.clax.json").exists());
    assert!(
        e.p("edge/dev.empathic.clax.json").exists(),
        "another install's manifest is kept"
    );
    assert_eq!(e.cmd(&["extension", "uninstall"])["status"], "absent");
}

#[test]
fn reinstalling_drops_files_the_new_build_lacks() {
    let e = Env::new();
    e.cmd(&["extension", "install"]);
    std::fs::write(e.p("ax/extension/old.js"), "x").unwrap();
    e.cmd(&["extension", "install"]);
    assert!(!e.p("ax/extension/old.js").exists());
    assert!(e.p("ax/extension/manifest.json").exists());
    let leftovers: Vec<_> = std::fs::read_dir(e.p("ax"))
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".extension"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

fn host<'a>(out: &'a Value, browser: &str) -> &'a Value {
    out["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["browser"] == browser)
        .unwrap_or_else(|| panic!("no {browser} in {out}"))
}

#[test]
fn reinstalling_after_an_id_change_rewrites_the_origin() {
    let e = Env::new();
    e.cmd(&["extension", "install"]);
    let launch = e.p("ax/extension/host/launch.sh").display().to_string();
    std::fs::write(
        e.p("chrome/dev.empathic.clax.json"),
        serde_json::to_vec(&serde_json::json!({
            "name": "dev.empathic.clax", "path": launch, "type": "stdio",
            "allowed_origins": ["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"]
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        host(&e.cmd(&["extension", "status"]), "chrome")["status"],
        "stale"
    );
    let out = e.cmd(&["extension", "install"]);
    assert_eq!(host(&out, "chrome")["status"], "installed");
    assert_eq!(
        host_manifest(&e.p("chrome"))["allowed_origins"],
        serde_json::json!([e.origin()])
    );
    assert_eq!(
        host(&e.cmd(&["extension", "status"]), "chrome")["status"],
        "installed"
    );
}

#[test]
fn another_homes_registration_is_kept_unless_forced() {
    let e = Env::new();
    std::fs::create_dir_all(e.p("other/extension/host")).unwrap();
    let other = e.p("other/extension/host/launch.sh");
    std::fs::write(&other, "#!/bin/sh\n").unwrap();
    let theirs = format!(
        r#"{{"name":"dev.empathic.clax","path":"{}","type":"stdio","allowed_origins":[]}}"#,
        other.display()
    );
    std::fs::write(e.p("chrome/dev.empathic.clax.json"), &theirs).unwrap();
    let out = e.cmd(&["extension", "install"]);
    let h = host(&out, "chrome");
    assert_eq!(h["status"], "conflict", "{out}");
    assert!(h["detail"].as_str().unwrap().contains("--force"), "{out}");
    assert_eq!(
        std::fs::read_to_string(e.p("chrome/dev.empathic.clax.json")).unwrap(),
        theirs
    );
    assert_eq!(host(&out, "brave")["status"], "installed");
    let out = e.cmd(&["extension", "install", "--force"]);
    let h = host(&out, "chrome");
    assert_eq!(h["status"], "installed", "{out}");
    assert_eq!(h["replaced"], other.display().to_string());
    assert_eq!(
        host_manifest(&e.p("chrome"))["path"],
        e.p("ax/extension/host/launch.sh").display().to_string()
    );
}

#[test]
fn a_registration_whose_launcher_is_gone_is_replaced() {
    let e = Env::new();
    std::fs::write(
        e.p("chrome/dev.empathic.clax.json"),
        r#"{"name":"dev.empathic.clax","path":"/gone/extension/host/launch.sh"}"#,
    )
    .unwrap();
    let out = e.cmd(&["extension", "install"]);
    let h = host(&out, "chrome");
    assert_eq!(h["status"], "installed", "{out}");
    assert_eq!(h["replaced"], "/gone/extension/host/launch.sh");
}

#[test]
fn a_symlinked_manifest_is_replaced_not_written_through() {
    let e = Env::new();
    std::fs::write(e.p("target.json"), "{}").unwrap();
    std::os::unix::fs::symlink(e.p("target.json"), e.p("chrome/dev.empathic.clax.json")).unwrap();
    e.cmd(&["extension", "install"]);
    assert_eq!(std::fs::read_to_string(e.p("target.json")).unwrap(), "{}");
    let meta = std::fs::symlink_metadata(e.p("chrome/dev.empathic.clax.json")).unwrap();
    assert!(meta.file_type().is_file(), "the symlink itself is replaced");
    assert_eq!(host_manifest(&e.p("chrome"))["name"], "dev.empathic.clax");
    let left: Vec<_> = std::fs::read_dir(e.p("chrome"))
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        left,
        vec!["dev.empathic.clax.json".to_string()],
        "no temporary file is left"
    );
}

#[test]
fn install_with_no_browser_says_so() {
    let e = Env::new();
    std::fs::remove_dir(e.p("chrome")).unwrap();
    std::fs::remove_dir(e.p("brave")).unwrap();
    let out = e.cmd(&["extension", "install"]);
    assert_eq!(out["status"], "no_browser", "{out}");
    assert!(
        out["detail"]
            .as_str()
            .unwrap()
            .contains("no supported browser"),
        "{out}"
    );
    assert!(
        e.p("ax/extension/manifest.json").exists(),
        "the files are still written"
    );
}

#[test]
fn a_first_install_makes_a_private_home_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    e.cmd_umask0(&["extension", "install"]);
    let mode = |p: &str| std::fs::metadata(e.p(p)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode("ax"), 0o700);
    for p in [
        "ax/extension",
        "ax/extension/icons",
        "ax/extension/manifest.json",
        "ax/extension/installed.json",
        "ax/extension/host/launch.sh",
        "chrome/dev.empathic.clax.json",
    ] {
        assert_eq!(mode(p) & 0o022, 0, "{p} is {:o}", mode(p));
    }
}

#[test]
fn status_checks_the_launcher_and_wrapper_copy() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    assert_eq!(e.cmd(&["extension", "status"])["launcher"], "missing");
    e.cmd(&["extension", "install"]);
    assert_eq!(e.cmd(&["extension", "status"])["launcher"], "current");
    let wrapper = e.p("ax/extension/host/ensure-clax.sh");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(e.cmd(&["extension", "status"])["launcher"], "stale");
    e.cmd(&["extension", "install"]);
    std::fs::write(
        e.p("ax/extension/host/launch.sh"),
        "#!/bin/sh\nexec other\n",
    )
    .unwrap();
    assert_eq!(e.cmd(&["extension", "status"])["launcher"], "stale");
    std::fs::remove_file(&wrapper).unwrap();
    assert_eq!(e.cmd(&["extension", "status"])["launcher"], "missing");
}

/// Every script that runs `clax init` or `clax uninit` keeps it away from
/// the real browser directories.
#[test]
fn scripts_that_run_init_isolate_the_browser_directories() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts");
    let mut checked = 0;
    for e in std::fs::read_dir(&scripts).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "sh") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        let runs_init = text.lines().any(|l| {
            let l = l.trim_start();
            if l.starts_with('#') || l.starts_with("echo") || l.starts_with("record") {
                return false;
            }
            let words: Vec<&str> = l.split_whitespace().collect();
            words.windows(2).any(|w| {
                (w[0].contains("CLAX_BIN") || w[0].trim_matches('"').ends_with("/clax"))
                    && matches!(w[1], "init" | "uninit")
            }) || (words.first() == Some(&"check_run")
                && matches!(words.last(), Some(&"init") | Some(&"uninit")))
        });
        if !runs_init {
            continue;
        }
        checked += 1;
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            text.contains("CLAX_NATIVE_HOST_DIRS="),
            "{name} runs clax init without setting CLAX_NATIVE_HOST_DIRS to a scratch directory"
        );
        assert!(
            text.lines()
                .any(|l| l.trim_start().starts_with("unset ") && l.contains("XDG_CONFIG_HOME")),
            "{name} runs clax init without unsetting XDG_CONFIG_HOME"
        );
    }
    assert!(
        checked >= 2,
        "the scan found the scripts that run clax init"
    );
}

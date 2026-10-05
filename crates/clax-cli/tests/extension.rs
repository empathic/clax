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
        let out = Command::cargo_bin("clax")
            .unwrap()
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

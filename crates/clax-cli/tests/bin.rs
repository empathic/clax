//! `clax bin`, and `clax init`/`uninit` pointing the `bin` setting at
//! themselves.
use assert_cmd::Command;
use predicates::str::contains;
use std::path::PathBuf;

/// A scratch HOME and CLAX_HOME, with no harness CLI on PATH.
struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("clax").unwrap();
        c.env("HOME", self.dir.path())
            .env("CLAX_HOME", self.p("ax"))
            .env("CLAUDE_CONFIG_DIR", self.p("claude"))
            .env("CODEX_HOME", self.p("codex"))
            .env("PI_CODING_AGENT_DIR", self.p("pi"))
            .env("GROK_HOME", self.p("grok"))
            .env("PATH", "/usr/bin:/bin")
            .env_remove("XDG_CONFIG_HOME")
            .env(
                "CLAX_NATIVE_HOST_DIRS",
                format!("chrome={}", self.p("browsers/chrome").display()),
            )
            .env_remove("CLAX_BIN");
        c
    }
    fn json(&self, args: &[&str]) -> (bool, serde_json::Value) {
        let out = self.cmd().args(args).arg("--json").output().unwrap();
        let v = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.success(), v)
    }
    fn config(&self) -> String {
        std::fs::read_to_string(self.p("ax/config.toml")).unwrap()
    }
}

const ME: &str = env!("CARGO_BIN_EXE_clax");

#[test]
fn bin_shows_sets_and_clears_the_binary_the_plugins_run() {
    let e = Env::new();
    let (ok, v) = e.json(&["bin"]);
    assert!(ok, "{v}");
    assert_eq!(v["setting"], serde_json::Value::Null, "{v}");
    assert_ne!(v["source"], "config", "{v}");
    e.cmd()
        .args(["bin", "set", "--this"])
        .assert()
        .success()
        .stdout(contains(format!("the plugins now run {ME}")));
    let (_, v) = e.json(&["bin", "show"]);
    assert_eq!(
        (v["source"].as_str(), v["path"].as_str(), v["ok"].as_bool()),
        (Some("config"), Some(ME), Some(true)),
        "{v}"
    );
    assert_eq!(e.config(), format!("bin = \"{ME}\"\n"));
    let out = e.cmd().arg("bin").env("CLAX_BIN", ME).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!(
            "the plugins run: {ME} (clax {}), from CLAX_BIN",
            env!("CARGO_PKG_VERSION")
        )),
        "{text}"
    );
    e.cmd()
        .args(["bin", "set", "relative/clax"])
        .assert()
        .failure()
        .stderr(contains("not an absolute path"));
    e.cmd()
        .args(["bin", "set", "/bin/sh"])
        .assert()
        .failure()
        .stderr(contains("not a usable clax binary"));
    e.cmd().args(["bin", "set"]).assert().failure();
    e.cmd()
        .args(["bin", "set", ME, "--this"])
        .assert()
        .failure();
    assert_eq!(e.config(), format!("bin = \"{ME}\"\n"));
    e.cmd()
        .args(["bin", "clear"])
        .assert()
        .success()
        .stdout(contains("removed the bin setting"));
    assert_eq!(e.config(), "");
    e.cmd()
        .args(["bin", "clear"])
        .assert()
        .success()
        .stdout(contains("no bin setting to remove"));
}

#[test]
fn a_bin_setting_does_not_disturb_the_rest_of_the_config() {
    let e = Env::new();
    std::fs::create_dir_all(e.p("ax")).unwrap();
    std::fs::write(e.p("ax/config.toml"), "[serve]\nport = 7489\n").unwrap();
    e.cmd().args(["bin", "set", ME]).assert().success();
    assert_eq!(
        e.config(),
        format!("bin = \"{ME}\"\n[serve]\nport = 7489\n")
    );
    // The CLI still reads the port, and mcp's preflight accepts the file.
    e.cmd()
        .args(["mcp", "--agent", "codex", "--preflight"])
        .assert()
        .success();
}

#[test]
fn init_points_the_bin_setting_at_this_binary_and_uninit_removes_it() {
    let e = Env::new();
    std::fs::create_dir_all(e.p("ax")).unwrap();
    std::fs::write(e.p("ax/config.toml"), "[serve]\nport = 7481\n").unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    assert_eq!(v["bin"]["status"], "set", "{v}");
    assert_eq!(
        e.config(),
        format!("bin = \"{ME}\"\n[serve]\nport = 7481\n")
    );
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(v["bin"]["status"], "cleared", "{v}");
    assert_eq!(e.config(), "[serve]\nport = 7481\n");
    // A setting that names another binary is left alone.
    std::fs::write(e.p("ax/config.toml"), "bin = \"/opt/other/clax\"\n").unwrap();
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(v["bin"]["status"], "kept", "{v}");
    assert_eq!(e.config(), "bin = \"/opt/other/clax\"\n");
}

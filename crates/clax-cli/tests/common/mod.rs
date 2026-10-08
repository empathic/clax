//! Helpers shared by the test binaries.

use assert_cmd::Command;

/// A scratch HOME and CLAX_HOME; dropping it stops the daemon started there.
pub struct Env {
    pub dir: tempfile::TempDir,
}
impl Env {
    pub fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("clax").unwrap();
        c.env("CLAX_HOME", self.dir.path().join("ax"))
            .env("CLAX_CODEX_BIN", "")
            .env("HOME", self.dir.path());
        // Harness directories default under the temp HOME, never the real ones.
        for var in [
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
            "CLAUDE_PLUGIN_ROOT",
            "PLUGIN_ROOT",
        ] {
            c.env_remove(var);
        }
        c
    }
    pub fn stop(&self) {
        self.cmd().arg("stop").assert().success();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().arg("stop").output();
        let info = std::fs::read_to_string(self.dir.path().join("ax/daemon.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        if let Some(pid) = info.and_then(|v| v["pid"].as_i64()) {
            // Signal 0 probes, SIGTERM ends the leaked test daemon.
            use nix::sys::signal::{Signal, kill};
            let pid = nix::unistd::Pid::from_raw(pid as i32);
            if kill(pid, None).is_ok() {
                let _ = kill(pid, Signal::SIGTERM);
            }
        }
    }
}

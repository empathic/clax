//! Helpers shared by the test binaries.

use assert_cmd::Command;

/// The `clax` the tests run (`clax_fake_exe::clax_bin`): `CLAX_TEST_BIN`,
/// else Cargo's build of it kept by content.
pub fn clax() -> std::path::PathBuf {
    clax_fake_exe::clax_bin(option_env!("CARGO_BIN_EXE_clax"))
}

/// `text` with every mention of the resolved path of [`clax`] written as
/// [`clax`]'s own path. The binary names itself by `std::env::current_exe`,
/// which on Linux resolves symbolic links (Bazel's runfiles are links into
/// its output tree) and on macOS returns the path it was run by; either
/// names the same file. The integration tests use it; `descriptors`, which
/// compiles this module too, does not.
#[allow(dead_code)]
pub fn as_clax(text: &str) -> String {
    let me = clax();
    let resolved = std::fs::canonicalize(&me).unwrap();
    if resolved == me {
        return text.to_string();
    }
    text.replace(&*resolved.to_string_lossy(), &me.to_string_lossy())
}

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
        let mut c = Command::new(crate::common::clax());
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
        // The CLI by hand, wherever the tests run (an agent's shell sets
        // these): never an agent channel unless a test says so.
        for var in [
            "CLAX_CHANNEL",
            "CLAUDECODE",
            "CODEX_THREAD_ID",
            "CODEX_SANDBOX",
            "CODEX_CI",
            "PI_CODING_AGENT",
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

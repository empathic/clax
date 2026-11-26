//! The Claude Code plugin's copy stands down when Grok Build or Antigravity
//! runs it: `clax mcp --agent claude` serves the stand-down server and `clax
//! hook --agent claude` does nothing; everywhere else they act.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Variables a test inherits from the harness it runs in; cleared first.
const HARNESS_VARS: &[&str] = &[
    "GROK_SESSION_ID",
    "GROK_HOOK_EVENT",
    "GROK_PLUGIN_ROOT",
    "GROK_HOME",
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "CLAX_SESSION_ID",
    "CLAX_BIN",
    "ANTIGRAVITY_CONVERSATION_ID",
    "PLUGIN_ROOT",
    "PLUGIN_DATA",
];

fn clax(home: &std::path::Path, env: &[(&str, String)]) -> Command {
    in_test_env(Command::new(crate::common::clax()), home, env)
}

fn in_test_env(mut c: Command, home: &std::path::Path, env: &[(&str, String)]) -> Command {
    for k in HARNESS_VARS {
        c.env_remove(k);
    }
    c.env("CLAX_HOME", home)
        .env("HOME", home.parent().unwrap())
        .env("CLAX_CODEX_BIN", "")
        .env("CLAX_NO_OPEN", "1")
        .env("RUST_LOG", "error");
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

/// An MCP server under test, answering one request at a time.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// A stand-in `agy`: runs `clax --port 0 mcp --agent claude` as its child,
/// with stdin and stdout passed through, so the server's parent is an `agy`.
const FAKE_AGY: &str = "#!/bin/bash\n\"$CLAX_TEST_BIN\" --port 0 mcp --agent claude\n";

/// [`FAKE_AGY`] that sets `CLAUDE_PID` to its own PID, as Claude Code does:
/// a Claude Code whose command line has `agy` for a word that names the
/// program (`claude agy`, a one-word prompt).
const FAKE_CLAUDE_NAMED_AGY: &str =
    "#!/bin/bash\nexport CLAUDE_PID=$$\n\"$CLAX_TEST_BIN\" --port 0 mcp --agent claude\n";

impl Mcp {
    fn start(home: &std::path::Path, agent: &str, env: &[(&str, String)]) -> Mcp {
        let mut cmd = clax(home, env);
        cmd.args(["--port", "0", "mcp", "--agent", agent]);
        Mcp::spawn(cmd)
    }
    /// The Claude Code copy's server, started by `fake` (a stand-in such as
    /// [`FAKE_AGY`]) installed as `bin/agy` and run with `env`.
    fn under_agy(
        home: &std::path::Path,
        bin: &std::path::Path,
        fake: &str,
        env: &[(&str, String)],
    ) -> Mcp {
        let agy = clax_fake_exe::install(&bin.join("agy"), fake);
        let mut cmd = in_test_env(Command::new(agy), home, env);
        cmd.env("CLAX_TEST_BIN", crate::common::clax());
        Mcp::spawn(cmd)
    }
    fn spawn(mut cmd: Command) -> Mcp {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut m = Mcp {
            child,
            stdin,
            stdout,
        };
        let init = m.request(
            0,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-grok", "version": "1"}}),
        );
        assert!(init["result"].is_object(), "{init}");
        m.notify("notifications/initialized");
        m
    }
    fn notify(&mut self, method: &str) {
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": method})
        )
        .unwrap();
    }
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server closed stdout"
            );
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }
    fn tool_names(&mut self) -> Vec<String> {
        let r = self.request(1, "tools/list", json!({}));
        r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hooks_log(home: &std::path::Path) -> String {
    std::fs::read_to_string(home.join("logs/hooks.log")).unwrap_or_default()
}

#[test]
fn a_claude_copy_that_grok_starts_serves_only_status() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::start(&home, "claude", &[("GROK_SESSION_ID", "019a-g".into())]);
    assert_eq!(m.tool_names(), ["status"]);
    let call = m.request(2, "tools/call", json!({"name": "status", "arguments": {}}));
    assert_ne!(call["result"]["isError"], json!(true), "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("clax_grok__publish"), "{text}");
    assert_eq!(m.request(3, "ping", json!({}))["result"], json!({}));
    assert!(
        !home.join("daemon.json").exists(),
        "a standing-down server starts no daemon"
    );
    assert!(hooks_log(&home).contains(" standdown mode=mcp agent=claude host=grok"));
}

#[test]
fn an_inherited_claude_pid_that_is_not_the_parent_still_stands_down() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = Mcp::start(
        &dir.path().join("ax"),
        "claude",
        &[
            ("GROK_SESSION_ID", "019a-g".into()),
            ("CLAUDE_PID", "1".into()),
        ],
    );
    assert_eq!(m.tool_names(), ["status"]);
}

#[test]
fn claude_code_as_the_parent_acts_even_with_a_grok_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // The test process is the server's parent, standing in for Claude Code.
    let mut m = Mcp::start(
        &home,
        "claude",
        &[
            ("GROK_SESSION_ID", "019a-g".into()),
            ("CLAUDE_PID", std::process::id().to_string()),
        ],
    );
    assert_eq!(m.tool_names().len(), 43);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").status();
}

#[test]
fn the_grok_agent_acts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::start(&home, "grok", &[("GROK_SESSION_ID", "019a-g".into())]);
    assert_eq!(m.tool_names().len(), 43);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").status();
}

/// Runs `clax hook --agent claude stop` with `stdin`; (stdout, stderr).
fn claude_stop(home: &std::path::Path, env: &[(&str, String)], stdin: &str) -> (String, String) {
    let mut child = clax(home, env)
        .args(["hook", "--agent", "claude", "stop"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn a_claude_hook_that_grok_runs_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // No daemon: a hook that acted would report "no clax daemon is running".
    let (out, err) = claude_stop(
        &home,
        &[("GROK_HOOK_EVENT", "Stop".into())],
        r#"{"session_id":"s1"}"#,
    );
    assert_eq!((out.as_str(), err.as_str()), ("", ""));
    let (out, err) = claude_stop(
        &home,
        &[],
        r#"{"sessionId":"s1","session_id":"s1","hookEventName":"Stop","reason":"end_turn"}"#,
    );
    assert_eq!((out.as_str(), err.as_str()), ("", ""));
    let log = hooks_log(&home);
    assert_eq!(
        log.matches(" standdown mode=hook agent=claude host=grok")
            .count(),
        2,
        "{log}"
    );
    assert!(!log.contains(" hook agent=claude "), "{log}");
}

#[test]
fn a_claude_hook_outside_grok_acts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let (_, err) = claude_stop(
        &home,
        &[("GROK_SESSION_ID", "g".into())],
        r#"{"session_id":"s1"}"#,
    );
    assert!(err.contains("no clax daemon is running"), "{err}");
    assert!(hooks_log(&home).contains(" hook agent=claude event=stop "));
}

#[test]
fn the_claude_code_copy_antigravity_starts_serves_only_status() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::under_agy(&home, dir.path(), FAKE_AGY, &[]);
    assert_eq!(m.tool_names(), ["status"]);
    let call = m.request(2, "tools/call", json!({"name": "status", "arguments": {}}));
    assert_ne!(call["result"]["isError"], json!(true), "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Clax runs from the clax-antigravity plugin in Antigravity")
            && text.contains("clax init --agent antigravity")
            && text.ends_with("To remove this copy: agy plugin uninstall clax"),
        "{text}"
    );
    assert!(
        !home.join("daemon.json").exists(),
        "a standing-down server starts no daemon"
    );
    let log = hooks_log(&home);
    assert!(
        log.contains(" standdown mode=mcp agent=claude host=antigravity"),
        "{log}"
    );
}

/// The `Stop` input Antigravity sends a hook.
const AGY_STOP: &str = r#"{"conversationId":"c0ffee00","hookEventName":"Stop","terminationReason":"NO_TOOL_CALL","executionNum":0,"fullyIdle":true}"#;

#[test]
fn a_claude_code_hook_antigravity_runs_does_nothing_and_logs_its_host() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let (out, err) = claude_stop(
        &home,
        &[
            ("ANTIGRAVITY_CONVERSATION_ID", "c0ffee00".into()),
            ("PLUGIN_ROOT", dir.path().display().to_string()),
        ],
        AGY_STOP,
    );
    assert_eq!((out.as_str(), err.as_str()), ("", ""));
    let log = hooks_log(&home);
    assert!(
        log.contains(" standdown mode=hook agent=claude host=antigravity"),
        "{log}"
    );
    assert!(!log.contains("host=grok"), "{log}");
    assert!(!log.contains(" hook agent=claude "), "{log}");
}

#[test]
fn a_claude_code_hook_grok_runs_still_logs_grok() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // Grok started from an Antigravity shell passes on its conversation ID.
    let (out, err) = claude_stop(
        &home,
        &[
            ("GROK_HOOK_EVENT", "Stop".into()),
            ("ANTIGRAVITY_CONVERSATION_ID", "c0ffee00".into()),
        ],
        r#"{"sessionId":"s1","session_id":"s1","hookEventName":"Stop","reason":"end_turn"}"#,
    );
    assert_eq!((out.as_str(), err.as_str()), ("", ""));
    let log = hooks_log(&home);
    assert!(
        log.contains(" standdown mode=hook agent=claude host=grok"),
        "{log}"
    );
    assert!(!log.contains("host=antigravity"), "{log}");
}

#[test]
fn claude_code_itself_still_acts_with_antigravitys_variables_inherited() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // Claude Code started from an Antigravity shell: Claude Code's own
    // envelope, Antigravity's conversation ID inherited.
    let inherited = [("ANTIGRAVITY_CONVERSATION_ID", "c0ffee00".to_string())];
    let (_, err) = claude_stop(&home, &inherited, r#"{"session_id":"s1"}"#);
    assert!(err.contains("no clax daemon is running"), "{err}");
    let log = hooks_log(&home);
    assert!(log.contains(" hook agent=claude event=stop "), "{log}");
    assert!(!log.contains("standdown"), "{log}");
    // The test process is the server's parent, standing in for Claude Code.
    let mut m = Mcp::start(&home, "claude", &inherited);
    assert_eq!(m.tool_names().len(), 43);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").output();
}

#[test]
fn claude_code_as_the_parent_acts_whatever_its_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::under_agy(&home, dir.path(), FAKE_CLAUDE_NAMED_AGY, &[]);
    assert_eq!(m.tool_names().len(), 43);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").output();
    assert!(!hooks_log(&home).contains("standdown"));
}

#[test]
fn an_agy_started_from_a_claude_code_shell_stands_the_copy_down() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // The inherited CLAUDE_PID is the test process, not the server's parent.
    let mut m = Mcp::under_agy(
        &home,
        dir.path(),
        FAKE_AGY,
        &[("CLAUDE_PID", std::process::id().to_string())],
    );
    assert_eq!(m.tool_names(), ["status"]);
    assert!(hooks_log(&home).contains(" standdown mode=mcp agent=claude host=antigravity"));
}

#[test]
fn an_agy_parent_names_antigravity_even_with_a_grok_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::under_agy(
        &home,
        dir.path(),
        FAKE_AGY,
        &[("GROK_SESSION_ID", "019a-g".into())],
    );
    let call = m.request(2, "tools/call", json!({"name": "status", "arguments": {}}));
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("clax init --agent antigravity"), "{text}");
    let log = hooks_log(&home);
    assert!(
        log.contains(" standdown mode=mcp agent=claude host=antigravity"),
        "{log}"
    );
    assert!(!log.contains("host=grok"), "{log}");
}

//! The Claude Code plugin's copy stands down when Grok Build runs it:
//! `clax mcp --agent claude` serves the stand-down server and `clax hook
//! --agent claude` does nothing; everywhere else they act.

use assert_cmd::cargo::cargo_bin;
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
];

fn clax(home: &std::path::Path, env: &[(&str, String)]) -> Command {
    let mut c = Command::new(cargo_bin("clax"));
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

impl Mcp {
    fn start(home: &std::path::Path, agent: &str, env: &[(&str, String)]) -> Mcp {
        let mut child = clax(home, env)
            .args(["--port", "0", "mcp", "--agent", agent])
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
    assert_eq!(m.tool_names().len(), 23);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").status();
}

#[test]
fn the_grok_agent_acts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::start(&home, "grok", &[("GROK_SESSION_ID", "019a-g".into())]);
    assert_eq!(m.tool_names().len(), 23);
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

//! `clax hook --agent claude call-id`, Claude Code's PostToolUse hook on
//! Clax's MCP tools (spec 2026-10-06-toolpath-audit-design §6.7): it
//! reports the harness's `tool_use_id` for the call, which the daemon
//! records as `tool.call_id` on the `hook` channel, and never fails the
//! harness.

use crate::common::Env;
use serde_json::{Value, json};

/// The running daemon's base URL and token.
fn daemon(e: &Env) -> (String, String) {
    let info: Value =
        serde_json::from_slice(&std::fs::read(e.dir.path().join("ax/daemon.json")).unwrap())
            .unwrap();
    (
        format!("http://127.0.0.1:{}", info["port"]),
        info["token"].as_str().unwrap().to_string(),
    )
}

fn api(e: &Env, path: &str, body: Value) -> Value {
    let (base, token) = daemon(e);
    let res = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .header("x-clax-via", "mcp")
        .json(&body)
        .send()
        .unwrap();
    assert!(res.status().is_success(), "POST {path}: {}", res.status());
    res.json().unwrap()
}

fn hooks_log(e: &Env) -> String {
    std::fs::read_to_string(e.dir.path().join("ax/logs/hooks.log")).unwrap_or_default()
}

/// PostToolUse stdin for a Clax tool call, as Claude Code writes it.
fn post_tool_use(hsid: &str, tool_use_id: &str, tool_input: Value) -> String {
    json!({
        "session_id": hsid, "transcript_path": format!("/t/{hsid}.jsonl"), "cwd": "/w",
        "permission_mode": "default", "hook_event_name": "PostToolUse",
        "tool_name": "mcp__plugin_clax_clax__publish", "tool_input": tool_input,
        "tool_response": [{"type": "text", "text": "ok"}], "tool_use_id": tool_use_id,
    })
    .to_string()
}

#[test]
fn the_hook_records_the_tool_use_id_of_a_clax_call() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let sid = api(
        &e,
        "/api/sessions",
        json!({"harness": "claude", "harness_session_id": "cc-hook-ids", "cwd": "/w"}),
    )["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let call_id = "01JBC0000000000000000000H1";
    // The call has just ended: a report names only a call that ended
    // within 5 s of its arrival.
    let now = chrono::Utc::now();
    let ms = chrono::SecondsFormat::Millis;
    let ended_at = now.to_rfc3339_opts(ms, true);
    let started_at = (now - chrono::Duration::milliseconds(500)).to_rfc3339_opts(ms, true);
    api(
        &e,
        &format!("/api/sessions/{sid}/tool-calls"),
        json!({"call_id": call_id, "tool": "publish",
               "args_sha256": "sha256:c6e6f48ad9e94345a81d22b0fa628e053e81e5785a38f0f61965c9196a4bfe93",
               "started_at": started_at, "ended_at": ended_at,
               "outcome": "ok"}),
    );
    e.cmd()
        .args(["hook", "--agent", "claude", "call-id"])
        .write_stdin(post_tool_use(
            "cc-hook-ids",
            "toolu_01HOOK",
            json!({"id": "k3m9q2w8x1ab", "if_version": 3}),
        ))
        .assert()
        .success()
        .stdout("");
    let c = rusqlite::Connection::open(e.dir.path().join("ax/clax.db")).unwrap();
    let (call_col, body): (Option<String>, String) = c
        .query_row(
            "SELECT call_id, body FROM audit_events WHERE kind = 'tool.call_id'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(call_col.as_deref(), Some(call_id));
    assert_eq!(body["call_id"], call_id);
    assert_eq!(body["harness_call_id"], "toolu_01HOOK");
    assert_eq!(body["harness_tool"], "mcp__plugin_clax_clax__publish");
    assert_eq!(body["via"], "hook");
    assert!(
        hooks_log(&e).contains(" hook agent=claude event=call-id "),
        "{}",
        hooks_log(&e)
    );
    e.stop();
}

#[test]
fn hook_failure_exits_zero_and_logs() {
    let e = Env::new();
    // No daemon.
    e.cmd()
        .args(["hook", "--agent", "claude", "call-id"])
        .write_stdin(post_tool_use("cc-down", "toolu_1", json!({})))
        .assert()
        .success()
        .stdout("");
    let log = hooks_log(&e);
    let line = log.lines().last().unwrap_or_default();
    assert!(
        line.contains(" hook agent=claude event=call-id ")
            && line.ends_with(" exit=0 stderr=\"no clax daemon is running\""),
        "{log}"
    );
    // A daemon, and input without a tool_use_id.
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    e.cmd()
        .args(["hook", "--agent", "claude", "call-id"])
        .write_stdin(r#"{"session_id":"cc-down","tool_name":"mcp__plugin_clax_clax__list"}"#)
        .assert()
        .success()
        .stdout("");
    let log = hooks_log(&e);
    let line = log.lines().last().unwrap_or_default();
    assert!(
        line.contains(" event=call-id ") && line.contains("tool_use_id"),
        "{log}"
    );
    e.stop();
}

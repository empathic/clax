//! The harness session IDs Clax records (spec
//! 2026-10-06-toolpath-audit-design §9.4), checked against what each
//! harness writes, in `tests/fixtures/harness-ids/`:
//!
//! - `codex-session-start.json` and `codex-rollout-session-meta.jsonl`: the
//!   stdin of a SessionStart hook and the first line of the rollout file of
//!   one Codex 0.162.0 session (`codex exec`, 2026-10-09). Only the paths
//!   were changed and `base_instructions` trimmed; the IDs are as Codex
//!   wrote them.
//! - `claude-session-start.json` and `claude-transcript-first-line.jsonl`:
//!   a Claude Code SessionStart hook's stdin and the first line of the
//!   transcript it names, in the shapes Claude Code writes them.
//! - `grok-session-start.json`: a Grok Build SessionStart hook's stdin,
//!   with camelCase keys beside the snake_case ones.
//! - `pi-session-header.jsonl`: the header Pi 0.73.1's `SessionManager`
//!   writes first in a session file; its `getSessionId()` is the `id`.
//!
//! Each join goes through the hook's own [`events::join`], and the refs are
//! read from an export, so the test covers what a reader is handed.

use crate::common::TestServer;
use clax_hooks::events::{self, Daemon};
use clax_hooks::input::HookInput;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::time::Duration;

fn fixture(name: &str) -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/harness-ids/");
    std::fs::read_to_string(format!("{dir}{name}")).unwrap()
}

/// The first line of a JSONL fixture.
fn first_line(name: &str) -> Value {
    serde_json::from_str(fixture(name).lines().next().unwrap()).unwrap()
}

/// A daemon that keeps the body of the join a hook sends.
#[derive(Default)]
struct Keep(RefCell<Option<Value>>);

impl Daemon for Keep {
    fn browser_url(&self, path: &str) -> String {
        path.to_string()
    }
    fn get(&self, _: &str) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
        self.get(path)
    }
    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        assert_eq!(path, "/api/sessions/join");
        *self.0.borrow_mut() = Some(body.clone());
        Ok(json!({"session": {"id": "kept"}}))
    }
    fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
}

/// The join `clax hook --agent <harness> session-start` sends for `stdin`.
fn join_body(harness: &str, stdin: &str) -> Value {
    let keep = Keep::default();
    events::join(harness, 4242, &[], &HookInput::parse(stdin), None, &keep).unwrap();
    keep.0.into_inner().unwrap()
}

/// Sends `body` to `path` as the agent side `via` does; the session it made.
async fn session(ts: &TestServer, path: &str, via: &str, body: &Value) -> String {
    let res = ts
        .authed(ts.client.post(format!("{}{path}", ts.base)))
        .header("x-clax-via", via)
        .json(body)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let v: Value = res.json().await.unwrap();
    v["session"]["id"].as_str().unwrap().to_string()
}

/// The `(rel, href)` refs of the steps an export of session `sid` holds.
async fn refs(ts: &TestServer, sid: &str) -> Vec<(String, String)> {
    let doc: Value = ts
        .authed(
            ts.client
                .get(format!("{}/api/toolpath/export?by_session={sid}", ts.base)),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut out = Vec::new();
    for path in doc["paths"].as_array().unwrap() {
        for step in path["steps"].as_array().into_iter().flatten() {
            for r in step["meta"]["refs"].as_array().into_iter().flatten() {
                let pair = (
                    r["rel"].as_str().unwrap().to_string(),
                    r["href"].as_str().unwrap().to_string(),
                );
                if !out.contains(&pair) {
                    out.push(pair);
                }
            }
        }
    }
    assert!(!out.is_empty(), "no refs in {doc}");
    out
}

fn has(refs: &[(String, String)], rel: &str, href: &str) -> bool {
    refs.iter().any(|(r, h)| r == rel && h == href)
}

/// The distinct rels of `refs`, sorted.
fn rels(refs: &[(String, String)]) -> Vec<String> {
    let mut v: Vec<String> = refs.iter().map(|(r, _)| r.clone()).collect();
    v.sort();
    v.dedup();
    v
}

#[tokio::test]
async fn codex_hook_session_id_is_rollout_session_meta_id() {
    let stdin = fixture("codex-session-start.json");
    let meta = first_line("codex-rollout-session-meta.jsonl");
    assert_eq!(meta["type"], "session_meta");
    let id = meta["payload"]["id"].as_str().unwrap();
    let input = HookInput::parse(&stdin);
    // The claim of spec §9.4 that Toolpath's Codex reader keys sessions on.
    assert_eq!(input.session_id.as_deref(), Some(id));
    let transcript = input.transcript_path.clone().unwrap();
    assert!(
        transcript.ends_with(&format!("-{id}.jsonl")),
        "{transcript}"
    );

    let ts = TestServer::spawn().await;
    let body = join_body("codex", &stdin);
    assert_eq!(body["harness_session_id"], id);
    let sid = session(&ts, "/api/sessions/join", "hook", &body).await;
    let refs = refs(&ts, &sid).await;
    assert!(
        has(&refs, "agent-session", &format!("agent://codex/{id}")),
        "{refs:?}"
    );
    assert!(
        has(&refs, "transcript", &format!("file://{transcript}")),
        "{refs:?}"
    );
}

#[tokio::test]
async fn claude_hook_session_id_matches_transcript_session_id() {
    let stdin = fixture("claude-session-start.json");
    let line = first_line("claude-transcript-first-line.jsonl");
    let id = line["sessionId"].as_str().unwrap();
    let input = HookInput::parse(&stdin);
    assert_eq!(input.session_id.as_deref(), Some(id));
    let transcript = input.transcript_path.clone().unwrap();
    assert!(
        transcript.ends_with(&format!("/{id}.jsonl")),
        "{transcript}"
    );

    let ts = TestServer::spawn().await;
    let sid = session(
        &ts,
        "/api/sessions/join",
        "hook",
        &join_body("claude", &stdin),
    )
    .await;
    let refs = refs(&ts, &sid).await;
    // Rendering maps `claude` to Toolpath's provider ID.
    assert!(
        has(&refs, "agent-session", &format!("agent://claude-code/{id}")),
        "{refs:?}"
    );
    assert!(
        has(&refs, "transcript", &format!("file://{transcript}")),
        "{refs:?}"
    );
}

#[tokio::test]
async fn grok_hook_records_same_refs_as_others() {
    let stdin = fixture("grok-session-start.json");
    let raw: Value = serde_json::from_str(&stdin).unwrap();
    let id = raw["sessionId"].as_str().unwrap();
    let transcript = raw["transcriptPath"].as_str().unwrap();

    let ts = TestServer::spawn().await;
    let body = join_body("grok", &stdin);
    assert_eq!(body["harness_session_id"], id);
    assert_eq!(body["transcript_path"], transcript);
    let grok = refs(
        &ts,
        &session(&ts, "/api/sessions/join", "hook", &body).await,
    )
    .await;
    assert!(
        has(&grok, "agent-session", &format!("agent://grok/{id}")),
        "{grok:?}"
    );
    assert!(
        has(&grok, "transcript", &format!("file://{transcript}")),
        "{grok:?}"
    );
    // The same kinds of refs as a Claude Code or Codex join.
    for (harness, file) in [
        ("claude", "claude-session-start.json"),
        ("codex", "codex-session-start.json"),
    ] {
        let sid = session(
            &ts,
            "/api/sessions/join",
            "hook",
            &join_body(harness, &fixture(file)),
        )
        .await;
        assert_eq!(rels(&refs(&ts, &sid).await), rels(&grok), "{harness}");
    }
}

#[tokio::test]
async fn pi_session_header_id_is_the_registered_id() {
    let header = first_line("pi-session-header.jsonl");
    assert_eq!(header["type"], "session");
    let id = header["id"].as_str().unwrap();
    let ts = TestServer::spawn().await;
    // As the extension registers: `getSessionId()` and the session file.
    let file = format!("/Users/alex/.pi/agent/sessions/--Users-alex-work-app--/{id}.jsonl");
    let sid = session(
        &ts,
        "/api/sessions",
        "pi",
        &json!({"harness": "pi", "harness_session_id": id, "cwd": header["cwd"],
                "pid": 1, "parent_pid": 1, "transcript_path": file}),
    )
    .await;
    let refs = refs(&ts, &sid).await;
    assert!(
        has(&refs, "agent-session", &format!("agent://pi/{id}")),
        "{refs:?}"
    );
    assert!(
        has(&refs, "transcript", &format!("file://{file}")),
        "{refs:?}"
    );
}

//! What `clax hook` tells the daemon about itself (spec
//! 2026-10-06-toolpath-audit-design §6.8, §6.9, §9.3): every request names
//! the `hook` channel, the session join carries the git state of the hook's
//! working directory and the transcript path, and the Stop hook's turn end
//! is recorded as the session's agent on `hook`, with git.

use crate::common::Env;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Runs a fixture-building git command in `dir`, isolated from the
/// machine's git configuration.
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repository with one commit and an unstaged change to it.
fn dirty_repo(e: &Env) -> PathBuf {
    let root = e.dir.path().join("app");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-q", "-m", "one"]);
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    root.canonicalize().unwrap()
}

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

fn api(e: &Env, method: &str, path: &str, headers: &[(&str, &str)], body: Value) -> Value {
    let (base, token) = daemon(e);
    let mut req = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .request(method.parse().unwrap(), format!("{base}{path}"))
        .bearer_auth(token)
        .json(&body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = req.send().unwrap();
    assert!(
        res.status().is_success(),
        "{method} {path}: {}",
        res.status()
    );
    res.json().unwrap()
}

/// One recorded event: its kind, actor and body.
struct Rec {
    kind: String,
    actor: Value,
    body: Value,
}

fn events(e: &Env) -> Vec<Rec> {
    let c = rusqlite::Connection::open(e.dir.path().join("ax/clax.db")).unwrap();
    let mut q = c
        .prepare("SELECT kind, actor, body FROM audit_events ORDER BY seq")
        .unwrap();
    q.query_map([], |r| {
        Ok(Rec {
            kind: r.get(0)?,
            actor: serde_json::from_str(&r.get::<_, String>(1)?).unwrap(),
            body: serde_json::from_str(&r.get::<_, String>(2)?).unwrap(),
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// Runs `clax hook --agent claude <event>` with `stdin`, with a capture
/// deadline a loaded machine cannot pass.
fn hook(e: &Env, event: &str, stdin: &Value) {
    e.cmd()
        .env("CLAX_TEST_GIT_DEADLINE_MS", "20000")
        .args(["hook", "--agent", "claude", event])
        .write_stdin(stdin.to_string())
        .assert()
        .success();
}

#[test]
fn hook_join_carries_git_and_transcript() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let repo = dirty_repo(&e);
    let transcript = e.dir.path().join("claude/projects/app/cc-git-1.jsonl");
    hook(
        &e,
        "session-start",
        &json!({"session_id": "cc-git-1", "transcript_path": transcript, "cwd": repo,
                "hook_event_name": "SessionStart", "source": "startup"}),
    );
    let recs = events(&e);
    let start = recs
        .iter()
        .find(|r| r.kind == "session.start")
        .expect("the join made the session");
    assert_eq!(start.body["harness_session_id"], "cc-git-1");
    assert_eq!(start.body["transcript_path"], transcript.to_str().unwrap());
    assert_eq!(start.body["via"], "hook");
    assert_eq!(start.body["git_capture"], "ok", "{}", start.body);
    let g = &start.body["git"];
    assert_eq!(g["repo_root"], repo.to_str().unwrap());
    assert_eq!(g["branch"], "main");
    assert_eq!(g["dirty"], true);
    assert!(
        g["diff_sha256"].as_str().unwrap().starts_with("sha256:"),
        "{g}"
    );
    assert_eq!(start.actor["type"], "agent");
    assert_eq!(start.actor["harness_session_id"], "cc-git-1");
    assert_eq!(start.actor["transcript_path"], transcript.to_str().unwrap());

    // A working directory that is not a repository says so.
    let plain = e.dir.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    hook(
        &e,
        "session-start",
        &json!({"session_id": "cc-git-2", "cwd": plain, "hook_event_name": "SessionStart"}),
    );
    let recs = events(&e);
    let second = recs.iter().rfind(|r| r.kind == "session.start").unwrap();
    assert_eq!(second.body["harness_session_id"], "cc-git-2");
    assert_eq!(second.body["git_capture"], "not-a-repo");
    assert!(second.body.get("git").is_none());
    e.stop();
}

#[test]
fn hook_requests_send_via_hook() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let repo = dirty_repo(&e);
    hook(
        &e,
        "session-start",
        &json!({"session_id": "cc-via-1", "cwd": repo, "hook_event_name": "SessionStart"}),
    );
    let sessions = api(&e, "GET", "/api/sessions?live=true", &[], Value::Null);
    let sid = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["harness_session_id"] == "cc-via-1")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The session works on an artifact, so the Stop hook's turn end ends
    // something.
    let made = api(
        &e,
        "POST",
        "/api/artifacts",
        &[("x-clax-session", &sid)],
        json!({"title": "T", "files": {"index.html": {"content": "<title>T</title>", "encoding": "utf8"}}}),
    );
    let aid = made["artifact"]["id"].as_str().unwrap();
    api(
        &e,
        "PUT",
        &format!("/api/sessions/{sid}/working/{aid}"),
        &[("x-clax-session", &sid)],
        json!({"message": "on it"}),
    );
    hook(
        &e,
        "stop",
        &json!({"session_id": "cc-via-1", "cwd": repo, "hook_event_name": "Stop",
                "stop_hook_active": false}),
    );
    let recs = events(&e);
    let stop = recs
        .iter()
        .rfind(|r| r.kind == "working.stop")
        .expect("the turn end ended the working record");
    assert_eq!(stop.body["via"], "hook", "{}", stop.body);
    assert_eq!(stop.actor["type"], "agent");
    assert_eq!(stop.actor["session_id"], sid.as_str());
    assert_eq!(stop.actor["harness_session_id"], "cc-via-1");
    assert_eq!(stop.body["git_capture"], "ok");
    assert_eq!(stop.body["git"]["repo_root"], repo.to_str().unwrap());
    // The join was the session's agent on `hook` too.
    let start = recs.iter().find(|r| r.kind == "session.start").unwrap();
    assert_eq!(start.body["via"], "hook");
    assert_eq!(start.actor["session_id"], sid.as_str());
    e.stop();
}

#[test]
fn hook_questions_send_via_hook_with_git() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let repo = dirty_repo(&e);
    hook(
        &e,
        "session-start",
        &json!({"session_id": "cc-ask-1", "cwd": repo, "hook_event_name": "SessionStart"}),
    );
    let questions = json!({"questions": [{"question": "Which layout?", "header": "Layout",
        "options": [{"label": "Cards", "description": "c"}, {"label": "Table", "description": "t"}],
        "multiSelect": false}]});
    // No surface is open, so the question goes to the terminal at once.
    hook(
        &e,
        "ask",
        &json!({"session_id": "cc-ask-1", "cwd": repo, "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion", "tool_input": questions,
                "tool_use_id": "toolu_ask_1"}),
    );
    // The answer given in the terminal.
    let mut response = questions.clone();
    response["answers"] = json!({"Which layout?": "Table"});
    hook(
        &e,
        "asked",
        &json!({"session_id": "cc-ask-1", "cwd": repo, "hook_event_name": "PostToolUse",
                "tool_name": "AskUserQuestion", "tool_input": questions,
                "tool_response": response, "tool_use_id": "toolu_ask_1"}),
    );
    let recs = events(&e);
    for kind in ["question.ask", "question.answer"] {
        let r = recs.iter().find(|r| r.kind == kind).unwrap_or_else(|| {
            panic!(
                "no {kind} in {:?}",
                recs.iter().map(|r| &r.kind).collect::<Vec<_>>()
            )
        });
        assert_eq!(r.body["via"], "hook", "{kind}: {}", r.body);
        assert_eq!(r.body["git_capture"], "ok", "{kind}: {}", r.body);
        assert_eq!(r.body["git"]["repo_root"], repo.to_str().unwrap(), "{kind}");
    }
    e.stop();
}

//! A fake Grok Build loads the Clax plugins the way Grok does: MCP servers
//! merged by name with the first definition kept, every enabled plugin's
//! hooks run, `GROK_SESSION_ID` and `GROK_HOOK_EVENT` set, and Grok's
//! camelCase hook input. Checks that exactly one Clax copy acts in every
//! combination of the Claude Code copy and clax-grok, and that a monitor
//! running `clax feedback follow` is told about a comment once.

use assert_cmd::cargo::cargo_bin;
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
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

fn claude_copy() -> PathBuf {
    Path::new(REPO).join("plugins/claude-code")
}
fn grok_copy() -> PathBuf {
    Path::new(REPO).join("plugins/clax-grok")
}

/// A scratch world: a home, a project, a Clax home with a daemon on a free
/// port that its `config.toml` names, so nothing falls back to the default
/// port.
struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let w = World {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(w.project()).unwrap();
        let ok = w
            .cmd(&cargo_bin("clax"))
            .args(["--port", "0", "serve"])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(ok.success());
        let (base, _) = w.daemon();
        let port = base.rsplit(':').next().unwrap().to_string();
        std::fs::write(
            w.clax_home().join("config.toml"),
            format!("[serve]\nport = {port}\n"),
        )
        .unwrap();
        w
    }
    fn clax_home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }
    fn project(&self) -> PathBuf {
        self.dir.path().join("project")
    }
    /// A command with the harness environment cleared and the scratch homes set.
    fn cmd(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        for k in HARNESS_VARS {
            c.env_remove(k);
        }
        c.env("HOME", self.dir.path())
            .env("CLAX_HOME", self.clax_home())
            .env("GROK_HOME", self.dir.path().join("grok-home"))
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("claude"))
            .env("CLAX_BIN", cargo_bin("clax"))
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env("RUST_LOG", "error")
            .current_dir(self.project());
        c
    }
    fn daemon(&self) -> (String, String) {
        let info: Value =
            serde_json::from_slice(&std::fs::read(self.clax_home().join("daemon.json")).unwrap())
                .unwrap();
        (
            format!("http://127.0.0.1:{}", info["port"]),
            info["token"].as_str().unwrap().to_string(),
        )
    }
    fn http(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
    }
    fn api(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (base, token) = self.daemon();
        let mut r = self
            .http()
            .request(method.parse().unwrap(), format!("{base}{path}"))
            .bearer_auth(token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send().unwrap().json().unwrap_or(Value::Null)
    }
    fn live_sessions(&self) -> Vec<Value> {
        self.api("GET", "/api/sessions?live=true", None)["sessions"]
            .as_array()
            .unwrap()
            .clone()
    }
    fn hooks_log(&self) -> String {
        std::fs::read_to_string(self.clax_home().join("logs/hooks.log")).unwrap_or_default()
    }
    /// Opens a thread on `aid` sent to the agent (its body mentions
    /// @agent), as the shell does; returns its ID.
    fn sent_thread(&self, aid: &str, body: &str) -> String {
        let (base, _) = self.daemon();
        let form = reqwest::blocking::multipart::Form::new()
            .text(
                "anchor",
                r#"{"kind":"element","selector":"body > p","quote":"hi"}"#,
            )
            .text("body", format!("@agent {body}"))
            .text("version", "1");
        let res = self
            .http()
            .post(format!("{base}/api/artifacts/{aid}/threads"))
            .multipart(form)
            .send()
            .unwrap();
        assert_eq!(res.status(), 201);
        let t: Value = res.json().unwrap();
        t["thread"]["id"]
            .as_str()
            .or(t["id"].as_str())
            .unwrap()
            .to_string()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self
            .cmd(&cargo_bin("clax"))
            .arg("stop")
            .stdout(Stdio::null())
            .status();
    }
}

/// `${CLAUDE_PLUGIN_ROOT}` and `${GROK_PLUGIN_ROOT}` expanded, as Grok does.
fn expand(s: &str, root: &Path) -> String {
    let r = root.display().to_string();
    s.replace("${CLAUDE_PLUGIN_ROOT}", &r)
        .replace("${GROK_PLUGIN_ROOT}", &r)
}

/// Each plugin's servers, merged by name with the first definition kept.
fn merged_servers(plugins: &[PathBuf]) -> Vec<(String, PathBuf, Value)> {
    let mut out: Vec<(String, PathBuf, Value)> = Vec::new();
    for p in plugins {
        let d: Value =
            serde_json::from_slice(&std::fs::read(p.join(".mcp.json")).unwrap()).unwrap();
        let servers: Map<String, Value> = d
            .get("mcpServers")
            .unwrap_or(&d)
            .as_object()
            .unwrap()
            .clone();
        for (name, s) in servers {
            if !out.iter().any(|(n, _, _)| *n == name) {
                out.push((name, p.clone(), s));
            }
        }
    }
    out
}

/// A spawned MCP server, after the handshake.
struct Server {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    instructions: String,
}

impl Server {
    fn spawn(w: &World, name: &str, root: &Path, def: &Value, session: &str) -> Server {
        let mut c = w.cmd(Path::new(&expand(def["command"].as_str().unwrap(), root)));
        for a in def["args"].as_array().into_iter().flatten() {
            c.arg(expand(a.as_str().unwrap(), root));
        }
        for (k, v) in def["env"].as_object().into_iter().flatten() {
            c.env(k, expand(v.as_str().unwrap(), root));
        }
        let mut child = c
            .env("GROK_SESSION_ID", session)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut s = Server {
            name: name.into(),
            child,
            stdin,
            stdout,
            instructions: String::new(),
        };
        let init = s.request(
            0,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-grok", "version": "1.0.45"}}),
        );
        assert!(
            init["result"].is_object(),
            "{name} failed its handshake: {init}"
        );
        s.instructions = init["result"]["instructions"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        writeln!(
            s.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .unwrap();
        s
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
                "{} closed stdout",
                self.name
            );
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }
    fn tools(&mut self) -> Vec<String> {
        self.request(1, "tools/list", json!({}))["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }
    fn call(&mut self, id: u64, tool: &str, args: Value) -> Value {
        self.request(id, "tools/call", json!({"name": tool, "arguments": args}))["result"].clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs every enabled plugin's hooks for `event`; each one's stdout.
fn run_hooks(
    w: &World,
    plugins: &[PathBuf],
    event: &str,
    session: &str,
    extra: Value,
) -> Vec<(PathBuf, String)> {
    let mut envelope = json!({
        "hookEventName": event, "hook_event_name": event,
        "sessionId": session, "session_id": session,
        "cwd": w.project(), "workspaceRoot": w.project(),
        "timestamp": "2026-10-01T10:00:00Z", "permissionMode": "ask",
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        envelope[k] = v.clone();
    }
    let mut out = Vec::new();
    for p in plugins {
        let hooks: Value =
            serde_json::from_slice(&std::fs::read(p.join("hooks/hooks.json")).unwrap()).unwrap();
        for entry in hooks["hooks"][event].as_array().into_iter().flatten() {
            for h in entry["hooks"].as_array().into_iter().flatten() {
                let mut child = w
                    .cmd(Path::new("/bin/sh"))
                    .args(["-c", h["command"].as_str().unwrap()])
                    .env("GROK_HOOK_EVENT", event)
                    .env("GROK_SESSION_ID", session)
                    .env("GROK_PLUGIN_ROOT", p)
                    .env("CLAUDE_PLUGIN_ROOT", p)
                    .env("CLAUDE_PROJECT_DIR", w.project())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(envelope.to_string().as_bytes())
                    .unwrap();
                let o = child.wait_with_output().unwrap();
                assert!(
                    o.status.success(),
                    "{} {event} hook exited {:?}",
                    p.display(),
                    o.status
                );
                out.push((p.clone(), String::from_utf8(o.stdout).unwrap()));
            }
        }
    }
    out
}

/// One Grok session with `plugins` enabled, in that discovery order.
struct Session {
    servers: Vec<Server>,
}

fn start(w: &World, plugins: &[PathBuf], id: &str) -> Session {
    let servers = merged_servers(plugins)
        .into_iter()
        .map(|(name, root, def)| Server::spawn(w, &name, &root, &def, id))
        .collect();
    let s = Session { servers };
    run_hooks(w, plugins, "SessionStart", id, json!({"source": "startup"}));
    s
}

/// The servers that act (offer more than `status`), by name.
fn acting(s: &mut Session) -> Vec<String> {
    s.servers
        .iter_mut()
        .filter_map(|srv| (srv.tools().len() > 1).then(|| srv.name.clone()))
        .collect()
}

#[test]
fn both_copies_enabled_one_acts_in_either_discovery_order() {
    for order in [[claude_copy(), grok_copy()], [grok_copy(), claude_copy()]] {
        let w = World::new();
        let mut s = start(&w, &order, "019a-both");
        let mut names: Vec<_> = s.servers.iter().map(|x| x.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["clax", "clax_grok"], "distinct names: both load");
        assert_eq!(acting(&mut s), ["clax_grok"]);
        let idle = s.servers.iter_mut().find(|x| x.name == "clax").unwrap();
        assert_eq!(idle.tools(), ["status"]);
        let r = idle.call(2, "status", json!({}));
        assert_ne!(r["isError"], json!(true));
        assert!(
            r["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("clax_grok__publish"),
            "{r}"
        );
        let live = w.live_sessions();
        assert_eq!(live.len(), 1, "{live:?}");
        assert_eq!(
            (
                live[0]["harness"].as_str(),
                live[0]["harness_session_id"].as_str()
            ),
            (Some("grok"), Some("019a-both"))
        );
        let log = w.hooks_log();
        assert_eq!(
            log.matches(" hook agent=grok event=session-start ").count(),
            1,
            "{log}"
        );
        assert!(!log.contains(" hook agent=claude "), "{log}");
        assert!(
            log.contains(" standdown mode=hook agent=claude host=grok"),
            "{log}"
        );
        assert!(
            log.contains(" standdown mode=mcp agent=claude host=grok"),
            "{log}"
        );
    }
}

#[test]
fn only_clax_grok_acts_alone() {
    let w = World::new();
    let mut s = start(&w, &[grok_copy()], "019a-grok");
    assert_eq!(acting(&mut s), ["clax_grok"]);
    assert_eq!(w.live_sessions().len(), 1);
    assert!(!w.hooks_log().contains("standdown"));
}

#[test]
fn only_the_claude_copy_says_how_to_get_clax_grok_and_acts_nowhere() {
    let w = World::new();
    let mut s = start(&w, &[claude_copy()], "019a-claude");
    assert!(acting(&mut s).is_empty());
    assert_eq!(s.servers.len(), 1);
    assert!(
        s.servers[0].instructions.contains("clax init --agent grok"),
        "{}",
        s.servers[0].instructions
    );
    assert!(w.live_sessions().is_empty(), "no session is registered");
    let out = run_hooks(
        &w,
        &[claude_copy()],
        "Stop",
        "019a-claude",
        json!({"reason": "end_turn", "stopHookActive": false}),
    );
    assert!(out.iter().all(|(_, o)| o.is_empty()), "{out:?}");
}

#[test]
fn neither_copy_means_no_clax() {
    let w = World::new();
    let s = start(&w, &[], "019a-none");
    assert!(s.servers.is_empty());
    assert!(w.live_sessions().is_empty());
}

/// The artifact ID in a `publish` result: its first text item is JSON.
fn published_id(r: &Value) -> String {
    for item in r["content"].as_array().into_iter().flatten() {
        let Some(text) = item["text"].as_str() else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<Value>(text)
            && let Some(id) = v["artifact_id"].as_str()
        {
            return id.to_string();
        }
    }
    panic!("no artifact ID in {r}");
}

#[test]
fn with_both_enabled_a_comment_is_announced_once_and_handed_over_once() {
    let w = World::new();
    let both = [claude_copy(), grok_copy()];
    let mut s = start(&w, &both, "019a-loop");
    let grok = s
        .servers
        .iter_mut()
        .find(|x| x.name == "clax_grok")
        .unwrap();
    // Publishing watches the artifact with replies armed.
    let r = grok.call(
        10,
        "publish",
        json!({"title": "Loop", "html": "<title>Loop</title><p id=x>hi</p>"}),
    );
    assert_ne!(r["isError"], json!(true), "{r}");
    let aid = published_id(&r);

    // The monitor the skill starts: its command line run through sh with the
    // session's environment.
    let mut monitor = w
        .cmd(Path::new("/bin/sh"))
        .args([
            "-c",
            &format!(
                "\"{}\" feedback follow --agent grok --harness-session 019a-loop --poll-secs 2 --grace-secs 1",
                cargo_bin("clax").display()
            ),
        ])
        .env("GROK_SESSION_ID", "019a-loop")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = monitor.stdout.take().unwrap();
    let (tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    // A viewer comments and sends it to the agent.
    let tid = w.sent_thread(&aid, "make it bigger");
    let line = lines
        .recv_timeout(Duration::from_secs(10))
        .expect("the monitor announces the comment");
    assert!(line.starts_with("[clax] New comment on "), "{line}");
    assert!(line.contains(&tid), "{line}");

    // Stop: exactly one block, from clax-grok; the Claude copy's Stop prints nothing.
    let out = run_hooks(
        &w,
        &both,
        "Stop",
        "019a-loop",
        json!({"reason": "end_turn", "stopHookActive": false}),
    );
    let blocks: Vec<_> = out
        .iter()
        .filter(|(_, o)| o.contains("\"decision\":\"block\""))
        .collect();
    assert_eq!(blocks.len(), 1, "{out:?}");
    assert_eq!(blocks[0].0, grok_copy());
    assert!(blocks[0].1.contains(&tid), "{}", blocks[0].1);

    // The next Stop (stopHookActive) blocks nothing; the monitor stays quiet.
    let again = run_hooks(
        &w,
        &both,
        "Stop",
        "019a-loop",
        json!({"reason": "end_turn", "stopHookActive": true}),
    );
    assert!(again.iter().all(|(_, o)| o.is_empty()), "{again:?}");
    match lines.recv_timeout(Duration::from_secs(4)) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("the monitor printed more: {other:?}"),
    }

    // The session-end Stop does nothing either.
    let shutdown = run_hooks(
        &w,
        &both,
        "Stop",
        "019a-loop",
        json!({"reason": "shutdown"}),
    );
    assert!(shutdown.iter().all(|(_, o)| o.is_empty()), "{shutdown:?}");

    // SessionEnd ends the row; the monitor then exits 0 and wrote nothing to stderr.
    run_hooks(&w, &both, "SessionEnd", "019a-loop", json!({}));
    assert!(w.live_sessions().is_empty());
    let started = Instant::now();
    let status = loop {
        if let Some(st) = monitor.try_wait().unwrap() {
            break st;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let _ = monitor.kill();
            panic!("the monitor did not exit within 5 s of SessionEnd");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    let mut err = String::new();
    monitor
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(err.is_empty(), "{err}");
}

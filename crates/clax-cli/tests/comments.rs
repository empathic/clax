//! End-to-end tests of `clax comments`, `clax versions`, `clax db` and the
//! working roster in `clax status`, against a daemon in a temporary
//! `CLAX_HOME` on a free port.

use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The harness environment a test inherits from the agent running it; every
/// `clax` run here starts without it.
const HARNESS_ENV: [&str; 12] = [
    "GROK_SESSION_ID",
    "GROK_HOOK_EVENT",
    "GROK_PLUGIN_ROOT",
    "GROK_HOME",
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "PI_CODING_AGENT_DIR",
    "CLAX_SESSION_ID",
];

/// A viewer cookie standing for someone else's browser.
const MARIA: &str = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";

fn clax(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_clax"));
    for var in HARNESS_ENV {
        c.env_remove(var);
    }
    c.env("CLAX_HOME", home)
        .env("HOME", home.parent().unwrap())
        .env("CLAX_CODEX_BIN", "")
        .env("CLAX_NO_OPEN", "1")
        .env("RUST_LOG", "error")
        .env_remove("NO_COLOR");
    c
}

/// A daemon in a temp home; stopped on drop.
struct Daemon {
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let d = Daemon {
            dir: tempfile::tempdir().unwrap(),
        };
        let st = clax(&d.home())
            .args(["--port", "0", "serve"])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
        d
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }
    fn info(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.home().join("daemon.json")).unwrap()).unwrap()
    }
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.info()["port"])
    }
    fn token(&self) -> String {
        self.info()["token"].as_str().unwrap().to_string()
    }
    fn http(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
    }
    fn api(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let mut req = self
            .http()
            .request(method, format!("{}{path}", self.base()))
            .bearer_auth(self.token());
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().unwrap();
        assert!(res.status().is_success(), "{path}: {}", res.status());
        res.json().unwrap()
    }

    /// Registers a live Claude Code session; returns its ID.
    fn session(&self, hsid: &str) -> String {
        let v = self.api(
            reqwest::Method::POST,
            "/api/sessions",
            Some(json!({"harness": "claude", "harness_session_id": hsid, "cwd": "/w"})),
        );
        v["session"]["id"].as_str().unwrap().to_string()
    }

    /// Publishes an artifact (as `session`, which then owns and watches it);
    /// returns its ID.
    fn publish(&self, session: Option<&str>, title: &str, files: Value, caps: Value) -> String {
        let mut req = self
            .http()
            .post(format!("{}/api/artifacts", self.base()))
            .bearer_auth(self.token())
            .json(&json!({"title": title, "files": files, "capabilities": caps}));
        if let Some(s) = session {
            req = req.header("x-clax-session", s);
        }
        let res = req.send().unwrap();
        assert_eq!(res.status(), 201);
        let v: Value = res.json().unwrap();
        v["artifact"]["id"].as_str().unwrap().to_string()
    }

    /// Names Maria's viewer.
    fn name_maria(&self) {
        let res = self
            .http()
            .put(format!("{}/api/viewers/me", self.base()))
            .header("cookie", format!("clax_viewer={MARIA}"))
            .json(&json!({"display_name": "Maria"}))
            .send()
            .unwrap();
        assert!(res.status().is_success());
    }

    /// A thread Maria leaves on `file` of version 1; returns its ID.
    fn thread(&self, aid: &str, file: &str, selector: &str, quote: &str, body: &str) -> String {
        let anchor = json!({"kind": "element", "selector": selector, "quote": quote, "file": file});
        let form = reqwest::blocking::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", body.to_string())
            .text("version", "1");
        let res = self
            .http()
            .post(format!("{}/api/artifacts/{aid}/threads", self.base()))
            .header("cookie", format!("clax_viewer={MARIA}"))
            .multipart(form)
            .send()
            .unwrap();
        assert_eq!(res.status(), 201);
        let t: Value = res.json().unwrap();
        t["thread"]["id"].as_str().unwrap().to_string()
    }

    fn run(&self, args: &[&str]) -> Output {
        clax(&self.home()).args(args).output().unwrap()
    }

    /// Runs a command that must succeed; its stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "clax {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Runs a command with `--json` that must succeed; its object.
    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        serde_json::from_str(&self.ok(&all)).unwrap()
    }

    /// Runs a command that must fail; its stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "clax {args:?} succeeded");
        String::from_utf8(out.stderr).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = clax(&self.home())
            .arg("stop")
            .stdout(Stdio::null())
            .status();
    }
}

/// Two artifacts: "Review" (index.html and about.html, owned by a live
/// claude session that is working on thread #1, which it replied to), with
/// #1 sent, #2 resolved, #3 on about.html, which version 2 drops; and
/// "Roadmap" with one thread whose text carries terminal escapes.
struct World {
    d: Daemon,
    sid: String,
    review: String,
    roadmap: String,
    t: [String; 4],
}

fn world() -> World {
    let d = Daemon::start();
    d.name_maria();
    let sid = d.session("h1");
    let review = d.publish(
        Some(&sid),
        "Review",
        json!({"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"},
               "about.html": {"content": "<p>About</p>", "encoding": "utf8"}}),
        json!({}),
    );
    let roadmap = d.publish(
        None,
        "Roadmap",
        json!({"index.html": {"content": "<h1>Roadmap</h1>", "encoding": "utf8"}}),
        json!({}),
    );
    let t0 = d.thread(
        &review,
        "index.html",
        "body > main > h2",
        "Quarterly goals",
        "@agent Two columns would read better",
    );
    let t1 = d.thread(
        &review,
        "index.html",
        "body > main > h2",
        "Quarterly",
        "Typo?",
    );
    let t2 = d.thread(&review, "about.html", "body > p", "About", "Old page");
    let t3 = d.thread(
        &roadmap,
        "index.html",
        "body > h1",
        "Road\x1b[2Jmap",
        "Evil \x1b[31mred\x1b[0m \x1b]0;pwned\x07 \u{202e}txet",
    );
    let res = d
        .http()
        .post(format!(
            "{}/api/artifacts/{review}/threads/{t1}/resolve",
            d.base()
        ))
        .header("cookie", format!("clax_viewer={MARIA}"))
        .send()
        .unwrap();
    assert!(res.status().is_success());
    let res = d
        .http()
        .post(format!(
            "{}/api/artifacts/{review}/threads/{t0}/comments",
            d.base()
        ))
        .bearer_auth(d.token())
        .header("x-clax-session", &sid)
        .json(&json!({"body": "On it.", "author_kind": "agent"}))
        .send()
        .unwrap();
    assert!(res.status().is_success());
    d.api(
        reqwest::Method::POST,
        &format!("/api/artifacts/{review}/versions"),
        Some(json!({"if_version": 1, "label": "Two columns", "note": "Split the goals",
                    "addresses": [t0], "files": {
                        "index.html": {"content": "<main><h2>Quarterly goals</h2><p>2</p></main>", "encoding": "utf8"},
                        "about.html": null}})),
    );
    d.api(
        reqwest::Method::PUT,
        &format!("/api/sessions/{sid}/working/{review}"),
        Some(json!({"thread_ids": [t0], "message": "Reflowing"})),
    );
    World {
        d,
        sid,
        review,
        roadmap,
        t: [t0, t1, t2, t3],
    }
}

#[test]
fn listings_group_by_artifact_newest_activity_first() {
    let w = world();
    let d = &w.d;
    let v = d.json(&["comments"]);
    let groups = v["artifacts"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{v}");
    // Roadmap's thread is newer than any of Review's, but Review has the
    // agent's reply since.
    assert_eq!(groups[0]["artifact_id"], w.review.as_str());
    assert_eq!(groups[1]["artifact_id"], w.roadmap.as_str());
    assert_eq!(groups[0]["title"], "Review");
    assert!(
        groups[0]["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/a/{}", w.review))
    );
    assert!(v["note"].as_str().unwrap().contains("people viewing"));
    let refs: Vec<&str> = groups[0]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["ref"].as_str().unwrap())
        .collect();
    let r = |n: u32| format!("{}#{n}", w.review);
    assert_eq!(
        refs,
        [r(1), r(3)],
        "open threads only, newest activity first"
    );
    let first = &groups[0]["threads"][0];
    // The `comments_read` thread object, and the listing's own fields.
    for key in [
        "thread_id",
        "status",
        "sent_to_agent",
        "version",
        "anchor",
        "clip_path",
        "comments",
        "feedback_state",
    ] {
        assert!(first.get(key).is_some(), "{key} missing from {first}");
    }
    assert_eq!(first["thread_id"], w.t[0].as_str());
    assert_eq!(first["n"], 1);
    assert_eq!(first["sent_to_agent"], true);
    assert_eq!(first["addressed_in"], json!([2]));
    assert_eq!(first["working"][0]["harness"], "claude");
    assert_eq!(first["working"][0]["session_id"], w.sid.as_str());
    assert_eq!(first["working"][0]["message"], "Reflowing");
    assert_eq!(first["comments"][1]["author_kind"], "agent");
    assert_eq!(groups[0]["threads"][1]["detached"], true);
    assert_eq!(first["detached"], false);

    let all = d.json(&["comments", "--all"]);
    let n: Vec<u64> = all["artifacts"][0]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["n"].as_u64().unwrap())
        .collect();
    assert_eq!(n, [1, 2, 3], "--all adds #2, resolved after #3 was made");

    let one = d.json(&["comments", &w.roadmap]);
    assert_eq!(one["artifact_id"], w.roadmap.as_str());
    assert_eq!(one["threads"].as_array().unwrap().len(), 1);
    assert!(one["note"].is_string() && one["next_cursor"].is_null());
    let by_url = d.json(&["comments", one["url"].as_str().unwrap()]);
    assert_eq!(by_url["threads"], one["threads"]);

    let text = d.ok(&["comments"]);
    let review_at = text.find("Review").unwrap();
    let roadmap_at = text.find("Roadmap").unwrap();
    assert!(review_at < roadmap_at, "{text}");
    assert!(text.contains(&w.review) && text.contains("#3"), "{text}");
    assert!(text.contains("detached"), "{text}");
    assert!(text.contains("claude working: Reflowing"), "{text}");
    assert!(text.contains("claude (agent): On it."), "{text}");
    assert!(!text.contains("#2"), "resolved threads need --all: {text}");
    assert!(d.ok(&["comments", "--all"]).contains("resolved"));
}

#[test]
fn readable_output_never_carries_terminal_controls() {
    let w = world();
    let d = &w.d;
    for args in [
        vec!["comments".to_string()],
        vec!["comments".to_string(), format!("{}#1", w.roadmap)],
    ] {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let text = d.ok(&args);
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
        assert!(!text.contains('\u{202e}'), "{text:?}");
        assert!(text.contains("\\x1b[31mred"), "{text}");
        assert!(text.contains("\\u{202e}txet"), "{text}");
    }
    // Under a pipe the output is plain, so even the CLI's own styling is absent.
    assert!(!d.ok(&["status"]).contains('\x1b'));
}

#[test]
fn show_prints_the_whole_thread() {
    let w = world();
    let d = &w.d;
    let r1 = format!("{}#1", w.review);
    let text = d.ok(&["comments", &r1]);
    for want in [
        r1.as_str(),
        w.t[0].as_str(),
        "body > main > h2",
        "«Quarterly goals»",
        "addressed in v2",
        "Maria",
        "@agent Two columns would read better",
        "claude (agent)",
        "On it.",
        "claude working: Reflowing",
        "sent to agent",
    ] {
        assert!(text.contains(want), "{want} missing from:\n{text}");
    }
    assert_eq!(d.ok(&["comments", "show", &r1]), text);
    // A thread ID names the thread on its own, and with its artifact.
    assert_eq!(d.ok(&["comments", &w.t[0]]), text);
    assert_eq!(
        d.ok(&["comments", &format!("{}#{}", w.review, w.t[0])]),
        text
    );
    let v = d.json(&["comments", &r1]);
    assert_eq!(v["artifact_id"], w.review.as_str());
    assert_eq!(v["threads"].as_array().unwrap().len(), 1);
    assert_eq!(v["threads"][0]["thread_id"], w.t[0].as_str());
    assert_eq!(v["threads"][0]["comments"].as_array().unwrap().len(), 2);
    let e = d.fails(&["comments", &format!("{}#9", w.review)]);
    assert!(e.contains("no thread #9") && e.contains("has 3"), "{e}");
    let e = d.fails(&["comments", "show", &w.review]);
    assert!(e.contains("names no thread"), "{e}");
}

#[test]
fn acting_on_threads_is_acting_as_the_owner_in_the_browser() {
    let w = world();
    let d = &w.d;
    let name = d.json(&["comments", "name", "Alex"]);
    assert_eq!(name["display_name"], "Alex");
    let me = name["public_id"].as_str().unwrap().to_string();
    assert_eq!(d.ok(&["comments", "name"]).trim(), "Alex");

    let r1 = format!("{}#1", w.review);
    let v = d.json(&["comments", "reply", &r1, "Thanks"]);
    assert_eq!(v["replied"], true);
    assert_eq!(v["author_name"], "Alex");
    assert_eq!(
        v["sent_to_agent"], true,
        "a reply on a sent thread goes on to the agent"
    );
    // From stdin.
    let mut child = clax(&d.home())
        .args(["comments", "reply", &format!("{}#3", w.review), "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Line one\nline two")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("as Alex"));

    // Stored exactly as a browser viewer's comment.
    let t = d.api(
        reqwest::Method::GET,
        &format!("/api/artifacts/{}/threads/{}", w.review, w.t[2]),
        None,
    );
    let c = &t["thread"]["comments"][1];
    assert_eq!(c["author_kind"], "viewer");
    assert_eq!(c["author_name"], "Alex");
    assert_eq!(c["author_public_id"], me.as_str());
    assert_eq!(c["body"], "Line one\nline two");
    assert!(c["via_harness"].is_null());

    let v = d.json(&["comments", "resolve", &format!("{}#3", w.review)]);
    assert_eq!(
        (v["resolved"].clone(), v["status"].clone()),
        (json!(true), json!("resolved"))
    );
    let t = d.api(
        reqwest::Method::GET,
        &format!("/api/artifacts/{}/threads/{}", w.review, w.t[2]),
        None,
    );
    assert_eq!(t["thread"]["resolved_by"], format!("viewer:{me}"));
    assert_eq!(t["thread"]["resolved_by_name"], "Alex");
    let e = d.fails(&["comments", "send", &format!("{}#3", w.review)]);
    assert!(e.contains("thread_resolved"), "{e}");

    let v = d.json(&["comments", "reopen", &format!("{}#2", w.review)]);
    assert_eq!(
        (v["reopened"].clone(), v["status"].clone()),
        (json!(true), json!("open"))
    );

    // Send goes to the live agent the page would pick.
    let v = d.json(&["comments", "send", &format!("{}#2", w.review)]);
    assert_eq!(v["sent"], true);
    assert!(v["to"].as_str().unwrap().starts_with("a_"), "{v}");
    assert_eq!(v["harness"], "claude");
    assert_eq!(v["feedback_state"]["state"], "sent");
    let e = d.fails(&[
        "comments",
        "send",
        &format!("{}#2", w.review),
        "--to",
        "a_nobody",
    ]);
    assert!(e.contains("unknown_agent"), "{e}");
    // No agent is live on Roadmap: the thread waits, as in the browser.
    let v = d.json(&["comments", "send", &format!("{}#1", w.roadmap)]);
    assert_eq!(v["sent"], true);
    assert!(v["to"].is_null());
    let text = d.ok(&["comments", "send", &format!("{}#1", w.roadmap)]);
    assert!(text.contains("no agent is live"), "{text}");
}

#[test]
fn the_cli_is_the_owner_the_browsers_are() {
    let w = world();
    let d = &w.d;
    let name = d.json(&["comments", "name", "Alex"]);
    // The owner's browser (the owner cookie, as the shell's token request
    // sets it) is the same viewer, with the same name.
    let host = d.base().trim_start_matches("http://").to_string();
    let cookie = format!(
        "{}={}",
        clax_server::identity::owner_cookie_name(&host),
        clax_server::identity::owner_cookie_value(&d.token())
    );
    let browser: Value = d
        .http()
        .get(format!("{}/api/viewers/me", d.base()))
        .header("cookie", cookie)
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(browser["viewer"]["public_id"], name["public_id"]);
    assert_eq!(browser["viewer"]["display_name"], "Alex");
    // A reply from the CLI is authored by that viewer.
    let v = d.json(&["comments", "reply", &format!("{}#1", w.roadmap), "hi"]);
    assert_eq!(v["author_name"], "Alex");
    let t = d.api(
        reqwest::Method::GET,
        &format!("/api/artifacts/{}/threads", w.roadmap),
        None,
    );
    let last = t["threads"][0]["comments"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["author_public_id"], name["public_id"]);
}

#[test]
fn an_unnamed_owner_replies_as_viewer_with_a_hint() {
    let w = world();
    let out =
        w.d.run(&["comments", "reply", &format!("{}#1", w.roadmap), "hi"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("as Viewer"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("clax comments name"));
}

#[test]
fn versions_name_their_publisher_and_the_threads_they_addressed() {
    let w = world();
    let d = &w.d;
    let v = d.json(&["versions", &w.review]);
    assert_eq!(v["artifact_id"], w.review.as_str());
    assert_eq!(v["current_version"], 2);
    let vs = v["versions"].as_array().unwrap();
    assert_eq!(vs[0]["n"], 2, "newest first");
    assert_eq!(vs[0]["label"], "Two columns");
    assert_eq!(vs[0]["note"], "Split the goals");
    assert!(vs[0]["publisher"].is_null(), "published without a session");
    assert_eq!(
        vs[0]["addressed"],
        json!([{"thread_id": w.t[0], "ref": format!("{}#1", w.review)}])
    );
    assert_eq!(vs[0]["files"], json!(["index.html"]));
    assert_eq!(vs[1]["publisher"]["harness"], "claude");
    assert!(
        vs[1]["publisher"]["agent"]
            .as_str()
            .unwrap()
            .starts_with("a_")
    );
    let text = d.ok(&["versions", &w.review]);
    assert!(text.contains("v2 (current)  Two columns"), "{text}");
    assert!(text.contains("addressed #1"), "{text}");
    assert!(
        text.contains("by the command line") && text.contains("by claude"),
        "{text}"
    );
}

#[test]
fn db_round_trips_and_refuses_a_stale_version() {
    let d = Daemon::start();
    let aid = d.publish(
        None,
        "Tracker",
        json!({"index.html": {"content": "<main></main>", "encoding": "utf8"}}),
        json!({"db": {"rules": [{"path": "locked", "write": "admin"}]}}),
    );
    let set = d.json(&[
        "db",
        "set",
        &aid,
        "tasks",
        "t1",
        r#"{"title": "Ship", "done": false}"#,
    ]);
    assert_eq!(set["artifact_id"], aid.as_str());
    assert_eq!(
        (&set["path"], &set["version"], &set["created"]),
        (&json!("tasks/t1"), &json!(1), &json!(true))
    );
    let got = d.json(&["db", "get", &aid, "tasks", "t1"]);
    assert_eq!(got["exists"], true);
    assert_eq!(got["doc"]["data"], json!({"title": "Ship", "done": false}));
    assert!(got["note"].is_string());
    let up = d.json(&[
        "db",
        "update",
        &aid,
        "tasks",
        "t1",
        r#"{"done": true}"#,
        "--if-version",
        "1",
    ]);
    assert!(up["version"].as_u64() > set["version"].as_u64());
    let e = d.fails(&[
        "db",
        "update",
        &aid,
        "tasks",
        "t1",
        r#"{"done": false}"#,
        "--if-version",
        "1",
    ]);
    assert!(e.contains("conflict"), "{e}");
    let v2 = up["version"].to_string();
    let ed = d.json(&[
        "db",
        "str-replace",
        &aid,
        "tasks",
        "t1",
        "title",
        "Ship",
        "Ship it",
        "--if-version",
        &v2,
    ]);
    assert!(ed["version"].as_u64() > up["version"].as_u64());
    let t2 = d.json(&[
        "db",
        "set",
        &aid,
        "tasks",
        "t2",
        r#"{"title": "Test", "done": false}"#,
    ]);
    let list = d.json(&["db", "list", &aid, "tasks"]);
    let ids: Vec<&str> = list["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["t1", "t2"]);
    let q = d.json(&[
        "db",
        "query",
        &aid,
        "tasks",
        "--where",
        r#"["done", "==", false]"#,
    ]);
    assert_eq!(q["docs"].as_array().unwrap().len(), 1);
    assert_eq!(q["docs"][0]["id"], "t2");
    let q = d.json(&[
        "db",
        "query",
        &aid,
        "tasks",
        "--order-by",
        "title",
        "--desc",
    ]);
    assert_eq!(q["docs"][0]["id"], "t2");
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(br#"{"title": "From a file"}"#).unwrap();
    let b = d.json(&[
        "db",
        "batch",
        &aid,
        &json!([
            {"op": "delete", "collection": "tasks", "doc_id": "t2", "if_version": t2["version"]},
            {"op": "set", "collection": "tasks", "doc_id": "t3", "file_path": f.path()},
        ])
        .to_string(),
    ]);
    assert_eq!(b["atomic"], true);
    assert_eq!(b["results"].as_array().unwrap().len(), 2);
    let got = d.json(&["db", "get", &aid, "tasks", "t3"]);
    assert_eq!(got["doc"]["data"]["title"], "From a file");
    let v3 = ed["version"].to_string();
    let del = d.json(&["db", "delete", &aid, "tasks", "t1", "--if-version", &v3]);
    assert_eq!(del["deleted"], true);
    // The owner writes where only admins may; narrowed to interact, not.
    d.json(&["db", "set", &aid, "locked", "x", "{}"]);
    let e = d.fails(&[
        "db",
        "set",
        &aid,
        "locked",
        "y",
        "{}",
        "--as-level",
        "interact",
    ]);
    assert!(
        e.contains("not_found"),
        "a write below the rule reads as absent: {e}"
    );
    let text = d.ok(&["db", "get", &aid, "tasks", "t3"]);
    assert!(text.starts_with("tasks/t3  v"), "{text}");
    assert!(text.contains("\"title\": \"From a file\""), "{text}");
}

#[test]
fn status_lists_who_is_working_on_what() {
    let w = world();
    let d = &w.d;
    let v = d.json(&["status"]);
    let r = v["working"].as_array().unwrap();
    assert_eq!(r.len(), 1, "{v}");
    assert_eq!(r[0]["harness"], "claude");
    assert_eq!(r[0]["session_id"], w.sid.as_str());
    assert_eq!(r[0]["artifact_id"], w.review.as_str());
    assert_eq!(r[0]["title"], "Review");
    assert_eq!(r[0]["threads"], json!([format!("{}#1", w.review)]));
    assert_eq!(r[0]["message"], "Reflowing");
    assert!(r[0]["for_s"].as_i64().is_some());
    let text = d.ok(&["status"]);
    assert!(text.contains("working now:"), "{text}");
    assert!(
        text.contains("on Review") && text.contains("#1: Reflowing") && text.contains(&w.sid),
        "{text}"
    );
    d.api(
        reqwest::Method::POST,
        &format!("/api/sessions/{}/working/end", w.sid),
        None,
    );
    assert!(d.ok(&["status"]).contains("no agent is working"));
}

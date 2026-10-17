//! The single-owner golden (spec 2026-10-08-multi-agent-multi-person §19):
//! every agent-facing output of a single-owner install whose database is at
//! migration 21, per feedback tier, each tier on a fresh copy of one fixture
//! under one fixed clock.
//!
//! - `fixtures/single-owner-21.sql` is the fixture: a whole migration-21
//!   database (schema, rows, `user_version`) and the files its rows name,
//!   loaded by `clax_testkit::load_fixture`. `write_fixture` built
//!   it through the daemon's routes; it runs only at a migration-21 schema.
//! - `fixtures/single-owner-21.golden.json` holds, per tier,
//!   [`collect_agent_outputs`] after [`normalise_minted`]. `write_golden`
//!   writes it: `cargo test -p clax-server --test integration --
//!   --ignored --exact golden_single_owner::write_golden`.
//!
//! Each output is taken through the real client of its channel where one
//! exists: the `clax hook` binary for the Stop and prompt tiers, `clax
//! feedback follow` for notices, `clax versions` for versions, and the MCP
//! tools (`clax_mcp::ClaxTools`, the code the shim serves) for the rest.
//! The queue and inject tiers are taken over HTTP as `codex queue` pushes
//! and the Pi extension take them.
//!
//! A module of clax-server's `integration` test binary; it changes no
//! process-wide state, and uses no shared test module.
//!
//! `the_golden_differs_only_as_section_19_lists` runs on every test run. It
//! collects the outputs now and compares them with the committed golden
//! ([`compare_with_section_19`]): a key of §19 item 6 that the golden lacks
//! must hold its single-owner value ([`solo_value`]), and every other
//! difference must be item 7's ` (not connected)` mark inserted after a
//! name. The marks are counted per name and compared with
//! [`EXPECTED_MARKS`], so a mark on an author the fixture does not hold
//! unattributed or as a LAN viewer fails; a mark on a comment the owner's
//! public ID wrote fails whatever the counts. Item 1's derived names show
//! only in item 6's `name` keys here, since the fixture's live agent was
//! registered before names existed; items 2 to 5, 8 and 9 change nothing
//! the collection takes and are tested on their own.
//!
//! The golden is regenerated (`write_golden`) only on a rebased base, at
//! the collector's own commit, before any later change: never to make this
//! comparison pass.

use clax_core::Tier;
use clax_core::model::Session;
use clax_core::working::ManualClock;
use clax_mcp::{ClaxTools, DaemonClient};
use clax_testkit::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The fixture's file name in `tests/fixtures/`.
pub const FIXTURE: &str = "single-owner-21.sql";
/// The golden's file name in `tests/fixtures/`.
pub const GOLDEN: &str = "single-owner-21.golden.json";
/// The time the clock stands at while the outputs are collected: a minute
/// after the fixture's last write.
pub const FIXED_T: &str = "2026-09-14T10:00:00Z";
/// Every feedback tier, in the golden's order.
pub const TIERS: [Tier; 6] = [
    Tier::Piggyback,
    Tier::StopHook,
    Tier::PromptHook,
    Tier::Wait,
    Tier::Queue,
    Tier::Inject,
];
/// Longest a client process may run before the collection fails.
const CLIENT_LIMIT: Duration = Duration::from_secs(30);

fn fixtures_dir() -> PathBuf {
    clax_testkit::fixtures_dir()
}

// --- collection ------------------------------------------------------------

/// What the test environment minted for one collection, which
/// [`normalise_minted`] replaces: the daemon's port, the home's path, the
/// PIDs, working directories and executables of the clients run (the MCP
/// tools run in this test process, so `status` names its executable), and
/// the newest timestamp of a ULID in the fixture.
struct Minted {
    port: u16,
    home: String,
    pids: Vec<u32>,
    cwds: Vec<String>,
    ulids_after_ms: u64,
    /// The artifact IDs the collection's creates minted.
    artifact_ids: Vec<String>,
    /// Holds the clients' working directory and `HOME`.
    scratch: tempfile::TempDir,
}

impl Minted {
    fn new(ts: &TestServer) -> Minted {
        let fixture = std::fs::read_to_string(fixtures_dir().join(FIXTURE)).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(scratch.path().join("work")).unwrap();
        let cwd = scratch.path().join("work");
        let mut cwds = vec![cwd.display().to_string()];
        if let Ok(c) = cwd.canonicalize() {
            cwds.push(c.display().to_string());
        }
        Minted {
            port: ts.addr.port(),
            home: ts.home.root().display().to_string(),
            pids: vec![std::process::id()],
            cwds,
            ulids_after_ms: newest_ulid_ms(&fixture),
            artifact_ids: Vec::new(),
            scratch,
        }
    }

    fn value(&self) -> Value {
        let mut homes = vec![self.home.clone()];
        if let Ok(c) = Path::new(&self.home).canonicalize() {
            homes.push(c.display().to_string());
        }
        json!({
            "port": self.port,
            "homes": homes,
            "pids": self.pids,
            "cwds": self.cwds,
            "exes": exes(),
            "ulids_after_ms": self.ulids_after_ms,
            "artifact_ids": self.artifact_ids,
        })
    }

    /// `clax <args>` with a cleared environment pointing at the daemon's
    /// home, run from the scratch working directory.
    fn clax(&self, home: &Path, args: &[&str]) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(clax_fake_exe::clax_bin(None));
        c.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.scratch.path())
            .env("TMPDIR", self.scratch.path())
            .env("CLAX_HOME", home)
            .env("CLAX_NO_OPEN", "1")
            .env("CLAX_CODEX_BIN", "")
            .env("RUST_LOG", "error")
            .current_dir(self.scratch.path().join("work"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        c
    }

    /// Runs `cmd` with `stdin`, recording its PID; its stdout, after
    /// asserting it exited 0 within [`CLIENT_LIMIT`].
    async fn run(&mut self, mut cmd: tokio::process::Command, stdin: &str) -> String {
        use tokio::io::AsyncWriteExt;
        let mut child = cmd.spawn().expect("spawn clax");
        if let Some(pid) = child.id() {
            self.pids.push(pid);
        }
        let mut input = child.stdin.take().unwrap();
        input.write_all(stdin.as_bytes()).await.unwrap();
        drop(input);
        let out = tokio::time::timeout(CLIENT_LIMIT, child.wait_with_output())
            .await
            .expect("the client finished in time")
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

/// This test process's executable, as given and canonical.
fn exes() -> Vec<String> {
    let exe = std::env::current_exe().unwrap();
    let mut out = vec![exe.display().to_string()];
    if let Ok(c) = exe.canonicalize() {
        out.push(c.display().to_string());
    }
    out.dedup();
    out
}

/// Writes the `daemon.json` the CLI and hooks discover the daemon by: this
/// process's PID, the test daemon's port and token.
fn write_daemon_json(ts: &TestServer) {
    let info = clax_core::daemon::DaemonInfo {
        port: ts.addr.port(),
        pid: std::process::id(),
        token: ts.token.clone(),
        started_at: ts.store.now(),
        bind: "127.0.0.1".into(),
        version: "test".into(),
        exe: None,
        commit: None,
    };
    clax_core::daemon::write_daemon_info(&ts.home, &info).unwrap();
}

/// The MCP tools of session `s`, as its shim serves them.
fn tools_for(ts: &TestServer, s: Session) -> ClaxTools {
    ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(s.id.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    )
    .with_opener(clax_mcp::tools::Opener::Program("/usr/bin/true".into()))
}

/// A tool result as JSON: its content (text parsed as JSON where it is
/// JSON) and whether it is an error.
fn tool_value(r: Result<CallToolResult, rmcp::ErrorData>) -> Value {
    let r = match r {
        Ok(r) => r,
        Err(e) => return json!({"protocol_error": e.message}),
    };
    let content: Vec<Value> = r
        .content
        .iter()
        .map(|c| match c.as_text() {
            Some(t) => {
                serde_json::from_str(&t.text).unwrap_or_else(|_| Value::String(t.text.clone()))
            }
            None => serde_json::to_value(c).unwrap(),
        })
        .collect();
    json!({"content": content, "is_error": r.is_error == Some(true)})
}

fn args<T: serde::de::DeserializeOwned>(v: Value) -> Parameters<T> {
    Parameters(serde_json::from_value(v).unwrap())
}

async fn get_json(ts: &TestServer, path: &str) -> Value {
    let res = ts.get_authed(path).await;
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    json!({"status": status, "body": body})
}

/// The fixture's one live session: its ID and harness session ID.
async fn live_session(ts: &TestServer) -> Session {
    let v = get_json(ts, "/api/sessions?live=true").await;
    let live = v["body"]["sessions"].as_array().unwrap();
    assert_eq!(live.len(), 1, "one live agent: {v}");
    serde_json::from_value(live[0].clone()).unwrap()
}

/// The tier's payload, taken through its channel's client.
async fn tier_payload(
    ts: &TestServer,
    tier: Tier,
    sid: &str,
    hsid: &str,
    tools: &ClaxTools,
    m: &mut Minted,
) -> Value {
    let hook = |event: &str| {
        json!({"session_id": hsid, "hook_event_name": event, "stop_hook_active": false,
            "prompt": "Carry on."})
        .to_string()
    };
    let lines = |out: String| -> Value {
        out.lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|_| Value::String(l.into())))
            .collect()
    };
    let home = ts.home.root().to_path_buf();
    match tier {
        Tier::Piggyback => tool_value(tools.list(args(json!({}))).await),
        Tier::Wait => tool_value(tools.wait_for_feedback(args(json!({"timeout_s": 1}))).await),
        Tier::StopHook => {
            let cmd = m.clax(&home, &["hook", "--agent", "claude", "stop"]);
            lines(m.run(cmd, &hook("Stop")).await)
        }
        Tier::PromptHook => {
            let cmd = m.clax(&home, &["hook", "--agent", "claude", "prompt"]);
            lines(m.run(cmd, &hook("UserPromptSubmit")).await)
        }
        Tier::Queue => get_json(ts, &format!("/api/sessions/{sid}/feedback?tier=queue")).await,
        Tier::Inject => get_json(ts, &format!("/api/sessions/{sid}/feedback?tier=inject")).await,
    }
}

/// Every agent-facing output of the daemon `ts` serves, with feedback
/// handed over by `tier`, in this order: the notices `clax feedback follow
/// --once` prints for the live session; the tier's payload; then, through
/// the session's MCP tools, `status`, `list`, the artifact listing, and for
/// each artifact (ascending ID) its detail and threads (the bodies the Pi
/// extension renders), `read` of each version's files, `comments_read` (all
/// threads, then each thread), `db_list` of each collection, and `clax
/// versions`; then the session listing and the working roster; then the
/// rest of the MCP surface, writes included ([`write_script`]). Each tool
/// result keeps the feedback it carried. The value's `minted` key lists
/// what [`normalise_minted`] replaces.
pub async fn collect_agent_outputs(ts: &TestServer, tier: Tier) -> Value {
    write_daemon_json(ts);
    let mut m = Minted::new(ts);
    let session = live_session(ts).await;
    let sid = session.id.clone();
    let hsid = session.harness_session_id.clone().unwrap();
    let home = ts.home.root().to_path_buf();
    let follow = m.clax(
        &home,
        &[
            "feedback",
            "follow",
            "--session",
            &sid,
            "--once",
            "--poll-secs",
            "1",
        ],
    );
    let notices: Vec<String> = m
        .run(follow, "")
        .await
        .lines()
        .map(str::to_string)
        .collect();
    let tools = tools_for(ts, session);
    let payload = tier_payload(ts, tier, &sid, &hsid, &tools, &mut m).await;
    let status = tool_value(tools.status(args(json!({}))).await);
    let list = tool_value(tools.list(args(json!({}))).await);
    let listed = get_json(ts, "/api/artifacts").await;
    let mut aids: Vec<String> = listed["body"]["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    aids.sort();
    let mut artifacts = Map::new();
    for aid in &aids {
        let detail = get_json(ts, &format!("/api/artifacts/{aid}")).await;
        let mut reads = Vec::new();
        for v in detail["body"]["versions"].as_array().unwrap() {
            let n = v["n"].as_u64().unwrap();
            let mut files: Vec<&String> = v["files"].as_object().unwrap().keys().collect();
            files.sort();
            for f in files {
                let r = tools
                    .read(args(json!({"url_or_id": aid, "version": n, "path": f})))
                    .await;
                reads.push(json!({"version": n, "path": f, "result": tool_value(r)}));
            }
        }
        let threads = get_json(
            ts,
            &format!("/api/artifacts/{aid}/threads?include_resolved=true"),
        )
        .await;
        let all = tool_value(
            tools
                .comments_read(args(json!({"url_or_id": aid, "include_resolved": true})))
                .await,
        );
        let mut each = Vec::new();
        let mut tids: Vec<String> = threads["body"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect();
        tids.sort();
        for tid in &tids {
            let r = tools
                .comments_read(args(json!({"url_or_id": aid, "thread_id": tid})))
                .await;
            each.push(json!({"thread_id": tid, "result": tool_value(r)}));
        }
        let mut db = Map::new();
        for c in &collections(ts, aid) {
            let r = tools
                .db_list(args(json!({"url_or_id": aid, "collection": c})))
                .await;
            db.insert(c.clone(), tool_value(r));
        }
        let versions = m.clax(&home, &["--json", "versions", aid]);
        let versions: Value = serde_json::from_str(&m.run(versions, "").await).unwrap();
        artifacts.insert(
            aid.clone(),
            json!({
                "detail": detail,
                "threads": threads,
                "read": reads,
                "comments_read": all,
                "comments_read_each": each,
                "db_list": db,
                "versions": versions,
            }),
        );
    }
    let sessions = get_json(ts, "/api/sessions").await;
    let working = get_json(ts, "/api/working").await;
    let writes = write_script(ts, &tools, &aids, &mut m).await;
    json!({
        "notices": notices,
        "payload": payload,
        "status": status,
        "list": list,
        "listed": listed,
        "artifacts": artifacts,
        "sessions": sessions,
        "working": working,
        "writes": writes,
        "minted": m.value(),
    })
}

/// One row of the fixture's database: the first column of `sql`'s first
/// row, bound to `p`.
fn fixture_id(ts: &TestServer, sql: &str, p: &str) -> String {
    let c = rusqlite::Connection::open_with_flags(
        ts.home.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.query_row(sql, [p], |r| r.get(0))
        .unwrap_or_else(|e| panic!("{sql} with {p}: {e}"))
}

/// The thread whose first comment is `body`.
fn thread_by_text(ts: &TestServer, body: &str) -> String {
    fixture_id(
        ts,
        "SELECT thread_id FROM comments WHERE body = ?1 ORDER BY created_at, id LIMIT 1",
        body,
    )
}

/// After every read, the rest of the MCP surface on the same copy, in this
/// fixed order, each call recorded with its arguments and result:
/// `comments_read` in its default view (open threads) for each artifact;
/// `working` set; `comments_reply` on Mia's thread, then one with
/// `addressed` (refused on an artifact); `comments_resolve` on
/// the lede thread; `watch` of Launch Plan off, then on; `publish` of
/// Quarterly Review v3 (`if_version` 2, `addresses`), a stale `if_version`
/// 1 publish (the conflict), a create without a title (refused), the
/// create of Release Notes and its update to v2; `asset_upload`; `pin`, `unpin` and `open`; the `db_*` tools (`db_get`,
/// `db_query`, `db_set`, `db_update`, a stale `db_update`, `db_str_replace`,
/// `db_batch`, `db_delete`); `ask` on the answered and the declined
/// question, and `ask` cancelling the open one; `working` cleared;
/// `comments_read` of Mia's thread (with the reply); then `delete` of
/// Launch Plan. The artifact IDs the creates minted are recorded in `m`
/// for [`normalise_minted`].
async fn write_script(
    ts: &TestServer,
    tools: &ClaxTools,
    aids: &[String],
    m: &mut Minted,
) -> Value {
    let artifact = |title: &str| fixture_id(ts, "SELECT id FROM artifacts WHERE title = ?1", title);
    let question = |header: &str| {
        fixture_id(
            ts,
            "SELECT id FROM questions WHERE questions_json LIKE ?1",
            &format!("%\"header\":\"{header}\"%"),
        )
    };
    let launch = artifact("Launch Plan");
    let review = artifact("Quarterly Review");
    let tracker = artifact("Team Tracker");
    let mia = thread_by_text(ts, "The Q3 number looks off; finance has 3.9M.");
    let lede = thread_by_text(ts, "Say \"year on year\" here.");
    let asset = m.scratch.path().join("work/chart.svg");
    std::fs::write(
        &asset,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"/>",
    )
    .unwrap();
    let asset = asset.display().to_string();

    let mut out = Vec::new();
    macro_rules! call {
        ($tool:ident, $args:expr) => {{
            let a: Value = $args;
            let r = tool_value(tools.$tool(args(a.clone())).await);
            out.push(json!({"tool": stringify!($tool), "args": a, "result": r.clone()}));
            r
        }};
    }
    for aid in aids {
        call!(comments_read, json!({"url_or_id": aid}));
    }
    call!(
        working,
        json!({"url_or_id": review, "thread_ids": [mia], "message": "Checking the Q3 figures"})
    );
    call!(
        comments_reply,
        json!({"url_or_id": review, "thread_id": mia,
            "text": "Finance is right: Q3 is 3.9M. I will fix the table."})
    );
    call!(
        comments_reply,
        json!({"url_or_id": review, "thread_id": mia, "text": "Fixed.", "addressed": true})
    );
    call!(
        comments_resolve,
        json!({"url_or_id": review, "thread_id": lede})
    );
    call!(watch, json!({"url_or_id": launch, "on": false}));
    call!(watch, json!({"url_or_id": launch, "on": true}));
    call!(
        publish,
        json!({"id": review, "if_version": 2, "html": REVIEW_V3, "note": "Q3 figure fixed",
            "label": "Q3 fix", "addresses": [mia]})
    );
    call!(
        publish,
        json!({"id": review, "if_version": 1, "html": REVIEW_V3, "note": "Stale"})
    );
    call!(publish, json!({"html": "<p>No title</p>"}));
    let created = call!(
        publish,
        json!({"html": NOTES_V1, "description": "What changed in v2", "icon": "notes",
            "files": {"style.css": {"content": "main { max-width: 40rem; }"}}})
    );
    let notes = created["content"][0]["artifact_id"]
        .as_str()
        .expect("the create's artifact ID")
        .to_string();
    call!(
        publish,
        json!({"id": notes, "if_version": 1, "html": NOTES_V2, "note": "Dated the release",
            "files": {"style.css": null}})
    );
    call!(
        asset_upload,
        json!({"url_or_id": review, "file_path": asset})
    );
    call!(pin, json!({"url_or_id": review}));
    call!(unpin, json!({"url_or_id": review}));
    call!(open, json!({"url_or_id": review}));
    call!(
        db_get,
        json!({"url_or_id": tracker, "collection": "tasks", "doc_id": "t1"})
    );
    call!(
        db_query,
        json!({"url_or_id": tracker, "collection": "tasks",
            "query": {"where": [["done", "==", false]], "order_by": {"field": "title"}}})
    );
    call!(
        db_set,
        json!({"url_or_id": tracker, "collection": "tasks", "doc_id": "t4",
            "data": {"title": "Record the demo", "done": false}})
    );
    call!(
        db_update,
        json!({"url_or_id": tracker, "collection": "tasks", "doc_id": "t3",
            "data": {"done": true}, "if_version": 4})
    );
    call!(
        db_update,
        json!({"url_or_id": tracker, "collection": "tasks", "doc_id": "t1",
            "data": {"done": true}, "if_version": 2})
    );
    call!(
        db_str_replace,
        json!({"url_or_id": tracker, "collection": "tasks", "doc_id": "t1",
            "field": "title", "old_str": "v2", "new_str": "version 2", "if_version": 1})
    );
    call!(
        db_batch,
        json!({"url_or_id": tracker, "writes": [
            {"op": "set", "collection": "notes", "doc_id": "n2", "data": {"text": "Bring the slides."}},
            {"op": "update", "collection": "notes", "doc_id": "n1", "data": {"text": "Demo on Friday at 11."}, "if_version": 3}
        ]})
    );
    call!(
        db_delete,
        json!({"url_or_id": tracker, "collection": "tasks", "doc_id": "t2", "if_version": 2})
    );
    call!(
        ask,
        json!({"question_id": question("Chart"), "timeout_s": 1})
    );
    call!(
        ask,
        json!({"question_id": question("Colour"), "timeout_s": 1})
    );
    call!(
        ask,
        json!({"question_id": question("Date"), "cancel": true})
    );
    call!(working, json!({"url_or_id": review, "done": true}));
    call!(
        comments_read,
        json!({"url_or_id": review, "thread_id": mia})
    );
    call!(delete, json!({"url_or_id": launch}));
    m.artifact_ids = artifact_ids(ts)
        .into_iter()
        .filter(|id| !aids.contains(id))
        .collect();
    Value::Array(out)
}

/// Every artifact ID in the daemon's database, deleted ones included, in
/// order.
fn artifact_ids(ts: &TestServer) -> Vec<String> {
    let c = rusqlite::Connection::open_with_flags(
        ts.home.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut st = c.prepare("SELECT id FROM artifacts ORDER BY id").unwrap();
    st.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The collections of `aid`'s page database, in order.
fn collections(ts: &TestServer, aid: &str) -> Vec<String> {
    let c = rusqlite::Connection::open_with_flags(
        ts.home.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut st = c
        .prepare("SELECT DISTINCT collection FROM docs WHERE artifact_id = ?1 ORDER BY collection")
        .unwrap();
    st.query_map([aid], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

// --- normalisation ---------------------------------------------------------

const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The millisecond timestamp of `s` when it is a ULID (26 Crockford base32
/// characters, the first at most `7`).
fn ulid_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 26 || b[0] > b'7' {
        return None;
    }
    let mut ms = 0u64;
    for (i, c) in b.iter().enumerate() {
        let d = CROCKFORD.iter().position(|x| x == c)? as u64;
        if i < 10 {
            ms = ms * 32 + d;
        }
    }
    Some(ms)
}

/// Each maximal run of Crockford base32 characters in `text` that is a
/// ULID, with its byte offset.
fn ulids_in(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if CROCKFORD.contains(&b[i]) {
            let start = i;
            while i < b.len() && CROCKFORD.contains(&b[i]) {
                i += 1;
            }
            if ulid_ms(&text[start..i]).is_some() {
                out.push((start, &text[start..i]));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The newest ULID timestamp in `text`, in milliseconds.
fn newest_ulid_ms(text: &str) -> u64 {
    ulids_in(text)
        .into_iter()
        .filter_map(|(_, u)| ulid_ms(u))
        .max()
        .unwrap_or(0)
}

/// Replaces what was minted during the collection or by the test
/// environment, as recorded under `value["minted"]` (which it removes), each
/// by an explicit rule and nothing else:
///
/// - the value of every key ending in `_at`, and of `since` and `waited_s`,
///   becomes `"<minted>"` (null stays null);
/// - every ULID newer than the fixture's newest becomes `<ulid:N>`, numbered
///   by first appearance, in values and in keys;
/// - every artifact ID the collection minted (a create's random ID) becomes
///   `<artifact:N>`, numbered by first appearance, wherever it appears: in
///   values and keys, inside URLs and paths;
/// - `127.0.0.1:<port>` and `localhost:<port>` lose the test daemon's port
///   (`:<port>`), and the temporary home's path becomes `<home>`;
/// - each client working directory becomes `<cwd>`, each client executable
///   `<exe>`, and a `pid` or `parent_pid` holding a client's PID `"<pid>"`.
pub fn normalise_minted(mut value: Value) -> Value {
    let minted = value
        .as_object_mut()
        .and_then(|o| o.remove("minted"))
        .expect("the collection's minted values");
    let port = minted["port"].as_u64().unwrap();
    let mut texts: Vec<(String, String)> = vec![
        (format!("127.0.0.1:{port}"), "127.0.0.1:<port>".into()),
        (format!("localhost:{port}"), "localhost:<port>".into()),
    ];
    // Longest first, so a path is replaced before any prefix of it.
    let mut paths: Vec<(String, &str)> = Vec::new();
    for h in minted["homes"].as_array().unwrap() {
        paths.push((h.as_str().unwrap().to_string(), "<home>"));
    }
    for c in minted["cwds"].as_array().unwrap() {
        paths.push((c.as_str().unwrap().to_string(), "<cwd>"));
    }
    for e in minted["exes"].as_array().unwrap() {
        paths.push((e.as_str().unwrap().to_string(), "<exe>"));
    }
    paths.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
    texts.extend(paths.into_iter().map(|(p, r)| (p, r.to_string())));
    let pids: Vec<u64> = minted["pids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let mut n = Normaliser {
        texts,
        pids,
        after_ms: minted["ulids_after_ms"].as_u64().unwrap(),
        ulids: Vec::new(),
        minted_artifacts: minted["artifact_ids"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        artifacts: Vec::new(),
    };
    n.value(None, value)
}

struct Normaliser {
    texts: Vec<(String, String)>,
    pids: Vec<u64>,
    after_ms: u64,
    /// Minted ULIDs in order of first appearance.
    ulids: Vec<String>,
    /// The artifact IDs the collection minted.
    minted_artifacts: Vec<String>,
    /// Those of them seen so far, in order of first appearance.
    artifacts: Vec<String>,
}

impl Normaliser {
    fn value(&mut self, key: Option<&str>, v: Value) -> Value {
        if let Some(k) = key
            && (k.ends_with("_at") || k == "since" || k == "waited_s")
            && !v.is_null()
        {
            return Value::String("<minted>".into());
        }
        match v {
            Value::String(s) => Value::String(self.text(&s)),
            Value::Number(x)
                if matches!(key, Some("pid" | "parent_pid"))
                    && x.as_u64().is_some_and(|p| self.pids.contains(&p)) =>
            {
                Value::String("<pid>".into())
            }
            Value::Array(a) => Value::Array(a.into_iter().map(|x| self.value(None, x)).collect()),
            Value::Object(o) => Value::Object(
                o.into_iter()
                    .map(|(k, x)| {
                        let x = self.value(Some(&k), x);
                        (self.text(&k), x)
                    })
                    .collect(),
            ),
            other => other,
        }
    }

    /// `s` with each minted artifact ID replaced by `<artifact:N>`, numbered
    /// by first appearance.
    fn artifact_ids(&mut self, s: String) -> String {
        let mut found: Vec<(usize, usize)> = Vec::new();
        for (i, id) in self.minted_artifacts.iter().enumerate() {
            found.extend(s.match_indices(id.as_str()).map(|(at, _)| (at, i)));
        }
        if found.is_empty() {
            return s;
        }
        found.sort();
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        for (at, i) in found {
            let id = &self.minted_artifacts[i];
            let n = match self.artifacts.iter().position(|x| x == id) {
                Some(k) => k + 1,
                None => {
                    self.artifacts.push(id.clone());
                    self.artifacts.len()
                }
            };
            out.push_str(&s[last..at]);
            out.push_str(&format!("<artifact:{n}>"));
            last = at + id.len();
        }
        out.push_str(&s[last..]);
        out
    }

    fn text(&mut self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        for (at, u) in ulids_in(s) {
            if ulid_ms(u).is_some_and(|ms| ms > self.after_ms) {
                let n = match self.ulids.iter().position(|x| x == u) {
                    Some(i) => i + 1,
                    None => {
                        self.ulids.push(u.to_string());
                        self.ulids.len()
                    }
                };
                out.push_str(&s[last..at]);
                out.push_str(&format!("<ulid:{n}>"));
                last = at + u.len();
            }
        }
        out.push_str(&s[last..]);
        out = self.artifact_ids(out);
        for (from, to) in &self.texts {
            if out.contains(from.as_str()) {
                out = out.replace(from.as_str(), to);
            }
        }
        out
    }
}

/// The golden as the collection gives it now: one entry per tier, each on a
/// fresh copy of the fixture under the fixed clock.
async fn golden_now() -> Value {
    let mut out = Map::new();
    for tier in TIERS {
        let ts = TestServer::from_fixture_with_clock(FIXTURE, FIXED_T).await;
        let got = normalise_minted(collect_agent_outputs(&ts, tier).await);
        out.insert(tier.as_str().to_string(), got);
    }
    Value::Object(out)
}

/// §19 item 6's additive keys: where the golden lacks one, its value now
/// must be the single-owner value [`solo_value`] accepts.
pub const ADDITIVE_KEYS: [&str; 7] = [
    "owner",
    "taken_by",
    "name",
    "owner_name",
    "person",
    "withheld",
    "rights",
];

/// Whether `v`, the value of item 6's key `key` that the golden lacks, is
/// what a single-owner install shows (spec §19 item 6): `withheld` absent or
/// null; `name`, and an artifact's `owner_name`, null or a name; `person` null or the owner (`owner: true`,
/// the owner's public ID); a thread's `owner` or `taken_by` null or one of
/// the owner's agents (`person` null); `rights` the owner's, `clax:full`.
pub fn solo_value(key: &str, v: &Value, owner: &str) -> bool {
    let owners_agent = |a: &Value| {
        a.is_null()
            || (a["person"].is_null()
                && a["handle"].as_str().is_some_and(|h| h.starts_with("a_"))
                && a.get("person_name").is_none_or(Value::is_string)
                && (a["name"].is_null() || a["name"].is_string()))
    };
    match key {
        "withheld" => v.is_null(),
        "name" | "owner_name" => v.is_null() || v.as_str().is_some_and(|n| !n.is_empty()),
        "person" => {
            v.is_null() || (v["owner"] == json!(true) && v["public_id"].as_str() == Some(owner))
        }
        "owner" | "taken_by" => owners_agent(v),
        "rights" => *v == clax_core::rights::Rights::full().to_json(),
        _ => false,
    }
}

/// §19 item 7's mark.
const NOT_CONNECTED: &str = " (not connected)";

/// The ` (not connected)` marks the fixture's outputs carry, by the name
/// marked: its two LAN viewers (`Mia`, and the unnamed `Viewer`), and the
/// owner's unattributed history (`Alex`: the comments from before
/// migration 12 and the batch from before migration 25, spec §9.6).
pub const EXPECTED_MARKS: [(&str, usize); 3] = [("Alex", 61), ("Mia", 35), ("Viewer", 35)];

/// What [`compare_with_section_19`] found.
#[derive(Debug, Default, PartialEq)]
pub struct Section19 {
    /// Each name marked ` (not connected)`, with how many times.
    pub marks: std::collections::BTreeMap<String, usize>,
    /// The marks on a comment whose `author_public_id` is the owner's.
    pub on_owner: Vec<String>,
    /// Every other difference: its path and both values.
    pub other: Vec<String>,
}

/// Compares `now` with the `committed` golden as §19 allows: an item 6 key
/// only `now` has must hold its single-owner value ([`solo_value`]), item
/// 7's marks are counted per name (the word before each), and anything else
/// is listed in [`Section19::other`]. A mark inside an object whose
/// `author_public_id` is `owner` is listed in [`Section19::on_owner`].
pub fn compare_with_section_19(committed: &Value, now: &Value, owner: &str) -> Section19 {
    fn walk(a: &Value, b: &Value, path: &str, owned: bool, owner: &str, out: &mut Section19) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let owned =
                    owned || y.get("author_public_id").and_then(Value::as_str) == Some(owner);
                let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
                for k in keys {
                    match (x.get(k), y.get(k)) {
                        (Some(p), Some(q)) => walk(p, q, &format!("{path}/{k}"), owned, owner, out),
                        (None, Some(q)) if ADDITIVE_KEYS.contains(&k.as_str()) => {
                            if !solo_value(k, q, owner) {
                                out.other
                                    .push(format!("{path}/{k}: not a single owner's: {q}"));
                            }
                        }
                        _ => out.other.push(format!("{path}/{k}: on one side only")),
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(p, q, &format!("{path}[{i}]"), owned, owner, out);
                }
            }
            (Value::String(x), Value::String(y))
                if x != y && y.replace(NOT_CONNECTED, "") == *x =>
            {
                for (at, _) in y.match_indices(NOT_CONNECTED) {
                    let name: String = y[..at]
                        .chars()
                        .rev()
                        .take_while(|c| !c.is_whitespace() && !"\"':(".contains(*c))
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    *out.marks.entry(name).or_default() += 1;
                    if owned {
                        out.on_owner.push(path.to_string());
                    }
                }
            }
            (x, y) if x != y => out.other.push(format!("{path}: {x} != {y}")),
            _ => {}
        }
    }
    let mut out = Section19::default();
    walk(committed, now, "", false, owner, &mut out);
    out
}

#[test]
fn the_section_19_comparison_counts_marks_per_name_and_flags_the_owners() {
    let committed = json!({"comments": [
        {"author_public_id": null, "author_name": "Alex", "body": "x"},
        {"author_public_id": "u_lan", "author_name": "Mia", "body": "y"},
        {"author_public_id": "u_owner", "author_name": "Alex", "body": "z"},
    ], "text": "Mia: \"y\"\nsent together by Alex. Note"});
    let mut now = committed.clone();
    now["comments"][0]["author_name"] = json!("Alex (not connected)");
    now["comments"][1]["author_name"] = json!("Mia (not connected)");
    now["comments"][1]["owner"] =
        json!({"handle": "a_1", "live": true, "name": null, "person": null, "person_name": "Alex"});
    now["comments"][1]["withheld"] = Value::Null;
    now["person"] = json!({"name": "Alex", "owner": true, "public_id": "u_owner"});
    now["text"] = json!("Mia (not connected): \"y\"\nsent together by Alex (not connected). Note");
    let got = compare_with_section_19(&committed, &now, "u_owner");
    assert_eq!(
        got.marks.into_iter().collect::<Vec<_>>(),
        [("Alex".to_string(), 2), ("Mia".to_string(), 2)]
    );
    assert!(got.on_owner.is_empty() && got.other.is_empty());
    // A mark on the owner's attributed comment, and any other change.
    now["comments"][2]["author_name"] = json!("Alex (not connected)");
    now["comments"][2]["body"] = json!("changed");
    let got = compare_with_section_19(&committed, &now, "u_owner");
    assert_eq!(got.on_owner, ["/comments[2]/author_name"]);
    assert_eq!(got.other.len(), 1, "{:?}", got.other);
    // Item 6's keys with values no single owner has: another person's
    // agent, withheld content, another person.
    now["comments"][2]["body"] = json!("z");
    now["comments"][1]["owner"]["person"] = json!("u_sam");
    now["comments"][1]["withheld"] = json!({"count": 1});
    now["person"]["public_id"] = json!("u_sam");
    let got = compare_with_section_19(&committed, &now, "u_owner");
    assert_eq!(
        got.other,
        [
            "/comments[1]/owner: not a single owner's: {\"handle\":\"a_1\",\"live\":true,\"name\":null,\"person\":\"u_sam\",\"person_name\":\"Alex\"}",
            "/comments[1]/withheld: not a single owner's: {\"count\":1}",
            "/person: not a single owner's: {\"name\":\"Alex\",\"owner\":true,\"public_id\":\"u_sam\"}",
        ]
    );
}

/// The golden check (module docs): the outputs now differ from the
/// committed golden only as §19 lists, item 6's keys with their
/// single-owner values, exactly [`EXPECTED_MARKS`]'s marks and none on the
/// owner's attributed comments.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_golden_differs_only_as_section_19_lists() {
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(fixtures_dir().join(GOLDEN)).unwrap())
            .unwrap();
    let owner = {
        let ts = TestServer::from_fixture_with_clock(FIXTURE, FIXED_T).await;
        ts.store
            .owner()
            .unwrap()
            .expect("the fixture's owner")
            .public_id
    };
    let got = compare_with_section_19(&committed, &golden_now().await, &owner);
    assert!(
        got.other.is_empty(),
        "differences §19 does not list: {:#?}",
        got.other
    );
    assert!(
        got.on_owner.is_empty(),
        "marks on the owner's own comments: {:?}",
        got.on_owner
    );
    let want: std::collections::BTreeMap<String, usize> = EXPECTED_MARKS
        .iter()
        .map(|(n, c)| (n.to_string(), *c))
        .collect();
    assert_eq!(got.marks, want, "(not connected) marks per name");
}

/// Writes the golden. Run by hand, only where the plan allows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "writes tests/fixtures/single-owner-21.golden.json"]
async fn write_golden() {
    let golden = golden_now().await;
    let mut text = serde_json::to_string_pretty(&golden).unwrap();
    text.push('\n');
    std::fs::write(fixtures_dir().join(GOLDEN), text).unwrap();
}

#[test]
fn normalise_minted_replaces_only_the_listed_values() {
    // `old` is the fixture's newest ULID; `new_a` and `new_b` are later. Keys
    // are visited in order, so the key `new_a` is numbered first.
    let old = "01K5A3B4C5D6E7F8G9H0J1K2M3";
    let new_a = "01K9ZZZZZZD6E7F8G9H0J1K2M3";
    let new_b = "01K9ZZZZZZD6E7F8G9H0J1K2M4";
    let v = json!({
        "minted": {
            "port": 51234,
            "homes": ["/tmp/x/ax"],
            "pids": [77],
            "cwds": ["/tmp/s/work"],
            "exes": ["/tmp/t/deps/golden-1"],
            "ulids_after_ms": ulid_ms(old).unwrap(),
            "artifact_ids": ["zzzzyyyyxxxx", "ccccdddd0000"],
        },
        "a_made": "http://localhost:51234/a/zzzzyyyyxxxx/v/2",
        "a_other": "ccccdddd0000 then zzzzyyyyxxxx, not abcdabcdabcd",
        "ccccdddd0000": "/tmp/x/ax/artifacts/ccccdddd0000/index.html",
        "created_at": "2026-09-14T10:00:00.000Z",
        "ended_at": null,
        "since": "x",
        "waited_s": 3,
        "url": "http://localhost:51234/a/abc",
        "self": "http://127.0.0.1:51234/api",
        "other_port": "http://localhost:5123/",
        "clip_path": "/tmp/x/ax/artifacts/abc/clips/t.png",
        "cwd": "/tmp/s/work",
        "binary": {"path": "/tmp/t/deps/golden-1"},
        "pid": 77,
        "parent_pid": 4200,
        "ids": [old, new_a, new_b, new_a],
        "text": format!("thread {new_b} and {old}"),
        new_a: 1,
    });
    assert_eq!(
        normalise_minted(v),
        json!({
            "a_made": "http://localhost:<port>/a/<artifact:1>/v/2",
            "a_other": "<artifact:2> then <artifact:1>, not abcdabcdabcd",
            "<artifact:2>": "<home>/artifacts/<artifact:2>/index.html",
            "created_at": "<minted>",
            "ended_at": null,
            "since": "<minted>",
            "waited_s": "<minted>",
            "url": "http://localhost:<port>/a/abc",
            "self": "http://127.0.0.1:<port>/api",
            "other_port": "http://localhost:5123/",
            "clip_path": "<home>/artifacts/abc/clips/t.png",
            "cwd": "<cwd>",
            "binary": {"path": "<exe>"},
            "pid": "<pid>",
            "parent_pid": 4200,
            "ids": [old, "<ulid:1>", "<ulid:2>", "<ulid:1>"],
            "text": format!("thread <ulid:2> and {old}"),
            "<ulid:1>": 1,
        })
    );
}

#[tokio::test]
async fn the_fixture_is_a_single_owner_install_at_migration_21() {
    let ts = TestServer::from_fixture_with_clock(FIXTURE, FIXED_T).await;
    let sid = live_session(&ts).await.id;
    let v = get_json(&ts, &format!("/api/sessions/{sid}")).await;
    assert_eq!(v["status"], 200, "{v}");
    let sql = std::fs::read_to_string(fixtures_dir().join(FIXTURE)).unwrap();
    assert!(sql.contains("PRAGMA user_version = 21;"));
    assert!(
        ts.store.integrity_check().unwrap() == "ok",
        "the fixture loads into a sound database"
    );
    // The fixed clock is the store's time.
    assert_eq!(ts.store.now(), "2026-09-14T10:00:00.000Z");
}

// --- the fixture -----------------------------------------------------------

/// When the fixture's writes start.
const FIXTURE_T0: &str = "2026-09-14T09:00:00Z";

/// SQL literal of one value.
fn literal(v: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(r) => format!("{r:?}"),
        ValueRef::Text(t) => {
            let s = std::str::from_utf8(t).expect("UTF-8 text");
            format!("'{}'", s.replace('\'', "''"))
        }
        ValueRef::Blob(b) => blob(b),
    }
}

fn blob(b: &[u8]) -> String {
    let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("X'{hex}'")
}

/// The database at `home`'s path, and every file under its `artifacts/`,
/// as one SQL script for `load_fixture`: the schema in creation order
/// (FTS5 shadow tables made by their virtual table), every row, the
/// `user_version`, and the `fixture_files` table.
fn dump(home: &clax_core::Home) -> String {
    let c = rusqlite::Connection::open_with_flags(
        home.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let shadow: Vec<String> = {
        let mut st = c
            .prepare("SELECT name FROM pragma_table_list WHERE type = 'shadow'")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let objects: Vec<(String, String, String)> = {
        let mut st = c
            .prepare(
                "SELECT type, name, sql FROM sqlite_master
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY rowid",
            )
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let version: u32 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let mut out = String::from(
        "-- The single-owner golden's fixture: a migration-21 Clax database and\n\
         -- the files its rows name. Written by golden_single_owner.rs's\n\
         -- write_fixture; loaded by clax_testkit::load_fixture.\n\
         PRAGMA foreign_keys = OFF;\nBEGIN;\n",
    );
    for (_, name, sql) in &objects {
        if !shadow.contains(name) {
            out.push_str(sql);
            out.push_str(";\n");
        }
    }
    let mut tables: Vec<String> = objects
        .iter()
        .filter(|(t, _, sql)| t == "table" && !sql.starts_with("CREATE VIRTUAL"))
        .map(|(_, n, _)| n.clone())
        .collect();
    let has_sequence: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'sqlite_sequence')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if has_sequence {
        tables.push("sqlite_sequence".into());
    }
    for t in &tables {
        let without_rowid = objects
            .iter()
            .any(|(_, n, sql)| n == t && sql.contains("WITHOUT ROWID"));
        let st = c.prepare(&format!("SELECT * FROM \"{t}\"")).unwrap();
        let cols: Vec<String> = st.column_names().into_iter().map(String::from).collect();
        let order = if without_rowid || t == "sqlite_sequence" {
            (1..=cols.len())
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            "rowid".into()
        };
        drop(st);
        let mut st = c
            .prepare(&format!("SELECT * FROM \"{t}\" ORDER BY {order}"))
            .unwrap();
        if shadow.contains(t) || t == "sqlite_sequence" {
            out.push_str(&format!("DELETE FROM \"{t}\";\n"));
        }
        let names = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let vals: Vec<String> = (0..cols.len())
                .map(|i| literal(r.get_ref(i).unwrap()))
                .collect();
            out.push_str(&format!(
                "INSERT INTO \"{t}\" ({names}) VALUES ({});\n",
                vals.join(", ")
            ));
        }
    }
    out.push_str(&format!("PRAGMA user_version = {version};\n"));
    out.push_str("CREATE TABLE fixture_files (path TEXT PRIMARY KEY, content BLOB NOT NULL);\n");
    let mut files = Vec::new();
    collect_files(home.root(), &home.root().join("artifacts"), &mut files);
    files.sort();
    for (rel, bytes) in files {
        out.push_str(&format!(
            "INSERT INTO fixture_files (path, content) VALUES ('{}', {});\n",
            rel.replace('\'', "''"),
            blob(&bytes)
        ));
    }
    out.push_str("COMMIT;\n");
    out
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries {
        let p = e.unwrap().path();
        if p.is_dir() {
            collect_files(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            out.push((rel, std::fs::read(&p).unwrap()));
        }
    }
}

/// Requests of the fixture's actors against one test daemon.
struct Actors<'a> {
    ts: &'a TestServer,
    clock: Arc<ManualClock>,
    lan: reqwest::Client,
}

impl Actors<'_> {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.ts.base)
    }

    /// Sends `r`, asserts status `want`, and returns the JSON body (null when empty).
    async fn send(&self, r: reqwest::RequestBuilder, want: u16) -> Value {
        let res = r.send().await.unwrap();
        let status = res.status().as_u16();
        let text = res.text().await.unwrap();
        assert_eq!(status, want, "{text}");
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    fn owner(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        r.header("cookie", self.ts.owner_cookie())
    }

    fn agent(&self, r: reqwest::RequestBuilder, sid: &str) -> reqwest::RequestBuilder {
        r.bearer_auth(&self.ts.token).header("x-clax-session", sid)
    }

    fn tick(&self, secs: i64) {
        self.clock.advance(secs);
    }

    async fn register(&self, harness: &str, hsid: &str, cwd: &str, pid: u32) -> String {
        let v = self
            .send(
                self.ts
                    .authed(self.ts.client.post(self.url("/api/sessions")))
                    .json(&json!({
                        "harness": harness, "harness_session_id": hsid, "cwd": cwd,
                        "pid": pid, "parent_pid": pid - 1,
                    })),
                201,
            )
            .await;
        v["session"]["id"].as_str().unwrap().to_string()
    }

    async fn publish(&self, sid: &str, body: Value) -> String {
        let v = self
            .send(
                self.agent(self.ts.client.post(self.url("/api/artifacts")), sid)
                    .json(&body),
                201,
            )
            .await;
        v["artifact"]["id"].as_str().unwrap().to_string()
    }

    /// A LAN viewer's cookie, named `name` when given.
    async fn lan_viewer(&self, name: Option<&str>) -> String {
        let res = self
            .lan
            .get(self.url("/api/viewers/me"))
            .send()
            .await
            .unwrap();
        let set = res.headers()["set-cookie"].to_str().unwrap().to_string();
        let cookie = set
            .split(';')
            .next()
            .and_then(|kv| kv.strip_prefix("clax_viewer="))
            .unwrap()
            .to_string();
        if let Some(n) = name {
            self.send(
                self.lan
                    .put(self.url("/api/viewers/me"))
                    .header("cookie", format!("clax_viewer={cookie}"))
                    .json(&json!({"display_name": n})),
                200,
            )
            .await;
        }
        cookie
    }

    /// A thread on `aid` anchored to `quote` in `selector`.
    #[allow(clippy::too_many_arguments)]
    async fn thread(
        &self,
        r: reqwest::RequestBuilder,
        version: u32,
        selector: &str,
        quote: &str,
        body: &str,
        clip: Option<&[u8]>,
        via_page: bool,
    ) -> String {
        let anchor = json!({"kind": "element", "selector": selector, "quote": quote,
            "prefix": "", "suffix": "", "html_hash": "sha256:00", "rect": null, "custom_name": null});
        let mut form = reqwest::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", body.to_string())
            .text("version", version.to_string());
        if via_page {
            form = form.text("via_page", "true");
        }
        if let Some(bytes) = clip {
            form = form.part(
                "clip",
                reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        }
        let v = self.send(r.multipart(form), 201).await;
        v["thread"]["id"].as_str().unwrap().to_string()
    }

    fn threads_url(&self, aid: &str) -> String {
        self.url(&format!("/api/artifacts/{aid}/threads"))
    }

    async fn reply(&self, r: reqwest::RequestBuilder, body: Value) {
        self.send(r.json(&body), 201).await;
    }

    async fn send_thread(&self, aid: &str, tid: &str) {
        self.send(
            self.owner(
                self.ts
                    .client
                    .post(self.url(&format!("/api/artifacts/{aid}/threads/{tid}/send"))),
            ),
            200,
        )
        .await;
    }

    async fn take(&self, sid: &str, tier: &str) -> Value {
        self.send(
            self.ts.authed(
                self.ts
                    .client
                    .get(self.url(&format!("/api/sessions/{sid}/feedback?tier={tier}"))),
            ),
            200,
        )
        .await
    }
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nclip-of-the-header";

const REVIEW_V1: &str = "<!doctype html><html><head><title>Quarterly Review</title>\
<link rel=stylesheet href=style.css></head><body><main><h2>Quarterly goals</h2>\
<p class=lede>Revenue grew 12% on Q2.</p><table><tr><td>Q3</td><td>4.1M</td></tr></table>\
</main></body></html>";
/// Release Notes v1 and v2, created and updated by the collector's write
/// script.
const NOTES_V1: &str = "<!doctype html><html><head><title>Release Notes</title>\
<link rel=stylesheet href=style.css></head><body><main><h1>Release Notes</h1>\
<p>Version 2 ships the sticky header.</p></main></body></html>";
const NOTES_V2: &str = "<!doctype html><html><head><title>Release Notes</title></head>\
<body><main><h1>Release Notes</h1><p>Version 2 ships the sticky header on 1 October.</p>\
</main></body></html>";
/// Quarterly Review v3, published by the collector's write script.
const REVIEW_V3: &str = "<!doctype html><html><head><title>Quarterly Review</title>\
<link rel=stylesheet href=style.css></head><body><header class=sticky>Quarterly Review</header>\
<main><h2>Quarterly goals</h2><p class=lede>Revenue grew 12% on Q2.</p>\
<table><tr><td>Q3</td><td>3.9M</td></tr></table></main></body></html>";
const REVIEW_V2: &str = "<!doctype html><html><head><title>Quarterly Review</title>\
<link rel=stylesheet href=style.css></head><body><header class=sticky>Quarterly Review</header>\
<main><h2>Quarterly goals</h2><p class=lede>Revenue grew 12% on Q2.</p>\
<table><tr><td>Q3</td><td>4.1M</td></tr></table></main></body></html>";
const TRACKER: &str = "<!doctype html><html><head><title>Team Tracker</title></head>\
<body><main><h2>Tasks</h2><ul id=tasks></ul></main></body></html>";
const LAUNCH: &str = "<!doctype html><html><head><title>Launch Plan</title></head>\
<body><main><h2>Launch steps</h2><ol><li>Freeze</li><li>Ship</li></ol></main></body></html>";

/// Builds the fixture through the daemon's routes under a clock that
/// starts at [`FIXTURE_T0`] and ends before [`FIXED_T`], then writes it.
/// Its rows: an ended Codex session whose artifact holds unattributed
/// comments (as migration 12 left comments made before it) and feedback
/// released when it ended; the one live Claude Code session with two
/// artifacts it published (one with a page database) and a watch with
/// replies off on the Codex artifact; owner threads with a clip, a reply,
/// a second version and a resolve; a named and an anonymous LAN viewer;
/// comments and documents the page wrote from the owner's browser;
/// feedback acknowledged, delivered and due a resend, sent and notified,
/// sent, sent in a batch, and on the unarmed watch; questions answered,
/// declined and open; and inbox items, one read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "writes tests/fixtures/single-owner-21.sql; run only at a migration-21 schema"]
async fn write_fixture() {
    assert_eq!(
        clax_core::store::migrations::MIGRATIONS.len(),
        21,
        "the fixture is a migration-21 database"
    );
    let clock = Arc::new(ManualClock::at(FIXTURE_T0));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let a = Actors {
        ts: &ts,
        clock: clock.clone(),
        lan: ts.lan().0,
    };
    let c = &ts.client;
    // The owner opens the shell (the owner identity).
    a.send(a.owner(c.get(a.url("/api/viewers/me"))), 200).await;
    a.send(
        a.owner(c.put(a.url("/api/viewers/me")))
            .json(&json!({"display_name": "Alex"})),
        200,
    )
    .await;
    a.tick(60);

    // An earlier Codex session and its artifact, with comments from before
    // participants were recorded.
    let codex = a
        .register("codex", "codex-thread-1", "/work/site", 51000)
        .await;
    let launch = a
        .publish(
            &codex,
            json!({"title": "Launch Plan", "description": "Steps to ship v2",
                "files": {"index.html": {"content": LAUNCH, "encoding": "utf8"}}}),
        )
        .await;
    a.tick(60);
    let old1 = a
        .thread(
            a.owner(c.post(a.threads_url(&launch))),
            1,
            "body > main > h2",
            "Launch steps",
            "Add a rollback step.",
            None,
            false,
        )
        .await;
    a.send_thread(&launch, &old1).await;
    a.take(&codex, "piggyback").await;
    a.tick(30);
    a.reply(
        a.agent(
            c.post(a.url(&format!("/api/artifacts/{launch}/threads/{old1}/comments"))),
            &codex,
        ),
        json!({"body": "Added a rollback step after Ship.", "author_kind": "agent"}),
    )
    .await;
    a.tick(30);
    let old2 = a
        .thread(
            a.owner(c.post(a.threads_url(&launch))),
            1,
            "body > main > ol > li:nth-child(1)",
            "Freeze",
            "Freeze on Thursday, not Friday.",
            None,
            false,
        )
        .await;
    a.tick(30);

    // Sent while the Codex session is the artifact's only agent.
    let unsent = a
        .thread(
            a.owner(c.post(a.threads_url(&launch))),
            1,
            "body > main > ol",
            "Ship",
            "Name the release owner.",
            None,
            false,
        )
        .await;
    a.send_thread(&launch, &unsent).await;
    a.tick(10);

    // The live Claude Code session and its artifacts.
    let live = a.register("claude", "golden-live", "/work/web", 4242).await;
    let review = a
        .publish(
            &live,
            json!({"title": "Quarterly Review", "description": "Q3 numbers for the board",
                "files": {"index.html": {"content": REVIEW_V1, "encoding": "utf8"},
                          "style.css": {"content": "main { max-width: 60rem; }", "encoding": "utf8"}}}),
        )
        .await;
    a.tick(30);
    let tracker = a
        .publish(
            &live,
            json!({"title": "Team Tracker", "capabilities": {"db": {}, "comments": {}},
                "files": {"index.html": {"content": TRACKER, "encoding": "utf8"}}}),
        )
        .await;
    a.tick(30);
    a.send(
        a.ts.authed(c.put(a.url(&format!("/api/sessions/{live}/watches/{launch}"))))
            .json(&json!({"replies_armed": false})),
        200,
    )
    .await;
    a.tick(30);

    // An owner thread with a clip: sent, taken in band (acknowledged),
    // replied to, addressed by version 2 and resolved by the agent.
    let header = a
        .thread(
            a.owner(c.post(a.threads_url(&review))),
            1,
            "body > main > h2",
            "Quarterly goals",
            "Make the header sticky.",
            Some(PNG),
            false,
        )
        .await;
    a.send_thread(&review, &header).await;
    a.tick(20);
    a.take(&live, "piggyback").await;
    a.tick(40);
    a.reply(
        a.agent(
            c.post(a.url(&format!(
                "/api/artifacts/{review}/threads/{header}/comments"
            ))),
            &live,
        ),
        json!({"body": "Done: the header now sticks to the top.", "author_kind": "agent"}),
    )
    .await;
    a.tick(20);
    a.send(
        a.agent(
            c.post(a.url(&format!("/api/artifacts/{review}/versions"))),
            &live,
        )
        .json(&json!({"if_version": 1, "note": "Sticky header", "addresses": [header],
            "files": {"index.html": {"content": REVIEW_V2, "encoding": "utf8"},
                      "style.css": {"content": "main { max-width: 60rem; } .sticky { position: sticky; top: 0; }", "encoding": "utf8"}}})),
        201,
    )
    .await;
    a.tick(20);
    a.send(
        a.agent(
            c.post(a.url(&format!("/api/artifacts/{review}/threads/{header}/resolve"))),
            &live,
        )
        .json(&json!({"as": "agent"})),
        200,
    )
    .await;
    a.tick(60);

    // LAN viewers: Mia (named) opens a thread, an anonymous viewer replies;
    // the owner sends it and the prompt hook delivers it, unacknowledged,
    // long enough ago to be resent.
    let mia = a.lan_viewer(Some("Mia")).await;
    let anon = a.lan_viewer(None).await;
    let numbers = a
        .thread(
            a.lan
                .post(a.threads_url(&review))
                .header("cookie", format!("clax_viewer={mia}")),
            2,
            "body > main > table",
            "Q3",
            "The Q3 number looks off; finance has 3.9M.",
            None,
            false,
        )
        .await;
    a.tick(30);
    a.reply(
        a.lan
            .post(a.url(&format!(
                "/api/artifacts/{review}/threads/{numbers}/comments"
            )))
            .header("cookie", format!("clax_viewer={anon}")),
        json!({"body": "Agreed, and Q2 too."}),
    )
    .await;
    a.tick(30);
    a.send_thread(&review, &numbers).await;
    a.tick(10);
    a.take(&live, "prompt_hook").await;
    a.tick(300);

    // Sent and announced by a notice; then sent without one.
    let blue = a
        .thread(
            a.owner(c.post(a.threads_url(&review))),
            2,
            "body > header",
            "Quarterly Review",
            "Use the brand blue for the header.",
            None,
            false,
        )
        .await;
    a.send_thread(&review, &blue).await;
    a.send(
        a.ts.authed(c.get(a.url(&format!("/api/sessions/{live}/notices?wait=0")))),
        200,
    )
    .await;
    a.tick(30);
    let lede = a
        .thread(
            a.owner(c.post(a.threads_url(&review))),
            2,
            "body > main > p.lede",
            "Revenue grew 12% on Q2.",
            "Say \"year on year\" here.",
            None,
            false,
        )
        .await;
    a.send_thread(&review, &lede).await;
    a.tick(30);

    // The page writes comments and documents from the owner's browser, and
    // two owner threads go in one batch with a note.
    let page_thread = a
        .thread(
            a.owner(c.post(a.threads_url(&tracker))),
            1,
            "body > main > h2",
            "Tasks",
            "Task \"Ship v2\" is blocked on review.",
            None,
            true,
        )
        .await;
    a.reply(
        a.owner(c.post(a.url(&format!(
            "/api/artifacts/{tracker}/threads/{page_thread}/comments"
        )))),
        json!({"body": "Unblocked: review done.", "via_page": true}),
    )
    .await;
    a.send_thread(&tracker, &page_thread).await;
    for (doc, data) in [
        (
            "tasks/t1",
            json!({"title": "Ship v2", "done": false, "owner": "Alex"}),
        ),
        (
            "tasks/t2",
            json!({"title": "Write the changelog", "done": true}),
        ),
        ("notes/n1", json!({"text": "Demo on Friday at 10."})),
    ] {
        a.send(
            a.owner(c.put(a.url(&format!("/api/artifacts/{tracker}/docs/{doc}"))))
                .json(&json!({"data": data})),
            200,
        )
        .await;
        a.tick(5);
    }
    a.send(
        a.agent(
            c.put(a.url(&format!("/api/artifacts/{tracker}/docs/tasks/t3"))),
            &live,
        )
        .json(&json!({"data": {"title": "Add filters", "done": false}})),
        200,
    )
    .await;
    a.tick(20);
    let b1 = a
        .thread(
            a.owner(c.post(a.threads_url(&tracker))),
            1,
            "body > main > h2",
            "Tasks",
            "Group tasks by owner.",
            None,
            false,
        )
        .await;
    let b2 = a
        .thread(
            a.owner(c.post(a.threads_url(&tracker))),
            1,
            "#tasks",
            "",
            "Show done tasks struck through.",
            None,
            false,
        )
        .await;
    a.send(
        a.owner(c.post(a.url(&format!("/api/artifacts/{tracker}/threads:send"))))
            .json(&json!({"thread_ids": [b1, b2], "note": "Before the demo"})),
        200,
    )
    .await;
    a.tick(20);

    // A thread on the Codex artifact reaches the live session through its
    // watch with replies off.
    a.send_thread(&launch, &old2).await;
    a.tick(20);

    // Questions: answered (its answer not yet received), declined, open.
    for (header_text, outcome) in [("Chart", "answer"), ("Colour", "decline"), ("Date", "open")] {
        let q = a
            .send(
                a.ts.authed(c.post(a.url(&format!("/api/sessions/{live}/questions"))))
                    .json(
                        &json!({"source": "ask", "artifact_id": review, "questions": [{
                        "question": format!("Which {header_text} should the review use?"),
                        "header": header_text,
                        "options": [{"label": "First", "description": "the first one"},
                                    {"label": "Second"}]}]}),
                    ),
                201,
            )
            .await;
        let qid = q["question"]["id"].as_str().unwrap().to_string();
        a.tick(15);
        match outcome {
            "answer" => {
                a.send(
                    a.owner(c.post(a.url(&format!("/api/questions/{qid}/answer"))))
                        .json(&json!({"answers": [{"selected": ["Second"]}]})),
                    200,
                )
                .await;
            }
            "decline" => {
                a.send(
                    a.owner(c.post(a.url(&format!("/api/questions/{qid}/decline"))))
                        .json(&json!({})),
                    200,
                )
                .await;
            }
            _ => {}
        }
        a.tick(15);
    }

    // The Codex session ends: what it had not taken has no other target
    // and is released.
    a.send(
        a.ts.authed(c.patch(a.url(&format!("/api/sessions/{codex}"))))
            .json(&json!({"ended": true})),
        200,
    )
    .await;
    a.tick(10);

    // The owner reads the oldest inbox item.
    let inbox = a.send(a.owner(c.get(a.url("/api/inbox"))), 200).await;
    let items = inbox["items"].as_array().unwrap();
    let oldest = items.last().unwrap()["id"].as_str().unwrap().to_string();
    a.send(
        a.owner(c.post(a.url(&format!("/api/inbox/{oldest}/read"))))
            .json(&json!({})),
        200,
    )
    .await;

    // Pre-participant comments on the Codex artifact: no author, no mentions.
    {
        let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
        db.execute(
            "UPDATE comments SET author_public_id = NULL
             WHERE thread_id IN (?1, ?2) AND author_kind = 'viewer'",
            rusqlite::params![old1, old2],
        )
        .unwrap();
        db.execute(
            "DELETE FROM mentions WHERE comment_id IN
             (SELECT id FROM comments WHERE thread_id IN (?1, ?2))",
            rusqlite::params![old1, old2],
        )
        .unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    let end = clax_core::time::stamp(clax_core::working::Clock::now(clock.as_ref()));
    assert!(
        end.as_str() < FIXED_T,
        "the fixture ends at {end}, before {FIXED_T}"
    );
    std::fs::write(fixtures_dir().join(FIXTURE), dump(&ts.home)).unwrap();
}

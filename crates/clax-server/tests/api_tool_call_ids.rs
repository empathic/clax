//! Claude Code's exact call IDs (spec 2026-10-06-toolpath-audit-design
//! §6.7): the PostToolUse hook reports `{tool_use_id, tool_name,
//! args_sha256}` to `POST /api/sessions/<sid>/tool-call-ids`, and the
//! daemon records `tool.call_id`, naming the one `tool.call` of the
//! session that qualifies, or `null`.
//!
//! The hook's request body is built by the hook's own
//! [`events::tool_call_id`] from stdin as Claude Code writes it, and the
//! calls it is matched against are made by the shim's own [`CallScope`].

use crate::common::TestServer;
use clax_core::audit::ToolOutcome;
use clax_hooks::events::{self, Daemon};
use clax_hooks::input::HookInput;
use clax_mcp::calls::CallScope;
use clax_server::routes::sessions::{CALL_ID_CAP, CallIdWait};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

/// A daemon that lists one live Claude Code session, `hsid` as Clax
/// session `sid`, and keeps what the hook posts.
struct Keep {
    hsid: String,
    sid: String,
    posted: RefCell<Vec<(String, Value)>>,
}

impl Keep {
    fn new(hsid: &str, sid: &str) -> Keep {
        Keep {
            hsid: hsid.into(),
            sid: sid.into(),
            posted: RefCell::new(vec![]),
        }
    }
}

impl Daemon for Keep {
    fn browser_url(&self, path: &str) -> String {
        path.to_string()
    }
    fn get(&self, path: &str) -> anyhow::Result<Value> {
        assert_eq!(path, "/api/sessions?live=true");
        Ok(json!({"sessions": [
            {"id": self.sid, "harness": "claude", "harness_session_id": self.hsid},
        ]}))
    }
    fn get_with_timeout(&self, path: &str, _: Duration) -> anyhow::Result<Value> {
        self.get(path)
    }
    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        self.posted
            .borrow_mut()
            .push((path.to_string(), body.clone()));
        Ok(json!({"recorded": true}))
    }
    fn patch(&self, _: &str, _: &Value) -> anyhow::Result<Value> {
        unreachable!("the call-ID hook patches nothing")
    }
}

/// PostToolUse stdin as Claude Code writes it, with `tool_input` spliced
/// in as JSON text, so its number forms reach the hook as written.
fn stdin(hsid: &str, tool_name: &str, tool_use_id: &str, tool_input: &str) -> String {
    format!(
        r#"{{"session_id":"{hsid}","transcript_path":"/t/{hsid}.jsonl","cwd":"/w","permission_mode":"default","hook_event_name":"PostToolUse","tool_name":"{tool_name}","tool_input":{tool_input},"tool_response":[{{"type":"text","text":"ok"}}],"tool_use_id":"{tool_use_id}"}}"#
    )
}

/// The path and body the hook posts for `stdin`, run against a session
/// listing that names `hsid` as `sid`.
fn hook_request(hsid: &str, sid: &str, stdin: &str) -> (String, Value) {
    let keep = Keep::new(hsid, sid);
    let out = events::tool_call_id("claude", &HookInput::parse(stdin), &keep).unwrap();
    assert_eq!(out, clax_hooks::output::HookOutput::none());
    let mut posted = keep.posted.into_inner();
    assert_eq!(posted.len(), 1, "{posted:?}");
    posted.pop().unwrap()
}

/// Makes a tool call as the shim does, ending `ended_ago` before now, and
/// reports it for session `sid`, naming `artifact` when given; returns its
/// call ID.
async fn report_call(
    ts: &TestServer,
    sid: &str,
    tool: &str,
    args: &str,
    ended_ago: Duration,
    artifact: Option<&str>,
) -> String {
    let args: Map<String, Value> = serde_json::from_str(args).unwrap();
    let scope = CallScope::begin(tool, Some(&args), None);
    let mut report = scope.report(ToolOutcome::Ok);
    let ended = chrono::Utc::now() - ended_ago;
    let ms = chrono::SecondsFormat::Millis;
    report.started_at = (ended - chrono::Duration::milliseconds(100)).to_rfc3339_opts(ms, true);
    report.ended_at = ended.to_rfc3339_opts(ms, true);
    report.artifact_id = artifact.map(str::to_string);
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/tool-calls", ts.base)),
        )
        .header("x-clax-via", "mcp")
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    scope.header().call_id.clone()
}

/// [`report_call`] for a call that has just ended.
async fn shim_call(ts: &TestServer, sid: &str, tool: &str, args: &str) -> String {
    report_call(ts, sid, tool, args, Duration::ZERO, None).await
}

/// Sends the hook's request as `clax hook` does; the status and answer.
async fn send(ts: &TestServer, path: &str, body: &Value) -> (u16, Value) {
    let res = ts
        .authed(ts.client.post(format!("{}{path}", ts.base)))
        .header("x-clax-via", "hook")
        .json(body)
        .send()
        .await
        .unwrap();
    let code = res.status().as_u16();
    (code, res.json().await.unwrap_or(Value::Null))
}

/// Starts the hook's request as `clax hook` does, answered later.
fn send_later(ts: &TestServer, path: &str, body: Value) -> tokio::task::JoinHandle<(u16, Value)> {
    let (url, token, client) = (
        format!("{}{path}", ts.base),
        ts.token.clone(),
        ts.client.clone(),
    );
    tokio::spawn(async move {
        let res = client
            .post(url)
            .bearer_auth(token)
            .header("x-clax-via", "hook")
            .json(&body)
            .send()
            .await
            .unwrap();
        (res.status().as_u16(), res.json::<Value>().await.unwrap())
    })
}

/// Returns once `n` call-ID reports wait.
async fn parked(wait: &CallIdWait, n: usize) {
    tokio::time::timeout(
        Duration::from_secs(60),
        wait.parked.subscribe().wait_for(|p| *p == n),
    )
    .await
    .unwrap_or_else(|_| panic!("{n} reports wait"))
    .unwrap();
}

/// The `tool.call_id` events, oldest first: actor, artifact and call
/// columns, and body.
fn call_id_events(ts: &TestServer) -> Vec<(Value, Option<String>, Option<String>, Value)> {
    let c = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let mut q = c
        .prepare(
            "SELECT actor, artifact_id, call_id, body FROM audit_events
             WHERE kind = 'tool.call_id' ORDER BY seq",
        )
        .unwrap();
    q.query_map([], |r| {
        Ok((
            serde_json::from_str(&r.get::<_, String>(0)?).unwrap(),
            r.get(1)?,
            r.get(2)?,
            serde_json::from_str(&r.get::<_, String>(3)?).unwrap(),
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// A daemon whose call-ID requests do not wait for a late `tool.call`.
async fn no_grace() -> TestServer {
    TestServer::spawn_with(|st| {
        st.call_ids = Arc::new(CallIdWait::new(Duration::ZERO, CALL_ID_CAP))
    })
    .await
}

/// A daemon whose call-ID requests wait far longer than a test may take,
/// and its [`CallIdWait`] with room for `cap` waiting reports.
async fn long_grace(cap: usize) -> (TestServer, Arc<CallIdWait>) {
    let wait = Arc::new(CallIdWait::new(Duration::from_secs(600), cap));
    let ts = TestServer::spawn_with(|st| st.call_ids = wait.clone()).await;
    (ts, wait)
}

async fn session(ts: &TestServer, hsid: &str) -> String {
    ts.register_session("claude", hsid).await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

const PUBLISH: &str = "mcp__plugin_clax_clax__publish";
const A: &str = r#"{"id":"k3m9q2w8x1ab","if_version":3}"#;
const A_SHA: &str = "sha256:c6e6f48ad9e94345a81d22b0fa628e053e81e5785a38f0f61965c9196a4bfe93";

#[tokio::test]
async fn posttooluse_matches_recent_call_by_tool_and_hash() {
    let ts = no_grace().await;
    let sid = session(&ts, "cc-ids").await;
    let aid = ts.publish("Called", &[("index.html", "<p>x")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = r#"{"id":"k3m9q2w8x1ab","if_version":4}"#;
    // An identical call that ended long before the report: not a candidate.
    report_call(&ts, &sid, "publish", A, Duration::from_secs(30), None).await;
    let other_args = shim_call(&ts, &sid, "publish", b).await;
    let other_tool = shim_call(&ts, &sid, "read", A).await;
    let call = report_call(&ts, &sid, "publish", A, Duration::ZERO, Some(&aid)).await;

    let (path, body) = hook_request("cc-ids", &sid, &stdin("cc-ids", PUBLISH, "toolu_01A", A));
    assert_eq!(path, format!("/api/sessions/{sid}/tool-call-ids"));
    assert_eq!(
        body,
        json!({"tool_use_id": "toolu_01A", "tool_name": PUBLISH, "args_sha256": A_SHA})
    );
    let (code, v) = send(&ts, &path, &body).await;
    assert_eq!(code, 201, "{v}");
    assert_eq!(v["recorded"], true);
    assert_eq!(v["call_id"], call.as_str());
    let recs = call_id_events(&ts);
    assert_eq!(recs.len(), 1);
    let (actor, artifact_col, call_col, body) = &recs[0];
    assert_eq!(actor["type"], "agent");
    assert_eq!(actor["session_id"], sid.as_str());
    assert_eq!(actor["harness_session_id"], "cc-ids");
    assert_eq!(call_col.as_deref(), Some(call.as_str()));
    assert_eq!(
        artifact_col.as_deref(),
        Some(aid.as_str()),
        "the link is on the call's artifact"
    );
    assert_eq!(body["via"], "hook");
    assert_eq!(body["call_id"], call.as_str());
    assert_eq!(body["harness_call_id"], "toolu_01A");
    assert_eq!(body["harness_tool"], PUBLISH);
    assert_eq!(body["args_sha256"], A_SHA);
    assert!(body.get("call").is_none(), "{body}");

    // The same harness call reported again records nothing.
    let (code, v) = send(
        &ts,
        &path,
        &json!({"tool_use_id": "toolu_01A", "tool_name": PUBLISH, "args_sha256": A_SHA}),
    )
    .await;
    assert_eq!((code, &v["recorded"]), (200, &json!(false)), "{v}");
    assert_eq!(call_id_events(&ts).len(), 1);

    // The call is claimed, and the older one ended too long ago.
    let (path, body) = hook_request("cc-ids", &sid, &stdin("cc-ids", PUBLISH, "toolu_02B", A));
    let (code, v) = send(&ts, &path, &body).await;
    assert_eq!((code, &v["call_id"]), (201, &Value::Null), "{v}");

    // Another tool, or other arguments, name their own calls.
    let (path, body) = hook_request(
        "cc-ids",
        &sid,
        &stdin("cc-ids", "mcp__clax__read", "toolu_04D", A),
    );
    let (_, v) = send(&ts, &path, &body).await;
    assert_eq!(v["call_id"], other_tool.as_str(), "{v}");
    let (path, body) = hook_request("cc-ids", &sid, &stdin("cc-ids", PUBLISH, "toolu_05E", b));
    let (_, v) = send(&ts, &path, &body).await;
    assert_eq!(v["call_id"], other_args.as_str(), "{v}");

    // The link is rendered as the harness's tool-use ref.
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
    let refs: Vec<String> = doc["paths"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["steps"].as_array().cloned().unwrap_or_default())
        .flat_map(|s| s["meta"]["refs"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["rel"] == "tool-use")
        .map(|r| r["href"].as_str().unwrap().to_string())
        .collect();
    assert!(
        refs.contains(&"agent://claude-code/cc-ids/tool/toolu_01A".to_string()),
        "{refs:?}"
    );
}

#[tokio::test]
async fn a_call_that_ended_long_ago_or_has_a_harness_id_is_not_matched() {
    let ts = no_grace().await;
    let sid = session(&ts, "cc-old").await;
    let args = r#"{"url_or_id":"k3m9q2w8x1ab"}"#;
    report_call(&ts, &sid, "read", args, Duration::from_secs(6), None).await;
    // A call that already carries a harness ID (as Pi's do).
    let mut report = CallScope::begin(
        "read",
        serde_json::from_str::<Map<String, Value>>(args)
            .ok()
            .as_ref(),
        None,
    )
    .report(ToolOutcome::Ok);
    report.harness_call_id = Some("toolu_known".into());
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/tool-calls", ts.base)),
        )
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let (path, body) = hook_request(
        "cc-old",
        &sid,
        &stdin("cc-old", "mcp__plugin_clax_clax__read", "toolu_late", args),
    );
    let (code, v) = send(&ts, &path, &body).await;
    assert_eq!((code, &v["call_id"]), (201, &Value::Null), "{v}");
}

#[tokio::test]
async fn a_clock_step_back_hides_no_call() {
    let ts = no_grace().await;
    let sid = session(&ts, "cc-clock").await;
    let call = shim_call(&ts, &sid, "publish", A).await;
    // A later event recorded with an earlier time, as after a clock step.
    let later = shim_call(&ts, &sid, "list", "{}").await;
    let at = (chrono::Utc::now() - chrono::Duration::seconds(120))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    rusqlite::Connection::open(ts.home.db_path())
        .unwrap()
        .execute(
            "UPDATE audit_events SET at = ?1 WHERE call_id = ?2",
            [&at, &later],
        )
        .unwrap();
    let (path, body) = hook_request(
        "cc-clock",
        &sid,
        &stdin("cc-clock", PUBLISH, "toolu_clock", A),
    );
    let (_, v) = send(&ts, &path, &body).await;
    assert_eq!(v["call_id"], call.as_str(), "{v}");
}

#[tokio::test]
async fn unmatched_records_null_call_id() {
    let ts = no_grace().await;
    let sid = session(&ts, "cc-none").await;
    let (path, body) = hook_request(
        "cc-none",
        &sid,
        &stdin(
            "cc-none",
            "mcp__plugin_clax_clax__list",
            "toolu_01XYZ",
            "{}",
        ),
    );
    let (code, v) = send(&ts, &path, &body).await;
    assert_eq!(code, 201, "{v}");
    assert_eq!(v["recorded"], true);
    assert_eq!(v["call_id"], Value::Null);
    let recs = call_id_events(&ts);
    assert_eq!(recs.len(), 1);
    let (actor, artifact, call_col, body) = &recs[0];
    assert_eq!(actor["session_id"], sid.as_str());
    assert_eq!((artifact, call_col), (&None, &None));
    assert_eq!(body["call_id"], Value::Null);
    assert!(body.as_object().unwrap().contains_key("call_id"));
    assert_eq!(body["harness_call_id"], "toolu_01XYZ");
    assert_eq!(body["harness_tool"], "mcp__plugin_clax_clax__list");
    assert_eq!(
        body["args_sha256"],
        "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
    assert_eq!(body["via"], "hook");
}

#[tokio::test]
async fn identical_calls_in_flight_record_null() {
    let ts = no_grace().await;
    let sid = session(&ts, "cc-twins").await;
    shim_call(&ts, &sid, "read", A).await;
    shim_call(&ts, &sid, "read", A).await;
    for id in ["toolu_t1", "toolu_t2"] {
        let (path, body) = hook_request(
            "cc-twins",
            &sid,
            &stdin("cc-twins", "mcp__plugin_clax_clax__read", id, A),
        );
        let (code, v) = send(&ts, &path, &body).await;
        assert_eq!((code, &v["call_id"]), (201, &Value::Null), "{id}: {v}");
    }
}

#[tokio::test]
async fn a_lost_hook_does_not_shift_later_reports() {
    let (ts, wait) = long_grace(CALL_ID_CAP).await;
    let sid = session(&ts, "cc-lost").await;
    let tool = "mcp__plugin_clax_clax__wait_for_feedback";
    // A call whose hook never reported.
    report_call(
        &ts,
        &sid,
        "wait_for_feedback",
        "{}",
        Duration::from_secs(20),
        None,
    )
    .await;
    // The next identical call's report arrives before the call's: it waits
    // for its own call instead of taking the unclaimed one.
    let (path, body) = hook_request("cc-lost", &sid, &stdin("cc-lost", tool, "toolu_n2", "{}"));
    let hook = send_later(&ts, &path, body);
    parked(&wait, 1).await;
    let second = shim_call(&ts, &sid, "wait_for_feedback", "{}").await;
    let (code, v) = hook.await.unwrap();
    assert_eq!((code, &v["call_id"]), (201, &json!(second)), "{v}");
    // And the one after it is paired with its own call.
    let third = shim_call(&ts, &sid, "wait_for_feedback", "{}").await;
    let (path, body) = hook_request("cc-lost", &sid, &stdin("cc-lost", tool, "toolu_n3", "{}"));
    let (code, v) = send(&ts, &path, &body).await;
    assert_eq!((code, &v["call_id"]), (201, &json!(third)), "{v}");
}

#[tokio::test]
async fn a_call_reported_while_the_hook_waits_is_matched() {
    let (ts, wait) = long_grace(CALL_ID_CAP).await;
    let sid = session(&ts, "cc-race").await;
    let args: Map<String, Value> = serde_json::from_str(r#"{"id":"k3m9q2w8x1ab"}"#).unwrap();
    let scope = CallScope::begin("open", Some(&args), None);
    let (path, body) = hook_request(
        "cc-race",
        &sid,
        &stdin(
            "cc-race",
            "mcp__plugin_clax_clax__open",
            "toolu_early",
            r#"{"id":"k3m9q2w8x1ab"}"#,
        ),
    );
    let hook = send_later(&ts, &path, body);
    // The hook's report waits, unmatched, before the call is reported.
    parked(&wait, 1).await;
    assert!(call_id_events(&ts).is_empty());
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/tool-calls", ts.base)),
        )
        .json(&scope.report(ToolOutcome::Ok))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let (code, v) = tokio::time::timeout(Duration::from_secs(60), hook)
        .await
        .expect("the hook's request was answered once the call was reported")
        .unwrap();
    assert_eq!(code, 201, "{v}");
    assert_eq!(v["call_id"], scope.header().call_id.as_str());
}

#[tokio::test]
async fn reports_for_identical_calls_that_both_arrive_early_record_null() {
    let (ts, wait) = long_grace(CALL_ID_CAP).await;
    let sid = session(&ts, "cc-early").await;
    let read = "mcp__plugin_clax_clax__read";
    let (path, first) = hook_request("cc-early", &sid, &stdin("cc-early", read, "toolu_e1", A));
    let first = send_later(&ts, &path, first);
    parked(&wait, 1).await;
    // The second report cannot tell its call from the first's: both
    // record null at once, without waiting for either call.
    let (path, second) = hook_request("cc-early", &sid, &stdin("cc-early", read, "toolu_e2", A));
    let (code, v) = tokio::time::timeout(Duration::from_secs(60), send(&ts, &path, &second))
        .await
        .expect("the second report does not wait");
    assert_eq!((code, &v["call_id"]), (201, &Value::Null), "{v}");
    let (code, v) = tokio::time::timeout(Duration::from_secs(60), first)
        .await
        .expect("the first report stops waiting")
        .unwrap();
    assert_eq!((code, &v["call_id"]), (201, &Value::Null), "{v}");
    // The calls, reported afterwards, are named by neither.
    shim_call(&ts, &sid, "read", A).await;
    shim_call(&ts, &sid, "read", A).await;
    let recs = call_id_events(&ts);
    assert_eq!(recs.len(), 2);
    assert!(
        recs.iter()
            .all(|(_, _, call, body)| call.is_none() && body["call_id"].is_null())
    );
}

#[tokio::test]
async fn two_reports_woken_by_one_call_never_both_claim_it() {
    // A short grace: a report that loses the race waits it out, then
    // records null.
    let wait = Arc::new(CallIdWait::new(Duration::from_millis(300), CALL_ID_CAP));
    let ts = TestServer::spawn_with(|st| st.call_ids = wait.clone()).await;
    let sid = session(&ts, "cc-race2").await;
    let open = "mcp__plugin_clax_clax__open";
    for round in 0..10 {
        let args = format!(r#"{{"id":"round{round}"}}"#);
        let reports: Vec<_> = ["a", "b"]
            .iter()
            .map(|who| {
                let id = format!("toolu_{round}{who}");
                let (path, body) =
                    hook_request("cc-race2", &sid, &stdin("cc-race2", open, &id, &args));
                send_later(&ts, &path, body)
            })
            .collect();
        let call = shim_call(&ts, &sid, "open", &args).await;
        for r in reports {
            let (code, v) = tokio::time::timeout(Duration::from_secs(60), r)
                .await
                .expect("each report is answered within its grace")
                .unwrap();
            assert_eq!(code, 201, "round {round}: {v}");
        }
        let naming = call_id_events(&ts)
            .iter()
            .filter(|(_, _, c, _)| c.as_deref() == Some(call.as_str()))
            .count();
        assert!(naming <= 1, "round {round}: {naming} rows name {call}");
    }
}

#[tokio::test]
async fn past_the_cap_a_report_records_null_at_once() {
    let (ts, wait) = long_grace(1).await;
    let (s1, s2) = (session(&ts, "cc-cap1").await, session(&ts, "cc-cap2").await);
    let list = "mcp__plugin_clax_clax__list";
    let (path, body) = hook_request("cc-cap1", &s1, &stdin("cc-cap1", list, "toolu_c1", "{}"));
    let _first = send_later(&ts, &path, body);
    parked(&wait, 1).await;
    let (path, body) = hook_request("cc-cap2", &s2, &stdin("cc-cap2", list, "toolu_c2", "{}"));
    let (code, v) = tokio::time::timeout(Duration::from_secs(60), send(&ts, &path, &body))
        .await
        .expect("a report past the cap does not wait");
    assert_eq!((code, &v["call_id"]), (201, &Value::Null), "{v}");
    assert_eq!(*wait.parked.borrow(), 1);
}

#[test]
fn hook_hash_equals_shim_hash_for_same_input() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../clax-core/tests/toolpath/args-hash-vectors.json"
    ))
    .unwrap();
    assert_eq!(vectors.len(), 7);
    for v in &vectors {
        let text = v["arguments"].as_str().unwrap();
        let (_, body) = hook_request("h", "S", &stdin("h", PUBLISH, "toolu_v", text));
        let args: Map<String, Value> = serde_json::from_str(text).unwrap();
        let shim = CallScope::begin("publish", Some(&args), None);
        assert_eq!(body["args_sha256"], shim.header().args_sha256, "{text}");
        assert_eq!(body["args_sha256"], v["args_sha256"], "{text}");
    }
    // No tool_input, or a null one, is the empty object, as for a call
    // without arguments.
    let none = CallScope::begin("publish", None, None)
        .header()
        .args_sha256
        .clone();
    for input in [
        format!(r#"{{"session_id":"h","tool_name":"{PUBLISH}","tool_use_id":"toolu_n"}}"#),
        format!(
            r#"{{"session_id":"h","tool_name":"{PUBLISH}","tool_use_id":"toolu_n","tool_input":null}}"#
        ),
    ] {
        let keep = Keep::new("h", "S");
        events::tool_call_id("claude", &HookInput::parse(&input), &keep).unwrap();
        assert_eq!(keep.posted.borrow()[0].1["args_sha256"], none, "{input}");
    }
}

#[tokio::test]
async fn call_id_reports_are_checked() {
    let ts = no_grace().await;
    let sid = session(&ts, "cc-bad").await;
    let path = format!("/api/sessions/{sid}/tool-call-ids");
    let good = json!({"tool_use_id": "toolu_1", "tool_name": PUBLISH,
        "args_sha256": format!("sha256:{}", "ab".repeat(32))});
    for (field, bad) in [
        ("tool_use_id", json!("")),
        ("tool_use_id", json!("a\nb")),
        ("tool_name", json!("Bash")),
        ("tool_name", json!("mcp__github__publish")),
        ("tool_name", json!("mcp__clax__Publish")),
        ("args_sha256", json!("sha256:xyz")),
    ] {
        let mut body = good.clone();
        body[field] = bad;
        let (code, v) = send(&ts, &path, &body).await;
        assert_eq!(
            (code, &v["error"]["code"]),
            (400, &json!("invalid_tool_call_id")),
            "{field}: {v}"
        );
    }
    // A server whose name holds "clax", with a tool Clax does not have:
    // answered at once, and nothing is recorded.
    let mut other = good.clone();
    other["tool_name"] = json!("mcp__claxon_notes__frobnicate");
    let (code, _) = send(&ts, &path, &other).await;
    assert_eq!(code, 204);
    let (code, v) = send(
        &ts,
        "/api/sessions/01JBC0000000000000000000ZZ/tool-call-ids",
        &good,
    )
    .await;
    assert_eq!(
        (code, &v["error"]["code"]),
        (404, &json!("unknown_session")),
        "{v}"
    );
    let res = ts
        .client
        .post(format!("{}{path}", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&good)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert!(call_id_events(&ts).is_empty());
}

#[test]
fn the_hook_reports_only_clax_tools_with_an_id() {
    // Another tool, or input without a tool_use_id: an error the hook logs,
    // and no request.
    for stdin in [
        r#"{"session_id":"h","tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{}}"#,
        r#"{"session_id":"h","tool_name":"mcp__plugin_clax_clax__list","tool_input":{}}"#,
    ] {
        let keep = Keep::new("h", "S");
        assert!(events::tool_call_id("claude", &HookInput::parse(stdin), &keep).is_err());
        assert!(keep.posted.borrow().is_empty());
    }
    // No live session: nothing to report.
    let keep = Keep::new("other", "S");
    let out = events::tool_call_id(
        "claude",
        &HookInput::parse(&stdin("h", PUBLISH, "toolu_1", "{}")),
        &keep,
    )
    .unwrap();
    assert_eq!(out, clax_hooks::output::HookOutput::none());
    assert!(keep.posted.borrow().is_empty());
}

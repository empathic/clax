//! Shared test support: a scripted Messages API on 127.0.0.1 (raw HTTP,
//! one scripted reply per request, in order, recording the usage it
//! reported), a store on a scratch home with an injected clock, a run on a
//! one-page artifact, and the deps that drive it (the QuickJS sandbox, a
//! [`FakeBotHost`] over the store).

#![allow(dead_code)]

use clax_bot::host::{ArtifactMeta, CommentMeta, FakeBotHost, RequestContext, ThreadMeta};
use clax_bot::run::{Deps, Keys, RunSettings, SettingsSource};
use clax_bot::{EventHub, Next, TickOpts, TickOutcome};
use clax_bot_sandbox::{CancelToken, Sandbox};
use clax_core::audit::AuditCtx;
use clax_core::working::ManualClock;
use clax_core::{Home, publish::PublishRequest};
use clax_sample::messages::{ApiKey, MessagesClient, RetryPolicy};
use clax_store::Store;
use clax_store::bot_runs::{DriverId, NewRun, RequestKind, Run};
use clax_store::credits::{KeySource, UsageRow};
use clax_store::roles::OWNER;
use clax_store::secrets::Kek;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const INSTANCE_KEY: &str = "sk-ant-instance-0000";
pub const OWN_KEY: &str = "sk-ant-own-1111";

/// A page that follows the page contract.
pub const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Quarterly Report</title>
<style>
  :root { --bg: #fff; --fg: #111; --accent: #c00; }
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) { --bg: #111; --fg: #eee; }
  }
  :root[data-theme="dark"] { --bg: #111; --fg: #eee; }
  body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); }
  h1 { color: var(--accent); }
</style>
</head>
<body>
<h1>Quarterly goals</h1>
</body>
</html>
"##;

// ---------------------------------------------------------------------------
// The scripted Messages API
// ---------------------------------------------------------------------------

/// Token counts a reply reports: input, cache read, cache write, output.
pub type Tokens = (u64, u64, u64, u64);

/// One scripted answer.
pub enum Reply {
    /// A 200 event stream with this body, reporting `usage`.
    Sse { body: String, usage: Tokens },
    /// A non-2xx answer with this JSON body.
    Status(u16, String),
    /// Reads the request and never answers.
    Hang,
    /// Sends `message_start` with this usage (output 1) and
    /// [`PARTIAL_DELTAS`] text deltas, then nothing.
    StartThenHang(Tokens),
    /// Sends `message_start` with this usage (output 1) and
    /// [`PARTIAL_DELTAS`] text deltas, then closes the connection: a cut
    /// stream.
    Cut(Tokens),
}

/// A request the fake API received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
pub struct FakeApi {
    pub base: String,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// The usage each answered call reported, as the provider would bill it.
    served: Arc<Mutex<Vec<Tokens>>>,
    pub arrived: Arc<tokio::sync::Notify>,
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = sock.read(&mut tmp).await.ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let headers: Vec<(String, String)> = head
        .split("\r\n")
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut tmp).await.ok().filter(|n| *n > 0)?;
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Seen {
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    })
}

fn event(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

fn message_start(usage: Tokens) -> String {
    let (input, read, write, _) = usage;
    event(
        "message_start",
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-sonnet-5-5",
            "usage": {"input_tokens": input, "cache_read_input_tokens": read,
                      "cache_creation_input_tokens": write, "output_tokens": 1}}}),
    )
}

/// The text a partial stream sends: [`PARTIAL_DELTAS`] deltas of this.
pub const PARTIAL_TEXT: &str = "Here is a plan for the page. ";
pub const PARTIAL_DELTAS: usize = 40;

/// A stream that starts, then streams text with no `message_delta` (the
/// provider reports output only at the end), and how many output tokens
/// the provider bills for it (about 4 bytes a token).
fn partial(usage: Tokens) -> (String, u64) {
    let mut body = message_start(usage);
    body.push_str(&event(
        "content_block_start",
        json!({"type": "content_block_start", "index": 0,
               "content_block": {"type": "text", "text": ""}}),
    ));
    for _ in 0..PARTIAL_DELTAS {
        body.push_str(&event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": PARTIAL_TEXT}}),
        ));
    }
    (body, (PARTIAL_TEXT.len() * PARTIAL_DELTAS / 4) as u64)
}

impl FakeApi {
    pub async fn start() -> FakeApi {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = FakeApi {
            base: format!("http://{}", listener.local_addr().unwrap()),
            replies: Arc::default(),
            seen: Arc::default(),
            served: Arc::default(),
            arrived: Arc::default(),
        };
        let a = api.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let a = a.clone();
                tokio::spawn(async move {
                    let Some(seen) = read_request(&mut sock).await else {
                        return;
                    };
                    a.seen.lock().unwrap().push(seen);
                    let reply = a.replies.lock().unwrap().pop_front();
                    let head = |status: u16, ctype: &str, len: Option<usize>| match len {
                        Some(n) => format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {n}\r\nconnection: close\r\n\r\n"
                        ),
                        None => format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\nconnection: close\r\n\r\n"
                        ),
                    };
                    match reply {
                        Some(Reply::Sse { body, usage }) => {
                            a.served.lock().unwrap().push(usage);
                            a.arrived.notify_one();
                            let _ = sock
                                .write_all(
                                    head(200, "text/event-stream", Some(body.len())).as_bytes(),
                                )
                                .await;
                            let _ = sock.write_all(body.as_bytes()).await;
                        }
                        Some(Reply::Status(s, body)) => {
                            a.arrived.notify_one();
                            let _ = sock
                                .write_all(head(s, "application/json", Some(body.len())).as_bytes())
                                .await;
                            let _ = sock.write_all(body.as_bytes()).await;
                        }
                        Some(Reply::Hang) => {
                            a.arrived.notify_one();
                            std::future::pending::<()>().await;
                        }
                        Some(Reply::StartThenHang(usage)) => {
                            let (body, output) = partial(usage);
                            a.served
                                .lock()
                                .unwrap()
                                .push((usage.0, usage.1, usage.2, output));
                            let _ = sock
                                .write_all(head(200, "text/event-stream", None).as_bytes())
                                .await;
                            let _ = sock.write_all(body.as_bytes()).await;
                            let _ = sock.flush().await;
                            a.arrived.notify_one();
                            std::future::pending::<()>().await;
                        }
                        Some(Reply::Cut(usage)) => {
                            let (body, output) = partial(usage);
                            a.served
                                .lock()
                                .unwrap()
                                .push((usage.0, usage.1, usage.2, output));
                            a.arrived.notify_one();
                            let _ = sock
                                .write_all(head(200, "text/event-stream", None).as_bytes())
                                .await;
                            let _ = sock.write_all(body.as_bytes()).await;
                        }
                        None => {
                            a.arrived.notify_one();
                            let body =
                                json!({"type": "error", "error": {"type": "invalid_request_error",
                                "message": "no scripted reply"}})
                                .to_string();
                            let _ = sock
                                .write_all(
                                    head(400, "application/json", Some(body.len())).as_bytes(),
                                )
                                .await;
                            let _ = sock.write_all(body.as_bytes()).await;
                        }
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        api
    }

    pub fn push(&self, r: Reply) {
        self.replies.lock().unwrap().push_back(r);
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn left(&self) -> usize {
        self.replies.lock().unwrap().len()
    }

    /// The tokens every answered call reported, summed.
    pub fn served(&self) -> Tokens {
        self.served
            .lock()
            .unwrap()
            .iter()
            .fold((0, 0, 0, 0), |a, u| {
                (a.0 + u.0, a.1 + u.1, a.2 + u.2, a.3 + u.3)
            })
    }
}

/// One content block of a scripted turn.
pub enum B {
    Text(&'static str),
    Thinking(&'static str),
    /// A tool call with its input JSON as it streams (which may not parse).
    Tool(&'static str, &'static str, String),
}

/// The event stream of one assistant turn.
pub fn stream(blocks: &[B], stop: &str, usage: Tokens) -> Reply {
    let mut s = message_start(usage);
    let mut ev = |name: &str, data: Value| s.push_str(&event(name, data));
    for (i, b) in blocks.iter().enumerate() {
        match b {
            B::Text(t) => {
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": i, "content_block": {"type": "text", "text": ""}}),
                );
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": t}}),
                );
            }
            B::Thinking(t) => {
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": i, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
                );
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": i, "delta": {"type": "thinking_delta", "thinking": t}}),
                );
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": i, "delta": {"type": "signature_delta", "signature": "sig"}}),
                );
            }
            B::Tool(id, name, input) => {
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}}),
                );
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": input}}),
                );
            }
        }
        ev(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": i}),
        );
    }
    ev(
        "message_delta",
        json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": usage.3}}),
    );
    ev("message_stop", json!({"type": "message_stop"}));
    Reply::Sse { body: s, usage }
}

/// Default usage of a scripted turn: 1000 input, 2000 cache read, 500
/// cache write, 300 output tokens.
pub const USAGE: Tokens = (1000, 2000, 500, 300);

pub fn script_turn(id: &'static str, title: &str, code: &str) -> Reply {
    stream(
        &[
            B::Text("Let me look."),
            B::Tool(
                id,
                "run_script",
                json!({"title": title, "code": code}).to_string(),
            ),
        ],
        "tool_use",
        USAGE,
    )
}

pub fn finish_turn(summary: &str) -> Reply {
    stream(
        &[B::Tool(
            "tu_fin",
            "finish",
            json!({"summary": summary}).to_string(),
        )],
        "tool_use",
        USAGE,
    )
}

pub fn text_turn(text: &'static str) -> Reply {
    stream(&[B::Text(text)], "end_turn", USAGE)
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// Settings a test may change between ticks.
pub struct TestSettings(pub Mutex<RunSettings>);

impl SettingsSource for TestSettings {
    fn settings(&self) -> clax_core::Result<RunSettings> {
        Ok(self.0.lock().unwrap().clone())
    }
}

pub struct Env {
    pub _dir: tempfile::TempDir,
    pub store: Arc<Store>,
    pub clock: Arc<ManualClock>,
    pub host: Arc<FakeBotHost>,
    pub api: FakeApi,
    pub settings: Arc<TestSettings>,
    pub kek: Kek,
    pub aid: String,
    pub sandbox: Arc<dyn Sandbox>,
    pub hub: Arc<EventHub>,
    pub instance_key: bool,
    /// The run's thread, another thread of the artifact, and a thread of
    /// another artifact, all in the store.
    pub t1: String,
    pub t2: String,
    pub foreign: String,
}

fn new_thread(store: &Store, aid: &str, body: &str) -> String {
    let anchor: clax_core::anchor::Anchor = serde_json::from_value(json!({
        "kind": "element", "selector": "h1", "quote": "Quarterly goals",
        "prefix": "", "suffix": "", "html_hash": "sha256:00", "file": "index.html"
    }))
    .unwrap();
    store
        .create_thread(
            &AuditCtx::DAEMON,
            &clax_core::ArtifactId::parse(aid).unwrap(),
            clax_store::threads::NewThread {
                version_n: 1,
                anchor,
                author_name: "Owner".into(),
                author_public_id: None,
                body: body.into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
}

fn new_artifact(store: &Store) -> String {
    let req: PublishRequest = serde_json::from_value(json!({
        "title": "Quarterly Report",
        "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}
    }))
    .unwrap();
    store
        .create_artifact(
            &AuditCtx::DAEMON,
            clax_core::publish::validate(req).unwrap(),
            None,
        )
        .unwrap()
        .0
        .id
}

fn thread(id: &str, author: &str, from_sender: bool, text: &str) -> ThreadMeta {
    ThreadMeta {
        id: id.into(),
        anchor: Some(json!({"selector": "h1"})),
        resolved: false,
        comments: vec![CommentMeta {
            id: format!("c-{id}"),
            author: author.into(),
            from_sender,
            at: "2026-10-21T09:00:00Z".into(),
            text: text.into(),
        }],
    }
}

pub fn thread_context(aid: &str, t1: &str) -> RequestContext {
    RequestContext {
        artifact: ArtifactMeta {
            id: aid.into(),
            title: "Quarterly Report".into(),
            description: None,
            version: 1,
            url: None,
        },
        person_name: Some("Owner".into()),
        threads: vec![thread(t1, "Owner", true, "Make the heading blue.")],
        brief: None,
    }
}

impl Env {
    pub async fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let store = Arc::new(
            Store::open_with_clock(&Home::at(dir.path().join("ax")), clock.clone()).unwrap(),
        );
        let aid = new_artifact(&store);
        let t1 = new_thread(&store, &aid, "Make the heading blue.");
        let t2 = new_thread(&store, &aid, "Delete the page.");
        let other = new_artifact(&store);
        let foreign = new_thread(&store, &other, "Elsewhere.");
        let api = FakeApi::start().await;
        let settings = RunSettings {
            base_url: api.base.clone(),
            ..RunSettings::default()
        };
        // The base URL in effect is the run's, set as the owner's own
        // configuration would set it (so the environment's instance key is
        // sent to it), and a key set in a test is bound to it.
        let mut ov = clax_store::bot_settings::Overrides::default();
        ov.set(
            clax_store::bot_settings::Setting::BaseUrl,
            &serde_json::json!(api.base),
            "file",
        )
        .unwrap();
        store.set_bot_overrides(ov);
        let sandbox: Arc<dyn Sandbox> =
            Arc::new(clax_bot_js::QuickJs::new(clax_bot::api::op_names()));
        let host = Arc::new(FakeBotHost::new(store.clone(), thread_context(&aid, &t1)));
        Env {
            _dir: dir,
            host,
            store,
            clock,
            api,
            settings: Arc::new(TestSettings(Mutex::new(settings))),
            kek: Kek::generate(),
            aid,
            sandbox,
            hub: Arc::new(EventHub::new()),
            instance_key: true,
            t1,
            t2,
            foreign,
        }
    }

    /// `code` with `T1` and `T2` standing for the run's thread and the
    /// other thread of the artifact, replaced in one pass: a thread ID is a
    /// ULID, which may itself hold `T1` or `T2`, so a second pass over the
    /// first's output would rewrite inside the ID it put there.
    pub fn code(&self, code: &str) -> String {
        let mut out = String::with_capacity(code.len());
        let mut rest = code;
        while let Some(i) = rest.find('T') {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            if let Some(after) = tail.strip_prefix("T1") {
                out.push_str(&self.t1);
                rest = after;
            } else if let Some(after) = tail.strip_prefix("T2") {
                out.push_str(&self.t2);
                rest = after;
            } else {
                out.push('T');
                rest = &tail[1..];
            }
        }
        out.push_str(rest);
        out
    }

    /// [`script_turn`] with [`Env::code`].
    pub fn script_turn(&self, id: &'static str, title: &str, code: &str) -> Reply {
        script_turn(id, title, &self.code(code))
    }

    /// Writes document `path` of the artifact as its owner.
    pub fn set_doc(&self, path: &str, data: Value) {
        clax_store::app_data::shim::Docs::of(&self.store)
            .doc_set(
                &AuditCtx::DAEMON,
                &clax_core::ArtifactId::parse(&self.aid).unwrap(),
                path,
                data,
                clax_store::docs::Pin {
                    if_version: None,
                    lww: true,
                },
                &clax_core::db::Caller::new(clax_core::db::Level::Owner, None),
            )
            .unwrap();
    }

    /// ClaxBot's replies in the store on the artifact's threads, by
    /// thread.
    pub fn replies(&self) -> Vec<(String, String)> {
        self.host.replies(&[&self.t1, &self.t2])
    }

    pub fn set(&self, f: impl FnOnce(&mut RunSettings)) {
        f(&mut self.settings.0.lock().unwrap());
    }

    pub fn run(&self) -> String {
        self.run_as(RequestKind::Thread)
    }

    pub fn run_as(&self, request: RequestKind) -> String {
        self.store
            .create_run(
                &clax_bot::run::bot_ctx(OWNER),
                NewRun {
                    artifact_id: self.aid.clone(),
                    person: OWNER.into(),
                    request,
                    thread_ids: vec![self.t1.clone()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                },
            )
            .unwrap()
            .id
    }

    pub fn deps_with(&self, sandbox: Arc<dyn Sandbox>) -> Deps {
        let mut d = Deps::new(
            self.store.clone(),
            MessagesClient::new(),
            sandbox,
            self.host.clone(),
            self.settings.clone(),
            Keys {
                kek: Some(self.kek.clone()),
                env_instance: self.instance_key.then(|| ApiKey::new(INSTANCE_KEY)),
            },
            self.clock.clone(),
        );
        d.retry = RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        };
        d
    }

    pub fn deps(&self) -> Deps {
        self.deps_with(self.sandbox.clone())
    }

    pub async fn try_tick(
        &self,
        deps: &Deps,
        run: &str,
        driver: &DriverId,
        budget: Option<Duration>,
    ) -> clax_core::Result<TickOutcome> {
        clax_bot::tick(
            deps,
            run,
            driver,
            TickOpts {
                budget,
                cancel: CancelToken::new(),
                events: &*self.hub,
            },
        )
        .await
    }

    pub async fn tick(&self, deps: &Deps, run: &str, driver: &DriverId) -> TickOutcome {
        self.try_tick(deps, run, driver, None).await.unwrap()
    }

    /// Ticks until the run ends or waits; every outcome.
    pub async fn drive(&self, deps: &Deps, run: &str, driver: &DriverId) -> Vec<TickOutcome> {
        let mut out = Vec::new();
        for _ in 0..50 {
            let o = self.tick(deps, run, driver).await;
            let stop = o.next != Next::Runnable;
            out.push(o);
            if stop {
                return out;
            }
        }
        panic!("the run did not end in 50 ticks: {out:?}");
    }

    pub fn run_row(&self, id: &str) -> Run {
        self.store.run(id).unwrap().unwrap()
    }

    pub fn usage(&self) -> Vec<UsageRow> {
        self.store.usage_rows(OWNER, self.clock_now()).unwrap()
    }

    /// The ledger's token rows, summed by kind: (input, cache read, cache
    /// write, output); and how many of them charged credits.
    pub fn ledger_tokens(&self) -> (Tokens, usize) {
        let mut t = (0u64, 0u64, 0u64, 0u64);
        let mut charged = 0;
        for r in self.usage() {
            let q = r.qty as u64;
            match r.line.rsplit('.').next().unwrap_or("") {
                "input" => t.0 += q,
                "cached_input" => t.1 += q,
                "cache_write" => t.2 += q,
                "output" => t.3 += q,
                _ => continue,
            }
            if !r.absorbed {
                charged += 1;
            }
        }
        (t, charged)
    }

    pub fn clock_now(&self) -> chrono::DateTime<chrono::Utc> {
        self.store.now_utc()
    }

    /// The kinds and bodies of the bot.* audit events, in order.
    pub fn bot_events(&self) -> Vec<(String, Value)> {
        self.store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("bot."))
            .map(|e| (e.kind.clone(), serde_json::from_str(&e.body).unwrap()))
            .collect()
    }
}

/// How long a test waits for something that happens promptly.
pub const PROMPTLY: Duration = Duration::from_secs(30);

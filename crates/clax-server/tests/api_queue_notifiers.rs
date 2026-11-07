//! A queue's notifiers (`routes/queue_notifier.rs`) and their delivery
//! (`clax_server::notifiers`), against a fake Zulip on loopback: who may set
//! one, that its key is write-only, what each person is told, and that a
//! failure shows in fixed words and is tried again. Nothing here contacts
//! Zulip.

use clax_core::audit::AuditCtx;
use clax_core::report::{NewFix, ReporterProfile};
use clax_server::notifiers;
use clax_store::queue_notifiers::FixLinks;
use clax_store::queues::{ExternalScope, NewReport, ReportWriter, ReporterToken, Verdict};
use clax_testkit::{TestServer, element_anchor, until};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

const KEY: &str = "zulip-api-key-CANARY-wxyz";
const BOT: &str = "clax-bot@acme.example.com";
const CTX: &AuditCtx = &AuditCtx::DAEMON;
const SUMMARY: &str = "MODEL-WORDS changed the total";
const REPORT_TEXT: &str = "The total is wrong\nSECOND-LINE of the report";
/// Told to the fake: a 200 whose body says `result: error`.
const ZULIP_ERROR: u16 = 1200;

/// A status and the headers the fake answers with.
type Answer = (u16, Vec<(&'static str, String)>);

/// What the fake answers next, and what it was sent.
#[derive(Default)]
struct Zulip {
    answers: Mutex<VecDeque<Answer>>,
    got: Mutex<Vec<Got>>,
}

#[derive(Clone, Debug)]
struct Got {
    path: String,
    authorization: Option<String>,
    form: HashMap<String, String>,
}

impl Zulip {
    fn answer(&self, status: u16) {
        self.answer_with(status, vec![]);
    }

    fn answer_with(&self, status: u16, headers: Vec<(&'static str, String)>) {
        self.answers.lock().unwrap().push_back((status, headers));
    }

    fn got(&self) -> Vec<Got> {
        self.got.lock().unwrap().clone()
    }
}

async fn fake(z: Arc<Zulip>, req: axum::extract::Request) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = req.uri().path().to_string();
    let authorization = req
        .headers()
        .get("authorization")
        .and_then(|a| a.to_str().ok())
        .map(str::to_string);
    let body = axum::body::to_bytes(req.into_body(), 1 << 20)
        .await
        .unwrap();
    let form = url::form_urlencoded::parse(&body).into_owned().collect();
    z.got.lock().unwrap().push(Got {
        path,
        authorization,
        form,
    });
    let (status, headers) = z
        .answers
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or((200, vec![]));
    let mut r = if status == 200 {
        axum::Json(json!({"result": "success", "msg": "", "id": 1})).into_response()
    } else if status == ZULIP_ERROR {
        axum::Json(json!({"result": "error", "msg": format!("leak {KEY}")})).into_response()
    } else {
        (
            axum::http::StatusCode::from_u16(status).unwrap(),
            axum::Json(json!({"result": "error", "msg": format!("leak {KEY}")})),
        )
            .into_response()
    };
    for (k, v) in headers {
        r.headers_mut().insert(k, v.parse().unwrap());
    }
    r
}

async fn zulip() -> (Arc<Zulip>, String) {
    let z = Arc::new(Zulip::default());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let shared = z.clone();
    let app = axum::Router::new().fallback(move |req| fake(shared.clone(), req));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (z, base)
}

struct World {
    ts: TestServer,
    zulip: Arc<Zulip>,
    site: String,
    owner: String,
    sam: String,
    sam_public: String,
    sam_client: clax_testkit::TestClient,
    qid: String,
}

async fn call(
    ts: &TestServer,
    cookie: &str,
    m: Method,
    path: &str,
    b: Option<Value>,
) -> (u16, Value) {
    let mut r = ts
        .client
        .request(m, format!("{}{path}", ts.base))
        .header("cookie", cookie);
    if let Some(b) = b {
        r = r.json(&b);
    }
    let r = r.send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

async fn world() -> World {
    world_of(TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await).await
}

async fn world_of(ts: TestServer) -> World {
    let sam_client = ts.connect_person("Sam", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam_client.grant_id()).unwrap();
    let sam = format!("clax_viewer={}", ts.browser_cookie(&v.id));
    let owner = ts.owner_cookie();
    let (s, q) = call(
        &ts,
        &owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Bugs", "origins": ["https://app.example.com"]})),
    )
    .await;
    assert_eq!(s, 201, "{q}");
    let (zulip, site) = zulip().await;
    World {
        qid: q["queue"]["id"].as_str().unwrap().to_string(),
        ts,
        zulip,
        site,
        owner,
        sam,
        sam_public: v.public_id,
        sam_client,
    }
}

impl World {
    fn route(&self) -> String {
        format!("/api/queues/{}/notifiers/zulip", self.qid)
    }

    /// Where the queue's people read report `tid`.
    fn report_link(&self, tid: &str) -> String {
        format!("{}/queues/{}/{tid}", self.ts.state.browser_base, self.qid)
    }

    fn body(&self) -> Value {
        json!({
            "site": self.site, "bot_email": BOT, "api_key": KEY, "stream": "bugs",
            "people": {"Ana-Dev": "ana@acme.example.com"},
        })
    }

    async fn set(&self) -> Value {
        let (s, v) = call(
            &self.ts,
            &self.owner,
            Method::PUT,
            &self.route(),
            Some(self.body()),
        )
        .await;
        assert_eq!(s, 200, "{v}");
        v["notifier"].clone()
    }

    async fn panel(&self) -> Value {
        let (s, v) = call(&self.ts, &self.owner, Method::GET, &self.route(), None).await;
        assert_eq!(s, 200, "{v}");
        assert!(!v.to_string().contains(KEY), "{v}");
        v["notifier"].clone()
    }

    async fn file(&self, reporter: Option<ReporterProfile>) -> String {
        let page = "https://app.example.com/invoices/4821";
        self.file_at(reporter, page).await
    }

    async fn file_at(&self, reporter: Option<ReporterProfile>, page: &str) -> String {
        let tok = ReporterToken {
            queue_id: self.qid.clone(),
            service_grant_id: "g1".into(),
            reporter_id: "emp-1".into(),
            origin: "https://app.example.com".into(),
            public_key: String::new(),
            expires_at: "2099-01-01T00:00:00.000Z".into(),
            profile: reporter,
        };
        let r = NewReport {
            external_id: None,
            url: page.into(),
            title: "Invoice 4821".into(),
            anchor: serde_json::from_value(element_anchor()).unwrap(),
            body: REPORT_TEXT.into(),
            author_name: "Ana".into(),
            metadata: json!(null),
            revision: None,
            clip: None,
            context: None,
            server: None,
            diagnostics: None,
        };
        self.ts
            .store
            .file_report_touched_async(CTX, &self.qid, Some(&tok), ExternalScope::Instance, r)
            .await
            .unwrap()
            .0
            .id
    }

    fn propose(&self, tid: &str, by: &ReportWriter<'_>, fix: NewFix) {
        self.ts
            .store
            .propose_fix(CTX, &self.qid, tid, by, fix)
            .unwrap();
    }

    async fn deliver(&self) -> usize {
        notifiers::deliver_due(&self.ts.state).await
    }
}

fn person() -> ReportWriter<'static> {
    ReportWriter::Person {
        name: "Sam".into(),
        public_id: None,
    }
}

fn fix(verify_url: Option<&str>) -> NewFix {
    NewFix {
        summary: SUMMARY.into(),
        url: Some("https://github.com/acme/web/pull/7".into()),
        commit: None,
        release: None,
        verify_url: verify_url.map(str::to_string),
        verify_after: None,
    }
}

fn held_by(run: &str) -> ReportWriter<'static> {
    ReportWriter::Run {
        run: run.into(),
        name: "ClaxBot".into(),
        key: "k1".into(),
        note: Some("MODEL-WORDS for the members".into()),
        held: true,
        preview_url: None,
    }
}

fn ana() -> ReporterProfile {
    ReporterProfile {
        name: Some("Ana".into()),
        github: Some("ANA-dev".into()),
        ..ReporterProfile::default()
    }
}

/// `f` of a sent message, or empty.
fn field<'a>(g: &'a Got, f: &str) -> &'a str {
    g.form.get(f).map_or("", String::as_str)
}

#[tokio::test]
async fn a_queue_admin_alone_reads_and_sets_a_notifier_and_the_key_never_comes_back() {
    let w = world().await;
    let (s, v) = call(&w.ts, &w.sam, Method::GET, &w.route(), None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    let member = format!("/api/queues/{}/members/{}", w.qid, w.sam_public);
    let (s, _) = call(
        &w.ts,
        &w.owner,
        Method::PUT,
        &member,
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!(s, 204);
    let (s, v) = call(&w.ts, &w.sam, Method::GET, &w.route(), None).await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");
    let (s, v) = call(&w.ts, &w.owner, Method::GET, &w.route(), None).await;
    assert_eq!((s, &v["notifier"]), (200, &Value::Null), "{v}");

    let n = w.set().await;
    assert!(!n.to_string().contains(KEY), "{n}");
    assert_eq!(n["api_key"]["last4"], "wxyz", "{n}");
    assert_eq!(n["site"], w.site.as_str(), "{n}");
    assert_eq!(n["people"]["ana-dev"], "ana@acme.example.com", "{n}");

    // A stale write and a delete name the rev they were made from.
    let (s, v) = call(&w.ts, &w.owner, Method::PUT, &w.route(), Some(w.body())).await;
    assert_eq!((s, code(&v)), (400, "notifier_changed"), "{v}");
    let (s, v) = call(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &w.route(),
        Some(json!({"if_rev": n["rev"]})),
    )
    .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.panel().await, Value::Null);
}

#[tokio::test]
async fn another_origins_page_may_not_set_one() {
    let w = world().await;
    let r =
        w.ts.client
            .put(format!("{}{}", w.ts.base, w.route()))
            .header("cookie", &w.owner)
            .header("origin", "https://evil.example.com")
            .json(&w.body())
            .send()
            .await
            .unwrap();
    let status = r.status().as_u16();
    let v: Value = r.json().await.unwrap();
    // The owner's session with another origin is no credential at all: the
    // gate refuses it before any route.
    assert_eq!((status, code(&v)), (401, "no_credential"), "{v}");
    assert_eq!(w.panel().await, Value::Null);
}

#[tokio::test]
async fn a_reporter_zulip_knows_gets_a_direct_message_with_the_title_and_link() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    assert_eq!(w.deliver().await, 0, "told once");
    let got = w.zulip.got();
    assert_eq!(got.len(), 1, "{got:?}");
    let m = &got[0];
    assert_eq!(m.path, "/api/v1/messages");
    use base64::Engine as _;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{BOT}:{KEY}"));
    assert_eq!(
        m.authorization.as_deref(),
        Some(format!("Basic {basic}").as_str())
    );
    assert_eq!(field(m, "type"), "direct");
    assert_eq!(field(m, "to"), r#"["ana@acme.example.com"]"#);
    let content = field(m, "content");
    assert!(content.contains("The total is wrong"), "{content}");
    assert!(!content.contains("SECOND-LINE"), "{content}");
    assert!(content.contains("https://pr-7.acme.dev/"), "{content}");
    assert!(!content.contains("MODEL-WORDS"), "{content}");
    assert!(!content.contains("/pull/7"), "{content}");
    let page = "under My reports, where you filed it: https://app.example.com/invoices/4821";
    assert!(
        content.contains(page),
        "the page its overlay is on: {content}"
    );
    assert!(!content.contains("on the page"), "{content}");
    assert!(!content.contains(&w.report_link(&tid)), "{content}");
    assert!(w.panel().await["last_ok_at"].is_string());
}

#[tokio::test]
async fn a_report_filed_on_this_instance_is_answered_at_its_own_my_reports() {
    let w = world().await;
    w.set().await;
    let base = w.ts.state.browser_base.clone();
    let page = format!("{base}/a/AAAAAAAAAAAA");
    let tid = w.file_at(Some(ana()), &page).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    let content = field(&got[0], "content");
    let link = format!("under My reports, where you filed it: {base}/#my-reports");
    assert!(content.contains(&link), "{content}");
}

#[tokio::test]
async fn anyone_else_is_told_on_the_stream_in_fixed_words_with_a_link_to_the_report() {
    let w = world().await;
    w.set().await;
    let named = ReporterProfile {
        name: Some("SECRET-NAME".into()),
        ..ReporterProfile::default()
    };
    let tid = w.file(Some(named)).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    let anon = w.file(None).await;
    w.propose(&anon, &person(), fix(Some("https://pr-8.acme.dev/")));
    assert_eq!(w.deliver().await, 2);
    let got = w.zulip.got();
    let by_topic = |t: &str| got.iter().find(|g| field(g, "topic") == t).unwrap();
    for (id, preview) in [(&tid, "pr-7"), (&anon, "pr-8")] {
        let m = by_topic(id);
        assert_eq!(field(m, "type"), "stream");
        assert_eq!(field(m, "to"), "bugs");
        let content = field(m, "content");
        assert!(content.contains(&w.report_link(id)), "{content}");
        assert!(!content.contains(preview), "{content}");
        assert!(!content.contains("/pull/"), "{content}");
        assert!(!content.contains("total is wrong"), "{content}");
        assert!(!content.contains("MODEL-WORDS"), "{content}");
        assert!(!content.contains("SECRET-NAME"), "{content}");
    }
}

/// A confidential run's fix a reporter may check, its link there because a
/// person approves each deploy.
async fn confidential_fix(w: &World, reporter: Option<ReporterProfile>) -> String {
    let tid = w.file(reporter).await;
    let run = run_on(w, &tid);
    w.ts.store.mark_run_confidential(&run).unwrap();
    let by = ReportWriter::Run {
        run,
        name: "ClaxBot".into(),
        key: "k1".into(),
        note: None,
        held: false,
        preview_url: None,
    };
    w.propose(&tid, &by, fix(Some("https://pr-7.acme.dev/")));
    tid
}

#[tokio::test]
async fn a_confidential_fix_to_check_gives_the_stream_no_preview_link() {
    let w = world().await;
    w.set().await;
    let tid = confidential_fix(&w, None).await;
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(field(&got[0], "type"), "stream");
    let content = field(&got[0], "content");
    assert!(content.contains(&w.report_link(&tid)), "{content}");
    assert!(!content.contains("pr-7"), "{content}");
}

#[tokio::test]
async fn a_confidential_fix_to_check_gives_its_mapped_reporter_the_link_and_title() {
    let w = world().await;
    w.set().await;
    confidential_fix(&w, Some(ana())).await;
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(field(&got[0], "type"), "direct");
    let content = field(&got[0], "content");
    assert!(content.contains("https://pr-7.acme.dev/"), "{content}");
    assert!(content.contains("The total is wrong"), "{content}");
    assert!(!content.contains("MODEL-WORDS"), "{content}");
}

/// What an agent writes in `verify_url` reaches no mention or link processor.
#[tokio::test]
async fn an_agents_verify_url_mentions_nobody() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let url = "https://pr-7.acme.dev/@**all**/@*g*/[x](y)/#**s**";
    w.propose(&tid, &person(), fix(Some(url)));
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    let content = field(&got[0], "content");
    assert!(
        content.contains("https://pr-7.acme.dev/@%2A%2Aall"),
        "{content}"
    );
    for raw in ["@*", "**", "[x]", "](", "#*"] {
        assert!(!content.contains(raw), "{raw}: {content}");
    }
}

#[tokio::test]
async fn a_held_fix_tells_the_stream_what_a_person_must_do() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let sha = "0123456789abcdef0123456789abcdef01234567";
    // Longer than a report's line, and in the target's own markup.
    let step = format!(
        "To deploy its preview, ask in `#deploys` for `/go {sha}` on the pull request, \
         then wait for the check named `preview` to pass."
    );
    let links = FixLinks {
        url: Some("https://github.com/acme/web/pull/7".into()),
        commit: Some(sha.into()),
        preview_step: Some(step.clone()),
    };
    w.ts.store.keep_fix_links(&tid, "k1", &links).unwrap();
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    assert_eq!(w.deliver().await, 1, "at the proposal, with no wait ended");
    let got = w.zulip.got();
    let m = &got[0];
    assert_eq!(field(m, "type"), "stream", "never to a reporter");
    assert_eq!(field(m, "topic"), tid);
    let content = field(m, "content");
    assert!(
        content.contains("https://github.com/acme/web/pull/7"),
        "{content}"
    );
    assert!(content.contains(&step), "the target's words: {content}");
    assert!(
        content.contains("Nothing tells its reporter until the preview answers."),
        "{content}"
    );
    assert!(!content.contains("/preview"), "{content}");
    assert!(content.contains(&w.report_link(&tid)), "{content}");
    assert!(!content.contains("MODEL-WORDS"), "{content}");
}

/// What every message of a wait's end says.
const WAIT_OVER: &str = "without seeing one up.";
const REACHES_NOBODY: &str = "A preview that is up or deployed from now on reaches nobody";
const HAND_OVER: &str = "proposing one of their own with the preview's link";
const STILL_WAITING: &str = "Nothing tells its reporter until the preview answers.";

/// A held fix on report `tid` with a pull request and `step`, from a new run,
/// whose ID this answers.
fn held_fix(w: &World, tid: &str, step: &str) -> String {
    let links = FixLinks {
        url: Some("https://github.com/acme/web/pull/7".into()),
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        preview_step: Some(step.into()),
    };
    w.ts.store.keep_fix_links(tid, "k1", &links).unwrap();
    let run = run_on(w, tid);
    w.propose(tid, &held_by(&run), fix(None));
    run
}

/// Asserts the fixed words of a wait's end in `m`, sent to the stream alone
/// and holding nothing of the report's or the model's.
fn assert_wait_over(w: &World, tid: &str, m: &Got) {
    assert_eq!(field(m, "type"), "stream", "never to a reporter");
    assert_eq!(field(m, "topic"), tid);
    let content = field(m, "content");
    let report = w.report_link(tid);
    for words in [WAIT_OVER, REACHES_NOBODY, HAND_OVER, report.as_str()] {
        assert!(content.contains(words), "{words}: {content}");
    }
    assert!(!content.contains(STILL_WAITING), "{content}");
    for private in ["MODEL-WORDS", "total is wrong", "SECOND-LINE", "Ana"] {
        assert!(!content.contains(private), "{private}: {content}");
    }
}

#[tokio::test]
async fn a_wait_that_ends_after_the_held_message_went_tells_the_stream_once() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let step = "To deploy its preview, press go.";
    let run = held_fix(&w, &tid, step);
    assert_eq!(w.deliver().await, 1, "the held message");
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1, "the wait's end");
    let got = w.zulip.got();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_wait_over(&w, &tid, &got[1]);
    let content = field(&got[1], "content");
    assert!(content.contains(step), "{content}");
    assert!(content.contains("/pull/7"), "{content}");

    assert!(!w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 0, "a replay tells nobody twice");
    assert_eq!(w.zulip.got().len(), 2);
}

#[tokio::test]
async fn a_held_message_unsent_when_the_wait_ends_goes_as_the_waits_end_alone() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let step = "To deploy its preview, press go.";
    let run = held_fix(&w, &tid, step);
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_wait_over(&w, &tid, &got[0]);
    assert!(field(&got[0], "content").contains(step), "nothing is lost");
    assert_eq!(w.panel().await["pending"], 0);
}

/// Zulip is down from the proposal until after the wait's end: the held
/// message, tried and refused before it, is not tried again after it, and the
/// wait's end, tried and refused in its turn, goes once.
#[tokio::test]
async fn an_outage_across_the_waits_end_sends_its_notice_once_and_no_held_words() {
    let w = world().await;
    let n = w.set().await;
    let tid = w.file(None).await;
    let run = held_fix(&w, &tid, "To deploy its preview, press go.");
    w.zulip.answer(500);
    assert_eq!(w.deliver().await, 0);
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());

    let mut b = w.body();
    b["if_rev"] = n["rev"].clone();
    let (s, v) = call(&w.ts, &w.owner, Method::PUT, &w.route(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    w.zulip.answer(500);
    w.deliver().await;
    let tried = w.zulip.got().len();
    let n = w.panel().await;
    assert_eq!(n["pending"], 1, "the wait's end alone waits: {n}");

    let mut b = w.body();
    b["if_rev"] = n["rev"].clone();
    let (s, v) = call(&w.ts, &w.owner, Method::PUT, &w.route(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.deliver().await, 1);
    assert_eq!(w.deliver().await, 0);
    let got = w.zulip.got();
    assert_eq!(got.len(), tried + 1, "{got:?}");
    assert!(field(&got[0], "content").contains(STILL_WAITING), "{got:?}");
    for m in &got[1..] {
        assert_wait_over(&w, &tid, m);
    }
}

#[tokio::test]
async fn a_preview_that_answers_in_time_tells_of_no_waits_end() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let run = held_fix(&w, &tid, "To deploy its preview, press go.");
    assert_eq!(w.deliver().await, 1, "the held message");
    assert!(
        w.ts.store
            .set_fix_verify_url(CTX, &w.qid, &tid, &run, "https://pr-7.acme.dev/")
            .unwrap()
    );
    assert!(!w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1, "the fix to check");
    let got = w.zulip.got();
    assert_eq!(got.len(), 2, "{got:?}");
    for m in &got {
        assert!(!field(m, "content").contains(WAIT_OVER), "{m:?}");
    }
}

#[tokio::test]
async fn a_confidential_runs_waits_end_names_no_pull_request_and_no_step() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let run = held_fix(&w, &tid, "To deploy its preview, press go.");
    w.ts.store.mark_run_confidential(&run).unwrap();
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_wait_over(&w, &tid, &got[0]);
    let content = field(&got[0], "content");
    for named in [
        "/pull/7",
        "Pull request",
        "press go",
        "To deploy",
        "0123456",
    ] {
        assert!(!content.contains(named), "{named}: {content}");
    }
}

/// What the message says a member does, through the route it would take: a
/// fix of their own with the preview's link reaches the reporter, and a wait's
/// end not yet sent goes with the held fix.
#[tokio::test]
async fn a_hand_over_after_the_waits_end_reaches_the_reporter() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let run = held_fix(&w, &tid, "To deploy its preview, press go.");
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    let route = format!("/api/queues/{}/reports/{tid}/fix", w.qid);
    let (s, v) = call(
        &w.ts,
        &w.owner,
        Method::POST,
        &route,
        Some(json!({"summary": "Handed over", "verify_url": "https://pr-7.acme.dev/"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(field(&got[0], "type"), "direct", "{got:?}");
    assert!(field(&got[0], "content").contains("https://pr-7.acme.dev/"));
}

#[tokio::test]
async fn a_notifier_set_after_the_proposal_is_told_nothing_of_the_waits_end() {
    let w = world().await;
    let tid = w.file(None).await;
    let run = held_fix(&w, &tid, "To deploy its preview, press go.");
    // The stamps are to the millisecond, and one in the same counts as there.
    until("the clock has moved", || async {
        let stamp = w.ts.store.now();
        let proposed =
            w.ts.store
                .report(&w.qid, &tid, clax_store::queues::Visibility::All);
        (stamp > proposed.unwrap().unwrap().fixes[0].created_at).then_some(())
    })
    .await;
    w.set().await;
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 0);
    assert!(w.zulip.got().is_empty());
}

#[tokio::test]
async fn a_rejected_fix_tells_the_stream_in_fixed_words_with_no_reporters_text() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    let by = Verdict::Person {
        name: "Ana".into(),
        public_id: None,
    };
    let body = "REOPEN-WORDS it still adds up wrong".to_string();
    let store = &w.ts.store;
    let reopened = store.reopen_report(CTX, &w.qid, &tid, by, body, None, None, None);
    reopened.unwrap();
    assert_eq!(w.deliver().await, 1);
    assert_eq!(w.deliver().await, 0, "told once");
    let got = w.zulip.got();
    let m = &got[0];
    assert_eq!(field(m, "type"), "stream", "never to a person alone");
    assert_eq!(field(m, "topic"), tid);
    let content = field(m, "content");
    assert!(content.contains("was rejected"), "{content}");
    assert!(content.contains(&w.report_link(&tid)), "{content}");
    for held_back in ["REOPEN-WORDS", "The total is wrong", "MODEL-WORDS", "Ana"] {
        assert!(!content.contains(held_back), "{content}");
    }
}

#[tokio::test]
async fn a_confidential_runs_held_fix_names_no_link_and_no_commit() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let links = FixLinks {
        url: Some("https://github.com/acme/secret/pull/7".into()),
        commit: Some(sha.into()),
        preview_step: Some(format!("To deploy its preview, post `/go {sha}`.")),
    };
    w.ts.store.keep_fix_links(&tid, "k1", &links).unwrap();
    let run = run_on(&w, &tid);
    w.ts.store.mark_run_confidential(&run).unwrap();
    w.propose(&tid, &held_by(&run), fix(None));
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    let content = field(&got[0], "content");
    assert!(content.contains(&w.report_link(&tid)), "{content}");
    assert!(!content.contains("secret"), "{content}");
    assert!(!content.contains(sha), "{content}");
    assert!(!content.contains("To deploy"), "{content}");
}

#[tokio::test]
async fn a_hand_over_drops_the_held_notice_and_tells_of_the_link() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let run = run_on(&w, &tid);
    w.ts.store.mark_run_confidential(&run).unwrap();
    w.propose(&tid, &held_by(&run), fix(None));
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(!field(&got[0], "content").contains("held"), "{got:?}");
}

#[tokio::test]
async fn a_link_arriving_drops_the_held_notice() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let run = run_on(&w, &tid);
    w.ts.store.mark_run_confidential(&run).unwrap();
    w.propose(&tid, &held_by(&run), fix(None));
    assert!(
        w.ts.store
            .set_fix_verify_url(CTX, &w.qid, &tid, &run, "https://pr-7.acme.dev/")
            .unwrap()
    );
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(!field(&got[0], "content").contains("held"), "{got:?}");
}

#[tokio::test]
async fn a_second_preview_link_tells_the_reporter_again_with_that_link() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    let set = |url: &str| {
        w.ts.store
            .set_fix_verify_url(CTX, &w.qid, &tid, &run, url)
            .unwrap()
    };
    assert!(set("https://pr-7.acme.dev/"));
    assert_eq!(w.deliver().await, 1);
    assert!(set("https://pr-7b.acme.dev/"));
    assert_eq!(w.deliver().await, 1);
    assert!(
        !set("https://pr-7b.acme.dev/"),
        "the same link changes nothing"
    );
    assert_eq!(w.deliver().await, 0);
    let got = w.zulip.got();
    assert!(field(&got[1], "content").contains("pr-7b"), "{got:?}");

    // Changed before the first went, only the last is told.
    let later = w.file(Some(ana())).await;
    let run = run_on(&w, &later);
    let by = ReportWriter::Run {
        run: run.clone(),
        name: "ClaxBot".into(),
        key: "k2".into(),
        note: None,
        held: true,
        preview_url: None,
    };
    w.propose(&later, &by, fix(None));
    for url in ["https://one.acme.dev/", "https://two.acme.dev/"] {
        w.ts.store
            .set_fix_verify_url(CTX, &w.qid, &later, &run, url)
            .unwrap();
    }
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(
        field(&got[2], "content").contains("two.acme.dev"),
        "{got:?}"
    );
}

/// A target that says nothing of its previews gets no instruction of Clax's
/// own, and a commit is shown only inside what the target said.
#[tokio::test]
async fn a_target_that_names_no_step_gets_no_instruction() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let links = FixLinks {
        url: Some("https://github.com/acme/web/pull/7".into()),
        commit: Some(sha.into()),
        preview_step: None,
    };
    w.ts.store.keep_fix_links(&tid, "k1", &links).unwrap();
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_wait_over(&w, &tid, &got[0]);
    let content = field(&got[0], "content");
    assert!(content.contains("/pull/7"), "{content}");
    assert!(!content.contains("/preview"), "{content}");
    assert!(!content.contains("To deploy"), "{content}");
    assert!(!content.contains(sha), "{content}");
}

/// A pull request is not what every preview needs, so a step stands alone.
#[tokio::test]
async fn a_step_is_told_with_no_pull_request() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let links = FixLinks {
        preview_step: Some("To deploy its preview, run the deploy job.".into()),
        ..FixLinks::default()
    };
    w.ts.store.keep_fix_links(&tid, "k1", &links).unwrap();
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    let content = field(&got[0], "content");
    assert!(content.contains("run the deploy job"), "{content}");
}

/// A link no message can carry costs its reporter the direct message; the
/// stream is told as for anyone, and the panel says what happened.
#[tokio::test]
async fn a_fix_link_no_message_can_carry_shows_on_the_panel() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    w.propose(&tid, &person(), fix(Some("https://**all**.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(field(&got[0], "type"), "stream");
    assert!(!field(&got[0], "content").contains("**all**"), "{got:?}");
    let panel = w.panel().await;
    assert_eq!(panel["direct_failure"], notifiers::FIX_LINK_UNUSABLE);
    assert_eq!(panel["last_failure"], Value::Null, "the stream works");
}

/// Nothing is lost for someone the stream would have told anyway.
#[tokio::test]
async fn a_fix_link_no_message_can_carry_is_nothing_to_an_unmapped_reporter() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    w.propose(&tid, &person(), fix(Some("https://**all**.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    assert_eq!(w.panel().await["direct_failure"], Value::Null);
}

#[tokio::test]
async fn a_held_fixs_pull_request_no_message_can_carry_shows_on_the_panel() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let links = FixLinks {
        url: Some("https://**all**.acme.dev/pull/7".into()),
        ..FixLinks::default()
    };
    w.ts.store.keep_fix_links(&tid, "k1", &links).unwrap();
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    let content = field(&got[0], "content");
    assert!(content.contains(&w.report_link(&tid)), "{content}");
    assert!(!content.contains("Pull request"), "{content}");
    let panel = w.panel().await;
    assert_eq!(panel["last_failure"], notifiers::HELD_LINK_UNUSABLE);
    assert!(panel["last_ok_at"].is_string(), "the message went: {panel}");
}

/// A message that did not go says why, whatever its link: the notice is tried
/// again, so what became of the link is for the try that goes.
#[tokio::test]
async fn a_message_that_failed_keeps_its_own_words_on_the_panel() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    let links = FixLinks {
        url: Some("https://**all**.acme.dev/pull/7".into()),
        ..FixLinks::default()
    };
    w.ts.store.keep_fix_links(&tid, "k1", &links).unwrap();
    let run = run_on(&w, &tid);
    w.propose(&tid, &held_by(&run), fix(None));
    assert!(w.ts.store.set_fix_preview_missed(&tid, &run).unwrap());
    w.zulip.answer(401);
    assert_eq!(w.deliver().await, 0);
    assert_eq!(w.panel().await["last_failure"], notifiers::KEY_REFUSED);
}

/// A run of the owner's on report `tid`.
fn run_on(w: &World, tid: &str) -> String {
    use clax_store::bot_runs::{NewRun, RequestKind};
    let aid =
        w.ts.store
            .report(&w.qid, tid, clax_store::queues::Visibility::All)
            .unwrap()
            .unwrap()
            .artifact
            .id;
    w.ts.store
        .create_run(
            CTX,
            NewRun {
                artifact_id: aid,
                person: clax_store::roles::OWNER.into(),
                request: RequestKind::Thread,
                thread_ids: vec![tid.into()],
                model: "claude-sonnet-5-5".into(),
                key_source: clax_store::credits::KeySource::Instance,
                base_version: 1,
                agent_grant: None,
                web: String::new(),
            },
        )
        .unwrap()
        .id
}

#[tokio::test]
async fn a_notice_about_a_fix_that_moved_on_is_dropped_unsent() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    w.propose(&tid, &person(), fix(Some("https://pr-8.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    let got = w.zulip.got();
    assert_eq!(got.len(), 1);
    assert!(field(&got[0], "content").contains("pr-8"));
}

#[tokio::test]
async fn a_failure_shows_in_fixed_words_and_a_saved_key_sends_what_waited() {
    let w = world().await;
    let n = w.set().await;
    let tid = w.file(None).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    w.zulip.answer(401);
    assert_eq!(w.deliver().await, 0);
    let p = w.panel().await;
    assert_eq!(p["last_failure"], notifiers::KEY_REFUSED, "{p}");
    assert_eq!(p["pending"], 1, "{p}");
    assert!(!p.to_string().contains("leak"), "{p}");
    assert_eq!(w.deliver().await, 0, "it waits");
    assert_eq!(w.zulip.got().len(), 1);

    let mut b = w.body();
    b["api_key"] = json!("a-better-key-0000");
    b["if_rev"] = n["rev"].clone();
    let (s, v) = call(&w.ts, &w.owner, Method::PUT, &w.route(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.deliver().await, 1);
    let p = w.panel().await;
    assert_eq!(p["last_failure"], Value::Null, "{p}");
    assert_eq!(p["pending"], 0, "{p}");
}

#[tokio::test]
async fn a_failing_site_is_tried_once_a_pass_and_each_kind_of_failure_has_its_words() {
    for (status, words) in [
        (401, notifiers::KEY_REFUSED),
        (403, notifiers::KEY_REFUSED),
        (429, notifiers::RATE_LIMITED),
        (500, notifiers::UNREACHABLE),
        (302, notifiers::REDIRECTED),
    ] {
        let w = world().await;
        w.set().await;
        for _ in 0..3 {
            let tid = w.file(None).await;
            w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
        }
        w.zulip
            .answer_with(status, vec![("location", format!("{}/elsewhere", w.site))]);
        assert_eq!(w.deliver().await, 0);
        assert_eq!(
            w.zulip.got().len(),
            1,
            "{status}: no redirect followed, nor a second try"
        );
        let p = w.panel().await;
        assert_eq!(p["last_failure"], words, "{status}: {p}");
        assert_eq!(p["pending"], 3, "{status}: {p}");
    }
}

/// A refused message says nothing of the site: the rest of the pass goes.
#[tokio::test]
async fn a_refused_message_fails_that_notice_alone() {
    for status in [400, ZULIP_ERROR] {
        let w = world().await;
        w.set().await;
        for _ in 0..3 {
            let tid = w.file(None).await;
            w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
        }
        w.zulip.answer(status);
        assert_eq!(w.deliver().await, 2, "{status}");
        assert_eq!(w.zulip.got().len(), 3, "{status}");
        let p = w.panel().await;
        assert_eq!(p["pending"], 1, "{status}: {p}");
        assert!(!p.to_string().contains("leak"), "{p}");
    }
}

#[tokio::test]
async fn a_refused_direct_message_falls_back_to_the_stream_and_stays_on_the_panel() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    w.zulip.answer(400);
    assert_eq!(w.deliver().await, 1, "told, on the stream");
    let got = w.zulip.got();
    assert_eq!(field(&got[0], "type"), "direct");
    assert_eq!(field(&got[1], "type"), "stream");
    assert_eq!(field(&got[1], "topic"), tid);
    let content = field(&got[1], "content");
    assert!(content.contains(&w.report_link(&tid)), "{content}");
    assert!(!content.contains("pr-7"), "{content}");
    assert!(!content.contains("total is wrong"), "{content}");
    let p = w.panel().await;
    assert_eq!(p["pending"], 0, "{p}");
    assert_eq!(p["direct_failure"], notifiers::MESSAGE_REFUSED, "{p}");
    assert_eq!(p["last_failure"], Value::Null, "{p}");

    // A stream that works hides no failing direct messages.
    let other = w.file(None).await;
    w.propose(&other, &person(), fix(Some("https://pr-8.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    let p = w.panel().await;
    assert_eq!(p["direct_failure"], notifiers::MESSAGE_REFUSED, "{p}");

    // One that goes ends it.
    let third = w.file(Some(ana())).await;
    w.propose(&third, &person(), fix(Some("https://pr-9.acme.dev/")));
    assert_eq!(w.deliver().await, 1);
    assert_eq!(w.panel().await["direct_failure"], Value::Null);
}

/// A site that is down is no reason to tell a stream what a person's direct
/// message would have said: the notice waits.
#[tokio::test]
async fn a_direct_message_to_a_site_that_is_down_waits_with_no_fallback() {
    let w = world().await;
    w.set().await;
    let tid = w.file(Some(ana())).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    w.zulip.answer(500);
    assert_eq!(w.deliver().await, 0);
    assert_eq!(w.zulip.got().len(), 1);
    let p = w.panel().await;
    assert_eq!(p["pending"], 1, "{p}");
    assert_eq!(p["direct_failure"], notifiers::UNREACHABLE, "{p}");
}

#[tokio::test]
async fn an_answer_that_is_not_zulips_success_is_a_failure() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    w.zulip.answer(204);
    assert_eq!(w.deliver().await, 0);
    assert_eq!(w.panel().await["last_failure"], notifiers::UNREACHABLE);
}

/// `PublicOnly` is a resolver, and an IP literal never reaches one, so the
/// site is held to the rule again at every use.
#[tokio::test]
async fn a_send_is_refused_once_the_home_is_hosted() {
    use clax_store::bot_settings::BaseUrlPolicy;
    let w = world().await;
    let n = w.set().await;
    let tid = w.file(None).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    w.ts.store.set_base_url_policy(BaseUrlPolicy::Hosted);
    assert_eq!(w.deliver().await, 0);
    assert!(w.zulip.got().is_empty(), "nothing left for the site");
    let p = w.panel().await;
    assert_eq!(p["last_failure"], notifiers::SITE_REFUSED, "{p}");
    assert_eq!(p["pending"], 1, "{p}");
    let test = format!("{}/test", w.route());
    let body = json!({"if_rev": n["rev"]});
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body)).await;
    assert_eq!((s, code(&v)), (502, "notifier_failed"), "{v}");
    assert_eq!(v["error"]["message"], notifiers::SITE_REFUSED, "{v}");
    assert!(w.zulip.got().is_empty());
}

#[tokio::test]
async fn agents_service_grants_and_reporters_get_nothing_on_any_route() {
    let w = world().await;
    let n = w.set().await;
    let member = format!("/api/queues/{}/members/{}", w.qid, w.sam_public);
    let (s, _) = call(
        &w.ts,
        &w.owner,
        Method::PUT,
        &member,
        Some(json!({"role": "admin"})),
    )
    .await;
    assert_eq!(s, 204);
    let route = w.route();
    let test = format!("{route}/test");
    let rev = json!({"if_rev": n["rev"]});
    let routes = [
        ("GET", route.clone(), None),
        ("PUT", route.clone(), Some(w.body())),
        ("DELETE", route.clone(), Some(rev.clone())),
        ("POST", test, Some(rev)),
    ];
    let (s, g) = call(
        &w.ts,
        &w.owner,
        Method::POST,
        &format!("/api/queues/{}/service-grants", w.qid),
        Some(json!({"name": "backend"})),
    )
    .await;
    assert_eq!(s, 201, "{g}");
    let token = g["token"].as_str().unwrap();
    let grant =
        w.ts.store
            .service_grant_by_token(&clax_core::secret::sha256_hex(token))
            .unwrap()
            .unwrap();
    let key = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    let (reporter, _) =
        w.ts.store
            .mint_reporter_token(&grant, "r1", "https://app.example.com", &key, None)
            .unwrap();
    let reporter = reporter.expose();
    for (m, path, body) in routes {
        // A person's agent, though its person is an admin of the queue.
        let r = w.sam_client.agent(m, &path, None, body.clone()).await;
        assert_eq!(r.status().as_u16(), 403, "{m} {path}");
        // The owner's agent.
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let mut r =
            w.ts.authed(
                w.ts.client
                    .request(method.clone(), format!("{}{path}", w.ts.base)),
            )
            .header("x-clax-channel", "agent");
        if let Some(b) = &body {
            r = r.json(b);
        }
        let r = r.send().await.unwrap();
        let status = r.status().as_u16();
        let v: Value = r.json().await.unwrap();
        assert_eq!((status, code(&v)), (403, "not_for_agents"), "{m} {path}");
        // A service grant's token and a reporter's are not credentials here.
        for auth in [
            format!("Bearer {token}"),
            format!("Clax-Queue {token}"),
            format!("Bearer {reporter}"),
            format!("Clax-Reporter {reporter}"),
        ] {
            let mut r =
                w.ts.client
                    .request(method.clone(), format!("{}{path}", w.ts.base))
                    .header("authorization", auth);
            if let Some(b) = &body {
                r = r.json(b);
            }
            assert_eq!(r.send().await.unwrap().status().as_u16(), 401, "{m} {path}");
        }
    }
    assert!(w.zulip.got().is_empty(), "no refused caller sent anything");
    assert_eq!(w.panel().await["rev"], n["rev"], "nor changed anything");
}

#[tokio::test]
async fn no_audit_row_holds_the_key() {
    let w = world().await;
    let n = w.set().await;
    let test = format!("{}/test", w.route());
    let body = json!({"if_rev": n["rev"]});
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body.clone())).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(&w.ts, &w.owner, Method::DELETE, &w.route(), Some(body)).await;
    assert_eq!(s, 204, "{v}");
    let c = rusqlite::Connection::open(w.ts.home.db_path()).unwrap();
    let rows: Vec<String> = c
        .prepare("SELECT body FROM audit_events WHERE body LIKE '%notifier%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(rows.len() >= 4, "{rows:?}");
    assert!(rows.iter().all(|r| !r.contains(KEY)), "{rows:?}");
}

#[tokio::test]
async fn the_test_button_sends_to_the_stream_and_says_how_it_went() {
    let clock = Arc::new(clax_core::working::ManualClock::at("2026-11-06T12:00:00Z"));
    let ts = TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), clock.clone()).await;
    let w = world_of(ts).await;
    let test = format!("{}/test", w.route());
    let (s, v) = call(
        &w.ts,
        &w.owner,
        Method::POST,
        &test,
        Some(json!({"if_rev": "x"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "no_notifier"), "{v}");
    let n = w.set().await;
    let (s, v) = call(
        &w.ts,
        &w.owner,
        Method::POST,
        &test,
        Some(json!({"if_rev": "x"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "notifier_changed"), "{v}");
    let body = json!({"if_rev": n["rev"]});
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body.clone())).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["notifier"]["last_ok_at"].is_string(), "{v}");
    let got = w.zulip.got();
    assert_eq!(field(&got[0], "type"), "stream");
    assert_eq!(field(&got[0], "to"), "bugs");

    // A person's button is no way to flood a stream.
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body.clone())).await;
    assert_eq!((s, code(&v)), (429, "test_too_soon"), "{v}");
    assert_eq!(w.zulip.got().len(), 1);
    clock.advance(31);

    w.zulip.answer(401);
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body)).await;
    assert_eq!((s, code(&v)), (502, "notifier_failed"), "{v}");
    assert_eq!(v["error"]["message"], notifiers::KEY_REFUSED, "{v}");
    assert!(!v.to_string().contains("leak"), "{v}");
    assert_eq!(w.panel().await["last_failure"], notifiers::KEY_REFUSED);
}

/// A save is what a person tests next, so the wait between tests ends with it.
#[tokio::test]
async fn a_test_may_follow_a_save_at_once() {
    let w = world().await;
    let n = w.set().await;
    let test = format!("{}/test", w.route());
    let body = json!({"if_rev": n["rev"]});
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body)).await;
    assert_eq!(s, 200, "{v}");
    let mut changed = w.body();
    changed["if_rev"] = n["rev"].clone();
    changed["stream"] = json!("reports");
    let (s, v) = call(&w.ts, &w.owner, Method::PUT, &w.route(), Some(changed)).await;
    assert_eq!(s, 200, "{v}");
    let body = json!({"if_rev": v["notifier"]["rev"]});
    let (s, v) = call(&w.ts, &w.owner, Method::POST, &test, Some(body)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(field(&w.zulip.got()[1], "to"), "reports");
}

#[tokio::test]
async fn the_daemons_loop_delivers_what_is_due() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    let task = notifiers::spawn_delivery(w.ts.state.clone(), std::time::Duration::from_millis(10));
    let z = w.zulip.clone();
    until("the notice to go", || {
        let z = z.clone();
        async move { (z.got().len() == 1).then_some(()) }
    })
    .await;
    task.abort();
}

#[tokio::test]
async fn a_held_off_server_sets_the_next_try() {
    let w = world().await;
    w.set().await;
    let tid = w.file(None).await;
    w.propose(&tid, &person(), fix(Some("https://pr-7.acme.dev/")));
    let before = w.ts.store.now_utc();
    w.zulip
        .answer_with(429, vec![("retry-after", "1800".into())]);
    assert_eq!(w.deliver().await, 0);
    let next = w.ts.store.notices_next_due().unwrap().unwrap();
    assert!(next - before >= chrono::Duration::seconds(1700), "{next}");
}

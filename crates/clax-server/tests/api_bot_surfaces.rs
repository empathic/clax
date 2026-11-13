//! The ClaxBot surfaces host protocol over HTTP (`docs/contract.md`,
//! "ClaxBot surfaces"; `clax_server::routes::bot_surfaces`,
//! `clax_http::bot_surfaces`): who reaches the routes, registration and
//! its refusals, a `serve` held and answered by a clock the test drives,
//! runs started of a host's kind, and the contract fixture's script
//! replayed through a daemon.

use clax_bot::bot_surfaces::{CallFailure, Millis, Retry};
use clax_bot::broker::{CallBroker, Sending};
use clax_bot::clax_surface::wire::{
    Bound, Delivery, End, Notation, Outcome, RegisterRequest, RunSettings, ServeRequest, Settings,
    StartRunRequest,
};
use clax_bot::surface::{CallPlace, Claim};
use clax_core::audit::AuditCtx;
use clax_core::working::ManualClock;
use clax_http::bot_surfaces as hub;
use clax_store::bot_runs::{DriverId, StepCommit, StepKind};
use clax_testkit::fake_host::{
    Behaviour, FakeHost, HostTransport, HttpTransport, Names, ScriptWorld, Served, replay,
};
use clax_testkit::fake_messages::{FakeMessages, Turn};
use clax_testkit::remotes::json_of;
use clax_testkit::{ManualSleeper, TestServer};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FIXTURE: &str = include_str!("../../../docs/claxbot-surfaces/contract.json");
const KEY: &str = "sk-ant-api03-TOPSECRETKEYVALUE-abcd";
/// When a test's clock starts.
const START: &str = "2026-11-11T00:00:00Z";

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the fixture is JSON")
}

fn file(name: &str) -> String {
    fixture()["files"][name]
        .as_str()
        .expect("a file")
        .to_string()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// The runs a driver was told of, in order.
#[derive(Default)]
struct Told(Mutex<Vec<String>>);

impl clax_http::bot::Runs for Told {
    fn started(&self, run: &str) {
        self.0.lock().unwrap().push(run.to_string());
    }
}

impl Told {
    fn runs(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// A daemon whose clock and sleeps the test moves, and whose driver only
/// records what it is told.
struct Daemon {
    ts: TestServer,
    clock: Arc<ManualClock>,
    sleeper: Arc<ManualSleeper>,
    told: Arc<Told>,
}

fn told_bot(told: &Arc<Told>) -> Arc<clax_http::bot::Bot> {
    Arc::new(clax_http::bot::Bot {
        kek: Some(clax_store::secrets::Kek::from_bytes([7; 32])),
        runs: told.clone(),
        ..Default::default()
    })
}

async fn daemon_at(at: &str) -> Daemon {
    let clock = Arc::new(ManualClock::at(at));
    let sleeper = Arc::new(ManualSleeper::default());
    let told = Arc::new(Told::default());
    let (s, t) = (sleeper.clone(), told.clone());
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        state.bot_surfaces.set_sleeper(s);
        state.bot = told_bot(&t);
    })
    .await;
    Daemon {
        ts,
        clock,
        sleeper,
        told,
    }
}

async fn daemon() -> Daemon {
    daemon_at(START).await
}

impl Daemon {
    fn now(&self) -> Millis {
        hub::now(&self.ts.store)
    }

    /// Sets the instance key, so the owner's runs are not refused.
    async fn configured(&self) {
        let r = self
            .ts
            .client
            .put(format!("{}/api/admin/provider/key", self.ts.base))
            .header("cookie", self.ts.owner_cookie())
            .json(&json!({"key": KEY}))
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(r).await;
        assert_eq!(s, 200, "{v}");
    }

    /// A host of the fixture's `desk`, registered.
    async fn desk(&self) -> FakeHost<HttpTransport> {
        let mut host = FakeHost::new(
            HttpTransport::of(&self.ts),
            "desk",
            Notation::Dts,
            &file("desk.d.ts"),
        );
        host.register().await.expect("desk registers");
        host
    }

    /// A run of `desk.tidy` the host started, claimed by a driver of the
    /// test's with its first step open: one that may call its host.
    async fn running(&self, host: &mut FakeHost<HttpTransport>) -> Running {
        let id = host
            .start_run("tidy", json!({"ask": "Tidy the desk."}), None)
            .await
            .expect("the run starts");
        let driver = DriverId::mint();
        let st = &self.ts.store;
        st.claim_run(&id, &driver, 60).unwrap().expect("claimed");
        st.begin_step(&id, &driver, 1, StepKind::Script).unwrap();
        // What the driver was told of the start is not what a test of a
        // call counts.
        self.told.0.lock().unwrap().clear();
        Running {
            id,
            driver,
            seqs: BTreeMap::new(),
        }
    }

    /// A call of `run` for `registration`, sent through the daemon's
    /// broker under the run's claim, at the place its key has: the call's
    /// ID, or what its script is thrown.
    fn call(
        &self,
        registration: &str,
        run: &mut Running,
        op: &str,
        args: Value,
        key: &str,
        deadline: Option<Millis>,
    ) -> Result<String, CallFailure> {
        let next = i64::try_from(run.seqs.len()).unwrap();
        let seq = *run.seqs.entry(key.to_string()).or_insert(next);
        let st = &self.ts.store;
        let definition = st
            .registration(registration)
            .unwrap()
            .map(|r| r.def_hash)
            .unwrap_or_default();
        let place = CallPlace {
            step: 1,
            attempt: 1,
            seq,
            key: key.to_string(),
        };
        let sending = Sending {
            registration,
            definition: &definition,
            run: &run.id,
            op,
            args: args.as_array().map_or(&[], Vec::as_slice),
            raw: &args,
            place: &place,
            waits: false,
            deadline,
            retry: Retry::NONE,
            joined: None,
        };
        self.ts
            .state
            .bot_surfaces
            .broker()
            .send(&Claim::new(st, &run.driver, &run.id), &sending)
    }

    /// A read of the lamps with 30 s to answer, under a key of its own.
    fn lamps(&self, host: &FakeHost<HttpTransport>, run: &mut Running) -> String {
        let key = format!("{}:1:{}", run.id, run.seqs.len());
        let deadline = Some(self.now().after(30_000));
        self.call(
            host.registration().unwrap(),
            run,
            "desk.lamps",
            json!([]),
            &key,
            deadline,
        )
        .expect("the call is opened")
    }

    /// What the run's journal holds for the call: whether it is a value,
    /// and the value or the failure. `None` while the call waits.
    fn outcome(&self, call: &str) -> Option<(bool, Value)> {
        let st = &self.ts.store;
        let row = st.pending_call(call).unwrap().expect("the call is kept");
        row.answered_at.as_ref()?;
        st.bot_calls(&row.run_id, row.step_n)
            .unwrap()
            .into_iter()
            .find(|e| e.seq() == row.seq)
            .map(|e| (e.ok(), e.result().clone()))
    }

    fn advance(&self, ms: u64) {
        self.clock.advance_millis(i64::try_from(ms).unwrap());
    }
}

/// A run a test drives by hand.
struct Running {
    id: String,
    driver: DriverId,
    /// The place of each key among the step's calls.
    seqs: BTreeMap<String, i64>,
}

fn serve_request(registration: &str, wait_ms: u64) -> ServeRequest {
    ServeRequest {
        registration: registration.to_string(),
        wait_ms,
        answers: Vec::new(),
    }
}

fn calls_of(body: &Value) -> Vec<Delivery> {
    serde_json::from_value(body["calls"].clone()).expect("deliveries")
}

// ---------------------------------------------------------------------------
// Who may
// ---------------------------------------------------------------------------

/// Every route of the protocol, with a body it would take.
fn routes() -> Vec<(Method, &'static str, Value)> {
    let register = json!({"clax_surface": 1, "notation": "dts", "definition": file("desk.d.ts")});
    vec![
        (Method::GET, "/api/bot/surfaces", Value::Null),
        (Method::PUT, "/api/bot/surfaces/desk", register),
        (
            Method::POST,
            "/api/bot/surfaces/desk/serve",
            json!({"registration": "0".repeat(32), "wait_ms": 0}),
        ),
        (
            Method::POST,
            "/api/bot/surfaces/desk/runs",
            json!({"registration": "0".repeat(32), "kind": "tidy", "request": {"ask": "x"}}),
        ),
        (
            Method::PUT,
            "/api/bot/surfaces/desk/content?registration=00000000000000000000000000000000&run=r1",
            Value::Null,
        ),
        (
            Method::GET,
            "/api/bot/surfaces/desk/content/c1?registration=00000000000000000000000000000000&run=r1",
            Value::Null,
        ),
        (
            Method::GET,
            "/api/bot/surfaces/desk/runs/r1?registration=00000000000000000000000000000000",
            Value::Null,
        ),
        (
            Method::POST,
            "/api/bot/surfaces/desk/runs/r1/messages",
            json!({"registration": "0".repeat(32), "message": {"say": "x"}}),
        ),
        (
            Method::PUT,
            "/api/bot/surfaces/desk/joins",
            json!({"registration": "0".repeat(32), "clax": []}),
        ),
    ]
}

#[tokio::test]
async fn only_the_daemons_token_reaches_the_routes() {
    let ts = TestServer::spawn_on(
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        |_| {},
    )
    .await;
    let person = ts.person("Ana").await;
    let viewer = ts.viewer(Some("Lou")).await;
    let agent = ts.connect_person("Sam", "clax:collaborator").await;
    for (method, path, body) in routes() {
        let send = |r: reqwest::RequestBuilder| {
            let body = body.clone();
            async move {
                let r = if body.is_null() { r } else { r.json(&body) };
                json_of(r.send().await.unwrap()).await
            }
        };
        let req = || {
            ts.client
                .request(method.clone(), format!("{}{path}", ts.base))
        };
        // No credential: the daemon's own gate, before the route.
        let (s, v) = send(req()).await;
        assert_eq!(
            (s, code(&v)),
            (401, "no_credential"),
            "{method} {path}: {v}"
        );
        // The owner's browser, a connected person's and a viewer's.
        for cookie in [
            ts.owner_cookie(),
            format!("clax_viewer={}", person.cookie),
            format!("clax_viewer={}", viewer.cookie),
        ] {
            let (s, v) = send(req().header("cookie", &cookie)).await;
            assert_eq!((s, code(&v)), (401, "unauthorized"), "{method} {path}: {v}");
        }
        // A person's agent of another machine, whatever its rights.
        let r = agent
            .agent(
                method.as_str(),
                path,
                None,
                (!body.is_null()).then(|| body.clone()),
            )
            .await;
        let (s, v) = json_of(r).await;
        assert_eq!((s, code(&v)), (403, "forbidden"), "{method} {path}: {v}");
        // The token: by hand, and as the owner's agent.
        for marked in [false, true] {
            let r = ts.authed(req());
            let r = if marked {
                r.header("x-clax-channel", "agent")
            } else {
                r
            };
            let (s, v) = send(r).await;
            assert!(
                s != 401 && s != 403,
                "{method} {path} (agent marker: {marked}): {s} {v}"
            );
        }
    }
}

#[tokio::test]
async fn a_hosted_daemon_has_no_such_routes() {
    let clock = Arc::new(ManualClock::at(START));
    let ts = TestServer::spawn_hosted(clock, Some("setup-secret-for-tests")).await;
    for (method, path, body) in routes() {
        let send = |path: String| {
            let r = ts.authed(
                ts.client
                    .request(method.clone(), format!("{}{path}", ts.base)),
            );
            let r = if body.is_null() { r } else { r.json(&body) };
            async move { json_of(r.send().await.unwrap()).await }
        };
        let (s, v) = send(path.to_string()).await;
        // As a path the daemon has no route for is answered.
        let (none, _) = send(format!("{path}/no-such-route")).await;
        assert_eq!(s, none, "{method} {path}: {v}");
        assert!(s == 404 || s == 401, "{method} {path}: {s} {v}");
        assert!(v.get("registration").is_none() && v.get("surfaces").is_none());
    }
}

// ---------------------------------------------------------------------------
// Registering
// ---------------------------------------------------------------------------

async fn put(ts: &TestServer, path: &str, body: &Value) -> (u16, Value) {
    let r = ts.authed(ts.client.put(format!("{}{path}", ts.base)).json(body));
    json_of(r.send().await.unwrap()).await
}

async fn post(ts: &TestServer, path: &str, body: &Value) -> (u16, Value) {
    let r = ts.authed(ts.client.post(format!("{}{path}", ts.base)).json(body));
    json_of(r.send().await.unwrap()).await
}

async fn get(ts: &TestServer, path: &str) -> (u16, Value) {
    json_of(ts.get_authed(path).await).await
}

#[tokio::test]
async fn a_surface_registers_in_either_notation_and_is_listed() {
    let d = daemon().await;
    let ts = &d.ts;
    let dts = json!({"clax_surface": 1, "notation": "dts", "definition": file("desk.d.ts")});
    let (s, a) = put(ts, "/api/bot/surfaces/desk", &dts).await;
    assert_eq!(s, 200, "{a}");
    d.advance(1);
    let manifest = json!({"clax_surface": 1, "notation": "manifest",
        "definition": file("desk.json"), "prompt": file("desk.prompt.md"),
        "settings": {"allow": ["desk.*"]}});
    let (s, b) = put(ts, "/api/bot/surfaces/desk", &manifest).await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(a["hash"], b["hash"]);
    assert_ne!(a["registration"], b["registration"]);
    for r in [&a, &b] {
        let id = r["registration"].as_str().unwrap();
        assert!(clax_bot::bot_surfaces::is_registration_id(id), "{id}");
        // A daemon that keeps content withholds no operation.
        assert_eq!(r.get("withheld"), None);
    }
    // The store keeps who registered it, the prompt and the settings.
    let row = ts
        .store
        .registration(b["registration"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row.registered_by, clax_store::roles::OWNER);
    assert_eq!(row.prompt.as_deref(), Some(file("desk.prompt.md").as_str()));
    assert_eq!(row.settings, json!({"allow": ["desk.*"]}));

    // Listed, the oldest first; served once, the first says when.
    let (s, v) = post(
        ts,
        "/api/bot/surfaces/desk/serve",
        &json!({"registration": a["registration"], "wait_ms": 0}),
    )
    .await;
    assert_eq!((s, &v), (200, &json!({"calls": []})));
    let (s, v) = get(ts, "/api/bot/surfaces").await;
    assert_eq!(s, 200, "{v}");
    let listed = v["surfaces"].as_array().unwrap();
    assert_eq!(listed.len(), 2, "{v}");
    assert_eq!(
        listed[0],
        json!({"name": "desk", "registration": a["registration"], "hash": a["hash"],
            "connected": true, "withheld": [],
            "registered_at": "2026-11-11T00:00:00.000Z",
            "last_served_at": "2026-11-11T00:00:00.001Z", "joins": {"clax": []}})
    );
    assert_eq!(listed[1]["registration"], b["registration"]);
    assert_eq!(listed[1]["last_served_at"], Value::Null);

    // Not served for 15 s: not connected. For five minutes more: gone.
    d.advance(15_001);
    let (_, v) = get(ts, "/api/bot/surfaces").await;
    assert_eq!(v["surfaces"][0]["connected"], false, "{v}");
    d.advance(300_000);
    let (_, v) = get(ts, "/api/bot/surfaces").await;
    assert_eq!(v["surfaces"], json!([]));
}

#[tokio::test]
async fn a_registration_is_refused_with_its_code_and_status() {
    let d = daemon().await;
    let ts = &d.ts;
    let desk = file("desk.d.ts");
    let ok = |extra: Value| {
        let mut v = json!({"clax_surface": 1, "notation": "dts", "definition": desk});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    for (name, body, status, want) in [
        (
            "desk",
            ok(json!({"clax_surface": 2})),
            400,
            "unsupported_version",
        ),
        ("shelf", ok(json!({})), 400, "name_mismatch"),
        (
            "desk",
            ok(json!({"settings": {"allow": ["no good"]}})),
            400,
            "invalid_settings",
        ),
        (
            "desk",
            ok(json!({"prompt": "Keep it tidy.\n"})),
            400,
            "invalid_prompt",
        ),
        (
            "desk",
            ok(json!({"prompt": "x".repeat(64 * 1024 + 1)})),
            400,
            "prompt_too_large",
        ),
        (
            "desk",
            ok(json!({"registration": "0".repeat(32)})),
            404,
            "unknown_registration",
        ),
        // Not the message: a member it does not have, and no JSON at all.
        ("desk", ok(json!({"promt": "x"})), 400, "invalid_request"),
        ("desk", json!("declare const desk"), 400, "invalid_request"),
        // A name of Clax's own.
        (
            "clax",
            ok(json!({"definition": desk.replace("const desk", "const clax")})),
            400,
            "invalid_definition",
        ),
    ] {
        let (s, v) = put(ts, &format!("/api/bot/surfaces/{name}"), &body).await;
        assert_eq!((s, code(&v)), (status, want), "{v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty())
        );
    }
    let (_, v) = put(
        ts,
        "/api/bot/surfaces/desk",
        &ok(json!({"clax_surface": 2})),
    )
    .await;
    assert_eq!(v["error"]["supported"], json!([1]));

    // A definition's mistakes come with their lines, or their pointers.
    let (s, v) = put(
        ts,
        "/api/bot/surfaces/desk",
        &ok(json!({"definition": "declare const desk: {\n  lamps(): Promise<Lamp[]>;\n};\n"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_definition"));
    assert_eq!(v["error"]["errors"][0]["line"], 2, "{v}");
    let (s, v) = put(
        ts,
        "/api/bot/surfaces/desk",
        &json!({"clax_surface": 1, "notation": "manifest", "definition":
            "{\"clax_surface\": 1, \"name\": \"desk\", \"operations\": [{\"name\": \"lamps\", \"access\": \"read\", \"result\": {}}]}"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_definition"));
    assert_eq!(v["error"]["errors"][0]["pointer"], "/operations/0", "{v}");

    // A host's kind says what its runs may call: one without `@allow` is
    // refused with the checker's words.
    let no_allow = desk.replace(" @allow desk.*", "");
    assert_ne!(no_allow, desk);
    let (s, v) = put(
        ts,
        "/api/bot/surfaces/desk",
        &ok(json!({"definition": no_allow})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_definition"), "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("@allow"),
        "{v}"
    );

    // Nothing was registered.
    let (_, v) = get(ts, "/api/bot/surfaces").await;
    assert_eq!(v["surfaces"], json!([]));
    assert!(
        ts.store
            .registrations_by(clax_store::roles::OWNER, true)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_body_over_the_limit_is_too_large() {
    let d = daemon().await;
    let ts = &d.ts;
    let limit = ts.state.bot_surfaces.limits().request_bytes;
    let long = "x".repeat(limit);
    for (method, path, body) in [
        (
            Method::PUT,
            "/api/bot/surfaces/desk",
            json!({"clax_surface": 1, "notation": "dts", "definition": long}),
        ),
        (
            Method::POST,
            "/api/bot/surfaces/desk/serve",
            json!({"registration": "0".repeat(32), "wait_ms": 0,
                "answers": [{"id": "c", "result": long}]}),
        ),
        (
            Method::POST,
            "/api/bot/surfaces/desk/runs",
            json!({"registration": "0".repeat(32), "kind": "tidy", "request": {"ask": long}}),
        ),
    ] {
        let r = ts.authed(
            ts.client
                .request(method.clone(), format!("{}{path}", ts.base))
                .json(&body),
        );
        let (s, v) = json_of(r.send().await.unwrap()).await;
        assert_eq!((s, code(&v)), (413, "too_large"), "{method} {path}: {v}");
    }
}

#[tokio::test]
async fn a_new_daemon_on_the_home_goes_on_as_a_registration_with_its_calls() {
    // The second daemon reads the system's clock: the first one's is
    // ahead of it, so the call's deadline has not come there either.
    let d = daemon_at("2099-01-01T00:00:00Z").await;
    d.configured().await;
    let mut host = d.desk().await;
    let id = host.registration().unwrap().to_string();
    let mut run = d.running(&mut host).await;
    let call = d.lamps(&host, &mut run);

    let told = Arc::new(Told::default());
    let t = told.clone();
    let again = d.ts.reopened_with(move |s| s.bot = told_bot(&t)).await;
    // The registration was a lease, and ended with the process.
    let (s, v) = post(
        &again,
        "/api/bot/surfaces/desk/serve",
        &json!({"registration": id, "wait_ms": 0}),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "unknown_registration"));
    let (_, v) = get(&again, "/api/bot/surfaces").await;
    assert_eq!(v["surfaces"], json!([]));
    // Its host registers as it again, and has its call.
    let mut host = FakeHost::new(
        HttpTransport::of(&again),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .resume(&id)
    .on(
        "lamps",
        Behaviour::result(json!([{"id": "l1", "on": true}])),
    );
    let registered = host.register().await.expect("it goes on");
    assert_eq!(registered.registration, id);
    // This daemon's clock runs. A serve that waits for nothing is sent
    // the call; one that is held, the call being nothing new to its host,
    // is sent it again when its wait is over, however little later the
    // clock then reads.
    let transport = HttpTransport::of(&again);
    for wait_ms in [0, 1] {
        let (s, v) = transport.serve("desk", &serve_request(&id, wait_ms)).await;
        assert_eq!(s, 200, "{v}");
        let calls = calls_of(&v);
        assert_eq!(calls.len(), 1, "wait_ms {wait_ms}: {v}");
        assert_eq!(calls[0].id(), Some(call.as_str()));
    }
    let Served::Delivered(calls) = host.serve_once().await else {
        panic!("not served")
    };
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id(), Some(call.as_str()));
    host.serve_once().await;
    let row = again.store.pending_call(&call).unwrap().unwrap();
    assert!(row.answered_at.is_some());
    let entry = &again.store.bot_calls(&run.id, 1).unwrap()[0];
    assert_eq!(
        (entry.ok(), entry.result()),
        (true, &json!([{"id": "l1", "on": true}]))
    );
    assert_eq!(told.runs(), vec![run.id.clone()]);
}

// ---------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_call_is_delivered_until_accepted_and_its_answer_is_journaled() {
    let d = daemon().await;
    d.configured().await;
    let mut host = d.desk().await.on(
        "lamps",
        Behaviour::Accept {
            when: clax_testkit::fake_host::When::Rounds(1),
            then: Outcome::Result(json!([{"id": "l1", "on": false}])),
        },
    );
    let mut run = d.running(&mut host).await;
    let other = d.desk().await;
    let call = d.lamps(&host, &mut run);

    // Sent with every serve until its host says it has it.
    let transport = HttpTransport::of(&d.ts);
    let id = host.registration().unwrap().to_string();
    for _ in 0..2 {
        let (s, v) = transport.serve("desk", &serve_request(&id, 0)).await;
        assert_eq!(s, 200, "{v}");
        let calls = calls_of(&v);
        assert_eq!(calls.len(), 1, "{v}");
        assert_eq!(calls[0].id(), Some(call.as_str()));
        assert_eq!(v["calls"][0]["key"], format!("{}:1:0", run.id));
        assert_eq!(v["calls"][0]["op"], "lamps");
    }
    // Another registration of the name is not sent it, and its answer
    // for it is ignored.
    let mut wrong = serve_request(other.registration().unwrap(), 0);
    wrong.answers = serde_json::from_value(json!([{"id": call, "result": []}])).unwrap();
    let (s, v) = transport.serve("desk", &wrong).await;
    assert_eq!((s, &v), (200, &json!({"calls": []})));
    assert_eq!(d.outcome(&call), None);
    assert!(d.told.runs().is_empty());

    // The host: delivered, accepted (and so not sent again), answered.
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.len() == 1));
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.is_empty()));
    assert_eq!(d.outcome(&call), None);
    let row = d.ts.store.pending_call(&call).unwrap().unwrap();
    assert!(row.accepted_at.is_some() && row.sent_at.is_some());
    host.serve_once().await;
    host.serve_once().await;
    assert_eq!(
        d.outcome(&call),
        Some((true, json!([{"id": "l1", "on": false}])))
    );
    // The run's driver was told, once.
    assert_eq!(d.told.runs(), vec![run.id.clone()]);
    // An answer for a settled call changes nothing.
    host.answer(&call, Outcome::Result(json!([])));
    host.serve_once().await;
    assert_eq!(
        d.outcome(&call),
        Some((true, json!([{"id": "l1", "on": false}])))
    );
    assert_eq!(d.told.runs().len(), 1);
}

#[tokio::test]
async fn an_answer_after_the_deadline_is_ignored_and_the_call_expires() {
    let d = daemon().await;
    d.configured().await;
    let mut host = d.desk().await;
    let mut run = d.running(&mut host).await;
    let call = d.lamps(&host, &mut run);
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.len() == 1));
    d.advance(30_000);
    host.answer(&call, Outcome::Result(json!([])));
    // 30 s without a serve: the registration is not connected, and is
    // still held.
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.is_empty()));
    let (ok, failure) = d.outcome(&call).expect("settled");
    assert!(!ok);
    assert_eq!(failure["code"], "expired", "{failure}");
    assert_eq!(d.told.runs(), vec![run.id.clone()]);
}

#[tokio::test]
async fn a_held_serve_is_answered_when_a_call_arrives() {
    let d = daemon().await;
    d.configured().await;
    let mut host = d.desk().await;
    let mut run = d.running(&mut host).await;
    let id = host.registration().unwrap().to_string();
    let transport = HttpTransport::of(&d.ts);
    let held = {
        let (t, id) = (transport.clone(), id.clone());
        tokio::spawn(async move { t.serve("desk", &serve_request(&id, 60_000)).await })
    };
    // It waits for the longest a serve is held, whatever it asked for.
    let wait = Duration::from_millis(12_000);
    d.sleeper.wait_for(wait, 1).await;
    let call = d.lamps(&host, &mut run);
    let (s, v) = held.await.unwrap();
    assert_eq!(s, 200, "{v}");
    assert_eq!(calls_of(&v)[0].id(), Some(call.as_str()));
    assert_eq!(d.sleeper.waiting(wait), 0);

    // With nothing to send it is answered empty when its wait is over.
    let mut accept = serve_request(&id, 5_000);
    accept.answers = serde_json::from_value(json!([{"id": call, "accepted": true}])).unwrap();
    let held = {
        let t = transport.clone();
        tokio::spawn(async move { t.serve("desk", &accept).await })
    };
    let wait = Duration::from_millis(5_000);
    d.sleeper.wait_for(wait, 1).await;
    d.advance(5_000);
    assert_eq!(d.sleeper.fire(wait), 1);
    let (s, v) = held.await.unwrap();
    assert_eq!((s, &v), (200, &json!({"calls": []})));
}

#[tokio::test]
async fn one_serve_more_than_a_registration_may_hold_ends_the_oldest() {
    let d = daemon().await;
    let host = d.desk().await;
    let id = host.registration().unwrap().to_string();
    let transport = HttpTransport::of(&d.ts);
    let wait = Duration::from_millis(12_000);
    let mut held = Vec::new();
    for n in 1..=4 {
        let (t, id) = (transport.clone(), id.clone());
        held.push(tokio::spawn(async move {
            t.serve("desk", &serve_request(&id, 12_000)).await
        }));
        d.sleeper.wait_for(wait, n).await;
    }
    let (t, fifth) = (transport.clone(), id.clone());
    let fifth = tokio::spawn(async move { t.serve("desk", &serve_request(&fifth, 12_000)).await });
    // The first is answered, with nothing, and four are held again.
    let (s, v) = held.remove(0).await.unwrap();
    assert_eq!((s, &v), (200, &json!({"calls": []})));
    d.sleeper.wait_for(wait, 4).await;
    // The rest end with their wait.
    d.advance(12_000);
    assert_eq!(d.sleeper.fire(wait), 4);
    for h in held.into_iter().chain([fifth]) {
        let (s, v) = h.await.unwrap();
        assert_eq!((s, &v), (200, &json!({"calls": []})));
    }
}

#[tokio::test]
async fn the_timer_settles_a_call_at_its_deadline_and_tells_its_runs_driver() {
    let d = daemon().await;
    d.configured().await;
    let mut host = d.desk().await;
    let mut run = d.running(&mut host).await;
    let timer = tokio::spawn(hub::run_timer(d.ts.state.clone()));
    let mut passes = d.ts.state.bot_surfaces.passes();
    let key = format!("{}:1:0", run.id);
    let deadline = Some(d.now().after(1_000));
    let call = d
        .call(
            host.registration().unwrap(),
            &mut run,
            "desk.lamps",
            json!([]),
            &key,
            deadline,
        )
        .unwrap();
    // It sleeps until the call's deadline, and no longer.
    let wait = Duration::from_millis(1_000);
    d.sleeper.wait_for(wait, 1).await;
    assert_eq!(d.outcome(&call), None);
    d.advance(1_000);
    assert_eq!(d.sleeper.fire(wait), 1);
    passes.changed().await.unwrap();
    let (ok, failure) = d.outcome(&call).expect("settled");
    assert!(!ok);
    // Never sent: nothing happened.
    assert_eq!(
        (failure["code"].as_str(), failure["retryable"].as_bool()),
        (Some("unavailable"), Some(true))
    );
    assert_eq!(d.told.runs(), vec![run.id.clone()]);
    timer.abort();
}

// ---------------------------------------------------------------------------
// Starting a run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_run_of_a_hosts_kind_is_made_with_its_operations_and_no_artifact() {
    let d = daemon().await;
    d.configured().await;
    let ts = &d.ts;
    let mut host = d.desk().await;
    let reg = host.registration().unwrap().to_string();
    let hash = host.registered().unwrap().hash.clone();
    let id = host
        .start_run("tidy", json!({"ask": "Tidy the desk."}), None)
        .await
        .expect("the run starts");
    let (run, made, steps) = ts.store.run_with_steps_as_made(&id).unwrap().unwrap();
    assert_eq!(run.status.as_str(), "queued");
    assert_eq!(made.kind, "desk.tidy");
    assert_eq!(run.artifact(), None);
    assert_eq!(run.person, clax_store::roles::OWNER);
    assert!(steps.is_empty());
    assert_eq!(
        serde_json::from_str::<Value>(&made.request_json).unwrap(),
        json!({"ask": "Tidy the desk."})
    );
    // Everything the kind allows (an operation that names content too,
    // on a daemon that keeps it), and no binding of Clax's own surface.
    assert_eq!(
        serde_json::from_str::<Value>(&made.bindings_json).unwrap(),
        json!({"v": 2, "bindings": [{"surface": "desk", "hash": hash, "registration": reg,
            "settings": {}, "scope": null,
            "operations": ["desk.lamps", "desk.snapshot", "desk.settings.set", "desk.notify",
                           "desk.ask"]}]})
    );
    let started: Vec<String> = ts
        .store
        .runs_started_by(&reg, 10)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(started, vec![id.clone()]);
    // The driver was told.
    assert_eq!(d.told.runs(), vec![id.clone()]);

    // The starter narrows what the run has.
    let narrow = host
        .start_run(
            "tidy",
            json!({"ask": "Look."}),
            Some(vec!["desk.lamps".into(), "desk.snapshot".into()]),
        )
        .await
        .unwrap();
    let (_, made, _) = ts.store.run_with_steps_as_made(&narrow).unwrap().unwrap();
    let bindings: Value = serde_json::from_str(&made.bindings_json).unwrap();
    assert_eq!(
        bindings["bindings"][0]["operations"],
        json!(["desk.lamps", "desk.snapshot"])
    );

    // And so does the registration's `allow`.
    let mut limited = FakeHost::new(
        HttpTransport::of(ts),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .with_registration(
        None,
        Settings {
            allow: Some(vec!["desk.settings.*".into()]),
            ..Settings::default()
        },
    );
    limited.register().await.unwrap();
    let run = limited
        .start_run("tidy", json!({"ask": "Set."}), None)
        .await
        .unwrap();
    let (_, made, _) = ts.store.run_with_steps_as_made(&run).unwrap().unwrap();
    let bindings: Value = serde_json::from_str(&made.bindings_json).unwrap();
    assert_eq!(
        bindings["bindings"][0]["operations"],
        json!(["desk.settings.set"])
    );
    // `allow` is in the operations, and not kept again with the settings.
    assert_eq!(bindings["bindings"][0]["settings"], json!({}));

    // Its person reads it with the token, and stops it.
    let (s, v) = get(ts, &format!("/api/bot/runs/{id}")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["artifact_id"], Value::Null);
    assert_eq!(
        (&v["request"], &v["status"], &v["thread_ids"]),
        (&json!("other"), &json!("queued"), &json!([]))
    );
    assert_eq!(v["acting_for"]["id"], clax_store::roles::OWNER);
    let (s, v) = post(ts, &format!("/api/bot/runs/{id}/cancel"), &Value::Null).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["run"]["status"], "stopping");
    assert_eq!(v["run"]["artifact_id"], Value::Null);
    // The cancel is filed on no artifact.
    let cancels: Vec<_> = ts
        .store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "bot.cancel")
        .collect();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0].ids.artifact, None);
}

/// A run keeps, in its binding, the settings its registration had when
/// it was started, all but `allow`: a host that goes on as the
/// registration with others changes them for the runs it starts
/// afterwards alone.
#[tokio::test]
async fn a_run_keeps_its_registrations_settings_as_they_were_when_it_started() {
    let d = daemon().await;
    d.configured().await;
    let ts = &d.ts;
    let settings_of = |run: &str| -> Value {
        let (_, made, _) = ts.store.run_with_steps_as_made(run).unwrap().unwrap();
        serde_json::from_str::<Value>(&made.bindings_json).unwrap()["bindings"][0]["settings"]
            .clone()
    };
    let loose = Settings {
        allow: Some(vec!["desk.*".into()]),
        max_effects_per_run: Some(Bound::At(3)),
        call_timeout_s: Some(Bound::At(20)),
        runs: Some(RunSettings {
            max_steps: Some(Bound::At(10)),
            max_run_minutes: Some(Bound::None),
        }),
        ..Settings::default()
    };
    let mut host = FakeHost::new(
        HttpTransport::of(ts),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .with_registration(None, loose);
    host.register().await.unwrap();
    let reg = host.registration().unwrap().to_string();
    let early = host
        .start_run("tidy", json!({"ask": "Tidy."}), None)
        .await
        .unwrap();
    let tight = Settings {
        call_timeout_s: Some(Bound::At(5)),
        wait_limit_s: Some(Bound::At(60)),
        ..Settings::default()
    };
    let mut again = FakeHost::new(
        HttpTransport::of(ts),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .with_registration(None, tight)
    .resume(&reg);
    again.register().await.unwrap();
    assert_eq!(again.registration(), Some(reg.as_str()));
    let late = again
        .start_run("tidy", json!({"ask": "Tidy again."}), None)
        .await
        .unwrap();
    assert_eq!(
        settings_of(&early),
        json!({"max_effects_per_run": 3, "call_timeout_s": 20,
               "runs": {"max_steps": 10, "max_run_minutes": "none"}})
    );
    assert_eq!(
        settings_of(&late),
        json!({"call_timeout_s": 5, "wait_limit_s": 60})
    );
}

#[tokio::test]
async fn a_run_with_no_artifact_is_its_persons_and_an_admins() {
    let d = daemon().await;
    d.configured().await;
    let ts = &d.ts;
    let mut host = d.desk().await;
    let mut run = d.running(&mut host).await;
    // It used something, so the usage page lists it.
    ts.store
        .commit_bot_step(
            &AuditCtx::DAEMON,
            &run.id,
            &run.driver,
            1,
            StepCommit {
                usage: vec![clax_store::credits::Metered {
                    line: clax_store::credits::LINE_SCRIPT_COMPUTE.to_string(),
                    qty: 1_000,
                    key_source: clax_store::credits::KeySource::Instance,
                    absorbed: false,
                    estimated: false,
                    rate: None,
                }],
                ..StepCommit::default()
            },
        )
        .unwrap();
    run.seqs.clear();
    let (s, v) = get(ts, "/api/me/usage").await;
    assert_eq!(s, 200, "{v}");
    let listed = v["runs"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{v}");
    assert_eq!(listed[0]["id"], run.id);
    assert_eq!(listed[0]["artifact_id"], Value::Null);
    assert_eq!(listed[0]["artifact_title"], Value::Null);

    let ana = ts.person("Ana").await;
    let sam = ts.person("Sam").await;
    let lou = ts.viewer(Some("Lou")).await;
    ts.store
        .set_admin(&AuditCtx::DAEMON, &ana.public_id, true)
        .unwrap();
    let as_cookie = |cookie: String| {
        let r = ts
            .client
            .get(format!("{}/api/bot/runs/{}", ts.base, run.id))
            .header("cookie", format!("clax_viewer={cookie}"));
        async move { json_of(r.send().await.unwrap()).await }
    };
    let (s, v) = as_cookie(ana.cookie.clone()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["artifact_id"], Value::Null);
    for cookie in [sam.cookie.clone(), lou.cookie.clone()] {
        let (s, v) = as_cookie(cookie).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    }
    let r = ts
        .client
        .post(format!("{}/api/bot/runs/{}/cancel", ts.base, run.id))
        .header("cookie", format!("clax_viewer={}", sam.cookie));
    let (s, v) = json_of(r.send().await.unwrap()).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    // It has no draft to read.
    let (s, _) = get(ts, &format!("/api/bot/runs/{}/draft/", run.id)).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn a_start_that_is_refused_makes_no_run() {
    let d = daemon().await;
    let ts = &d.ts;
    let host = d.desk().await;
    let reg = host.registration().unwrap().to_string();
    let transport = HttpTransport::of(ts);
    let start = |kind: &str, request: Value| StartRunRequest {
        registration: reg.clone(),
        kind: kind.to_string(),
        request,
        allow: None,
        bind: None,
    };
    let ask = json!({"ask": "Tidy."});

    // Not set up: refused as a run of threads is, and recorded on no
    // artifact.
    let (s, v) = transport
        .start_run("desk", &start("tidy", ask.clone()))
        .await;
    assert_eq!((s, code(&v)), (403, "not_configured"), "{v}");
    let aid = ts.publish("Notes", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread(&aid, 1, "Make it blue").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .client
        .post(format!("{}/api/bot/runs", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"thread_ids": [tid], "artifact_id": aid}));
    let (ts_status, thread) = json_of(r.send().await.unwrap()).await;
    assert_eq!((ts_status, code(&thread)), (s, code(&v)));
    assert_eq!(v["error"]["message"], thread["error"]["message"]);
    let refused: Vec<_> = ts
        .store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "bot.refused")
        .collect();
    assert_eq!(refused.len(), 2);
    assert_eq!(refused[0].ids.artifact, None);
    assert_eq!(refused[1].ids.artifact.as_deref(), Some(aid.as_str()));

    d.configured().await;
    let mut bound = start("tidy", ask.clone());
    bound.bind = Some(
        json!({"clax": {"artifact": aid}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut bad_allow = start("tidy", ask.clone());
    bad_allow.allow = Some(vec!["no good".into()]);
    let mut elsewhere = start("tidy", ask.clone());
    elsewhere.registration = "0".repeat(32);
    for (surface, body, status, want) in [
        // The request is not the kind's.
        (
            "desk",
            start("tidy", json!({"ask": 1})),
            400,
            "invalid_request",
        ),
        ("desk", start("tidy", json!({})), 400, "invalid_request"),
        // A kind of another surface, and one of none.
        ("desk", start("thread", ask.clone()), 400, "invalid_request"),
        (
            "desk",
            start("clax.thread", ask.clone()),
            400,
            "invalid_request",
        ),
        (
            "desk",
            start("desk.tidy", ask.clone()),
            400,
            "invalid_request",
        ),
        ("desk", bad_allow, 400, "invalid_request"),
        ("desk", bound, 501, "not_yet"),
        // A registration the daemon does not hold, or of another surface.
        ("desk", elsewhere, 404, "unknown_registration"),
        (
            "shelf",
            start("tidy", ask.clone()),
            404,
            "unknown_registration",
        ),
    ] {
        let (s, v) = transport.start_run(surface, &body).await;
        assert_eq!((s, code(&v)), (status, want), "{body:?}: {v}");
    }
    let (_, v) = transport
        .start_run("desk", &start("tidy", json!({"ask": 1})))
        .await;
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("kind tidy: "),
        "{v}"
    );
    assert!(ts.store.runs_started_by(&reg, 10).unwrap().is_empty());
    assert!(d.told.runs().is_empty());
    let starts = ts
        .store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "bot.run_start")
        .count();
    assert_eq!(starts, 0);
}

// ---------------------------------------------------------------------------
// A run, from its start to its host's being told of its end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_run_started_over_http_calls_its_host_and_tells_it_how_it_ended() {
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Look at the lamps",
            "const lamps = await clax.desk.lamps(); return lamps.filter((l) => l.on).length;",
        ),
        Turn::finish("One lamp is on."),
    ])
    .await;
    // A daemon as `clax serve` runs one: its driver ticks the run, on the
    // system's clock.
    let ts = TestServer::spawn_with(|s| {
        let d = clax_server::bot::driver(s);
        clax_server::bot::start_driver(&d);
    })
    .await;
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(
                clax_store::bot_settings::Setting::BaseUrl,
                Some(json!(fake.base)),
            )],
        )
        .unwrap();
    let r = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}));
    assert_eq!(r.send().await.unwrap().status(), 200);

    // Each serve is held until the daemon has something for the host.
    let mut host = FakeHost::new(
        HttpTransport::of(&ts),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .on(
        "lamps",
        Behaviour::result(json!([{"id": "l1", "on": true}, {"id": "l2", "on": false}])),
    )
    .wait_ms(10_000);
    host.register().await.expect("desk registers");
    let run = host
        .start_run("tidy", json!({"ask": "Which lamps are on?"}), None)
        .await
        .expect("the run starts");
    let rounds = host
        .serve_until(|h| !h.ended().is_empty(), 12)
        .await
        .expect("the host is told the run ended");
    assert!(rounds >= 2, "{rounds}");

    // The script's call reached the host once, under the run's key.
    let calls = host.calls_of("lamps");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].run, run);
    assert_eq!(calls[0].key, format!("{run}:2:0"));
    assert_eq!(host.applied(&calls[0].key), 1);
    // And the run's end, with the kind's result.
    let ended = host.ended();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].run, run);
    assert_eq!(ended[0].end, End::Finished {});
    assert_eq!(ended[0].result, Some(json!({"summary": "One lamp is on."})));
    // Acknowledged, by a serve that waits for nothing: it is not sent
    // again.
    let mut host = host.wait_ms(0);
    assert!(matches!(host.serve_once().await, Served::Delivered(d) if d.is_empty()));
    assert!(matches!(host.serve_once().await, Served::Delivered(d) if d.is_empty()));
    let (s, v) = get(&ts, &format!("/api/bot/runs/{run}")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "finished");
    assert_eq!(v["artifact_id"], Value::Null);
    assert_eq!(fake.left(), 0);
}

/// A run started over HTTP names, in its binding, the prompt file and
/// the definition its registration had then, and its first turn is sent
/// those: a host that goes on as the registration with others before
/// that turn changes them for the runs it starts afterwards alone.
#[tokio::test]
async fn a_run_keeps_the_prompt_file_and_definition_it_was_started_with() {
    let fake = FakeMessages::start(vec![Turn::finish("First."), Turn::finish("Second.")]).await;
    let clock = Arc::new(ManualClock::at(START));
    let slot = Arc::new(Mutex::new(None));
    let kept = slot.clone();
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        *kept.lock().unwrap() = Some(clax_server::bot::driver(state));
    })
    .await;
    let driver: Arc<clax_bot::Driver> = slot.lock().unwrap().take().expect("a driver");
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(
                clax_store::bot_settings::Setting::BaseUrl,
                Some(json!(fake.base)),
            )],
        )
        .unwrap();
    let r = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}));
    assert_eq!(r.send().await.unwrap().status(), 200);
    let sha = |text: &str| clax_core::audit::sha256_hex(text.as_bytes());
    let binding = |run: &str| -> Value {
        let (_, made, _) = ts.store.run_with_steps_as_made(run).unwrap().unwrap();
        serde_json::from_str::<Value>(&made.bindings_json).unwrap()["bindings"][0].clone()
    };

    let first = file("desk.prompt.md");
    let second = first.replace("You look after a desk", "You keep another desk");
    assert_ne!(second, first);
    let without_lamps = file("desk.d.ts")
        .replace(
            "  /** The lamps on the desk. @read */\n  lamps(): Promise<Lamp[]>;\n",
            "",
        )
        .replace("interface Lamp { id: string; on: boolean }\n", "");
    assert_ne!(without_lamps, file("desk.d.ts"));

    // A run started with the first prompt file and definition, which
    // takes no turn yet.
    let mut host = FakeHost::new(
        HttpTransport::of(&ts),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .with_registration(Some(&first), Settings::default());
    host.register().await.expect("desk registers");
    let reg = host.registration().unwrap().to_string();
    let first_hash = host.registered().unwrap().hash.clone();
    let early = host
        .start_run("tidy", json!({"ask": "Tidy the desk."}), None)
        .await
        .expect("the run starts");
    assert_eq!(
        (&binding(&early)["hash"], &binding(&early)["prompt"]),
        (&json!(first_hash), &json!(sha(&first)))
    );
    // Its host goes on as the registration with another definition and
    // another prompt file.
    let mut host = FakeHost::new(
        HttpTransport::of(&ts),
        "desk",
        Notation::Dts,
        &without_lamps,
    )
    .resume(&reg)
    .with_registration(Some(&second), Settings::default());
    let again = host.register().await.expect("it goes on");
    assert_eq!(again.registration, reg);
    assert_ne!(again.hash, first_hash);
    let row = ts.store.registration(&reg).unwrap().unwrap();
    assert_eq!(row.prompt_hash, Some(sha(&second)));
    // Both texts are kept, each under its hash.
    for text in [&first, &second] {
        assert_eq!(
            ts.store.surface_prompt(&sha(text)).unwrap().as_ref(),
            Some(text)
        );
    }
    // The first run's first turn is sent what it was started with.
    driver.run_until_idle(BUDGET).await;
    let system_of = |n: usize| -> String {
        fake.requests()[n]["system"][0]["text"]
            .as_str()
            .expect("a system prompt")
            .to_string()
    };
    assert_eq!(fake.requests().len(), 1);
    let sent = system_of(0);
    assert!(sent.starts_with("You look after a desk"), "{sent}");
    assert!(sent.contains("lamps(): Promise<desk.Lamp[]>"), "{sent}");
    assert_eq!(
        ts.store.run(&early).unwrap().unwrap().status.as_str(),
        "finished"
    );
    // A run started since is sent the new ones.
    let late = host
        .start_run("tidy", json!({"ask": "Tidy the desk."}), None)
        .await
        .expect("the run starts");
    assert_eq!(
        (&binding(&late)["hash"], &binding(&late)["prompt"]),
        (&json!(again.hash), &json!(sha(&second)))
    );
    driver.run_until_idle(BUDGET).await;
    assert_eq!(fake.requests().len(), 2);
    let sent = system_of(1);
    assert!(sent.starts_with("You keep another desk"), "{sent}");
    assert!(!sent.contains("lamps()"), "{sent}");
    assert_eq!(
        ts.store.run(&late).unwrap().unwrap().status.as_str(),
        "finished"
    );
}

// ---------------------------------------------------------------------------
// A run that waits
// ---------------------------------------------------------------------------

/// A script that reads, asks, and returns what it was answered or the
/// code it was thrown.
const ASK: &str = r#"const lamps = await clax.desk.lamps();
let answer;
try { answer = await clax.desk.ask("Move the left lamp?"); } catch (e) { answer = "thrown " + e.code; }
return [lamps.length, answer];"#;

const HOUR_MS: u64 = 60 * 60 * 1000;
/// How long a script waits for a call that may wait before its run parks
/// (`clax_bot::Delivery::DAEMON`).
const PARK_AFTER: Duration = Duration::from_secs(10);
/// The time a driver is given to become idle, as the daemon's own loop
/// gives it: a turn is not begun with too little of it left, and no test
/// comes near it.
const BUDGET: Duration = Duration::from_secs(3600);

/// A daemon with its driver (ticked by the test, never left to sleep), a
/// scripted model and the desk's host, and a run of `desk.tidy` that
/// read the lamps, asked, was accepted by its host, and parked.
struct Parked {
    d: Daemon,
    driver: Arc<clax_bot::Driver>,
    fake: FakeMessages,
    host: FakeHost<HttpTransport>,
    run: String,
    /// The ID of the question the host holds.
    ask: String,
}

fn lamps_now() -> Behaviour {
    Behaviour::result(json!([{"id": "l1", "on": true}, {"id": "l2", "on": false}]))
}

/// The host's answer to the question, a millisecond of its own clock
/// after it accepted it: sent with the first serve after the test moves
/// that clock.
fn yes_later() -> Behaviour {
    Behaviour::Accept {
        when: clax_testkit::fake_host::When::AfterMs(1),
        then: Outcome::Result(json!("yes")),
    }
}

async fn parked_at(at: &str) -> Parked {
    let fake = FakeMessages::start(vec![
        Turn::script("tu_1", "Ask about the lamp", ASK),
        Turn::finish("Asked."),
    ])
    .await;
    let clock = Arc::new(ManualClock::at(at));
    let sleeper = Arc::new(ManualSleeper::default());
    let told = Arc::new(Told::default());
    let slot = Arc::new(Mutex::new(None));
    let (s, kept) = (sleeper.clone(), slot.clone());
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        state.bot_surfaces.set_sleeper(s);
        *kept.lock().unwrap() = Some(clax_server::bot::driver(state));
    })
    .await;
    let driver = slot.lock().unwrap().take().expect("a driver");
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(
                clax_store::bot_settings::Setting::BaseUrl,
                Some(json!(fake.base)),
            )],
        )
        .unwrap();
    let d = Daemon {
        ts,
        clock,
        sleeper,
        told,
    };
    d.configured().await;
    // Held serves wait five seconds of the test's clock, which no one
    // moves while one is held: each returns when the daemon has
    // something for the host.
    let mut host = FakeHost::new(
        HttpTransport::of(&d.ts),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .on("lamps", lamps_now())
    .on("ask", yes_later())
    .wait_ms(5_000);
    host.register().await.expect("desk registers");
    let run = host
        .start_run(
            "tidy",
            json!({"ask": "Move the left lamp if I say so."}),
            None,
        )
        .await
        .expect("the run starts");
    let ticking = {
        let driver = driver.clone();
        tokio::spawn(async move { driver.run_until_idle(BUDGET).await })
    };
    // The read, answered at once; then the question, which the host
    // accepts and does not answer. Each serve also brings the run's
    // events (its first turn's commit), so the host serves until it has
    // the question.
    host.serve_until(|h| !h.calls_of("ask").is_empty(), 8)
        .await
        .expect("the question comes");
    assert_eq!(
        (host.calls_of("lamps").len(), host.calls_of("ask").len()),
        (1, 1)
    );
    let mut host = host.wait_ms(0);
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.is_empty()));
    let ask = host.calls_of("ask")[0].id.clone();
    // The script has waited as long as a script may: the run parks, and
    // the driver has nothing left to do.
    d.sleeper.wait_for(PARK_AFTER, 1).await;
    d.advance(10_000);
    d.sleeper.fire(PARK_AFTER);
    ticking.await.unwrap();
    // The host is told the run parked.
    let Served::Delivered(told) = host.serve_once().await else {
        panic!("not served")
    };
    assert!(
        matches!(&told[..], [Delivery::Event(e)] if e.event["type"] == "parked"),
        "{told:?}"
    );
    Parked {
        d,
        driver,
        fake,
        host,
        run,
        ask,
    }
}

async fn parked() -> Parked {
    parked_at(START).await
}

impl Parked {
    async fn run_json(&self) -> Value {
        let (s, v) = get(&self.d.ts, &format!("/api/bot/runs/{}", self.run)).await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// Ticks the driver until it has nothing to do.
    async fn tick(&self) {
        self.driver.run_until_idle(BUDGET).await;
    }

    /// The outcome of the run's script step, once committed.
    fn script_value(&self) -> Value {
        let (_, steps) = self.d.ts.store.run_with_steps(&self.run).unwrap().unwrap();
        let step = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
        step.payload.as_ref().expect("committed")["outcome"]["value"].clone()
    }

    /// The host, back after its lease ended, serves until it is told the
    /// run ended and has acknowledged it.
    async fn told_the_end(&mut self) -> clax_bot::clax_surface::wire::Ended {
        self.host.register().await.expect("it goes on");
        self.host.serve_once().await;
        self.host.serve_once().await;
        let ended = self.host.ended();
        assert_eq!(ended.len(), 1, "{ended:?}");
        let ended = ended[0].clone();
        assert_eq!(ended.run, self.run);
        // Acknowledged: nothing more is sent.
        assert!(matches!(self.host.serve_once().await, Served::Delivered(c) if c.is_empty()));
        ended
    }

    /// How often the host was sent each call, and applied it.
    fn sent(&self) -> ((usize, u32), (usize, u32)) {
        let of = |op: &str| {
            let calls = self.host.calls_of(op);
            (calls.len(), self.host.applied(&calls[0].key))
        };
        (of("lamps"), of("ask"))
    }
}

#[tokio::test]
async fn a_run_parks_on_a_call_that_waits_and_goes_on_when_its_host_answers_a_day_later() {
    let mut p = parked().await;
    let st = p.d.ts.store.clone();

    // Parked: no driver holds it, its script step is begun and not
    // committed, and nothing was charged for it.
    let v = p.run_json().await;
    assert_eq!(v["status"], "parked", "{v}");
    assert_eq!(
        (&v["waiting_on"]["surface"], &v["waiting_on"]["op"]),
        (&json!("desk"), &json!("desk.ask")),
        "{v}"
    );
    assert_eq!(v["waiting_on"]["since"], "2026-11-11T00:00:10.000Z");
    assert_eq!(v["waiting_on"]["deadline"], "2026-11-12T00:00:00.000Z");
    let (row, steps) = st.run_with_steps(&p.run).unwrap().unwrap();
    assert_eq!(row.claimed_by, None);
    assert_eq!(row.due_at.as_deref(), Some("2026-11-12T00:00:00.000Z"));
    assert_eq!(steps.len(), 2);
    assert!(steps[0].committed_at.is_some() && steps[1].committed_at.is_none());
    assert_eq!(steps[1].kind, StepKind::Script);
    let usage = st.usage_of_run(&p.run).unwrap();
    assert!(usage.iter().all(|u| u.step_n == Some(1)), "{usage:?}");
    // The host was sent each call once; it has answered the read and
    // holds the question.
    assert_eq!(p.sent(), ((1, 1), (1, 0)));
    assert_eq!(p.fake.left(), 1);
    // A driver that looks at it meanwhile leaves it.
    p.tick().await;
    assert_eq!(p.run_json().await["status"], "parked");

    // 23 hours pass. The host's lease ended long ago: it registers as the
    // registration it was and answers with its next serve.
    p.d.advance(23 * HOUR_MS);
    p.host.clock().advance(1);
    p.host.register().await.expect("it goes on");
    assert!(matches!(p.host.serve_once().await, Served::Delivered(c) if c.is_empty()));
    // The answer is in the run's journal with no claim held, the run is
    // due, and its driver was told by the serve.
    let journal = st.bot_calls(&p.run, 2).unwrap();
    assert_eq!(journal.len(), 2);
    assert_eq!(
        (journal[1].ok(), journal[1].result()),
        (true, &json!("yes"))
    );
    let v = p.run_json().await;
    assert_eq!(v["status"], "queued", "{v}");
    assert!(v.get("waiting_on").is_none(), "{v}");

    // The step is run again from the journal: its host is sent nothing
    // it was sent before. The run goes on and finishes.
    p.tick().await;
    assert_eq!(p.run_json().await["status"], "finished");
    assert_eq!(p.script_value(), json!([2, "yes"]));
    assert_eq!(p.fake.left(), 0);
    let ended = p.told_the_end().await;
    assert_eq!(ended.end, End::Finished {});
    assert_eq!(ended.result, Some(json!({"summary": "Asked."})));
    assert_eq!(p.sent(), ((1, 1), (1, 1)));
}

#[tokio::test]
async fn a_parked_runs_call_is_settled_at_its_deadline_by_the_timer_and_the_run_goes_on() {
    let mut p = parked().await;
    // The deadline comes: 24 hours after the question was sent. The
    // timer's pass settles the call and tells the run's driver.
    p.d.advance(24 * HOUR_MS);
    hub::pass(&p.d.ts.state).await;
    let journal = p.d.ts.store.bot_calls(&p.run, 2).unwrap();
    // Sent to its host and not answered, an effect: it may have happened.
    assert_eq!(journal[1].result()["code"], "unknown_outcome");
    assert_eq!(p.run_json().await["status"], "queued");
    p.tick().await;
    assert_eq!(p.run_json().await["status"], "finished");
    assert_eq!(p.script_value(), json!([2, "thrown unknown_outcome"]));
    // The host's answer, when it comes, is for a settled call.
    p.host.clock().advance(1);
    let ended = p.told_the_end().await;
    assert_eq!(ended.end, End::Finished {});
    assert_eq!(p.d.ts.store.bot_calls(&p.run, 2).unwrap().len(), 2);
    assert_eq!(p.sent(), ((1, 1), (1, 1)));
}

#[tokio::test]
async fn a_parked_runs_call_is_settled_by_the_tick_that_comes_due_when_no_timer_ran() {
    let p = parked().await;
    p.d.advance(24 * HOUR_MS);
    // The driver looks at the run when it is due (its own sleep ended):
    // the tick settles the call itself.
    p.driver.wake(&p.run);
    p.tick().await;
    assert_eq!(p.run_json().await["status"], "finished");
    assert_eq!(p.script_value(), json!([2, "thrown unknown_outcome"]));
    // The timer's pass, coming after, finds nothing to settle and tells
    // no driver.
    let told = Arc::new(Told::default());
    let mut state = p.d.ts.state.clone();
    state.bot = told_bot(&told);
    hub::pass(&state).await;
    assert!(told.runs().is_empty());
    assert_eq!(p.d.ts.store.bot_calls(&p.run, 2).unwrap().len(), 2);
}

#[tokio::test]
async fn a_parked_run_that_is_cancelled_stops_and_its_hosts_late_answer_is_ignored() {
    let mut p = parked().await;
    let (s, v) = post(
        &p.d.ts,
        &format!("/api/bot/runs/{}/cancel", p.run),
        &Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["run"]["status"], "stopping", "{v}");
    assert!(v["run"].get("waiting_on").is_none(), "{v}");
    p.tick().await;
    let v = p.run_json().await;
    assert_eq!(v["status"], "stopped", "{v}");
    // The host answers all the same: nothing is journaled, and it is
    // told the run was stopped.
    p.host.clock().advance(1);
    let ended = p.told_the_end().await;
    assert!(matches!(ended.end, End::Stopped { .. }), "{ended:?}");
    assert_eq!(p.d.ts.store.bot_calls(&p.run, 2).unwrap().len(), 1);
    assert_eq!(p.run_json().await["status"], "stopped");
    assert_eq!(p.fake.left(), 1);
}

#[tokio::test]
async fn a_run_parked_when_the_daemon_restarts_goes_on_when_its_host_comes_back() {
    // The second daemon reads the system's clock: the first one's is
    // ahead of it, so the question's deadline has not come there.
    let p = parked_at("2099-01-01T00:00:00Z").await;
    let id = p.host.registration().unwrap().to_string();
    let slot = Arc::new(Mutex::new(None));
    let kept = slot.clone();
    let again =
        p.d.ts
            .reopened_with(move |s| *kept.lock().unwrap() = Some(clax_server::bot::driver(s)))
            .await;
    let driver = slot.lock().unwrap().take().expect("a driver");
    // The driver starts as a daemon's does, and leaves the parked run.
    driver.start(true).await.unwrap();
    driver.run_until_idle(BUDGET).await;
    let (s, v) = get(&again, &format!("/api/bot/runs/{}", p.run)).await;
    assert_eq!((s, &v["status"]), (200, &json!("parked")), "{v}");

    // The host comes back as the registration it was and answers the
    // question it holds.
    let mut host = FakeHost::new(
        HttpTransport::of(&again),
        "desk",
        Notation::Dts,
        &file("desk.d.ts"),
    )
    .resume(&id);
    assert_eq!(host.register().await.unwrap().registration, id);
    host.answer(&p.ask, Outcome::Result(json!("yes")));
    // It is not sent the question again: it accepted it.
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.is_empty()));
    driver.run_until_idle(BUDGET).await;
    let (_, v) = get(&again, &format!("/api/bot/runs/{}", p.run)).await;
    assert_eq!(v["status"], "finished", "{v}");
    let Served::Delivered(told) = host.serve_once().await else {
        panic!("not served")
    };
    // The end, after the run's events (the step resumed, and each step
    // committed after).
    let ends: Vec<&Delivery> = told
        .iter()
        .filter(|d| !matches!(d, Delivery::Event(_)))
        .collect();
    assert_eq!(ends.len(), 1, "{told:?}");
    let ended = host.ended()[0].clone();
    assert_eq!(
        (ended.run.as_str(), &ended.end),
        (p.run.as_str(), &End::Finished {})
    );
    assert_eq!(ended.result, Some(json!({"summary": "Asked."})));
    // Nothing was sent again, to the host as it was or as it came back.
    assert!(host.calls_of("lamps").is_empty() && host.calls_of("ask").is_empty());
    assert_eq!(p.sent(), ((1, 1), (1, 0)));
}

// ---------------------------------------------------------------------------
// The sources
// ---------------------------------------------------------------------------

/// The daemon's crates above the run loop's (which `clax-bot`'s own scan
/// reads: `crates/clax-bot/tests/outside.rs`).
const DAEMON_CRATES: [&str; 9] = [
    "clax-bot-js",
    "clax-cli",
    "clax-commands",
    "clax-hooks",
    "clax-http",
    "clax-mcp",
    "clax-realtime",
    "clax-server",
    "clax-worker",
];

fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `text` holds `word` as a word of its own.
fn names(text: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + word.len()..].chars().next().is_some_and(part)
    })
}

/// The files and directories that are modules of a crate's tests alone:
/// each `mod <name>;` a file declares under `#[cfg(test)]`.
fn test_modules(files: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let dir = path.parent().unwrap();
        let base = if ["lib", "main", "mod"].contains(&stem) {
            dir.to_path_buf()
        } else {
            dir.join(stem)
        };
        let mut lines = text.lines().map(str::trim).peekable();
        while let Some(line) = lines.next() {
            if line != "#[cfg(test)]" {
                continue;
            }
            let declared = lines.peek().and_then(|next| {
                next.rsplit_once("mod ")
                    .and_then(|(_, name)| name.strip_suffix(';'))
            });
            if let Some(name) = declared {
                out.push(base.join(format!("{name}.rs")));
                out.push(base.join(name));
            }
        }
    }
    out
}

/// No source of the daemon names a host's surface: neither the
/// fixture's `desk`, anywhere in a file's code and comments, nor
/// ClaxShell's `shell` as a surface (`clax.shell`, or an operation or a
/// kind of it in a string, `"shell.…"`) in a line of code. Clax's own
/// web UI is called the shell, so the word alone says nothing; a
/// comment may give `shell.d.ts` or `shell.triage` as an example. Tests
/// are left out: a module declared under `#[cfg(test)]`, a file under a
/// `tests` directory, and what follows `#[cfg(test)]` in any file. A
/// surface reaches the daemon from its host's registration and from
/// nowhere else.
#[test]
fn no_source_of_the_daemon_names_a_hosts_surface() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut read = 0;
    for name in DAEMON_CRATES {
        let mut files = Vec::new();
        sources(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name} has no sources here");
        let tests = test_modules(&files);
        for path in files {
            let shown = path.display().to_string();
            let test = path.components().any(|c| c.as_os_str() == "tests")
                || tests.iter().any(|t| path.starts_with(t));
            if test {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            read += 1;
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            assert!(
                !names(&code.to_lowercase(), "desk"),
                "{shown} names the fixture's surface"
            );
            for (n, line) in code.lines().enumerate() {
                let line = line.trim();
                if line.starts_with("//") {
                    continue;
                }
                let as_surface = line.contains("clax.shell")
                    || line.match_indices("\"shell.").any(|(at, m)| {
                        let rest = &line[at + m.len()..];
                        rest.starts_with(|c: char| c.is_ascii_lowercase())
                            && !rest.starts_with("d.ts")
                            && !rest.starts_with("json")
                    });
                assert!(!as_surface, "{shown}:{}: {line}", n + 1);
            }
        }
    }
    assert!(read > 200, "{read} sources were read");
    assert!(names("a desk.", "desk") && !names("desktop", "desk") && !names("_desk", "desk"));
}

// ---------------------------------------------------------------------------
// The contract fixture's script
// ---------------------------------------------------------------------------

/// The script's runs by the names it gives them (`run-1`), each a run of
/// the daemon's with an ID of its own.
#[derive(Default)]
struct Aliases(Mutex<BTreeMap<String, String>>);

impl Aliases {
    /// `body` with each run's ID written as the script's name for it.
    fn named(&self, body: Value) -> Value {
        let mut text = body.to_string();
        for (alias, id) in self.0.lock().unwrap().iter() {
            text = text.replace(id, alias);
        }
        serde_json::from_str(&text).unwrap()
    }
}

/// The daemon's routes as the script's host calls them: a serve that
/// asks to be held is held while the clock is moved past its wait, and
/// every answer names runs as the script does.
struct Wire<'a> {
    d: &'a Daemon,
    http: HttpTransport,
    aliases: Arc<Aliases>,
}

#[async_trait::async_trait]
impl HostTransport for Wire<'_> {
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value) {
        self.http.put_surface(name, body).await
    }

    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value) {
        let (status, answer) = if body.wait_ms == 0 {
            self.http.serve(name, body).await
        } else {
            let (http, name, body) = (self.http.clone(), name.to_string(), body.clone());
            let wait = Duration::from_millis(body.wait_ms);
            let held = tokio::spawn(async move { http.serve(&name, &body).await });
            self.d.sleeper.wait_for(wait, 1).await;
            self.d.advance(u64::try_from(wait.as_millis()).unwrap());
            self.d.sleeper.fire(wait);
            held.await.unwrap()
        };
        (status, self.aliases.named(answer))
    }

    async fn start_run(&self, name: &str, body: &StartRunRequest) -> (u16, Value) {
        self.http.start_run(name, body).await
    }
}

/// The runs' side of the script, on the daemon: its clock, the store's
/// runs, and the hub the routes serve.
struct World<'a> {
    d: &'a Daemon,
    /// The clock when the script's is 0.
    zero: Millis,
    aliases: Arc<Aliases>,
    runs: BTreeMap<String, Running>,
}

impl World<'_> {
    /// The run the script calls `alias`, made for `registration` the
    /// first time it is named.
    async fn run(&mut self, alias: &str, registration: &str) -> &mut Running {
        if !self.runs.contains_key(alias) {
            let (s, v) = post(
                &self.d.ts,
                "/api/bot/surfaces/desk/runs",
                &json!({"registration": registration, "kind": "tidy",
                    "request": {"ask": "Tidy the desk."}}),
            )
            .await;
            assert_eq!(s, 200, "{v}");
            let id = v["run"].as_str().unwrap().to_string();
            let driver = DriverId::mint();
            let st = &self.d.ts.store;
            st.claim_run(&id, &driver, 60).unwrap().expect("claimed");
            st.begin_step(&id, &driver, 1, StepKind::Script).unwrap();
            self.aliases
                .0
                .lock()
                .unwrap()
                .insert(alias.to_string(), id.clone());
            self.runs.insert(
                alias.to_string(),
                Running {
                    id,
                    driver,
                    seqs: BTreeMap::new(),
                },
            );
        }
        self.runs.get_mut(alias).unwrap()
    }
}

fn thrown(names: &mut Names, code: &str, retryable: bool, step: &Value) {
    let want = &step["error"];
    names.expect(
        want["code"] == code,
        &format!("the script is thrown {code}"),
        step,
    );
    names.expect(
        want["retryable"].as_bool().unwrap_or(false) == retryable,
        &format!("retryable is {retryable}"),
        step,
    );
}

#[async_trait::async_trait]
impl ScriptWorld for World<'_> {
    async fn at(&mut self, ms: u64) {
        // Several steps share an instant: the clock is moved on, never
        // back, and the timer's pass is made by hand at each.
        let now = self.d.now().since(self.zero);
        self.d.advance(ms.saturating_sub(now));
        hub::pass(&self.d.ts.state).await;
    }

    async fn run_step(&mut self, step: &Value, names: &mut Names) {
        let filled = |v: &Value, names: &mut Names| names.fill(v, step);
        let zero = self.zero;
        match step["run"].as_str().unwrap_or("") {
            "call" => {
                let registration = filled(&step["registration"], names);
                let registration = registration.as_str().unwrap_or("").to_string();
                let asked = &step["call"];
                let alias = asked["run"].as_str().unwrap_or("");
                let d = self.d;
                let run = self.run(alias, &registration).await;
                let made = d.call(
                    &registration,
                    run,
                    asked["op"].as_str().unwrap_or(""),
                    asked["args"].clone(),
                    asked["key"].as_str().unwrap_or(""),
                    asked["deadline_ms"].as_u64().map(|ms| zero.after(ms)),
                );
                match made {
                    Ok(id) => {
                        names.expect(
                            step["error"].is_null(),
                            "a call the script expects refused",
                            step,
                        );
                        names.hold(&step["id"], &json!(id), step);
                    }
                    Err(failure) => thrown(names, &failure.code, failure.retryable, step),
                }
            }
            "outcome" => {
                let id = filled(&step["id"], names);
                match self.d.outcome(id.as_str().unwrap_or("")) {
                    None => names.differ("the call is not settled", step),
                    Some((true, value)) => {
                        names.expect(
                            step["error"].is_null(),
                            "a value where an error is expected",
                            step,
                        );
                        names.expect(
                            step["result"] == value,
                            &format!("the result is {value}"),
                            step,
                        );
                    }
                    Some((false, failure)) => thrown(
                        names,
                        failure["code"].as_str().unwrap_or(""),
                        failure["retryable"].as_bool().unwrap_or(false),
                        step,
                    ),
                }
            }
            "waiting" => {
                let id = filled(&step["id"], names);
                let open = self.d.outcome(id.as_str().unwrap_or("")).is_none();
                names.expect(open, "the call is not waiting", step);
            }
            "ended" => {
                let registration = filled(&step["registration"], names);
                let ended = &step["ended"];
                let Ok(end) = serde_json::from_value::<End>(ended["end"].clone()) else {
                    return names.differ("the step's end is not an End", step);
                };
                let alias = ended["run"].as_str().unwrap_or("");
                let Some(run) = self.runs.get(alias) else {
                    return names.differ("the run never called", step);
                };
                // The run ends in the store first: its open step, then
                // the step that finishes it.
                let st = &self.d.ts.store;
                let ctx = AuditCtx::DAEMON;
                st.commit_bot_step(&ctx, &run.id, &run.driver, 1, StepCommit::default())
                    .unwrap();
                st.begin_step(&run.id, &run.driver, 2, StepKind::Finish)
                    .unwrap();
                st.commit_bot_step(&ctx, &run.id, &run.driver, 2, StepCommit::default())
                    .unwrap();
                let id = self.d.ts.state.bot_surfaces.broker().ended(
                    registration.as_str().unwrap_or(""),
                    &run.id,
                    end,
                    ended.get("result").cloned(),
                );
                match id {
                    Ok(id) => names.hold(&step["id"], &json!(id), step),
                    Err(e) => names.differ(&format!("the end was not recorded: {e}"), step),
                }
            }
            other => names.differ(
                &format!("a run step this world does not know: {other}"),
                step,
            ),
        }
    }
}

#[tokio::test]
async fn the_fixtures_script_replays_clean_through_a_daemon() {
    let d = daemon().await;
    d.configured().await;
    let aliases = Arc::new(Aliases::default());
    let wire = Wire {
        d: &d,
        http: HttpTransport::of(&d.ts),
        aliases: aliases.clone(),
    };
    let mut world = World {
        d: &d,
        zero: d.now(),
        aliases,
        runs: BTreeMap::new(),
    };
    let names = replay(&fixture(), &wire, &mut world).await;
    assert_eq!(names.differences(), &[] as &[String]);
    // The script's two registrations and the calls it made were the
    // daemon's: every call is settled in its run's journal, and the end
    // was acknowledged.
    for name in ["$a", "$b"] {
        let id = names.get(name).and_then(Value::as_str).expect(name);
        assert!(d.ts.store.registration(id).unwrap().is_some(), "{name}");
        let waiting = d.ts.store.pending_summary(id).unwrap();
        assert_eq!((waiting.calls.len(), waiting.ends.len()), (0, 0), "{name}");
    }
    assert_eq!(world.runs.len(), 2);
}

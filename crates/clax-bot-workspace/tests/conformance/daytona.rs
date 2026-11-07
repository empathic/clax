//! Daytona's client against a fake of its documented routes, and, when
//! `DAYTONA_API_KEY` is set, against Daytona itself.

use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clax_bot_workspace::daytona::Daytona;
use clax_bot_workspace::runtime::{ContainerRuntime, ContainerSpec, Exec, is_gone};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Sandbox {
    labels: BTreeMap<String, String>,
    /// The states it is yet to pass through; it rests on the last.
    states: VecDeque<String>,
    error_reason: Option<String>,
    name: Option<String>,
    recoverable: bool,
    allow_list: Option<String>,
    policy_error: Option<String>,
}

/// What a test sets up, and what it can read afterwards.
#[derive(Default)]
struct Shared {
    live: Mutex<HashMap<String, Sandbox>>,
    /// The next answers, whatever is asked.
    faults: Mutex<VecDeque<StatusCode>>,
    /// The states the next sandbox made passes through.
    new_states: Mutex<Vec<String>>,
    /// A sandbox made with a reason it failed.
    new_reason: Mutex<Option<String>>,
    /// The list ignores its `labels` filter.
    list_ignores_labels: Mutex<bool>,
    /// An answer to `process/execute` in place of running anything.
    execute_answer: Mutex<Option<String>>,
    /// Every request, as `METHOD path`.
    calls: Mutex<Vec<String>>,
    made: AtomicUsize,
    /// The API forgets `domainAllowList`, on the way in and out.
    drops_allow_list: Mutex<bool>,
    /// The API reports an allow-list and does not enforce it.
    echoes_without_enforcing: Mutex<bool>,
    /// `POST /sandbox` makes one, then answers 502.
    makes_then_fails: Mutex<bool>,
    /// The policy error a new sandbox reports.
    new_policy_error: Mutex<Option<String>>,
    /// `recover` does nothing.
    recover_fails: Mutex<Option<()>>,
    /// The next answer is a 400 with this body.
    huge_error: Mutex<Option<String>>,
    /// Every request redirects here.
    redirects_to: Mutex<Option<String>>,
}

#[derive(Clone)]
struct Fake {
    base: String,
    shared: Arc<Shared>,
}

impl Fake {
    fn calls(&self, prefix: &str) -> usize {
        let calls = self.shared.calls.lock().unwrap();
        calls.iter().filter(|c| c.starts_with(prefix)).count()
    }

    fn fail_next(&self, statuses: &[StatusCode]) {
        self.shared.faults.lock().unwrap().extend(statuses);
    }

    fn set_state(&self, id: &str, state: &str) {
        let mut live = self.shared.live.lock().unwrap();
        live.get_mut(id).unwrap().states = [state.to_string()].into();
    }

    fn set_states(&self, states: &[&str]) {
        *self.shared.new_states.lock().unwrap() = states.iter().map(|s| s.to_string()).collect();
    }

    fn client(&self) -> Daytona {
        let mut d = Daytona::new(&self.base, "key", None);
        d.poll = Duration::ZERO;
        d.backoff = Duration::ZERO;
        d
    }
}

fn body_of(f: &Fake, id: &str, s: &Sandbox) -> Value {
    json!({
        "id": id,
        "state": s.states.front(),
        "errorReason": s.error_reason,
        "recoverable": s.recoverable,
        "domainAllowList": s.allow_list,
        "networkPolicyError": s.policy_error,
        "labels": s.labels,
        "toolboxProxyUrl": format!("{}/toolbox", f.base),
    })
}

async fn faults(State(f): State<Fake>, request: Request, next: Next) -> Response {
    let call = format!("{} {}", request.method(), request.uri().path());
    if let Some(body) = f.shared.huge_error.lock().unwrap().take() {
        return (StatusCode::BAD_REQUEST, body).into_response();
    }
    let redirect = f.shared.redirects_to.lock().unwrap().clone();
    if let Some(to) = redirect {
        let target = format!("{to}{}", request.uri());
        return axum::response::Redirect::temporary(&target).into_response();
    }
    f.shared.calls.lock().unwrap().push(call);
    if let Some(status) = f.shared.faults.lock().unwrap().pop_front() {
        let mut r = (status, "{\"message\":\"not now\"}").into_response();
        if status == StatusCode::TOO_MANY_REQUESTS {
            r.headers_mut().insert("retry-after", "0".parse().unwrap());
        }
        return r;
    }
    next.run(request).await
}

async fn create(State(f): State<Fake>, Json(b): Json<Value>) -> Response {
    let n = f.shared.made.fetch_add(1, Ordering::SeqCst);
    let id = format!("sb-{n}");
    let mut states: VecDeque<String> = f.shared.new_states.lock().unwrap().drain(..).collect();
    if states.is_empty() {
        states.push_back("started".to_string());
    }
    let s = Sandbox {
        labels: serde_json::from_value(b["labels"].clone()).unwrap_or_default(),
        states,
        error_reason: f.shared.new_reason.lock().unwrap().take(),
        allow_list: (!*f.shared.drops_allow_list.lock().unwrap())
            .then(|| b["domainAllowList"].as_str().map(str::to_string))
            .flatten(),
        policy_error: f.shared.new_policy_error.lock().unwrap().clone(),
        name: b["name"].as_str().map(str::to_string),
        recoverable: false,
    };
    let answer = body_of(&f, &id, &s);
    f.shared.live.lock().unwrap().insert(id, s);
    if std::mem::take(&mut *f.shared.makes_then_fails.lock().unwrap()) {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    Json(answer).into_response()
}

/// The ID of the sandbox `id_or_name`.
fn resolve(live: &HashMap<String, Sandbox>, id_or_name: &str) -> Option<String> {
    if live.contains_key(id_or_name) {
        return Some(id_or_name.to_string());
    }
    let named = live
        .iter()
        .find(|(_, s)| s.name.as_deref() == Some(id_or_name));
    named.map(|(id, _)| id.clone())
}

async fn read(State(f): State<Fake>, Path(id): Path<String>) -> Result<Json<Value>, StatusCode> {
    let mut live = f.shared.live.lock().unwrap();
    let id = resolve(&live, &id).ok_or(StatusCode::NOT_FOUND)?;
    let s = live.get_mut(&id).unwrap();
    if s.states.len() > 1 {
        s.states.pop_front();
    }
    Ok(Json(body_of(&f, &id, s)))
}

async fn delete(State(f): State<Fake>, Path(id): Path<String>) -> StatusCode {
    match f.shared.live.lock().unwrap().remove(&id) {
        Some(_) => StatusCode::OK,
        None => StatusCode::NOT_FOUND,
    }
}

async fn start(State(f): State<Fake>, Path(id): Path<String>) -> StatusCode {
    if f.shared.recover_fails.lock().unwrap().is_some() {
        return StatusCode::OK;
    }
    let mut live = f.shared.live.lock().unwrap();
    match live.get_mut(&id) {
        Some(s) => {
            s.states = ["starting", "started"].map(String::from).into();
            StatusCode::OK
        }
        None => StatusCode::NOT_FOUND,
    }
}

fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn query_of(uri: &Uri, name: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decoded(v))
    })
}

/// One sandbox a page, so that following `nextCursor` is what finds them all.
async fn list(State(f): State<Fake>, uri: Uri) -> Json<Value> {
    let want: BTreeMap<String, String> =
        serde_json::from_str(&query_of(&uri, "labels").unwrap_or("{}".to_string())).unwrap();
    let ignore = *f.shared.list_ignores_labels.lock().unwrap();
    let live = f.shared.live.lock().unwrap();
    let mut ids: Vec<&String> = live
        .iter()
        .filter(|(_, s)| ignore || want.iter().all(|(k, v)| s.labels.get(k) == Some(v)))
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    let at: usize = query_of(&uri, "cursor").map_or(0, |c| c.parse().unwrap());
    let items: Vec<Value> = ids
        .get(at)
        .map(|id| body_of(&f, id, &live[*id]))
        .into_iter()
        .collect();
    let next = (at + 1 < ids.len()).then(|| (at + 1).to_string());
    Json(json!({"items": items, "nextCursor": next}))
}

/// As Daytona's toolbox: `command` is split into arguments, not given to a
/// shell, so only the client's own `sh -c '…'` form runs a pipeline. A
/// sandbox enforces its allow-list through `FAKE_ALLOWED`, which
/// [`fake_reaches`] reads.
async fn execute(
    State(f): State<Fake>,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> Result<Response, StatusCode> {
    let allowed = {
        let live = f.shared.live.lock().unwrap();
        let s = live.get(&id).ok_or(StatusCode::NOT_FOUND)?;
        if s.states.front().map(String::as_str) != Some("started") {
            return Err(StatusCode::CONFLICT);
        }
        s.allow_list.clone().unwrap_or_default()
    };
    let canned = f.shared.execute_answer.lock().unwrap().clone();
    if let Some(answer) = canned {
        return Ok(answer.into_response());
    }
    let command = b["command"].as_str().unwrap();
    let inner = command
        .strip_prefix("sh -c '")
        .and_then(|c| c.strip_suffix('\''))
        .expect("the client wraps every command for sh");
    let secs = b["timeout"].as_u64().expect("a timeout is always named");
    let mut c = tokio::process::Command::new("sh");
    c.args(["-c", &format!("{{ {inner}; }} 2>&1")]);
    c.process_group(0);
    c.stdout(std::process::Stdio::piped());
    if !*f.shared.echoes_without_enforcing.lock().unwrap() {
        c.env("FAKE_ALLOWED", allowed);
    }
    if let Some(cwd) = b["cwd"].as_str() {
        c.current_dir(cwd);
    }
    if let Some(envs) = b["envs"].as_object() {
        for (k, v) in envs {
            c.env(k, v.as_str().unwrap());
        }
    }
    let child = c.spawn().unwrap();
    let group = child.id().unwrap();
    let Ok(out) = tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output()).await
    else {
        let target = format!("-{group}");
        let killed = tokio::process::Command::new("kill")
            .args(["-KILL", "--", &target])
            .status();
        killed.await.unwrap();
        return Err(StatusCode::REQUEST_TIMEOUT);
    };
    let out = out.unwrap();
    Ok(Json(json!({
        "exitCode": out.status.code(),
        "result": String::from_utf8_lossy(&out.stdout),
    }))
    .into_response())
}

/// Shell text that succeeds when a fake sandbox's allow-list lets `host`
/// through: an empty list is open egress.
fn fake_reaches(host: &str) -> String {
    format!("case \",$FAKE_ALLOWED,\" in *,{host},*) exit 0;; esac; [ -z \"$FAKE_ALLOWED\" ]")
}

impl Fake {
    /// Stops a sandbox as its idle rule would.
    async fn stop(&self, id: &str) -> Result<()> {
        let mut live = self.shared.live.lock().unwrap();
        live.get_mut(id).unwrap().states = ["stopped".to_string()].into();
        Ok(())
    }

    async fn contract(&self) -> Result<()> {
        let stop = async |id: &str| self.stop(id).await;
        crate::contract::check(
            &self.client(),
            &ContainerSpec::default(),
            fake_reaches,
            stop,
        )
        .await
    }
}

async fn fake() -> Fake {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let f = Fake {
        base,
        shared: Arc::default(),
    };
    let app = Router::new()
        .route("/sandbox", post(create).get(list))
        .route("/sandbox/{id}", get(read).delete(delete))
        .route("/sandbox/{id}/start", post(start))
        .route("/sandbox/{id}/recover", post(start))
        .route("/toolbox/{id}/process/execute", post(execute))
        .layer(middleware::from_fn_with_state(f.clone(), faults))
        .with_state(f.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    f
}

const T: Duration = Duration::from_secs(10);
const BUSY: StatusCode = StatusCode::TOO_MANY_REQUESTS;
const DOWN: StatusCode = StatusCode::BAD_GATEWAY;

#[tokio::test]
async fn daytona_keeps_the_contract() {
    fake().await.contract().await.unwrap();
}

#[tokio::test]
async fn a_client_that_loses_the_allow_list_fails_the_contract() {
    let f = fake().await;
    *f.shared.drops_allow_list.lock().unwrap() = true;
    let e = f.contract().await.unwrap_err();
    assert!(format!("{e:#}").contains("allow-list"), "{e:#}");
}

#[tokio::test]
async fn a_sandbox_that_reports_a_list_it_does_not_enforce_fails_the_contract() {
    let f = fake().await;
    *f.shared.echoes_without_enforcing.lock().unwrap() = true;
    let e = f.contract().await.unwrap_err();
    assert!(format!("{e:#}").contains("off its list"), "{e:#}");
}

#[tokio::test]
async fn making_a_sandbox_goes_again_on_429_and_never_on_a_server_error() {
    let f = fake().await;
    let d = f.client();
    f.fail_next(&[BUSY, BUSY]);
    d.create(&ContainerSpec::default()).await.unwrap();
    assert_eq!(f.calls("POST /sandbox"), 3);
    assert_eq!(f.shared.live.lock().unwrap().len(), 1);

    // A 502 may have made one, and a second would be paid for twice.
    f.fail_next(&[DOWN]);
    let e = d.create(&ContainerSpec::default()).await.err().unwrap();
    assert!(format!("{e:#}").contains("502"), "{e:#}");
    assert_eq!(f.calls("POST /sandbox"), 4);
}

#[tokio::test]
async fn reading_listing_and_deleting_go_again_on_a_server_error() {
    let f = fake().await;
    let d = f.client();
    let c = d.create(&ContainerSpec::default()).await.unwrap();
    f.fail_next(&[DOWN, StatusCode::SERVICE_UNAVAILABLE, BUSY]);
    d.attach(c.id(), &ContainerSpec::default()).await.unwrap();
    f.fail_next(&[DOWN, BUSY]);
    assert_eq!(d.find(&BTreeMap::new()).await.unwrap().len(), 1);
    f.fail_next(&[DOWN]);
    d.destroy(c.id()).await.unwrap();
    assert!(f.shared.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_command_goes_again_on_429_and_is_never_run_twice() {
    let f = fake().await;
    let d = f.client();
    let c = d.create(&ContainerSpec::default()).await.unwrap();
    f.fail_next(&[BUSY]);
    assert_eq!(c.exec(&Exec::new("echo hi", T)).await.unwrap().exit_code, 0);
    f.fail_next(&[DOWN]);
    assert!(c.exec(&Exec::new("echo hi", T)).await.is_err());
    assert_eq!(f.calls("POST /toolbox"), 3);
}

#[tokio::test]
async fn retries_run_out_with_a_clear_error() {
    let f = fake().await;
    let mut d = f.client();
    d.retries = 2;
    f.fail_next(&[BUSY, BUSY, BUSY, BUSY]);
    let e = d.create(&ContainerSpec::default()).await.err().unwrap();
    assert!(format!("{e:#}").contains("rate limit"), "{e:#}");
    assert_eq!(f.calls("POST /sandbox"), 3);
}

#[tokio::test]
async fn a_refused_key_says_so() {
    let f = fake().await;
    f.fail_next(&[StatusCode::UNAUTHORIZED]);
    let e = f.client().find(&BTreeMap::new()).await.unwrap_err();
    assert!(format!("{e:#}").contains("refused the API key"), "{e:#}");
    assert_eq!(f.calls("GET /sandbox"), 1);
}

#[tokio::test]
async fn a_new_sandbox_is_waited_for_through_every_state_before_started() {
    let f = fake().await;
    f.set_states(&["creating", "pulling_snapshot", "starting", "started"]);
    let c = f.client().create(&ContainerSpec::default()).await.unwrap();
    assert_eq!(f.calls("GET /sandbox/"), 3);
    assert_eq!(c.exec(&Exec::new("true", T)).await.unwrap().exit_code, 0);
}

#[tokio::test]
async fn a_sandbox_that_fails_to_start_says_why_and_is_deleted() {
    let f = fake().await;
    f.set_states(&["creating", "error"]);
    *f.shared.new_reason.lock().unwrap() = Some("no capacity".to_string());
    let e = f
        .client()
        .create(&ContainerSpec::default())
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("no capacity"), "{e:#}");
    assert!(f.shared.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_sandbox_that_never_starts_times_out_naming_its_state_and_is_deleted() {
    let f = fake().await;
    f.set_states(&["pending_build"]);
    let mut d = f.client();
    d.start_timeout = Duration::ZERO;
    let e = d.create(&ContainerSpec::default()).await.err().unwrap();
    assert!(format!("{e:#}").contains("still pending_build"), "{e:#}");
    assert!(f.shared.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn attaching_starts_a_stopped_sandbox_and_nothing_starts_a_failed_one() {
    let f = fake().await;
    let d = f.client();
    f.set_states(&["started"]);
    let c = d.create(&ContainerSpec::default()).await.unwrap();
    let id = c.id().to_string();
    let set = |state: &str| {
        let mut live = f.shared.live.lock().unwrap();
        live.get_mut(&id).unwrap().states = [state.to_string()].into();
    };
    set("stopped");
    let open = ContainerSpec::default();
    let again = d.attach(&id, &open).await.unwrap();
    assert_eq!(f.calls("POST /sandbox/"), 1);
    assert_eq!(
        again.exec(&Exec::new("true", T)).await.unwrap().exit_code,
        0
    );
    set("error");
    let e = d.attach(&id, &open).await.err().unwrap();
    assert!(is_gone(&e), "{e:#}");
    assert_eq!(f.calls("POST /sandbox/"), 1);
}

#[tokio::test]
async fn find_follows_every_page_and_holds_the_filter_itself() {
    let f = fake().await;
    let d = f.client();
    let labelled = |v: &str| ContainerSpec {
        labels: BTreeMap::from([("clax.run".to_string(), v.to_string())]),
        ..ContainerSpec::default()
    };
    for v in ["a", "a", "a", "b"] {
        d.create(&labelled(v)).await.unwrap();
    }
    let a = labelled("a").labels;
    assert_eq!(d.find(&a).await.unwrap().len(), 3);
    assert_eq!(d.find(&BTreeMap::new()).await.unwrap().len(), 4);
    *f.shared.list_ignores_labels.lock().unwrap() = true;
    assert_eq!(d.find(&a).await.unwrap().len(), 3);
}

#[tokio::test]
async fn an_answer_over_the_limit_is_not_read_to_its_end() {
    let f = fake().await;
    let d = f.client();
    let c = d.create(&ContainerSpec::default()).await.unwrap();
    let huge = "x".repeat(40 * 1024 * 1024);
    *f.shared.execute_answer.lock().unwrap() =
        Some(format!("{{\"exitCode\":0,\"result\":\"{huge}\"}}"));
    let o = c.exec(&Exec::new("true", T)).await.unwrap();
    assert_ne!(o.exit_code, 0);
    assert!(o.output.len() < 1024);
}

/// Proves the client against the real service, and creates and deletes real
/// sandboxes in doing so:
///
/// ```sh
/// DAYTONA_API_KEY=… bazelisk test //crates/clax-bot-workspace:conformance \
///   --test_env=DAYTONA_API_KEY --test_arg=live_daytona --test_output=all
/// ```
///
/// `DAYTONA_API_URL` and `DAYTONA_ORGANIZATION_ID` pass the same way.
#[tokio::test]
async fn live_daytona_keeps_the_contract() {
    if std::env::var_os("DAYTONA_API_KEY").is_none() {
        crate::skipped("live_daytona_keeps_the_contract: DAYTONA_API_KEY is not set");
        return;
    }
    let d = Daytona::from_env().unwrap();
    let spec = ContainerSpec {
        auto_stop_minutes: Some(5),
        ..ContainerSpec::default()
    };
    let stop = async |id: &str| {
        let url = std::env::var("DAYTONA_API_URL")
            .unwrap_or_else(|_| clax_bot_workspace::daytona::DEFAULT_API_URL.into());
        let key = std::env::var("DAYTONA_API_KEY").unwrap();
        let mut r = reqwest::Client::new()
            .post(format!("{url}/sandbox/{id}/stop"))
            .bearer_auth(key);
        if let Ok(org) = std::env::var("DAYTONA_ORGANIZATION_ID") {
            r = r.header("X-Daytona-Organization-ID", org);
        }
        r.send().await?.error_for_status()?;
        Ok(())
    };
    crate::contract::check(&d, &spec, crate::contract::reaches, stop)
        .await
        .unwrap();
}

fn limited() -> ContainerSpec {
    ContainerSpec {
        domain_allow_list: vec!["a.dev".to_string(), "b.dev".to_string()],
        ..ContainerSpec::default()
    }
}

#[tokio::test]
async fn a_sandbox_that_does_not_echo_its_allow_list_is_refused_and_deleted() {
    let f = fake().await;
    *f.shared.drops_allow_list.lock().unwrap() = true;
    assert!(f.client().create(&limited()).await.is_err());
    assert!(f.shared.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_sandbox_whose_network_policy_failed_is_refused_and_deleted() {
    let f = fake().await;
    *f.shared.new_policy_error.lock().unwrap() = Some("proxy binding failed".to_string());
    let e = f.client().create(&limited()).await.err().unwrap();
    assert!(format!("{e:#}").contains("proxy binding"), "{e:#}");
    assert!(f.shared.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_script_reading_stdin_does_not_eat_the_rest_of_itself() {
    let f = fake().await;
    let c = f.client().create(&ContainerSpec::default()).await.unwrap();
    let o = c
        .exec(&Exec::new("cat >/dev/null\necho after", T))
        .await
        .unwrap();
    assert!(o.output.contains("after"), "{o:?}");
}

#[tokio::test]
async fn a_redirect_is_not_followed_with_the_api_key() {
    let f = fake().await;
    let elsewhere = fake().await;
    *f.shared.redirects_to.lock().unwrap() = Some(elsewhere.base.clone());
    assert!(f.client().find(&BTreeMap::new()).await.is_err());
    assert_eq!(elsewhere.calls("GET"), 0);
}

#[tokio::test]
async fn attaching_refuses_and_deletes_a_sandbox_not_holding_the_list() {
    let f = fake().await;
    let d = f.client();
    let c = d.create(&limited()).await.unwrap();
    let id = c.id().to_string();
    assert!(d.attach(&id, &limited()).await.is_ok());
    let wider = ContainerSpec {
        domain_allow_list: vec!["a.dev".to_string()],
        ..ContainerSpec::default()
    };
    let e = d.attach(&id, &wider).await.err().unwrap();
    assert!(is_gone(&e), "{e:#}");
    assert!(f.shared.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_sandbox_that_could_not_be_read_is_not_deleted_on_attach() {
    let f = fake().await;
    let mut d = f.client();
    let c = d.create(&ContainerSpec::default()).await.unwrap();
    let id = c.id().to_string();
    f.set_state(&id, "starting");
    d.start_timeout = Duration::ZERO;
    let e = d
        .attach(&id, &ContainerSpec::default())
        .await
        .err()
        .unwrap();
    assert!(!is_gone(&e), "{e:#}");
    assert_eq!(f.shared.live.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_create_whose_answer_was_lost_is_settled_by_its_name() {
    let f = fake().await;
    let d = f.client();
    let named = ContainerSpec {
        name: Some("clax-r1".to_string()),
        ..ContainerSpec::default()
    };
    *f.shared.makes_then_fails.lock().unwrap() = true;
    let c = d.create(&named).await.unwrap();
    assert_eq!(f.calls("POST /sandbox"), 1);
    assert_eq!(c.exec(&Exec::new("true", T)).await.unwrap().exit_code, 0);
}

#[tokio::test]
async fn a_create_that_failed_for_good_says_so_when_the_name_finds_nothing() {
    let f = fake().await;
    f.fail_next(&[DOWN]);
    let named = ContainerSpec {
        name: Some("clax-r1".to_string()),
        ..ContainerSpec::default()
    };
    let e = f.client().create(&named).await.err().unwrap();
    assert!(format!("{e:#}").contains("502"), "{e:#}");
}

#[tokio::test]
async fn an_error_body_is_read_only_so_far() {
    let f = fake().await;
    f.shared
        .huge_error
        .lock()
        .unwrap()
        .replace("x".repeat(1 << 20));
    let e = f.client().find(&BTreeMap::new()).await.unwrap_err();
    assert!(format!("{e:#}").len() < 64 * 1024);
}

#[tokio::test]
async fn a_recoverable_error_is_recovered_on_attach_and_not_deleted() {
    let f = fake().await;
    let mut d = f.client();
    let c = d.create(&ContainerSpec::default()).await.unwrap();
    let id = c.id().to_string();
    let recoverable = || {
        let mut live = f.shared.live.lock().unwrap();
        let s = live.get_mut(&id).unwrap();
        s.states = ["error".to_string()].into();
        s.recoverable = true;
    };
    recoverable();
    let open = ContainerSpec::default();
    d.attach(&id, &open).await.unwrap();
    assert_eq!(f.calls("POST /sandbox/"), 1);

    // One that stays in error is kept for the next try.
    recoverable();
    f.shared.recover_fails.lock().unwrap().replace(());
    d.start_timeout = Duration::ZERO;
    let e = d.attach(&id, &open).await.err().unwrap();
    assert!(!is_gone(&e), "{e:#}");
    assert_eq!(f.shared.live.lock().unwrap().len(), 1);
}

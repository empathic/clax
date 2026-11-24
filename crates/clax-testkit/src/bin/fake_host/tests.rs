//! The binary's command line, its registration file and its patience
//! with a daemon that does not answer, against the hub in process
//! ([`MemoryHub`]): nothing here waits on a clock.

use super::*;
use clax_surface::wire::{RegisterRequest, ServeRequest, StartRunRequest};
use clax_testkit::fake_host::{HostClock, MemoryHub};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const DESK: &str = "/** A desk. */\ndeclare const desk: {\n  /** The lamps on the desk. @read */\n  lamps(): Promise<string[]>;\n};\n";

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

const REQUIRED: &str = "--url u --token-file t --surface s --answers a --stop-file f";

#[test]
fn the_command_line_reads_each_flag_and_its_defaults() {
    let o = parse(args(REQUIRED)).unwrap();
    assert_eq!(
        o,
        Options {
            url: "u".into(),
            token_file: "t".into(),
            surface: "s".into(),
            answers: "a".into(),
            stop_file: "f".into(),
            wait_ms: 1_000,
            registration: None,
            registration_file: None,
            deliveries: None,
            retry_for_ms: 0,
        }
    );
    let o = parse(args(&format!(
        "{REQUIRED} --wait-ms 5 --registration r --registration-file rf --deliveries d --retry-for 30"
    )))
    .unwrap();
    assert_eq!(
        (
            o.wait_ms,
            o.registration.as_deref(),
            o.registration_file,
            o.deliveries,
            o.retry_for_ms
        ),
        (5, Some("r"), Some("rf".into()), Some("d".into()), 30_000)
    );
}

#[test]
fn the_command_line_refuses_what_it_does_not_read() {
    let refused = |text: &str| parse(args(text)).unwrap_err();
    assert_eq!(
        refused("--url u --token-file t --surface s --answers a"),
        "--stop-file is required"
    );
    assert_eq!(
        refused(&format!("{REQUIRED} --retry 3")),
        "--retry is not a flag of fake_host"
    );
    assert_eq!(
        refused(&format!("{REQUIRED} --url v")),
        "--url is given twice"
    );
    assert!(refused(&format!("{REQUIRED} --retry-for soon")).starts_with("--retry-for: "));
    assert_eq!(
        refused(&format!("{REQUIRED} --wait-ms")),
        "expected --<name> <value>, got --wait-ms"
    );
}

#[test]
fn a_registration_file_holds_one_id_and_is_none_when_absent_or_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registration");
    assert_eq!(read_registration(&path), Ok(None));
    write_registration(&path, "0123").unwrap();
    assert_eq!(read_registration(&path), Ok(Some("0123".into())));
    write_registration(&path, "4567").unwrap();
    assert_eq!(read_registration(&path), Ok(Some("4567".into())));
    std::fs::write(&path, "\n").unwrap();
    assert_eq!(read_registration(&path), Ok(None));
}

/// [`MemoryHub`] that gets no answer for its first `fails` requests.
#[derive(Clone)]
struct Away {
    hub: MemoryHub,
    fails: Arc<AtomicU32>,
}

impl Away {
    fn new(hub: &MemoryHub, fails: u32) -> Away {
        Away {
            hub: hub.clone(),
            fails: Arc::new(AtomicU32::new(fails)),
        }
    }

    fn away(&self) -> Option<(u16, Value)> {
        let failing = self
            .fails
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        failing.then(|| {
            (
                0,
                json!({"error": {"code": "transport", "message": "connection refused"}}),
            )
        })
    }
}

#[async_trait::async_trait]
impl HostTransport for Away {
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value) {
        match self.away() {
            Some(refused) => refused,
            None => self.hub.put_surface(name, body).await,
        }
    }
    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value) {
        match self.away() {
            Some(refused) => refused,
            None => self.hub.serve(name, body).await,
        }
    }
    async fn start_run(&self, name: &str, body: &StartRunRequest) -> (u16, Value) {
        match self.away() {
            Some(refused) => refused,
            None => self.hub.start_run(name, body).await,
        }
    }
}

fn host<T: HostTransport>(transport: T) -> FakeHost<T> {
    FakeHost::new(transport, "desk", Notation::Dts, DESK)
        .on("lamps", Behaviour::result(json!(["l1"])))
}

struct Run<T> {
    outcome: Result<FakeHost<T>, String>,
    pauses: Vec<u64>,
    out: String,
}

/// Serves `host` as the binary does, until `stopped`, pausing for no
/// time and keeping each pause asked for; `retoken` is the binary's
/// reading of its token file.
async fn serve_with<T: HostTransport>(
    serving: &Serving<'_>,
    host: FakeHost<T>,
    meant: Option<&str>,
    stopped: impl FnMut(&FakeHost<T>, &[u64]) -> bool,
    retoken: impl FnMut(&mut T),
) -> Run<T> {
    let pauses = RefCell::new(Vec::new());
    let mut stopped = stopped;
    let mut out = Vec::new();
    let outcome = serving
        .serve(
            host,
            meant.map(str::to_string),
            |h| stopped(h, &pauses.borrow()),
            |ms| {
                pauses.borrow_mut().push(ms);
                std::future::ready(())
            },
            retoken,
            &mut out,
        )
        .await;
    Run {
        outcome,
        pauses: pauses.into_inner(),
        out: String::from_utf8(out).unwrap(),
    }
}

async fn serve<T: HostTransport>(
    serving: &Serving<'_>,
    host: FakeHost<T>,
    meant: Option<&str>,
    stopped: impl FnMut(&FakeHost<T>, &[u64]) -> bool,
) -> Run<T> {
    serve_with(serving, host, meant, stopped, |_| {}).await
}

fn patient(ms: u64) -> Serving<'static> {
    Serving {
        registration_file: None,
        deliveries: None,
        retry_for_ms: ms,
    }
}

#[tokio::test]
async fn a_daemon_that_never_answers_is_tried_for_the_budget_and_no_longer() {
    let hub = MemoryHub::new(HostClock::at(0));
    let away = Away::new(&hub, u32::MAX);
    let run = serve(&patient(1_000), host(away), None, |_, _| false).await;
    assert_eq!(run.pauses, [100, 200, 400, 300]);
    let e = run.outcome.err().unwrap();
    assert!(e.starts_with("register: 0 "), "{e}");
    assert!(e.contains("transport"), "{e}");
}

#[tokio::test]
async fn with_no_budget_a_daemon_that_does_not_answer_ends_the_host() {
    let hub = MemoryHub::new(HostClock::at(0));
    let run = serve(&patient(0), host(Away::new(&hub, 1)), None, |_, _| false).await;
    assert_eq!(run.pauses, [] as [u64; 0]);
    assert!(run.outcome.is_err());
}

#[tokio::test]
async fn pauses_grow_to_five_seconds_and_each_answer_restores_the_budget() {
    let hub = MemoryHub::new(HostClock::at(0));
    // Away for registration's first nine tries; then registered, and
    // stopped once it has served a round.
    let run = serve(&patient(60_000), host(Away::new(&hub, 9)), None, |h, _| {
        h.rounds() >= 1
    })
    .await;
    assert_eq!(
        run.pauses,
        [100, 200, 400, 800, 1_600, 3_200, 5_000, 5_000, 5_000]
    );
    let host = run.outcome.unwrap();
    assert!(host.registration().is_some());
    let mut p = Patience::new(1_000);
    assert_eq!(
        (p.pause(), p.pause(), p.pause()),
        (Some(100), Some(200), Some(400))
    );
    p.answered();
    assert_eq!(
        p.pause(),
        Some(100),
        "a new outage starts at the first pause"
    );
}

#[tokio::test]
async fn the_stop_file_is_looked_for_between_tries() {
    let hub = MemoryHub::new(HostClock::at(0));
    let run = serve(
        &patient(60_000),
        host(Away::new(&hub, u32::MAX)),
        None,
        |_, pauses| pauses.len() == 2,
    )
    .await;
    assert_eq!(run.pauses, [100, 200]);
    let host = run.outcome.unwrap();
    assert_eq!(host.registration(), None, "it never registered");
}

#[tokio::test]
async fn any_other_refusal_ends_the_host_with_its_body() {
    let hub = MemoryHub::new(HostClock::at(0));
    let wrong = FakeHost::new(hub, "shelf", Notation::Dts, DESK);
    let run = serve(&patient(60_000), wrong, None, |_, _| false).await;
    let e = run.outcome.err().unwrap();
    assert!(e.starts_with("register: 400 "), "{e}");
    assert!(e.contains("name_mismatch"), "{e}");
    assert_eq!(run.pauses, [] as [u64; 0]);
}

#[tokio::test]
async fn a_host_started_again_goes_on_as_the_id_in_its_file_and_is_sent_what_waited() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("registration");
    let deliveries = dir.path().join("deliveries.jsonl");
    let serving = Serving {
        registration_file: Some(&file),
        deliveries: Some(&deliveries),
        retry_for_ms: 0,
    };
    let hub = MemoryHub::new(HostClock::at(0));
    // The first process registers and stops at once.
    let first = serve(&serving, host(hub.clone()), None, |h, _| {
        h.registration().is_some()
    })
    .await;
    let id = first.outcome.unwrap().registration().unwrap().to_string();
    assert_eq!(read_registration(&file), Ok(Some(id.clone())));
    let registered: Value = serde_json::from_str(first.out.trim()).unwrap();
    assert_eq!(registered["registration"], json!(id));
    // A call for it waits; a second process reads the file.
    let call = hub
        .call(
            &id,
            "run-1",
            "desk.lamps",
            vec![],
            "run-1:1:0",
            Some(30_000),
        )
        .unwrap();
    let meant = read_registration(&file).unwrap();
    let second = serve(&serving, host(hub.clone()), meant.as_deref(), |h, _| {
        h.applied("run-1:1:0") == 1
    })
    .await;
    let second = second.outcome.unwrap();
    assert_eq!(second.registration(), Some(id.as_str()));
    assert_eq!(second.calls_of("lamps").len(), 1);
    assert_eq!(
        hub.outcome(&call),
        Some(Ok(json!(["l1"]))),
        "the answer went with the last round, on stopping"
    );
    let lines = std::fs::read_to_string(&deliveries).unwrap();
    let seen: Vec<Value> = lines
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(seen.len(), 1, "{lines}");
    assert_eq!(seen[0]["id"], json!(call));
}

#[tokio::test]
async fn an_id_the_daemon_never_gave_is_registered_afresh_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("registration");
    let never = "0".repeat(32);
    write_registration(&file, &never).unwrap();
    let serving = Serving {
        registration_file: Some(&file),
        deliveries: None,
        retry_for_ms: 0,
    };
    let hub = MemoryHub::new(HostClock::at(0));
    let run = serve(&serving, host(hub), Some(&never), |h, _| {
        h.registration().is_some()
    })
    .await;
    let id = run.outcome.unwrap().registration().unwrap().to_string();
    assert_ne!(id, never);
    assert_eq!(read_registration(&file), Ok(Some(id)));
}

/// [`MemoryHub`] behind a token: any other is refused, as a daemon that
/// restarted refuses its old one.
struct Locked {
    hub: MemoryHub,
    token: String,
}

impl Locked {
    fn refused(&self) -> Option<(u16, Value)> {
        (self.token != "new").then(|| {
            (
                401,
                json!({"error": {"code": "unauthorized", "message": "invalid token"}}),
            )
        })
    }
}

#[async_trait::async_trait]
impl HostTransport for Locked {
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value) {
        match self.refused() {
            Some(refused) => refused,
            None => self.hub.put_surface(name, body).await,
        }
    }
    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value) {
        match self.refused() {
            Some(refused) => refused,
            None => self.hub.serve(name, body).await,
        }
    }
    async fn start_run(&self, name: &str, body: &StartRunRequest) -> (u16, Value) {
        match self.refused() {
            Some(refused) => refused,
            None => self.hub.start_run(name, body).await,
        }
    }
}

#[tokio::test]
async fn a_refused_token_is_read_again_and_tried_within_the_budget() {
    let hub = MemoryHub::new(HostClock::at(0));
    let locked = Locked {
        hub: hub.clone(),
        token: "old".into(),
    };
    // The token file holds the old token for two more reads.
    let mut reads = 0;
    let run = serve_with(
        &patient(60_000),
        host(locked),
        None,
        |h, _| h.registration().is_some(),
        |t: &mut Locked| {
            reads += 1;
            if reads > 2 {
                t.token = "new".into();
            }
        },
    )
    .await;
    assert_eq!(run.pauses, [100, 200, 400]);
    assert!(run.outcome.unwrap().registration().is_some());
    // With no budget, a refused token ends the host as before.
    let locked = Locked {
        hub,
        token: "old".into(),
    };
    let run = serve(&patient(0), host(locked), None, |_, _| false).await;
    let e = run.outcome.err().unwrap();
    assert!(e.starts_with("register: 401 "), "{e}");
}

#[test]
fn a_token_file_is_the_token_or_a_homes_daemon_json() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("token");
    std::fs::write(&plain, "cx_abc\n").unwrap();
    assert_eq!(read_token(plain.to_str().unwrap()), Ok("cx_abc".into()));
    let info = dir.path().join("daemon.json");
    std::fs::write(&info, r#"{"pid": 1, "port": 7, "token": "cx_def"}"#).unwrap();
    assert_eq!(read_token(info.to_str().unwrap()), Ok("cx_def".into()));
    std::fs::write(&info, r#"{"pid": 1}"#).unwrap();
    assert!(
        read_token(info.to_str().unwrap())
            .unwrap_err()
            .ends_with("holds no token")
    );
}

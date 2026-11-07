//! The door: who may ask, in which version, and what a request may hold.

use crate::fixture::{Host, INSTANCE, QUEUE, TOKEN, World, job, run};
use clax_bot::workspace::{CODE_SHELL_UNAVAILABLE, Offer, PreviewSink, Workspace};
use clax_bot_remote::wire::{WIRE_HEADER, WIRE_INSTANCE_HEADER, WIRE_VERSION, WireFailure};
use clax_bot_workspace::ledger::{Entry, Ledger};
use clax_bot_workspace_service::routes::MAX_BODY_BYTES;
use clax_bot_workspace_service::settings::{
    DEFAULT_ADDR, ENV_ADDR, ENV_HOME, ENV_INSTANCE, ENV_TOKEN,
};
use clax_bot_workspace_service::{BearerToken, Settings};
use serde_json::json;

/// Every route, and what is not one.
const ROUTES: [(&str, &str); 8] = [
    ("POST", "/v1/jobs"),
    ("GET", "/v1/jobs/k1?wait=20"),
    ("DELETE", "/v1/runs/r1"),
    ("POST", "/v1/reap"),
    ("GET", "/v1/previews"),
    ("POST", "/v1/previews/ack"),
    ("GET", "/no/such/route"),
    ("PUT", "/v1/jobs"),
];

#[tokio::test]
async fn a_wrong_token_is_refused_on_every_route_before_anything_else() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let wrong = format!("Bearer {}", TOKEN.replace('a', "b"));
    let short = "Bearer a".to_string();
    let basic = format!("Basic {TOKEN}");
    let bare = TOKEN.to_string();
    for (method, path) in ROUTES {
        let body = job("k1", "r1", "date").to_string().into_bytes();
        for auth in [&wrong, &short, &basic, &bare] {
            // The wrong version too: the token is looked at first.
            let headers = [
                ("authorization", auth.as_str()),
                (WIRE_HEADER, "0"),
                (WIRE_INSTANCE_HEADER, "another"),
            ];
            let (status, _) = h.raw(method, path, &headers, body.clone()).await;
            assert_eq!(status, 401, "{method} {path}");
        }
        let (status, _) = h.raw(method, path, &[], body).await;
        assert_eq!(status, 401, "{method} {path}");
    }
    assert_eq!(w.runtime.made(), 0);
}

#[tokio::test]
async fn a_host_with_the_wrong_token_fails_closed() {
    let w = World::new();
    let service = w.service();
    let h = Host::new(&service, "not the secret");
    let r = run("r1");
    let e = h.remote.exec_keyed(&r, None, "r1:1:0", "date", None, 60);
    let e = e.await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.message.contains("credentials"), "{}", e.message);
    assert!(!e.message.contains(TOKEN));
}

#[tokio::test]
async fn another_wire_version_is_refused_on_every_route() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let bearer = format!("Bearer {TOKEN}");
    let auth = ("authorization", bearer.as_str());
    for (method, path) in ROUTES {
        let wrong = (WIRE_VERSION + 1).to_string();
        let tries = [
            vec![auth, (WIRE_HEADER, wrong.as_str())],
            vec![auth, (WIRE_HEADER, "one")],
            vec![auth],
        ];
        for headers in tries {
            let body = job("k1", "r1", "date").to_string().into_bytes();
            let (status, answer) = h.raw(method, path, &headers, body).await;
            assert_eq!(status, 426, "{method} {path}");
            let failure: WireFailure = serde_json::from_value(answer).unwrap();
            assert_eq!(failure, WireFailure::skew());
        }
    }
    assert_eq!(w.runtime.made(), 0);
}

#[tokio::test]
async fn a_body_past_the_limit_is_refused_and_starts_nothing() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let flood = "x".repeat(MAX_BODY_BYTES + 1);
    let (status, _) = h
        .call("POST", "/v1/jobs", Some(job("k1", "r1", &flood)))
        .await;
    assert_eq!(status, 413);
    let (status, _) = h.call("GET", "/v1/jobs/k1", None).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_body_that_is_no_request_is_refused_in_the_wires_words() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let (status, answer) = h.call("POST", "/v1/jobs", Some(json!({"key": 1}))).await;
    assert_eq!(status, 400);
    assert_eq!(answer["code"], "bad_request");
}

#[tokio::test]
async fn an_id_that_could_leave_its_directory_is_refused() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let outside = w.root.path().join("escaped.json");
    std::fs::create_dir_all(w.ledger_dir()).unwrap();
    for id in [
        "..%2Fescaped",
        "..",
        "a%2Fb",
        "a%5Cb",
        "%2E%2E",
        ".hidden",
        "a%00b",
    ] {
        let (status, _) = h.call("DELETE", &format!("/v1/runs/{id}"), None).await;
        assert_eq!(status, 400, "{id}");
        let (status, _) = h.call("GET", &format!("/v1/jobs/{id}"), None).await;
        assert_eq!(status, 400, "{id}");
    }
    let long = "a".repeat(1000);
    assert_eq!(
        h.call("DELETE", &format!("/v1/runs/{long}"), None).await.0,
        400
    );
    assert_eq!(
        h.call("GET", &format!("/v1/jobs/{long}"), None).await.0,
        400
    );
    for bad in ["../escaped", "", "a/b", "a\\b", "a\nb"] {
        let (status, _) = h
            .call("POST", "/v1/jobs", Some(job("k1", bad, "date")))
            .await;
        assert_eq!(status, 400, "{bad:?}");
        let (status, _) = h
            .call("POST", "/v1/jobs", Some(job(bad, "r1", "date")))
            .await;
        assert_eq!(status, 400, "{bad:?}");
        let mut on_thread = job("k1", "r1", "date");
        on_thread["run"]["thread_ids"] = json!([bad]);
        let (status, _) = h.call("POST", "/v1/jobs", Some(on_thread)).await;
        assert_eq!(status, 400, "{bad:?}");
    }
    assert!(!outside.exists());
    assert_eq!(w.runtime.made(), 0);
}

#[tokio::test]
async fn a_real_key_and_run_id_pass() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let id = "01KA2B3C4D5E6F7G8H9J0KMNPQ";
    let key = format!("{id}:12:3");
    let (status, _) = h
        .call("POST", "/v1/jobs", Some(job(&key, id, "date")))
        .await;
    assert_eq!(status, 200);
    let (status, _) = h.call("GET", &format!("/v1/jobs/{key}"), None).await;
    assert_eq!(status, 200);
}

#[test]
fn it_listens_on_loopback_unless_told_otherwise() {
    let settings = Settings::read(|name| match name {
        ENV_TOKEN => Some(TOKEN.to_string()),
        ENV_HOME => Some("/srv/workspace".to_string()),
        ENV_INSTANCE => Some(INSTANCE.to_string()),
        _ => None,
    });
    let settings = settings.unwrap();
    assert!(settings.addr.ip().is_loopback());
    assert_eq!(settings.addr.to_string(), DEFAULT_ADDR);
}

#[test]
fn a_missing_or_short_secret_stops_it_and_no_message_repeats_one() {
    let home = |name: &str| match name {
        ENV_HOME => Some("/srv".to_string()),
        ENV_INSTANCE => Some(INSTANCE.to_string()),
        _ => None,
    };
    assert!(Settings::read(home).is_err());
    let short = |name: &str| match name {
        ENV_TOKEN => Some("hunter2".to_string()),
        n => home(n),
    };
    let Err(e) = Settings::read(short) else {
        panic!("a short secret was taken");
    };
    assert!(!format!("{e:#}").contains("hunter2"));
    assert!(BearerToken::new("hunter2".to_string()).is_err());
}

#[test]
fn an_address_that_cannot_be_read_stops_it() {
    let settings = Settings::read(|name| match name {
        ENV_TOKEN => Some(TOKEN.to_string()),
        ENV_HOME => Some("/srv".to_string()),
        ENV_INSTANCE => Some(INSTANCE.to_string()),
        ENV_ADDR => Some("nowhere".to_string()),
        _ => None,
    });
    assert!(settings.is_err());
}

#[tokio::test]
async fn another_instance_is_refused_on_every_route_after_the_token() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let bearer = format!("Bearer {TOKEN}");
    let wire = WIRE_VERSION.to_string();
    let good = [
        ("authorization", bearer.as_str()),
        (WIRE_HEADER, wire.as_str()),
    ];
    for (method, path) in ROUTES {
        let body = job("k1", "r1", "date").to_string().into_bytes();
        let other = [good[0], good[1], (WIRE_INSTANCE_HEADER, "another")];
        let (status, answer) = h.raw(method, path, &other, body.clone()).await;
        assert_eq!(status, 403, "{method} {path}");
        assert_eq!(answer["code"], "wrong_instance");
        let (status, _) = h.raw(method, path, &good, body).await;
        assert_eq!(status, 403, "{method} {path}");
    }
    assert_eq!(w.runtime.made(), 0);
}

#[tokio::test]
async fn a_second_worker_with_the_same_secret_reaps_and_acknowledges_nothing() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.remote
        .exec_keyed(&run("r1"), None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    service.previews().preview_missed("r1").await.unwrap();

    let second = Host::of_instance(&service, "instance-2");
    second.remote.reap(&[]).await;
    assert!(second.remote.poll_previews().await.is_err());
    second.remote.finish(&run("r1")).await;

    assert_eq!(w.runtime.live(), 1);
    assert_eq!(service.previews().events().len(), 1);
    assert!(second.sink.told.lock().unwrap().is_empty());
}

#[test]
fn a_service_with_no_instance_does_not_start() {
    let settings = Settings::read(|name| match name {
        ENV_TOKEN => Some(TOKEN.to_string()),
        ENV_HOME => Some("/srv".to_string()),
        _ => None,
    });
    assert!(settings.is_err());
    let settings = Settings::read(|name| match name {
        ENV_TOKEN => Some(TOKEN.to_string()),
        ENV_HOME => Some("/srv".to_string()),
        ENV_INSTANCE => Some("not\nan id".to_string()),
        _ => None,
    });
    assert!(settings.is_err());
}

/// Every word of an offer, none at its default.
fn every_word() -> Offer {
    Offer {
        shell: true,
        change: true,
        confidential: true,
        reviewed_previews: true,
        wake_on_reply: true,
        no_follow_ups: true,
        answers_only: true,
        unsure: true,
        guidance: Some("Run the tests with just test.".to_string()),
        preview_gate: Some("comment /preview <sha>".to_string()),
    }
}

#[tokio::test]
async fn every_word_of_a_kept_offer_crosses_the_wire() {
    let w = World::new();
    let entry = Entry {
        offer: Some(every_word()),
        ..Entry::default()
    };
    Ledger::at(w.ledger_dir()).write("r1", &entry).unwrap();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    assert_eq!(w.workspace().offer(&r, None), every_word());
    h.remote.reap(&["r1".to_string()]).await;
    assert_eq!(h.remote.offer(&r, None), every_word());
}

#[tokio::test]
async fn every_word_a_target_can_say_crosses_the_wire() {
    let mut w = World::new();
    w.telling = true;
    let service = w.service();
    let h = w.host(&service);
    h.remote.reap(&[]).await;
    let got = h.remote.offer(&run("fresh"), Some(QUEUE));
    assert_eq!(got, w.workspace().offer(&run("fresh"), Some(QUEUE)));
    let said = Offer {
        answers_only: false,
        unsure: false,
        ..every_word()
    };
    assert_eq!(got, said);
}

/// Keeps what is logged, for a test to read.
#[derive(Clone, Default)]
struct Lines(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn each_job_begun_leaves_an_audit_line_and_never_its_command() {
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let mut body = job("r1:1:0", "r1", "cat a-secret-file");
    body["queue"] = json!("some-queue");
    let (status, _) = h.call("POST", "/v1/jobs", Some(body.clone())).await;
    assert_eq!(status, 200);
    // A join begins nothing.
    h.call("POST", "/v1/jobs", Some(body)).await;

    let text = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
    let audits: Vec<&str> = text.lines().filter(|l| l.contains("a job began")).collect();
    assert_eq!(audits.len(), 1, "{text}");
    for word in [
        "run=\"r1\"",
        "key=\"r1:1:0\"",
        "queue=\"some-queue\"",
        "kind=\"exec\"",
    ] {
        assert!(audits[0].contains(word), "{word} in {}", audits[0]);
    }
    assert!(!text.contains("a-secret-file"), "{text}");
}

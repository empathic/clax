//! Report queues' webhooks (spec 2026-10-27-clax-sdk-design §9,
//! `routes/queues.rs`, `clax_http::webhooks`): an admin names an endpoint
//! and sees its secret once; it is sent one signed verification and no
//! report event until it answers it rightly, and Verify again is capped;
//! a report's change reaches a verified endpoint signed, with a stable
//! event ID; a failing endpoint is retried on the state's clock, given up
//! and marked failing, redelivered by hand, and disabled after
//! DISABLE_AFTER give-ups in a row until re-enabled and verified again; a
//! redirect is not followed; an endpoint URL that is not https (nor
//! loopback http on a local Clax) is refused. Each delivery pass is run by
//! the test (`deliver_due`), against an in-process receiver.

use clax_core::audit::AuditCtx;
use clax_core::webhook::{
    DISABLE_AFTER, MAX_ANSWER_BYTES, MAX_ATTEMPTS, Payload, VERIFY_EVENT, VERIFY_SENDS_PER_HOUR,
    VERIFY_SENDS_PER_QUEUE_HOUR, backoff, challenge_answer, verify,
};
use clax_core::working::{Clock, ManualClock};
use clax_http::webhooks::deliver_due;
use clax_store::queues::{ExternalScope, NewReport, ReporterToken};
use clax_testkit::webhook_receiver::{Received, Receiver};
use clax_testkit::{TestServer, element_anchor};
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;

struct W {
    ts: TestServer,
    clock: Arc<ManualClock>,
    owner: String,
    qid: String,
    rx: Receiver,
}

async fn call(w: &W, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut r =
        w.ts.client
            .request(m, format!("{}{path}", w.ts.base))
            .header("cookie", &w.owner);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    let text = res.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

async fn world() -> W {
    let clock = Arc::new(ManualClock::at("2026-11-24T12:00:00Z"));
    let ts = TestServer::spawn_with_clock(clock.clone()).await;
    let owner = ts.owner_cookie();
    let rx = Receiver::start().await;
    let mut w = W {
        ts,
        clock,
        owner,
        qid: String::new(),
        rx,
    };
    let (s, v) = call(
        &w,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "App reports", "origins": ["https://app.example.com"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    w.qid = v["queue"]["id"].as_str().unwrap().to_string();
    w
}

/// Names the receiver as an endpoint of the queue for `events`, not yet
/// sent its verification; answers its ID and secret.
async fn unverified(w: &W, events: Value) -> (String, String) {
    let (s, v) = call(
        w,
        Method::POST,
        &format!("/api/queues/{}/webhooks", w.qid),
        Some(json!({"url": w.rx.url, "events": events})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["webhook"]["state"], json!("unverified"), "{v}");
    (
        v["webhook"]["id"].as_str().unwrap().to_string(),
        v["secret"].as_str().unwrap().to_string(),
    )
}

/// Names the receiver as an endpoint of the queue for `events` and has it
/// answer its verification rightly; it answers 200 to anything else.
async fn hook(w: &W, events: Value) -> (String, String) {
    let (wid, secret) = unverified(w, events).await;
    w.rx.verify_with(&secret, 200);
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    let got = w.rx.take();
    assert!(got[0].is_verification());
    assert_eq!(endpoint(w, &wid).await["state"], json!("verified"));
    (wid, secret)
}

/// Endpoint `wid` as the listing answers it.
async fn endpoint(w: &W, wid: &str) -> Value {
    let (s, v) = call(
        w,
        Method::GET,
        &format!("/api/queues/{}/webhooks", w.qid),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v["webhooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["id"] == json!(wid))
        .unwrap()
        .clone()
}

/// `POST …/webhooks/<wid>/verify`.
async fn verify_again(w: &W, wid: &str) -> (u16, Value) {
    call(
        w,
        Method::POST,
        &format!("/api/queues/{}/webhooks/{wid}/verify", w.qid),
        None,
    )
    .await
}

async fn file(w: &W) -> String {
    let token = ReporterToken {
        queue_id: w.qid.clone(),
        service_grant_id: "g1".into(),
        reporter_id: "emp-1".into(),
        origin: "https://app.example.com".into(),
        public_key: String::new(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        profile: Some(clax_core::report::ReporterProfile {
            name: Some("Ana".into()),
            email: Some("ana@example.com".into()),
            github: None,
            avatar_url: None,
        }),
    };
    w.ts.store
        .file_report_touched_async(
            &AuditCtx::DAEMON,
            &w.qid,
            Some(&token),
            ExternalScope::Instance,
            NewReport {
                external_id: Some("invoice:9".into()),
                url: "https://app.example.com/invoices/9".into(),
                title: "Invoice 9".into(),
                anchor: serde_json::from_value(element_anchor()).unwrap(),
                body: "The total is wrong".into(),
                author_name: "emp-1".into(),
                metadata: Value::Null,
                revision: None,
                clip: None,
                context: None,
                server: None,
                diagnostics: None,
            },
        )
        .await
        .unwrap()
        .0
        .id
}

/// Endpoint `wid`'s whole delivery log, latest first.
async fn all_log(w: &W, wid: &str) -> Vec<Value> {
    let (s, v) = call(
        w,
        Method::GET,
        &format!("/api/queues/{}/webhooks/{wid}/deliveries", w.qid),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v["deliveries"].as_array().unwrap().clone()
}

/// Endpoint `wid`'s report deliveries, latest first.
async fn log(w: &W, wid: &str) -> Vec<Value> {
    all_log(w, wid)
        .await
        .into_iter()
        .filter(|d| d["event"] != json!(VERIFY_EVENT))
        .collect()
}

#[tokio::test]
async fn a_report_s_change_reaches_its_endpoint_signed() {
    let w = world().await;
    let (wid, secret) = hook(&w, json!(["report.created", "report.replied"])).await;
    assert!(secret.starts_with("whsec_"));
    // The secret is shown once: the listing never carries it.
    let (_, listed) = call(
        &w,
        Method::GET,
        &format!("/api/queues/{}/webhooks", w.qid),
        None,
    )
    .await;
    assert_eq!(listed["webhooks"][0]["id"], json!(wid));
    assert_eq!(
        listed["webhooks"][0]["events"],
        json!(["report.created", "report.replied"])
    );
    assert!(!listed.to_string().contains(&secret));

    let tid = file(&w).await;
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    let got = w.rx.take();
    assert_eq!(got.len(), 1);
    let r = &got[0];
    assert_eq!(r.path, "/hook");
    assert_eq!(r.header("content-type"), "application/json");
    assert_eq!(r.header("clax-event"), "report.created");
    let now = w.clock.now().timestamp();
    assert_eq!(r.header("clax-timestamp"), now.to_string());
    verify(
        &secret,
        r.header("clax-timestamp"),
        r.header("clax-signature"),
        &r.body,
        now,
    )
    .expect("the delivery verifies under the endpoint's secret");
    assert!(
        verify(
            "whsec_other",
            r.header("clax-timestamp"),
            r.header("clax-signature"),
            &r.body,
            now
        )
        .is_err()
    );
    let p: Payload = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(r.header("clax-event-id"), p.id);
    assert_eq!(p.kind, "report.created");
    assert_eq!(p.report.id, tid);
    assert_eq!(p.report.external_id, "invoice:9");
    assert_eq!(
        p.report.reporter.unwrap().email.as_deref(),
        Some("ana@example.com")
    );
    assert_eq!(
        p.report.link.as_deref(),
        Some(format!("{}/queues/{}/{tid}", w.ts.state.browser_base, w.qid).as_str())
    );
    let l = log(&w, &wid).await;
    assert_eq!(l[0]["state"], json!("delivered"));
    assert_eq!(l[0]["last_status"], json!(200));
    assert_eq!(l[0]["event_id"], json!(p.id));

    // Nothing is due now; a reply is.
    assert_eq!(deliver_due(&w.ts.state).await, 0);
    let (s, v) = call(
        &w,
        Method::POST,
        &format!("/api/queues/{}/reports/{tid}/comments", w.qid),
        Some(json!({"body": "Looking at it"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    let reply = w.rx.take().remove(0).json();
    assert_eq!(reply["type"], json!("report.replied"));
    assert_eq!(reply["comment"]["body"], json!("Looking at it"));
}

/// A failing endpoint is tried again after each backoff on the state's
/// clock and never sooner, under the same event ID; given up at
/// MAX_ATTEMPTS and marked failing; redelivered by hand, which clears the
/// mark once it is delivered.
#[tokio::test]
async fn a_failing_endpoint_is_retried_given_up_and_redelivered() {
    let w = world().await;
    let (wid, _) = hook(&w, json!([])).await;
    w.rx.answer(503);
    file(&w).await;
    let mut ids = Vec::new();
    for n in 1..=MAX_ATTEMPTS {
        assert_eq!(deliver_due(&w.ts.state).await, 1, "attempt {n}");
        ids.push(w.rx.take().remove(0).header("clax-event-id").to_string());
        if n < MAX_ATTEMPTS {
            let wait = backoff(n).as_secs() as i64;
            w.clock.advance(wait - 1);
            assert_eq!(deliver_due(&w.ts.state).await, 0, "not before its backoff");
            w.clock.advance(1);
        }
    }
    ids.dedup();
    assert_eq!(ids.len(), 1, "every attempt carries the one event ID");
    let l = log(&w, &wid).await;
    assert_eq!(l[0]["state"], json!("failed"));
    assert_eq!(l[0]["attempts"], json!(MAX_ATTEMPTS));
    assert_eq!(l[0]["last_status"], json!(503));
    assert_eq!(l[0]["last_error"], json!("receiver says so"));
    let (_, hooks) = call(
        &w,
        Method::GET,
        &format!("/api/queues/{}/webhooks", w.qid),
        None,
    )
    .await;
    assert!(hooks["webhooks"][0]["failing_since"].is_string());
    w.clock.advance(24 * 3600);
    assert_eq!(
        deliver_due(&w.ts.state).await,
        0,
        "a failed delivery is not tried again"
    );

    w.rx.answer(204);
    let did = l[0]["id"].as_str().unwrap();
    let (s, v) = call(
        &w,
        Method::POST,
        &format!(
            "/api/queues/{}/webhooks/{wid}/deliveries/{did}/redeliver",
            w.qid
        ),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["delivery"]["state"], json!("pending"));
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    assert_eq!(w.rx.take()[0].header("clax-event-id"), ids[0]);
    assert_eq!(log(&w, &wid).await[0]["state"], json!("delivered"));
    let (_, hooks) = call(
        &w,
        Method::GET,
        &format!("/api/queues/{}/webhooks", w.qid),
        None,
    )
    .await;
    assert!(hooks["webhooks"][0]["failing_since"].is_null());
}

/// A redirect is a failed attempt: the delivery never goes where it
/// points.
#[tokio::test]
async fn a_redirect_is_not_followed() {
    let w = world().await;
    let (wid, _) = hook(&w, json!([])).await;
    w.rx.answer(307);
    file(&w).await;
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    let got = w.rx.take();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].path, "/hook");
    let l = log(&w, &wid).await;
    assert_eq!(l[0]["state"], json!("pending"));
    assert_eq!(l[0]["last_status"], json!(307));
    assert!(l[0]["last_error"].as_str().unwrap().contains("redirect"));
}

#[tokio::test]
async fn an_endpoint_url_is_checked_and_an_endpoint_removed() {
    let w = world().await;
    for url in [
        "http://hooks.example.com/clax",
        "ftp://localhost/x",
        "https://user:pw@hooks.example.com/",
        "not a url",
    ] {
        let (s, v) = call(
            &w,
            Method::POST,
            &format!("/api/queues/{}/webhooks", w.qid),
            Some(json!({"url": url})),
        )
        .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (400, Some("invalid_url")),
            "{url}: {v}"
        );
    }
    let (s, v) = call(
        &w,
        Method::POST,
        &format!("/api/queues/{}/webhooks", w.qid),
        Some(json!({"url": w.rx.url, "events": ["report.deleted"]})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_events"))
    );
    let (wid, _) = hook(&w, json!([])).await;
    file(&w).await;
    let (s, _) = call(
        &w,
        Method::DELETE,
        &format!("/api/queues/{}/webhooks/{wid}", w.qid),
        None,
    )
    .await;
    assert_eq!(s, 204);
    assert_eq!(
        deliver_due(&w.ts.state).await,
        0,
        "its deliveries went with it"
    );
    let (s, _) = call(
        &w,
        Method::GET,
        &format!("/api/queues/{}/webhooks/{wid}/deliveries", w.qid),
        None,
    )
    .await;
    assert_eq!(s, 404);
}

/// Only the queue's admins configure its endpoints: the owner's agent is
/// refused.
#[tokio::test]
async fn agents_configure_no_endpoint() {
    let w = world().await;
    for (m, path) in [
        (Method::GET, format!("/api/queues/{}/webhooks", w.qid)),
        (Method::POST, format!("/api/queues/{}/webhooks", w.qid)),
    ] {
        let res =
            w.ts.authed(w.ts.client.request(m, format!("{}{path}", w.ts.base)))
                .header("x-clax-channel", "agent")
                .json(&json!({"url": w.rx.url}))
                .send()
                .await
                .unwrap();
        assert_eq!(res.status().as_u16(), 403, "{path}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], json!("not_for_agents"));
    }
}

/// A new endpoint is sent one signed verification at once and nothing
/// else; a wrong answer leaves it unverified, and the verification is not
/// tried again on the retry schedule. Verify again sends a new challenge;
/// the right answer verifies it, and only the reports' changes after that
/// reach it.
#[tokio::test]
async fn an_endpoint_is_sent_no_report_event_until_it_answers_its_verification() {
    let w = world().await;
    let (wid, secret) = unverified(&w, json!([])).await;
    let tid = file(&w).await;
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    let got = w.rx.take();
    assert_eq!(got.len(), 1, "the verification alone");
    let r = &got[0];
    assert_eq!(r.header("clax-event"), VERIFY_EVENT);
    let now = w.clock.now().timestamp();
    verify(
        &secret,
        r.header("clax-timestamp"),
        r.header("clax-signature"),
        &r.body,
        now,
    )
    .expect("the verification is signed with the endpoint's secret");
    let body = r.json();
    assert_eq!(body["type"], json!(VERIFY_EVENT));
    assert_eq!(body["id"].as_str().unwrap(), r.header("clax-event-id"));
    assert_eq!(body["webhook"], json!({"id": wid, "url": w.rx.url}));
    assert_eq!(body["queue"]["id"], json!(w.qid));
    let first = body["challenge"].as_str().unwrap().to_string();
    // The receiver answered 200 "receiver says so".
    let h = endpoint(&w, &wid).await;
    assert_eq!(h["state"], json!("unverified"));
    assert!(
        h["last_error"]
            .as_str()
            .unwrap()
            .contains("not a JSON object"),
        "{h}"
    );
    assert!(h["verify_by"].is_string());
    let l = all_log(&w, &wid).await;
    assert_eq!(l.len(), 1);
    assert_eq!(
        (
            &l[0]["event"],
            &l[0]["state"],
            &l[0]["attempts"],
            &l[0]["report_id"]
        ),
        (
            &json!(VERIFY_EVENT),
            &json!("failed"),
            &json!(1),
            &Value::Null
        )
    );
    w.clock.advance(backoff(1).as_secs() as i64 * 10);
    assert_eq!(
        deliver_due(&w.ts.state).await,
        0,
        "a verification is tried once"
    );

    w.rx.verify_with(&secret, 200);
    let (s, v) = verify_again(&w, &wid).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["webhook"]["state"], json!("unverified"));
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    let again = w.rx.take().remove(0).json();
    assert_ne!(
        again["challenge"].as_str().unwrap(),
        first,
        "a new challenge"
    );
    let h = endpoint(&w, &wid).await;
    assert_eq!(h["state"], json!("verified"));
    assert!(h["verified_at"].is_string() && h["verify_by"].is_null());
    assert_eq!(deliver_due(&w.ts.state).await, 0, "no event is back-filled");
    let (s, v) = verify_again(&w, &wid).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_verified"))
    );

    let (s, v) = call(
        &w,
        Method::POST,
        &format!("/api/queues/{}/reports/{tid}/comments", w.qid),
        Some(json!({"body": "Looking at it"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    assert_eq!(w.rx.take()[0].json()["type"], json!("report.replied"));
}

/// Only a 2xx JSON object of at most MAX_ANSWER_BYTES whose `answer` is
/// the challenge's HMAC under the endpoint's secret verifies: not another
/// status, an echo of the request, the challenge itself, another secret's
/// HMAC, an answer too long, or a redirect to one that would be right.
#[tokio::test]
async fn only_the_right_answer_verifies() {
    let w = world().await;
    let (wid, secret) = unverified(&w, json!([])).await;
    let right = |secret: String| {
        move |r: &Received| challenge_answer(&secret, r.json()["challenge"].as_str().unwrap())
    };
    type Wrong = Box<dyn Fn(&Received) -> (u16, Vec<u8>) + Send + Sync>;
    let cases: Vec<(&str, Wrong)> = vec![
        ("a non-2xx", {
            let a = right(secret.clone());
            Box::new(move |r: &Received| (500, json!({"answer": a(r)}).to_string().into_bytes()))
        }),
        (
            "an echo of the request",
            Box::new(|r: &Received| (200, r.body.clone())),
        ),
        (
            "the challenge itself",
            Box::new(|r: &Received| {
                (
                    200,
                    json!({"answer": r.json()["challenge"]})
                        .to_string()
                        .into_bytes(),
                )
            }),
        ),
        ("another secret's HMAC", {
            let a = right("whsec_other".into());
            Box::new(move |r: &Received| (200, json!({"answer": a(r)}).to_string().into_bytes()))
        }),
        ("an answer too long", {
            let a = right(secret.clone());
            Box::new(move |r: &Received| {
                let pad = "x".repeat(MAX_ANSWER_BYTES);
                (
                    200,
                    json!({"answer": a(r), "pad": pad}).to_string().into_bytes(),
                )
            })
        }),
        ("a redirect", Box::new(|_: &Received| (307, Vec::new()))),
    ];
    for (n, (why, f)) in cases.into_iter().enumerate() {
        if n > 0 {
            // Within the hour's cap: an hour on each time.
            w.clock.advance(3600);
            let (s, v) = verify_again(&w, &wid).await;
            assert_eq!(s, 200, "{why}: {v}");
        }
        w.rx.respond(f);
        assert_eq!(deliver_due(&w.ts.state).await, 1, "{why}");
        let got = w.rx.take();
        assert_eq!(got.len(), 1, "{why}: one request, no redirect followed");
        assert_eq!(got[0].path, "/hook");
        let h = endpoint(&w, &wid).await;
        assert_eq!(h["state"], json!("unverified"), "{why}");
        assert_eq!(
            all_log(&w, &wid).await[0]["state"],
            json!("failed"),
            "{why}"
        );
    }
    w.clock.advance(3600);
    w.rx.verify_with(&secret, 200);
    assert_eq!(verify_again(&w, &wid).await.0, 200);
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    assert_eq!(endpoint(&w, &wid).await["state"], json!("verified"));
}

/// Verify again is refused past VERIFY_SENDS_PER_HOUR verifications in an
/// hour, the one sent at naming included; an hour on, it is taken again.
#[tokio::test]
async fn verify_again_is_capped() {
    let w = world().await;
    let (wid, _) = unverified(&w, json!([])).await;
    for _ in 1..VERIFY_SENDS_PER_HOUR {
        assert_eq!(verify_again(&w, &wid).await.0, 200);
    }
    let (s, v) = verify_again(&w, &wid).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (429, Some("limit_reached")),
        "{v}"
    );
    // PER_ENDPOINT a pass.
    let sent = deliver_due(&w.ts.state).await + deliver_due(&w.ts.state).await;
    assert_eq!(sent, VERIFY_SENDS_PER_HOUR);
    w.clock.advance(3600);
    assert_eq!(verify_again(&w, &wid).await.0, 200);
}

/// DISABLE_AFTER deliveries given up in a row disable an endpoint: it is
/// queued no event, and re-enabling it (Verify again) has it verified
/// again before any event is sent.
#[tokio::test]
async fn sustained_failure_disables_and_reenabling_verifies_again() {
    let w = world().await;
    let (wid, secret) = hook(&w, json!(["report.created"])).await;
    w.rx.verify_with(&secret, 503);
    for _ in 0..DISABLE_AFTER {
        file(&w).await;
    }
    // Every attempt of each, PER_ENDPOINT a pass, until all are given up.
    for _ in 0..3 * MAX_ATTEMPTS {
        deliver_due(&w.ts.state).await;
        w.clock.advance(6 * 3600);
    }
    let h = endpoint(&w, &wid).await;
    assert_eq!(h["state"], json!("disabled"), "{h}");
    assert_eq!(h["given_up"], json!(DISABLE_AFTER));
    w.rx.take();
    file(&w).await;
    assert_eq!(
        deliver_due(&w.ts.state).await,
        0,
        "a disabled endpoint is sent nothing"
    );
    let did = log(&w, &wid).await[0]["id"].as_str().unwrap().to_string();
    let (s, v) = call(
        &w,
        Method::POST,
        &format!(
            "/api/queues/{}/webhooks/{wid}/deliveries/{did}/redeliver",
            w.qid
        ),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_verified"))
    );

    w.rx.verify_with(&secret, 204);
    let (s, v) = verify_again(&w, &wid).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["webhook"]["state"], json!("unverified"));
    assert_eq!(v["webhook"]["given_up"], json!(0));
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    assert!(w.rx.take()[0].is_verification());
    assert_eq!(endpoint(&w, &wid).await["state"], json!("verified"));
    file(&w).await;
    assert_eq!(deliver_due(&w.ts.state).await, 1);
    assert_eq!(w.rx.take()[0].json()["type"], json!("report.created"));
    assert_eq!(log(&w, &wid).await[0]["state"], json!("delivered"));
}

/// Removing an endpoint and adding it again does not get round the
/// verification cap: past VERIFY_SENDS_PER_QUEUE_HOUR an hour a queue's
/// new endpoint is refused (429 `verifications_limited`), none is made
/// and nothing is sent; an hour on it is taken.
#[tokio::test]
async fn removing_and_adding_again_is_capped_per_queue() {
    let w = world().await;
    for _ in 0..VERIFY_SENDS_PER_QUEUE_HOUR {
        let (wid, _) = unverified(&w, json!([])).await;
        let (s, _) = call(
            &w,
            Method::DELETE,
            &format!("/api/queues/{}/webhooks/{wid}", w.qid),
            None,
        )
        .await;
        assert_eq!(s, 204);
    }
    let (s, v) = call(
        &w,
        Method::POST,
        &format!("/api/queues/{}/webhooks", w.qid),
        Some(json!({"url": w.rx.url})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (429, Some("verifications_limited")),
        "{v}"
    );
    let (_, listed) = call(
        &w,
        Method::GET,
        &format!("/api/queues/{}/webhooks", w.qid),
        None,
    )
    .await;
    assert_eq!(listed["webhooks"], json!([]));
    assert_eq!(deliver_due(&w.ts.state).await, 0);
    assert!(w.rx.take().is_empty(), "nothing was sent");
    w.clock.advance(3600);
    unverified(&w, json!([])).await;
}

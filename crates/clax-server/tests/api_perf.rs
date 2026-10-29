//! The latency gate's hidden routes: `POST /api/admin/perf/calibrate`, the
//! calibration read, for the token only, one at a time; `GET
//! /api/admin/perf/verify`, the gateway's latest signature verification
//! times, for the token only.

use clax_testkit::TestServer;
use serde_json::Value;

const PATH: &str = "/api/admin/perf/calibrate";

#[tokio::test]
async fn the_token_times_the_calibration_read_each_call() {
    let ts = TestServer::spawn().await;
    for _ in 0..2 {
        let res = ts.post_json(PATH, serde_json::json!({})).await;
        assert_eq!(res.status(), 200);
        let v: Value = res.json().await.unwrap();
        assert!(v["ms"].as_f64().unwrap() > 0.0, "{v}");
    }
}

#[tokio::test]
async fn every_other_caller_is_refused() {
    let ts = TestServer::spawn().await;
    let events = ts.events_cookie();
    let url = format!("{}{PATH}", ts.base);
    for (who, cookie) in [
        ("no credential", None),
        ("a browser of the owner's", Some(ts.owner_cookie())),
        ("a viewer", Some("clax_viewer=v".to_string())),
        ("the event streams' cookie", Some(events)),
    ] {
        let mut req = ts.client.post(&url);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 401, "{who}");
    }
    // The token narrowed as an agent's db tools narrow it.
    for level in ["view", "interact", "admin"] {
        let res = ts
            .post_json(&format!("{PATH}?as_level={level}"), serde_json::json!({}))
            .await;
        assert_eq!(res.status(), 403, "{level}");
    }
}

#[tokio::test]
async fn a_call_while_one_runs_is_refused() {
    let mut slot = None;
    let ts = TestServer::spawn_with(|s| slot = Some(s.calibration.clone())).await;
    let running = slot.unwrap().try_lock_owned().unwrap();
    let res = ts.post_json(PATH, serde_json::json!({})).await;
    assert_eq!(res.status(), 409);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "busy", "{v}");
    drop(running);
    let res = ts.post_json(PATH, serde_json::json!({})).await;
    assert_eq!(res.status(), 200);
}

const VERIFY: &str = "/api/admin/perf/verify";

#[tokio::test]
async fn the_gateway_times_each_signature_it_verifies() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let times = || async {
        let r = ts.authed(ts.client.get(format!("{}{VERIFY}", ts.base)));
        r.send().await.unwrap().json::<Value>().await.unwrap()
    };
    let v = times().await;
    assert_eq!(v, serde_json::json!({"count": 0, "median_us": null}));
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    for _ in 0..3 {
        assert_eq!(sam.get(None, "/api/sessions?live=true").await.status(), 200);
    }
    let v = times().await;
    assert_eq!(v["count"], 3, "{v}");
    assert!(v["median_us"].as_f64().is_some(), "{v}");
}

#[tokio::test]
async fn only_the_token_reads_the_verification_times() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let url = format!("{}{VERIFY}", ts.base);
    for (who, cookie) in [
        ("no credential", None),
        ("a browser of the owner's", Some(ts.owner_cookie())),
        ("a viewer", Some("clax_viewer=v".to_string())),
    ] {
        let mut req = ts.client.get(&url);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        assert_eq!(req.send().await.unwrap().status(), 401, "{who}");
    }
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    assert_eq!(sam.get(None, VERIFY).await.status(), 403);
}

//! `POST /api/admin/perf/calibrate`: the latency gate's calibration read,
//! for the token only, one at a time.

use crate::common;
use common::TestServer;
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
    let host = ts.base.trim_start_matches("http://");
    let events = format!(
        "{}={}",
        clax_server::auth::events_cookie_name(host),
        clax_server::auth::events_cookie_value(&ts.token)
    );
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

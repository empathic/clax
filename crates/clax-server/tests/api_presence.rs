mod common;
use clax_core::presence::Presence;
use clax_core::working::ManualClock;
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn presence_is_reported_by_viewers_announced_and_lapses() {
    let c = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let pc = c.clone();
    let ts = TestServer::spawn_with(move |s| s.presence = Arc::new(Presence::new(pc))).await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let alex = ts.viewer(Some("Alex")).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=presence")).await;
    let put = |state: &'static str, origin: Option<&'static str>| {
        let (ts, aid, cookie) = (&ts, aid.clone(), alex.cookie.clone());
        async move {
            let mut r = ts
                .client
                .put(format!("{}/api/viewers/me/presence", ts.base))
                .header("cookie", format!("clax_viewer={cookie}"))
                .json(&json!({"artifact_id": aid, "state": state, "where": "«Quarterly goals»"}));
            if let Some(o) = origin {
                r = r.header("origin", o);
            }
            r.send().await.unwrap()
        }
    };
    assert_eq!(put("here", None).await.status(), 200);
    let e = ev.next_named("presence").await;
    assert_eq!(e["people"][0]["display_name"], "Alex");
    assert_eq!(e["people"][0]["state"], "here");
    assert_eq!(e["people"][0]["where"], "«Quarterly goals»");
    assert!(!e.to_string().contains(&alex.cookie), "never the cookie");
    assert_eq!(put("here", Some("http://evil.example")).await.status(), 403);
    let anon = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .json(&json!({"artifact_id": aid, "state": "here"}))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 400);
    c.advance(91);
    clax_server::presence::sweep_and_announce(&ts.presence, &ts.events);
    assert_eq!(
        ev.next_named("presence").await["people"][0]["state"],
        "gone"
    );
    let g: Value = ts
        .get(&format!("/api/artifacts/{aid}/presence"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(g["people"][0]["state"], "gone");
}

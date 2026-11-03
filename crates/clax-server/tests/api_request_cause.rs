//! Every request is named (`x-request-id`, `Server-Timing`), and the events
//! it publishes carry that name as their `cause` on `/api/events`.

use clax_testkit::TestServer;

#[tokio::test]
async fn an_event_names_the_request_that_caused_it() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Cause", &[("index.html", "<p>hi</p>")]).await;
    let aid = a["artifact"]["id"]
        .as_str()
        .or_else(|| a["id"].as_str())
        .expect("an artifact id")
        .to_owned();
    let mut events = ts.events(&format!("?artifact={aid}")).await;
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{aid}", ts.base))
                .header("x-request-id", "browser-click-7"),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(res.headers()["x-request-id"], "browser-click-7");
    let timing = res.headers()["server-timing"].to_str().unwrap();
    assert!(timing.contains("req;desc=\"browser-click-7\""), "{timing}");
    assert!(timing.contains("app;dur="), "{timing}");
    loop {
        let (name, data) = events.next().await;
        if name == "artifact_deleted" {
            assert_eq!(events.cause(), Some("browser-click-7"), "{data}");
            break;
        }
    }
}

#[tokio::test]
async fn an_event_published_outside_a_request_names_no_cause() {
    let ts = TestServer::spawn().await;
    let mut events = ts.events("?artifact=a1").await;
    ts.events.publish(clax_core::Event::ArtifactDeleted {
        artifact_id: "a1".into(),
    });
    loop {
        let (name, data) = events.next().await;
        if name == "artifact_deleted" {
            assert_eq!(events.cause(), None, "{data}");
            break;
        }
    }
}

#[tokio::test]
async fn an_answer_without_a_caller_id_gets_a_minted_one() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .get(format!("{}/api/health", ts.base))
        .send()
        .await
        .unwrap();
    let id = res.headers()["x-request-id"].to_str().unwrap();
    assert!(clax_http::request_id::valid(id), "{id}");
}

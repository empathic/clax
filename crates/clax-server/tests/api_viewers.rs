mod common;
use common::TestServer;
use serde_json::Value;

#[tokio::test]
async fn lookups_name_viewers_by_public_id_and_never_return_cookies() {
    let ts = TestServer::spawn().await;
    let a = ts.viewer(Some("Alex")).await;
    let b = ts.viewer(None).await;
    let res = ts
        .get(&format!(
            "/api/viewers?ids={},{},u_ffffffffffffffffffffff",
            a.public_id, b.public_id
        ))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let text = v.to_string();
    assert!(!text.contains(&a.cookie) && !text.contains(&b.cookie));
    let mut got: Vec<(String, Value)> = v["viewers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["id"].as_str().unwrap().to_string(),
                x["display_name"].clone(),
            )
        })
        .collect();
    got.sort_by(|x, y| x.0.cmp(&y.0));
    let mut want = vec![
        (a.public_id.clone(), Value::from("Alex")),
        (b.public_id.clone(), Value::Null),
    ];
    want.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(got, want);
    let many = vec![a.public_id.as_str(); 65].join(",");
    assert_eq!(
        ts.get(&format!("/api/viewers?ids={many}")).await.status(),
        400
    );
    assert_eq!(
        ts.get(&format!("/api/viewers?ids={}", a.cookie))
            .await
            .status(),
        400,
        "cookie values are not public IDs"
    );
    assert_eq!(ts.get("/api/viewers").await.status(), 400);
    assert_eq!(
        ts.get_authed(&format!("/api/viewers?ids={}&q=al", a.public_id))
            .await
            .status(),
        400,
        "ids and q together"
    );
}

async fn names(ts: &TestServer, q: &str) -> Vec<String> {
    let v: Value = ts
        .get_authed(&format!("/api/viewers?q={q}"))
        .await
        .json()
        .await
        .unwrap();
    v["viewers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["display_name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn search_returns_at_most_eight_and_matches_literally() {
    let ts = TestServer::spawn().await;
    for i in 0..10 {
        ts.viewer(Some(&format!("Member {i}"))).await;
    }
    ts.viewer(Some("50% off")).await;
    ts.viewer(Some("a_b")).await;
    ts.viewer(Some("axb")).await;
    ts.viewer(Some("Ärger")).await;
    let members = names(&ts, "member").await;
    assert_eq!(members.len(), 8);
    assert_eq!(members[0], "Member 0");
    assert_eq!(names(&ts, "%25").await, ["50% off"]);
    assert_eq!(names(&ts, "a_b").await, ["a_b"]);
    assert_eq!(names(&ts, "%C3%A4rger").await, ["Ärger"]);
    assert!(names(&ts, &"m".repeat(61)).await.is_empty());
    assert!(names(&ts, "%20%20").await.is_empty());
}

#[tokio::test]
async fn search_needs_the_token() {
    let ts = TestServer::spawn().await;
    ts.viewer(Some("Alex")).await;
    assert_eq!(ts.get("/api/viewers?q=al").await.status(), 401);
    let v: Value = ts
        .get_authed("/api/viewers?q=al")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["viewers"][0]["display_name"], "Alex");
}

#[tokio::test]
async fn lookups_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .get(format!(
            "{}/api/viewers?ids=u_ffffffffffffffffffffff",
            ts.base
        ))
        .header("origin", "http://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

use clax_testkit::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn minting_needs_the_token_and_the_id_in_effect() {
    let ts = TestServer::spawn().await;
    let url = format!("{}/api/extension/credentials", ts.base);
    let id = ts.extension_id();
    assert_eq!(
        id,
        clax_core::extension::extension_id_in_effect(ts.home.root())
    );
    let body = json!({"extension_id": id});
    assert_eq!(
        ts.client
            .post(&url)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let res = ts
        .authed(ts.client.post(&url))
        .json(&json!({"extension_id": "a".repeat(32)}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_extension"
    );
    let res = ts
        .authed(ts.client.post(&url))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let cred = v["credential"].as_str().unwrap().to_string();
    assert!(clax_core::extension::is_credential(&cred), "{cred}");
    let owner = ts.owner_viewer().await;
    assert_eq!(
        v["viewer"]["public_id"],
        owner.public_id.as_str(),
        "the extension pairs as the owner"
    );
    assert!(
        v["viewer"].get("id").is_none(),
        "never the owner's cookie value"
    );
    assert_eq!(v["expires_in_s"], 30 * 86_400);
    let st: Value = ts.get_authed("/api/extension").await.json().await.unwrap();
    assert_eq!(st["live_credentials"], 1);
    assert_eq!(st["extension_id"], id.as_str());
    assert!(st["last_used_at"].is_string());
    assert_eq!(st["viewer"]["public_id"], owner.public_id.as_str());
    assert!(
        !st.to_string().contains(&cred),
        "status never shows a credential"
    );
    let r: Value = ts
        .authed(ts.client.delete(&url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["revoked"], 1);
    let st: Value = ts.get_authed("/api/extension").await.json().await.unwrap();
    assert_eq!(st["live_credentials"], 0);
}

#[tokio::test]
async fn status_and_revoke_need_the_token_and_status_never_makes_the_owner() {
    let ts = TestServer::spawn().await;
    assert_eq!(ts.get("/api/extension").await.status(), 401);
    let url = format!("{}/api/extension/credentials", ts.base);
    assert_eq!(ts.client.delete(&url).send().await.unwrap().status(), 401);
    let st: Value = ts.get_authed("/api/extension").await.json().await.unwrap();
    assert_eq!(st["live_credentials"], 0);
    assert!(st["last_used_at"].is_null());
    assert!(
        st["viewer"].is_null(),
        "a status read creates no owner: {st}"
    );
}

#[tokio::test]
async fn minting_refuses_unknown_fields() {
    let ts = TestServer::spawn().await;
    let res = ts
        .post_json(
            "/api/extension/credentials",
            json!({"extension_id": ts.extension_id(), "viewer": "x"}),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn the_owners_agents_cannot_mint_an_extension_credential() {
    let ts = TestServer::spawn().await;
    let url = format!("{}/api/extension/credentials", ts.base);
    let body = json!({"extension_id": ts.extension_id()});
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for (k, v) in [
        ("x-clax-channel", "agent"),
        ("x-clax-session", sid.as_str()),
    ] {
        let res = ts
            .authed(ts.client.post(&url))
            .header(k, v)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{k}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "not_for_agents");
        assert!(v.get("credential").is_none());
    }
    let status: Value = ts.get_authed("/api/extension").await.json().await.unwrap();
    assert_eq!(status["live_credentials"], 0, "{status}");
    // The CLI by hand (the native host, run by Chrome) still pairs, once the
    // owner confirms in their browser while an agent is live.
    let path = url.trim_start_matches(&ts.base).to_string();
    let res = ts.confirmed(reqwest::Method::POST, &path, body).await;
    assert_eq!(res.status(), 200);
}

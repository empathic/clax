//! `GET /api/resolve` (`clax_server::resolve`): what a URL names, a local
//! artifact, a connected remote's artifact, a Clax instance this home is
//! not connected to, or a page, against hosted test daemons as remotes.

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use clax_testkit::mcp::{ADAPTER, mcp_post, rpc_message};
use clax_testkit::remotes::{NotYetClax, PAGE, Public, closed_port, connect, enc, json_of, post};
use serde_json::{Value, json};
use std::sync::Arc;

/// The owner's resolution of `url` (the daemon token, by hand).
async fn resolve(ts: &TestServer, url: &str) -> Value {
    let (s, v) = json_of(
        ts.get_authed(&format!("/api/resolve?url={}", enc(url)))
            .await,
    )
    .await;
    assert_eq!(s, 200, "{url}: {v}");
    v
}

#[tokio::test]
async fn resolve_tells_local_remote_unconnected_and_page_apart() {
    let public = Public::with_site_domain("sites.test").await;
    let other = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let rid = remote["id"].as_str().unwrap();
    let raid = public.publish().await;
    let aid = local.publish("Mine", &[("index.html", PAGE)]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let pushed = v["link"]["url"].as_str().unwrap().to_string();
    let pushed_id = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let port = public.ts.addr.port();
    let o = &public.origin;
    let theirs = |version: Option<u32>| {
        let mut v = json!({"kind": "remote", "remote": rid, "origin": o,
            "name": remote["name"], "artifact_id": raid, "url": format!("{o}/a/{raid}")});
        if let Some(n) = version {
            v["version"] = json!(n);
        }
        v
    };
    let mine = json!({"kind": "local", "artifact_id": aid});
    let free = format!("http://127.0.0.1:{}/x", closed_port());
    let cases: Vec<(String, Value)> = vec![
        (format!("{}/a/{aid}", local.base), mine.clone()),
        (
            format!("{}/a/{aid}/v/1", local.base),
            json!({"kind": "local", "artifact_id": aid, "version": 1}),
        ),
        (format!("{o}/a/{raid}"), theirs(None)),
        (format!("{o}/a/{raid}/get"), theirs(None)),
        (format!("{o}/a/{raid}/"), theirs(None)),
        (format!("{o}/a/{raid}?v=2"), theirs(None)),
        (format!("{o}/a/{raid}#t"), theirs(None)),
        (format!("{o}/a/{raid}/v/2"), theirs(Some(2))),
        (format!("{o}/c/{raid}/v/1/index.html"), theirs(Some(1))),
        (format!("http://{raid}.sites.test:{port}/"), theirs(None)),
        (
            format!("http://{raid}.sites.test:{port}/_clax/v/3/x.html"),
            theirs(Some(3)),
        ),
        (
            format!("{}/a/{raid}", other.origin),
            json!({"kind": "sign_in_required", "origin": other.origin,
                   "name": format!("localhost:{}", other.ts.addr.port()),
                   "request": {"tool": "connect_instance",
                               "command": format!("clax remote request '{}/a/{raid}'", other.origin)}}),
        ),
        (free.clone(), json!({"kind": "page", "url": free})),
        (pushed.clone(), mine.clone()),
        (format!("{o}/a/{pushed_id}/get?v=1#t"), mine.clone()),
        (
            format!("http://{pushed_id}.sites.test:{port}/"),
            mine.clone(),
        ),
        (
            format!("{o}/settings"),
            json!({"kind": "page", "url": format!("{o}/settings")}),
        ),
    ];
    for (url, want) in cases {
        assert_eq!(resolve(&local, &url).await, want, "{url}");
    }

    // A URL on this daemon that names no artifact, or not http(s), is
    // refused.
    for bad in [format!("{}/settings", local.base), "ftp://x/a/b".into()] {
        let (s, v) = json_of(
            local
                .get_authed(&format!("/api/resolve?url={}", enc(&bad)))
                .await,
        )
        .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (400, Some("invalid_url")),
            "{bad}: {v}"
        );
    }
    let (s, _) = json_of(local.get_authed("/api/resolve").await).await;
    assert_eq!(s, 400);

    // The MCP tools read through it: an unconnected Clax's URL names the
    // instance to connect, a connected remote's is read there.
    let call = |url: String| {
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "read", "arguments": {"url_or_id": url}}})
    };
    let tool_error = |v: Value| -> Value {
        let text = v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        serde_json::from_str::<Value>(&text).unwrap()["error"].clone()
    };
    let res = mcp_post(&local, ADAPTER, &call(format!("{}/a/{raid}", other.origin)))
        .bearer_auth(&local.token)
        .header("mcp-protocol-version", "2025-06-18")
        .send()
        .await
        .unwrap();
    let e = tool_error(rpc_message(res).await);
    assert_eq!(e["code"], "remote_sign_in_required", "{e}");
    assert_eq!(e["origin"], json!(other.origin), "{e}");
    assert!(
        e["next"].as_str().unwrap().contains("connect_instance"),
        "{e}"
    );
    assert_eq!(
        e["request"],
        json!({"tool": "connect_instance",
               "command": format!("clax remote request '{}/a/{raid}'", other.origin)}),
        "{e}"
    );
    let res = mcp_post(&local, ADAPTER, &call(format!("{o}/a/{raid}")))
        .bearer_auth(&local.token)
        .header("mcp-protocol-version", "2025-06-18")
        .send()
        .await
        .unwrap();
    let v = rpc_message(res).await;
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    let read: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        (read["artifact_id"].as_str(), read["content"].as_str()),
        (Some(raid.as_str()), Some(PAGE)),
        "{v}"
    );
    let res = mcp_post(&local, ADAPTER, &call(pushed))
        .bearer_auth(&local.token)
        .header("mcp-protocol-version", "2025-06-18")
        .send()
        .await
        .unwrap();
    let v = rpc_message(res).await;
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    let read: Value = serde_json::from_str(text).unwrap();
    assert_eq!(read["artifact_id"], json!(aid), "{v}");
}

#[tokio::test]
async fn a_connected_persons_agent_resolves_remote_urls_as_pages() {
    let public = Public::new().await;
    let unconnected = NotYetClax::start().await;
    unconnected.become_clax();
    let local = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    connect(&local, &public).await;
    let raid = public.publish().await;
    let aid = local.publish("Mine", &[("index.html", PAGE)]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = post(&local, &format!("/api/artifacts/{aid}/push"), json!({})).await;
    assert_eq!(s, 200, "{v}");
    let pushed = v["link"]["url"].as_str().unwrap().to_string();
    let sam = local.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let theirs = format!("{}/a/{raid}", public.origin);
    let late = format!("{}/a/{raid}", unconnected.origin);
    let sams = |url: String| {
        let (sam, sid) = (&sam, &sid);
        async move {
            let r = sam
                .get(
                    Some(sid.as_str()),
                    &format!("/api/resolve?url={}", enc(&url)),
                )
                .await;
            json_of(r).await
        }
    };
    for url in [theirs.clone(), late.clone(), pushed.clone()] {
        assert_eq!(
            sams(url.clone()).await,
            (200, json!({"kind": "page", "url": url})),
        );
    }
    assert_eq!(
        unconnected.asked(),
        0,
        "a person's agent makes this daemon ask no other origin"
    );
    let own = format!("{}/a/{aid}", local.base);
    assert_eq!(
        sams(own).await,
        (200, json!({"kind": "local", "artifact_id": aid}))
    );

    // The owner's agent, a session of the daemon token, gets them all.
    let owner_sid = local.register_session("claude", "owner-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let owners = |url: String| {
        let local = &local;
        let owner_sid = &owner_sid;
        async move {
            let r = local
                .authed(
                    local
                        .client
                        .get(format!("{}/api/resolve?url={}", local.base, enc(&url))),
                )
                .header("x-clax-session", owner_sid)
                .send()
                .await
                .unwrap();
            json_of(r).await.1
        }
    };
    assert_eq!(owners(theirs).await["kind"], "remote");
    assert_eq!(owners(late).await["kind"], "sign_in_required");
    assert_eq!(owners(pushed).await["artifact_id"], json!(aid));

    // No credential: refused.
    let r = local
        .client
        .get(format!(
            "{}/api/resolve?url={}",
            local.base,
            enc(&format!("{}/a/{aid}", local.base))
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn a_negative_discovery_is_kept_ten_minutes_and_an_unconnected_positive_an_hour() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let local = TestServer::spawn_with_clock(clock.clone()).await;
    let site = NotYetClax::start().await;
    let url = format!("{}/a/7q3k9mzx2b4t", site.origin);
    let page = json!({"kind": "page", "url": url});
    assert_eq!(resolve(&local, &url).await, page);
    assert_eq!(site.asked(), 1);
    site.become_clax();
    assert_eq!(resolve(&local, &url).await, page, "the answer is kept");
    clock.advance(599);
    assert_eq!(resolve(&local, &url).await, page, "for ten minutes");
    assert_eq!(site.asked(), 1);
    clock.advance(1);
    let clax = json!({"kind": "sign_in_required", "origin": site.origin, "name": "Late",
        "request": {"tool": "connect_instance", "command": format!("clax remote request '{url}'")}});
    assert_eq!(resolve(&local, &url).await, clax, "then asked again");
    assert_eq!(site.asked(), 2);
    // An instance this home is not connected to is asked again after an
    // hour: nothing calls it, so no failed call would forget it.
    clock.advance(3599);
    assert_eq!(resolve(&local, &url).await, clax);
    assert_eq!(site.asked(), 2, "kept for an hour");
    clock.advance(1);
    assert_eq!(resolve(&local, &url).await, clax);
    assert_eq!(site.asked(), 3, "then asked again");
}

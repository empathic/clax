//! The grant flow (spec §4.2, §6.2–§6.6, M20): invites, ad-hoc requests the
//! owner approves with the user code, continuation, revocation, the
//! browser link, the `people` topic, and the signatures that guard every
//! call of the flow.

use crate::common::TestServer;
use clax_core::RegisterSession;
use clax_core::rights::Rights;
use clax_core::sig::KeyPair;
use clax_core::working::ManualClock;
use clax_server::testing::{Call, LAN_LOCAL, TestClient};
use serde_json::{Value, json};
use std::sync::Arc;

const T0: &str = "2026-10-01T09:00:00Z";

async fn lan_server() -> TestServer {
    TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await
}

async fn lan_server_at(clock: &Arc<ManualClock>) -> TestServer {
    TestServer::spawn_on_with_clock("0.0.0.0".parse().unwrap(), clock.clone()).await
}

async fn code(res: reqwest::Response) -> String {
    let v: Value = res.json().await.unwrap();
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

async fn status_code(res: reqwest::Response) -> (u16, String) {
    (res.status().as_u16(), code(res).await)
}

/// The invite code in a `clax connect` command.
fn code_of(cmd: &str) -> &str {
    cmd.rsplit_once("/i/").expect("an invite command").1
}

/// A request waiting for the owner: its client, grant ID and user code.
async fn pending(ts: &TestServer, access: &str) -> (TestClient, String, String) {
    let c = TestClient::new(ts);
    let r = c
        .grant_request(json!({"access_token": {"access": [access]}}))
        .await;
    let user_code = r["interact"]["user_code"].as_str().unwrap().to_string();
    let gid = c.grant_id();
    (c, gid, user_code)
}

fn same_rights(a: &Value, b: &Rights) -> bool {
    let a = Rights::parse(a).unwrap();
    a.is_narrower_or_equal(b) && b.is_narrower_or_equal(&a)
}

#[tokio::test]
async fn invite_flow_issues_a_key_bound_token_and_a_browser_link() {
    let ts = lan_server().await;
    let cmd = ts.invite_command("Sam").await; // through the owner's browser credential
    assert!(
        cmd.starts_with(&format!(
            "clax connect http://{LAN_LOCAL}:{}/i/",
            ts.addr.port()
        )),
        "{cmd}"
    );
    let c = TestClient::new(&ts);
    let r = c
        .grant_request(json!({"access_token": {"access": ["clax:collaborator"]}, "clax": {"invite": code_of(&cmd)}}))
        .await;
    assert!(
        r["access_token"]["value"]
            .as_str()
            .unwrap()
            .starts_with("cxg_")
    );
    assert!(
        r["access_token"].get("expires_in").is_none() && r["access_token"].get("flags").is_none()
    );
    assert!(
        r["access_token"]["manage"]["access_token"]["value"]
            .as_str()
            .unwrap()
            .starts_with("cxm_")
    );
    assert!(same_rights(
        &r["access_token"]["access"],
        &Rights::preset("clax:collaborator").unwrap()
    ));
    assert_eq!(r["clax"]["person"]["display_name"], "Sam");
    let person = ts.store.viewer_of_grant(&c.grant_id()).unwrap();
    assert_eq!(r["subject"]["sub_ids"][0]["id"], person.public_id.as_str());
    assert_eq!(r["subject"]["sub_ids"][0]["format"], "opaque");
    // A LAN viewer's browser opens the link.
    let visitor = ts.viewer(Some("Visitor")).await;
    let link = r["clax"]["browser_link"].as_str().unwrap();
    let (lan, _) = ts.lan();
    let planted = lan
        .get(link)
        .header("cookie", format!("clax_viewer={}", visitor.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(planted.status(), 200);
    let h = planted.headers();
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(h["referrer-policy"], "no-referrer");
    let cookies: Vec<&str> = h
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(cookies.len(), 1);
    assert!(
        cookies[0].starts_with(&format!("clax_viewer={}; Path=/;", person.id))
            && cookies[0].contains("HttpOnly")
            && cookies[0].contains("SameSite=Lax"),
        "{cookies:?}"
    );
    let page = planted.text().await.unwrap();
    assert!(page.contains("/people") && page.contains("no-referrer"));
    assert!(
        ts.viewer_exists(&visitor.cookie),
        "the carried viewer is replaced, never folded"
    );
    assert!(
        !ts.store
            .get_viewer(&visitor.cookie)
            .unwrap()
            .unwrap()
            .person
    );
    assert_eq!(lan.get(link).send().await.unwrap().status(), 404, "one use");
}

#[tokio::test]
async fn the_browser_link_is_refused_on_the_daemons_machine_and_for_the_owner() {
    let ts = lan_server().await;
    let c = TestClient::new(&ts);
    let cmd = ts.invite_command("Sam").await;
    let r = c
        .grant_request(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    let link = r["clax"]["browser_link"].as_str().unwrap();
    let here = ts.client.get(link).send().await.unwrap();
    assert_eq!(here.status(), 403);
    assert_eq!(here.headers()["cache-control"], "no-store");
    let (lan, _) = ts.lan();
    assert_eq!(
        lan.get(link).send().await.unwrap().status(),
        200,
        "a refusal does not spend the code"
    );
    // The owner's own machine joins no browser.
    let mine = TestClient::new(&ts);
    let cmd = ts.invite_with(json!({"me": true})).await;
    let r = mine
        .grant_request(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    assert!(r["clax"].get("browser_link").is_none(), "{r}");
    assert_eq!(
        r["clax"]["person"]["public_id"],
        ts.owner_public_id().await.as_str()
    );
    assert!(same_rights(&r["access_token"]["access"], &Rights::full()));
    // Unknown and malformed codes.
    let base = link.rsplit_once("/connect/b/").unwrap().0;
    for bad in ["x", &"A".repeat(43)] {
        let res = lan
            .get(format!("{base}/connect/b/{bad}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "{bad}");
        assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    }
}

#[tokio::test]
async fn invites_need_the_lan_and_a_browser() {
    let ts = TestServer::spawn().await;
    let res = ts
        .owner_browser()
        .post("/api/people/invites", json!({"name": "Sam"}))
        .await;
    assert_eq!(status_code(res).await, (409, "not_on_lan".into()));
    assert!(
        ts.store.list_invites().unwrap().is_empty(),
        "no invite is made"
    );
    let lan = lan_server().await;
    let res = lan
        .post_json("/api/people/invites", json!({"name": "Sam"}))
        .await; // token alone
    assert_eq!(status_code(res).await, (403, "browser_required".into()));
    let visitor = lan.viewer(None).await;
    let res = lan
        .client
        .post(format!("{}/api/people/invites", lan.base))
        .header("cookie", format!("clax_viewer={}", visitor.cookie))
        .json(&json!({"name": "Sam"}))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(res).await, (403, "browser_required".into()));
    let res = lan
        .owner_browser()
        .post("/api/people/invites", json!({"name": "Sam", "me": true}))
        .await;
    assert_eq!(status_code(res).await, (400, "invalid_args".into()));
}

#[tokio::test]
async fn invites_are_listed_without_codes_and_revoked_in_a_browser() {
    let ts = lan_server().await;
    let res = ts
        .owner_browser()
        .post(
            "/api/people/invites",
            json!({"name": "Sam", "rights": "clax:reader"}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let id = v["invite"]["id"].as_str().unwrap().to_string();
    let cmd = v["invite"]["command"].as_str().unwrap().to_string();
    let invite = code_of(&cmd).to_string();
    let listed: Value = ts
        .get_authed("/api/people/invites")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(listed["invites"][0]["id"], id.as_str());
    assert_eq!(listed["invites"][0]["person"]["display_name"], "Sam");
    let text = listed.to_string();
    assert!(!text.contains(&invite) && !text.contains(&invite.replace('-', "")));
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/people/invites/{id}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(res).await, (403, "browser_required".into()));
    let res = ts
        .owner_browser()
        .delete(&format!("/api/people/invites/{id}"))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert!(v["invite"]["revoked_at"].is_string());
    let res = TestClient::new(&ts)
        .grant_request_raw(json!({"clax": {"invite": invite}}))
        .await;
    assert_eq!(status_code(res).await, (400, "invalid_invite".into()));
}

#[tokio::test]
async fn adhoc_request_waits_for_the_owner_with_the_code() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let mut people = ts.stream_as_owner(&["people"]).await;
    let c = TestClient::new(&ts);
    let res = c
        .grant_request_raw(json!({"access_token": {"access": ["clax:reader"]}}))
        .await;
    assert_eq!(res.status(), 201);
    let r: Value = res.json().await.unwrap();
    let user_code = r["interact"]["user_code"].as_str().unwrap().to_string();
    let b = user_code.as_bytes();
    assert!(
        b.len() == 9
            && b[..4]
                .iter()
                .all(|x| x.is_ascii_uppercase() && *x != b'I' && *x != b'O')
            && b[4] == b'-'
            && b[5..].iter().all(u8::is_ascii_digit),
        "{user_code}"
    );
    assert_eq!(r["continue"]["wait"], 5);
    let cont = r["continue"]["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(cont.starts_with("cxc_"));
    let gid = r["continue"]["uri"]
        .as_str()
        .unwrap()
        .strip_prefix("/api/grants/")
        .and_then(|u| u.strip_suffix("/continue"))
        .unwrap()
        .to_string();
    assert!(r.get("access_token").is_none());
    // The people topic announces it, without the code or the token.
    let ev = people.next_named("people").await;
    assert_eq!(ev["change"]["what"], "grant_requested");
    assert_eq!(ev["change"]["grant"]["id"], gid.as_str());
    assert_eq!(ev["change"]["grant"]["status"], "pending");
    assert_eq!(
        ev["change"]["grant"]["client"]["key"],
        clax_core::sig::fingerprint(&c.key.public()).as_str()
    );
    let text = ev.to_string();
    assert!(
        !text.contains(&user_code) && !text.contains(&cont),
        "{text}"
    );
    let listed: Value = ts
        .get_authed("/api/grants?status=pending")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(listed["grants"][0]["id"], gid.as_str());
    assert!(!listed.to_string().contains(&user_code));

    c.keep(&r);
    // Sooner than the wait after the request: too fast.
    assert_eq!(c.poll().await.1["error"]["code"], "too_fast");
    let owner = ts.owner_browser();
    let approve = format!("/api/grants/{gid}/approve");
    let wrong = if user_code.starts_with('Z') {
        "AAAA-0000"
    } else {
        "ZZZZ-0000"
    };
    let res = owner
        .post(&approve, json!({"user_code": wrong, "name": "Sam"}))
        .await;
    assert_eq!(status_code(res).await, (400, "wrong_code".into()));
    let res = owner
        .post(
            &approve,
            json!({"user_code": user_code.to_lowercase(), "name": "Sam"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["grant"]["status"], "approved");
    assert_eq!(v["grant"]["person"]["display_name"], "Sam");
    let ev = people.next_named("people").await;
    assert_eq!(ev["change"]["what"], "grant_approved");
    clock.advance(5);
    let (status, v) = c.poll().await;
    assert_eq!(status, 200, "{v}");
    assert!(
        v["access_token"]["value"]
            .as_str()
            .unwrap()
            .starts_with("cxg_")
    );
    assert!(same_rights(
        &v["access_token"]["access"],
        &Rights::preset("clax:reader").unwrap()
    ));
    assert!(
        v["clax"]["browser_link"]
            .as_str()
            .unwrap()
            .contains("/connect/b/")
    );
    assert_eq!(v["clax"]["person"]["display_name"], "Sam");
    let fresh = v["continue"]["access_token"]["value"].as_str().unwrap();
    assert_ne!(fresh, cont, "a new continuation token");
    let ev = people.next_named("people").await;
    assert_eq!(ev["change"]["what"], "grant_connected");
    // Decided already; issued once.
    let res = owner
        .post(&approve, json!({"user_code": user_code, "name": "Mia"}))
        .await;
    assert_eq!(status_code(res).await, (409, "not_pending".into()));
    clock.advance(5);
    assert_eq!(
        c.poll().await,
        (
            409,
            json!({"error": {"code": "already_issued", "message": "this grant's tokens were already issued"}})
        )
    );
}

#[tokio::test]
async fn a_request_only_makes_a_new_person() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sam_pid = ts.store.viewer_of_grant(&sam.grant_id()).unwrap().public_id;
    let (_c, gid, user_code) = pending(&ts, "clax:collaborator").await;
    let owner = ts.owner_browser();
    let approve = format!("/api/grants/{gid}/approve");
    for body in [
        json!({"user_code": user_code, "name": "X", "viewer": sam_pid}),
        json!({"user_code": user_code, "me": true}),
        json!({"user_code": user_code, "viewer": sam_pid}),
    ] {
        let res = owner.post(&approve, body.clone()).await;
        assert_eq!(
            status_code(res).await,
            (400, "invalid_args".into()),
            "{body}"
        );
    }
    let res = owner
        .post(&approve, json!({"user_code": user_code, "name": "Sam"}))
        .await;
    assert_eq!(status_code(res).await, (409, "name_taken".into()));
    let res = owner
        .post(&approve, json!({"user_code": user_code, "name": "Mia"}))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_ne!(v["grant"]["person"]["public_id"], sam_pid.as_str());
    assert_eq!(ts.store.grant(&gid).unwrap().wrong_codes, 0);
}

#[tokio::test]
async fn polling_faster_than_wait_is_too_fast_and_expiry_denies() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let mut people = ts.stream_as_owner(&["people"]).await;
    let (c, gid, _) = pending(&ts, "clax:reader").await;
    let cont = c.held().continue_token.unwrap();
    assert_eq!(c.poll().await.0, 400);
    clock.advance(5);
    let (status, v) = c.poll().await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["continue"]["wait"], 5);
    assert_eq!(v["continue"]["access_token"]["value"], cont.as_str());
    assert!(v.get("access_token").is_none());
    assert_eq!(c.poll().await.1["error"]["code"], "too_fast");
    clock.advance(4);
    assert_eq!(c.poll().await.1["error"]["code"], "too_fast");
    clock.advance(600);
    assert_eq!(c.poll().await.1["error"]["code"], "denied");
    // The sweep reports the expiry on the people topic.
    let res = ts.post_json("/api/_test/grants/sweep", json!({})).await;
    assert_eq!(res.status(), 200);
    loop {
        let ev = people.next_named("people").await;
        if ev["change"]["what"] == "grant_ended" {
            assert_eq!(ev["change"]["grant_id"], gid.as_str());
            assert_eq!(ev["change"]["by"], "expiry");
            assert_eq!(ev["change"]["grant"]["status"], "expired");
            break;
        }
    }
    // The third wrong code denies a request.
    let (d, gid, user_code) = pending(&ts, "clax:reader").await;
    let wrong = if user_code.starts_with('Z') {
        "AAAA-0000"
    } else {
        "ZZZZ-0000"
    };
    let owner = ts.owner_browser();
    for _ in 0..3 {
        let res = owner
            .post(
                &format!("/api/grants/{gid}/approve"),
                json!({"user_code": wrong, "name": "Sam"}),
            )
            .await;
        assert_eq!(status_code(res).await, (400, "wrong_code".into()));
    }
    let res = owner
        .post(
            &format!("/api/grants/{gid}/approve"),
            json!({"user_code": user_code, "name": "Sam"}),
        )
        .await;
    assert_eq!(status_code(res).await, (409, "not_pending".into()));
    assert_eq!(d.poll().await.1["error"]["code"], "denied");
}

#[tokio::test]
async fn revocation_through_manage_and_continue() {
    let ts = lan_server().await;
    let mut people = ts.stream_as_owner(&["people"]).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let gid = sam.grant_id();
    let sid = ts
        .store
        .register_session(
            &clax_core::audit::AuditCtx::DAEMON,
            RegisterSession {
                harness: "claude".into(),
                harness_session_id: Some("h1".into()),
                cwd: "/w".into(),
                grant_id: Some(gid.clone()),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let held = sam.held();
    let res = sam
        .gnap(
            "POST",
            &format!("/api/grants/{gid}/token"),
            held.manage.as_deref().unwrap(),
        )
        .await;
    assert_eq!(res.status(), 405, "there is no rotation");
    assert_eq!(sam.revoke_token().await.status(), 204);
    let ev = loop {
        let ev = people.next_named("people").await;
        if ev["change"]["what"] == "grant_ended" {
            break ev;
        }
    };
    assert_eq!(ev["change"]["grant_id"], gid.as_str());
    assert_eq!(ev["change"]["by"], "person");
    assert_eq!(ev["change"]["sessions_ended"], json!([sid]));
    assert!(
        ts.store
            .get_session(&sid)
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
    );
    assert!(
        !ts.store
            .token_live(held.access.as_deref().unwrap())
            .unwrap()
    );
    assert_eq!(
        (
            sam.poll().await.0,
            sam.poll().await.1["error"]["code"].clone()
        ),
        (401, json!("grant_revoked"))
    );
    assert_eq!(
        status_code(sam.revoke_token().await).await,
        (401, "grant_revoked".into())
    );
    assert_eq!(
        status_code(sam.disconnect().await).await,
        (401, "grant_revoked".into())
    );
    // The key is free to connect again.
    let again = TestClient::with_key(&ts, KeyPair::from_pkcs8(sam.key.pkcs8().expose()).unwrap());
    let cmd = ts.invite_command("Samuel").await;
    again
        .grant_request(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    // Disconnecting through the continuation does the same.
    let mia = ts.connect_person("Mia", "clax:reader").await;
    assert_eq!(mia.disconnect().await.status(), 204);
    assert_eq!(mia.poll().await.1["error"]["code"], "grant_revoked");
    assert_eq!(
        status_code(mia.revoke_token().await).await,
        (401, "grant_revoked".into())
    );
    assert!(
        !ts.store
            .token_live(mia.held().access.as_deref().unwrap())
            .unwrap()
    );
}

#[tokio::test]
async fn the_owner_denies_and_revokes_in_a_browser() {
    let ts = lan_server().await;
    let (c, gid, user_code) = pending(&ts, "clax:reader").await;
    let approve = format!("/api/grants/{gid}/approve");
    let res = ts
        .post_json(&approve, json!({"user_code": user_code, "name": "Sam"}))
        .await;
    assert_eq!(status_code(res).await, (403, "browser_required".into()));
    let res = ts
        .post_json(&format!("/api/grants/{gid}/deny"), json!({}))
        .await;
    assert_eq!(status_code(res).await, (403, "browser_required".into()));
    let res = ts
        .client
        .post(format!("{}{approve}", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", "http://evil.example")
        .json(&json!({"user_code": user_code, "name": "Sam"}))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(res).await, (403, "forbidden_origin".into()));
    let visitor = ts.viewer(None).await;
    let res = ts
        .client
        .get(format!("{}/api/grants", ts.base))
        .header("cookie", format!("clax_viewer={}", visitor.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(res).await, (403, "forbidden".into()));
    let owner = ts.owner_browser();
    let res = owner
        .post(&format!("/api/grants/{gid}/deny"), json!({}))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["grant"]["status"], "denied");
    assert_eq!(c.poll().await.1["error"]["code"], "denied");
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/grants/{}", ts.base, sam.grant_id())),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(status_code(res).await, (403, "browser_required".into()));
    let res = owner
        .delete(&format!("/api/grants/{}", sam.grant_id()))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (
            v["grant"]["status"].as_str(),
            v["grant"]["ended_by"].as_str()
        ),
        (Some("revoked"), Some("owner"))
    );
    assert_eq!(sam.poll().await.1["error"]["code"], "grant_revoked");
}

#[tokio::test]
async fn grant_routes_are_signed() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let c = TestClient::new(&ts);
    let body = serde_json::to_vec(&c.grant_body(json!({}))).unwrap();
    let post = |extra: Call<'static>| Call {
        method: "POST",
        path: "/api/grants",
        body: Some(body.clone()),
        ..extra
    };
    let res = c
        .call(post(Call {
            unsigned: true,
            ..Default::default()
        }))
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    // Signed by key A presenting key B.
    let other = Box::leak(Box::new(KeyPair::generate()));
    let res = c
        .call(post(Call {
            key: Some(other),
            ..Default::default()
        }))
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    for skew in [121, -121] {
        let res = c
            .call(post(Call {
                skew,
                ..Default::default()
            }))
            .await;
        assert_eq!(
            res.headers()["date"],
            "Thu, 01 Oct 2026 09:00:00 GMT",
            "the daemon's time"
        );
        assert_eq!(status_code(res).await, (401, "stale_request".into()));
    }
    for (k, v) in [
        ("origin", "http://evil.example"),
        ("cookie", "clax_viewer=x"),
    ] {
        let res = c
            .call(post(Call {
                headers: vec![(k, v.to_string())],
                ..Default::default()
            }))
            .await;
        assert_eq!(
            status_code(res).await,
            (403, "forbidden_origin".into()),
            "{k}"
        );
    }
    let res = c
        .call(Call {
            method: "POST",
            path: "/api/grants",
            body: Some(vec![b' '; 17 * 1024]),
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(res).await, (413, "body_too_large".into()));
    // A bad signature spends no nonce; a replay is refused.
    let n = clax_core::sig::new_nonce();
    let res = c
        .call(post(Call {
            corrupt: true,
            nonce: Some(n.clone()),
            ..Default::default()
        }))
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    let res = c
        .call(post(Call {
            nonce: Some(n.clone()),
            ..Default::default()
        }))
        .await;
    assert_eq!(res.status(), 201);
    let r: Value = res.json().await.unwrap();
    c.keep(&r);
    let res = c
        .call(post(Call {
            nonce: Some(n.clone()),
            ..Default::default()
        }))
        .await;
    assert_eq!(status_code(res).await, (401, "replayed_request".into()));
    // Continuation: the token names the grant and its key.
    let gid = c.grant_id();
    let cont = c.held().continue_token.unwrap();
    let path = format!("/api/grants/{gid}/continue");
    let path: &'static str = Box::leak(path.into_boxed_str());
    clock.advance(5);
    let res = c.gnap("POST", path, "cxc_nope").await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
    let res = c.gnap("POST", "/api/grants/g_other/continue", &cont).await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
    let res = c
        .call(Call {
            method: "POST",
            path,
            authorization: Some(format!("Bearer {cont}")),
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
    for bad in [
        Call {
            unsigned: true,
            ..Default::default()
        },
        Call {
            key: Some(other),
            ..Default::default()
        },
        Call {
            signed_token: Some("cxc_other".into()),
            ..Default::default()
        },
        Call {
            corrupt: true,
            ..Default::default()
        },
    ] {
        let res = c
            .call(Call {
                method: "POST",
                path,
                authorization: Some(format!("GNAP {cont}")),
                ..bad
            })
            .await;
        assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    }
    let res = c
        .call(Call {
            method: "POST",
            path,
            authorization: Some(format!("GNAP {cont}")),
            skew: 200,
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(res).await, (401, "stale_request".into()));
    let n = clax_core::sig::new_nonce();
    let signed = |nonce: &str| Call {
        method: "POST",
        path,
        authorization: Some(format!("GNAP {cont}")),
        nonce: Some(nonce.to_string()),
        ..Default::default()
    };
    assert_eq!(c.call(signed(&n)).await.status(), 200);
    clock.advance(5);
    let res = c.call(signed(&n)).await;
    assert_eq!(status_code(res).await, (401, "replayed_request".into()));
}

#[tokio::test]
async fn grant_requests_are_checked() {
    let ts = lan_server().await;
    let c = TestClient::new(&ts);
    let cases = [
        (
            json!({"access_token": {"access": ["clax:reader"], "flags": ["bearer"]}}),
            "invalid_args",
        ),
        (
            json!({"access_token": {"access": ["clax:admin"]}}),
            "invalid_rights",
        ),
        (json!({"interact": {"start": ["redirect"]}}), "invalid_args"),
    ];
    for (body, want) in cases {
        let res = c.grant_request_raw(body.clone()).await;
        assert_eq!(status_code(res).await, (400, want.into()), "{body}");
    }
    let mut body = c.grant_body(json!({}));
    body["client"]["key"]["proof"] = json!("httpsig");
    let res = c.grant_request_raw(body).await;
    assert_eq!(status_code(res).await, (400, "invalid_args".into()));
    // A key holds one live grant.
    c.grant_request(json!({})).await;
    let res = c.grant_request_raw(json!({})).await;
    assert_eq!(status_code(res).await, (409, "key_in_use".into()));
}

#[tokio::test]
async fn the_people_topic_is_the_owners() {
    let ts = lan_server().await;
    let visitor = ts.viewer(None).await;
    assert_eq!(visitor.subscribe_status(&["people"]).await, 403);
}

/// The token index's size and state, and the request cache's keys.
async fn index(ts: &TestServer) -> Value {
    ts.get_authed("/api/_test/grants/index")
        .await
        .json()
        .await
        .unwrap()
}

fn peer(i: u32) -> std::net::IpAddr {
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(
        10,
        1,
        (i / 250) as u8,
        (i % 250) as u8 + 1,
    ))
}

#[tokio::test]
async fn the_caps_hold_against_cancelling_and_asking_again() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let ask = |i: u32| TestClient::new(&ts).from_peer(peer(i));
    for _ in 0..3 {
        let c = ask(0);
        c.grant_request(json!({})).await;
        assert_eq!(c.disconnect().await.status(), 204);
        assert_eq!(c.poll().await.1["error"]["code"], "denied", "cancelled");
    }
    let res = ask(0).grant_request_raw(json!({})).await;
    assert_eq!(status_code(res).await, (429, "limit_reached".into()));
    for i in 1..18 {
        let c = ask(i);
        c.grant_request(json!({})).await;
        assert_eq!(c.disconnect().await.status(), 204);
    }
    let res = ask(99).grant_request_raw(json!({})).await;
    assert_eq!(
        status_code(res).await,
        (429, "limit_reached".into()),
        "twenty in all"
    );
    clock.advance(10 * 60 + 1);
    ask(0).grant_request(json!({})).await;
}

#[tokio::test]
async fn the_index_holds_issued_grants_only_and_ended_requests_are_purged() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let owner = ts.owner_browser();
    for round in 0..3u32 {
        for i in 0..10u32 {
            let c = TestClient::new(&ts).from_peer(peer(round * 10 + i));
            c.grant_request(json!({})).await;
            if i % 2 == 0 {
                assert_eq!(c.disconnect().await.status(), 204);
            } else {
                let res = owner
                    .post(&format!("/api/grants/{}/deny", c.grant_id()), json!({}))
                    .await;
                assert_eq!(res.status(), 200);
            }
            assert_eq!(index(&ts).await["tokens"], 0, "round {round}, request {i}");
        }
        clock.advance(10 * 60 + 1);
    }
    assert_eq!(ts.store.list_grants().unwrap().len(), 30);
    let sam = ts.connect_person("Sam", "clax:reader").await;
    assert_eq!(index(&ts).await["tokens"], 3);
    clock.advance(24 * 3600);
    let res = ts.post_json("/api/_test/grants/sweep", json!({})).await;
    assert_eq!(res.status(), 200);
    let left: Vec<String> = ts
        .store
        .list_grants()
        .unwrap()
        .into_iter()
        .map(|g| g.id)
        .collect();
    assert_eq!(left, vec![sam.grant_id()], "issued grants stay as history");
    assert_eq!(index(&ts).await["tokens"], 3);
}

#[tokio::test]
async fn a_flood_of_throwaway_keys_never_locks_out_an_invite() {
    let ts = lan_server().await;
    let flood = |from: u32| {
        let ts = &ts;
        async move {
            for _ in 0..256 {
                let res = TestClient::new(ts)
                    .from_peer(peer(from))
                    .grant_request_raw(json!({"clax": {"invite": "AAAA-AAAA-AAAA"}}))
                    .await;
                assert_eq!(status_code(res).await, (400, "invalid_invite".into()));
            }
        }
    };
    flood(1).await;
    let cmd = ts.invite_command("Sam").await;
    TestClient::new(&ts)
        .from_peer(peer(2))
        .grant_request(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    // A flood from the very address the real machine uses.
    flood(3).await;
    let cmd = ts.invite_command("Mia").await;
    TestClient::new(&ts)
        .from_peer(peer(3))
        .grant_request(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    let v = index(&ts).await;
    assert!(v["request_keys"].as_u64().unwrap() <= 256, "{v}");
}

#[tokio::test]
async fn signatures_bind_host_body_query_method_and_route() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let c = TestClient::new(&ts);
    let body = serde_json::to_vec(&c.grant_body(json!({}))).unwrap();
    let mut longer = body.clone();
    longer.push(b' ');
    let host = format!("localhost:{}", ts.addr.port());
    for tamper in [
        Call {
            sent_host: Some(host.clone()),
            ..Default::default()
        },
        Call {
            sent_body: Some(longer),
            ..Default::default()
        },
        Call {
            sent_path: Some("/api/grants?x=1"),
            ..Default::default()
        },
    ] {
        let res = c
            .call(Call {
                method: "POST",
                path: "/api/grants",
                body: Some(body.clone()),
                ..tamper
            })
            .await;
        assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    }
    assert!(ts.store.list_grants().unwrap().is_empty());
    // A poll signed as POST and sent as DELETE disconnects nothing.
    c.grant_request(json!({})).await;
    let gid = c.grant_id();
    let cont = c.held().continue_token.unwrap();
    let path = format!("/api/grants/{gid}/continue");
    let res = c
        .call(Call {
            method: "POST",
            path: &path,
            authorization: Some(format!("GNAP {cont}")),
            sent_method: Some("DELETE"),
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    assert_eq!(ts.store.grant(&gid).unwrap().status, "pending");
    // A manage call signed for one route and sent to another.
    let sam = ts.connect_person("Sam", "clax:reader").await;
    let gid = sam.grant_id();
    let manage = sam.held().manage.unwrap();
    let (signed, sent) = (
        format!("/api/grants/{gid}/continue"),
        format!("/api/grants/{gid}/token"),
    );
    let res = sam
        .call(Call {
            method: "DELETE",
            path: &signed,
            authorization: Some(format!("GNAP {manage}")),
            sent_path: Some(&sent),
            ..Default::default()
        })
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_signature".into()));
    assert_eq!(ts.store.grant(&gid).unwrap().status, "approved");
    assert_eq!(sam.revoke_token().await.status(), 204);
}

#[tokio::test]
async fn responses_with_tokens_or_codes_are_not_stored() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let res = ts
        .owner_browser()
        .post("/api/people/invites", json!({"name": "Sam"}))
        .await;
    assert_eq!(res.headers()["cache-control"], "no-store");
    let cmd = res.json::<Value>().await.unwrap()["invite"]["command"]
        .as_str()
        .unwrap()
        .to_string();
    let res = TestClient::new(&ts)
        .grant_request_raw(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    assert_eq!(
        (
            res.status().as_u16(),
            res.headers()["cache-control"].to_str().unwrap()
        ),
        (200, "no-store")
    );
    let (c, gid, user_code) = pending(&ts, "clax:reader").await;
    clock.advance(5);
    let (path, cont) = (
        format!("/api/grants/{gid}/continue"),
        c.held().continue_token.unwrap(),
    );
    let res = c.gnap("POST", &path, &cont).await;
    assert_eq!(
        (
            res.status().as_u16(),
            res.headers()["cache-control"].to_str().unwrap()
        ),
        (200, "no-store"),
        "a pending poll echoes the continuation"
    );
    let res = ts
        .owner_browser()
        .post(
            &format!("/api/grants/{gid}/approve"),
            json!({"user_code": user_code, "name": "Mia"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    clock.advance(5);
    let res = c.gnap("POST", &path, &cont).await;
    assert_eq!(
        (
            res.status().as_u16(),
            res.headers()["cache-control"].to_str().unwrap()
        ),
        (200, "no-store")
    );
    let res = TestClient::new(&ts).grant_request_raw(json!({})).await;
    assert_eq!(
        (
            res.status().as_u16(),
            res.headers()["cache-control"].to_str().unwrap()
        ),
        (201, "no-store")
    );
}

#[tokio::test]
async fn the_browser_link_refuses_navigations_from_other_pages() {
    let ts = lan_server().await;
    let cmd = ts.invite_command("Sam").await;
    let r = TestClient::new(&ts)
        .grant_request(json!({"clax": {"invite": code_of(&cmd)}}))
        .await;
    let link = r["clax"]["browser_link"].as_str().unwrap();
    let (lan, _) = ts.lan();
    for site in ["cross-site", "same-site"] {
        let res = lan
            .get(link)
            .header("sec-fetch-site", site)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{site}");
        assert!(res.headers().get("set-cookie").is_none());
    }
    let res = lan
        .get(link)
        .header("sec-fetch-site", "none")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the code was kept");
}

#[tokio::test]
async fn ending_an_ended_grant_announces_nothing() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:reader").await;
    let mut people = ts.stream_as_owner(&["people"]).await;
    let owner = ts.owner_browser();
    let path = format!("/api/grants/{}", sam.grant_id());
    for _ in 0..2 {
        let res = owner.delete(&path).await;
        assert_eq!(res.status(), 200);
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["grant"]["status"], "revoked");
    }
    ts.invite_command("Marker").await;
    let ev = people.next_named("people").await;
    assert_eq!(ev["change"]["what"], "grant_ended");
    let ev = people.next_named("people").await;
    assert_eq!(
        ev["change"]["what"], "invite_created",
        "one end, announced once"
    );
}

#[tokio::test]
async fn a_failed_index_change_still_hands_out_the_tokens() {
    let ts = lan_server().await;
    let res = ts
        .post_json("/api/_test/grants/index/fail", json!({}))
        .await;
    assert_eq!(res.status(), 204);
    let sam = ts.connect_person("Sam", "clax:reader").await;
    assert!(sam.held().access.unwrap().starts_with("cxg_"));
    let v = index(&ts).await;
    assert_eq!(
        (v["tokens"].as_u64(), v["stale"].as_bool()),
        (Some(0), Some(true))
    );
    assert_eq!(
        status_code(sam.revoke_token().await).await,
        (401, "invalid_token".into()),
        "unknown until the rebuild"
    );
    let res = ts.post_json("/api/_test/grants/sweep", json!({})).await;
    assert_eq!(res.status(), 200);
    let v = index(&ts).await;
    assert_eq!(
        (v["tokens"].as_u64(), v["stale"].as_bool()),
        (Some(3), Some(false))
    );
    assert_eq!(sam.revoke_token().await.status(), 204);
}

#[tokio::test]
async fn a_sniffed_pending_poll_cannot_be_replayed_after_a_flood() {
    let clock = Arc::new(ManualClock::at(T0));
    let ts = lan_server_at(&clock).await;
    let (c, gid, user_code) = pending(&ts, "clax:reader").await;
    let cont = c.held().continue_token.unwrap();
    let path = format!("/api/grants/{gid}/continue");
    let nonce = clax_core::sig::new_nonce();
    // The poll a sniffer captures: same key, path, token, time and nonce
    // sign to the same request.
    let poll = |skew: i64| Call {
        method: "POST",
        path: &path,
        authorization: Some(format!("GNAP {cont}")),
        nonce: Some(nonce.clone()),
        skew,
        ..Default::default()
    };
    clock.advance(5);
    assert_eq!(c.call(poll(0)).await.status(), 200);
    let res = ts
        .owner_browser()
        .post(
            &format!("/api/grants/{gid}/approve"),
            json!({"user_code": user_code, "name": "Sam"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    // Sixteen addresses flood the grant-request cache with throwaway keys.
    for p in 0..16u32 {
        for _ in 0..16 {
            let res = TestClient::new(&ts)
                .from_peer(peer(100 + p))
                .grant_request_raw(json!({"clax": {"invite": "AAAA-AAAA-AAAA"}}))
                .await;
            assert_eq!(res.status(), 400);
        }
    }
    clock.advance(3);
    let sniffer = TestClient::with_key(&ts, KeyPair::from_pkcs8(c.key.pkcs8().expose()).unwrap())
        .from_peer(peer(200));
    let res = sniffer.call(poll(-3)).await;
    assert_eq!(status_code(res).await, (401, "replayed_request".into()));
    let (status, v) = c.poll().await;
    assert_eq!(status, 200, "{v}");
    assert!(
        v["access_token"]["value"]
            .as_str()
            .unwrap()
            .starts_with("cxg_")
    );
}

#[tokio::test]
async fn a_guessed_continuation_costs_no_store_read_before_its_signature_header() {
    let ts = lan_server().await;
    let c = TestClient::new(&ts);
    for call in [
        Call {
            method: "POST",
            path: "/api/grants/g_guess/continue",
            authorization: Some("GNAP cxc_guess".into()),
            unsigned: true,
            ..Default::default()
        },
        Call {
            method: "POST",
            path: "/api/grants/g_guess/continue",
            authorization: Some("GNAP cxc_guess".into()),
            skew: 500,
            ..Default::default()
        },
    ] {
        let unsigned = call.unsigned;
        let res = c.call(call).await;
        let want = if unsigned {
            "invalid_signature"
        } else {
            "stale_request"
        };
        assert_eq!(status_code(res).await, (401, want.into()));
    }
    let res = c
        .gnap("POST", "/api/grants/g_guess/continue", "cxc_guess")
        .await;
    assert_eq!(status_code(res).await, (401, "invalid_token".into()));
}

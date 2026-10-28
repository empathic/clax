//! The owner's passkey signing page and its routes (`[sign]`,
//! `clax_server::routes::sign`): only the owner's browser reaches them, a
//! message is queued with the home's chitin CLI, and nothing is signed here.
use clax_testkit::TestServer;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// The `tailscale serve` name these tests trust.
const TRUSTED: &str = "phobos.tail556f5b.ts.net";
/// The owner's tailnet login.
const OWNER: &str = "bdelanghe@github";

/// What chitin printed on phobos, 2026-10-28, for `CHITIN_TITLE=clax chitin
/// say --as bobby --queue personal-super-human "recording clax's /sign
/// fixture; declined at once"` (stdout), and then for `chitin pending --as
/// bobby --json`; the item was declined at once.
const QUEUED: &str = "queued 391709736a2c\n";
const PENDING: &str = r#"[{"id":"391709736a2c","created_at":"2026-10-28T19:11:20.982Z","as":"bobby","to":"personal-super-human","text":"recording clax's /sign fixture; declined at once","queued_by":"clax","chars":48,"text_hash":"9cf3b21e","age_s":0,"expired":false,"gated":true,"declined":false}]"#;

/// A chitin that answers the `queued.out` and `pending.json` beside it ([`QUEUED`], [`PENDING`]), and
/// keeps its arguments and environment beside it.
const FAKE_CHITIN: &str = r#"#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$@" > "$d/args"
printf '%s|%s\n' "$CHITIN_TITLE" "$CHITIN_STATE_DIR" > "$d/env"
case "$1" in
  pending) cat "$d/pending.json" ;;
  say) cat "$d/queued.out" ;;
  *) exit 2 ;;
esac
"#;

/// A daemon whose tailnet login [`OWNER`] is the owner through [`TRUSTED`],
/// with `[sign]` naming a fake chitin in `dir` (or none).
async fn signing(dir: Option<&Path>) -> TestServer {
    let sign = dir.map(|d| {
        std::fs::write(d.join("pending.json"), PENDING).unwrap();
        std::fs::write(d.join("queued.out"), QUEUED).unwrap();
        let bin = clax_fake_exe::install(&d.join("chitin"), FAKE_CHITIN);
        Arc::new(clax_core::config::SignConfig {
            chitin: bin.to_string_lossy().into_owned(),
            as_title: "bobby".into(),
            state_dir: Some("/state/chitin".into()),
        })
    });
    TestServer::spawn_with(move |s| {
        s.trusted_hosts = vec![TRUSTED.to_string()].into();
        s.tailscale_identity = true;
        s.owner_tailscale_login = Some(Arc::from(OWNER));
        s.sign = sign;
    })
    .await
}

/// `req` through [`TRUSTED`] as tailnet `login`, from the shell's own origin.
fn via(req: reqwest::RequestBuilder, login: &str) -> reqwest::RequestBuilder {
    req.header("host", TRUSTED)
        .header("tailscale-user-login", login)
        .header("sec-fetch-site", "same-origin")
}

fn args(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("args"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn the_owners_browser_gets_the_page_and_whats_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let ts = signing(Some(dir.path())).await;
    let res = via(ts.client.get(format!("{}/sign", ts.base)), OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let csp = res.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains("script-src 'nonce-") && csp.contains("frame-ancestors 'none'"),
        "{csp}"
    );
    let page = res.text().await.unwrap();
    assert!(page.contains("navigator.credentials.get") && page.contains("/approve/v1/assertion/"));
    assert!(!page.contains("{nonce}"));
    let res = via(ts.client.get(format!("{}/api/sign", ts.base)), OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["as"], "bobby");
    assert_eq!(body["pending"][0]["id"], "391709736a2c");
    assert_eq!(args(dir.path()), ["pending", "--as", "bobby", "--json"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("env")).unwrap(),
        "clax|/state/chitin\n"
    );
}

#[tokio::test]
async fn the_owner_queues_a_message_for_the_passkey_and_nothing_is_signed_here() {
    let dir = tempfile::tempdir().unwrap();
    let ts = signing(Some(dir.path())).await;
    let text = "approve bdelanghe/home#701 at 0123abc";
    let res = via(ts.client.post(format!("{}/api/sign/queue", ts.base)), OWNER)
        .header("origin", format!("https://{TRUSTED}"))
        .json(&json!({"to": "lander", "text": text}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body,
        json!({"id": "391709736a2c", "as": "bobby", "to": "lander", "text": text})
    );
    assert_eq!(
        args(dir.path()),
        ["say", "--as", "bobby", "--queue", "lander", text]
    );
    for (bad, code) in [
        (json!({"to": "--as", "text": "x"}), "invalid_recipient"),
        (json!({"to": "a b", "text": "x"}), "invalid_recipient"),
        (json!({"to": "lander", "text": "  "}), "invalid_text"),
        (
            json!({"to": "lander", "text": "x".repeat(801)}),
            "invalid_text",
        ),
        (
            json!({"to": "lander", "text": "a\u{1b}[2J"}),
            "invalid_text",
        ),
    ] {
        let res = via(ts.client.post(format!("{}/api/sign/queue", ts.base)), OWNER)
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "{bad}");
        let b: Value = res.json().await.unwrap();
        assert_eq!(b["error"]["code"], code, "{bad}: {b}");
    }
}

#[tokio::test]
async fn no_one_but_the_owners_browser_reaches_it() {
    let dir = tempfile::tempdir().unwrap();
    let ts = signing(Some(dir.path())).await;
    // A tailnet viewer: a 403 page, and the routes refuse.
    let res = via(ts.client.get(format!("{}/sign", ts.base)), "eve@github")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    for login in ["eve@github", "BDELANGHE@github"] {
        let res = via(ts.client.get(format!("{}/api/sign", ts.base)), login)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{login}");
        let res = via(ts.client.post(format!("{}/api/sign/queue", ts.base)), login)
            .json(&json!({"to": "lander", "text": "x"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{login}");
    }
    // Another site's page in the owner's browser.
    let res = ts
        .client
        .post(format!("{}/api/sign/queue", ts.base))
        .header("host", TRUSTED)
        .header("tailscale-user-login", OWNER)
        .header("sec-fetch-site", "cross-site")
        .header("origin", "https://evil.example")
        .json(&json!({"to": "lander", "text": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // The token alone (the CLI, an agent): a browser is required.
    let res = ts
        .client
        .get(format!("{}/api/sign", ts.base))
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let b: Value = res.json().await.unwrap();
    assert_eq!(b["error"]["code"], "browser_required");
    // None of them ran chitin.
    assert!(args(dir.path()).is_empty());
}

#[tokio::test]
async fn without_sign_there_is_no_page_and_no_route() {
    let ts = signing(None).await;
    for path in ["/sign", "/api/sign"] {
        let res = via(ts.client.get(format!("{}{path}", ts.base)), OWNER)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "{path}");
    }
}

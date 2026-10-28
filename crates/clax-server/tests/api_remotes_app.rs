//! The Clax app's core and a public Clax (`clax_server::public`, app mode,
//! and `clax_server::routes::remote_apps`): publishing one chosen local
//! version as the next public version, its bundle forwarded beside it, a
//! link made without a push, and the routes that read and share the public
//! copy through the remote's token, which the app never holds.

use clax_core::ArtifactId;
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, connect, connect_app, json_of, local_req, post};
use reqwest::header::HOST;
use serde_json::{Value, json};

const MIME: &str = "application/vnd.empathic.clax+zip";
const LINEAGE: &str = "l_abcdefghijklmnopqrstuvwxyz";

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

fn files(pairs: &[(&str, Option<&str>)]) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in pairs {
        m.insert(
            k.to_string(),
            match v {
                Some(c) => json!({"content": c, "encoding": "utf8"}),
                None => Value::Null,
            },
        );
    }
    Value::Object(m)
}

/// A local app: version 1 of `pairs`, its ID.
async fn app(local: &TestServer, title: &str, pairs: &[(&str, &str)]) -> String {
    local.publish(title, pairs).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Publishes version `n + 1` of `aid` locally with `pairs` (a `None`
/// removes the path).
async fn version(local: &TestServer, aid: &str, n: u32, pairs: &[(&str, Option<&str>)]) {
    let (s, v) = post(
        local,
        &format!("/api/artifacts/{aid}/versions"),
        json!({"if_version": n, "files": files(pairs)}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
}

/// `POST /api/artifacts/<aid>/push {mode: app, version}`.
async fn publish(local: &TestServer, aid: &str, n: u32) -> (u16, Value) {
    post(
        local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": n}),
    )
    .await
}

async fn get(local: &TestServer, path: &str) -> (u16, Value) {
    local_req(local, reqwest::Method::GET, path, json!(null), None).await
}

/// The public artifact's ID a publish answered.
fn raid_of(v: &Value) -> String {
    v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Each file of version `n` of `aid` in `ts`'s store: its path and text.
fn texts(ts: &TestServer, aid: &str, n: u32) -> Vec<(String, String)> {
    let id = ArtifactId::parse(aid).unwrap();
    let v = ts.store.get_version(&id, n).unwrap().unwrap();
    v.files
        .keys()
        .map(|p| {
            let (b, _) = ts.store.read_file(&id, n, p).unwrap().unwrap();
            (p.clone(), String::from_utf8(b).unwrap())
        })
        .collect()
}

/// Version `n`'s content hash in `ts`'s store.
fn content_hash(ts: &TestServer, aid: &str, n: u32) -> String {
    let id = ArtifactId::parse(aid).unwrap();
    ts.store
        .get_version(&id, n)
        .unwrap()
        .unwrap()
        .content_sha256
        .unwrap()
}

fn current(public: &Public, raid: &str) -> u32 {
    let id = ArtifactId::parse(raid).unwrap();
    public
        .ts
        .store
        .get_artifact(&id)
        .unwrap()
        .map_or(0, |a| a.current_version)
}

/// The agent token `local` holds on its only remote.
fn token_of(local: &TestServer) -> String {
    let r = local.store.remotes().unwrap().remove(0);
    local
        .store
        .remote_token(&r.id)
        .unwrap()
        .unwrap()
        .expose()
        .to_string()
}

/// A POST to `public` as agent token `token`, in a session of its own,
/// with idempotency key `key`.
async fn as_token(
    public: &Public,
    token: &str,
    path: &str,
    body: Value,
    key: &str,
) -> (u16, Value) {
    let base = public.ts.base.to_string();
    let host = format!("localhost:{}", public.ts.addr.port());
    let (s, sess) = json_of(
        public
            .ts
            .client
            .post(format!("{base}/api/sessions"))
            .header(HOST, &host)
            .bearer_auth(token)
            .json(&json!({"harness": "clax", "cwd": ""}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 201, "{sess}");
    json_of(
        public
            .ts
            .client
            .post(format!("{base}{path}"))
            .header(HOST, &host)
            .bearer_auth(token)
            .header("x-clax-session", sess["session"]["id"].as_str().unwrap())
            .header(
                "x-clax-session-secret",
                sess["session_secret"].as_str().unwrap(),
            )
            .header("idempotency-key", key)
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await
}

/// `PUT /api/artifacts/<aid>/remotes/<rid>/bundle?remote_n=<n>` with
/// `bytes`, of release `bv`, whose files hash to `content`.
async fn put_bundle(
    local: &TestServer,
    aid: &str,
    rid: &str,
    n: u32,
    bytes: &[u8],
    bv: u64,
    content: &str,
) -> (u16, Value) {
    json_of(
        local
            .authed(local.client.put(format!(
                "{}/api/artifacts/{aid}/remotes/{rid}/bundle?remote_n={n}",
                local.base
            )))
            .header("content-type", MIME)
            .header("clax-bundle-lineage", LINEAGE)
            .header("clax-bundle-version", bv.to_string())
            .header("clax-bundle-content-sha256", content)
            .body(bytes.to_vec())
            .send()
            .await
            .unwrap(),
    )
    .await
}

/// `GET path` of `local`: the status, the content type and the bytes.
async fn bytes(local: &TestServer, path: &str) -> (u16, String, Vec<u8>) {
    let r = local
        .authed(local.client.get(format!("{}{path}", local.base)))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let ct = r
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (status, ct, r.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn app_push_sends_only_the_chosen_version() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(
        &local,
        "Tips",
        &[("index.html", "<h1>v1</h1>"), ("a.txt", "a")],
    )
    .await;
    version(&local, &aid, 1, &[("index.html", Some("<h1>v2</h1>"))]).await;
    version(
        &local,
        &aid,
        2,
        &[
            ("index.html", Some("<h1>v3</h1>")),
            ("a.txt", None),
            ("b.txt", Some("b")),
        ],
    )
    .await;
    // Version 2 alone becomes the public copy's version 1.
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "pushed");
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 1}]), "{v}");
    assert_eq!(v["remote"]["id"], rid.as_str());
    let raid = raid_of(&v);
    assert_eq!(v["url"], format!("{}/a/{raid}", public.origin));
    assert_eq!(v["link"]["remote_n"], 1);
    assert_eq!(current(&public, &raid), 1);
    assert_eq!(texts(&public.ts, &raid, 1), texts(&local, &aid, 2));
    assert_eq!(
        content_hash(&public.ts, &raid, 1),
        content_hash(&local, &aid, 2)
    );
    // Version 3 next: the path it dropped goes there too.
    let (s, v) = publish(&local, &aid, 3).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 3, "remote_n": 2}]), "{v}");
    assert_eq!(texts(&public.ts, &raid, 2), texts(&local, &aid, 3));
    assert_eq!(
        content_hash(&public.ts, &raid, 2),
        content_hash(&local, &aid, 3)
    );
    // An older one after it: its files as they were, as the next version.
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 1, "remote_n": 3}]), "{v}");
    assert_eq!(texts(&public.ts, &raid, 3), texts(&local, &aid, 1));
    assert_eq!(
        content_hash(&public.ts, &raid, 3),
        content_hash(&local, &aid, 1)
    );
    // A version the app does not have, or no version, is refused.
    let (s, v) = publish(&local, &aid, 9).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = post(
        &local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    assert_eq!(current(&public, &raid), 3);
}

#[tokio::test]
async fn app_push_same_local_version_twice_makes_two_public_versions() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect_app(&local, &public).await;
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    for n in 1..5 {
        let page = format!("<h1>v{}</h1>", n + 1);
        version(&local, &aid, n, &[("index.html", Some(page.as_str()))]).await;
    }
    let mut made = Vec::new();
    for n in [3, 5, 3] {
        let (s, v) = publish(&local, &aid, n).await;
        assert_eq!(s, 200, "{v}");
        made.push(v["pushed"][0]["remote_n"].as_u64().unwrap());
    }
    assert_eq!(made, vec![1, 2, 3], "v3 a second time is a new version");
    let (s, v) = publish(&local, &aid, 3).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    assert_eq!(current(&public, &raid), 4);
    assert_eq!(texts(&public.ts, &raid, 3), texts(&local, &aid, 3));
    assert_eq!(texts(&public.ts, &raid, 4), texts(&local, &aid, 3));
}

#[tokio::test]
async fn app_push_replays_after_lost_answer() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let token = token_of(&local);
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    version(&local, &aid, 1, &[("index.html", Some("<h1>v2</h1>"))]).await;
    // The public Clax made the first publish (of version 2), but its answer
    // never came back.
    let key = clax_server::public::app_version_key(&aid, 2, "new:0", 0);
    let (s, first) = as_token(
        &public,
        &token,
        "/api/artifacts",
        json!({"title": "Tips", "files": files(&[("index.html", Some("<h1>v2</h1>"))])}),
        &key,
    )
    .await;
    assert_eq!(s, 201, "{first}");
    let raid = first["artifact"]["id"].as_str().unwrap().to_string();
    // Sent again: that artifact, not a second one.
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(raid_of(&v), raid);
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 1}]));
    assert_eq!(public.ts.store.list_artifacts().unwrap().len(), 1);
    // A later publish whose answer was lost: no `remote_moved`, and no
    // second public version.
    version(&local, &aid, 2, &[("index.html", Some("<h1>v3</h1>"))]).await;
    let key = clax_server::public::app_version_key(&aid, 3, &raid, 1);
    let (s, v) = as_token(
        &public,
        &token,
        &format!("/api/artifacts/{raid}/versions"),
        json!({"if_version": 1, "files": files(&[("index.html", Some("<h1>v3</h1>"))])}),
        &key,
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = publish(&local, &aid, 3).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 3, "remote_n": 2}]));
    assert_eq!(current(&public, &raid), 2);
    // The bundle's upload is sent until it lands: the same bytes again
    // answer as the first time.
    let content = content_hash(&local, &aid, 3);
    let (s, first) = put_bundle(&local, &aid, &rid, 2, b"bundle", 4, &content).await;
    assert_eq!(s, 201, "{first}");
    let (s, again) = put_bundle(&local, &aid, &rid, 2, b"bundle", 4, &content).await;
    assert_eq!((s, &again), (201, &first));
}

#[tokio::test]
async fn app_push_refuses_blob_refs() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    connect_app(&local, &public).await;
    for page in [
        "<img src=\"/_blob/01ARZ3NDEKTSV4RRFFQ69G5FAV\">",
        "<img src=\"http://localhost:7480/_blob/01ARZ3NDEKTSV4RRFFQ69G5FAV\">",
    ] {
        let aid = app(
            &local,
            "Logo",
            &[("index.html", "<p>hi</p>"), ("app.js", page)],
        )
        .await;
        let (s, v) = publish(&local, &aid, 1).await;
        assert_eq!((s, code(&v)), (400, "uses_assets"), "{v}");
        assert!(local.store.remote_links(&aid).unwrap().is_empty());
    }
    assert!(public.ts.store.list_artifacts().unwrap().is_empty());
    // Text that names no asset's address is sent.
    let aid = app(
        &local,
        "Docs",
        &[("index.html", "<p>/_blob/ is where assets live</p>")],
    )
    .await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn app_push_creates_with_comments_off_and_restricted() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let id = ArtifactId::parse(&raid).unwrap();
    assert!(!public.ts.store.comments_on(&id).unwrap());
    let (s, sharing) = get(
        &local,
        &format!("/api/remotes/{rid}/artifacts/{raid}/sharing"),
    )
    .await;
    assert_eq!(s, 200, "{sharing}");
    assert_eq!(sharing["general"], "restricted", "{sharing}");
    assert_eq!(sharing["comments"], false, "{sharing}");
    assert_eq!(sharing["people"], json!([]), "{sharing}");
    // No one without a session reads it.
    let r = public
        .ts
        .client
        .get(format!("{}/api/artifacts/{raid}/head", public.ts.base))
        .header(HOST, format!("localhost:{}", public.ts.addr.port()))
        .send()
        .await
        .unwrap();
    assert!(matches!(r.status().as_u16(), 401 | 404), "{}", r.status());
    // A later version leaves the switch as the owner set it.
    let (s, v) = local_req(
        &local,
        reqwest::Method::PUT,
        &format!("/api/remotes/{rid}/artifacts/{raid}/sharing/comments"),
        json!({"on": true}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    version(&local, &aid, 1, &[("index.html", Some("<h1>v2</h1>"))]).await;
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert!(public.ts.store.comments_on(&id).unwrap());
}

#[tokio::test]
async fn app_push_after_web_edit_is_remote_moved() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    // Someone publishes on the web.
    let (s, v) = public
        .cli(
            reqwest::Method::POST,
            &format!("/api/artifacts/{raid}/versions"),
            Some(json!({"if_version": 1, "files": files(&[("index.html", Some("<h1>web</h1>"))])})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    version(&local, &aid, 1, &[("index.html", Some("<h1>v2</h1>"))]).await;
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!((s, code(&v)), (409, "remote_moved"), "{v}");
    assert_eq!(v["error"]["remote_version"], 2, "{v}");
    assert_eq!(v["error"]["pushed_version"], 1, "{v}");
    assert_eq!(current(&public, &raid), 2);
    let link = local.store.remote_link(&aid, &rid).unwrap().unwrap();
    assert_eq!((link.pushed_n, link.remote_n), (1, 1));
    assert_eq!(texts(&public.ts, &raid, 2)[0].1, "<h1>web</h1>");
}

#[tokio::test]
async fn bundle_forward_roundtrip() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let content = content_hash(&local, &aid, 1);
    // No link yet: nothing to put it on.
    let (s, v) = put_bundle(&local, &aid, &rid, 1, b"zip", 1, &content).await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let bundle: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
    let (s, v) = put_bundle(&local, &aid, &rid, 1, &bundle, 3, &content).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (
            v["artifact_id"].as_str(),
            v["version"].as_u64(),
            v["bundle_version"].as_u64()
        ),
        (Some(raid.as_str()), Some(1), Some(3)),
        "{v}"
    );
    let (s, again) = put_bundle(&local, &aid, &rid, 1, &bundle, 3, &content).await;
    assert_eq!((s, &again), (201, &v));
    // The remote's refusals come back as it made them.
    let (s, v) = put_bundle(&local, &aid, &rid, 1, b"other", 4, "sha256:00").await;
    assert_eq!((s, code(&v)), (400, "bundle_mismatch"), "{v}");
    let (s, v) = put_bundle(&local, &aid, &rid, 7, &bundle, 4, &content).await;
    assert_eq!(s, 404, "{v}");
    // And it reads back byte for byte.
    let (s, ct, got) = bytes(
        &local,
        &format!("/api/remotes/{rid}/artifacts/{raid}/versions/1/bundle"),
    )
    .await;
    assert_eq!((s, ct.as_str()), (200, MIME));
    assert_eq!(got, bundle);
    let (s, head) = get(&local, &format!("/api/remotes/{rid}/artifacts/{raid}/head")).await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["bundle"]["bundle_version"], 3, "{head}");
    assert_eq!(head["bundle"]["content_sha256"], content.as_str(), "{head}");
}

#[tokio::test]
async fn link_without_push_then_app_push_bases_on_it() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The public copy, made on another device: two versions.
    let (s, v) = public
        .cli(
            reqwest::Method::POST,
            "/api/artifacts",
            Some(json!({"title": "Tips", "files": files(&[("index.html", Some("<h1>1</h1>")), ("old.txt", Some("o"))])})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let raid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = public
        .cli(
            reqwest::Method::POST,
            &format!("/api/artifacts/{raid}/versions"),
            Some(json!({"if_version": 1, "files": files(&[("index.html", Some("<h1>2</h1>"))])})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // Got here as version 2, its files as one local version.
    let aid = app(
        &local,
        "Tips",
        &[("index.html", "<h1>2</h1>"), ("old.txt", "o")],
    )
    .await;
    let link = |remote_n: u32| {
        json!({
            "artifact_id": aid,
            "remote_artifact_id": raid,
            "url": format!("{}/a/{raid}", public.origin),
            "remote_n": remote_n,
        })
    };
    let (s, v) = post(&local, &format!("/api/remotes/{rid}/links"), link(2)).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (
            v["link"]["pushed_n"].as_u64(),
            v["link"]["remote_n"].as_u64()
        ),
        (Some(1), Some(2)),
        "{v}"
    );
    assert_eq!(v["link"]["url"], format!("{}/a/{raid}", public.origin));
    assert_eq!(v["link"]["auto"], false);
    // A change here goes up as version 3, built on version 2.
    version(
        &local,
        &aid,
        1,
        &[("index.html", Some("<h1>3</h1>")), ("old.txt", None)],
    )
    .await;
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(raid_of(&v), raid);
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 3}]));
    assert_eq!(texts(&public.ts, &raid, 3), texts(&local, &aid, 2));
    // Linked again on a stale base: the next publish is refused as moved.
    let (s, v) = post(&local, &format!("/api/remotes/{rid}/links"), link(2)).await;
    assert_eq!(s, 201, "{v}");
    version(&local, &aid, 2, &[("index.html", Some("<h1>4</h1>"))]).await;
    let (s, v) = publish(&local, &aid, 3).await;
    assert_eq!((s, code(&v)), (409, "remote_moved"), "{v}");
    assert_eq!(current(&public, &raid), 3);
    // A link names a public artifact by its ID, a local artifact that is
    // here, and a remote this core is signed in to.
    for (body, want) in [
        (
            json!({"artifact_id": aid, "remote_artifact_id": "../x", "url": "u", "remote_n": 1}),
            400,
        ),
        (
            json!({"artifact_id": "nope", "remote_artifact_id": raid, "url": "u", "remote_n": 1}),
            400,
        ),
        (
            json!({"artifact_id": "a0000000000z", "remote_artifact_id": raid, "url": "u", "remote_n": 1}),
            404,
        ),
        (
            json!({"artifact_id": aid, "remote_artifact_id": raid, "url": "u", "remote_n": 0}),
            400,
        ),
    ] {
        let (s, v) = post(&local, &format!("/api/remotes/{rid}/links"), body).await;
        assert_eq!(s, want, "{v}");
    }
    let (s, v) = post(&local, "/api/remotes/r_missing/links", link(1)).await;
    assert_eq!(s, 404, "{v}");
}

#[tokio::test]
async fn proxies_forward_head_bundle_files_sharing_apps() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let token = token_of(&local);
    let aid = app(
        &local,
        "Tips",
        &[("index.html", "<h1>v1</h1>"), ("js/a b.js", "let x = 1;")],
    )
    .await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let content = content_hash(&local, &aid, 1);
    let (s, v) = put_bundle(&local, &aid, &rid, 1, b"zip", 1, &content).await;
    assert_eq!(s, 201, "{v}");
    let copy = format!("/api/remotes/{rid}/artifacts/{raid}");
    // The head, as the remote answers it.
    let (s, head) = get(&local, &format!("{copy}/head")).await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["artifact_id"], raid.as_str());
    assert_eq!(head["version"], 1);
    assert_eq!(head["content_sha256"], content.as_str());
    assert_eq!(head["comments"], false);
    assert_eq!(head["bundle"]["version"], 1);
    // Passed through whole: every field the remote's head carries.
    let (s, direct) = public
        .cli(
            reqwest::Method::GET,
            &format!("/api/artifacts/{raid}/head"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{direct}");
    assert_eq!(head, direct);
    // The bundle.
    let (s, ct, got) = bytes(&local, &format!("{copy}/versions/1/bundle")).await;
    assert_eq!((s, ct.as_str(), got.as_slice()), (200, MIME, &b"zip"[..]));
    // The version's files, and each file's bytes, its path encoded.
    let (s, listed) = get(&local, &format!("{copy}/versions/1/files")).await;
    assert_eq!(s, 200, "{listed}");
    let mut paths: Vec<&String> = listed["version"]["files"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    paths.sort();
    assert_eq!(paths, ["index.html", "js/a b.js"], "{listed}");
    let (s, _, got) = bytes(&local, &format!("{copy}/versions/1/files/js/a%20b.js")).await;
    assert_eq!((s, got.as_slice()), (200, &b"let x = 1;"[..]));
    let (s, _, got) = bytes(&local, &format!("{copy}/versions/1/files/index.html")).await;
    assert_eq!((s, got.as_slice()), (200, &b"<h1>v1</h1>"[..]));
    // A path or ID that would leave the public copy goes nowhere.
    for path in [
        format!("{copy}/versions/1/files/..%2F..%2Fhead"),
        format!("{copy}/versions/1/files/js//a"),
        format!("/api/remotes/{rid}/artifacts/{raid}%2Fversions/head"),
        format!("/api/remotes/{rid}/artifacts/..%2F..%2Fhosted%2Fapps/head"),
    ] {
        let (s, _, got) = bytes(&local, &path).await;
        assert!(matches!(s, 400 | 404), "{path}: {s}");
        assert!(!String::from_utf8_lossy(&got).contains(&raid), "{path}");
    }
    // The remote's refusals, as it made them.
    let (s, v) = get(
        &local,
        &format!("/api/remotes/{rid}/artifacts/a0000000000z/head"),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, _, _) = bytes(&local, &format!("{copy}/versions/1/files/missing.txt")).await;
    assert_eq!(s, 404);
    // Sharing.
    let (s, sharing) = get(&local, &format!("{copy}/sharing")).await;
    assert_eq!(s, 200, "{sharing}");
    assert_eq!(
        (sharing["can_share"].as_bool(), sharing["general"].as_str()),
        (Some(true), Some("restricted")),
        "{sharing}"
    );
    let put = |path: String, body: Value| {
        let local = &local;
        async move { local_req(local, reqwest::Method::PUT, &path, body, None).await }
    };
    let (s, v) = put(
        format!("{copy}/sharing/link"),
        json!({"access": "viewer", "listed": false}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = put(format!("{copy}/sharing/link"), json!({"access": "owner"})).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = put(format!("{copy}/sharing/comments"), json!({"on": true})).await;
    assert_eq!((s, v["comments"].as_bool()), (200, Some(true)), "{v}");
    let (s, v) = put(
        format!("{copy}/sharing/shares"),
        json!({"github_login": "octocat", "access": "editor"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let sid = v["share"]["id"].as_str().unwrap().to_string();
    let (s, sharing) = get(&local, &format!("{copy}/sharing")).await;
    assert_eq!(s, 200);
    assert_eq!(
        (sharing["general"].as_str(), sharing["comments"].as_bool()),
        (Some("link"), Some(true)),
        "{sharing}"
    );
    assert_eq!(sharing["people"][0]["github_login"], "octocat", "{sharing}");
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("{copy}/sharing/shares/{sid}"),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // The apps library.
    let (s, apps) = get(&local, &format!("/api/remotes/{rid}/apps")).await;
    assert_eq!(s, 200, "{apps}");
    assert_eq!(apps[0]["artifact_id"], raid.as_str(), "{apps}");
    assert_eq!(apps[0]["access"], "owner", "{apps}");
    // No answer names the token.
    for path in [
        format!("{copy}/head"),
        format!("{copy}/sharing"),
        format!("/api/remotes/{rid}/apps"),
        "/api/remotes".to_string(),
    ] {
        let (_, _, got) = bytes(&local, &path).await;
        assert!(!String::from_utf8_lossy(&got).contains(&token), "{path}");
    }
    // The owner's alone: an agent of theirs is refused, and a hosted Clax
    // has none of these routes.
    let sid = local.register_session("claude", "h-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for path in [format!("{copy}/head"), format!("/api/remotes/{rid}/apps")] {
        let (s, v) = local_req(&local, reqwest::Method::GET, &path, json!(null), Some(&sid)).await;
        assert_eq!(s, 403, "{path}: {v}");
    }
    for path in [format!("{copy}/head"), format!("/api/remotes/{rid}/apps")] {
        let (s, _) = public.cli(reqwest::Method::GET, &path, None).await;
        assert_eq!(s, 404, "{path}");
    }
    let (s, _) = public
        .cli(
            reqwest::Method::POST,
            &format!("/api/artifacts/{raid}/push"),
            Some(json!({"mode": "app", "version": 1})),
        )
        .await;
    assert_eq!(s, 404);
    // A remote this core is not signed in to.
    let (s, v) = get(
        &local,
        &format!("/api/remotes/r_missing/artifacts/{raid}/head"),
    )
    .await;
    assert_eq!(s, 404, "{v}");
}

#[tokio::test]
async fn editor_pull_then_push_publishes_as_editor() {
    let mut public = Public::new().await;
    // Ada, the owner, publishes from her phone.
    let ada = TestServer::spawn().await;
    let ada_rid = connect_app(&ada, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&ada, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&ada, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let content = content_hash(&ada, &aid, 1);
    let (s, v) = put_bundle(&ada, &aid, &ada_rid, 1, b"adas", 1, &content).await;
    assert_eq!(s, 201, "{v}");
    // Sam, who may edit it, on his.
    let sam_id = public.become_person("Sam").await;
    let sam = TestServer::spawn().await;
    let sam_rid = connect_app(&sam, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(
        &ada,
        reqwest::Method::PUT,
        &format!("/api/remotes/{ada_rid}/artifacts/{raid}/sharing/shares"),
        json!({"person": sam_id, "access": "editor"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // Sam sees it in his library.
    let (s, apps) = get(&sam, &format!("/api/remotes/{sam_rid}/apps")).await;
    assert_eq!(s, 200, "{apps}");
    assert_eq!(apps[0]["access"], "editor", "{apps}");
    // Got here: its files as a local version (as the app takes them from
    // the bundle), linked at version 1.
    let mine = app(&sam, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = post(
        &sam,
        &format!("/api/remotes/{sam_rid}/links"),
        json!({"artifact_id": mine, "remote_artifact_id": raid, "url": format!("{}/a/{raid}", public.origin), "remote_n": 1}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    // His change goes up as version 2, published by him.
    version(&sam, &mine, 1, &[("index.html", Some("<h1>Sam's</h1>"))]).await;
    let (s, v) = publish(&sam, &mine, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 2}]), "{v}");
    assert_eq!(texts(&public.ts, &raid, 2), texts(&sam, &mine, 2));
    let content = content_hash(&sam, &mine, 2);
    let (s, v) = put_bundle(&sam, &mine, &sam_rid, 2, b"sams", 2, &content).await;
    assert_eq!(s, 201, "{v}");
    let (s, head) = get(
        &ada,
        &format!("/api/remotes/{ada_rid}/artifacts/{raid}/head"),
    )
    .await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["version"], 2);
    assert_eq!(head["published_by"]["name"], "Sam", "{head}");
    assert_eq!(head["owner"]["name"], "Ada", "{head}");
    assert_eq!(head["bundle"]["published_by"]["name"], "Sam", "{head}");
    // Ada's phone, still based on version 1, is refused as moved.
    version(&ada, &aid, 1, &[("index.html", Some("<h1>Ada's</h1>"))]).await;
    let (s, v) = publish(&ada, &aid, 2).await;
    assert_eq!((s, code(&v)), (409, "remote_moved"), "{v}");
}

#[tokio::test]
async fn git_token_asks_the_remote_by_hand_and_refuses_agents() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let path = format!("/api/remotes/{rid}/artifacts/{raid}/git-token");
    // The owner by hand: the remote's own answer to a session's read token,
    // passed on. This hosted test Clax keeps no git remotes, so it refuses,
    // and the answer is its refusal, not a token of this core's.
    let (s, v) = post(&local, &path, json!({})).await;
    assert_ne!(s, 401, "{v}");
    assert!(!(200..300).contains(&s), "no git remotes there: {v}");
    assert!(v["error"]["code"].is_string(), "{v}");
    // An agent of the owner's is refused here, before the remote is asked.
    let sid = local.register_session("claude", "h-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = local_req(&local, reqwest::Method::POST, &path, json!({}), Some(&sid)).await;
    assert_eq!(s, 403, "{v}");
    // A remote this core is not signed in to.
    let (s, v) = post(
        &local,
        &format!("/api/remotes/r_missing/artifacts/{raid}/git-token"),
        json!({}),
    )
    .await;
    assert_eq!(s, 404, "{v}");
}

#[tokio::test]
async fn unpublish_deletes_remote_and_unlinks() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{rid}/artifacts/{raid}"),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["unlinked"], json!([aid]), "{v}");
    assert_eq!(current(&public, &raid), 0, "the public copy is gone");
    assert!(local.store.remote_link(&aid, &rid).unwrap().is_none());
    // The app is here still, and publishing it again makes a new public
    // copy.
    let (s, v_new) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v_new}");
    assert_ne!(raid_of(&v_new), raid);
    assert_eq!(v_new["pushed"], json!([{"n": 1, "remote_n": 1}]));
    // Gone already: nothing is deleted, and the new copy stays linked.
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{rid}/artifacts/{raid}"),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["deleted"].as_bool(), &v["unlinked"]),
        (Some(false), &json!([]))
    );
    assert!(local.store.remote_link(&aid, &rid).unwrap().is_some());
    // One the account may read but not delete is refused as the remote
    // refused it, and stays linked.
    let mut public = public;
    let sam = public.become_person("Sam").await;
    let (s, v) = local_req(
        &local,
        reqwest::Method::PUT,
        &format!(
            "/api/remotes/{rid}/artifacts/{}/sharing/shares",
            raid_of(&v_new)
        ),
        json!({"person": sam, "access": "viewer"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let other = TestServer::spawn().await;
    let orid = connect_app(&other, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let oaid = app(&other, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = post(
        &other,
        &format!("/api/remotes/{orid}/links"),
        json!({"artifact_id": oaid, "remote_artifact_id": raid_of(&v_new), "url": "u", "remote_n": 1}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = local_req(
        &other,
        reqwest::Method::DELETE,
        &format!("/api/remotes/{orid}/artifacts/{}", raid_of(&v_new)),
        json!(null),
        None,
    )
    .await;
    assert_eq!(s, 403, "{v}");
    assert!(other.store.remote_link(&oaid, &orid).unwrap().is_some());
    assert_eq!(current(&public, &raid_of(&v_new)), 1);
}

#[tokio::test]
async fn relink_moves_an_existing_links_base() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    // Someone else's version 2 lands there; this device takes it in as its
    // own version 2 and moves its link onto it.
    let (s, v) = public
        .cli(
            reqwest::Method::POST,
            &format!("/api/artifacts/{raid}/versions"),
            Some(json!({"if_version": 1, "files": files(&[("index.html", Some("<h1>2</h1>"))])})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    version(&local, &aid, 1, &[("index.html", Some("<h1>2+mine</h1>"))]).await;
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!((s, code(&v)), (409, "remote_moved"), "{v}");
    let (s, v) = post(
        &local,
        &format!("/api/remotes/{rid}/links"),
        json!({"artifact_id": aid, "remote_artifact_id": raid, "url": "u", "remote_n": 2}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (
            v["link"]["pushed_n"].as_u64(),
            v["link"]["remote_n"].as_u64()
        ),
        (Some(2), Some(2)),
        "{v}"
    );
    let l = local.store.remote_link(&aid, &rid).unwrap().unwrap();
    assert_eq!((l.pushed_n, l.remote_n), (2, 2));
    // The combined version goes up on the moved base.
    let (s, v) = publish(&local, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 3}]), "{v}");
    assert_eq!(texts(&public.ts, &raid, 3), texts(&local, &aid, 2));
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/comments {body}`.
async fn tell(local: &TestServer, rid: &str, raid: &str, body: Value) -> (u16, Value) {
    post(
        local,
        &format!("/api/remotes/{rid}/artifacts/{raid}/comments"),
        body,
    )
    .await
}

#[tokio::test]
async fn comment_posts_on_the_public_copy() {
    let mut public = Public::new().await;
    let ada = TestServer::spawn().await;
    let rid = connect_app(&ada, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&ada, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&ada, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    // A new public app takes no comments.
    let (s, v) = tell(&ada, &rid, &raid, json!({"body": "v1 fails"})).await;
    assert_eq!((s, code(&v)), (403, "comments_off"), "{v}");
    let (s, v) = local_req(
        &ada,
        reqwest::Method::PUT,
        &format!("/api/remotes/{rid}/artifacts/{raid}/sharing/comments"),
        json!({"on": true}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = tell(&ada, &rid, &raid, json!({"body": "v1 fails"})).await;
    assert_eq!(s, 201, "{v}");
    let (s, threads) = public
        .cli(
            reqwest::Method::GET,
            &format!("/api/artifacts/{raid}/threads"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{threads}");
    let all = threads.to_string();
    assert!(all.contains("v1 fails"), "{threads}");
    assert!(!all.contains(&token_of(&ada)), "{threads}");
    // An empty comment, and a public copy named by anything but its ID,
    // are refused here.
    let (s, v) = tell(&ada, &rid, &raid, json!({"body": "  "})).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = tell(&ada, &rid, "not-an-id", json!({"body": "x"})).await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = tell(&ada, "r_missing", &raid, json!({"body": "x"})).await;
    assert_eq!(s, 404, "{v}");
    // An account that may only view it is refused as the remote refused it.
    let sam_id = public.become_person("Sam").await;
    let (s, v) = local_req(
        &ada,
        reqwest::Method::PUT,
        &format!("/api/remotes/{rid}/artifacts/{raid}/sharing/shares"),
        json!({"person": sam_id, "access": "viewer"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let sam = TestServer::spawn().await;
    let sam_rid = connect_app(&sam, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = tell(&sam, &sam_rid, &raid, json!({"body": "from Sam"})).await;
    assert_eq!(s, 403, "{v}");
    let (_, threads) = public
        .cli(
            reqwest::Method::GET,
            &format!("/api/artifacts/{raid}/threads"),
            None,
        )
        .await;
    assert!(!threads.to_string().contains("from Sam"), "{threads}");
}

/// `POST /api/artifacts/<aid>/push {mode: app, version, forked_from}`.
async fn publish_from(local: &TestServer, aid: &str, n: u32, from: &str) -> (u16, Value) {
    post(
        local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": n, "forked_from": from}),
    )
    .await
}

fn lineage(public: &Public, raid: &str) -> Option<(String, u32)> {
    let id = ArtifactId::parse(raid).unwrap();
    public
        .ts
        .store
        .get_artifact(&id)
        .unwrap()
        .unwrap()
        .forked_from
        .map(|f| (f.artifact_id, f.version))
}

#[tokio::test]
async fn app_push_of_a_new_copy_records_forked_from() {
    let mut public = Public::new().await;
    let ada = TestServer::spawn().await;
    let rid = connect_app(&ada, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&ada, "Tips", &[("index.html", "<h1>1</h1>")]).await;
    let (s, v) = publish(&ada, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let source = raid_of(&v);
    version(&ada, &aid, 1, &[("index.html", Some("<h1>2</h1>"))]).await;
    let (s, v) = publish(&ada, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(lineage(&public, &source), None);
    // Published as a new app: a new public copy, made from the source's
    // current version.
    let mine = app(&ada, "Tips, mine", &[("index.html", "<h1>mine</h1>")]).await;
    let (s, v) = publish_from(&ada, &mine, 1, &source).await;
    assert_eq!(s, 200, "{v}");
    let made = raid_of(&v);
    assert_ne!(made, source);
    assert_eq!(lineage(&public, &made), Some((source.clone(), 2)));
    // Once linked, an app has its public copy: no lineage to give it.
    version(&ada, &mine, 1, &[("index.html", Some("<h1>mine 2</h1>"))]).await;
    let (s, v) = publish_from(&ada, &mine, 2, &source).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    assert_eq!(current(&public, &made), 1);
    // Another mode takes none, and a lineage names an artifact by its ID.
    let other = app(&ada, "Other", &[("index.html", "<h1>o</h1>")]).await;
    let (s, v) = post(
        &ada,
        &format!("/api/artifacts/{other}/push"),
        json!({"mode": "new", "forked_from": source}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = publish_from(&ada, &other, 1, "../x").await;
    assert_eq!(s, 400, "{v}");
    // A source that is not there makes nothing.
    let (s, v) = publish_from(&ada, &other, 1, "a0000000000z").await;
    assert_ne!(s, 200, "{v}");
    assert!(ada.store.remote_link(&other, &rid).unwrap().is_none());
    // A local Clax takes no lineage on a create: it forks with its route.
    let (s, v) = post(
        &ada,
        "/api/artifacts",
        json!({"title": "T", "files": files(&[("index.html", Some("x"))]), "forked_from": aid}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    // Sam forks what is shared with him, and nothing he cannot read.
    let secret = app(&ada, "Secret", &[("index.html", "<h1>s</h1>")]).await;
    let (s, v) = publish(&ada, &secret, 1).await;
    assert_eq!(s, 200, "{v}");
    let secret_raid = raid_of(&v);
    let sam_id = public.become_person("Sam").await;
    let (s, v) = local_req(
        &ada,
        reqwest::Method::PUT,
        &format!("/api/remotes/{rid}/artifacts/{source}/sharing/shares"),
        json!({"person": sam_id, "access": "viewer"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let sam = TestServer::spawn().await;
    connect_app(&sam, &public).await;
    let fork = app(&sam, "Sam's Tips", &[("index.html", "<h1>sam</h1>")]).await;
    let (s, v) = publish_from(&sam, &fork, 1, &secret_raid).await;
    assert_ne!(s, 200, "{v}");
    let (s, v) = publish_from(&sam, &fork, 1, &source).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(lineage(&public, &raid_of(&v)), Some((source, 2)));
}

#[tokio::test]
async fn app_push_after_unlink_makes_a_new_copy() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let first = raid_of(&v);
    let (s, v) = local_req(
        &local,
        reqwest::Method::DELETE,
        &format!("/api/artifacts/{aid}/remotes/{rid}"),
        json!(null),
        None,
    )
    .await;
    assert!((200..300).contains(&s), "{s} {v}");
    assert!(local.store.remote_link(&aid, &rid).unwrap().is_none());
    // The public copy stays; the next publish makes another.
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    assert_ne!(raid_of(&v), first);
    assert_eq!(v["pushed"], json!([{"n": 1, "remote_n": 1}]), "{v}");
    assert_eq!(current(&public, &first), 1);
}

#[tokio::test]
async fn proxied_files_are_sandboxed_and_unsniffed() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(
        &local,
        "Tips",
        &[
            ("index.html", "<h1>1</h1>"),
            ("x.html", "<script>1</script>"),
        ],
    )
    .await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let copy = format!("/api/remotes/{rid}/artifacts/{raid}");
    for path in [
        format!("{copy}/versions/1/files/x.html"),
        format!("{copy}/versions/1/files"),
        format!("{copy}/head"),
    ] {
        let r = local
            .authed(local.client.get(format!("{}{path}", local.base)))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200, "{path}");
        let h = r.headers();
        assert_eq!(
            h.get("content-security-policy")
                .map(|v| v.to_str().unwrap()),
            Some("sandbox; default-src 'none'"),
            "{path}"
        );
        assert_eq!(
            h.get("x-content-type-options").map(|v| v.to_str().unwrap()),
            Some("nosniff"),
            "{path}"
        );
    }
}

/// `method` `path` at `public` with agent token `token` and no session:
/// the status and the body's bytes.
async fn token_req(
    public: &Public,
    token: &str,
    method: reqwest::Method,
    path: &str,
) -> (u16, Vec<u8>) {
    let r = public
        .ts
        .client
        .request(method, format!("{}{path}", public.ts.base))
        .header(HOST, format!("localhost:{}", public.ts.addr.port()))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn a_device_reads_shared_apps_as_its_person() {
    let mut public = Public::new().await;
    let ada = TestServer::spawn().await;
    let ada_rid = connect_app(&ada, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&ada, "Tips", &[("index.html", "<h1>v1</h1>")]).await;
    let (s, v) = publish(&ada, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let content = content_hash(&ada, &aid, 1);
    let (s, v) = put_bundle(&ada, &aid, &ada_rid, 1, b"adas", 1, &content).await;
    assert_eq!(s, 201, "{v}");
    let sam_id = public.become_person("Sam").await;
    let (s, v) = local_req(
        &ada,
        reqwest::Method::PUT,
        &format!("/api/remotes/{ada_rid}/artifacts/{raid}/sharing/shares"),
        json!({"person": sam_id, "access": "viewer"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // Sam's phone reads it as Sam's browser does: nothing withheld.
    let sam = TestServer::spawn().await;
    let sam_rid = connect_app(&sam, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let copy = format!("/api/remotes/{sam_rid}/artifacts/{raid}");
    let (s, head) = get(&sam, &format!("{copy}/head")).await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["title"], "Tips", "{head}");
    assert_eq!(head["content_sha256"], content.as_str(), "{head}");
    assert_eq!(head["bundle"]["content_sha256"], content.as_str(), "{head}");
    let (s, _, got) = bytes(&sam, &format!("{copy}/versions/1/bundle")).await;
    assert_eq!((s, got.as_slice()), (200, &b"adas"[..]));
    let (s, listed) = get(&sam, &format!("{copy}/versions/1/files")).await;
    assert_eq!(s, 200, "{listed}");
    assert!(
        listed["version"]["files"]["index.html"].is_object(),
        "{listed}"
    );
    let (s, _, got) = bytes(&sam, &format!("{copy}/versions/1/files/index.html")).await;
    assert_eq!((s, got.as_slice()), (200, &b"<h1>v1</h1>"[..]));
    // An app of Ada's not shared with Sam: his phone reads none of it, as
    // one that is not there (out of his reach).
    let other = app(&ada, "Ada's own", &[("index.html", "<h1>mine</h1>")]).await;
    let (s, v) = publish(&ada, &other, 1).await;
    assert_eq!(s, 200, "{v}");
    let own = format!("/api/remotes/{sam_rid}/artifacts/{}", raid_of(&v));
    for path in [
        format!("{own}/head"),
        format!("{own}/versions/1/files"),
        format!("{own}/versions/1/bundle"),
        format!("{own}/versions/1/files/index.html"),
    ] {
        let (s, v) = get(&sam, &path).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "{path} {v}");
    }
    // A plain agent token of Sam's still reads it as an agent channel.
    let laptop = TestServer::spawn().await;
    connect(&laptop, &public).await;
    let agent = token_of(&laptop);
    let (s, body) = token_req(
        &public,
        &agent,
        reqwest::Method::GET,
        &format!("/api/artifacts/{raid}/head"),
    )
    .await;
    assert_eq!(s, 200);
    let head: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(head["title"], Value::Null, "{head}");
    assert_eq!(head["content_sha256"], Value::Null, "{head}");
    for path in [
        format!("/api/artifacts/{raid}/versions/1/bundle"),
        format!("/api/artifacts/{raid}/versions/1/files/index.html"),
    ] {
        let (s, _) = token_req(&public, &agent, reqwest::Method::GET, &path).await;
        assert_eq!(s, 403, "{path}");
    }
    // A device that is revoked reads nothing.
    let device = token_of(&sam);
    let (s, _) = token_req(
        &public,
        &device,
        reqwest::Method::DELETE,
        "/api/hosted/agent-token",
    )
    .await;
    assert!((200..300).contains(&s), "{s}");
    let (s, _) = token_req(
        &public,
        &device,
        reqwest::Method::GET,
        &format!("/api/artifacts/{raid}/head"),
    )
    .await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn app_push_names_the_copy_it_makes() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let push = |aid: String, body: Value| {
        let local = &local;
        async move { post(local, &format!("/api/artifacts/{aid}/push"), body).await }
    };
    let aid = app(&local, "Tips", &[("index.html", "<h1>1</h1>")]).await;
    // An empty name, or a name with another mode, is refused.
    let (s, v) = push(
        aid.clone(),
        json!({"mode": "app", "version": 1, "title": " "}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (s, v) = push(aid.clone(), json!({"mode": "new", "title": "X"})).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    assert!(local.store.remote_link(&aid, &rid).unwrap().is_none());
    // The copy it makes takes the name.
    let (s, v) = push(
        aid.clone(),
        json!({"mode": "app", "version": 1, "title": "Tips (yours)"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let id = ArtifactId::parse(&raid).unwrap();
    let there = public.ts.store.get_artifact(&id).unwrap().unwrap();
    assert_eq!(there.title, "Tips (yours)");
    // Linked, it has its copy: a name is refused, and none is changed.
    version(&local, &aid, 1, &[("index.html", Some("<h1>2</h1>"))]).await;
    let (s, v) = push(
        aid.clone(),
        json!({"mode": "app", "version": 2, "title": "Other"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    assert_eq!(current(&public, &raid), 1);
}

/// A public Clax from before app publishing, as its routes answer: its
/// device sign-in's body denies fields it does not know (`client_kind` and
/// `return_to` among them) and approves at once, its publish ignores them (`comments`, `forked_from`) and
/// answers no Comments switch, its sharing routes refuse a token 403
/// `browser_required`, and it has none of the app's routes (404). Every
/// request it takes is logged as `<method> <path>`.
struct OldPublic {
    origin: String,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

/// The public copy [`OldPublic`] makes.
const OLD_COPY: &str = "abcdefghjkmn";

impl OldPublic {
    async fn spawn() -> OldPublic {
        use axum::extract::rejection::JsonRejection;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        use axum::routing::{delete, get, patch, post as on_post};
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct DeviceCodeBody {
            #[serde(default)]
            client_name: String,
        }
        fn refuse(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
            (
                status,
                axum::Json(json!({"error": {"code": code, "message": message}})),
            )
                .into_response()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let o2 = origin.clone();
        let app = axum::Router::new()
            .route(
                "/api/hosted/device/code",
                on_post(
                    move |req: Result<axum::Json<DeviceCodeBody>, JsonRejection>| {
                        let o2 = o2.clone();
                        async move {
                            match req {
                                Err(e) => {
                                    refuse(StatusCode::BAD_REQUEST, "invalid_json", &e.body_text())
                                }
                                Ok(_) => axum::Json(json!({
                                    "device_code": "the-device-code",
                                    "user_code": "ABCD-EFGH",
                                    "verification_uri": format!("{o2}/device"),
                                    "verification_uri_complete": format!("{o2}/device?code=ABCD-EFGH"),
                                    "expires_at": "2099-01-01T00:00:00Z",
                                    "expires_in": 600,
                                    "interval": 5,
                                }))
                                .into_response(),
                            }
                        }
                    },
                ),
            )
            .route(
                "/api/hosted/device/token",
                on_post(|| async {
                    axum::Json(json!({
                        "token": "the-token",
                        "person": {"public_id": "p_ada", "display_name": "Ada"},
                        "grant_id": "g_old",
                    }))
                }),
            )
            .route(
                "/api/sessions",
                on_post(|| async {
                    (
                        StatusCode::CREATED,
                        axum::Json(json!({"session": {"id": "s_old"}, "session_secret": "x"})),
                    )
                }),
            )
            .route(
                "/api/sessions/{sid}",
                patch(|| async { axum::Json(json!({})) }),
            )
            .route(
                "/api/artifacts",
                on_post(|| async {
                    (
                        StatusCode::CREATED,
                        axum::Json(json!({
                            "artifact": {"id": OLD_COPY, "title": "Tips", "current_version": 1},
                            "version": {"n": 1},
                            "url": format!("/a/{OLD_COPY}"),
                        })),
                    )
                }),
            )
            .route(
                "/api/artifacts/{aid}",
                delete(|| async { axum::Json(json!({"deleted": true})) }),
            )
            .route(
                "/api/hosted/artifacts/{aid}/sharing",
                get(|| async {
                    refuse(
                        StatusCode::FORBIDDEN,
                        "browser_required",
                        "sharing is managed in the browser",
                    )
                }),
            )
            .fallback(|| async { refuse(StatusCode::NOT_FOUND, "not_found", "not found") })
            .layer(axum::middleware::from_fn({
                let log = log.clone();
                move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let log = log.clone();
                    async move {
                        log.lock()
                            .unwrap()
                            .push(format!("{} {}", req.method(), req.uri().path()));
                        next.run(req).await
                    }
                }
            }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        OldPublic { origin, log }
    }

    fn took(&self, line: &str) -> bool {
        self.log.lock().unwrap().iter().any(|l| l == line)
    }

    /// How many requests it took that were `line`.
    fn times(&self, line: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| *l == line)
            .count()
    }

    /// Signs `local` in to it as a daemon does (no kind), its token
    /// `the-token`: the remote's ID.
    fn connect(&self, local: &TestServer) -> String {
        use clax_store::remotes::{PendingSignIn, SignedInAs};
        let r = local
            .store
            .add_pending_remote(
                &self.origin,
                "old",
                &PendingSignIn {
                    device_code: clax_core::Secret::new("the-device-code".into()),
                    user_code: "ABCD-EFGH".into(),
                    verification_url: format!("{}/device?code=ABCD-EFGH", self.origin),
                    expires_at: "2099-01-01T00:00:00Z".into(),
                    interval_s: 5,
                },
            )
            .unwrap();
        local
            .store
            .connect_remote(
                &r.id,
                &SignedInAs {
                    token: clax_core::Secret::new("the-token".into()),
                    person_public_id: "p_ada".into(),
                    person_name: Some("Ada".into()),
                    grant_id: "g_old".into(),
                },
            )
            .unwrap();
        r.id
    }
}

#[tokio::test]
async fn app_sign_in_to_a_public_clax_from_before_client_kinds_is_remote_too_old() {
    let old = OldPublic::spawn().await;
    let local = TestServer::spawn().await;
    let (s, v) = post(
        &local,
        "/api/remotes",
        json!({"url": old.origin, "kind": "clax-app", "machine": "Clax on phone"}),
    )
    .await;
    assert_eq!((s, code(&v)), (502, "remote_too_old"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&old.origin),
        "{v}"
    );
    assert!(local.store.remotes().unwrap().is_empty());
    // A daemon's sign-in, which names no kind, still starts there.
    let (s, v) = post(&local, "/api/remotes", json!({"url": old.origin})).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["remote"]["status"], "pending", "{v}");
}

#[tokio::test]
async fn a_public_clax_without_return_to_still_signs_in() {
    let old = OldPublic::spawn().await;
    let local = TestServer::spawn().await;
    let (s, v) = post(
        &local,
        "/api/remotes",
        json!({"url": old.origin, "return_to": "/a/x?publish=1"}),
    )
    .await;
    // Refused the field, the sign-in is asked again without it: the
    // approval page opens in a tab of its own.
    assert_eq!((s, &v["return_to_kept"]), (201, &json!(false)), "{v}");
    assert_eq!(old.times("POST /api/hosted/device/code"), 2);
    let id = v["remote"]["id"].as_str().unwrap();
    let (s, v) = post(&local, &format!("/api/remotes/{id}/poll"), json!({})).await;
    assert_eq!((s, v["status"].as_str()), (200, Some("connected")), "{v}");
}

#[tokio::test]
async fn app_publish_to_a_public_clax_from_before_app_publishing_takes_its_copy_back() {
    let old = OldPublic::spawn().await;
    let local = TestServer::spawn().await;
    let rid = old.connect(&local);
    let aid = app(&local, "Tips", &[("index.html", "<h1>1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!((s, code(&v)), (502, "remote_too_old"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&old.origin),
        "{v}"
    );
    // The copy made with comments on is deleted there, and nothing links
    // to it here.
    assert!(old.took("POST /api/artifacts"));
    assert!(old.took(&format!("DELETE /api/artifacts/{OLD_COPY}")));
    assert!(local.store.remote_link(&aid, &rid).unwrap().is_none());
}

#[tokio::test]
async fn forwarding_to_a_public_clax_from_before_app_publishing_is_remote_too_old() {
    let old = OldPublic::spawn().await;
    let local = TestServer::spawn().await;
    let rid = old.connect(&local);
    let base = format!("/api/remotes/{rid}/artifacts/{OLD_COPY}");
    for path in [
        format!("{base}/head"),
        format!("{base}/versions/1/bundle"),
        format!("{base}/sharing"),
        format!("/api/remotes/{rid}/apps"),
    ] {
        let (s, v) = get(&local, &path).await;
        assert_eq!((s, code(&v)), (502, "remote_too_old"), "{path}: {v}");
    }
    let (s, v) = post(&local, &format!("{base}/comments"), json!({"body": "hi"})).await;
    assert_eq!((s, code(&v)), (502, "remote_too_old"), "{v}");
}

#[tokio::test]
async fn an_install_of_a_restricted_copy_counts_through_the_core() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let aid = app(&local, "Tips", &[("index.html", "<h1>1</h1>")]).await;
    let (s, v) = publish(&local, &aid, 1).await;
    assert_eq!(s, 200, "{v}");
    let raid = raid_of(&v);
    let install = "ab".repeat(32);
    // The copy is Restricted: an install posted with no account is not
    // counted there.
    let res = public
        .ts
        .client
        .post(format!("{}/api/artifacts/{raid}/installs", public.ts.base))
        .header(HOST, format!("localhost:{}", public.ts.addr.port()))
        .json(&json!({"install": install}))
        .send()
        .await
        .unwrap();
    assert!(!res.status().is_success(), "{}", res.status());
    let id = ArtifactId::parse(&raid).unwrap();
    assert_eq!(public.ts.store.install_count(&id).unwrap(), 0);
    // Through the core, with its token, it is; the same device once.
    for _ in 0..2 {
        let res = local
            .authed(local.client.post(format!(
                "{}/api/remotes/{rid}/artifacts/{raid}/installs",
                local.base
            )))
            .json(&json!({"install": install}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
    }
    assert_eq!(public.ts.store.install_count(&id).unwrap(), 1);
}

#[tokio::test]
async fn a_current_public_clax_s_404_still_passes_through() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let rid = connect_app(&local, &public).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = get(
        &local,
        &format!("/api/remotes/{rid}/artifacts/{OLD_COPY}/head"),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

//! An app published from one Clax app home, taken by a second, and kept
//! current, through one public Clax: the owner's device publishes a
//! version and its bundle, a recipient's device links the public copy and
//! fetches the bundle, the owner republishes and the recipient's head
//! names the new bundle, then an editor publishes on the web and the
//! owner's next publish is refused as moved.

use clax_core::ArtifactId;
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, connect_app, json_of, local_req, post};
use reqwest::header::HOST;
use serde_json::{Value, json};

const MIME: &str = "application/vnd.empathic.clax+zip";
const LINEAGE: &str = "l_abcdefghijklmnopqrstuvwxyz";

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

fn utf8(pairs: &[(&str, &str)]) -> Value {
    pairs
        .iter()
        .map(|(k, c)| (k.to_string(), json!({"content": c, "encoding": "utf8"})))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

fn rid_of(local: &TestServer) -> String {
    local.store.remotes().unwrap().remove(0).id
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

async fn get(local: &TestServer, path: &str) -> (u16, Value) {
    local_req(local, reqwest::Method::GET, path, json!(null), None).await
}

/// Publishes version `n + 1` of `aid` in `local` with `pairs`.
async fn version(local: &TestServer, aid: &str, n: u32, pairs: &[(&str, &str)]) {
    let (s, v) = post(
        local,
        &format!("/api/artifacts/{aid}/versions"),
        json!({"if_version": n, "files": utf8(pairs)}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
}

/// The app's publish: local version `n` of `aid` as the next public one.
async fn publish(local: &TestServer, aid: &str, n: u32) -> (u16, Value) {
    post(
        local,
        &format!("/api/artifacts/{aid}/push"),
        json!({"mode": "app", "version": n}),
    )
    .await
}

/// Uploads `bytes` as the bundle of public version `remote_n`, release
/// `bv`, whose files hash to `content`.
async fn put_bundle(
    local: &TestServer,
    aid: &str,
    remote_n: u64,
    bytes: &[u8],
    bv: u64,
    content: &str,
) -> (u16, Value) {
    let rid = rid_of(local);
    json_of(
        local
            .authed(local.client.put(format!(
                "{}/api/artifacts/{aid}/remotes/{rid}/bundle?remote_n={remote_n}",
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

/// Who takes the app on the second home.
#[derive(Clone, Copy, PartialEq)]
enum Recipient {
    /// Ada herself, on her other device: the public copy is hers.
    OwnersOtherDevice,
    /// Bob, to whom Ada shares it at Can view.
    Viewer,
}

/// A POST to `path` on `public` by a hosted agent of the person its
/// browser is, as the web's agents publish: a token the browser mints,
/// in a session of its own.
async fn web_publish(public: &Public, path: &str, body: Value) -> (u16, Value) {
    let (s, v) = public
        .browser(
            reqwest::Method::POST,
            "/api/hosted/agent-tokens",
            json!({"label": "on the web"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let token = v["token"].as_str().unwrap().to_string();
    let host = format!("localhost:{}", public.ts.addr.port());
    let (s, sess) = json_of(
        public
            .ts
            .client
            .post(format!("{}/api/sessions", public.ts.base))
            .header(HOST, &host)
            .bearer_auth(&token)
            .json(&json!({"harness": "claude", "cwd": ""}))
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
            .post(format!("{}{path}", public.ts.base))
            .header(HOST, &host)
            .bearer_auth(&token)
            .header("x-clax-session", sess["session"]["id"].as_str().unwrap())
            .header(
                clax_core::remote::SESSION_SECRET_HEADER,
                sess["session_secret"].as_str().unwrap(),
            )
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn publish_pull_republish_update_then_web_edit_moves() {
    scenario(Recipient::OwnersOtherDevice).await;
}

#[tokio::test]
async fn a_viewer_pulls_and_is_offered_the_republished_bundle() {
    scenario(Recipient::Viewer).await;
}

/// The six steps: A publishes with a bundle, B links and fetches it, A
/// republishes, B's head names the new bundle, an editor publishes on the
/// web, and A's next publish is refused as moved.
async fn scenario(recipient: Recipient) {
    let mut public = Public::new().await;
    // A: Ada's phone, signed in as her device.
    let a = TestServer::spawn().await;
    connect_app(&a, &public).await;
    let a_rid = rid_of(&a);
    // Ada's browser: only the owner's browser invites people.
    let ada = public.cookie.clone();
    // B: the second home, signed in as Ada's other device or as Bob's.
    let bob = match recipient {
        Recipient::OwnersOtherDevice => None,
        Recipient::Viewer => Some(public.become_person("Bob").await),
    };
    let b = TestServer::spawn().await;
    connect_app(&b, &public).await;
    let b_rid = rid_of(&b);

    // 1. A publishes version 2 of its app, and its bundle beside it.
    let aid = a.publish("Tips", &[("index.html", "<h1>draft</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    version(&a, &aid, 1, &[("index.html", "<h1>one</h1>")]).await;
    let (s, v) = publish(&a, &aid, 2).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 2, "remote_n": 1}]), "{v}");
    let raid = v["link"]["remote_artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let url = v["url"].as_str().unwrap().to_string();
    let one = content_hash(&a, &aid, 2);
    let bundle_one = b"bundle one".to_vec();
    let (s, v) = put_bundle(&a, &aid, 1, &bundle_one, 1, &one).await;
    assert_eq!(s, 201, "{v}");
    // Shared with Bob to view.
    if let Some(bob) = &bob {
        let (s, v) = local_req(
            &a,
            reqwest::Method::PUT,
            &format!("/api/remotes/{a_rid}/artifacts/{raid}/sharing/shares"),
            json!({"person": bob, "access": "viewer"}),
            None,
        )
        .await;
        assert_eq!(s, 200, "{v}");
    }

    // 2. B finds it in its library, reads the head, fetches the bundle
    // byte for byte, installs its files and links them at that version.
    let (s, apps) = get(&b, &format!("/api/remotes/{b_rid}/apps")).await;
    assert_eq!(s, 200, "{apps}");
    let listed: Vec<&Value> = apps
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["artifact_id"] == raid.as_str())
        .collect();
    assert_eq!(listed.len(), 1, "{apps}");
    let access = match recipient {
        Recipient::OwnersOtherDevice => "owner",
        Recipient::Viewer => "viewer",
    };
    assert_eq!(listed[0]["access"], access, "{apps}");
    let copy = format!("/api/remotes/{b_rid}/artifacts/{raid}");
    let (s, head) = get(&b, &format!("{copy}/head")).await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["version"], 1, "{head}");
    assert_eq!(head["title"], "Tips", "{head}");
    assert_eq!(head["content_sha256"], one.as_str(), "{head}");
    assert_eq!(head["bundle"]["version"], 1, "{head}");
    assert_eq!(head["bundle"]["bundle_version"], 1, "{head}");
    assert_eq!(head["bundle"]["content_sha256"], one.as_str(), "{head}");
    let (s, ct, got) = bytes(&b, &format!("{copy}/versions/1/bundle")).await;
    assert_eq!((s, ct.as_str()), (200, MIME));
    assert_eq!(got, bundle_one);
    let mine = b.publish("Tips", &[("index.html", "<h1>one</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The bundle's files as installed here hash as the head's do.
    assert_eq!(content_hash(&b, &mine, 1), one);
    let (s, v) = post(
        &b,
        &format!("/api/remotes/{b_rid}/links"),
        json!({"artifact_id": mine, "remote_artifact_id": raid, "url": url, "remote_n": 1}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["link"]["remote_n"], 1, "{v}");

    // 3. A publishes again: version 3 as public version 2, with its bundle.
    version(&a, &aid, 2, &[("index.html", "<h1>two</h1>")]).await;
    let (s, v) = publish(&a, &aid, 3).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pushed"], json!([{"n": 3, "remote_n": 2}]), "{v}");
    let two = content_hash(&a, &aid, 3);
    let bundle_two = b"bundle two".to_vec();
    let (s, v) = put_bundle(&a, &aid, 2, &bundle_two, 2, &two).await;
    assert_eq!(s, 201, "{v}");

    // 4. B's head names the new bundle, which B fetches byte for byte.
    let (s, head) = get(&b, &format!("{copy}/head")).await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["version"], 2, "{head}");
    assert_eq!(head["content_sha256"], two.as_str(), "{head}");
    assert_eq!(head["bundle"]["version"], 2, "{head}");
    assert_eq!(head["bundle"]["bundle_version"], 2, "{head}");
    assert_eq!(head["bundle"]["content_sha256"], two.as_str(), "{head}");
    assert_eq!(head["bundle"]["published_by"]["name"], "Ada", "{head}");
    // The publisher's public ID is the one A's remote names as its own.
    let (_, remotes) = get(&a, "/api/remotes").await;
    let ada_id = remotes["remotes"][0]["person_public_id"].clone();
    assert!(ada_id.is_string(), "{remotes}");
    assert_eq!(
        head["bundle"]["published_by"]["public_id"], ada_id,
        "{head}"
    );
    let (s, _, got) = bytes(&b, &format!("{copy}/versions/2/bundle")).await;
    assert_eq!(s, 200);
    assert_eq!(got, bundle_two);

    // 5. Cy, who may edit it, publishes on the web through her hosted
    // agent: a version with no bundle.
    public.cookie = ada;
    let cy = public.become_person("Cy").await;
    let (s, v) = local_req(
        &a,
        reqwest::Method::PUT,
        &format!("/api/remotes/{a_rid}/artifacts/{raid}/sharing/shares"),
        json!({"person": cy, "access": "editor"}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = web_publish(
        &public,
        &format!("/api/artifacts/{raid}/versions"),
        json!({"if_version": 2, "files": utf8(&[("index.html", "<h1>web</h1>")])}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    // The head is Cy's; the newest bundle is still A's version 2, so B is
    // offered nothing new.
    let (s, head) = get(&b, &format!("{copy}/head")).await;
    assert_eq!(s, 200, "{head}");
    assert_eq!(head["version"], 3, "{head}");
    assert_eq!(head["published_by"]["name"], "Cy", "{head}");
    assert_eq!(head["published_by"]["public_id"], json!(cy), "{head}");
    assert_ne!(head["published_by"]["public_id"], ada_id, "{head}");
    assert_eq!(head["bundle"]["version"], 2, "{head}");
    assert_eq!(head["bundle"]["bundle_version"], 2, "{head}");

    // 6. A's next app publish, still based on public version 2, is moved.
    version(&a, &aid, 3, &[("index.html", "<h1>three</h1>")]).await;
    let (s, v) = publish(&a, &aid, 4).await;
    assert_eq!((s, code(&v)), (409, "remote_moved"), "{v}");
    assert_eq!(v["error"]["remote_version"], 3, "{v}");
    assert_eq!(v["error"]["pushed_version"], 2, "{v}");
    let id = ArtifactId::parse(&raid).unwrap();
    let art = public.ts.store.get_artifact(&id).unwrap().unwrap();
    assert_eq!(art.current_version, 3);
    assert_eq!(texts(&public.ts, &raid, 3)[0].1, "<h1>web</h1>");
    let link = a.store.remote_link(&aid, &a_rid).unwrap().unwrap();
    assert_eq!((link.pushed_n, link.remote_n), (3, 2));
}

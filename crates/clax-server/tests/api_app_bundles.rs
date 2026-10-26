//! Apps on a hosted Clax: the bundle a version carries (`PUT` and `GET
//! /api/artifacts/<id>/versions/<n>/bundle`), the head route, installs, the
//! Comments switch and its enforcement, the apps library, and the artifact
//! JSON's `comments`, `forks` and `app`.

use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";
const MIME: &str = "application/vnd.empathic.clax+zip";
const LINEAGE: &str = "l_abcdefghijklmnopqrstuvwxyz";

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A hosted daemon on a stopped clock.
struct Site {
    ts: TestServer,
    origin: String,
}

/// One browser: the session cookie it holds, if any.
#[derive(Clone, Default)]
struct Browser {
    cookie: Option<String>,
}

fn cookie_of(res: &reqwest::Response) -> Option<String> {
    res.headers().get(SET_COOKIE).and_then(|v| {
        v.to_str()
            .unwrap()
            .strip_prefix("clax_session=")
            .map(|c| c.split(';').next().unwrap().to_string())
    })
}

impl Site {
    async fn new() -> Site {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let ts = TestServer::spawn_hosted(clock, Some(SECRET)).await;
        let origin = format!("http://localhost:{}", ts.addr.port());
        Site { ts, origin }
    }

    fn host(&self) -> String {
        format!("localhost:{}", self.ts.addr.port())
    }

    /// A request from `b`, as the shell at the public origin makes it.
    fn req(&self, b: &Browser, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .ts
            .client
            .request(method.clone(), format!("{}{path}", self.ts.base))
            .header(HOST, self.host());
        if method != reqwest::Method::GET {
            r = r.header(ORIGIN, &self.origin);
        }
        if let Some(c) = &b.cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        r
    }

    async fn get(&self, b: &Browser, path: &str) -> (u16, Value) {
        json_of(
            self.req(b, reqwest::Method::GET, path)
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    async fn send(
        &self,
        b: &Browser,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        json_of(self.req(b, method, path).json(&body).send().await.unwrap()).await
    }

    async fn post(&self, b: &Browser, path: &str, body: Value) -> (u16, Value) {
        self.send(b, reqwest::Method::POST, path, body).await
    }

    async fn put(&self, b: &Browser, path: &str, body: Value) -> (u16, Value) {
        self.send(b, reqwest::Method::PUT, path, body).await
    }

    async fn sign_in(&self, b: &mut Browser, path: &str, body: Value) {
        let res = self
            .req(b, reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap();
        let cookie = cookie_of(&res);
        let (status, v) = json_of(res).await;
        assert_eq!(status, 200, "{path}: {v}");
        b.cookie = Some(cookie.expect("a session cookie"));
    }

    /// The owner (Ada), signed in through setup.
    async fn owner(&self) -> Browser {
        let mut b = Browser::default();
        self.sign_in(
            &mut b,
            "/api/hosted/setup",
            json!({"secret": SECRET, "name": "Ada"}),
        )
        .await;
        b
    }

    /// A person invited by `owner` with the default rights (their own
    /// space alone), joined in a new browser.
    async fn member(&self, owner: &Browser, name: &str) -> Browser {
        let (s, v) = self
            .post(owner, "/api/people/invites", json!({"name": name}))
            .await;
        assert_eq!(s, 201, "{v}");
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let mut b = Browser::default();
        self.sign_in(
            &mut b,
            &format!("/api/hosted/invites/{code}/accept"),
            json!({}),
        )
        .await;
        b
    }

    /// A guest named `name`, signed in at a page shared by link.
    async fn guest(&self, name: &str) -> Browser {
        let res = self
            .req(&Browser::default(), reqwest::Method::PUT, "/api/viewers/me")
            .json(&json!({"display_name": name}))
            .send()
            .await
            .unwrap();
        let cookie = cookie_of(&res);
        let (status, v) = json_of(res).await;
        assert_eq!(status, 200, "{v}");
        Browser { cookie }
    }

    async fn pid(&self, b: &Browser) -> String {
        let (_, v) = self.get(b, "/api/hosted/me").await;
        v["person"]["public_id"].as_str().unwrap().to_string()
    }

    /// An artifact the owner's agent published, titled `title`.
    async fn artifact(&self, title: &str) -> String {
        self.ts
            .publish(title, &[("index.html", "<h1>x</h1>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Publishes version `n + 1` of `aid`, at version `n`, through the
    /// owner's agent.
    async fn republish(&self, aid: &str, n: u32, html: &str) {
        let res = self
            .ts
            .post_json(
                &format!("/api/artifacts/{aid}/versions"),
                json!({"if_version": n, "files": {"index.html": {"content": html, "encoding": "utf8"}}}),
            )
            .await;
        assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    }

    /// Version `n`'s `content_sha256`, as the owner reads it.
    async fn content(&self, owner: &Browser, aid: &str, n: u32) -> String {
        let (s, v) = self
            .get(owner, &format!("/api/artifacts/{aid}/versions/{n}"))
            .await;
        assert_eq!(s, 200, "{v}");
        v["version"]["content_sha256"].as_str().unwrap().to_string()
    }

    async fn share(&self, owner: &Browser, aid: &str, person: &str, access: &str) {
        let (s, v) = self
            .put(
                owner,
                &format!("/api/hosted/artifacts/{aid}/shares"),
                json!({"person": person, "access": access}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    async fn link(&self, owner: &Browser, aid: &str, access: Option<&str>) {
        let (s, v) = self
            .put(
                owner,
                &format!("/api/hosted/artifacts/{aid}/link"),
                json!({"access": access}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    /// `b` puts `bytes` as version `n`'s bundle, of release `bv`, holding
    /// the files whose hash is `content`.
    async fn put_bundle(
        &self,
        b: &Browser,
        aid: &str,
        n: u32,
        bytes: Vec<u8>,
        bv: u64,
        content: &str,
    ) -> (u16, Value) {
        json_of(
            self.req(
                b,
                reqwest::Method::PUT,
                &format!("/api/artifacts/{aid}/versions/{n}/bundle"),
            )
            .header("content-type", MIME)
            .header("clax-bundle-lineage", LINEAGE)
            .header("clax-bundle-version", bv.to_string())
            .header("clax-bundle-content-sha256", content)
            .body(bytes)
            .send()
            .await
            .unwrap(),
        )
        .await
    }

    /// A thread `b` starts on `aid`, as the shell does.
    async fn thread(&self, b: &Browser, aid: &str, body: &str) -> (u16, Value) {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        json_of(
            self.req(
                b,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/threads"),
            )
            .multipart(form)
            .send()
            .await
            .unwrap(),
        )
        .await
    }

    /// `POST /api/artifacts/<aid>/installs` as the Clax app sends it: no
    /// `Origin`, and whatever cookie `b` holds.
    async fn install(&self, b: &Browser, aid: &str, install: &str) -> (u16, Value) {
        let mut r = self
            .ts
            .client
            .post(format!("{}/api/artifacts/{aid}/installs", self.ts.base))
            .header(HOST, self.host())
            .json(&json!({"install": install}));
        if let Some(c) = &b.cookie {
            r = r.header(COOKIE, format!("clax_session={c}"));
        }
        json_of(r.send().await.unwrap()).await
    }
}

/// An agent token `b`'s person minted, with a session it registered.
struct PersonAgent {
    token: String,
    sid: String,
    secret: String,
}

impl Site {
    /// An agent token of `b`'s person, labelled `label`, and a session of it.
    async fn person_agent(&self, b: &Browser, label: &str) -> PersonAgent {
        let (s, v) = self
            .post(b, "/api/hosted/agent-tokens", json!({"label": label}))
            .await;
        assert_eq!(s, 201, "{v}");
        let token = v["token"].as_str().unwrap().to_string();
        let (s, v) = json_of(
            self.ts
                .client
                .post(format!("{}/api/sessions", self.ts.base))
                .header(HOST, self.host())
                .bearer_auth(&token)
                .json(&json!({"harness": "claude", "cwd": ""}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(s, 201, "{v}");
        PersonAgent {
            token,
            sid: v["session"]["id"].as_str().unwrap().to_string(),
            secret: v["session_secret"].as_str().unwrap().to_string(),
        }
    }

    /// A request from agent `a`, from its session.
    fn agent_req(
        &self,
        a: &PersonAgent,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        self.ts
            .client
            .request(method, format!("{}{path}", self.ts.base))
            .header(HOST, self.host())
            .bearer_auth(&a.token)
            .header("x-clax-session", &a.sid)
            .header(clax_core::remote::SESSION_SECRET_HEADER, &a.secret)
    }

    async fn agent_get(&self, a: &PersonAgent, path: &str) -> (u16, Value) {
        json_of(
            self.agent_req(a, reqwest::Method::GET, path)
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    /// `b` approves `author`'s content reaching their agents, everywhere.
    async fn approve(&self, b: &Browser, author: &str) {
        let (s, v) = self
            .post(
                b,
                "/api/approvals",
                json!({"author": author, "scope": "everywhere"}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
}

fn install_id(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

#[tokio::test]
async fn bundle_put_get_roundtrip_and_head() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Tip Calculator!").await;
    let content = site.content(&owner, &aid, 1).await;
    let bytes = b"PK\x03\x04 a bundle the remote never reads".to_vec();
    let (s, v) = site
        .put_bundle(&owner, &aid, 1, bytes.clone(), 3, &content)
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (
            &v["artifact_id"],
            &v["version"],
            &v["bundle_version"],
            &v["size"]
        ),
        (&json!(aid), &json!(1), &json!(3), &json!(bytes.len()))
    );
    assert_eq!(
        v["sha256"],
        json!(format!("sha256:{}", clax_core::audit::sha256_hex(&bytes)))
    );
    // The same bytes again (a retry whose answer was lost) land once.
    let (s, again) = site
        .put_bundle(&owner, &aid, 1, bytes.clone(), 3, &content)
        .await;
    assert_eq!((s, &again), (201, &v));
    // A version that does not exist.
    let (s, v) = site
        .put_bundle(&owner, &aid, 9, bytes.clone(), 4, &content)
        .await;
    assert_eq!(s, 404, "{v}");
    // The bytes come back as they went up.
    let res = site
        .req(
            &owner,
            reqwest::Method::GET,
            &format!("/api/artifacts/{aid}/versions/1/bundle"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], MIME);
    assert_eq!(
        res.headers()["content-disposition"],
        "attachment; filename=\"tip-calculator.clax\""
    );
    assert_eq!(res.bytes().await.unwrap().to_vec(), bytes);
    // A version published from the web carries none.
    site.republish(&aid, 1, "<h1>web</h1>").await;
    let (s, v) = site
        .get(&owner, &format!("/api/artifacts/{aid}/versions/2/bundle"))
        .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    // The head is the newest version; its bundle the newest that has one.
    let (s, h) = site
        .get(&owner, &format!("/api/artifacts/{aid}/head"))
        .await;
    assert_eq!(s, 200, "{h}");
    let content2 = site.content(&owner, &aid, 2).await;
    assert_eq!(
        (
            &h["artifact_id"],
            &h["title"],
            &h["version"],
            &h["content_sha256"]
        ),
        (
            &json!(aid),
            &json!("Tip Calculator!"),
            &json!(2),
            &json!(content2)
        )
    );
    assert_eq!(h["url"], json!(format!("{}/a/{aid}", site.origin)));
    assert_eq!(h["comments"], json!(true));
    assert!(h["capabilities"].is_object(), "{h}");
    // A version published without a note has none.
    assert_eq!(h["note"], Value::Null, "{h}");
    // The owner's card carries her public ID, as her devices' remotes do.
    let ada_id = h["owner"]["public_id"].clone();
    assert!(ada_id.is_string(), "{h}");
    let ada = json!({"name": "Ada", "login": null, "public_id": ada_id});
    assert_eq!(h["owner"], ada);
    assert_eq!(h["published_by"], ada);
    assert_eq!(
        h["bundle"],
        json!({
            "version": 1,
            "lineage": LINEAGE,
            "bundle_version": 3,
            "content_sha256": content,
            "url": format!("{}/api/artifacts/{aid}/versions/1/bundle", site.origin),
            "published_by": ada,
        })
    );
    // The head carries its version's change note.
    let res = site
        .ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({
                "if_version": 2,
                "note": "Bigger buttons",
                "files": {"index.html": {"content": "<h1>v3</h1>", "encoding": "utf8"}},
            }),
        )
        .await;
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    let (s, h) = site
        .get(&owner, &format!("/api/artifacts/{aid}/head"))
        .await;
    assert_eq!(
        (s, &h["version"], &h["note"]),
        (200, &json!(3), &json!("Bigger buttons")),
        "{h}"
    );
    // An artifact without bundles has a head with none.
    let plain = site.artifact("Plain").await;
    let (s, h) = site
        .get(&owner, &format!("/api/artifacts/{plain}/head"))
        .await;
    assert_eq!((s, &h["bundle"]), (200, &Value::Null), "{h}");
}

#[tokio::test]
async fn bundle_put_refuses_hash_mismatch() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let other = format!("sha256:{}", "0".repeat(64));
    let (s, v) = site
        .put_bundle(&owner, &aid, 1, b"bundle".to_vec(), 1, &other)
        .await;
    assert_eq!((s, code(&v)), (400, "bundle_mismatch"), "{v}");
    let (s, _) = site
        .get(&owner, &format!("/api/artifacts/{aid}/versions/1/bundle"))
        .await;
    assert_eq!(s, 404, "a refused bundle is not kept");
    let content = site.content(&owner, &aid, 1).await;
    // Missing or malformed headers, and another content type.
    let path = format!("/api/artifacts/{aid}/versions/1/bundle");
    let (s, v) = json_of(
        site.req(&owner, reqwest::Method::PUT, &path)
            .header("content-type", MIME)
            .header("clax-bundle-version", "1")
            .header("clax-bundle-content-sha256", &content)
            .body(b"bundle".to_vec())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "no lineage: {v}");
    for bad in ["lin_0123456789abcdef", "l_abcdefghijklmnopqrstuvwxy1"] {
        let (s, v) = json_of(
            site.req(&owner, reqwest::Method::PUT, &path)
                .header("content-type", MIME)
                .header("clax-bundle-lineage", bad)
                .header("clax-bundle-version", "1")
                .header("clax-bundle-content-sha256", &content)
                .body(b"bundle".to_vec())
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!((s, code(&v)), (400, "invalid_args"), "lineage {bad}: {v}");
    }
    let (s, v) = json_of(
        site.req(&owner, reqwest::Method::PUT, &path)
            .header("content-type", MIME)
            .header("clax-bundle-lineage", LINEAGE)
            .header("clax-bundle-version", "one")
            .header("clax-bundle-content-sha256", &content)
            .body(b"bundle".to_vec())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "a bad version: {v}");
    let (s, v) = json_of(
        site.req(&owner, reqwest::Method::PUT, &path)
            .header("content-type", "application/zip")
            .header("clax-bundle-lineage", LINEAGE)
            .header("clax-bundle-version", "1")
            .header("clax-bundle-content-sha256", &content)
            .body(b"bundle".to_vec())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "another type: {v}");
    // One byte over the cap.
    let big = vec![0u8; clax_store::app_bundles::MAX_BUNDLE_BYTES as usize + 1];
    let (s, v) = site.put_bundle(&owner, &aid, 1, big, 1, &content).await;
    assert_eq!((s, code(&v)), (413, "bundle_too_large"), "{v}");
    // A later bundle of another lineage, or of no higher release.
    let (s, v) = site
        .put_bundle(&owner, &aid, 1, b"bundle".to_vec(), 5, &content)
        .await;
    assert_eq!(s, 201, "{v}");
    site.republish(&aid, 1, "<h1>2</h1>").await;
    let content2 = site.content(&owner, &aid, 2).await;
    let (s, v) = site
        .put_bundle(&owner, &aid, 2, b"bundle 2".to_vec(), 5, &content2)
        .await;
    assert_eq!((s, code(&v)), (400, "bundle_version_order"), "{v}");
}

#[tokio::test]
async fn bundle_put_editor_allowed_viewer_refused() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    let kim = site.member(&owner, "Kim").await;
    site.share(&owner, &aid, &site.pid(&sam).await, "editor")
        .await;
    site.share(&owner, &aid, &site.pid(&kim).await, "viewer")
        .await;
    let content = site.content(&owner, &aid, 1).await;
    let (s, v) = site
        .put_bundle(&kim, &aid, 1, b"kim's".to_vec(), 1, &content)
        .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "a viewer: {v}");
    let (s, v) = site
        .put_bundle(
            &Browser::default(),
            &aid,
            1,
            b"anyone's".to_vec(),
            1,
            &content,
        )
        .await;
    assert_eq!(s, 401, "no session: {v}");
    let (s, v) = site
        .put_bundle(&sam, &aid, 1, b"sam's".to_vec(), 1, &content)
        .await;
    assert_eq!(s, 201, "an editor: {v}");
    // The viewer reads it, and the head names its publisher.
    let (s, _) = site
        .get(&kim, &format!("/api/artifacts/{aid}/versions/1/bundle"))
        .await;
    assert_eq!(s, 200);
    let (s, h) = site.get(&kim, &format!("/api/artifacts/{aid}/head")).await;
    assert_eq!(s, 200, "{h}");
    let sam_card = &h["bundle"]["published_by"];
    assert_eq!(
        (&sam_card["name"], &sam_card["login"]),
        (&json!("Sam"), &Value::Null),
        "{h}"
    );
    // The editor's public ID tells them apart from anyone else named Sam.
    assert!(sam_card["public_id"].is_string(), "{h}");
    assert_eq!(
        (&h["owner"]["name"], &h["owner"]["login"]),
        (&json!("Ada"), &Value::Null),
        "{h}"
    );
    assert!(h["owner"]["public_id"].is_string(), "{h}");
    assert_ne!(h["owner"]["public_id"], sam_card["public_id"], "{h}");
}

#[tokio::test]
async fn a_person_agent_is_refused_a_bundle_whose_uploader_or_files_it_has_not_approved() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    let kim = site.member(&owner, "Kim").await;
    let sam_pid = site.pid(&sam).await;
    site.share(&owner, &aid, &sam_pid, "editor").await;
    site.share(&owner, &aid, &site.pid(&kim).await, "viewer")
        .await;
    // Kim's agents see the owner's content, not Sam's.
    site.approve(&kim, &site.pid(&owner).await).await;
    let kims = site.person_agent(&kim, "kim-laptop").await;
    let head = format!("/api/artifacts/{aid}/head");
    let bundle = |n: u32| format!("/api/artifacts/{aid}/versions/{n}/bundle");
    // The owner's version 1, carrying the bundle Sam uploaded.
    let content1 = site.content(&owner, &aid, 1).await;
    let (s, v) = site
        .put_bundle(&sam, &aid, 1, b"sam's".to_vec(), 1, &content1)
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = site.agent_get(&kims, &bundle(1)).await;
    assert_eq!((s, code(&v)), (403, "withheld"), "Sam's bytes: {v}");
    let (s, _) = site.get(&kim, &bundle(1)).await;
    assert_eq!(s, 200, "Kim's own browser reads it");
    let (s, h) = site.agent_get(&kims, &head).await;
    assert_eq!(s, 200, "{h}");
    assert_eq!(h["content_sha256"], json!(content1), "{h}");
    assert_eq!(
        (&h["bundle"]["version"], &h["bundle"]["content_sha256"]),
        (&json!(1), &Value::Null),
        "{h}"
    );
    let (_, h) = site.get(&kim, &head).await;
    assert_eq!(h["bundle"]["content_sha256"], json!(content1), "{h}");
    // Sam's version 2, carrying the bundle the owner uploaded.
    let sams = site.person_agent(&sam, "sam-laptop").await;
    let (s, v) = json_of(
        site.agent_req(
            &sams,
            reqwest::Method::POST,
            &format!("/api/artifacts/{aid}/versions"),
        )
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<h1>Sam</h1>", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let content2 = site.content(&owner, &aid, 2).await;
    let (s, v) = site
        .put_bundle(&owner, &aid, 2, b"ada's".to_vec(), 2, &content2)
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = site.agent_get(&kims, &bundle(2)).await;
    assert_eq!((s, code(&v)), (403, "withheld"), "Sam's files: {v}");
    let (s, h) = site.agent_get(&kims, &head).await;
    assert_eq!(s, 200, "{h}");
    assert_eq!(
        (
            &h["content_sha256"],
            &h["bundle"]["version"],
            &h["bundle"]["content_sha256"]
        ),
        (&Value::Null, &json!(2), &Value::Null),
        "{h}"
    );
    // Once Kim approves Sam, both reach Kim's agents.
    site.approve(&kim, &sam_pid).await;
    for n in [1, 2] {
        let (s, _) = site.agent_get(&kims, &bundle(n)).await;
        assert_eq!(s, 200, "version {n}");
    }
    let (_, h) = site.agent_get(&kims, &head).await;
    assert_eq!(h["bundle"]["content_sha256"], json!(content2), "{h}");
}

#[tokio::test]
async fn a_keyed_resend_answers_what_it_made_after_comments_go_off() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &site.pid(&sam).await, "commenter")
        .await;
    let (s, t) = site.thread(&owner, &aid, "before").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let sams = site.person_agent(&sam, "sam-laptop").await;
    let reply = |key: &'static str| {
        let req = site
            .agent_req(
                &sams,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/threads/{tid}/comments"),
            )
            .header("idempotency-key", key)
            .json(&json!({"body": "once"}));
        async move { json_of(req.send().await.unwrap()).await }
    };
    let start = |key: &'static str| {
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", "a thread")
            .text("version", "1");
        let req = site
            .agent_req(
                &sams,
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/threads"),
            )
            .header("idempotency-key", key)
            .multipart(form);
        async move { json_of(req.send().await.unwrap()).await }
    };
    let (s, first) = reply("k-reply").await;
    assert_eq!(s, 201, "{first}");
    let (s, made) = start("k-thread").await;
    assert_eq!(s, 201, "{made}");
    let (s, v) = site
        .put(
            &owner,
            &format!("/api/hosted/artifacts/{aid}/comments"),
            json!({"on": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A resend answers what its key made.
    let (s, again) = reply("k-reply").await;
    assert_eq!(s, 201, "{again}");
    assert_eq!(again["comment"]["id"], first["comment"]["id"], "{again}");
    let (s, again) = start("k-thread").await;
    assert_eq!(s, 201, "{again}");
    assert_eq!(again["thread"]["id"], made["thread"]["id"], "{again}");
    // A new key writes nothing.
    let (s, v) = reply("k-reply-2").await;
    assert_eq!((s, code(&v)), (403, "comments_off"), "{v}");
    let (s, v) = start("k-thread-2").await;
    assert_eq!((s, code(&v)), (403, "comments_off"), "{v}");
    let (_, v) = site
        .get(&owner, &format!("/api/artifacts/{aid}/threads"))
        .await;
    let threads = v["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2, "{v}");
    let comments: usize = threads
        .iter()
        .map(|t| t["comments"].as_array().unwrap().len())
        .sum();
    assert_eq!(comments, 3, "{v}");
}

#[tokio::test]
async fn head_link_guest_reads_restricted_guest_404() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let content = site.content(&owner, &aid, 1).await;
    let (s, _) = site
        .put_bundle(&owner, &aid, 1, b"bundle".to_vec(), 1, &content)
        .await;
    assert_eq!(s, 201);
    let anyone = Browser::default();
    let head = format!("/api/artifacts/{aid}/head");
    let bundle = format!("/api/artifacts/{aid}/versions/1/bundle");
    // Restricted: not there.
    for p in [&head, &bundle] {
        let (s, v) = site.get(&anyone, p).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "{p}: {v}");
    }
    let (s, v) = site.install(&anyone, &aid, &install_id('a')).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    // Anyone with the link.
    site.link(&owner, &aid, Some("viewer")).await;
    let (s, h) = site.get(&anyone, &head).await;
    assert_eq!((s, &h["version"]), (200, &json!(1)), "{h}");
    let (s, _) = site.get(&anyone, &bundle).await;
    assert_eq!(s, 200);
    let guest = site.guest("Gus").await;
    let (s, h) = site.get(&guest, &head).await;
    assert_eq!((s, &h["bundle"]["version"]), (200, &json!(1)), "{h}");
    // Restricted again.
    site.link(&owner, &aid, None).await;
    let (s, _) = site.get(&guest, &head).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn comments_off_refuses_owner_commenter_guest_and_reply() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &site.pid(&sam).await, "commenter")
        .await;
    site.link(&owner, &aid, Some("commenter")).await;
    let guest = site.guest("Gus").await;
    let (s, t) = site.thread(&owner, &aid, "before").await;
    assert_eq!(s, 201, "{t}");
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let switch = format!("/api/hosted/artifacts/{aid}/comments");
    // Only who may share it turns it off.
    let (s, v) = site.put(&sam, &switch, json!({"on": false})).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = site.put(&guest, &switch, json!({"on": false})).await;
    assert_eq!(s, 403, "{v}");
    // An agent's token is not a browser (yet).
    let (s, v) = json_of(
        site.ts
            .authed(site.ts.client.put(format!("{}{switch}", site.ts.base)))
            .json(&json!({"on": false}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "browser_required"), "{v}");
    let (s, v) = site.put(&owner, &switch, json!({"on": false})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v, json!({"artifact_id": aid, "comments": false}));
    let (_, sharing) = site
        .get(&owner, &format!("/api/hosted/artifacts/{aid}/sharing"))
        .await;
    assert_eq!(sharing["comments"], json!(false), "{sharing}");
    // Nobody comments: the owner, a commenter, a guest, a reply, an agent.
    for (who, b) in [("owner", &owner), ("commenter", &sam), ("guest", &guest)] {
        let (s, v) = site.thread(b, &aid, "after").await;
        assert_eq!((s, code(&v)), (403, "comments_off"), "{who}'s thread: {v}");
        let (s, v) = site
            .post(
                b,
                &format!("/api/artifacts/{aid}/threads/{tid}/comments"),
                json!({"body": "a reply"}),
            )
            .await;
        assert_eq!((s, code(&v)), (403, "comments_off"), "{who}'s reply: {v}");
    }
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_testkit::element_anchor().to_string())
        .text("body", "mirrored up")
        .text("version", "1");
    let (s, v) = json_of(
        site.ts
            .authed(
                site.ts
                    .client
                    .post(format!("{}/api/artifacts/{aid}/threads", site.ts.base)),
            )
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (s, code(&v)),
        (403, "comments_off"),
        "the owner's token: {v}"
    );
    let (s, v) = json_of(
        site.ts
            .authed(site.ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                site.ts.base
            )))
            .json(&json!({"body": "a reply"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (s, code(&v)),
        (403, "comments_off"),
        "the owner's token: {v}"
    );
    // What was said stays readable.
    let (s, v) = site
        .get(&sam, &format!("/api/artifacts/{aid}/threads"))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["threads"].as_array().unwrap().len(), 1, "{v}");
    // On again.
    let (s, v) = site.put(&owner, &switch, json!({"on": true})).await;
    assert_eq!((s, &v["comments"]), (200, &json!(true)), "{v}");
    let (s, v) = site.thread(&guest, &aid, "again").await;
    assert_eq!(s, 201, "{v}");
}

/// Who may resolve the artifact's threads (and so send them while Comments
/// is off) is said to the caller; a Comments switch change is announced to
/// the artifact's open views, off and on; a refused change announces
/// nothing.
#[tokio::test]
async fn may_resolve_is_said_and_a_comments_switch_change_announced() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &site.pid(&sam).await, "commenter")
        .await;
    let switch = format!("/api/hosted/artifacts/{aid}/comments");
    let mut rx = site.ts.events.subscribe();
    let switches = |rx: &mut tokio::sync::broadcast::Receiver<clax_core::Stamped>| {
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let clax_core::Event::Comments {
                artifact_id,
                comments,
            } = e.event
            {
                got.push((artifact_id, comments));
            }
        }
        got
    };
    let (s, v) = site.put(&sam, &switch, json!({"on": false})).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(switches(&mut rx), []);
    // Who may resolve its threads, and so send while it is off.
    for (who, b, may) in [("owner", &owner, true), ("commenter", &sam, false)] {
        let (s, v) = site.get(b, &format!("/api/artifacts/{aid}")).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["artifact"]["may_resolve"], json!(may), "{who}: {v}");
    }
    let (s, v) = site.put(&owner, &switch, json!({"on": false})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(switches(&mut rx), [(aid.clone(), false)]);
    let (s, v) = site.put(&owner, &switch, json!({"on": true})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(switches(&mut rx), [(aid.clone(), true)]);
}

/// Who may edit the artifact is said to the caller: the owner and a person
/// it is shared with to edit, not one it is shared with to comment.
#[tokio::test]
async fn may_edit_is_said_to_owner_and_editors_only() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &site.pid(&sam).await, "editor")
        .await;
    let kim = site.member(&owner, "Kim").await;
    site.share(&owner, &aid, &site.pid(&kim).await, "commenter")
        .await;
    for (who, b, may) in [
        ("owner", &owner, true),
        ("editor", &sam, true),
        ("commenter", &kim, false),
    ] {
        let (s, v) = site.get(b, &format!("/api/artifacts/{aid}")).await;
        assert_eq!(s, 200, "{who}: {v}");
        assert_eq!(v["artifact"]["may_edit"], json!(may), "{who}: {v}");
    }
}

/// An artifact shared with Sam at Can comment and by link at Can comment,
/// with a guest, four open threads of the guest's, and its Comments switch
/// off: the site, the owner, Sam, the guest, the artifact, the threads.
async fn commented_with_comments_off() -> (Site, Browser, Browser, Browser, String, Vec<String>) {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let sam = site.member(&owner, "Sam").await;
    site.share(&owner, &aid, &site.pid(&sam).await, "commenter")
        .await;
    site.link(&owner, &aid, Some("commenter")).await;
    let guest = site.guest("Gus").await;
    let mut tids = Vec::new();
    for n in 0..4 {
        let (s, t) = site.thread(&guest, &aid, &format!("thread {n}")).await;
        assert_eq!(s, 201, "{t}");
        tids.push(t["thread"]["id"].as_str().unwrap().to_string());
    }
    let (s, v) = site
        .put(
            &owner,
            &format!("/api/hosted/artifacts/{aid}/comments"),
            json!({"on": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    (site, owner, sam, guest, aid, tids)
}

/// While Comments is off, Can comment acts as Can view (spec
/// 2026-10-21-app-push-pull D4a), which sends nothing: a commenter's send
/// and batch send are refused `comments_off`, until Comments is on again.
#[tokio::test]
async fn comments_off_refuses_a_commenters_send() {
    let (site, owner, sam, guest, aid, tids) = commented_with_comments_off().await;
    let send = |tid: &str| format!("/api/artifacts/{aid}/threads/{tid}/send");
    let batch = format!("/api/artifacts/{aid}/threads:send");
    for (who, b) in [("guest", &guest), ("commenter", &sam)] {
        let (s, v) = site.post(b, &send(&tids[0]), json!({})).await;
        assert_eq!((s, code(&v)), (403, "comments_off"), "{who}'s send: {v}");
        let (s, v) = site.post(b, &batch, json!({"thread_ids": [tids[1]]})).await;
        assert_eq!((s, code(&v)), (403, "comments_off"), "{who}'s batch: {v}");
    }
    let (s, v) = site
        .put(
            &owner,
            &format!("/api/hosted/artifacts/{aid}/comments"),
            json!({"on": true}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = site.post(&guest, &send(&tids[2]), json!({})).await;
    assert_eq!(s, 200, "the guest's send: {v}");
    let (s, v) = site
        .post(&guest, &batch, json!({"thread_ids": [tids[3]]}))
        .await;
    assert_eq!(s, 200, "the guest's batch: {v}");
}

/// While Comments is off the owner, who resolves, still sends and batch
/// sends.
#[tokio::test]
async fn comments_off_keeps_the_owners_send() {
    let (site, owner, _sam, _guest, aid, tids) = commented_with_comments_off().await;
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/artifacts/{aid}/threads/{}/send", tids[0]),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "the owner's send: {v}");
    let (s, v) = site
        .post(
            &owner,
            &format!("/api/artifacts/{aid}/threads:send"),
            json!({"thread_ids": [tids[1]]}),
        )
        .await;
    assert_eq!(s, 200, "the owner's batch: {v}");
    // The owner's token too.
    let (s, v) = json_of(
        site.ts
            .authed(site.ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{}/send",
                site.ts.base, tids[2]
            )))
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "the owner's token's send: {v}");
}

#[tokio::test]
async fn create_with_comments_false() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let file = json!({"index.html": {"content": "<p>x</p>", "encoding": "utf8"}});
    let res = site
        .ts
        .post_json(
            "/api/artifacts",
            json!({"title": "App", "files": file, "comments": false}),
        )
        .await;
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["artifact"]["comments"], json!(false), "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let (_, v) = site.get(&owner, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(v["artifact"]["comments"], json!(false), "{v}");
    let (s, v) = site.thread(&owner, &aid, "hello").await;
    assert_eq!((s, code(&v)), (403, "comments_off"), "{v}");
    // By default, on.
    let plain = site.artifact("Plain").await;
    let (_, v) = site.get(&owner, &format!("/api/artifacts/{plain}")).await;
    assert_eq!(v["artifact"]["comments"], json!(true), "{v}");
    // A new version does not set it.
    let res = site
        .ts
        .post_json(
            &format!("/api/artifacts/{plain}/versions"),
            json!({"if_version": 1, "files": file, "comments": false}),
        )
        .await;
    let (s, v) = json_of(res).await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
}

#[tokio::test]
async fn a_local_daemon_takes_no_comments_switch_on_create() {
    // A local daemon has no route to turn the switch back on.
    let ts = TestServer::spawn().await;
    let file = json!({"index.html": {"content": "<p>x</p>", "encoding": "utf8"}});
    for on in [false, true] {
        let res = ts
            .post_json(
                "/api/artifacts",
                json!({"title": "App", "files": file, "comments": on}),
            )
            .await;
        let (s, v) = json_of(res).await;
        assert_eq!((s, code(&v)), (400, "invalid_args"), "{on}: {v}");
    }
    assert!(ts.store.list_artifacts().unwrap().is_empty());
    let res = ts
        .post_json("/api/artifacts", json!({"title": "Page", "files": file}))
        .await;
    let (s, v) = json_of(res).await;
    assert_eq!((s, &v["artifact"]["comments"]), (201, &json!(true)), "{v}");
}

#[tokio::test]
async fn installs_count_distinct() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let content = site.content(&owner, &aid, 1).await;
    site.put_bundle(&owner, &aid, 1, b"bundle".to_vec(), 1, &content)
        .await;
    site.link(&owner, &aid, Some("viewer")).await;
    let anyone = Browser::default();
    for c in ['a', 'a', 'b'] {
        let (s, v) = site.install(&anyone, &aid, &install_id(c)).await;
        assert_eq!(s, 204, "{v}");
    }
    let (s, v) = site.install(&anyone, &aid, "not hex").await;
    assert_eq!((s, code(&v)), (400, "invalid_args"), "{v}");
    let (_, v) = site.get(&owner, &format!("/api/artifacts/{aid}")).await;
    assert_eq!(v["artifact"]["app"]["installs"], json!(2), "{v}");
    // A cookie without `Origin` counts as no one: it opens nothing
    // restricted.
    let other = site.artifact("Other").await;
    let (s, _) = site.install(&owner, &other, &install_id('c')).await;
    assert_eq!(s, 404, "a cross-site post does not speak for the cookie");
    // Per client address, on the clock.
    let mut limited = None;
    for i in 0..40 {
        let (s, v) = site
            .install(
                &anyone,
                &aid,
                &install_id(char::from(b'0' + (i % 10) as u8)),
            )
            .await;
        if s == 429 {
            limited = Some(v);
            break;
        }
        assert_eq!(s, 204, "{v}");
    }
    let v = limited.expect("installs are rate limited");
    assert_eq!(code(&v), "rate_limited", "{v}");
}

#[tokio::test]
async fn apps_library_lists_owned_and_shared_with_access() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let sam = site.member(&owner, "Sam").await;
    let sam_pid = site.pid(&sam).await;
    let mut ids = Vec::new();
    for t in ["Edit me", "View me", "No bundle"] {
        ids.push(site.artifact(t).await);
    }
    let (edit, view, plain) = (&ids[0], &ids[1], &ids[2]);
    for aid in [edit, view] {
        let content = site.content(&owner, aid, 1).await;
        let (s, v) = site
            .put_bundle(&owner, aid, 1, b"bundle".to_vec(), 2, &content)
            .await;
        assert_eq!(s, 201, "{v}");
    }
    site.share(&owner, edit, &sam_pid, "editor").await;
    site.share(&owner, view, &sam_pid, "viewer").await;
    site.share(&owner, plain, &sam_pid, "editor").await;
    site.link(&owner, view, Some("viewer")).await;
    // Sam's own app, started in their browser.
    let (s, v) = site
        .post(
            &sam,
            "/api/artifacts:new",
            json!({"title": "Mine", "brief": "an app"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mine = v["artifact"]["id"].as_str().unwrap().to_string();
    let content = site.content(&sam, &mine, 1).await;
    let (s, v) = site
        .put_bundle(&sam, &mine, 1, b"mine".to_vec(), 1, &content)
        .await;
    assert_eq!(s, 201, "{v}");
    // Kim, with an app of their own, one shared with them alone, and one
    // anyone with its link opens but no one is given.
    let kim = site.member(&owner, "Kim").await;
    let kim_pid = site.pid(&kim).await;
    let (s, v) = site
        .post(
            &kim,
            "/api/artifacts:new",
            json!({"title": "Kim's own", "brief": "an app"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let kims_own = v["artifact"]["id"].as_str().unwrap().to_string();
    let kims_only = site.artifact("For Kim").await;
    let linked = site.artifact("Linked").await;
    for (b, aid) in [(&kim, &kims_own), (&owner, &kims_only), (&owner, &linked)] {
        let content = site.content(b, aid, 1).await;
        let (s, v) = site
            .put_bundle(b, aid, 1, b"bundle".to_vec(), 1, &content)
            .await;
        assert_eq!(s, 201, "{v}");
    }
    site.share(&owner, &kims_only, &kim_pid, "viewer").await;
    site.link(&owner, &linked, Some("viewer")).await;
    let by_id = |v: &Value| -> std::collections::BTreeMap<String, Value> {
        v.as_array()
            .unwrap_or_else(|| panic!("a list: {v}"))
            .iter()
            .map(|a| (a["artifact_id"].as_str().unwrap().to_string(), a.clone()))
            .collect()
    };
    let (s, v) = site.get(&sam, "/api/hosted/apps").await;
    assert_eq!(s, 200, "{v}");
    let apps = by_id(&v);
    assert_eq!(apps.len(), 3, "{v}");
    assert_eq!(
        apps[edit],
        json!({
            "artifact_id": edit, "title": "Edit me", "url": format!("{}/a/{edit}", site.origin),
            "version": 1, "bundle_version": 2, "access": "editor", "general": "restricted",
        })
    );
    assert_eq!(
        (&apps[view]["access"], &apps[view]["general"]),
        (&json!("viewer"), &json!("link"))
    );
    assert_eq!(apps[&mine]["access"], json!("owner"));
    // Each member sees their own apps and those shared with them, never
    // another's.
    let keys = |v: &Value| by_id(v).keys().cloned().collect::<Vec<_>>();
    let sorted = |mut k: Vec<String>| {
        k.sort();
        k
    };
    let (s, v) = site.get(&kim, "/api/hosted/apps").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        keys(&v),
        sorted(vec![kims_own.clone(), kims_only.clone()]),
        "Kim's apps: {v}"
    );
    let apps = by_id(&v);
    assert_eq!(
        (&apps[&kims_own]["access"], &apps[&kims_only]["access"]),
        (&json!("owner"), &json!("viewer")),
        "{v}"
    );
    let (_, v) = site.get(&sam, "/api/hosted/apps").await;
    assert_eq!(
        keys(&v),
        sorted(vec![edit.clone(), view.clone(), mine.clone()]),
        "Sam's apps: {v}"
    );
    let (s, v) = site.get(&owner, "/api/hosted/apps").await;
    assert_eq!(s, 200, "{v}");
    let apps = by_id(&v);
    assert_eq!(
        keys(&v),
        sorted(vec![
            edit.clone(),
            view.clone(),
            kims_only.clone(),
            linked.clone()
        ]),
        "the owner's own apps: {v}"
    );
    assert!(apps.values().all(|a| a["access"] == json!("owner")), "{v}");
    let (s, _) = site.get(&Browser::default(), "/api/hosted/apps").await;
    assert_eq!(s, 401);
    let (s, v) = json_of(
        site.ts
            .authed(
                site.ts
                    .client
                    .get(format!("{}/api/hosted/apps", site.ts.base)),
            )
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "browser_required"), "{v}");
}

#[tokio::test]
async fn artifact_json_carries_app_comments_forks() {
    let site = Site::new().await;
    let owner = site.owner().await;
    let aid = site.artifact("Plan").await;
    let get = format!("/api/artifacts/{aid}");
    let (_, v) = site.get(&owner, &get).await;
    let a = &v["artifact"];
    assert_eq!(
        (&a["comments"], &a["forks"], &a["app"]),
        (&json!(true), &json!(0), &Value::Null),
        "{v}"
    );
    let content = site.content(&owner, &aid, 1).await;
    site.put_bundle(&owner, &aid, 1, b"bundle".to_vec(), 7, &content)
        .await;
    let (s, v) = site
        .post(&owner, &format!("/api/artifacts/{aid}/fork"), json!({}))
        .await;
    assert_eq!(s, 201, "{v}");
    let (_, v) = site.get(&owner, &get).await;
    let a = &v["artifact"];
    assert_eq!(a["forks"], json!(1), "{v}");
    let ada = json!({"name": "Ada", "login": null, "public_id": a["app"]["owner"]["public_id"]});
    assert!(ada["public_id"].is_string(), "{v}");
    let app = json!({
        "version": 1,
        "bundle_version": 7,
        "installs": 0,
        "published_by": ada,
        "owner": ada,
    });
    assert_eq!(a["app"], app, "{v}");
    // The shell's bootstrap says the same.
    let html = site
        .req(&owner, reqwest::Method::GET, &format!("/a/{aid}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    const OPEN: &str = r#"<script type="application/json" id="clax-boot">"#;
    let start = html.find(OPEN).expect("a bootstrap block") + OPEN.len();
    let end = start + html[start..].find("</script>").unwrap();
    let boot: Value = serde_json::from_str(&html[start..end]).unwrap();
    let a = &boot["artifact"]["artifact"];
    assert_eq!(
        (&a["comments"], &a["forks"], &a["app"]),
        (&json!(true), &json!(1), &app),
        "{boot}"
    );
}

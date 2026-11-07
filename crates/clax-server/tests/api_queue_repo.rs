//! A queue's link to a repository and the instance's GitHub App
//! (`routes/queue_repo.rs`): who may read and change them, in every role a
//! caller can hold; that credentials are write-only; that a queue admin
//! cannot aim the instance's App anywhere.

use clax_store::queue_repos::{Egress, RepoCredentials, TokenKind};
use clax_store::secrets::Kek;
use clax_testkit::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

const READ: &str = "github_pat_READ_CANARY_0123-read";
const WRITE: &str = "github_pat_WRITE_CANARY_0123-rite";
/// Made for these tests and nothing else.
const PEM: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEpQIBAAKCAQEAq3WZuIsnf9SqGWJ0K7IqcSusFxMW7HwcLwI4TxYI+OR3viPN
NHvwLqP0h6ZPV5vyVdrIeFd3Ml5XqP88/C2jRwXEsr5PfNwT9qQG3eiSAs7i1o15
Vc5MuuslZ+D3KmIcJY3kk+q7ok6SpmFC90fZ6uur2UT0if5E2nBiSZBmZB++xs6X
euZbRZfZ8qVe/TdugHXzJrdI+Vtd5rpnSWSGxJWvR/kGxULBi8YydGV516vUwT+m
FBbVLabRxnnH/Hz8Meqw4UoLQYM4dGU6g4CHZbCUoR3aNAh6qUpJicWxi6Y/+AN4
7a1B2oLFswkxltImL7wi8MhYKow+S7schSPEWwIDAQABAoIBAQCXeASc6xGx9AVw
93D5SSQFZjdZCxwtPlcHkEZ+V1EFz4+ZAbe+rd641U6op1tgYfvrrKN87If4jy+R
tCoD/a1IiOgDn+xIuYNJWA5Y66RRpsLuF30hOGbAmgspgavYXWPQKVHM+olYEWGs
ai/mlCsHRbtC7Y+6eE4LS00I/+/0ckTD2dpVkgQ5BUPvQJjO3NpireeDqx2nfVRx
40i7zuAtMHjjzdrlsqqlVSX+gayAd0NDaxK+YwrbyoUFEZfh5WS/SazlQa5ZDRZE
vbh6Iel9vb1HyWTPnrNwus3rpSZkAwcLiwe8W47qnMSKmZSJGHwcjTCjroH53qc1
OZokWC3BAoGBANHU8YKwNiiYjhcg1PxGdk6blnTku3NFPunbRxdWl/2VmlhwoD1L
gE7rLrEeQVBRZ2ngyrQ7HDZoWGVIRR9Pvu+cBOG1Z/MWZbZP82qvf+ZIwdp2lP3B
KiUQ5Z51cESVSwOq2yIS3JIAJJ52ATGxB8mKb4BkPY0bTkqOfjvEVx11AoGBANEv
R1ESOu0MApf2Q6sgWv63Kzf/nGP3IZm4pEIM8SFO4tVszns2j2biGt5jg5ZcMPIH
Ms3LlQVuOkrjxRp9/YQFpoHQTojSYfWqPvTmhxq6xhHDNrPw9eE31KHajgPBblLp
W+ZVXukPe5+VMOU/1HStdwZw0BnMxZsTTHH4txCPAoGAVLyr81z54hcJeqmVmXNT
H5eRgQ9F690Q9RvKb9BWfN/HfuOsslQlMrmjF3/iXG8jVgyjj3k3gXlOCUjQMT7h
Gp5lEklR+0cWODO9V14cRoEnV/P/YyJ4xJM4R2+hsPChZI/BAtefH47TOvLotIqL
XIZ9VNomSS+3xEAbFkPJ8mkCgYEAiyo49+Z47VsgHpjnS+Gqp+qUBh03ngm7gNkl
7gtOICqrHfHVLIOPw5KI7Yqm8FFE/r9rPR5DdelvPoNpjcZj8oEP1S0QElZkdVtd
pu/20gGP5CFLM0sLSVorS8Jq2HTHQMGfMg15pvN1+yT18o4UecstdEt5JrDkhdbA
9NHSeGECgYEAjebvk4v0qfKVs45pZevWLu90hzGS/aEwBe1cbO/Znt5EmDpDZVmG
Vuvd4h/iBTBvDwyTcLrKgCN91rrbw5DibOpHlikiXHHGNkH7splQz04F/mipJwFC
2Jm3PRCKsIoa1HDqL+9xNHSNXC20i9FpmTz60kTG/Do6GP7xC6bPoDo=
-----END RSA PRIVATE KEY-----";
/// A line of [`PEM`], which nothing but the sealed row may hold.
const KEY_LINE: &str = "NHvwLqP0h6ZPV5vyVdrIeFd3Ml5XqP88";

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

async fn browser(
    ts: &TestServer,
    cookie: &str,
    m: Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = ts
        .client
        .request(m, format!("{}{path}", ts.base))
        .header("cookie", cookie);
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// A daemon listening beyond loopback, Sam connected, a queue of the
/// owner's, and the owner's and Sam's cookies.
struct World {
    ts: TestServer,
    /// The App's forge, a local stand-in for GitHub.
    forge: String,
    owner: String,
    sam: String,
    sam_public: String,
    sam_client: clax_testkit::TestClient,
    qid: String,
}

/// The one call a queue's link makes of a forge: `acme/web` has the App
/// installed on the account `acme-inc`, as a request the App signed is told.
async fn installation(req: axum::extract::Request) -> axum::response::Response {
    use axum::response::IntoResponse;
    let signed = req
        .headers()
        .get("authorization")
        .and_then(|a| a.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
        .is_some_and(|jwt| jwt.split('.').count() == 3);
    if !signed {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    if req.uri().path() != "/repos/acme/web/installation" {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    axum::Json(json!({"id": 42, "account": {"login": "acme-inc"}})).into_response()
}

async fn forge() -> String {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(installation);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn world() -> World {
    world_with(|_| {}).await
}

async fn world_with(f: impl FnOnce(&mut clax_http::state::AppState)) -> World {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), f).await;
    let sam_client = ts.connect_person("Sam", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam_client.grant_id()).unwrap();
    let sam = format!("clax_viewer={}", ts.browser_cookie(&v.id));
    let owner = ts.owner_cookie();
    let (s, q) = browser(
        &ts,
        &owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Bugs", "origins": ["https://app.example.com"]})),
    )
    .await;
    assert_eq!(s, 201, "{q}");
    World {
        qid: q["queue"]["id"].as_str().unwrap().to_string(),
        forge: forge().await,
        owner,
        sam,
        sam_public: v.public_id,
        sam_client,
        ts,
    }
}

impl World {
    fn repo(&self) -> String {
        format!("/api/queues/{}/repo", self.qid)
    }

    async fn set_member(&self, role: Option<&str>) {
        let path = format!("/api/queues/{}/members/{}", self.qid, self.sam_public);
        let (m, body) = match role {
            Some(r) => (Method::PUT, Some(json!({"role": r}))),
            None => (Method::DELETE, None),
        };
        let (s, v) = browser(&self.ts, &self.owner, m, &path, body).await;
        assert_eq!(s, 204, "{v}");
    }

    async fn set_app(&self) {
        let (s, v) = browser(
            &self.ts,
            &self.owner,
            Method::PUT,
            "/api/admin/repos/app",
            Some(json!({
                "app_id": 7, "slug": "clax-bot", "api_url": self.forge, "private_key": PEM,
            })),
        )
        .await;
        assert_eq!(s, 200, "{v}");
        assert!(!v.to_string().contains(KEY_LINE), "{v}");
    }

    fn install_url(&self) -> String {
        format!("{}/apps/clax-bot/installations/new", self.forge)
    }

    /// Every audit event's kind and body, in one text.
    fn trail(&self) -> String {
        self.ts
            .store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .map(|e| format!("{} {}", e.kind, e.body))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn link() -> Value {
    json!({
        "repo": "acme/web",
        "preview_url": "https://pr-{pr}.acme.dev",
        "access": {"kind": "token"},
        "ci_accepted": true,
    })
}

fn app_link() -> Value {
    json!({
        "repo": "acme/web",
        "preview_url": "https://pr-{pr}.acme.dev",
        "access": {"kind": "github_app"},
        "ci_accepted": true,
    })
}

/// What each route answers `cookie`, in the routes' order.
async fn every_queue_route(w: &World, cookie: &str) -> Vec<(u16, String)> {
    let repo = w.repo();
    let token = format!("{repo}/tokens/write");
    let mut out = Vec::new();
    for (m, p, b) in [
        (Method::GET, &repo, None),
        (Method::PUT, &repo, Some(link())),
        (Method::DELETE, &repo, Some(json!({"if_rev": "any"}))),
        (
            Method::PUT,
            &token,
            Some(json!({"token": WRITE, "if_rev": "any"})),
        ),
        (Method::DELETE, &token, None),
    ] {
        let (s, v) = browser(&w.ts, cookie, m, p, b).await;
        out.push((s, code(&v).to_string()));
    }
    out
}

#[tokio::test]
async fn a_queue_admin_connects_a_repository_and_its_tokens_are_write_only() {
    let w = world().await;
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"], Value::Null);
    assert_eq!(v["app"]["configured"], false);
    assert_eq!(v["runtime"]["configured"], false);

    w.set_member(Some("admin")).await;
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "needs_token");
    assert_eq!(v["repo"]["repo_url"], "https://github.com/acme/web.git");
    assert_eq!(v["repo"]["forge_api_url"], "https://api.github.com");
    assert_eq!(v["repo"]["access"]["write_token"]["set"], false);
    let rev = v["repo"]["rev"].as_str().unwrap().to_string();

    for (which, token) in [("read", READ), ("write", WRITE)] {
        let (s, v) = browser(
            &w.ts,
            &w.sam,
            Method::PUT,
            &format!("{}/tokens/{which}", w.repo()),
            Some(json!({"token": token, "if_rev": rev})),
        )
        .await;
        assert_eq!(s, 200, "{v}");
        assert!(!v.to_string().contains("CANARY"), "{v}");
    }
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "ready");
    assert_eq!(v["repo"]["access"]["write_token"]["last4"], "rite");
    assert_eq!(v["repo"]["access"]["read_token"]["last4"], "read");
    assert_eq!(v["repo"]["set_by"]["id"], w.sam_public);
    assert!(!v.to_string().contains("CANARY"), "{v}");

    let trail = w.trail();
    assert!(!trail.contains("CANARY"), "{trail}");
    assert!(trail.contains("\"op\":\"repo\""), "{trail}");
    assert!(trail.contains("\"scope\":\"repo_write\""), "{trail}");
    assert!(trail.contains("\"page\":\"queue_repo\""), "{trail}");

    // Identical values are refused, whichever comes second.
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &format!("{}/tokens/read", w.repo()),
        Some(json!({"token": WRITE, "if_rev": rev})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "same_token"), "{v}");
    assert!(!v.to_string().contains("CANARY"), "{v}");

    // The workspace alone reads them back.
    let kek = Kek::from_bytes([7; 32]);
    let t =
        w.ts.store
            .open_queue_repo(&kek, &w.qid, Egress::Held)
            .unwrap()
            .unwrap();
    let RepoCredentials::Token { read, write } = t.credentials else {
        panic!("tokens");
    };
    assert_eq!(read.unwrap().expose(), READ);
    assert_eq!(write.expose(), WRITE);

    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::DELETE,
        &format!("{}/tokens/read", w.repo()),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["access"]["read_token"]["set"], false);
    assert_eq!(v["repo"]["status"], "needs_read_token");

    let (s, _) = disconnect(&w, &w.sam).await;
    assert_eq!(s, 204);
    assert!(w.ts.store.queue_repo(&w.qid).unwrap().is_none());
}

#[tokio::test]
async fn every_role_short_of_queue_admin_gets_nothing() {
    let w = world().await;
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!(s, 200, "{v}");

    // Not a member: the queue does not exist for them.
    let none = vec![(404, "not_a_member".to_string()); 5];
    assert_eq!(every_queue_route(&w, &w.sam).await, none);

    // Triage reads reports, not the repository.
    w.set_member(Some("triage")).await;
    let triage = vec![(403, "not_queue_admin".to_string()); 5];
    assert_eq!(every_queue_route(&w, &w.sam).await, triage);

    // An admin demoted is refused at once.
    w.set_member(Some("admin")).await;
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
    assert_eq!(s, 200, "{v}");
    w.set_member(Some("triage")).await;
    assert_eq!(every_queue_route(&w, &w.sam).await, triage);

    // Removed, likewise.
    w.set_member(Some("admin")).await;
    w.set_member(None).await;
    assert_eq!(every_queue_route(&w, &w.sam).await, none);

    // Revoked from the instance, an admin of the queue has no way in.
    w.set_member(Some("admin")).await;
    w.ts.store
        .revoke_person(&clax_core::audit::AuditCtx::DAEMON, &w.sam_public)
        .unwrap();
    for (s, c) in every_queue_route(&w, &w.sam).await {
        assert!(matches!(s, 401 | 403 | 404), "{s} {c}");
        assert_ne!(s, 200);
    }

    // No credential at all.
    let (s, _) = json_of(
        w.ts.client
            .get(format!("{}{}", w.ts.base, w.repo()))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 401);

    // A viewer who is no person.
    let cookie = clax_core::secret::viewer_cookie();
    let v =
        w.ts.store
            .upsert_viewer(cookie.expose(), Some("Guest"))
            .unwrap();
    let guest = format!("clax_viewer={}", w.ts.browser_cookie(&v.id));
    let (s, v) = browser(&w.ts, &guest, Method::GET, &w.repo(), None).await;
    assert_eq!((s, code(&v)), (403, "not_a_person"), "{v}");
}

#[tokio::test]
async fn agents_service_grants_and_reporters_get_nothing() {
    let w = world().await;
    w.set_member(Some("admin")).await;
    let repo = w.repo();
    // A person's agent, though its person is an admin of the queue: the
    // grant gate turns it away.
    for (m, b) in [("GET", None), ("PUT", Some(link())), ("DELETE", None)] {
        let (s, v) = json_of(w.sam_client.agent(m, &repo, None, b).await).await;
        assert_eq!((s, code(&v)), (403, "forbidden"), "{m}: {v}");
    }
    // The owner's agent.
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.get(format!("{}{repo}", w.ts.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");

    // A service grant's token and a reporter's are not credentials here.
    let (s, g) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        &format!("/api/queues/{}/service-grants", w.qid),
        Some(json!({"name": "backend"})),
    )
    .await;
    assert_eq!(s, 201, "{g}");
    let token = g["token"].as_str().unwrap();
    let grant =
        w.ts.store
            .service_grant_by_token(&clax_core::secret::sha256_hex(token))
            .unwrap()
            .unwrap();
    let key = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    let (reporter, _) =
        w.ts.store
            .mint_reporter_token(&grant, "r1", "https://app.example.com", &key, None)
            .unwrap();
    let reporter = reporter.expose();
    for auth in [
        format!("Bearer {token}"),
        format!("Clax-Queue {token}"),
        format!("Bearer {reporter}"),
        format!("Clax-Reporter {reporter}"),
    ] {
        let (s, v) = json_of(
            w.ts.client
                .get(format!("{}{repo}", w.ts.base))
                .header("authorization", auth)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(s, 401, "{v}");
    }
}

#[tokio::test]
async fn only_the_owner_sets_the_app_and_approves_what_it_reaches() {
    let w = world().await;
    let app = "/api/admin/repos";
    let set = "/api/admin/repos/app";
    let body = json!({"app_id": 7, "slug": "clax-bot", "private_key": PEM});
    // A connected person, and a person the owner made an instance admin.
    for promote in [false, true] {
        if promote {
            w.ts.store
                .set_admin(&clax_core::audit::AuditCtx::DAEMON, &w.sam_public, true)
                .unwrap();
        }
        let want = if promote { "owner_only" } else { "forbidden" };
        let (s, v) = browser(&w.ts, &w.sam, Method::GET, app, None).await;
        assert_eq!((s, code(&v)), (403, want), "{v}");
        let (s, v) = browser(&w.ts, &w.sam, Method::PUT, set, Some(body.clone())).await;
        assert_eq!((s, code(&v)), (403, want), "{v}");
        let (s, v) = browser(&w.ts, &w.sam, Method::DELETE, set, None).await;
        assert_eq!((s, code(&v)), (403, want), "{v}");
    }
    let (s, v) = json_of(w.sam_client.agent("GET", app, None, None).await).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.get(format!("{}{app}", w.ts.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");

    w.set_app().await;
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, app, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["app"]["app_id"], 7);
    assert_eq!(v["app"]["install_url"], w.install_url());
    assert_eq!(v["app"]["key"]["set"], true);
    assert_eq!(v["kek"]["set"], true);
    assert!(!v.to_string().contains("CANARY"), "{v}");
    assert!(!w.trail().contains("CANARY"));
    assert!(!w.trail().contains(KEY_LINE));

    // Sam, an admin of the queue, names a repository: nothing reaches it
    // until the owner says so.
    w.set_member(Some("admin")).await;
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(app_link())).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "needs_approval");
    assert_eq!(v["app"]["configured"], true);
    assert_eq!(v["app"]["install_url"], w.install_url());
    assert_eq!(v["repo"]["access"]["installation_id"], 42);
    assert_eq!(v["repo"]["access"]["account"], "acme-inc");

    let (_, v) = browser(&w.ts, &w.owner, Method::GET, app, None).await;
    assert_eq!(v["links"][0]["queue_id"], w.qid);
    assert_eq!(v["links"][0]["approved"], false);
    assert_eq!(
        (&v["links"][0]["installation_id"], &v["links"][0]["account"]),
        (&json!(42), &json!("acme-inc")),
        "the owner approves by the account the forge named"
    );

    let approve = format!("{app}/links/{}", w.qid);
    let yes = json!({"rev": v["links"][0]["rev"], "approved": true});
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &approve, Some(yes.clone())).await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let stale = json!({"rev": "an-older-rev", "approved": true});
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &approve, Some(stale)).await;
    assert_eq!((s, code(&v)), (400, "link_changed"), "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &approve, Some(yes)).await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["status"], "ready");
    assert_eq!(v["repo"]["access"]["approved"], true);
    assert!(w.trail().contains("\"op\":\"repo_approval\""));

    // The owner's own link needs no second step.
    let (s, v) = disconnect(&w, &w.owner).await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(app_link())).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "ready");

    let (s, _) = browser(&w.ts, &w.owner, Method::DELETE, set, None).await;
    assert_eq!(s, 204);
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["status"], "needs_app");
    assert_eq!(v["app"]["configured"], false);
}

#[tokio::test]
async fn a_queue_admin_cannot_aim_the_instances_app_or_a_token_elsewhere() {
    let w = world().await;
    w.set_app().await;
    w.set_member(Some("admin")).await;

    for field in ["forge_api_url", "repo_url"] {
        let mut b = app_link();
        b[field] = json!("https://collector.example.com/x");
        let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(b)).await;
        assert_eq!((s, code(&v)), (400, "forge_not_instance"), "{v}");
    }

    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!(s, 200, "{v}");
    let rev = v["repo"]["rev"].as_str().unwrap().to_string();
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &format!("{}/tokens/write", w.repo()),
        Some(json!({"token": WRITE, "if_rev": rev})),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // Sam cannot read the owner's token, so may not send it anywhere.
    let mut b = link();
    b["if_rev"] = json!(rev);
    b["repo_url"] = json!("https://collector.example.com/acme/web.git");
    b["forge_api_url"] = json!("https://collector.example.com/api");
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "needs_token");
    assert_eq!(v["repo"]["access"]["write_token"]["set"], false);
    assert!(
        w.ts.store
            .open_queue_repo(&Kek::from_bytes([7; 32]), &w.qid, Egress::Held)
            .unwrap()
            .is_none()
    );

    // The host's own network is the owner's alone.
    for (field, code_) in [
        ("repo_url", "invalid_repo_url"),
        ("forge_api_url", "invalid_forge_url"),
    ] {
        for bad in [
            "http://127.0.0.1:7480/x",
            "https://169.254.169.254/x",
            "https://x:y@github.com/a/b.git",
            "--upload-pack=x",
        ] {
            let mut b = link();
            b[field] = json!(bad);
            if field == "repo_url" {
                b["forge_api_url"] = json!("https://git.acme.dev/api");
            }
            b["if_rev"] = json!(rev_of(&w, &w.sam).await);
            let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(b)).await;
            assert_eq!((s, code(&v)), (400, code_), "{bad}: {v}");
            assert!(!v.to_string().contains("x:y"), "{v}");
        }
    }
}

#[tokio::test]
async fn a_repository_the_app_is_not_installed_on_says_where_to_install_it() {
    let w = world().await;
    w.set_app().await;
    let mut b = app_link();
    b["repo"] = json!("acme/other");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!((s, code(&v)), (400, "app_not_installed"), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains(&w.install_url()), "{message}");
    assert!(message.contains("acme/other"), "{message}");
    assert_eq!(w.ts.store.queue_repo(&w.qid).unwrap(), None);
}

#[tokio::test]
async fn an_installation_is_asked_of_the_forge_and_never_taken_from_a_person() {
    let w = world().await;
    w.set_app().await;
    let mut b = app_link();
    b["access"]["installation_id"] = json!(99);
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert!((400..500).contains(&s), "{s}: {v}");
    assert_eq!(w.ts.store.queue_repo(&w.qid).unwrap(), None);
}

#[tokio::test]
async fn a_repository_that_is_no_repository_never_reaches_the_forge() {
    let w = world().await;
    w.set_app().await;
    let mut b = app_link();
    b["repo"] = json!("../../app/installations");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!((s, code(&v)), (400, "invalid_repo"), "{v}");
}

#[tokio::test]
async fn a_forge_that_gives_no_answer_is_a_bad_gateway_and_no_link() {
    let w = world().await;
    let body = json!({
        "app_id": 7, "slug": "clax-bot", "api_url": "http://127.0.0.1:1", "private_key": PEM,
    });
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        "/api/admin/repos/app",
        Some(body),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(app_link())).await;
    assert_eq!((s, code(&v)), (502, "forge_error"), "{v}");
    assert!(!v.to_string().contains(KEY_LINE), "{v}");
}

#[tokio::test]
async fn an_app_key_that_cannot_sign_is_said_and_not_quoted() {
    let w = world().await;
    let body = json!({
        "app_id": 7, "slug": "clax-bot", "api_url": w.forge,
        "private_key": "-----BEGIN RSA PRIVATE KEY-----\nMIIE_KEY_CANARY\n-----END RSA PRIVATE KEY-----\n",
    });
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        "/api/admin/repos/app",
        Some(body),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(app_link())).await;
    assert_eq!((s, code(&v)), (400, "invalid_app_key"), "{v}");
    assert!(!v.to_string().contains("CANARY"), "{v}");
}

#[tokio::test]
async fn with_no_app_there_is_nothing_to_ask() {
    let w = world().await;
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(app_link())).await;
    assert_eq!((s, code(&v)), (400, "no_github_app"), "{v}");
}

#[tokio::test]
async fn a_link_says_what_last_failed_and_saving_it_starts_again() {
    let w = world().await;
    let (_, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!(v["repo"]["last_failure"], Value::Null);
    w.ts.store
        .note_repo_failure(&w.qid, None, "The daemon could not clone it.")
        .unwrap();
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["last_failure"], "The daemon could not clone it.");
    assert!(v["repo"]["last_failure_at"].is_string(), "{v}");
    let mut b = link();
    b["base"] = json!("release");
    b["if_rev"] = v["repo"]["rev"].clone();
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["last_failure"], Value::Null);
}

#[tokio::test]
async fn with_no_workspace_a_daemon_says_so() {
    let w = world().await;
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    let runtime = &v["runtime"];
    assert_eq!(runtime["configured"], false, "{v}");
    assert_eq!(runtime["third_party"], false, "{v}");
    assert_eq!(runtime["failed"], false, "{v}");
    assert_eq!(runtime["operator_target"], false, "{v}");
}

/// A world whose driver booted on a home holding `workspace.toml` as `text`,
/// or something of that name it cannot read.
async fn booted(text: Option<&str>) -> (World, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("workspace.toml");
    match text {
        Some(t) => std::fs::write(&path, t).unwrap(),
        None => std::fs::create_dir(&path).unwrap(),
    }
    let root = home.path().to_path_buf();
    let w = world_with(move |s| {
        clax_server::bot::driver_for_home(s, Some(&root));
    })
    .await;
    (w, home)
}

#[tokio::test]
async fn a_workspace_that_cannot_be_read_never_says_nothing_is_cloned() {
    for text in [Some("[runtime]\nkind = \"nope\"\n"), None] {
        let (w, _home) = booted(text).await;
        let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
        let runtime = &v["runtime"];
        assert_eq!(runtime["configured"], true, "{text:?}: {v}");
        assert_eq!(runtime["third_party"], true, "{text:?}: {v}");
        assert_eq!(runtime["failed"], true, "{text:?}: {v}");
    }
}

#[tokio::test]
async fn what_the_routes_say_is_the_workspace_that_booted_and_no_file() {
    let (w, home) = booted(Some("shell_for_all = true\n[runtime]\nkind = \"docker\"\n")).await;
    let path = home.path().join("workspace.toml");
    std::fs::write(path, "not what is running").unwrap();
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    assert_eq!(v["runtime"]["configured"], true, "{v}");
    assert_eq!(v["runtime"]["failed"], false, "{v}");
    assert_eq!(v["runtime"]["egress_held"], false, "{v}");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/admin/repos", None).await;
    assert_eq!(v["runtime"]["configured"], true, "{v}");
}

#[tokio::test]
async fn a_queue_an_operators_file_names_says_its_link_is_not_what_runs() {
    let facts = clax_http::bot::RuntimeFacts {
        configured: true,
        ..Default::default()
    };
    let held = std::sync::Arc::new(std::sync::Mutex::new(None));
    let h = held.clone();
    let w = world_with(move |s| {
        *h.lock().unwrap() = Some(s.bot.runtime_facts.clone());
    })
    .await;
    let slot = held.lock().unwrap().take().unwrap();
    slot.set(clax_http::bot::RuntimeFacts {
        operator_queues: vec![w.qid.clone()],
        ..facts
    })
    .unwrap();
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    assert_eq!(v["runtime"]["operator_target"], true, "{v}");
}

#[tokio::test]
async fn deleting_a_queue_takes_its_link_and_tokens() {
    let w = world().await;
    browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(link())).await;
    let kek = Kek::from_bytes([7; 32]);
    w.ts.store
        .set_queue_repo_token(
            &clax_core::audit::AuditCtx::DAEMON,
            &kek,
            &w.qid,
            TokenKind::Write,
            WRITE,
            &w.ts.store.queue_repo(&w.qid).unwrap().unwrap().rev,
        )
        .unwrap();
    let (s, _) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("/api/queues/{}", w.qid),
        None,
    )
    .await;
    assert_eq!(s, 204);
    assert!(w.ts.store.queue_repo(&w.qid).unwrap().is_none());
    assert!(
        w.ts.store
            .secret_meta(&clax_store::secrets::SecretSlot::RepoWrite(w.qid.clone()))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn no_kek_means_no_token_is_taken() {
    let ts = TestServer::spawn_on("127.0.0.1".parse().unwrap(), |s| {
        s.bot = std::sync::Arc::new(clax_http::bot::Bot::default());
    })
    .await;
    let owner = ts.owner_cookie();
    let (_, q) = browser(
        &ts,
        &owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Bugs"})),
    )
    .await;
    let repo = format!("/api/queues/{}/repo", q["queue"]["id"].as_str().unwrap());
    let (s, v) = browser(&ts, &owner, Method::PUT, &repo, Some(link())).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = browser(
        &ts,
        &owner,
        Method::PUT,
        &format!("{repo}/tokens/write"),
        Some(json!({"token": WRITE, "if_rev": "any"})),
    )
    .await;
    assert_eq!((s, code(&v)), (503, "no_kek"), "{v}");
}

/// Disconnects the link as `cookie`'s panel, which read it just now, does.
async fn disconnect(w: &World, cookie: &str) -> (u16, Value) {
    let rev = rev_of(w, cookie).await;
    let body = json!({"if_rev": rev});
    browser(&w.ts, cookie, Method::DELETE, &w.repo(), Some(body)).await
}

/// The link's `rev` as `cookie`'s panel reads it now.
async fn rev_of(w: &World, cookie: &str) -> String {
    let (s, v) = browser(&w.ts, cookie, Method::GET, &w.repo(), None).await;
    assert_eq!(s, 200, "{v}");
    v["repo"]["rev"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_stale_panel_cannot_seal_a_token_to_an_address_its_person_never_saw() {
    let w = world().await;
    w.set_member(Some("admin")).await;
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!(s, 200, "{v}");
    // Sam opens the panel, showing github.com.
    let seen = rev_of(&w, &w.sam).await;
    // The owner points the link elsewhere.
    let mut b = link();
    b["repo_url"] = json!("https://git.acme.dev/acme/web.git");
    b["forge_api_url"] = json!("https://git.acme.dev/api");
    b["if_rev"] = json!(seen);
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");

    // Sam's token, pasted into the stale panel, is sealed nowhere.
    let token = format!("{}/tokens/write", w.repo());
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &token,
        Some(json!({"token": WRITE, "if_rev": seen})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "link_changed"), "{v}");
    assert!(!v.to_string().contains("CANARY"), "{v}");
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["access"]["write_token"]["set"], false);

    // Sam's stale Save does not put the old address back.
    let mut b = link();
    b["if_rev"] = json!(seen);
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!((s, code(&v)), (400, "link_changed"), "{v}");
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["repo_url"], "https://git.acme.dev/acme/web.git");

    // Nor does a panel that saw no link overwrite one.
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!((s, code(&v)), (400, "link_changed"), "{v}");

    // A link removed and made again is another link to a stale panel.
    let before = rev_of(&w, &w.sam).await;
    disconnect(&w, &w.owner).await;
    browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(link())).await;
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &token,
        Some(json!({"token": WRITE, "if_rev": before})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "link_changed"), "{v}");

    // Read again, it goes through.
    let fresh = rev_of(&w, &w.sam).await;
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &token,
        Some(json!({"token": WRITE, "if_rev": fresh})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn a_revoked_makers_link_goes_unless_a_live_admin_remains() {
    let w = world().await;
    // Sam's own queue, with a token link no one else can reach.
    let (s, q) = browser(
        &w.ts,
        &w.sam,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Sam's"})),
    )
    .await;
    assert_eq!(s, 201, "{q}");
    let sams = q["queue"]["id"].as_str().unwrap().to_string();
    let path = format!("/api/queues/{sams}/repo");
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &path, Some(link())).await;
    assert_eq!(s, 200, "{v}");
    let rev = v["repo"]["rev"].as_str().unwrap();
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &format!("{path}/tokens/write"),
        Some(json!({"token": WRITE, "if_rev": rev})),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // The owner's queue, where Sam is one admin among others.
    w.set_member(Some("admin")).await;
    browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(link())).await;

    w.ts.store
        .revoke_person(&clax_core::audit::AuditCtx::DAEMON, &w.sam_public)
        .unwrap();

    assert!(w.ts.store.queue_repo(&sams).unwrap().is_none());
    assert!(
        w.ts.store
            .secret_meta(&clax_store::secrets::SecretSlot::RepoWrite(sams.clone()))
            .unwrap()
            .is_none()
    );
    assert!(w.ts.store.queue_repo(&w.qid).unwrap().is_some());
    let trail = w.trail();
    assert!(trail.contains("\"reason\":\"no_live_admin\""), "{trail}");
}

#[tokio::test]
async fn the_owner_sees_every_link_and_can_disconnect_any() {
    let w = world().await;
    let (s, q) = browser(
        &w.ts,
        &w.sam,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Sam's"})),
    )
    .await;
    assert_eq!(s, 201, "{q}");
    let sams = q["queue"]["id"].as_str().unwrap().to_string();
    let path = format!("/api/queues/{sams}/repo");
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &path, Some(link())).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "needs_token");

    // No queue of the owner's, and no App: the owner still sees it.
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, "/api/admin/repos", None).await;
    assert_eq!(s, 200, "{v}");
    let line = &v["links"][0];
    assert_eq!(line["queue_id"], sams);
    assert_eq!(line["access"], "token");
    assert_eq!(line["repo_url"], "https://github.com/acme/web.git");
    assert_eq!(line["maker"]["id"], w.sam_public);
    assert_eq!(line["admins"][0]["id"], w.sam_public);
    assert_eq!(v["token_approval"], false);

    // The owner is no member of Sam's queue, and disconnects it all the same.
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &path, None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
    let line = format!("/api/admin/repos/links/{sams}");
    let (s, v) = browser(&w.ts, &w.sam, Method::DELETE, &line, None).await;
    assert_eq!(s, 403, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::DELETE, &line, None).await;
    assert_eq!(s, 204, "{v}");
    assert!(w.ts.store.queue_repo(&sams).unwrap().is_none());
}

#[tokio::test]
async fn the_owner_may_require_approval_of_token_links() {
    let w = world().await;
    w.set_member(Some("admin")).await;
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, &w.repo(), Some(link())).await;
    assert_eq!(s, 200, "{v}");
    let rev = v["repo"]["rev"].as_str().unwrap().to_string();
    sealed(&w, &w.sam, "write", WRITE).await;
    let v = sealed(&w, &w.sam, "read", READ).await;
    assert_eq!(v["repo"]["status"], "ready");

    let policy = "/api/admin/repos/policy";
    let on = json!({"token_approval": true});
    let (s, v) = browser(&w.ts, &w.sam, Method::PUT, policy, Some(on.clone())).await;
    assert_eq!(s, 403, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, policy, Some(on)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["token_approval"], true);
    assert!(w.trail().contains("\"token_approval\":true"));

    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["status"], "needs_approval");
    let kek = Kek::from_bytes([7; 32]);
    assert!(
        w.ts.store
            .open_queue_repo(&kek, &w.qid, Egress::Held)
            .unwrap()
            .is_none()
    );

    let approval = format!("/api/admin/repos/links/{}", w.qid);
    let yes = json!({"rev": rev, "approved": true});
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &approval, Some(yes)).await;
    assert_eq!(s, 204, "{v}");
    assert!(
        w.ts.store
            .open_queue_repo(&kek, &w.qid, Egress::Held)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_repository_on_github_cannot_be_cloned_from_another_repositorys_address() {
    let w = world().await;
    let mut b = link();
    b["repo"] = json!("acme/renamed");
    b["repo_url"] = json!("https://github.com/acme/web.git");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!((s, code(&v)), (400, "repo_url_mismatch"), "{v}");

    for ok in [
        "https://github.com/acme/web.git",
        "https://GitHub.com/ACME/web",
    ] {
        let mut b = link();
        b["repo_url"] = json!(ok);
        b["if_rev"] = match w.ts.store.queue_repo(&w.qid).unwrap() {
            Some(l) => json!(l.rev),
            None => Value::Null,
        };
        let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
        assert_eq!(s, 200, "{ok}: {v}");
    }

    // On a forge Clax knows nothing about, the path is its own.
    let mut b = link();
    b["repo_url"] = json!("https://git.acme.dev/mirrors/web.git");
    b["forge_api_url"] = json!("https://git.acme.dev/api");
    b["if_rev"] = json!(w.ts.store.queue_repo(&w.qid).unwrap().unwrap().rev);
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
}

/// Saves `link()` with `extra` over the queue's link as read just now.
async fn saved(w: &World, cookie: &str, extra: Value) -> Value {
    let (_, v) = browser(&w.ts, cookie, Method::GET, &w.repo(), None).await;
    let mut b = link();
    b["if_rev"] = v["repo"]["rev"].clone();
    for (k, val) in extra.as_object().unwrap() {
        b[k] = val.clone();
    }
    let (s, v) = browser(&w.ts, cookie, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    v
}

/// Seals `token` as the queue's `which` token.
async fn sealed(w: &World, cookie: &str, which: &str, token: &str) -> Value {
    let (_, v) = browser(&w.ts, cookie, Method::GET, &w.repo(), None).await;
    let (s, v) = browser(
        &w.ts,
        cookie,
        Method::PUT,
        &format!("{}/tokens/{which}", w.repo()),
        Some(json!({"token": token, "if_rev": v["repo"]["rev"]})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

/// The owner approves the only link as it now stands.
async fn approved(w: &World) {
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/admin/repos", None).await;
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &format!("/api/admin/repos/links/{}", w.qid),
        Some(json!({"rev": v["links"][0]["rev"], "approved": true})),
    )
    .await;
    assert_eq!(s, 204, "{v}");
}

#[tokio::test]
async fn a_private_link_says_what_it_still_needs_one_step_at_a_time() {
    let w = world().await;
    w.set_member(Some("admin")).await;
    let v = saved(&w, &w.sam, json!({"ci_accepted": false})).await;
    assert_eq!(v["repo"]["status"], "needs_token");
    assert_eq!(v["repo"]["ci_accepted"], false);

    let v = sealed(&w, &w.sam, "write", WRITE).await;
    assert_eq!(v["repo"]["status"], "needs_read_token");
    let kek = Kek::from_bytes([7; 32]);
    let opened = w.ts.store.open_queue_repo(&kek, &w.qid, Egress::Held);
    assert!(opened.unwrap().is_none(), "no clone can work with no token");

    let v = sealed(&w, &w.sam, "read", READ).await;
    assert_eq!(v["repo"]["status"], "needs_ci_acceptance");
    let opened = w.ts.store.open_queue_repo(&kek, &w.qid, Egress::Held);
    assert!(opened.unwrap().is_none(), "nobody has accepted its CI");

    let v = saved(&w, &w.sam, json!({"ci_accepted": true})).await;
    assert_eq!(v["repo"]["ci_accepted"], true);
    assert_eq!(v["repo"]["status"], "ready");
    let opened = w.ts.store.open_queue_repo(&kek, &w.qid, Egress::Held);
    assert!(opened.unwrap().is_some());
    assert!(w.trail().contains("\"ci_accepted\":true"));

    // Taking the read token away again leaves a link that cannot clone.
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::DELETE,
        &format!("{}/tokens/read", w.repo()),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "needs_read_token");
}

#[tokio::test]
async fn a_public_link_needs_no_read_token_and_no_acceptance_of_its_ci() {
    let w = world().await;
    saved(&w, &w.owner, json!({"public": true})).await;
    let v = sealed(&w, &w.owner, "write", WRITE).await;
    assert_eq!(v["repo"]["status"], "ready");
}

#[tokio::test]
async fn the_owners_own_approval_does_not_accept_a_repositorys_ci() {
    let w = world().await;
    w.set_app().await;
    let mut b = app_link();
    b["ci_accepted"] = json!(false);
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["access"]["approved"], true);
    assert_eq!(v["repo"]["status"], "needs_ci_acceptance");

    let mut b = app_link();
    b["ci_accepted"] = json!(true);
    b["if_rev"] = v["repo"]["rev"].clone();
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(b)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["repo"]["status"], "ready", "the owner has said it now");
}

#[tokio::test]
async fn the_owner_reads_what_makes_a_link_dangerous_on_one_born_that_way() {
    let w = world().await;
    w.set_member(Some("admin")).await;
    saved(
        &w,
        &w.sam,
        json!({
            "public": true,
            "allow_open_egress": true,
            "reviewed_previews": true,
            "wake_on_reply": true,
            "preview_ready_path": "/up",
            "protected_paths": ["deploy/"],
            "ci_accepted": true,
        }),
    )
    .await;
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, "/api/admin/repos", None).await;
    assert_eq!(s, 200, "{v}");
    let line = &v["links"][0];
    assert_eq!(line["public"], true, "{line}");
    assert_eq!(line["allow_open_egress"], true, "{line}");
    assert_eq!(line["reviewed_previews"], true, "{line}");
    assert_eq!(line["wake_on_reply"], true, "{line}");
    assert_eq!(line["preview_url"], "https://pr-{pr}.acme.dev", "{line}");
    assert_eq!(line["preview_ready_path"], "/up", "{line}");
    assert_eq!(line["protected_paths"], json!(["deploy/"]), "{line}");
    assert_eq!(line["ci_accepted"], true, "{line}");
}

#[tokio::test]
async fn a_queue_admins_change_to_anything_the_owner_read_voids_the_approval() {
    let w = world().await;
    w.set_member(Some("admin")).await;
    let on = json!({"token_approval": true});
    let policy = "/api/admin/repos/policy";
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, policy, Some(on)).await;
    assert_eq!(s, 200, "{v}");
    let start = json!({"ci_accepted": true});
    saved(&w, &w.sam, start.clone()).await;
    sealed(&w, &w.sam, "write", WRITE).await;
    sealed(&w, &w.sam, "read", READ).await;
    for change in [
        json!({"public": true}),
        json!({"allow_open_egress": true}),
        json!({"reviewed_previews": true}),
        json!({"wake_on_reply": true}),
        json!({"preview_url": "https://other-{pr}.acme.dev"}),
        json!({"preview_ready_path": "/up"}),
        json!({"protected_paths": ["deploy/"]}),
    ] {
        saved(&w, &w.sam, start.clone()).await;
        approved(&w).await;
        let (_, v) = browser(&w.ts, &w.sam, Method::GET, &w.repo(), None).await;
        assert_eq!(v["repo"]["status"], "ready", "before {change}");

        let mut b = start.clone();
        for (k, val) in change.as_object().unwrap() {
            b[k] = val.clone();
        }
        let v = saved(&w, &w.sam, b).await;
        assert_eq!(v["repo"]["status"], "needs_approval", "after {change}");
    }

    // Withdrawing acceptance voids the approval too, so accepting again goes
    // back to the owner.
    saved(&w, &w.sam, start.clone()).await;
    approved(&w).await;
    let v = saved(&w, &w.sam, json!({"ci_accepted": false})).await;
    assert_eq!(v["repo"]["status"], "needs_ci_acceptance");
    let v = saved(&w, &w.sam, start.clone()).await;
    assert_eq!(v["repo"]["status"], "needs_approval");

    // What makes a link no more dangerous is its admins' to change.
    approved(&w).await;
    let mut b = start.clone();
    b["preview_timeout_secs"] = json!(120);
    let v = saved(&w, &w.sam, b).await;
    assert_eq!(v["repo"]["status"], "ready");
}

#[tokio::test]
async fn an_app_link_reads_as_connected_only_once_it_has_worked_itself() {
    let w = world().await;
    w.set_app().await;
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &w.repo(), Some(app_link())).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["repo"]["last_ok_at"],
        Value::Null,
        "asking where the App is installed tries nothing of the link: {v}"
    );

    // Another queue's link worked: that says nothing of this one.
    let (s, q) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Other"})),
    )
    .await;
    assert_eq!(s, 201, "{q}");
    let other = q["queue"]["id"].as_str().unwrap().to_string();
    let path = format!("/api/queues/{other}/repo");
    let (s, v) = browser(&w.ts, &w.owner, Method::PUT, &path, Some(app_link())).await;
    assert_eq!(s, 200, "{v}");
    let stamp = w.ts.store.queue_repo_stamp(&other).unwrap().unwrap();
    w.ts.store
        .note_repo_ok(&other, &stamp, clax_store::queue_repos::RepoUse::Write)
        .unwrap();
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &path, None).await;
    assert!(v["repo"]["last_ok_at"].is_string(), "{v}");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &w.repo(), None).await;
    assert_eq!(v["repo"]["last_ok_at"], Value::Null, "{v}");
}

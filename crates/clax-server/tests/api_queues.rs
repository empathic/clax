//! Report queues' administration (`routes/queues.rs`) and discovery
//! (`routes/discovery.rs`): queues made, listed, changed and deleted in a
//! browser; members and their roles; service grants shown once and their
//! revocation ending the reporter tokens they minted; another person's
//! queue out of reach; agents refused; `/.well-known/clax` on a local and a
//! hosted Clax.

use base64::Engine as _;
use clax_core::working::ManualClock;
use clax_testkit::TestServer;
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A request from the browser holding `cookie` (`name=value`).
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

/// A daemon listening beyond loopback (so it invites), Sam connected, and
/// the owner's and Sam's browser cookies.
struct World {
    ts: TestServer,
    owner: String,
    sam: String,
    sam_public: String,
    sam_client: clax_testkit::TestClient,
}

async fn world() -> World {
    world_with(|_| {}).await
}

/// [`world`] with its daemon's state set up by `f`.
async fn world_with(f: impl FnOnce(&mut clax_http::state::AppState)) -> World {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), f).await;
    let sam_client = ts.connect_person("Sam", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam_client.grant_id()).unwrap();
    let sam = format!("clax_viewer={}", ts.browser_cookie(&v.id));
    World {
        owner: ts.owner_cookie(),
        sam,
        sam_public: v.public_id,
        sam_client,
        ts,
    }
}

async fn make_queue(w: &World, cookie: &str, name: &str) -> String {
    let (s, v) = browser(
        &w.ts,
        cookie,
        Method::POST,
        "/api/queues",
        Some(json!({"name": name, "origins": ["https://App.Example.com/"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["queue"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_queue_is_made_listed_changed_and_deleted_in_the_browser() {
    let w = world().await;
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "  Bugs ", "origins": ["https://App.Example.com/", "HTTPS://*.Fly.Dev/"]})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let q = &v["queue"];
    assert_eq!(q["name"], "Bugs");
    assert_eq!(
        q["origins"],
        json!(["https://app.example.com", "https://*.fly.dev"])
    );
    assert_eq!(q["reporters_see"], "own");
    assert_eq!(q["role"], "admin");
    let qid = q["id"].as_str().unwrap().to_string();

    let (s, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"].as_array().unwrap().len(), 1);
    assert_eq!(v["queues"][0]["id"], qid.as_str());

    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Reports", "reporters_see": "all"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queue"]["name"], "Reports");
    assert_eq!(v["queue"]["reporters_see"], "all");
    assert_eq!(
        v["queue"]["origins"],
        json!(["https://app.example.com", "https://*.fly.dev"])
    );

    // Bad input is refused, naming what is wrong.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"origins": ["app.example.com"]})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_origin"), "{v}");
    // Origin patterns are listed, normalized as an origin is, as broad as
    // the owner wants; a malformed one is refused.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"origins": [
            "https://app.example.com",
            "HTTPS://PathBase-PR-*.Fly.Dev:443/",
            "https://*.fly.dev",
            "https://web.*.example.com",
            "https://*",
            "https://*.dev",
            "https://pr-*.dev",
            "https://a.*.com",
        ]})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["queue"]["origins"],
        json!([
            "https://app.example.com",
            "https://pathbase-pr-*.fly.dev",
            "https://*.fly.dev",
            "https://web.*.example.com",
            "https://*",
            "https://*.dev",
            "https://pr-*.dev",
            "https://a.*.com",
        ])
    );
    for bad in ["https://a**.fly.dev", "https://*.fly.dev:*"] {
        let (s, v) = browser(
            &w.ts,
            &w.owner,
            Method::PATCH,
            &format!("/api/queues/{qid}"),
            Some(json!({"origins": [bad]})),
        )
        .await;
        assert_eq!((s, code(&v)), (400, "invalid_origin"), "{bad}: {v}");
    }
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        "/api/queues",
        Some(json!({"name": " "})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_name"), "{v}");

    // A page of another origin cannot make one in the owner's name.
    let (s, v) = json_of(
        w.ts.client
            .post(format!("{}/api/queues", w.ts.base))
            .header("cookie", &w.owner)
            .header("origin", "https://evil.example")
            .json(&json!({"name": "Mine"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    // The owner's session with another origin is no credential at all
    // (`local_trust::session_counts`): the gate refuses it before any route.
    assert_eq!((s, code(&v)), (401, "no_credential"), "{v}");

    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("/api/queues/{qid}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Back"})),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");
}

#[tokio::test]
async fn members_hold_the_role_an_admin_gives_them() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    let members = format!("/api/queues/{qid}/members");
    let sams = format!("{members}/{}", w.sam_public);

    // Not yet a member: Sam reaches nothing of it.
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &members, None).await;
    assert_eq!((s, code(&v)), (404, "not_a_member"), "{v}");

    // Triage: Sam lists it and changes nothing.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &members, None).await;
    assert_eq!(s, 200, "{v}");
    let owner = w.ts.store.owner().unwrap().unwrap();
    let maker = json!({"public_id": owner.public_id, "name": owner.display_name,
                       "role": "admin", "maker": true, "owner": true});
    assert_eq!(
        v["members"],
        json!([maker, {"public_id": w.sam_public, "name": "Sam", "role": "triage",
                        "maker": false, "owner": false}]),
        "the maker first, as its admin"
    );
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"][0]["id"], qid.as_str());
    assert_eq!(v["queues"][0]["role"], "triage");
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Sam's"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &members, None).await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");

    // Sam's agent lists the queues Sam is a member of.
    let (s, v) = json_of(w.sam_client.agent("GET", "/api/queues", None, None).await).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"][0]["role"], "triage");

    // Admin: Sam changes it.
    browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "admin"})),
    )
    .await;
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Sam's"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // Only connected people are added.
    let ann = w.ts.viewer(Some("Ann")).await;
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &format!("{members}/{}", ann.public_id),
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "owner"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_json"), "{v}");

    // Removed: Sam reaches nothing of it again.
    let (s, v) = browser(&w.ts, &w.owner, Method::DELETE, &sams, None).await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &members, None).await;
    assert_eq!(v["members"], json!([maker]));
    // The maker stays its admin: neither changed nor removed.
    let mine = format!("{members}/{}", owner.public_id);
    for (m, body) in [
        (Method::PUT, Some(json!({"role": "triage"}))),
        (Method::DELETE, None),
    ] {
        let (s, v) = browser(&w.ts, &w.owner, m, &mine, body).await;
        assert_eq!((s, code(&v)), (400, "queue_maker"), "{v}");
    }
}

/// An admin adds a teammate by GitHub login: one signed in here is a
/// member at once, anyone else is listed `pending` until removed or bound;
/// a new member, either way or by public ID, finds an `access` item in
/// their inbox.
#[tokio::test]
async fn members_are_added_by_github_login_at_once_or_pending_and_told_in_their_inbox() {
    let gh = clax_testkit::github::FakeGitHub::start("id", "secret").await;
    let w = world_with(move |s| gh.turn_on(s)).await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    let members = format!("/api/queues/{qid}/members");
    let add = |login: &str, role: &str| json!({"github_login": login, "role": role});

    // Nobody here signed in as octo-cat: an invite, listed as pending.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &members,
        Some(add("@Octo-Cat", "triage")),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let invite = v["invite"].clone();
    assert_eq!(
        (&invite["github_login"], &invite["role"], &v["member"]),
        (&json!("octo-cat"), &json!("triage"), &Value::Null)
    );
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &members,
        Some(add("octo-cat", "admin")),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (&v["invite"]["id"], &v["invite"]["role"]),
        (&invite["id"], &json!("admin"))
    );
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &members, None).await;
    assert_eq!(v["pending"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["pending"][0]["github_login"], "octo-cat");
    assert_eq!(v["members"].as_array().unwrap().len(), 1, "only the maker");
    let id = invite["id"].as_str().unwrap();
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("{members}/pending/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &members, None).await;
    assert_eq!(v["pending"], json!([]));
    assert_eq!(
        v["github_logins"], true,
        "a local Clax with [github] signs people in with GitHub"
    );

    for (login, want) in [("-nope", "invalid_login"), ("a b", "invalid_login")] {
        let (s, v) = browser(
            &w.ts,
            &w.owner,
            Method::PUT,
            &members,
            Some(add(login, "triage")),
        )
        .await;
        assert_eq!((s, code(&v)), (400, want), "{login}: {v}");
    }

    // Someone who signed in here with GitHub: a member at once.
    let (gh, _) =
        w.ts.store
            .github_sign_in(
                &clax_core::audit::AuditCtx::DAEMON,
                &clax_store::github::GitHubUser {
                    id: 42,
                    login: "Mia-Gh".into(),
                    name: Some("Mia".into()),
                    avatar_url: None,
                },
                false,
            )
            .unwrap();
    let mia = gh.who.viewer.public_id;
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &members,
        Some(add("mia-gh", "triage")),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        v["member"],
        json!({"public_id": mia, "name": "Mia", "role": "triage", "maker": false, "owner": false})
    );

    // Sam, added by public ID, is told in their inbox; a role change is no
    // second item.
    let sams = format!("{members}/{}", w.sam_public);
    for role in ["triage", "admin"] {
        let (s, v) = browser(
            &w.ts,
            &w.owner,
            Method::PUT,
            &sams,
            Some(json!({"role": role})),
        )
        .await;
        assert_eq!(s, 204, "{v}");
    }
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::GET,
        "/api/inbox?kind=access&read=all",
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    let owner = w.ts.store.owner().unwrap().unwrap();
    assert_eq!(
        items[0]["access"],
        json!({"queue_id": qid, "queue_name": "Bugs", "role": "triage", "by": owner.display_name})
    );
    assert_eq!(
        (&items[0]["url"], &items[0]["gone"]),
        (&json!("/queues"), &json!(false))
    );

    // A triage member adds no one; an agent is refused.
    browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &sams,
        Some(json!({"role": "triage"})),
    )
    .await;
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &members,
        Some(add("zed", "triage")),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.put(format!("{}{members}", w.ts.base)))
            .header("x-clax-channel", "agent")
            .json(&add("zed", "triage"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
}

#[tokio::test]
async fn another_persons_queue_is_not_found() {
    let w = world().await;
    let mia = w.ts.connect_person("Mia", "clax:collaborator").await;
    let mia_viewer = w.ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
    let mia = format!("clax_viewer={}", w.ts.browser_cookie(&mia_viewer.id));
    let qid = make_queue(&w, &w.sam, "Sam's bugs").await;
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"][0]["role"], "admin");
    assert_eq!(v["queues"][0]["member"], true);
    let (_, v) = browser(&w.ts, &mia, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"], json!([]));
    for (m, path, body) in [
        (
            Method::PATCH,
            format!("/api/queues/{qid}"),
            Some(json!({"name": "x"})),
        ),
        (Method::DELETE, format!("/api/queues/{qid}"), None),
        (Method::GET, format!("/api/queues/{qid}/members"), None),
        (Method::GET, format!("/api/queues/{qid}/people?q=sam"), None),
        (
            Method::GET,
            format!("/api/queues/{qid}/service-grants"),
            None,
        ),
        (
            Method::POST,
            format!("/api/queues/{qid}/service-grants"),
            Some(json!({"name": "x"})),
        ),
        (Method::GET, format!("/api/queues/{qid}/reports"), None),
    ] {
        let (s, v) = browser(&w.ts, &mia, m.clone(), &path, body).await;
        assert_eq!((s, code(&v)), (404, "not_a_member"), "{m} {path}: {v}");
    }
    // A viewer who is no connected person makes none.
    let ann = w.ts.viewer(Some("Ann")).await;
    let (s, v) = browser(
        &w.ts,
        &format!("clax_viewer={}", ann.cookie),
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Ann's"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_a_person"), "{v}");
}

/// The owner oversees every queue in their browser: a person's queue is
/// listed to them as admin, not a member, and theirs to administer and
/// triage; their agents work only the queues they are members of. Added
/// (by `*owner` or their own public ID), the owner is a member like anyone.
#[tokio::test]
async fn the_owner_oversees_every_queue_and_joins_one_like_anyone() {
    let w = world().await;
    let qid = make_queue(&w, &w.sam, "Sam's bugs").await;
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(v["queues"][0]["id"], qid.as_str());
    assert_eq!(
        (&v["queues"][0]["role"], &v["queues"][0]["member"]),
        (&json!("admin"), &json!(false))
    );
    for path in [
        format!("/api/queues/{qid}/members"),
        format!("/api/queues/{qid}/service-grants"),
        format!("/api/queues/{qid}/reports"),
    ] {
        let (s, v) = browser(&w.ts, &w.owner, Method::GET, &path, None).await;
        assert_eq!(s, 200, "{path}: {v}");
    }
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Bugs"})),
    )
    .await;
    assert_eq!((s, &v["queue"]["role"]), (200, &json!("admin")), "{v}");
    // The owner's agent is no member yet.
    let agent_list = async || {
        json_of(
            w.ts.authed(w.ts.client.get(format!("{}/api/queues", w.ts.base)))
                .header("x-clax-channel", "agent")
                .send()
                .await
                .unwrap(),
        )
        .await
        .1
    };
    assert_eq!(agent_list().await["queues"], json!([]));

    let members = format!("/api/queues/{qid}/members");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &format!("{members}/*owner"),
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let owner = w.ts.store.owner().unwrap().unwrap();
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &members, None).await;
    assert_eq!(v["members"][0]["public_id"], w.sam_public.as_str());
    assert_eq!(v["members"][0]["maker"], true);
    assert_eq!(
        v["members"][1],
        json!({"public_id": owner.public_id, "name": owner.display_name,
               "role": "triage", "maker": false, "owner": true})
    );
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, "/api/queues", None).await;
    assert_eq!(
        (&v["queues"][0]["role"], &v["queues"][0]["member"]),
        (&json!("triage"), &json!(true))
    );
    assert_eq!(agent_list().await["queues"][0]["role"], "triage");
    // Sam removes the owner by their public ID.
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::DELETE,
        &format!("{members}/{}", owner.public_id),
        None,
    )
    .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(agent_list().await["queues"], json!([]));
}

/// A queue's admin finds the people they may add: the owner and connected
/// people, by name or whole public ID, at most 20, and no plain viewer.
#[tokio::test]
async fn a_queue_admin_finds_people_to_add_by_name() {
    let w = world().await;
    w.ts.store.set_owner_name("Alex", true).unwrap();
    let qid = make_queue(&w, &w.sam, "Sam's bugs").await;
    w.ts.viewer(Some("Sabine")).await;
    let people = |q: &str| format!("/api/queues/{qid}/people?q={q}");
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &people("SA"), None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["people"],
        json!([{"public_id": w.sam_public, "name": "Sam", "owner": false, "avatar_url": null}]),
        "Sabine is connected to nothing"
    );
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &people("alex"), None).await;
    assert_eq!(v["people"][0]["owner"], true);
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &people(&w.sam_public), None).await;
    assert_eq!(v["people"].as_array().unwrap().len(), 1);
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &people(""), None).await;
    assert_eq!(v["people"], json!([]));
    let long = "a".repeat(101);
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, &people(&long), None).await;
    assert_eq!((s, code(&v)), (400, "invalid_argument"), "{v}");
    // A triage member finds no one.
    let mia = w.ts.connect_person("Mia", "clax:collaborator").await;
    let mia_viewer = w.ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
    let mia = format!("clax_viewer={}", w.ts.browser_cookie(&mia_viewer.id));
    browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &format!("/api/queues/{qid}/members/{}", mia_viewer.public_id),
        Some(json!({"role": "triage"})),
    )
    .await;
    let (s, v) = browser(&w.ts, &mia, Method::GET, &people("sa"), None).await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");
    // Many matches answer at most 20.
    for i in 0..25 {
        let p =
            w.ts.connect_person(&format!("Zed {i:02}"), "clax:collaborator")
                .await;
        drop(p);
    }
    let (_, v) = browser(&w.ts, &w.sam, Method::GET, &people("zed"), None).await;
    assert_eq!(v["people"].as_array().unwrap().len(), 20);
}

/// `queues.members_create`: on by default on a local Clax, so a member
/// makes a queue; the owner turns it off, and then only the owner and
/// administrators make queues while an existing queue's admin keeps
/// administering it. `GET /api/viewers/me` says what the browser may do.
#[tokio::test]
async fn the_owner_decides_whether_members_make_queues() {
    let w = world().await;
    let me = async |cookie: &str| {
        browser(&w.ts, cookie, Method::GET, "/api/viewers/me", None)
            .await
            .1
    };
    assert_eq!(
        me(&w.sam).await["queues"],
        json!({"member": 0, "may_make": true})
    );
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, "/api/admin/queues", None).await;
    assert_eq!(
        (s, &v),
        (200, &json!({"members_create": true, "source": "default"}))
    );
    let qid = make_queue(&w, &w.sam, "Sam's bugs").await;
    assert_eq!(
        me(&w.sam).await["queues"],
        json!({"member": 1, "may_make": true})
    );

    // Sam is no administrator: Sam neither reads nor changes the setting.
    let (s, v) = browser(&w.ts, &w.sam, Method::GET, "/api/admin/queues", None).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        "/api/admin/queues",
        Some(json!({"members_create": true})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");

    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        "/api/admin/queues",
        Some(json!({"members_create": false})),
    )
    .await;
    assert_eq!(
        (s, &v),
        (200, &json!({"members_create": false, "source": "db"}))
    );
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::POST,
        "/api/queues",
        Some(json!({"name": "Another"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "queues_admin_only"), "{v}");
    assert_eq!(
        me(&w.sam).await["queues"],
        json!({"member": 1, "may_make": false})
    );
    // Sam's own queue is still Sam's to change.
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PATCH,
        &format!("/api/queues/{qid}"),
        Some(json!({"name": "Still Sam's"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // The owner always makes one.
    make_queue(&w, &w.owner, "Mine").await;
    // A value that is no boolean is refused; null puts the default back.
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        "/api/admin/queues",
        Some(json!({"members_create": "yes"})),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_setting"), "{v}");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        "/api/admin/queues",
        Some(json!({"members_create": null})),
    )
    .await;
    assert_eq!(
        (s, &v),
        (200, &json!({"members_create": true, "source": "default"}))
    );
    let changes: Vec<Value> =
        w.ts.store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "admin.settings")
            .map(|e| serde_json::from_str(&e.body).unwrap())
            .collect();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[0]["fields"], json!(["queues.members_create"]));
    assert_eq!(changes[0]["values"]["queues.members_create"], false);
    // The setting is no agent's.
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.get(format!("{}/api/admin/queues", w.ts.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
}

#[tokio::test]
async fn agents_list_queues_and_administer_none() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    // The owner's agent lists the owner's queues.
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.get(format!("{}/api/queues", w.ts.base)))
            .header("x-clax-channel", "agent")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["queues"][0]["id"], qid.as_str());
    // … and makes none.
    let (s, v) = json_of(
        w.ts.authed(w.ts.client.post(format!("{}/api/queues", w.ts.base)))
            .header("x-clax-channel", "agent")
            .json(&json!({"name": "Agent's"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    // A person's agent is refused every admin route.
    let (s, v) = json_of(
        w.sam_client
            .agent("POST", "/api/queues", None, Some(json!({"name": "x"})))
            .await,
    )
    .await;
    assert_eq!(s, 403, "{v}");
    // The owner by hand administers.
    let (s, v) = json_of(
        w.ts.authed(
            w.ts.client
                .get(format!("{}/api/queues/{qid}/service-grants", w.ts.base)),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn a_service_grant_is_shown_once_and_its_revocation_ends_its_reporter_tokens() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Bugs").await;
    let grants = format!("/api/queues/{qid}/service-grants");
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        &grants,
        Some(json!({"name": "Backend"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["grant"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["grant"]["name"], "Backend");
    assert!(v["grant"]["created_at"].is_string());
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cxq_"), "{token}");
    let pkcs8 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(v["private_key"].as_str().unwrap())
        .unwrap();
    let key = clax_core::sig::KeyPair::from_pkcs8(&pkcs8).unwrap();

    // The grant is its token's, with the key's public half alone.
    let grant =
        w.ts.store
            .service_grant_by_token(&clax_core::secret::sha256_hex(&token))
            .unwrap()
            .expect("the token names the grant");
    assert_eq!(grant.id, gid);
    assert_eq!(
        grant.public_key,
        clax_core::sig::encode_public(&key.public())
    );

    // Listed without its token or key.
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &grants, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["grants"].as_array().unwrap().len(), 1);
    let listed = v["grants"][0].as_object().unwrap();
    let mut keys: Vec<&str> = listed.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["created_at", "id", "name", "origins"]);
    assert_eq!(
        listed["origins"],
        Value::Null,
        "a production grant has no origin limit"
    );
    assert!(!v.to_string().contains(&token));

    // It mints a reporter token, which its revocation ends.
    let reporter_key = clax_core::sig::KeyPair::generate();
    let (rtoken, _) =
        w.ts.store
            .mint_reporter_token(
                &grant,
                "user-1",
                "https://app.example.com",
                &clax_core::sig::encode_public(&reporter_key.public()),
                None,
            )
            .unwrap();
    assert!(
        w.ts.store
            .reporter_by_token(&rtoken.sha256_hex())
            .unwrap()
            .is_some()
    );
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("{grants}/{gid}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(
        w.ts.store.reporter_by_token(&rtoken.sha256_hex()).unwrap(),
        None
    );
    assert_eq!(
        w.ts.store
            .service_grant_by_token(&clax_core::secret::sha256_hex(&token))
            .unwrap(),
        None
    );
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &grants, None).await;
    assert_eq!(v["grants"], json!([]));
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::DELETE,
        &format!("{grants}/{gid}"),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

fn assert_discovery(v: &Value, kind: &str) {
    assert_eq!(v["kind"], kind, "{v}");
    assert_eq!(v["contract_version"], "1");
    let sign_in = v["sign_in"].as_array().unwrap();
    assert!(sign_in.contains(&json!("invite")), "{v}");
    assert!(sign_in.contains(&json!("reporter_token")), "{v}");
    assert!(
        v["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("queues"))
    );
}

#[tokio::test]
async fn discovery_says_what_this_clax_is_without_a_credential() {
    let ts = TestServer::spawn().await;
    let (s, v) = json_of(
        ts.client
            .get(format!("{}/.well-known/clax", ts.base))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_discovery(&v, "local");

    let clock = Arc::new(ManualClock::at("2026-10-27T10:00:00Z"));
    let hosted = TestServer::spawn_hosted(clock, Some("the setup secret")).await;
    let (s, v) = json_of(
        hosted
            .client
            .get(format!("{}/.well-known/clax", hosted.base))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_discovery(&v, "hosted");
    assert!(
        v["sign_in"]
            .as_array()
            .unwrap()
            .contains(&json!("sign_in_link"))
    );
}

/// Addendum 9: who answers a queue is the queue's own, ordered, checked,
/// set by its admins, and audited.
#[tokio::test]
async fn an_admin_orders_who_answers_the_queue_and_a_triage_member_changes_nothing() {
    let w = world().await;
    let qid = make_queue(&w, &w.owner, "Feedback").await;
    let rs = format!("/api/queues/{qid}/responders");
    let (s, v) = browser(&w.ts, &w.owner, Method::GET, &rs, None).await;
    assert_eq!((s, &v["responders"]), (200, &json!([])), "{v}");

    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::POST,
        &format!("/api/queues/{qid}/service-grants"),
        Some(json!({"name": "changelist-clax-queue"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let gid = v["grant"]["id"].as_str().unwrap().to_string();
    let list = json!([
        {"kind": "agent", "name": "work-changeling-queue-agent", "service_grant_id": gid, "escalate_after_secs": 1800},
        {"kind": "bot", "profile": {"instructions": "Triage UI bugs first.", "tier": "quick", "tools": ["read_report"]}}
    ]);
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &rs,
        Some(json!({"responders": list})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = browser(&w.ts, &w.owner, Method::GET, &rs, None).await;
    assert_eq!(
        v["responders"][0]["name"], "work-changeling-queue-agent",
        "{v}"
    );
    assert_eq!(v["responders"][1]["kind"], "bot");
    assert_eq!(
        v["responders"][1]["profile"]["max_runs_per_day"],
        Value::Null
    );

    for (bad, why) in [
        (
            json!([{"kind": "agent", "name": "a"}, {"kind": "agent", "name": "a"}]),
            "listed twice",
        ),
        (
            json!([{"kind": "agent", "name": "a", "service_grant_id": "nope"}]),
            "no such service grant",
        ),
        (
            json!([{"kind": "bot", "profile": {"instructions": "", "tier": "huge", "tools": []}}]),
            "tier",
        ),
        (
            json!([{"kind": "agent", "name": "a", "escalate_after_secs": 5}]),
            "escalation",
        ),
    ] {
        let (s, v) = browser(
            &w.ts,
            &w.owner,
            Method::PUT,
            &rs,
            Some(json!({"responders": bad})),
        )
        .await;
        assert_eq!((s, code(&v)), (400, "invalid_responders"), "{v}");
        assert!(v["error"]["message"].as_str().unwrap().contains(why), "{v}");
    }
    let (s, v) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &rs,
        Some(json!({"responders": [{"kind": "person", "name": "x"}]})),
    )
    .await;
    assert_eq!(s, 400, "{v}");

    // A triage member reads nothing of it and changes nothing.
    let (s, _) = browser(
        &w.ts,
        &w.owner,
        Method::PUT,
        &format!("/api/queues/{qid}/members/{}", w.sam_public),
        Some(json!({"role": "triage"})),
    )
    .await;
    assert_eq!(s, 204);
    let (s, v) = browser(
        &w.ts,
        &w.sam,
        Method::PUT,
        &rs,
        Some(json!({"responders": []})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_queue_admin"), "{v}");

    let audited =
        w.ts.store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "queue.update" && e.body.contains("\"responders\""))
            .count();
    assert_eq!(audited, 1);
}

/// A hosted daemon on a stopped clock, as a browser on its public host
/// reaches it: its owner signs in with the setup secret, a member by an
/// owner's invite.
struct Hosted {
    ts: TestServer,
    raw: reqwest::Client,
}

impl Hosted {
    async fn new() -> Hosted {
        let clock = Arc::new(ManualClock::at("2026-11-24T10:00:00Z"));
        let ts = TestServer::spawn_hosted(clock, Some("the setup secret")).await;
        let raw = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Hosted { ts, raw }
    }

    /// `method path` from the browser holding session `cookie`.
    async fn send(
        &self,
        cookie: Option<&str>,
        m: Method,
        path: &str,
        body: Value,
    ) -> (u16, Value, reqwest::header::HeaderMap) {
        let host = format!("localhost:{}", self.ts.addr.port());
        let mut r = self
            .raw
            .request(m.clone(), format!("{}{path}", self.ts.base))
            .header(reqwest::header::HOST, &host);
        if m != Method::GET {
            r = r
                .header(reqwest::header::ORIGIN, format!("http://{host}"))
                .json(&body);
        }
        if let Some(c) = cookie {
            r = r.header(reqwest::header::COOKIE, format!("clax_session={c}"));
        }
        let res = r.send().await.unwrap();
        let headers = res.headers().clone();
        let (s, v) = json_of(res).await;
        (s, v, headers)
    }

    async fn call(&self, cookie: &str, m: Method, path: &str, body: Value) -> (u16, Value) {
        let (s, v, _) = self.send(Some(cookie), m, path, body).await;
        (s, v)
    }

    async fn sign_in(&self, path: &str, body: Value) -> String {
        let (s, v, h) = self.send(None, Method::POST, path, body).await;
        assert_eq!(s, 200, "{path}: {v}");
        let c = h[reqwest::header::SET_COOKIE].to_str().unwrap();
        c.strip_prefix("clax_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    async fn owner(&self) -> String {
        self.sign_in(
            "/api/hosted/setup",
            json!({"secret": "the setup secret", "name": "Ada"}),
        )
        .await
    }

    /// A member `owner` invites: their session cookie and public ID.
    async fn member(&self, owner: &str, name: &str) -> (String, String) {
        let (s, v) = self
            .call(
                owner,
                Method::POST,
                "/api/people/invites",
                json!({"name": name}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let code = v["invite"]["url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let b = self
            .sign_in(&format!("/api/hosted/invites/{code}/accept"), json!({}))
            .await;
        let (_, me) = self
            .call(&b, Method::GET, "/api/hosted/me", Value::Null)
            .await;
        (b, me["person"]["public_id"].as_str().unwrap().to_string())
    }
}

/// On a hosted Clax members make no queue until an administrator turns it
/// on; the owner always does; an administrator the owner names oversees
/// every queue, and makes one whatever the setting.
#[tokio::test]
async fn on_a_hosted_clax_members_make_queues_once_an_administrator_says_so() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let (mia, _) = h.member(&owner, "Mia").await;
    let (ana, ana_id) = h.member(&owner, "Ana").await;
    let (_, me) = h
        .call(&mia, Method::GET, "/api/viewers/me", Value::Null)
        .await;
    assert_eq!(me["queues"], json!({"member": 0, "may_make": false}));
    let (s, v) = h
        .call(&owner, Method::GET, "/api/admin/queues", Value::Null)
        .await;
    assert_eq!(
        (s, &v),
        (200, &json!({"members_create": false, "source": "default"}))
    );
    let (s, v) = h
        .call(&mia, Method::POST, "/api/queues", json!({"name": "Mia's"}))
        .await;
    assert_eq!((s, code(&v)), (403, "queues_admin_only"), "{v}");
    let (s, v) = h
        .call(
            &owner,
            Method::POST,
            "/api/queues",
            json!({"name": "Ada's"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");

    let (s, v) = h
        .call(
            &owner,
            Method::PUT,
            "/api/admin/queues",
            json!({"members_create": true}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = h
        .call(&mia, Method::POST, "/api/queues", json!({"name": "Mia's"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let mias = v["queue"]["id"].as_str().unwrap().to_string();

    // Ana, made an administrator, oversees Mia's queue too.
    let (s, v) = h
        .call(
            &owner,
            Method::PUT,
            &format!("/api/admin/people/{ana_id}/role"),
            json!({"role": "admin"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = h.call(&ana, Method::GET, "/api/queues", Value::Null).await;
    let listed: Vec<(&str, &Value)> = v["queues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| (q["name"].as_str().unwrap(), &q["member"]))
        .collect();
    assert_eq!(listed, [("Ada's", &json!(false)), ("Mia's", &json!(false))]);
    let (s, v) = h
        .call(
            &ana,
            Method::GET,
            &format!("/api/queues/{mias}/members"),
            Value::Null,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = h
        .call(
            &owner,
            Method::PUT,
            "/api/admin/queues",
            json!({"members_create": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = h
        .call(&ana, Method::POST, "/api/queues", json!({"name": "Ana's"}))
        .await;
    assert_eq!(
        s, 201,
        "an administrator makes one whatever the setting: {v}"
    );
}

//! Tags on a local daemon: the owner's list (`/api/tags`), who may manage
//! it (the owner and the owner's agents), who may apply tags to an artifact
//! (those who administer it), the filters (`?tag=`, match all), what each
//! caller sees, and the stream event a change announces.

use clax_testkit::TestServer;
use clax_testkit::people::World;
use serde_json::{Value, json};

async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// The names of an artifact view's or an answer's `tags`.
fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// A request with the daemon token (the owner by hand).
async fn owner(ts: &TestServer, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut r = ts.authed(ts.client.request(
        reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{}{path}", ts.base),
    ));
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// A request from the browser holding `cookie` (a whole `name=value`), from
/// `origin` when given.
async fn browser(
    ts: &TestServer,
    cookie: &str,
    origin: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = ts
        .client
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", ts.base),
        )
        .header("cookie", cookie);
    if let Some(o) = origin {
        r = r.header("origin", o);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

async fn make_tag(ts: &TestServer, name: &str, colour: &str) -> Value {
    let (s, v) = owner(
        ts,
        "POST",
        "/api/tags",
        Some(json!({"name": name, "colour": colour})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["tag"].clone()
}

async fn new_artifact(ts: &TestServer, title: &str) -> String {
    ts.publish(title, &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_owner_manages_the_list_and_each_change_is_announced() {
    let ts = TestServer::spawn().await;
    let mut gallery = ts.stream_as_owner(&["gallery"]).await;
    let d = make_tag(&ts, "  Design ", "blue").await;
    assert_eq!(d["name"], "Design");
    assert_eq!(d["colour"], "blue");
    assert!(d["id"].as_str().unwrap().starts_with("t_"));
    let ev = gallery.next_named("tags").await;
    assert_eq!(ev["artifact_id"], Value::Null, "{ev}");
    // Defaults, and the values' checks.
    let (s, v) = owner(&ts, "POST", "/api/tags", Some(json!({"name": "draft"}))).await;
    assert_eq!((s, v["tag"]["colour"].as_str()), (201, Some("gray")), "{v}");
    for (body, status, want) in [
        (json!({"name": "DESIGN"}), 409, "tag_exists"),
        (json!({"name": " "}), 400, "invalid_tag"),
        (json!({"name": "x".repeat(41)}), 400, "invalid_tag"),
        (json!({"name": "a\u{200b}b"}), 400, "invalid_tag"),
        (json!({"name": "ok", "colour": "#f00"}), 400, "invalid_colour"),
        (json!({"name": "ok", "extra": 1}), 400, "invalid_json"),
    ] {
        let (s, v) = owner(&ts, "POST", "/api/tags", Some(body.clone())).await;
        assert_eq!((s, code(&v)), (status, want), "{body}: {v}");
    }
    // The list: sorted by name, with counts, the palette, and who manages.
    let aid = new_artifact(&ts, "Plan").await;
    let (s, v) = owner(
        &ts,
        "PUT",
        &format!("/api/artifacts/{aid}/tags"),
        Some(json!({"tags": ["design"]})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = owner(&ts, "GET", "/api/tags", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(names(&v["tags"]), ["Design", "draft"]);
    assert_eq!(v["tags"][0]["count"], 1);
    assert_eq!(v["tags"][1]["count"], 0);
    assert_eq!(v["may_manage"], true);
    assert_eq!(v["colours"].as_array().unwrap().len(), 10);
    // A rename keeps the ID and reaches the artifact.
    let id = d["id"].as_str().unwrap();
    let (s, v) = owner(
        &ts,
        "PATCH",
        &format!("/api/tags/{id}"),
        Some(json!({"name": "Design review", "colour": "purple", "description": "needs eyes"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["tag"]["id"], json!(id));
    let (_, a) = owner(&ts, "GET", &format!("/api/artifacts/{aid}"), None).await;
    assert_eq!(
        a["artifact"]["tags"],
        json!([{"id": id, "name": "Design review", "colour": "purple"}])
    );
    let (s, v) = owner(
        &ts,
        "PATCH",
        &format!("/api/tags/{id}"),
        Some(json!({"name": "DRAFT"})),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "tag_exists"), "{v}");
    let (s, _) = owner(
        &ts,
        "PATCH",
        "/api/tags/t_0000000000",
        Some(json!({"name": "x"})),
    )
    .await;
    assert_eq!(s, 404);
    // Deleting removes it from every artifact.
    let (s, v) = owner(&ts, "DELETE", &format!("/api/tags/{id}"), None).await;
    assert_eq!((s, &v["artifacts"]), (200, &json!(1)), "{v}");
    let (_, a) = owner(&ts, "GET", &format!("/api/artifacts/{aid}"), None).await;
    assert_eq!(a["artifact"]["tags"], json!([]));
    let (s, _) = owner(&ts, "DELETE", &format!("/api/tags/{id}"), None).await;
    assert_eq!(s, 404);
    // The owner's agent manages tags too; so does the owner's browser,
    // from the shell's own origin alone.
    let sid = ts.register_session("claude", "tags-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = json_of(
        ts.authed(ts.client.post(format!("{}/api/tags", ts.base)))
            .header("x-clax-session", &sid)
            .json(&json!({"name": "by-agent"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let cookie = ts.owner_cookie();
    let (s, v) = browser(
        &ts,
        &cookie,
        Some(&ts.base),
        "POST",
        "/api/tags",
        Some(json!({"name": "by-browser", "colour": "teal"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = browser(
        &ts,
        &cookie,
        Some("http://evil.example"),
        "POST",
        "/api/tags",
        Some(json!({"name": "csrf"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    let (s, v) = browser(
        &ts,
        &cookie,
        Some("http://evil.example"),
        "PUT",
        &format!("/api/artifacts/{aid}/tags"),
        Some(json!({"tags": ["draft"]})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden_origin"), "{v}");
    // The audit journal holds each change.
    let st = ts.store.clone();
    let kinds: Vec<String> = tokio::task::spawn_blocking(move || {
        st.events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    })
    .await
    .unwrap();
    for k in ["tag.create", "tag.update", "tag.delete", "artifact.update"] {
        assert!(kinds.iter().any(|x| x == k), "no {k}: {kinds:?}");
    }
}

#[tokio::test]
async fn people_guests_and_agents_apply_tags_only_where_they_administer() {
    let w = World::two_people().await;
    let ts = &w.ts;
    make_tag(ts, "draft", "gray").await;
    make_tag(ts, "final", "green").await;
    let owners = new_artifact(ts, "Owner's").await;
    let mias = w.read.artifact.clone();
    let tags = |aid: &str| format!("/api/artifacts/{aid}/tags");
    let add = |t: &str| Some(json!({"add": [t]}));
    // Sam's agent (a collaborator, who updates only his own artifacts)
    // changes no one's tags but his own person's.
    let (s, v) = json_of(
        w.sam
            .agent("PATCH", &tags(&owners), Some(&w.sam_session), add("draft"))
            .await,
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    // An editor of every artifact (`clax:full`) is no administrator of
    // another's: its agent changes the contents, not the tags.
    let ed = ts.connect_person("Ed", "clax:full").await;
    let ed_session = ed.register("claude", "ed-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for aid in [&owners, &mias] {
        let (s, v) = json_of(
            ed.agent("PATCH", &tags(aid), Some(&ed_session), add("draft"))
                .await,
        )
        .await;
        assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    }
    let res = w
        .sam
        .agent(
            "POST",
            "/api/artifacts",
            Some(&w.sam_session),
            Some(json!({"title": "Sam's", "files": {"index.html": {"content": "<p>s", "encoding": "utf8"}}})),
        )
        .await;
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
    let sams = v["artifact"]["id"].as_str().unwrap().to_string();
    let (s, v) = json_of(
        w.sam
            .agent("PATCH", &tags(&sams), Some(&w.sam_session), add("Draft"))
            .await,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!((names(&v["tags"]), &v["changed"]), (vec!["draft".to_string()], &json!(true)));
    // Again: no change.
    let (s, v) = json_of(
        w.sam
            .agent("PATCH", &tags(&sams), Some(&w.sam_session), add("draft"))
            .await,
    )
    .await;
    assert_eq!((s, &v["changed"]), (200, &json!(false)), "{v}");
    // Without a session, a person's agent writes nothing.
    let (s, v) = json_of(w.sam.agent("PATCH", &tags(&sams), None, add("final")).await).await;
    assert_eq!((s, code(&v)), (400, "no_session"), "{v}");
    // Agents apply only: an unknown tag is refused, naming the tags.
    let (s, v) = json_of(
        w.sam
            .agent("PATCH", &tags(&sams), Some(&w.sam_session), add("urgent"))
            .await,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "unknown_tag"), "{v}");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("'urgent'") && msg.contains("draft, final"), "{msg}");
    // Nor do they make tags.
    let (s, v) = json_of(
        w.sam
            .agent(
                "POST",
                "/api/tags",
                Some(&w.sam_session),
                Some(json!({"name": "urgent"})),
            )
            .await,
    )
    .await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let (s, v) = json_of(w.sam.agent("GET", "/api/tags", Some(&w.sam_session), None).await).await;
    assert_eq!((s, &v["may_manage"]), (200, &json!(false)), "{v}");
    assert_eq!(names(&v["tags"]), ["draft", "final"], "a person's agent sees every tag");
    // Mia's browser tags her own artifact, not the owner's; nor makes tags.
    let mia = format!("clax_viewer={}", w.mia_cookie);
    let (s, v) = browser(ts, &mia, None, "PUT", &tags(&mias), Some(json!({"tags": ["final"]}))).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = browser(ts, &mia, None, "PUT", &tags(&owners), Some(json!({"tags": ["final"]}))).await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    let (s, v) = browser(ts, &mia, None, "POST", "/api/tags", Some(json!({"name": "mine"}))).await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    // The owner's agent tags any artifact, Mia's included.
    let (s, v) = json_of(
        ts.authed(ts.client.patch(format!("{}{}", ts.base, tags(&mias))))
            .header("x-clax-session", &w.owner_session)
            .json(&json!({"add": ["draft"], "remove": ["final"]}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, names(&v["tags"])), (200, vec!["draft".to_string()]), "{v}");
    // A guest (a viewer who is no connected person) and no credential.
    let gil = ts.viewer(Some("Gil")).await;
    let gil = format!("clax_viewer={}", gil.cookie);
    let (s, v) = browser(ts, &gil, None, "PUT", &tags(&owners), Some(json!({"tags": []}))).await;
    assert_eq!((s, code(&v)), (403, "admin_only"), "{v}");
    let (s, v) = browser(ts, &gil, None, "POST", "/api/tags", Some(json!({"name": "g"}))).await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let (s, _) = json_of(
        ts.client
            .put(format!("{}{}", ts.base, tags(&owners)))
            .json(&json!({"tags": []}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 401);
    // No such artifact.
    let (s, _) = owner(ts, "PUT", &tags("zzzzzzzzzzzz"), Some(json!({"tags": []}))).await;
    assert_eq!(s, 404);
    // A reader's agent has no update right: refused before anything.
    let rae = ts.connect_person("Rae", "clax:reader").await;
    let rae_session = rae.register("claude", "rae-1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = json_of(
        rae.agent("PATCH", &tags(&owners), Some(&rae_session), add("draft"))
            .await,
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
}

#[tokio::test]
async fn filters_match_every_tag_named() {
    let ts = TestServer::spawn().await;
    for (n, c) in [("a", "red"), ("b", "blue"), ("c", "green")] {
        make_tag(&ts, n, c).await;
    }
    // An agent's publishes: each makes an item in the owner's inbox.
    let sid = ts.register_session("claude", "tags-f").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut made = Vec::new();
    for t in ["X", "Y", "Z"] {
        made.push(
            ts.publish_as(&sid, t, "<p>x</p>").await["artifact"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let [x, y, z] = [made[0].clone(), made[1].clone(), made[2].clone()];
    for (aid, t) in [(&x, json!(["a", "b"])), (&y, json!(["a"])), (&z, json!(["B", "c"]))] {
        let (s, v) = owner(
            &ts,
            "PUT",
            &format!("/api/artifacts/{aid}/tags"),
            Some(json!({"tags": t})),
        )
        .await;
        assert_eq!(s, 200, "{v}");
    }
    let listed = |v: &Value| -> Vec<String> {
        let mut ids: Vec<String> = v["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    let sorted = |mut v: Vec<&String>| {
        v.sort();
        v.into_iter().cloned().collect::<Vec<String>>()
    };
    let (_, v) = owner(&ts, "GET", "/api/artifacts?tag=a", None).await;
    assert_eq!(listed(&v), sorted(vec![&x, &y]));
    let (_, v) = owner(&ts, "GET", "/api/artifacts?tag=a&tag=B", None).await;
    assert_eq!(listed(&v), vec![x.clone()]);
    let (_, v) = owner(&ts, "GET", "/api/artifacts?tag=b", None).await;
    assert_eq!(listed(&v), sorted(vec![&x, &z]));
    let (_, v) = owner(&ts, "GET", "/api/artifacts?tag=nope", None).await;
    assert!(listed(&v).is_empty());
    // With ?artifact= too.
    let (_, v) = owner(&ts, "GET", &format!("/api/artifacts?artifact={y}&tag=b"), None).await;
    assert!(listed(&v).is_empty());
    // The artifacts carry their tags, sorted by name.
    let (_, v) = owner(&ts, "GET", &format!("/api/artifacts?artifact={z}"), None).await;
    assert_eq!(names(&v["artifacts"][0]["tags"]), ["b", "c"]);
    // The inbox: each publish made an item about its artifact.
    let cookie = ts.owner_cookie();
    let ids = |v: &Value| -> Vec<String> {
        let mut out: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["artifact"]["id"].as_str().map(str::to_string))
            .collect();
        out.sort();
        out.dedup();
        out
    };
    let (s, all) = browser(&ts, &cookie, None, "GET", "/api/inbox", None).await;
    assert_eq!(s, 200, "{all}");
    assert_eq!(ids(&all).len(), 3, "{all}");
    let (s, v) = browser(&ts, &cookie, None, "GET", "/api/inbox?tag=a&tag=b", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(ids(&v), vec![x.clone()]);
    assert_eq!(v["total"], json!(v["items"].as_array().unwrap().len()));
    let (_, v) = browser(&ts, &cookie, None, "GET", "/api/inbox?tag=zz", None).await;
    assert!(ids(&v).is_empty());
    let (s, v) = browser(&ts, &cookie, None, "GET", "/api/inbox?group=artifact&tag=a", None).await;
    assert_eq!(s, 200, "{v}");
    let mut groups: Vec<String> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["artifact_id"].as_str().unwrap().to_string())
        .collect();
    groups.sort();
    assert_eq!(groups, sorted(vec![&x, &y]));
    let (s, v) = browser(&ts, &cookie, None, "GET", "/api/inbox?group=agent&tag=a", None).await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = browser(&ts, &cookie, None, "GET", "/api/inbox/summary?tag=c", None).await;
    assert_eq!(s, 200, "{v}");
    assert!(!v["latest"].as_array().unwrap().is_empty(), "{v}");
    for item in v["latest"].as_array().unwrap() {
        assert_eq!(item["artifact"]["id"], json!(z), "{item}");
    }
    // Mark all read under a tag marks only its items.
    let (s, v) = browser(
        &ts,
        &cookie,
        None,
        "POST",
        "/api/inbox/read",
        Some(json!({"all": true, "filter": {"tag": ["c"]}})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = browser(&ts, &cookie, None, "GET", "/api/inbox?read=unread", None).await;
    assert!(!ids(&v).contains(&z), "{v}");
    assert!(ids(&v).contains(&x), "{v}");
}

#[tokio::test]
async fn counts_follow_what_the_caller_sees_and_guests_see_only_tags_in_use() {
    let w = World::two_people().await;
    let ts = &w.ts;
    make_tag(ts, "used", "red").await;
    make_tag(ts, "unused", "blue").await;
    let owners = new_artifact(ts, "Owner's").await;
    let (s, v) = owner(
        ts,
        "PUT",
        &format!("/api/artifacts/{owners}/tags"),
        Some(json!({"tags": ["used"]})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // A guest: only the tags of what it sees, every artifact on a local daemon.
    let gil = ts.viewer(Some("Gil")).await;
    let (s, v) = browser(
        ts,
        &format!("clax_viewer={}", gil.cookie),
        None,
        "GET",
        "/api/tags",
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(names(&v["tags"]), ["used"]);
    assert_eq!(v["may_manage"], false);
    let (s, v) = json_of(ts.client.get(format!("{}/api/tags", ts.base)).send().await.unwrap()).await;
    assert_eq!((s, names(&v["tags"])), (200, vec!["used".to_string()]), "{v}");
    // A person's agent that may read only its person's own artifacts counts
    // only those.
    let rae = ts.connect_person("Rae", "clax:reader").await;
    let (s, v) = json_of(rae.get(None, "/api/tags").await).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["tags"][1]["count"], 1, "a reader of every artifact: {v}");
    let st = ts.store.clone();
    let pid = ts.store.viewer_of_grant(&rae.grant_id()).unwrap().public_id;
    tokio::task::spawn_blocking(move || {
        let own = json!([{"type": "clax-artifacts", "actions": ["read"], "locations": ["own"]}]);
        st.set_person_rights(
            &clax_core::audit::AuditCtx::DAEMON,
            &pid,
            Some(clax_core::rights::Rights::parse(&own).unwrap()),
        )
        .unwrap();
    })
    .await
    .unwrap();
    let (s, v) = json_of(rae.get(None, "/api/tags").await).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(names(&v["tags"]), ["unused", "used"]);
    assert!(v["tags"].as_array().unwrap().iter().all(|t| t["count"] == 0), "{v}");
    // The artifact view carries the tags for everyone who sees it, its
    // agents included.
    let (s, v) = json_of(
        w.sam
            .get(Some(&w.sam_session), &format!("/api/artifacts/{owners}"))
            .await,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(names(&v["artifact"]["tags"]), ["used"]);
}

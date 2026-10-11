//! Approval requests (spec 2026-10-08-multi-agent-multi-person §9, §10,
//! M18–M20, R1, R2): routing a thread to another person's agent asks that
//! person; an agent's `ask` asks and waits; the approver decides in their
//! own browser only; decisions and revocations reach every delivery path at
//! once; requests batch, are withdrawn, and are capped per approver.

use crate::common::people::World;
use clax_core::audit::AuditCtx;
use clax_server::testing::{EventReader, TestServer};
use serde_json::{Value, json};

/// `(status, body)` of a response.
async fn json_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(json!(text)))
}

/// The error code of a response.
async fn code(r: reqwest::Response) -> String {
    let (_, v) = json_of(r).await;
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

/// A browser whose viewer cookie is `cookie`, on this machine.
struct Browser<'a> {
    ts: &'a TestServer,
    cookie: String,
}

impl Browser<'_> {
    fn req(&self, m: &str, path: &str) -> reqwest::RequestBuilder {
        self.ts
            .client
            .request(m.parse().unwrap(), format!("{}{path}", self.ts.base))
            .header("cookie", &self.cookie)
    }

    async fn call(&self, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.req(m, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        json_of(r.send().await.unwrap()).await
    }

    /// The browser opens `/api/stream` and follows `topics`.
    async fn stream(&self, topics: &[&str]) -> EventReader {
        let res = self.req("GET", "/api/stream").send().await.unwrap();
        assert_eq!(res.status(), 200);
        let mut ev = EventReader::from_response(res);
        let id = ev.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        let (status, body) = self
            .call(
                "POST",
                &format!("/api/stream/{id}"),
                Some(json!({"subscribe": topics})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        ev
    }
}

/// The two-person world, with an artifact of the owner's that Sam's agent
/// watches.
struct Two {
    w: World,
    /// The owner's artifact.
    aid: String,
    sam_handle: String,
}

impl Two {
    async fn new() -> Two {
        Two::with(World::two_people().await).await
    }

    async fn with(w: World) -> Two {
        let a =
            w.ts.publish(
                "Quarterly Review",
                &[("index.html", "<main><h2>Quarterly goals</h2></main>")],
            )
            .await;
        let aid = a["artifact"]["id"].as_str().unwrap().to_string();
        let r = w
            .sam
            .agent(
                "PUT",
                &format!("/api/sessions/{}/watches/{aid}", w.sam_session),
                Some(&w.sam_session),
                None,
            )
            .await;
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        let sam_handle =
            w.ts.store
                .get_session(&w.sam_session)
                .unwrap()
                .unwrap()
                .agent_handle;
        Two { w, aid, sam_handle }
    }

    fn sam_browser(&self) -> Browser<'_> {
        let cookie = self
            .w
            .ts
            .store
            .viewer_of_grant(&self.w.sam.grant_id())
            .unwrap()
            .id;
        Browser {
            ts: &self.w.ts,
            cookie: format!("clax_viewer={cookie}"),
        }
    }

    fn mia_browser(&self) -> Browser<'_> {
        Browser {
            ts: &self.w.ts,
            cookie: format!("clax_viewer={}", self.w.mia_cookie),
        }
    }

    fn owner_browser(&self) -> Browser<'_> {
        Browser {
            ts: &self.w.ts,
            cookie: self.w.ts.owner_cookie(),
        }
    }

    fn sam_public(&self) -> String {
        self.w
            .ts
            .store
            .viewer_of_grant(&self.w.sam.grant_id())
            .unwrap()
            .public_id
    }

    /// A thread Mia's browser starts on the owner's artifact.
    async fn mia_thread(&self, body: &str) -> String {
        self.w
            .ts
            .thread_as(&self.aid, &self.w.mia_cookie, body)
            .await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Mia's browser sends `tid` to Sam's agent.
    async fn mia_sends_to_sam(&self, tid: &str) -> (u16, Value) {
        self.mia_browser()
            .call(
                "POST",
                &format!("/api/artifacts/{}/threads/{tid}/send", self.aid),
                Some(json!({"to": self.sam_handle})),
            )
            .await
    }

    /// Sam's agent's call.
    async fn sam_agent(&self, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        json_of(
            self.w
                .sam
                .agent(m, path, Some(&self.w.sam_session), body)
                .await,
        )
        .await
    }

    /// What Sam's agent's next tool result carries.
    async fn sam_feedback(&self, query: &str) -> Value {
        let (status, v) = self
            .sam_agent(
                "GET",
                &format!("/api/sessions/{}/feedback?{query}", self.w.sam_session),
                None,
            )
            .await;
        assert_eq!(status, 200, "{v}");
        v
    }

    /// Sam's open requests, as his browser lists them.
    async fn sams_requests(&self) -> Vec<Value> {
        let (status, v) = self
            .sam_browser()
            .call("GET", "/api/approvals/requests", None)
            .await;
        assert_eq!(status, 200, "{v}");
        v["requests"].as_array().unwrap().clone()
    }

    async fn sam_decides(&self, rid: &str, scope: &str) -> (u16, Value) {
        self.sam_browser()
            .call(
                "POST",
                &format!("/api/approvals/requests/{rid}/decide"),
                Some(json!({"scope": scope})),
            )
            .await
    }

    /// The thread as the owner's browser sees it.
    async fn owner_thread(&self, tid: &str) -> Value {
        let (status, v) = self
            .owner_browser()
            .call(
                "GET",
                &format!("/api/artifacts/{}/threads/{tid}", self.aid),
                None,
            )
            .await;
        assert_eq!(status, 200, "{v}");
        v["thread"].clone()
    }
}

/// Sam's approval of the owner everywhere, as `clax connect` records it
/// (M15).
async fn sam_approves_the_owner(t: &Two) {
    t.w.ts
        .store
        .approve(
            &AuditCtx::DAEMON,
            Some(&t.sam_public()),
            &t.w.ts.owner_public_id().await,
            clax_core::store::approvals::Scope::Everywhere,
            clax_core::store::approvals::Via::Connect,
        )
        .unwrap();
}

#[tokio::test]
async fn routing_to_another_persons_agent_makes_a_request() {
    let t = Two::new().await;
    let mut sam_inbox = t.sam_browser().stream(&["inbox"]).await;
    let tid = t.mia_thread("please fix the chart").await;
    let (status, v) = t.mia_sends_to_sam(&tid).await;
    assert_eq!(status, 200, "{v}");
    // The two listeners publish from their own tasks: either may come first.
    let mut got = std::collections::HashMap::new();
    while !(got.contains_key("inbox_item") && got.contains_key("approval_request")) {
        let (name, v) = sam_inbox.next().await;
        if name == "inbox_item" || name == "approval_request" {
            got.entry(name).or_insert(v);
        }
    }
    let item = got["inbox_item"].clone();
    assert_eq!(item["item"]["kind"], "approval", "{item}");
    // The card: the author, the agent, the thread and the withheld content
    // itself (people are not filtered), and the count.
    let card = &item["item"]["approval"];
    assert_eq!(card["author"]["name"], "Mia", "{card}");
    assert_eq!(card["author"]["public_id"], json!(t.w.mia_public));
    assert_eq!(card["agent"]["handle"], json!(t.sam_handle));
    assert_eq!(card["count"], 1);
    assert_eq!(card["cause"], "routing");
    assert_eq!(
        card["content"]["comments"][0]["body"],
        "please fix the chart"
    );
    assert_eq!(item["item"]["thread"]["id"], json!(tid));
    assert_eq!(item["item"]["read"], false);
    // The approval_request event on the same topic.
    let ev = got["approval_request"].clone();
    assert_eq!(ev["request"]["status"], "open", "{ev}");
    assert!(
        !ev.to_string().contains("please fix the chart"),
        "a request names no content: {ev}"
    );
    // The thread reads withheld for Sam's approval before any take.
    let st = &t.owner_thread(&tid).await["feedback_state"];
    assert_eq!(st["state"], "withheld", "{st}");
    assert_eq!(st["approver"], "Sam", "{st}");
    // Sam's agent is told, and that its person was asked.
    let fb = t.sam_feedback("tier=piggyback").await;
    let text = fb["text"].as_str().unwrap();
    assert!(text.contains("Your person has been asked."), "{text}");
    assert!(!text.contains("please fix the chart"), "{text}");
    assert_eq!(
        t.owner_thread(&tid).await["feedback_state"]["state"],
        "withheld"
    );
    // The owner's inbox holds no request of Sam's (R2).
    let (_, owners) = t
        .owner_browser()
        .call("GET", "/api/approvals/requests", None)
        .await;
    assert!(
        !owners.to_string().contains(&tid),
        "the owner's requests: {owners}"
    );
}

#[tokio::test]
async fn approval_changes_need_a_browser() {
    let t = Two::new().await;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    // The owner's own request (Mia → the owner's agent), and an approval
    // of the owner's that names no request.
    mia_sends_to_the_owners_agent(&t, &tid).await;
    let mine = t.w.ts.store.approval_requests(None, true, 10).unwrap().0;
    let rid = mine[0].id.clone();
    let unconnected = t.w.ts.store.live_approvals(None).unwrap()[0].id.clone();
    let ts = &t.w.ts;
    let changes = [
        (
            "POST",
            format!("/api/approvals/requests/{rid}/decide"),
            json!({"scope": "everywhere"}),
        ),
        (
            "POST",
            "/api/approvals".into(),
            json!({"author": t.w.mia_public, "scope": "everywhere"}),
        ),
        (
            "DELETE",
            format!("/api/approvals/{unconnected}"),
            json!(null),
        ),
    ];
    for (m, p, b) in &changes {
        // The daemon token alone, as the CLI or an owner agent could.
        let r = ts
            .authed(
                ts.client
                    .request(m.parse().unwrap(), format!("{}{p}", ts.base)),
            )
            .json(b)
            .send()
            .await
            .unwrap();
        assert_eq!(
            (r.status().as_u16(), code(r).await.as_str()),
            (403, "browser_required"),
            "token: {m} {p}"
        );
        // The owner's agent: the token naming its session.
        let r = ts
            .authed(
                ts.client
                    .request(m.parse().unwrap(), format!("{}{p}", ts.base)),
            )
            .header("x-clax-session", &t.w.owner_session)
            .json(b)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "owner agent: {m} {p}");
        // Sam's agent.
        let (status, _) = t.sam_agent(m, p, Some(b.clone())).await;
        assert_eq!(status, 403, "Sam's agent: {m} {p}");
    }
    // Nothing changed.
    assert_eq!(
        ts.store.approval_request(&rid, None).unwrap().status,
        clax_core::store::approvals::RequestStatus::Open
    );
    assert_eq!(ts.store.live_approvals(None).unwrap().len(), 1);
    // Agents read their own person's, never the content.
    let (status, v) = t.sam_agent("GET", "/api/approvals/requests", None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["requests"].as_array().unwrap().len(), 1, "{v}");
    assert!(!v.to_string().contains("please"), "{v}");
    let (status, v) = t.sam_agent("GET", "/api/approvals", None).await;
    assert_eq!((status, v["approvals"].clone()), (200, json!([])));
}

#[tokio::test]
async fn a_page_cannot_decide_an_approval() {
    let t = Two::new().await;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    let sams = t.sams_requests().await;
    let rid = sams[0]["id"].as_str().unwrap().to_string();
    mia_sends_to_the_owners_agent(&t, &tid).await;
    let owners = t.w.ts.store.approval_requests(None, true, 10).unwrap().0;
    let owner_rid = owners[0].id.clone();
    let ts = &t.w.ts;
    let page = format!("http://{}.localhost:{}", t.aid, ts.addr.port());
    let sam = t.sam_browser();
    let owner = t.owner_browser();
    let unconnected = ts.store.live_approvals(None).unwrap()[0].id.clone();
    for (who, b, rid) in [(&sam, "Sam", &rid), (&owner, "owner", &owner_rid)] {
        let changes = [
            (
                "POST",
                format!("/api/approvals/requests/{rid}/decide"),
                json!({"scope": "everywhere"}),
            ),
            (
                "POST",
                "/api/approvals".to_string(),
                json!({"author": t.w.mia_public, "scope": "everywhere"}),
            ),
            (
                "DELETE",
                format!("/api/approvals/{unconnected}"),
                json!(null),
            ),
        ];
        for (m, p, body) in &changes {
            for (h, v) in [
                ("origin", page.as_str()),
                ("origin", "null"),
                ("sec-fetch-site", "same-site"),
            ] {
                let r = who.req(m, p).header(h, v).json(body).send().await.unwrap();
                assert_eq!(
                    (r.status().as_u16(), code(r).await.as_str()),
                    (403, "forbidden_origin"),
                    "{b}: {h}: {v} on {m} {p}"
                );
            }
        }
    }
    assert_eq!(t.sams_requests().await.len(), 1, "still open");
}

#[tokio::test]
async fn only_the_approver_decides() {
    let t = Two::new().await;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    let rid = t.sams_requests().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/approvals/requests/{rid}/decide");
    let body = json!({"scope": "everywhere"});
    // The owner may not approve for Sam (R2), nor Mia for him.
    let (status, _) = t
        .owner_browser()
        .call("POST", &path, Some(body.clone()))
        .await;
    assert_eq!(status, 404);
    let (status, _) = t
        .mia_browser()
        .call("POST", &path, Some(body.clone()))
        .await;
    assert_eq!(status, 404);
    // A LAN viewer is no approver.
    let lan = t.w.ts.viewer(Some("Lan")).await;
    let (status, _) = Browser {
        ts: &t.w.ts,
        cookie: format!("clax_viewer={}", lan.cookie),
    }
    .call("POST", &path, Some(body.clone()))
    .await;
    assert_eq!(status, 404, "nothing of a LAN viewer's to decide (§9.3)");
    let (status, v) = t.sam_decides(&rid, "bogus").await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("invalid_scope"))
    );
    let (status, v) = t.sam_decides(&rid, "thread").await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["request"]["status"], "approved");
    assert_eq!(v["approval"]["scope"], "thread");
    assert_eq!(v["approval"]["thread_id"], json!(tid));
    let (status, v) = t.sam_decides(&rid, "decline").await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (409, json!("request_closed"))
    );
    // A thread approval covers Mia's later replies there (R1).
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["feedback"][0]["body"], "please", "{fb}");
    t.w.ts
        .reply_as(&t.aid, &tid, &t.w.mia_cookie, "and the legend")
        .await;
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["feedback"][0]["body"], "and the legend", "{fb}");
    assert!(t.sams_requests().await.is_empty());
}

#[tokio::test]
async fn ask_for_approval_waits_for_the_decision() {
    let t = Two::new().await;
    // Sam approved the owner when he connected (M15).
    sam_approves_the_owner(&t).await;
    // A thread Sam's agent was sent by the owner, where Mia then replies:
    // no routing, so the agent asks.
    let tid = t.w.ts.thread_as_owner(&t.aid, "the owner asks").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.w.ts
        .reply_as(&t.aid, &tid, &t.w.mia_cookie, "Mia replies")
        .await;
    let fb = t.sam_feedback("tier=piggyback").await;
    let text = fb["text"].as_str().unwrap();
    assert!(
        text.contains(&format!(
            "To ask your person, call ask with {{\"approval\": {{\"thread_id\": \"{tid}\"}}}}."
        )),
        "{text}"
    );
    let sid = t.w.sam_session.clone();
    let (status, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(
        (v["status"].clone(), v["scope"].clone()),
        (json!("open"), Value::Null)
    );
    let rid = v["approval_id"].as_str().unwrap().to_string();
    assert_eq!(v["request"]["cause"], "ask");
    assert_eq!(v["request"]["author"]["public_id"], json!(t.w.mia_public));
    // Asking again joins the open request.
    let (_, again) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    assert_eq!(again["approval_id"], json!(rid));
    // A poll without a wait answers at once.
    let (status, v) = t
        .sam_agent("GET", &format!("/api/sessions/{sid}/approvals/{rid}"), None)
        .await;
    assert_eq!((status, v["status"].clone()), (200, json!("open")), "{v}");
    // A waiting poll: held, then woken by Sam's decision in his browser.
    let waiting = format!("/api/sessions/{sid}/approvals/{rid}?wait=600");
    let poll = t.sam_agent("GET", &waiting, None);
    let control = async {
        t.w.ts.wait_question_waiters(&rid, 1).await;
        let (status, v) = t.sam_decides(&rid, "artifact").await;
        assert_eq!(status, 200, "{v}");
    };
    let ((status, v), ()) = tokio::join!(poll, control);
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        (v["status"].clone(), v["scope"].clone()),
        (json!("approved"), json!("artifact"))
    );
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["feedback"][0]["body"], "Mia replies", "{fb}");
    // Nothing is withheld there now.
    let (status, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("nothing_withheld"))
    );
    // Another person's request, or another session's, is not the agent's.
    let other = t.mia_thread("for the owner").await;
    mia_sends_to_the_owners_agent(&t, &other).await;
    let mine = t.w.ts.store.approval_requests(None, false, 10).unwrap().0;
    let (status, _) = t
        .sam_agent(
            "GET",
            &format!("/api/sessions/{sid}/approvals/{}", mine[0].id),
            None,
        )
        .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn an_ask_poll_ends_when_its_request_is_withdrawn() {
    let t = Two::new().await;
    sam_approves_the_owner(&t).await;
    let tid = t.w.ts.thread_as_owner(&t.aid, "the owner asks").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.w.ts
        .reply_as(&t.aid, &tid, &t.w.mia_cookie, "Mia replies")
        .await;
    let sid = t.w.sam_session.clone();
    let (_, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    let rid = v["approval_id"].as_str().unwrap().to_string();
    let waiting = format!("/api/sessions/{sid}/approvals/{rid}?wait=600");
    let poll = t.sam_agent("GET", &waiting, None);
    let control = async {
        t.w.ts.wait_question_waiters(&rid, 1).await;
        let (status, v) = t
            .owner_browser()
            .call(
                "POST",
                &format!("/api/artifacts/{}/threads/{tid}/resolve", t.aid),
                Some(json!({})),
            )
            .await;
        assert_eq!(status, 200, "{v}");
    };
    let ((status, v), ()) = tokio::join!(poll, control);
    assert_eq!(
        (status, v["status"].clone()),
        (200, json!("withdrawn")),
        "{v}"
    );
}

#[tokio::test]
async fn requests_batch_per_author_per_thread() {
    let t = Two::new().await;
    let tid = t.mia_thread("one").await;
    t.mia_sends_to_sam(&tid).await;
    t.w.ts.reply_as(&t.aid, &tid, &t.w.mia_cookie, "two").await;
    t.w.ts
        .reply_as(&t.aid, &tid, &t.w.mia_cookie, "three")
        .await;
    let rs = t.sams_requests().await;
    assert_eq!(rs.len(), 1, "{rs:?}");
    assert_eq!(rs[0]["count"], 3);
    // Another of Mia's threads is another request; the inbox holds one
    // item per request.
    let other = t.mia_thread("elsewhere").await;
    t.mia_sends_to_sam(&other).await;
    assert_eq!(t.sams_requests().await.len(), 2);
    let (_, inbox) = t
        .sam_browser()
        .call("GET", "/api/inbox?kind=approval", None)
        .await;
    assert_eq!(inbox["items"].as_array().unwrap().len(), 2, "{inbox}");
    let first = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["thread"]["id"] == json!(tid))
        .unwrap();
    let bodies: Vec<&str> = first["approval"]["content"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["one", "two", "three"]);
}

#[tokio::test]
async fn decline_keeps_rows_held_and_asks_again_only_after_ten_minutes() {
    let t = Two::new().await;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    let rid = t.sams_requests().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let g = t.w.ts.store.visibility_generation();
    let (status, v) = t.sam_decides(&rid, "decline").await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        (v["request"]["status"].clone(), v["approval"].clone()),
        (json!("declined"), Value::Null)
    );
    assert_eq!(
        t.w.ts.store.visibility_generation(),
        g + 1,
        "a decline bumps"
    );
    let fb = t.sam_feedback("tier=piggyback").await;
    assert!(fb["feedback"].as_array().unwrap().is_empty(), "{fb}");
    assert_eq!(fb["withheld"][0]["count"], 1, "{fb}");
    let sid = t.w.sam_session.clone();
    let (status, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("recently_declined"))
    );
    // Routing within the ten minutes asks nothing either.
    t.mia_sends_to_sam(&tid).await;
    assert!(t.sams_requests().await.is_empty());
}

#[tokio::test]
async fn approving_everywhere_closes_the_authors_other_requests() {
    let t = Two::new().await;
    let a = t.mia_thread("a").await;
    let b = t.mia_thread("b").await;
    t.mia_sends_to_sam(&a).await;
    t.mia_sends_to_sam(&b).await;
    let rs = t.sams_requests().await;
    assert_eq!(rs.len(), 2);
    let rid = rs[0]["id"].as_str().unwrap().to_string();
    let (status, v) = t.sam_decides(&rid, "everywhere").await;
    assert_eq!(status, 200, "{v}");
    assert!(t.sams_requests().await.is_empty());
    let (_, all) = t
        .sam_browser()
        .call("GET", "/api/approvals/requests?status=all", None)
        .await;
    let scopes: Vec<&Value> = all["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["scope"])
        .collect();
    assert_eq!(scopes, [&json!("everywhere"), &json!("everywhere")]);
    let (_, unread) = t
        .sam_browser()
        .call("GET", "/api/inbox?kind=approval&read=unread", None)
        .await;
    assert!(unread["items"].as_array().unwrap().is_empty(), "{unread}");
    // Both threads reach Sam's agent now.
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["feedback"].as_array().unwrap().len(), 2, "{fb}");
    // Revoked from his browser: Mia's next words are held again.
    let (_, list) = t.sam_browser().call("GET", "/api/approvals", None).await;
    let id = list["approvals"][0]["id"].as_str().unwrap().to_string();
    let (status, v) = t
        .sam_browser()
        .call("DELETE", &format!("/api/approvals/{id}"), None)
        .await;
    assert_eq!(status, 200, "{v}");
    t.w.ts.reply_as(&t.aid, &a, &t.w.mia_cookie, "later").await;
    let fb = t.sam_feedback("tier=piggyback").await;
    assert!(fb["feedback"].as_array().unwrap().is_empty(), "{fb}");
    assert_eq!(fb["withheld"][0]["count"], 1, "{fb}");
}

#[tokio::test]
async fn a_send_to_an_agent_that_cannot_read_is_refused() {
    let t = Two::new().await;
    // Sam's agents may read another artifact only.
    let rights = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": [t.w.scratch.artifact]},
        {"type": "clax-comments", "actions": ["read", "write", "reply", "resolve", "send", "watch"], "locations": ["*"]},
        {"type": "clax-ask", "actions": ["ask"], "locations": ["*"]}
    ]))
    .unwrap();
    t.w.ts
        .store
        .set_person_rights(&AuditCtx::DAEMON, &t.sam_public(), Some(rights))
        .unwrap();
    let tid = t.mia_thread("please").await;
    let (status, v) = t.mia_sends_to_sam(&tid).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("agent_cannot_read")),
        "{v}"
    );
    assert!(t.sams_requests().await.is_empty());
    // Naming it with @name writes nothing either.
    let name =
        t.w.ts
            .store
            .get_session(&t.w.sam_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let (status, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/comments", t.aid),
            Some(json!({"body": format!("@{name} look")})),
        )
        .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("agent_cannot_read")),
        "{v}"
    );
    let t2 = t.w.ts.store.get_thread(&tid).unwrap().unwrap();
    assert_eq!(t2.comments.len(), 1, "nothing was written");
}

#[tokio::test]
async fn naming_another_persons_agent_makes_a_request() {
    let t = Two::new().await;
    let name =
        t.w.ts
            .store
            .get_session(&t.w.sam_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let tid = t.mia_thread(&format!("@{name} please look")).await;
    let rs = t.sams_requests().await;
    assert_eq!(rs.len(), 1, "{rs:?}");
    assert_eq!(rs[0]["thread_id"], json!(tid));
    let fb = t.sam_feedback("tier=piggyback").await;
    assert!(
        fb["text"]
            .as_str()
            .unwrap()
            .contains("Your person has been asked."),
        "{fb}"
    );
}

#[tokio::test]
async fn a_page_send_routes_with_the_publisher_as_author() {
    let t = Two::new().await;
    // Sam approved the owner, not Mia. Mia's page (her files) runs in the
    // owner's browser and sends to Sam's agent.
    sam_approves_the_owner(&t).await;
    let aid = t.w.read.artifact.clone();
    let r =
        t.w.sam
            .agent(
                "PUT",
                &format!("/api/sessions/{}/watches/{aid}", t.w.sam_session),
                Some(&t.w.sam_session),
                None,
            )
            .await;
    assert_eq!(r.status(), 200);
    let form = reqwest::multipart::Form::new()
        .text("anchor", clax_server::testing::element_anchor().to_string())
        .text("body", "the page asks")
        .text("version", "2")
        .text("via_page", "true");
    let r = t
        .owner_browser()
        .req("POST", &format!("/api/artifacts/{aid}/threads"))
        .header("x-clax-via", "page")
        .multipart(form)
        .send()
        .await
        .unwrap();
    let (status, v) = json_of(r).await;
    assert_eq!(status, 201, "{v}");
    let tid = v["thread"]["id"].as_str().unwrap().to_string();
    let r = t
        .owner_browser()
        .req("POST", &format!("/api/artifacts/{aid}/threads/{tid}/send"))
        .header("x-clax-via", "page")
        .json(&json!({"to": t.sam_handle}))
        .send()
        .await
        .unwrap();
    let (status, v) = json_of(r).await;
    assert_eq!(status, 200, "{v}");
    let rs = t.sams_requests().await;
    assert_eq!(rs.len(), 1, "{rs:?}");
    assert_eq!(
        rs[0]["author"]["public_id"],
        json!(t.w.mia_public),
        "the publisher"
    );
    let (_, inbox) = t
        .sam_browser()
        .call("GET", "/api/inbox?kind=approval", None)
        .await;
    // "<publisher>'s page, via <viewer>'s browser": the page's authors (its
    // files are Mia's and the owner's) and the owner's browser.
    let via = &inbox["items"][0]["approval"]["via_page"];
    let owner = t.w.ts.owner_public_id().await;
    let authors: Vec<&Value> = via["authors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["public_id"])
        .collect();
    assert!(authors.contains(&&json!(t.w.mia_public)), "{inbox}");
    assert_eq!(via["viewer"]["public_id"], json!(owner), "{inbox}");
    // A person's agent never sends as a page.
    let path = format!("/api/artifacts/{aid}/threads/{tid}/send");
    let r =
        t.w.sam
            .call(clax_server::testing::Call {
                method: "POST",
                path: &path,
                authorization: Some(format!("Clax-Grant {}", t.w.sam.access())),
                body: Some(b"{}".to_vec()),
                headers: vec![
                    ("x-clax-session", t.w.sam_session.clone()),
                    ("x-clax-via", "page".into()),
                ],
                ..Default::default()
            })
            .await;
    let (status, v) = json_of(r).await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (400, json!("invalid_via_page")),
        "{v}"
    );
}

#[tokio::test]
async fn the_hundred_and_first_request_is_refused_with_a_banner() {
    use clax_core::store::approvals::MAX_OPEN_REQUESTS;
    let t = Two::new().await;
    use clax_core::store::approvals::MAX_OPEN_PER_AUTHOR;
    let id = clax_core::ArtifactId::parse(&t.aid).unwrap();
    // Five LAN viewers, each at their own share of Sam's requests.
    let authors: Vec<String> = (0..MAX_OPEN_REQUESTS / MAX_OPEN_PER_AUTHOR)
        .map(|i| {
            t.w.ts
                .store
                .upsert_viewer(
                    &format!("01ARZ3NDEKTSV4RRFFQ69G5F{:02}", i + 10),
                    Some(&format!("Lan{i}")),
                )
                .unwrap()
                .public_id
        })
        .collect();
    for i in 0..MAX_OPEN_REQUESTS {
        let author = &authors[(i / MAX_OPEN_PER_AUTHOR) as usize];
        let th = t
            .w
            .ts
            .store
            .create_thread(
                &AuditCtx::DAEMON,
                &id,
                clax_core::store::threads::NewThread {
                    version_n: 1,
                    anchor: serde_json::from_value(clax_server::testing::element_anchor()).unwrap(),
                    author_name: "Lan".into(),
                    author_public_id: Some(author.clone()),
                    body: format!("t{i}"),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        t.w.ts
            .store
            .send_to(
                &AuditCtx::DAEMON,
                &th.id,
                clax_core::SendTarget::Agent(&t.w.sam_session),
            )
            .unwrap();
    }
    let (_, v) = t
        .sam_browser()
        .call("GET", "/api/approvals/requests?limit=1", None)
        .await;
    assert_eq!(
        (v["open"].clone(), v["banner"].clone()),
        (json!(MAX_OPEN_REQUESTS), Value::Null)
    );
    // The 101st: routing holds it without a request, and People says so.
    let tid = t.mia_thread("one more").await;
    let (status, _) = t.mia_sends_to_sam(&tid).await;
    assert_eq!(status, 200);
    let (_, v) = t
        .sam_browser()
        .call("GET", "/api/approvals/requests?limit=1", None)
        .await;
    assert_eq!(v["open"], json!(MAX_OPEN_REQUESTS));
    assert_eq!(
        v["banner"],
        "More is waiting for your approval than the inbox lists: review People"
    );
    let sid = t.w.sam_session.clone();
    let (status, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    assert_eq!(
        (status, v["error"]["code"].clone()),
        (429, json!("limit_reached")),
        "{v}"
    );
    // Per approver: the owner's agent is still asked for.
    let r =
        t.w.ts
            .authed(t.w.ts.client.put(format!(
                "{}/api/sessions/{}/watches/{}",
                t.w.ts.base, t.w.owner_session, t.aid
            )))
            .header("x-clax-session", &t.w.owner_session)
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200);
    let owner_handle =
        t.w.ts
            .store
            .get_session(&t.w.owner_session)
            .unwrap()
            .unwrap()
            .agent_handle;
    let (status, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": owner_handle})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let owners = t.w.ts.store.approval_requests(None, true, 500).unwrap().0;
    assert!(
        owners
            .iter()
            .any(|r| r.thread_id.as_deref() == Some(tid.as_str())),
        "{owners:?}"
    );
}

#[tokio::test]
async fn a_decision_reaches_a_waiting_agent_and_a_revocation_holds_its_rows() {
    let t = Two::new().await;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    // Told once: the next take finds nothing.
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["withheld"].as_array().map(Vec::len), Some(1), "{fb}");
    let rid = t.sams_requests().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sid = t.w.sam_session.clone();
    // The decision lands between the poll's take and its wait.
    t.w.ts.hold_feedback(&sid, true).await;
    let poll = t.sam_feedback("tier=wait&wait=600");
    let control = async {
        t.w.ts.wait_held_feedback_polls(&sid, 1).await;
        let (status, v) = t.sam_decides(&rid, "thread").await;
        assert_eq!(status, 200, "{v}");
        t.w.ts.hold_feedback(&sid, false).await;
        let v: Value =
            t.w.ts
                .get_authed(&format!(
                    "/api/_test/feedback/{sid}/waiters?parked=wait&until=0"
                ))
                .await
                .json()
                .await
                .unwrap();
        if v["count"] != 0 {
            t.w.ts.expire_feedback(&sid).await;
        }
        v["count"].clone()
    };
    let (v, parked) = tokio::join!(poll, control);
    assert_eq!(parked, 0, "the poll missed the decision: {v}");
    assert_eq!(v["feedback"][0]["body"], "please", "{v}");
    // Revoked while the agent waits: its next take holds what follows.
    let (_, list) = t.sam_browser().call("GET", "/api/approvals", None).await;
    let id = list["approvals"][0]["id"].as_str().unwrap().to_string();
    let poll = t.sam_feedback("tier=wait&wait=600");
    let control = async {
        t.w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        let (status, v) = t
            .sam_browser()
            .call("DELETE", &format!("/api/approvals/{id}"), None)
            .await;
        assert_eq!(status, 200, "{v}");
        // The revocation woke the poll; it took nothing and waits again.
        t.w.ts.wait_parked_feedback_polls(&sid, "wait", 1).await;
        t.w.ts
            .reply_as(&t.aid, &tid, &t.w.mia_cookie, "after")
            .await;
    };
    let (v, ()) = tokio::join!(poll, control);
    assert!(v["feedback"].as_array().unwrap().is_empty(), "{v}");
    assert!(!v.to_string().contains("after"), "{v}");
    assert_eq!(v["withheld"][0]["count"], 1, "{v}");
}

#[tokio::test]
async fn a_decision_pushes_to_the_approvers_codex_session() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("args.fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let bin = clax_fake_exe::install(
        &dir.path().join("codex"),
        "#!/bin/sh\nd=\"$(dirname \"$0\")\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$d/args.fifo\"\nexit 0\n",
    );
    let w = World::two_people_with(move |s| {
        s.codex = std::sync::Arc::new(clax_server::push::CodexPush {
            bin: Some(bin),
            timeout: std::time::Duration::from_secs(10),
            source: clax_server::push::CodexSource::Env,
            ..Default::default()
        })
    })
    .await;
    let t = Two::with(w).await;
    let ts = &t.w.ts;
    let res = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "codex", "parent_pid": 4242, "harness_session_id": "cx-1", "cwd": "/w"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let cx = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{cx}/watches/{}", ts.base, t.aid)),
        )
        .header("x-clax-session", &cx)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let handle = ts.store.get_session(&cx).unwrap().unwrap().agent_handle;
    // Mia sends to the owner's Codex agent: held, nothing queued; the
    // owner is asked.
    let tid = t.mia_thread("Mia, held").await;
    let (status, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": handle})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let rid = ts
        .store
        .approval_requests(None, true, 10)
        .unwrap()
        .0
        .into_iter()
        .find(|r| r.thread_id.as_deref() == Some(tid.as_str()))
        .unwrap()
        .id;
    // The owner approves in a browser: queued at once, with no new comment.
    let reading = {
        let fifo = fifo.clone();
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo))
    };
    let (status, v) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/approvals/requests/{rid}/decide"),
            Some(json!({"scope": "thread"})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let text = match tokio::time::timeout(std::time::Duration::from_secs(60), reading).await {
        Ok(read) => read.unwrap().unwrap(),
        Err(_) => {
            // Release the blocked reader, so the test fails rather than hangs.
            drop(std::fs::OpenOptions::new().write(true).open(&fifo));
            panic!("the decision queued nothing to the owner's Codex session");
        }
    };
    assert!(text.contains("Mia, held"), "{text}");
}

#[tokio::test]
async fn requests_are_recorded_as_approval_events() {
    let t = Two::new().await;
    let before =
        t.w.ts
            .store
            .events_after(0, 100_000)
            .unwrap()
            .last()
            .unwrap()
            .seq;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    let rid = t.sams_requests().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.sam_decides(&rid, "thread").await;
    let rows = t.w.ts.store.events_after(before, 10_000).unwrap();
    let kinds: Vec<&str> = rows
        .iter()
        .map(|r| r.kind.as_str())
        .filter(|k| k.starts_with("approval.") || *k == "thread.send")
        .collect();
    assert_eq!(
        kinds,
        [
            "approval.request",
            "thread.send",
            "approval.grant",
            "approval.decide"
        ],
        "{kinds:?}"
    );
    let send = rows.iter().find(|r| r.kind == "thread.send").unwrap();
    let body: Value = serde_json::from_str(&send.body).unwrap();
    assert_eq!(body["approval_request_ids"], json!([rid]));
    // The router is Mia; the decider Sam.
    let req = rows.iter().find(|r| r.kind == "approval.request").unwrap();
    assert!(
        req.actor.to_string().contains(&t.w.mia_public),
        "{}",
        req.actor
    );
    let dec = rows.iter().find(|r| r.kind == "approval.decide").unwrap();
    assert!(
        dec.actor.to_string().contains(&t.sam_public()),
        "{}",
        dec.actor
    );
    // No request names content or a secret.
    for r in rows.iter().filter(|r| r.kind.starts_with("approval.")) {
        assert!(!r.body.contains("please"), "{}", r.body);
    }
}

/// The owner's agent, watching the artifact; its handle.
async fn owner_agent_watches(t: &Two) -> String {
    let r =
        t.w.ts
            .authed(t.w.ts.client.put(format!(
                "{}/api/sessions/{}/watches/{}",
                t.w.ts.base, t.w.owner_session, t.aid
            )))
            .header("x-clax-session", &t.w.owner_session)
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200);
    t.w.ts
        .store
        .get_session(&t.w.owner_session)
        .unwrap()
        .unwrap()
        .agent_handle
}

#[tokio::test]
async fn one_persons_sends_cannot_block_anothers_request() {
    use clax_core::store::approvals::MAX_OPEN_REQUESTS;
    let t = Two::new().await;
    let owner_handle = owner_agent_watches(&t).await;
    let sam = t.sam_browser();
    let sam_cookie =
        t.w.ts
            .store
            .viewer_of_grant(&t.w.sam.grant_id())
            .unwrap()
            .id;
    // Sam's browser sends a hundred threads to the owner's agent.
    for i in 0..MAX_OPEN_REQUESTS {
        let tid =
            t.w.ts
                .thread_as(&t.aid, &sam_cookie, &format!("spam {i}"))
                .await["id"]
                .as_str()
                .unwrap()
                .to_string();
        let (s, v) = sam
            .call(
                "POST",
                &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
                Some(json!({"to": owner_handle})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    let (_, owners) = t
        .owner_browser()
        .call("GET", "/api/approvals/requests?limit=1", None)
        .await;
    assert_eq!(owners["open"], 20, "Sam's share: {owners}");
    assert!(owners["banner"].is_string(), "the rest wait: {owners}");
    // Mia's send to the owner's agent is still a request.
    let mia_tid = t.mia_thread("Mia needs the owner").await;
    let (s, _) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{mia_tid}/send", t.aid),
            Some(json!({"to": owner_handle})),
        )
        .await;
    assert_eq!(s, 200);
    assert!(
        t.w.ts
            .store
            .approval_requests(None, true, 500)
            .unwrap()
            .0
            .iter()
            .any(|r| r.thread_id.as_deref() == Some(mia_tid.as_str())),
        "Mia's send was held without a request"
    );
    // One decision leaves Sam's held threads without a request: the
    // banner stays.
    let rid = t.w.ts.store.approval_requests(None, true, 1).unwrap().0[0]
        .id
        .clone();
    let (s, _) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/approvals/requests/{rid}/decide"),
            Some(json!({"scope": "decline"})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, owners) = t
        .owner_browser()
        .call("GET", "/api/approvals/requests?limit=1", None)
        .await;
    assert!(owners["banner"].is_string(), "{owners}");
}

#[tokio::test]
async fn the_card_shows_exactly_what_an_approval_releases() {
    let t = Two::new().await;
    let tid = t.mia_thread("Mia's own words").await;
    // Mia's agent replies on her thread, sent to it first.
    let r =
        t.w.mia
            .agent(
                "PUT",
                &format!("/api/sessions/{}/watches/{}", t.w.mia_session, t.aid),
                Some(&t.w.mia_session),
                None,
            )
            .await;
    assert_eq!(r.status(), 200);
    let mh =
        t.w.ts
            .store
            .get_session(&t.w.mia_session)
            .unwrap()
            .unwrap()
            .agent_handle;
    let (s, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": mh})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let r =
        t.w.mia
            .agent(
                "POST",
                &format!("/api/artifacts/{}/threads/{tid}/comments", t.aid),
                Some(&t.w.mia_session),
                Some(json!({"body": "MIA AGENT WORDS", "author_kind": "agent"})),
            )
            .await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    t.mia_sends_to_sam(&tid).await;
    let (_, inbox) = t
        .sam_browser()
        .call("GET", "/api/inbox?kind=approval", None)
        .await;
    let card = &inbox["items"][0]["approval"];
    let bodies: Vec<&str> = card["content"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["Mia's own words", "MIA AGENT WORDS"], "{card}");
    assert_eq!(card["count"], json!(bodies.len()), "{card}");
    // Approved at the thread: Sam's agent reads exactly those.
    let rid = card["id"].as_str().unwrap().to_string();
    let (s, _) = t.sam_decides(&rid, "thread").await;
    assert_eq!(s, 200);
    let (_, v) = t
        .sam_agent(
            "GET",
            &format!("/api/artifacts/{}/threads/{tid}", t.aid),
            None,
        )
        .await;
    let read: Vec<&str> = v["thread"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["body"].as_str())
        .collect();
    assert_eq!(read, bodies, "{v}");
}

#[tokio::test]
async fn a_send_naming_no_agent_asks_no_one() {
    let t = Two::new().await;
    // Mia's thread, sent without naming an agent: Sam's watching agent
    // gets the placeholder, and Sam no card.
    let tid = t.mia_thread("for whoever watches").await;
    let (s, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(t.sams_requests().await.is_empty());
    let fb = t.sam_feedback("tier=piggyback").await;
    let text = fb["text"].as_str().unwrap();
    assert!(text.contains("To ask your person, call ask"), "{text}");
    // The owner's own untargeted send, where Mia replied, asks Sam nothing.
    let own = t.w.ts.thread_as_owner(&t.aid, "the owner's").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.w.ts
        .reply_as(&t.aid, &own, &t.w.mia_cookie, "Mia replies")
        .await;
    let (s, _) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{own}/send", t.aid),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert!(t.sams_requests().await.is_empty());
}

#[tokio::test]
async fn an_agents_lists_leave_out_artifacts_it_may_not_read() {
    let t = Two::new().await;
    let tid = t.mia_thread("please").await;
    t.mia_sends_to_sam(&tid).await;
    let rid = t.sams_requests().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let rights = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": [t.w.scratch.artifact]},
        {"type": "clax-comments", "actions": ["read", "watch"], "locations": ["*"]},
        {"type": "clax-ask", "actions": ["ask"], "locations": ["*"]}
    ]))
    .unwrap();
    t.w.ts
        .store
        .set_person_rights(&AuditCtx::DAEMON, &t.sam_public(), Some(rights))
        .unwrap();
    let (status, v) = t.sam_agent("GET", "/api/approvals/requests", None).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["requests"], json!([]), "{v}");
    assert!(!v.to_string().contains(&t.aid), "{v}");
    let sid = t.w.sam_session.clone();
    let (status, _) = t
        .sam_agent("GET", &format!("/api/sessions/{sid}/approvals/{rid}"), None)
        .await;
    assert_eq!(status, 404);
    // His browser still lists it.
    assert_eq!(t.sams_requests().await.len(), 1);
}

#[tokio::test]
async fn a_card_outside_threads_shows_what_the_author_holds_there() {
    let t = Two::new().await;
    // Sam's agent asks for the owner's artifact outside its threads.
    let sid = t.w.sam_session.clone();
    let (status, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"artifact_id": t.aid})),
        )
        .await;
    assert_eq!(status, 201, "{v}");
    let (_, inbox) = t
        .sam_browser()
        .call("GET", "/api/inbox?kind=approval", None)
        .await;
    let content = &inbox["items"][0]["approval"]["content"];
    assert_eq!(content["title"], "Quarterly Review", "{content}");
    assert_eq!(content["files"], json!(["index.html"]), "{content}");
}

/// Mia's browser sends `tid` to the owner's agent by name: a request in
/// the owner's inbox.
async fn mia_sends_to_the_owners_agent(t: &Two, tid: &str) {
    let handle = owner_agent_watches(t).await;
    let (s, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": handle})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

/// A handoff names an agent (spec §12, M18): handing a thread to another
/// person's agent asks that person, as a send naming it does.
#[tokio::test]
async fn handoff_to_another_persons_agent_makes_a_routing_request() {
    let t = Two::new().await;
    let owner_handle = owner_agent_watches(&t).await;
    // The owner lets Mia's words reach the owner's agents; Sam has not.
    t.w.ts
        .store
        .approve(
            &AuditCtx::DAEMON,
            None,
            &t.w.mia_public,
            clax_core::store::approvals::Scope::Everywhere,
            clax_core::store::approvals::Via::Shell,
        )
        .unwrap();
    let tid = t.mia_thread("please fix the chart").await;
    let (status, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": owner_handle})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    assert!(t.sams_requests().await.is_empty());
    let sam_name =
        t.w.ts
            .store
            .get_session(&t.w.sam_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let owner = &t.w.owner_session;
    let handoff = format!("/api/sessions/{owner}/threads/{tid}/handoff");
    let (status, v) = json_of(
        t.w.ts
            .authed(t.w.ts.client.post(format!("{}{handoff}", t.w.ts.base)))
            .header("x-clax-session", owner)
            .json(&json!({"to": sam_name}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    // Another person's agent: by name and state only (§13).
    assert!(v["thread"]["owner"].get("handle").is_none(), "{v}");
    assert_eq!(v["thread"]["owner"]["name"], json!(sam_name), "{v}");
    assert_eq!(v["thread"]["owner"]["person"], json!(t.sam_public()));
    assert_eq!(v["thread"]["taken_by"], json!(sam_name));
    let rs = t.sams_requests().await;
    assert_eq!(rs.len(), 1, "{rs:?}");
    assert_eq!(rs[0]["thread_id"], json!(tid));
    assert_eq!(rs[0]["cause"], "routing");
    let fb = t.sam_feedback("tier=piggyback").await;
    let text = fb["text"].as_str().unwrap();
    assert!(text.contains("Your person has been asked."), "{text}");
    assert!(!text.contains("please fix the chart"), "{text}");
    // The thread is Sam's agent's now: the owner's agent may not reply.
    let (status, v) = json_of(
        t.w.ts
            .authed(t.w.ts.client.post(format!(
                "{}/api/artifacts/{}/threads/{tid}/comments",
                t.w.ts.base, t.aid
            )))
            .header("x-clax-session", owner)
            .json(&json!({"body": "mine", "author_kind": "agent"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, &v["error"]["code"]),
        (409, &json!("thread_taken")),
        "{v}"
    );
}

/// The owner agent's call: `m path` with `body`, from its session.
async fn owner_agent(t: &Two, m: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut r =
        t.w.ts
            .authed(
                t.w.ts
                    .client
                    .request(m.parse().unwrap(), format!("{}{path}", t.w.ts.base)),
            )
            .header("x-clax-session", &t.w.owner_session);
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// The owner agent's piggyback take: its feedback items.
async fn owner_take(t: &Two) -> Vec<Value> {
    let (s, v) = owner_agent(
        t,
        "GET",
        &format!(
            "/api/sessions/{}/feedback?tier=piggyback",
            t.w.owner_session
        ),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v["feedback"].as_array().cloned().unwrap_or_default()
}

async fn owner_reply(t: &Two, tid: &str) -> (u16, Value) {
    owner_agent(
        t,
        "POST",
        &format!("/api/artifacts/{}/threads/{tid}/comments", t.aid),
        Some(json!({"body": "owner agent reply", "author_kind": "agent"})),
    )
    .await
}

fn approve_mia_for_owner(t: &Two) {
    t.w.ts
        .store
        .approve(
            &AuditCtx::DAEMON,
            None,
            &t.w.mia_public,
            clax_core::store::approvals::Scope::Everywhere,
            clax_core::store::approvals::Via::Shell,
        )
        .unwrap();
}

/// Each feedback row of `tid`: its target and whether it was delivered.
fn rows_of(t: &Two, tid: &str) -> Vec<(Option<String>, bool)> {
    t.w.ts
        .store
        .feedback_rows(tid)
        .unwrap()
        .into_iter()
        .map(|f| (f.target_session_id, f.delivered_at.is_some()))
        .collect()
}

/// An untargeted send of the owner's is first taken by the owner's agents
/// only (O8): Sam's agent may receive it first, but owns nothing, and the
/// owner's agent keeps its row and may reply.
#[tokio::test]
async fn another_persons_first_take_makes_no_owner() {
    let t = Two::new().await;
    owner_agent_watches(&t).await;
    sam_approves_the_owner(&t).await;
    let tid = t.w.ts.thread_as_owner(&t.aid, "OWNERWORDS fix it").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["feedback"].as_array().unwrap().len(), 1, "{fb}");
    assert!(t.owner_thread(&tid).await.get("owner").is_none());
    let own = owner_take(&t).await;
    assert!(own.iter().any(|f| f["thread_id"] == json!(tid)), "{own:?}");
    let (s, v) = owner_reply(&t, &tid).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        t.w.ts.store.thread_owner(&tid).unwrap().unwrap().session_id,
        t.w.owner_session
    );
    // Now Sam's agent may not reply.
    let (s, v) = t
        .sam_agent(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/comments", t.aid),
            Some(json!({"body": "mine", "author_kind": "agent"})),
        )
        .await;
    assert_eq!(
        (s, &v["error"]["code"]),
        (409, &json!("thread_taken")),
        "{v}"
    );
}

/// A reply by an agent that sees only a placeholder takes nothing, and
/// leaves every row in place.
#[tokio::test]
async fn a_blind_reply_by_another_persons_agent_takes_nothing() {
    let t = Two::new().await;
    owner_agent_watches(&t).await;
    approve_mia_for_owner(&t);
    let tid = t.mia_thread("MIAWORDS please").await;
    let (s, v) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let fb = t.sam_feedback("tier=piggyback").await;
    assert!(fb["text"].as_str().unwrap().contains("withheld"), "{fb}");
    let (s, v) = t
        .sam_agent(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/comments", t.aid),
            Some(json!({"body": "mine now", "author_kind": "agent"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert!(v["thread"].get("owner").is_none(), "{v}");
    let own = owner_take(&t).await;
    assert!(
        own.iter()
            .any(|f| f["thread_id"] == json!(tid) && f["body"] == "MIAWORDS please"),
        "the owner's agent still gets it: {own:?}"
    );
    let (s, v) = owner_reply(&t, &tid).await;
    assert_eq!(s, 201, "{v}");
}

/// An agent that could not take an unowned thread may not release it or
/// hand it off: nothing changes.
#[tokio::test]
async fn another_persons_agent_cannot_release_an_unowned_thread() {
    let t = Two::new().await;
    owner_agent_watches(&t).await;
    approve_mia_for_owner(&t);
    let tid = t.mia_thread("MIAWORDS release").await;
    let (s, _) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            None,
        )
        .await;
    assert_eq!(s, 200);
    let before = rows_of(&t, &tid);
    let sid = t.w.sam_session.clone();
    for body in [json!({}), json!({"to": t.sam_handle})] {
        let (s, v) = t
            .sam_agent(
                "POST",
                &format!("/api/sessions/{sid}/threads/{tid}/handoff"),
                Some(body),
            )
            .await;
        assert_eq!(
            (s, &v["error"]["code"]),
            (409, &json!("thread_taken")),
            "{v}"
        );
    }
    assert_eq!(rows_of(&t, &tid), before, "nothing changed");
    let own = owner_take(&t).await;
    assert!(own.iter().any(|f| f["thread_id"] == json!(tid)), "{own:?}");
}

/// Sam's held row survives the owner agent's take, so the approval Sam's
/// agent asked for delivers it.
#[tokio::test]
async fn an_ask_still_delivers_after_another_persons_agent_takes() {
    let t = Two::new().await;
    owner_agent_watches(&t).await;
    approve_mia_for_owner(&t);
    let tid = t.mia_thread("MIAWORDS held").await;
    let (s, _) = t
        .mia_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            None,
        )
        .await;
    assert_eq!(s, 200);
    let _ = t.sam_feedback("tier=piggyback").await;
    let sid = t.w.sam_session.clone();
    let (s, v) = t
        .sam_agent(
            "POST",
            &format!("/api/sessions/{sid}/approvals"),
            Some(json!({"thread_id": tid})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let rid = v["approval_id"].as_str().unwrap().to_string();
    assert!(
        owner_take(&t)
            .await
            .iter()
            .any(|f| f["thread_id"] == json!(tid))
    );
    let (s, v) = t.sam_decides(&rid, "thread").await;
    assert_eq!(s, 200, "{v}");
    let fb = t.sam_feedback("tier=piggyback").await;
    assert_eq!(fb["feedback"][0]["body"], "MIAWORDS held", "{fb}");
}

/// A handoff refused (here, of a resolved thread) gives its target no
/// watch; one that goes through does.
#[tokio::test]
async fn a_refused_handoff_gives_no_watch() {
    let t = Two::new().await;
    owner_agent_watches(&t).await;
    let tid = t.w.ts.thread_as_owner(&t.aid, "resolved one").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let owner_handle =
        t.w.ts
            .store
            .get_session(&t.w.owner_session)
            .unwrap()
            .unwrap()
            .agent_handle;
    let (s, _) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": owner_handle})),
        )
        .await;
    assert_eq!(s, 200);
    t.w.ts
        .store
        .resolve_thread_touched(&AuditCtx::DAEMON, &tid, "viewer:anonymous")
        .unwrap();
    let mia_name =
        t.w.ts
            .store
            .get_session(&t.w.mia_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let watches = |t: &Two| {
        t.w.ts
            .store
            .list_watches(&t.w.mia_session)
            .unwrap()
            .iter()
            .any(|w| w.artifact_id == t.aid)
    };
    assert!(!watches(&t));
    let path = format!("/api/sessions/{}/threads/{tid}/handoff", t.w.owner_session);
    let (s, v) = owner_agent(&t, "POST", &path, Some(json!({"to": mia_name}))).await;
    assert_eq!(
        (s, &v["error"]["code"]),
        (400, &json!("thread_resolved")),
        "{v}"
    );
    assert!(!watches(&t), "a refused handoff gives no watch");
}

/// A handoff naming another agent is a send: an agent without `comments
/// send` may release its thread but not hand it to another.
#[tokio::test]
async fn a_handoff_to_another_agent_needs_comments_send() {
    let t = Two::new().await;
    sam_approves_the_owner(&t).await;
    let rights = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
        {"type": "clax-comments", "actions": ["read", "write", "reply", "resolve", "watch"], "locations": ["*"]},
        {"type": "clax-ask", "actions": ["ask"], "locations": ["*"]}
    ]))
    .unwrap();
    t.w.ts
        .store
        .set_person_rights(&AuditCtx::DAEMON, &t.sam_public(), Some(rights))
        .unwrap();
    owner_agent_watches(&t).await;
    let tid = t.w.ts.thread_as_owner(&t.aid, "for Sam").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": t.sam_handle})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let sid = t.w.sam_session.clone();
    let path = format!("/api/sessions/{sid}/threads/{tid}/handoff");
    let owner_name =
        t.w.ts
            .store
            .get_session(&t.w.owner_session)
            .unwrap()
            .unwrap()
            .name
            .unwrap();
    let (s, v) = t
        .sam_agent("POST", &path, Some(json!({"to": owner_name})))
        .await;
    assert_eq!(
        (s, &v["error"]["code"]),
        (403, &json!("not_permitted")),
        "{v}"
    );
    assert_eq!(v["error"]["right"]["action"], "send");
    let (s, v) = t.sam_agent("POST", &path, Some(json!({}))).await;
    assert_eq!(s, 200, "{v}");
    assert!(t.w.ts.store.thread_owner(&tid).unwrap().is_none());
}

/// Thread views name the owner agent's handle only to its own person's
/// readers and the owner (§13).
#[tokio::test]
async fn owner_handles_are_shown_only_to_their_person() {
    let t = Two::new().await;
    sam_approves_the_owner(&t).await;
    let owner_handle = owner_agent_watches(&t).await;
    let tid =
        t.w.ts
            .thread_as_owner(&t.aid, "for the owner's agent")
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
    let (s, _) = t
        .owner_browser()
        .call(
            "POST",
            &format!("/api/artifacts/{}/threads/{tid}/send", t.aid),
            Some(json!({"to": owner_handle})),
        )
        .await;
    assert_eq!(s, 200);
    let path = format!("/api/artifacts/{}/threads/{tid}", t.aid);
    // The owner's browser and agent see it.
    assert_eq!(
        t.owner_thread(&tid).await["owner"]["handle"],
        json!(owner_handle)
    );
    let (_, v) = owner_agent(&t, "GET", &path, None).await;
    assert_eq!(v["thread"]["owner"]["handle"], json!(owner_handle), "{v}");
    // Sam's agent and browser see its name and state only.
    let (s, v) = t.sam_agent("GET", &path, None).await;
    assert_eq!(s, 200, "{v}");
    let owner = &v["thread"]["owner"];
    assert!(owner.get("handle").is_none(), "{v}");
    assert_eq!(owner["live"], true);
    assert!(v["thread"]["taken_by"].is_string(), "{v}");
    assert!(!v.to_string().contains(&owner_handle), "{v}");
    let (_, v) = t.sam_browser().call("GET", &path, None).await;
    assert!(v["thread"]["owner"].get("handle").is_none(), "{v}");
}

//! Git tokens (hosted spec 2026-10-11-clax-dev-hosted-design §9 "Tokens"):
//! who may have one, of which scope, and what the answer carries, over a
//! host that records what it is asked ([`FakeRemotes`]); the native
//! daemon's answer without one; and the revocation of agents' tokens when
//! their session ends, their rights drop or a version they may not see
//! lands.

use crate::common;
use clax_testkit::people::World;
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_http::git_remotes::{GitRemotes, Held, Holder, Minted, RemoteFuture, Scope};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One token the fake minted.
#[derive(Clone, Debug)]
struct Row {
    id: String,
    artifact: ArtifactId,
    scope: Scope,
    ttl_s: u32,
    origin: Option<String>,
    holder: Holder,
    revoked: bool,
}

/// A host that mints numbered tokens and keeps them, revoking on request.
#[derive(Default)]
struct FakeRemotes {
    rows: Mutex<Vec<Row>>,
    revoked: tokio::sync::Notify,
    /// Each revoke_held call's person and session.
    by_holder: Mutex<Vec<(Option<String>, Option<String>)>>,
    /// Whether its pushes name no pusher (`names_pushers` is `false`).
    anonymous: std::sync::atomic::AtomicBool,
}

impl FakeRemotes {
    fn rows(&self) -> Vec<Row> {
        self.rows.lock().unwrap().clone()
    }

    fn row(&self, id: &str) -> Row {
        self.rows().into_iter().find(|r| r.id == id).unwrap()
    }

    /// Waits until token `id` is revoked (a bound for a lost wake-up, not
    /// a time budget).
    async fn until_revoked(&self, id: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let wake = self.revoked.notified();
            if self.row(id).revoked {
                return;
            }
            tokio::time::timeout_at(deadline, wake)
                .await
                .unwrap_or_else(|_| panic!("token {id} is revoked within 20 s"));
        }
    }
}

impl GitRemotes for FakeRemotes {
    fn names_pushers(&self) -> bool {
        !self.anonymous.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn mint<'a>(
        &'a self,
        artifact: &'a ArtifactId,
        scope: Scope,
        ttl_s: u32,
        origin: Option<&'a str>,
        holder: &'a Holder,
    ) -> RemoteFuture<'a, Minted> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let id = format!("cxt_{}", rows.len() + 1);
            rows.push(Row {
                id: id.clone(),
                artifact: artifact.clone(),
                scope,
                ttl_s,
                origin: origin.map(String::from),
                holder: holder.clone(),
                revoked: false,
            });
            let remote = format!("http://clax.test/git/a-{}.git", artifact.as_str());
            let token = format!("cxm_{}_{id}", scope.as_str());
            Ok(Minted {
                clone_command: format!(
                    "git -c http.extraHeader=\"Authorization: Bearer {token}\" clone {remote}"
                ),
                jj_clone_command: format!("jj git clone {remote}"),
                id,
                remote,
                token,
                expires_at: "2026-10-13T13:00:00.000Z".into(),
            })
        })
    }

    fn held(&self) -> RemoteFuture<'_, Vec<Held>> {
        Box::pin(async move {
            Ok(self
                .rows()
                .into_iter()
                .filter(|r| {
                    !r.revoked
                        && (r.holder.person.is_some()
                            || r.holder.session.is_some()
                            || r.holder.link)
                })
                .map(|r| Held {
                    id: r.id,
                    artifact: r.artifact,
                    scope: r.scope,
                    holder: r.holder,
                })
                .collect())
        })
    }

    fn revoke<'a>(&'a self, _artifact: &'a ArtifactId, id: &'a str) -> RemoteFuture<'a, bool> {
        Box::pin(async move {
            let was = {
                let mut rows = self.rows.lock().unwrap();
                let r = rows.iter_mut().find(|r| r.id == id);
                r.is_some_and(|r| !std::mem::replace(&mut r.revoked, true))
            };
            self.revoked.notify_waiters();
            Ok(was)
        })
    }

    fn revoke_held<'a>(
        &'a self,
        person: Option<&'a str>,
        session: Option<&'a str>,
    ) -> RemoteFuture<'a, usize> {
        Box::pin(async move {
            self.by_holder
                .lock()
                .unwrap()
                .push((person.map(String::from), session.map(String::from)));
            let mut n = 0;
            for r in self.rows.lock().unwrap().iter_mut() {
                let hit = session.is_some_and(|s| r.holder.session.as_deref() == Some(s))
                    || person.is_some_and(|p| r.holder.person.as_deref() == Some(p));
                if hit && !r.revoked {
                    r.revoked = true;
                    n += 1;
                }
            }
            self.revoked.notify_waiters();
            Ok(n)
        })
    }
}

async fn served() -> (TestServer, Arc<FakeRemotes>) {
    let fake = Arc::new(FakeRemotes::default());
    let f = fake.clone();
    let ts = TestServer::spawn_with(move |s| s.git_remotes.set(f)).await;
    (ts, fake)
}

fn aid_of(v: &Value) -> String {
    v["artifact"]["id"].as_str().unwrap().to_string()
}

/// `POST /api/artifacts/<aid>/git-token {scope}` with `cookie` (none: the
/// owner's token).
async fn browser(ts: &TestServer, aid: &str, scope: &str, cookie: Option<&str>) -> (u16, Value) {
    let req = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/git-token", ts.base))
        .json(&json!({"scope": scope}));
    let req = match cookie {
        Some(c) => req.header("cookie", c),
        None => ts.authed(req),
    };
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

async fn options(ts: &TestServer, aid: &str, cookie: Option<&str>) -> Value {
    let req = ts
        .client
        .get(format!("{}/api/artifacts/{aid}/git-token", ts.base));
    let req = match cookie {
        Some(c) => req.header("cookie", c),
        None => ts.authed(req),
    };
    let r = req.send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

/// `POST /api/sessions/<sid>/git-token/<aid> {scope}` with the owner's
/// token.
async fn owner_agent(ts: &TestServer, sid: &str, aid: &str, scope: &str) -> (u16, Value) {
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/git-token/{aid}", ts.base))
                .json(&json!({"scope": scope})),
        )
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_native_daemon_serves_no_git_remotes() {
    let ts = TestServer::spawn().await;
    let aid = aid_of(&ts.publish("Plain", &[("index.html", "<p>x</p>")]).await);
    let (status, v) = browser(&ts, &aid, "read", None).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (501, Some("not_available")),
        "{v}"
    );
    assert_eq!(
        options(&ts, &aid, None).await,
        json!({"available": false, "scopes": []})
    );
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = owner_agent(&ts, &sid, &aid, "read").await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (501, Some("not_available")),
        "{v}"
    );
}

#[tokio::test]
async fn the_owner_gets_read_and_write_tokens_with_the_commands_that_use_them() {
    let (ts, fake) = served().await;
    let aid = aid_of(&ts.publish("Plan", &[("index.html", "<p>x</p>")]).await);
    assert_eq!(
        options(&ts, &aid, None).await,
        json!({"available": true, "scopes": ["read", "write"], "ttl_s": 3600})
    );
    let (status, v) = browser(&ts, &aid, "write", None).await;
    assert_eq!(status, 200, "{v}");
    let remote = format!("http://clax.test/git/a-{aid}.git");
    let token = v["token"].as_str().unwrap();
    assert_eq!(v["remote"], remote.as_str());
    assert_eq!(v["scope"], "write");
    assert_eq!(v["artifact_id"], aid.as_str());
    assert!(
        v["clone_command"]
            .as_str()
            .unwrap()
            .ends_with(&format!("clone {remote}"))
    );
    let header = format!("http.extraHeader=\"Authorization: Bearer {token}\"");
    let c = &v["existing_checkout"];
    assert_eq!(c["remote_name"], "clax");
    assert_eq!(
        c["add_remote_command"],
        format!("git remote add clax {remote}")
    );
    assert_eq!(c["fetch_command"], format!("git -c {header} fetch clax"));
    assert_eq!(
        c["push_command"],
        format!("git -c {header} push clax HEAD:main")
    );
    assert_eq!(
        c["jj_add_remote_command"],
        format!("jj git remote add clax {remote}")
    );
    let jj = format!(
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0=\"Authorization: Bearer {token}\" jj"
    );
    assert_eq!(
        c["jj_fetch_command"],
        format!("{jj} git fetch --remote clax")
    );
    assert_eq!(
        c["jj_push_command"],
        format!("{jj} git push --remote clax --bookmark main")
    );
    // Only the add-remote command names the remote's URL, and none the
    // token in it.
    for k in ["add_remote_command", "jj_add_remote_command"] {
        assert!(!c[k].as_str().unwrap().contains(token), "{k}");
    }
    let row = fake.row(v["id"].as_str().unwrap_or("cxt_1"));
    assert_eq!((row.scope, row.ttl_s), (Scope::Write, 3600));
    assert_eq!(
        row.holder,
        Holder::default(),
        "the owner's token is no person's"
    );
    assert_eq!(
        row.origin.as_deref(),
        Some(ts.base.as_str()),
        "the request's own origin"
    );

    let (status, v) = browser(&ts, &aid, "read", None).await;
    assert_eq!(status, 200, "{v}");
    assert!(v["existing_checkout"].get("push_command").is_none(), "{v}");
    assert!(
        v["existing_checkout"].get("jj_push_command").is_none(),
        "{v}"
    );

    // From the owner's browser too, with its cookie.
    let (status, _) = browser(&ts, &aid, "read", Some(&ts.owner_cookie())).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_token_request_is_refused_without_a_scope_an_artifact_or_the_same_origin() {
    let (ts, fake) = served().await;
    let aid = aid_of(&ts.publish("Plan", &[("index.html", "<p>x</p>")]).await);
    let (status, v) = browser(&ts, &aid, "admin", None).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (400, Some("invalid_args")),
        "{v}"
    );
    let (status, _) = browser(&ts, "000000000000", "read", None).await;
    assert_eq!(status, 404);
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/git-token", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", "http://evil.test")
        .json(&json!({"scope": "read"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(fake.rows().is_empty(), "{:?}", fake.rows());
}

#[tokio::test]
async fn people_get_tokens_by_their_rights_and_guests_and_no_credential_get_none() {
    let (ts, fake) = served().await;
    let owners = aid_of(&ts.publish("Owner's", &[("index.html", "<p>x</p>")]).await);
    let rae = ts.person("Rae").await;
    let rae_cookie = format!("clax_viewer={}", rae.cookie);
    // Rae's own artifact: she created it.
    let hers = aid_of(&ts.publish("Rae's", &[("index.html", "<p>r</p>")]).await);
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE artifacts SET person = ?2 WHERE id = ?1",
        rusqlite::params![hers, rae.public_id],
    )
    .unwrap();
    drop(db);

    // clax:collaborator, a person's default: clone everywhere, push to
    // her own.
    assert_eq!(
        options(&ts, &owners, Some(&rae_cookie)).await,
        json!({"available": true, "scopes": ["read"], "ttl_s": 3600})
    );
    assert_eq!(
        options(&ts, &hers, Some(&rae_cookie)).await["scopes"],
        json!(["read", "write"])
    );
    let (status, v) = browser(&ts, &owners, "read", Some(&rae_cookie)).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        fake.row("cxt_1").holder,
        Holder {
            person: Some(rae.public_id.clone()),
            session: None,
            link: false
        }
    );
    let (status, v) = browser(&ts, &owners, "write", Some(&rae_cookie)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("not_permitted")),
        "{v}"
    );
    assert_eq!(
        v["error"]["right"],
        json!({"type": "clax-artifacts", "action": "update"})
    );
    let (status, v) = browser(&ts, &hers, "write", Some(&rae_cookie)).await;
    assert_eq!(status, 200, "{v}");

    // A reader's rights: read only, even on her own.
    ts.store
        .set_person_rights(
            &AuditCtx::DAEMON,
            &rae.public_id,
            Some(clax_core::rights::Rights::preset("clax:reader").unwrap()),
        )
        .unwrap();
    let (status, _) = browser(&ts, &hers, "write", Some(&rae_cookie)).await;
    assert_eq!(status, 403);

    // A guest (a named viewer who is not a person).
    let guest = ts.viewer(Some("Gil")).await;
    let guest_cookie = format!("clax_viewer={}", guest.cookie);
    let (status, v) = browser(&ts, &owners, "read", Some(&guest_cookie)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("sign_in_required")),
        "{v}"
    );
    let o = options(&ts, &owners, Some(&guest_cookie)).await;
    assert_eq!(o["scopes"], json!([]));
    assert_eq!(o["reason"]["code"], "sign_in_required");

    // No credential.
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{owners}/git-token", ts.base))
        .json(&json!({"scope": "read"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(fake.rows().len(), 2);
}

#[tokio::test]
async fn a_host_that_names_no_pusher_gives_write_tokens_to_the_owner_alone() {
    let (ts, fake) = served().await;
    fake.anonymous
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let rae = ts.person("Rae").await;
    let rae_cookie = format!("clax_viewer={}", rae.cookie);
    let hers = aid_of(&ts.publish("Rae's", &[("index.html", "<p>r</p>")]).await);
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE artifacts SET person = ?2 WHERE id = ?1",
        rusqlite::params![hers, rae.public_id],
    )
    .unwrap();
    drop(db);

    // Her push would be recorded as the owner's: she may clone, not push.
    assert_eq!(
        options(&ts, &hers, Some(&rae_cookie)).await["scopes"],
        json!(["read"])
    );
    let (status, v) = browser(&ts, &hers, "write", Some(&rae_cookie)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("push_unattributed")),
        "{v}"
    );
    let (status, v) = browser(&ts, &hers, "read", Some(&rae_cookie)).await;
    assert_eq!(status, 200, "{v}");

    // The owner and the owner's agents still push.
    assert_eq!(
        options(&ts, &hers, None).await["scopes"],
        json!(["read", "write"])
    );
    let (status, v) = browser(&ts, &hers, "write", None).await;
    assert_eq!(status, 200, "{v}");
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = owner_agent(&ts, &sid, &hers, "write").await;
    assert_eq!(status, 200, "{v}");
    let scopes: Vec<Scope> = fake.rows().iter().map(|r| r.scope).collect();
    assert_eq!(scopes, [Scope::Read, Scope::Write, Scope::Write]);
}

#[tokio::test]
async fn a_spectator_gets_no_token() {
    let (ts, fake) = served().await;
    let aid = aid_of(&ts.publish("Busy", &[("index.html", "<p>x</p>")]).await);
    ts.seats.set_limits(clax_core::seats::SeatLimits {
        people: 0,
        ..ts.seats.limits()
    });
    let rae = ts.person("Rae").await;
    let (status, v) = browser(
        &ts,
        &aid,
        "read",
        Some(&format!("clax_viewer={}", rae.cookie)),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("spectator")),
        "{v}"
    );
    assert!(fake.rows().is_empty());
}

#[tokio::test]
async fn agents_ask_from_their_session_and_never_on_the_browser_route() {
    let (ts, fake) = served().await;
    let aid = aid_of(&ts.publish("Plan", &[("index.html", "<p>x</p>")]).await);
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = owner_agent(&ts, &sid, &aid, "write").await;
    assert_eq!(status, 200, "{v}");
    let row = fake.row("cxt_1");
    assert_eq!(row.ttl_s, 900, "an agent's token lives 15 minutes");
    assert_eq!(
        row.holder,
        Holder {
            person: None,
            session: Some(sid.clone()),
            link: false
        }
    );
    // The browser route refuses the owner's agents.
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/git-token", ts.base))
                .header("x-clax-session", &sid)
                .json(&json!({"scope": "read"})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "not_for_agents");
    // No session, an ended one.
    let (status, _) = owner_agent(&ts, "s_missing", &aid, "read").await;
    assert_eq!(status, 404);
    ts.store.end_session(&AuditCtx::DAEMON, &sid).unwrap();
    let (status, v) = owner_agent(&ts, &sid, &aid, "read").await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (400, Some("unknown_session")),
        "{v}"
    );
}

#[tokio::test]
async fn an_agents_token_is_revoked_when_its_session_ends() {
    let (ts, fake) = served().await;
    let aid = aid_of(&ts.publish("Plan", &[("index.html", "<p>x</p>")]).await);
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other = ts.register_session("claude", "h2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(owner_agent(&ts, &sid, &aid, "read").await.0, 200);
    assert_eq!(owner_agent(&ts, &other, &aid, "read").await.0, 200);
    let (status, _) = browser(&ts, &aid, "read", None).await;
    assert_eq!(status, 200);
    // The session ends itself: the end revokes its tokens by holder.
    let r = ts
        .authed(
            ts.client
                .patch(format!("{}/api/sessions/{sid}", ts.base))
                .json(&json!({"ended": true})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert!(fake.row("cxt_1").revoked, "revoked before the end answered");
    assert!(
        fake.by_holder
            .lock()
            .unwrap()
            .contains(&(None, Some(sid.clone()))),
        "{:?}",
        fake.by_holder.lock().unwrap()
    );
    assert!(
        !fake.row("cxt_2").revoked,
        "another session's token lives on"
    );
    assert!(!fake.row("cxt_3").revoked, "the owner's own token lives on");
    // A session the store ends by another path: the recheck finds it.
    ts.store.end_session(&AuditCtx::DAEMON, &other).unwrap();
    fake.until_revoked("cxt_2").await;
    assert!(!fake.row("cxt_3").revoked);
}

#[tokio::test]
async fn a_persons_tokens_are_revoked_when_their_rights_narrow_or_they_are_revoked() {
    let (ts, fake) = served().await;
    let rae = ts.person("Rae").await;
    let hers = aid_of(&ts.publish("Rae's", &[("index.html", "<p>r</p>")]).await);
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE artifacts SET person = ?2 WHERE id = ?1",
        rusqlite::params![hers, rae.public_id],
    )
    .unwrap();
    drop(db);
    let cookie = format!("clax_viewer={}", rae.cookie);
    assert_eq!(browser(&ts, &hers, "write", Some(&cookie)).await.0, 200);
    assert_eq!(browser(&ts, &hers, "read", Some(&cookie)).await.0, 200);
    let r = ts
        .client
        .put(format!("{}/api/people/{}/rights", ts.base, rae.public_id))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"rights": ["clax:reader"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // A narrowing revokes all she holds before the route answers.
    assert!(fake.row("cxt_1").revoked && fake.row("cxt_2").revoked);
    assert_eq!(
        *fake.by_holder.lock().unwrap(),
        [(Some(rae.public_id.clone()), None)]
    );
    // A widening revokes nothing.
    assert_eq!(browser(&ts, &hers, "read", Some(&cookie)).await.0, 200);
    let r = ts
        .client
        .put(format!("{}/api/people/{}/rights", ts.base, rae.public_id))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"rights": ["clax:full"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(!fake.row("cxt_3").revoked);
    assert_eq!(fake.by_holder.lock().unwrap().len(), 1);
    // Revoking her revokes the rest.
    let r = ts
        .client
        .delete(format!("{}/api/people/{}", ts.base, rae.public_id))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert!(fake.row("cxt_3").revoked);
}

#[tokio::test]
async fn an_agent_whose_person_may_not_see_an_author_gets_no_token_and_loses_one_when_such_work_lands()
 {
    let fake = Arc::new(FakeRemotes::default());
    let f = fake.clone();
    let w = World::two_people_with(move |s| s.git_remotes.set(f)).await;
    let ts = &w.ts;
    // Sam approves the owner's work for his agents.
    let sam_public = ts
        .store
        .viewer_of_grant(&w.sam.grant_id())
        .unwrap()
        .public_id;
    let owner_public = ts.owner_public_id().await;
    ts.store
        .approve(
            &AuditCtx::DAEMON,
            Some(&sam_public),
            &owner_public,
            clax_store::approvals::Scope::Everywhere,
            clax_store::approvals::Via::Shell,
        )
        .unwrap();
    // Mia's words are in every version of her artifact, and no one
    // approved her: neither Sam's agent nor the owner's may clone it.
    let mias = &w.read.artifact;
    let r = w
        .sam
        .agent(
            "POST",
            &format!("/api/sessions/{}/git-token/{mias}", w.sam_session),
            Some(&w.sam_session),
            Some(json!({"scope": "read"})),
        )
        .await;
    assert_eq!(r.status(), 403);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "withheld", "{v}");
    assert_eq!(
        v["error"]["withheld_authors"],
        json!([{"public_id": w.mia_public, "name": "Mia"}])
    );
    assert!(
        v["error"]["message"].as_str().unwrap().contains("Mia"),
        "{v}"
    );
    let (status, v) = owner_agent(ts, &w.owner_session, mias, "read").await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("withheld")),
        "{v}"
    );
    assert!(fake.rows().is_empty());
    // Sam's own browser may: people see everything.
    let sam_cookie = {
        let v = ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
        format!("clax_viewer={}", ts.browser_cookie(&v.id))
    };
    assert_eq!(browser(ts, mias, "read", Some(&sam_cookie)).await.0, 200);

    // An artifact only the owner wrote: Sam's agent clones it, and may not
    // push (its rights update only Sam's own).
    let plain = aid_of(&ts.publish("Owner's", &[("index.html", "<p>o</p>")]).await);
    let r = w
        .sam
        .agent(
            "POST",
            &format!("/api/sessions/{}/git-token/{plain}", w.sam_session),
            Some(&w.sam_session),
            Some(json!({"scope": "read"})),
        )
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let sams = fake.rows().last().unwrap().clone();
    assert_eq!(sams.ttl_s, 900);
    assert_eq!(sams.holder.session.as_deref(), Some(w.sam_session.as_str()));
    assert!(
        sams.holder.person.is_some(),
        "the token is Sam's agent's, by Sam"
    );
    let r = w
        .sam
        .agent(
            "POST",
            &format!("/api/sessions/{}/git-token/{plain}", w.sam_session),
            Some(&w.sam_session),
            Some(json!({"scope": "write"})),
        )
        .await;
    assert_eq!(r.status(), 403);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "not_permitted", "{v}");
    // The owner's agent too.
    let (status, v) = owner_agent(ts, &w.owner_session, &plain, "read").await;
    assert_eq!(status, 200, "{v}");
    let owners = fake.rows().last().unwrap().id.clone();

    // A version by Mia lands on it: both agents' tokens are revoked.
    let r = ts
        .post_json(
            &format!("/api/artifacts/{plain}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<p>Mia's turn</p>"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE versions SET person = ?2, file_authors_json = ?3 WHERE artifact_id = ?1 AND n = 2",
        rusqlite::params![
            plain,
            w.mia_public,
            json!({"index.html": [w.mia_public]}).to_string()
        ],
    )
    .unwrap();
    drop(db);
    // The written authorship is announced as the version event announces a publish.
    ts.events.publish(clax_core::Event::Version {
        artifact_id: plain.clone(),
        n: 2,
        by_page: false,
        title: None,
        at: None,
    });
    fake.until_revoked(&sams.id).await;
    fake.until_revoked(&owners).await;

    // Sam narrows his machine's grant: its agents' tokens go at once.
    let third = aid_of(&ts.publish("Third", &[("index.html", "<p>3</p>")]).await);
    let r = w
        .sam
        .agent(
            "POST",
            &format!("/api/sessions/{}/git-token/{third}", w.sam_session),
            Some(&w.sam_session),
            Some(json!({"scope": "read"})),
        )
        .await;
    assert_eq!(r.status(), 200);
    let latest = fake.rows().last().unwrap().id.clone();
    let r = ts
        .client
        .put(format!(
            "{}/api/people/me/grants/{}/narrow",
            ts.base,
            w.sam.grant_id()
        ))
        .header("cookie", &sam_cookie)
        .json(&json!({"access": ["clax:reader"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert!(fake.row(&latest).revoked);
    assert!(
        fake.by_holder
            .lock()
            .unwrap()
            .contains(&(None, Some(w.sam_session.clone())))
    );
}

#[tokio::test]
async fn an_agent_whose_person_may_not_see_an_author_of_a_forks_source_gets_no_token_for_the_fork()
{
    let fake = Arc::new(FakeRemotes::default());
    let f = fake.clone();
    let w = World::two_people_with(move |s| s.git_remotes.set(f)).await;
    let ts = &w.ts;
    let sam_public = ts
        .store
        .viewer_of_grant(&w.sam.grant_id())
        .unwrap()
        .public_id;
    let owner_public = ts.owner_public_id().await;
    ts.store
        .approve(
            &AuditCtx::DAEMON,
            Some(&sam_public),
            &owner_public,
            clax_store::approvals::Scope::Everywhere,
            clax_store::approvals::Via::Shell,
        )
        .unwrap();
    // The source: the owner's, then a version of Mia's, then the owner's
    // again over every file.
    let src = aid_of(&ts.publish("Source", &[("index.html", "<p>one</p>")]).await);
    for (n, body) in [(1, "<p>Mia's two</p>"), (2, "<p>three</p>")] {
        let r = ts
            .post_json(
                &format!("/api/artifacts/{src}/versions"),
                json!({"if_version": n, "files": {"index.html": {"content": body}}}),
            )
            .await;
        assert_eq!(r.status(), 201);
    }
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE versions SET person = ?2, file_authors_json = ?3 WHERE artifact_id = ?1 AND n = 2",
        rusqlite::params![
            src,
            w.mia_public,
            json!({"index.html": [w.mia_public]}).to_string()
        ],
    )
    .unwrap();
    drop(db);
    let ask = |aid: String| {
        let w = &w;
        async move {
            let r = w
                .sam
                .agent(
                    "POST",
                    &format!("/api/sessions/{}/git-token/{aid}", w.sam_session),
                    Some(&w.sam_session),
                    Some(json!({"scope": "read"})),
                )
                .await;
            let status = r.status().as_u16();
            (status, r.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    // A fork of version 3 holds only the owner's words, and its
    // repository the source's history up to version 3, Mia's included.
    let (fork, v) = ts
        .store
        .fork_artifact(
            &AuditCtx::DAEMON,
            &ArtifactId::parse(&src).unwrap(),
            Some(3),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        clax_core::visibility::file_authors(&v.authors, "index.html"),
        [clax_core::visibility::Author::Owner]
    );
    let (status, v) = ask(fork.id.clone()).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("withheld")),
        "{v}"
    );
    assert_eq!(
        v["error"]["withheld_authors"],
        json!([{"public_id": w.mia_public, "name": "Mia"}])
    );
    // A fork of version 1, before Mia's: Sam's agent clones it.
    let (early, _) = ts
        .store
        .fork_artifact(
            &AuditCtx::DAEMON,
            &ArtifactId::parse(&src).unwrap(),
            Some(1),
            None,
            None,
        )
        .unwrap();
    let (status, v) = ask(early.id.clone()).await;
    assert_eq!(status, 200, "{v}");
    // With the source's row gone, the history's authors cannot be named.
    let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute_batch(&format!(
        "PRAGMA foreign_keys = OFF; UPDATE artifacts SET forked_from_id = 'zzzzzzzzzzzz' WHERE id = '{}';",
        early.id
    ))
    .unwrap();
    drop(db);
    let (status, v) = ask(early.id.clone()).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("withheld")),
        "{v}"
    );
    assert_eq!(v["error"]["withheld_authors"], json!([]));
    assert_eq!(
        fake.rows().len(),
        1,
        "only the early fork's token was minted"
    );
}

#[tokio::test]
async fn a_withdrawn_approval_or_an_ended_grant_revokes_tokens_before_its_route_answers() {
    let fake = Arc::new(FakeRemotes::default());
    let f = fake.clone();
    let w = World::two_people_with(move |s| s.git_remotes.set(f)).await;
    let ts = &w.ts;
    let sam_public = ts
        .store
        .viewer_of_grant(&w.sam.grant_id())
        .unwrap()
        .public_id;
    let sam_cookie = {
        let v = ts.store.viewer_of_grant(&w.sam.grant_id()).unwrap();
        format!("clax_viewer={}", ts.browser_cookie(&v.id))
    };
    let owner_public = ts.owner_public_id().await;
    let approve = |author: &str| {
        ts.store
            .approve(
                &AuditCtx::DAEMON,
                Some(&sam_public),
                author,
                clax_store::approvals::Scope::Everywhere,
                clax_store::approvals::Via::Shell,
            )
            .unwrap()
    };
    approve(&owner_public);
    let mias_approval = approve(&w.mia_public);
    let ask = |aid: String| {
        let w = &w;
        async move {
            w.sam
                .agent(
                    "POST",
                    &format!("/api/sessions/{}/git-token/{aid}", w.sam_session),
                    Some(&w.sam_session),
                    Some(json!({"scope": "read"})),
                )
                .await
                .status()
                .as_u16()
        }
    };
    // With Mia approved, Sam's agent clones her artifact.
    assert_eq!(ask(w.read.artifact.clone()).await, 200);
    let mias = fake.rows().last().unwrap().id.clone();
    let plain = aid_of(&ts.publish("Owner's", &[("index.html", "<p>o</p>")]).await);
    assert_eq!(ask(plain.clone()).await, 200);
    let owners = fake.rows().last().unwrap().id.clone();
    // Sam withdraws the approval: the token is gone when the route answers.
    let r = ts
        .client
        .delete(format!("{}/api/approvals/{}", ts.base, mias_approval.id))
        .header("cookie", &sam_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert!(fake.row(&mias).revoked, "revoked before the route answered");
    assert!(
        !fake.row(&owners).revoked,
        "the owner's work is still approved"
    );
    // The owner ends Sam's grant: its session's token goes with it.
    let r = ts
        .client
        .delete(format!("{}/api/grants/{}", ts.base, w.sam.grant_id()))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert!(
        fake.row(&owners).revoked,
        "revoked before the route answered"
    );
}

#[tokio::test]
async fn a_person_who_may_read_a_fork_but_not_its_source_gets_no_token_for_the_fork() {
    let (ts, fake) = served().await;
    let src = aid_of(&ts.publish("Source", &[("index.html", "<p>one</p>")]).await);
    let r = ts
        .post_json(
            &format!("/api/artifacts/{src}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<p>two</p>"}}}),
        )
        .await;
    assert_eq!(r.status(), 201);
    let (fork, _) = ts
        .store
        .fork_artifact(
            &AuditCtx::DAEMON,
            &ArtifactId::parse(&src).unwrap(),
            Some(2),
            None,
            None,
        )
        .unwrap();
    let rae = ts.person("Rae").await;
    let rae_cookie = format!("clax_viewer={}", rae.cookie);
    let read_at = |ids: &[&str]| {
        clax_core::rights::Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": ids}
        ]))
        .unwrap()
    };
    // The fork's repository holds the source's versions 1 and 2.
    ts.store
        .set_person_rights(
            &AuditCtx::DAEMON,
            &rae.public_id,
            Some(read_at(&[fork.id.as_str()])),
        )
        .unwrap();
    assert_eq!(
        options(&ts, &fork.id, Some(&rae_cookie)).await["scopes"],
        json!([])
    );
    let (status, v) = browser(&ts, &fork.id, "read", Some(&rae_cookie)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("not_permitted")),
        "{v}"
    );
    assert!(fake.rows().is_empty());
    // With read at the source too, she clones the fork.
    ts.store
        .set_person_rights(
            &AuditCtx::DAEMON,
            &rae.public_id,
            Some(read_at(&[fork.id.as_str(), src.as_str()])),
        )
        .unwrap();
    let (status, v) = browser(&ts, &fork.id, "read", Some(&rae_cookie)).await;
    assert_eq!(status, 200, "{v}");
}

// ── Pages shared by link (hosted) ───────────────────────────────────────────

/// A hosted daemon serving git remotes through `fake`, on a stopped clock.
struct Hosted {
    ts: TestServer,
    clock: Arc<clax_core::working::ManualClock>,
    fake: Arc<FakeRemotes>,
    host: String,
    origin: String,
}

impl Hosted {
    async fn new() -> Hosted {
        let clock = Arc::new(clax_core::working::ManualClock::at("2026-10-12T10:00:00Z"));
        let fake = Arc::new(FakeRemotes::default());
        let f = fake.clone();
        let ts = TestServer::spawn_hosted_with(clock.clone(), Some("the secret"), move |s| {
            s.git_remotes.set(f)
        })
        .await;
        let host = format!("localhost:{}", ts.addr.port());
        let origin = format!("http://{host}");
        Hosted {
            ts,
            clock,
            fake,
            host,
            origin,
        }
    }

    /// A request as a browser at the public origin makes it, with session
    /// cookie `cookie`.
    fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let mut r = self
            .ts
            .client
            .request(method.clone(), format!("{}{path}", self.ts.base))
            .header("host", &self.host);
        if method != reqwest::Method::GET {
            r = r.header("origin", &self.origin);
        }
        if let Some(c) = cookie {
            r = r.header("cookie", format!("clax_session={c}"));
        }
        r
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value, Option<String>) {
        let mut r = self.req(method, path, cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let set = r
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|c| c.strip_prefix("clax_session="))
            .map(|c| c.split(';').next().unwrap().to_string());
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null), set)
    }

    /// The owner's session cookie, signed in through setup.
    async fn owner(&self) -> String {
        let (s, v, c) = self
            .send(
                reqwest::Method::POST,
                "/api/hosted/setup",
                None,
                Some(json!({"secret": "the secret", "name": "Ada"})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        c.unwrap()
    }

    /// A guest named `name`'s session cookie.
    async fn guest(&self, name: &str) -> String {
        let (s, v, c) = self
            .send(
                reqwest::Method::PUT,
                "/api/viewers/me",
                None,
                Some(json!({"display_name": name})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        c.unwrap()
    }

    async fn share(&self, owner: &str, aid: &str, access: Option<&str>) {
        let (s, v, _) = self
            .send(
                reqwest::Method::PUT,
                &format!("/api/hosted/artifacts/{aid}/link"),
                Some(owner),
                Some(json!({"access": access})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    async fn options(&self, aid: &str, cookie: Option<&str>) -> (u16, Value) {
        let (s, v, _) = self
            .send(
                reqwest::Method::GET,
                &format!("/api/artifacts/{aid}/git-token"),
                cookie,
                None,
            )
            .await;
        (s, v)
    }

    async fn mint(&self, aid: &str, scope: &str, cookie: Option<&str>) -> (u16, Value) {
        let (s, v, _) = self
            .send(
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/git-token"),
                cookie,
                Some(json!({"scope": scope})),
            )
            .await;
        (s, v)
    }
}

fn err(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[tokio::test]
async fn anyone_who_reads_a_page_through_its_link_clones_it_read_only() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let aid = aid_of(&h.ts.publish("Plan", &[("index.html", "<p>one</p>")]).await);
    let other = aid_of(&h.ts.publish("Other", &[("index.html", "<p>two</p>")]).await);
    // Unshared: the page is not there for a visitor.
    assert_eq!(h.options(&aid, None).await.0, 404);
    assert_eq!(h.mint(&aid, "read", None).await.0, 404);
    h.share(&owner, &aid, Some("viewer")).await;
    // A visitor who gave no name: read alone, living as short as an agent's.
    let (s, o) = h.options(&aid, None).await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(o["available"], true);
    assert_eq!(o["scopes"], json!(["read"]));
    assert_eq!(o["ttl_s"], clax_http::git_remotes::LINK_TTL_S);
    assert_eq!(o["link"], true);
    let (s, v) = h.mint(&aid, "read", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["scope"], "read");
    assert!(v["existing_checkout"].get("push_command").is_none(), "{v}");
    let row = h.fake.rows().pop().unwrap();
    assert_eq!(row.artifact.as_str(), aid);
    assert_eq!(row.scope, Scope::Read);
    assert_eq!(row.ttl_s, clax_http::git_remotes::LINK_TTL_S);
    assert_eq!(
        row.holder,
        Holder {
            link: true,
            ..Holder::default()
        }
    );
    // Never write.
    let (s, v) = h.mint(&aid, "write", None).await;
    assert_eq!((s, err(&v)), (403, Some("not_permitted")), "{v}");
    // A named guest: the same, on a commenter's link too.
    h.share(&owner, &aid, Some("commenter")).await;
    let gil = h.guest("Gil").await;
    let (_, o) = h.options(&aid, Some(&gil)).await;
    assert_eq!(o["scopes"], json!(["read"]));
    let (s, v) = h.mint(&aid, "read", Some(&gil)).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = h.mint(&aid, "write", Some(&gil)).await;
    assert_eq!((s, err(&v)), (403, Some("not_permitted")), "{v}");
    // That page only.
    assert_eq!(h.mint(&other, "read", Some(&gil)).await.0, 404);
    // A browser change still needs the public origin.
    let r =
        h.ts.client
            .post(format!("{}/api/artifacts/{aid}/git-token", h.ts.base))
            .header("host", &h.host)
            .json(&json!({"scope": "read"}))
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 403);
    // Stopping sharing revokes the link's tokens before it answers.
    let minted: Vec<String> = h
        .fake
        .rows()
        .into_iter()
        .filter(|r| r.holder.link)
        .map(|r| r.id)
        .collect();
    assert_eq!(minted.len(), 2);
    h.share(&owner, &aid, None).await;
    for id in &minted {
        assert!(h.fake.row(id).revoked, "{id}");
    }
    assert_eq!(h.mint(&aid, "read", Some(&gil)).await.0, 404);
}

#[tokio::test]
async fn a_link_to_a_fork_does_not_open_its_source() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let src = aid_of(
        &h.ts
            .publish("Source", &[("index.html", "<p>one</p>")])
            .await,
    );
    let (fork, _) =
        h.ts.store
            .fork_artifact(
                &AuditCtx::DAEMON,
                &ArtifactId::parse(&src).unwrap(),
                Some(1),
                None,
                None,
            )
            .unwrap();
    let fork = fork.id.as_str().to_string();
    h.share(&owner, &fork, Some("viewer")).await;
    let (s, o) = h.options(&fork, None).await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(o["scopes"], json!([]));
    assert_eq!(o["reason"]["code"], "not_permitted");
    let (s, v) = h.mint(&fork, "read", None).await;
    assert_eq!((s, err(&v)), (403, Some("not_permitted")), "{v}");
    assert!(h.fake.rows().is_empty());
    // With the source shared too, the fork clones; stopping the source's
    // link revokes the fork's token.
    h.share(&owner, &src, Some("viewer")).await;
    let (s, v) = h.mint(&fork, "read", None).await;
    assert_eq!(s, 200, "{v}");
    let id = h.fake.rows()[0].id.clone();
    h.share(&owner, &src, None).await;
    assert!(h.fake.row(&id).revoked);
}

#[tokio::test]
async fn a_links_tokens_are_limited_per_client_on_the_clock() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let aid = aid_of(&h.ts.publish("Plan", &[("index.html", "<p>one</p>")]).await);
    h.share(&owner, &aid, Some("viewer")).await;
    // A write token, always refused a link, costs no attempt.
    assert_eq!(h.mint(&aid, "write", None).await.0, 403);
    for _ in 0..5 {
        assert_eq!(h.mint(&aid, "read", None).await.0, 200);
    }
    let (s, v) = h.mint(&aid, "read", None).await;
    assert_eq!((s, err(&v)), (429, Some("rate_limited")), "{v}");
    // What the dialog offers is not counted, and the owner is not limited.
    assert_eq!(h.options(&aid, None).await.0, 200);
    assert_eq!(h.mint(&aid, "read", Some(&owner)).await.0, 200);
    h.clock.advance(120);
    assert_eq!(h.mint(&aid, "read", None).await.0, 200);
}

impl Hosted {
    /// A person invited with `rights` and signed in, with their session
    /// cookie and public ID.
    async fn member(&self, owner: &str, name: &str, rights: Value) -> (String, String) {
        let (s, v, _) = self
            .send(
                reqwest::Method::POST,
                "/api/people/invites",
                Some(owner),
                Some(json!({"name": name, "rights": rights})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let url = v["invite"]["url"].as_str().unwrap();
        let code = url.rsplit('/').next().unwrap();
        let (s, v, c) = self
            .send(
                reqwest::Method::POST,
                &format!("/api/hosted/invites/{code}/accept"),
                None,
                Some(json!({})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let pid = v["person"]["public_id"].as_str().unwrap().to_string();
        (c.unwrap(), pid)
    }
}

#[tokio::test]
async fn a_person_who_reads_a_page_through_its_link_loses_its_tokens_when_sharing_stops() {
    let h = Hosted::new().await;
    let owner = h.owner().await;
    let aid = aid_of(&h.ts.publish("Plan", &[("index.html", "<p>one</p>")]).await);
    let other = aid_of(&h.ts.publish("Other", &[("index.html", "<p>two</p>")]).await);
    // Mia's rights read Other alone; Sam's read Plan.
    let reads =
        |a: &str| json!([{"type": "clax-artifacts", "actions": ["read"], "locations": [a]}]);
    let (mia, mia_id) = h.member(&owner, "Mia", reads(&other)).await;
    let (sam, sam_id) = h.member(&owner, "Sam", reads(&aid)).await;
    assert_eq!(h.mint(&aid, "read", Some(&mia)).await.0, 404);
    h.share(&owner, &aid, Some("viewer")).await;
    let mut held = Vec::new();
    for (cookie, pid) in [(&mia, &mia_id), (&sam, &sam_id)] {
        let (s, v) = h.mint(&aid, "read", Some(cookie)).await;
        assert_eq!(s, 200, "{v}");
        let row = h.fake.rows().pop().unwrap();
        assert_eq!(row.holder.person.as_deref(), Some(pid.as_str()), "{row:?}");
        held.push(row.id);
    }
    // A change of the link's access keeps the read it gives.
    h.share(&owner, &aid, Some("commenter")).await;
    assert!(!h.fake.row(&held[0]).revoked);
    // Stopping sharing revokes Mia's, whose read came from the link, before
    // it answers; Sam's own rights keep his.
    h.share(&owner, &aid, None).await;
    assert!(h.fake.row(&held[0]).revoked, "Mia's");
    assert!(!h.fake.row(&held[1]).revoked, "Sam's");
    assert_eq!(h.mint(&aid, "read", Some(&mia)).await.0, 404);
}

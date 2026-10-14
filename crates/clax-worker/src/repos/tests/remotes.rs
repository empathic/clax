//! The server's git token routes over the stand-in, as the Worker serves
//! them: `WorkerRemotes` over `RepoDO`'s internal API (in process), the
//! content store over the same repositories, and the stand-in's smart HTTP
//! on a free port, driven with real `git` through the commands the routes
//! answer. A push is recorded as `RepoDO::record_push` records it: the
//! pusher is whom the push's token was handed to.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;
use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_store::pushes::Pusher;
use clax_git::versions::repo_name;
use clax_http::state::AppState;
use clax_testkit::TestServer;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::{Home, InProc, clock, committer, local_host};
use crate::repos::api::{DoHost, DoTokens, RepoService};
use crate::repos::assets::{BucketAssets, MemObjects};
use crate::repos::git_http::{GitHttpRequest, serve_git, serve_options};
use crate::repos::local::LocalHost;
use crate::repos::remotes::WorkerRemotes;
use crate::repos::tokens::{self, LocalTokens};

type Service = RepoService<Connection, LocalTokens<Connection>>;

async fn route(
    State(service): State<Arc<Service>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let served = serve_git(
        &service.host,
        &GitHttpRequest {
            method: method.as_str(),
            path: uri.path(),
            query: uri.query(),
            authorization: header("authorization"),
            content_encoding: header("content-encoding"),
            body: &body,
        },
        &serve_options(),
        service.now_ms(),
    )
    .await;
    let mut b = Response::builder().status(served.response.status);
    for (k, v) in &served.response.headers {
        b = b.header(k, v);
    }
    b.body(axum::body::Body::from(served.response.body))
        .unwrap()
}

/// The server on the stand-in, its git remotes `WorkerRemotes`, and the
/// stand-in's remotes served at `PUBLIC_ORIGIN`.
struct World {
    ts: TestServer,
    app: AppState,
    service: Arc<Service>,
    host: LocalHost<Connection>,
    home: Arc<Home>,
}

impl World {
    async fn new() -> World {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let git_base = format!("http://{}", listener.local_addr().unwrap());
        let host = local_host();
        let service = Arc::new(RepoService {
            tokens: LocalTokens {
                host: host.clone(),
                public_origin: git_base,
            },
            host: host.clone(),
            clock: clock(),
        });
        let router = axum::Router::new()
            .fallback(route)
            .layer(DefaultBodyLimit::disable())
            .with_state(service.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let transport = InProc {
            service: service.clone(),
        };
        let content = GitContent::new(
            DoHost {
                transport: transport.clone(),
            },
            BucketAssets {
                objects: MemObjects::new(1000),
            },
            committer(),
            clock(),
        );
        let app: Arc<Mutex<Option<AppState>>> = Arc::default();
        let kept = app.clone();
        let ts = TestServer::spawn_on_one_connection_with(Arc::new(content), move |s| {
            s.git_remotes.set(Arc::new(WorkerRemotes {
                tokens: DoTokens { transport },
            }));
            *kept.lock().unwrap() = Some(s.clone());
        })
        .await;
        let app = app.lock().unwrap().take().unwrap();
        World {
            ts,
            app,
            service,
            host,
            home: Arc::new(Home::new()),
        }
    }

    /// Runs `command` with `sh` in `cwd`, which must succeed.
    async fn sh(&self, cwd: &Path, command: &str) -> String {
        let (home, cwd, command) = (self.home.clone(), cwd.to_path_buf(), command.to_string());
        tokio::task::spawn_blocking(move || home.ok("sh", &cwd, &["-c", &command], &[]))
            .await
            .unwrap()
    }

    /// Runs `command` with `sh` in `cwd`; whether it succeeded.
    async fn sh_ok(&self, cwd: &Path, command: &str) -> bool {
        let (home, cwd, command) = (self.home.clone(), cwd.to_path_buf(), command.to_string());
        tokio::task::spawn_blocking(move || {
            home.run("sh", &cwd, &["-c", &command], &[])
                .status
                .success()
        })
        .await
        .unwrap()
    }

    /// Records the commits pushed to `aid` as `RepoDO::record_push` does,
    /// published by whom `token` was handed to.
    async fn record_push(&self, aid: &str, token: &str) {
        let holder = tokens::holder(&self.host, &repo_name(aid), token, self.service.now_ms())
            .unwrap()
            .expect("a live token of the repository");
        let pusher = Pusher {
            person: holder.person,
            session_id: holder.session,
        };
        let recorded =
            clax_http::pushes::reconcile(&self.app, ArtifactId::parse(aid).unwrap(), pusher)
                .await
                .unwrap();
        assert!(!recorded.is_empty(), "the push is recorded as a version");
    }

    fn live(&self, aid: &str, token: &str) -> bool {
        tokens::check(&self.host, &repo_name(aid), token, self.service.now_ms())
            .unwrap()
            .is_some()
    }
}

fn aid_of(v: &Value) -> String {
    v["artifact"]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_write_token_pushes_a_version_credited_to_them_and_agents_tokens_follow_what_they_may_see()
 {
    let w = World::new().await;
    let ts = &w.ts;
    let rae = ts.person("Rae").await;
    let rae_cookie = format!("clax_viewer={}", rae.cookie);
    let aid = aid_of(
        &ts.publish("Plan", &[("index.html", "<h1>one</h1>\n")])
            .await,
    );
    // Rae's own artifact: her default rights publish it.
    let db = Connection::open(ts.home.db_path()).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    db.execute(
        "UPDATE artifacts SET person = ?2 WHERE id = ?1",
        rusqlite::params![aid, rae.public_id],
    )
    .unwrap();
    drop(db);

    // The owner's agent clones it while only the owner has written it.
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let agent_token = |aid: &str| {
        let req = ts.authed(
            ts.client
                .post(format!("{}/api/sessions/{sid}/git-token/{aid}", ts.base))
                .json(&json!({"scope": "read"})),
        );
        async move {
            let r = req.send().await.unwrap();
            let status = r.status().as_u16();
            (status, r.json::<Value>().await.unwrap())
        }
    };
    let (status, agents) = agent_token(&aid).await;
    assert_eq!(status, 200, "{agents}");
    let agents = agents["token"].as_str().unwrap().to_string();
    assert!(w.live(&aid, &agents));

    // Rae's browser asks for a write token, clones with the answer's
    // command, and pushes through the commands for an existing checkout.
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/git-token", ts.base))
        .header("cookie", &rae_cookie)
        .json(&json!({"scope": "write"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let t: Value = r.json().await.unwrap();
    let token = t["token"].as_str().unwrap().to_string();
    let clone_dir = w.home.dir.path().join("clone");
    std::fs::create_dir(&clone_dir).unwrap();
    w.sh(
        &clone_dir,
        &format!("{} work", t["clone_command"].as_str().unwrap()),
    )
    .await;
    let work = clone_dir.join("work");
    assert_eq!(
        std::fs::read_to_string(work.join("index.html")).unwrap(),
        "<h1>one</h1>\n"
    );
    std::fs::write(work.join("index.html"), "<h1>two, by Rae</h1>\n").unwrap();
    w.sh(
        &work,
        "git -c user.name=Rae -c user.email=rae@clax.test commit -qam 'Rae rewrites the heading'",
    )
    .await;
    let c = &t["existing_checkout"];
    w.sh(&work, c["add_remote_command"].as_str().unwrap()).await;
    w.sh(&work, c["push_command"].as_str().unwrap()).await;
    let config = std::fs::read_to_string(work.join(".git/config")).unwrap();
    assert!(
        !config.contains(&token),
        "the checkout keeps no token: {config}"
    );

    w.record_push(&aid, &token).await;
    let id = ArtifactId::parse(&aid).unwrap();
    let v2 = ts.store.get_version(&id, 2).unwrap().expect("version 2");
    assert_eq!(v2.authors.person.as_deref(), Some(rae.public_id.as_str()));
    assert_eq!(v2.note.as_deref(), Some("Rae rewrites the heading"));

    // Rae's version is work the owner has not approved for their agents:
    // the agent's token is revoked, and a new one refused.
    clax_http::git_remotes::recheck(&w.app, None)
        .await
        .unwrap();
    assert!(!w.live(&aid, &agents), "the blind agent's token is revoked");
    let (status, v) = agent_token(&aid).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("withheld")),
        "{v}"
    );
    assert_eq!(v["error"]["withheld_authors"][0]["name"], "Rae");
    assert!(w.live(&aid, &token), "Rae's own token is hers to keep");

    // Another artifact the owner alone wrote: the agent clones it until
    // its session ends.
    let plain = aid_of(
        &ts.publish("Notes", &[("index.html", "<p>notes</p>\n")])
            .await,
    );
    let (status, v) = agent_token(&plain).await;
    assert_eq!(status, 200, "{v}");
    let notes = v["token"].as_str().unwrap().to_string();
    let second = w.home.dir.path().join("second");
    std::fs::create_dir(&second).unwrap();
    w.sh(
        &second,
        &format!("{} notes", v["clone_command"].as_str().unwrap()),
    )
    .await;
    ts.store.end_session(&AuditCtx::DAEMON, &sid).unwrap();
    clax_http::git_remotes::recheck(&w.app, None)
        .await
        .unwrap();
    assert!(!w.live(&plain, &notes));
    assert!(
        !w.sh_ok(
            &second,
            &format!("{} again", v["clone_command"].as_str().unwrap())
        )
        .await,
        "a revoked token clones nothing"
    );
}

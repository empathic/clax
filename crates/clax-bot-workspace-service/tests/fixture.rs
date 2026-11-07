//! What the tests share: a runtime whose containers are directories, a forge
//! in memory, and a host on the far side of the wire.

use anyhow::{Result, bail};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use clax_bot::workspace::{PreviewSink, Workspace};
use clax_bot_remote::wire::{WIRE_HEADER, WIRE_INSTANCE_HEADER, WIRE_VERSION};
use clax_bot_remote::{
    Keeper, Kept, LinkHosts, Method, RemoteWorkspace, Transport, Unreachable, WireRequest,
    WireResponse,
};
use clax_bot_sandbox::HostError;
use clax_bot_workspace::forge::{Forge, NewPullRequest, PullRequest, UnsignedCommits};
use clax_bot_workspace::net::Reach;
use clax_bot_workspace::preview::Preview;
use clax_bot_workspace::repo::{Policy, Redactor, StaticAccess};
use clax_bot_workspace::runtime::{
    Container, ContainerRuntime, ContainerSpec, Exec, ExecOutput, Found, Gone,
};
use clax_bot_workspace::workspace::{ContainerWorkspace, REPO_DIR, Target};
use clax_bot_workspace_service::{BearerToken, Clock, Limits, Service, router};
use clax_core::working::ManualClock;
use clax_store::bot_runs::Run;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::{Handle, Runtime as Tokio};
use tokio::sync::{Notify, Semaphore};
use tower::ServiceExt;

pub const TOKEN: &str = "a-bearer-secret-of-at-least-32-bytes";
pub const QUEUE: &str = "q";
pub const INSTANCE: &str = "instance-1";

/// A command that answers only once [`Runtime::release`] says so.
pub const HOLD: &str = "hold";
/// A command that answers more than a script may read.
pub const FLOOD: &str = "flood";

#[derive(Default)]
struct Boxes {
    live: HashMap<String, (PathBuf, BTreeMap<String, String>)>,
    made: usize,
    commands: Vec<String>,
}

/// Outlives any one service, as a real runtime does.
#[derive(Clone)]
pub struct Runtime {
    root: Arc<TempDir>,
    boxes: Arc<Mutex<Boxes>>,
    gate: Arc<Semaphore>,
    /// Told when a [`HOLD`] has begun.
    pub entered: Arc<Notify>,
    /// How many [`HOLD`]s were dropped before they answered.
    cut: Arc<AtomicUsize>,
    /// Commands run in a shell, rather than answering their own text.
    shell: bool,
    /// Taking a change from a container fails, as a stopped one does.
    pub patches_fail: Arc<AtomicBool>,
    /// Taking a change from a container is a [`HOLD`].
    pub patches_held: Arc<AtomicBool>,
}

impl Runtime {
    fn new(shell: bool) -> Runtime {
        Runtime {
            root: Arc::new(tempfile::tempdir().unwrap()),
            boxes: Arc::default(),
            gate: Arc::new(Semaphore::new(0)),
            entered: Arc::default(),
            cut: Arc::default(),
            shell,
            patches_fail: Arc::default(),
            patches_held: Arc::default(),
        }
    }

    /// Lets one [`HOLD`] answer.
    pub fn release(&self) {
        self.gate.add_permits(1);
    }

    /// How many times `command` was run.
    pub fn ran(&self, command: &str) -> usize {
        let b = self.boxes.lock().unwrap();
        b.commands.iter().filter(|c| *c == command).count()
    }

    pub fn cut(&self) -> usize {
        self.cut.load(Ordering::SeqCst)
    }

    pub fn made(&self) -> usize {
        self.boxes.lock().unwrap().made
    }

    pub fn live(&self) -> usize {
        self.boxes.lock().unwrap().live.len()
    }

    /// Loses every container, as a runtime can.
    pub fn lose_all(&self) {
        self.boxes.lock().unwrap().live.clear();
    }
}

/// Counts a [`HOLD`] dropped while it waits.
struct Cut(Arc<AtomicUsize>, bool);

impl Drop for Cut {
    fn drop(&mut self) {
        if self.1 {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct LocalBox {
    id: String,
    dir: PathBuf,
    runtime: Runtime,
}

#[async_trait::async_trait]
impl Container for LocalBox {
    fn id(&self) -> &str {
        &self.id
    }

    async fn exec(&self, exec: &Exec) -> Result<ExecOutput> {
        {
            let mut b = self.runtime.boxes.lock().unwrap();
            if !b.live.contains_key(&self.id) {
                bail!("no such container");
            }
            b.commands.push(exec.command.clone());
        }
        let patch = exec.command.contains("git diff");
        if patch && self.runtime.patches_fail.load(Ordering::SeqCst) {
            bail!("stopped");
        }
        let answer = |output: String| {
            Ok(ExecOutput {
                exit_code: 0,
                output,
            })
        };
        if exec.command == HOLD || (patch && self.runtime.patches_held.load(Ordering::SeqCst)) {
            let mut cut = Cut(self.runtime.cut.clone(), true);
            self.runtime.entered.notify_one();
            self.runtime.gate.acquire().await?.forget();
            cut.1 = false;
            return answer("held".to_string());
        }
        if exec.command == FLOOD {
            return answer("x".repeat(200 * 1024));
        }
        if !self.runtime.shell {
            return answer(format!("ran {}", exec.command));
        }
        let repo = self.dir.join("repo");
        let repo = repo.to_str().unwrap();
        let cwd = exec.cwd.as_deref().map(|c| c.replace(REPO_DIR, repo));
        let out = tokio::process::Command::new("sh")
            .args([
                "-c",
                &format!("{{ {}\n}} 2>&1", exec.command.replace(REPO_DIR, repo)),
            ])
            .current_dir(cwd.unwrap_or_else(|| self.dir.to_str().unwrap().to_string()))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .envs(&exec.envs)
            .output()
            .await?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            output: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

#[async_trait::async_trait]
impl ContainerRuntime for Runtime {
    fn limits_domains(&self) -> bool {
        true
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let mut b = self.boxes.lock().unwrap();
        b.made += 1;
        let id = format!("box-{}", b.made);
        let dir = self.root.path().join(&id);
        std::fs::create_dir(&dir)?;
        b.live
            .insert(id.clone(), (dir.clone(), spec.labels.clone()));
        Ok(Box::new(LocalBox {
            id,
            dir,
            runtime: self.clone(),
        }))
    }

    async fn attach(&self, id: &str, _spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let b = self.boxes.lock().unwrap();
        let (dir, _) = b.live.get(id).ok_or_else(|| Gone(format!("no {id}")))?;
        Ok(Box::new(LocalBox {
            id: id.to_string(),
            dir: dir.clone(),
            runtime: self.clone(),
        }))
    }

    async fn find(&self, labels: &BTreeMap<String, String>) -> Result<Vec<Found>> {
        let b = self.boxes.lock().unwrap();
        let all = b.live.iter().map(|(id, (_, l))| Found {
            id: id.clone(),
            labels: l.clone(),
        });
        Ok(all.filter(|f| f.labels_hold(labels)).collect())
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        self.boxes.lock().unwrap().live.remove(id);
        Ok(())
    }
}

pub struct FakeForge {
    pub opened: Mutex<Vec<NewPullRequest>>,
    /// Opening a pull request waits for [`FakeForge::release`].
    pub gated: AtomicBool,
    gate: Semaphore,
    /// Told when a gated opening has begun.
    pub opening: Notify,
    /// A gated opening has begun.
    pub reached: AtomicBool,
}

impl Default for FakeForge {
    fn default() -> FakeForge {
        FakeForge {
            opened: Mutex::default(),
            gated: AtomicBool::new(false),
            gate: Semaphore::new(0),
            opening: Notify::new(),
            reached: AtomicBool::new(false),
        }
    }
}

impl FakeForge {
    pub fn release(&self) {
        self.gate.add_permits(1);
    }

    pub fn opened(&self) -> usize {
        self.opened.lock().unwrap().len()
    }
}

fn pull_request() -> PullRequest {
    PullRequest {
        number: 1,
        html_url: "https://forge.example/pr/1".to_string(),
    }
}

#[async_trait::async_trait]
impl Forge for FakeForge {
    async fn find_open(&self, head: &str) -> Result<Option<PullRequest>> {
        let opened = self.opened.lock().unwrap();
        Ok(opened.iter().any(|p| p.head == head).then(pull_request))
    }

    async fn open(&self, pr: &NewPullRequest) -> Result<PullRequest> {
        if self.gated.load(Ordering::SeqCst) {
            self.reached.store(true, Ordering::SeqCst);
            self.opening.notify_one();
            self.gate.acquire().await?.forget();
        }
        self.opened.lock().unwrap().push(pr.clone());
        Ok(pull_request())
    }
}

/// `git` with `args` in `dir`.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A bare repository under `root` whose `main` holds `app.txt`.
fn seed(root: &Path) -> PathBuf {
    let bare = root.join("repo.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(
        &bare,
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let work = root.join("seed");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("app.txt"), "one\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "seed"]);
    git(&work, &["push", "--quiet", bare.to_str().unwrap(), "main"]);
    bare
}

/// What outlives a service: its directory, the runtime, the forge.
pub struct World {
    pub root: TempDir,
    pub runtime: Runtime,
    pub forge: Arc<FakeForge>,
    url: String,
    /// The target's queue.
    queue: String,
    /// What the target says of itself: every word of an offer it can set.
    pub telling: bool,
    pub limits: Limits,
    /// What the service's clock reads.
    pub now_ms: Arc<AtomicI64>,
}

impl World {
    /// Commands answer their own text, and nothing runs but the test.
    pub fn new() -> World {
        World::with(
            Runtime::new(false),
            "file:///no/such/repository".to_string(),
        )
    }

    /// A real repository, and commands run in a real shell.
    pub fn with_repository() -> World {
        let mut w = World::with(Runtime::new(true), String::new());
        w.url = format!("file://{}", seed(w.root.path()).display());
        w
    }

    /// [`World::with_repository`], the target on `queue`.
    pub fn with_repository_on(queue: &str) -> World {
        World {
            queue: queue.to_string(),
            ..World::with_repository()
        }
    }

    fn with(runtime: Runtime, url: String) -> World {
        World {
            root: tempfile::tempdir().unwrap(),
            runtime,
            forge: Arc::default(),
            url,
            queue: QUEUE.to_string(),
            telling: false,
            limits: Limits::default(),
            now_ms: Arc::new(AtomicI64::new(1_000_000)),
        }
    }

    pub fn ledger_dir(&self) -> PathBuf {
        self.root.path().join("workspace-runs")
    }

    fn target(&self) -> Target {
        Target {
            stamp: None,
            queue: self.queue.clone(),
            repo_url: self.url.clone(),
            container_repo_url: self.url.clone(),
            base: "main".to_string(),
            access: Arc::new(StaticAccess::default()),
            confidential: self.telling,
            reviewed_previews: true,
            wake_on_reply: self.telling,
            unsigned: UnsignedCommits::Allow,
            policy: Policy {
                follow_ups: !self.telling,
                guidance: self
                    .telling
                    .then(|| "Run the tests with just test.".to_string()),
                preview_gate: self.telling.then(|| "comment /preview <sha>".to_string()),
                ..Policy::default()
            },
            forge: self.forge.clone(),
            preview: Preview {
                url_template: "http://127.0.0.1:1/{pr}".to_string(),
                ready_path: "/".to_string(),
                ready_names_commit: false,
                timeout: Duration::ZERO,
                poll: Duration::ZERO,
            },
            reach: Reach::any(),
        }
    }

    /// A workspace, new, on what outlives it.
    pub fn workspace(&self) -> ContainerWorkspace {
        let spec = ContainerSpec {
            labels: BTreeMap::from([("clax.workspace".to_string(), "here".to_string())]),
            ..ContainerSpec::default()
        };
        ContainerWorkspace::keeping(
            Arc::new(self.runtime.clone()),
            spec,
            vec![self.target()],
            true,
            self.ledger_dir(),
            Arc::new(Redactor::default()),
        )
    }

    /// A service, new: a restart is another of these.
    pub fn service(&self) -> Arc<Service> {
        let now = self.now_ms.clone();
        let clock: Clock = Arc::new(move || now.load(Ordering::SeqCst));
        Service::open_with(self.workspace(), self.root.path(), clock, self.limits).unwrap()
    }

    /// A service whose tasks all live on `tokio`, so that shutting it down
    /// is the process dying: nothing it was doing goes on.
    pub fn service_on(&self, tokio: &Tokio) -> Arc<Service> {
        let _entered = tokio.enter();
        self.service()
    }

    pub fn host(&self, service: &Arc<Service>) -> Host {
        Host::new(service, TOKEN)
    }

    /// Stops the told file being written, as a full disk does.
    pub fn block_saves(&self) {
        std::fs::create_dir(self.root.path().join("workspace-previews.next")).unwrap();
    }

    pub fn unblock_saves(&self) {
        std::fs::remove_dir(self.root.path().join("workspace-previews.next")).unwrap();
    }
}

/// Sends a request through the real router, in this process.
pub struct InProcess {
    router: Router,
    secret: String,
    /// Where a request's handler runs, when not here.
    on: Option<Handle>,
    pub sent: AtomicUsize,
    /// Nothing gets through, as in an outage.
    pub down: AtomicBool,
}

impl InProcess {
    async fn call(&self, request: Request<Body>) -> (u16, Vec<u8>) {
        let sent = self.router.clone().oneshot(request);
        let r = match &self.on {
            Some(tokio) => tokio.spawn(sent).await.unwrap().unwrap(),
            None => sent.await.unwrap(),
        };
        let status = r.status().as_u16();
        let body = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        (status, body.to_vec())
    }
}

#[async_trait::async_trait]
impl Transport for InProcess {
    async fn send(&self, r: WireRequest) -> Result<WireResponse, Unreachable> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(Unreachable);
        }
        let method = match r.method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        };
        let mut b = Request::builder()
            .method(method)
            .uri(r.path)
            .header("authorization", format!("Bearer {}", self.secret));
        for (name, value) in r.headers {
            b = b.header(name, value);
        }
        let request = b.body(Body::from(r.body.unwrap_or_default())).unwrap();
        let (status, body) = self.call(request).await;
        Ok(WireResponse { status, body })
    }
}

#[derive(Default)]
struct Forgets;

#[async_trait::async_trait]
impl Keeper for Forgets {
    async fn save(&self, _kept: &Kept) {}
}

/// What a host's runs were told.
#[derive(Default)]
pub struct Sink {
    pub told: Mutex<Vec<String>>,
    /// Every telling fails, as a host's store may.
    pub down: Mutex<bool>,
}

impl Sink {
    fn tell(&self, what: String) -> Result<(), HostError> {
        if *self.down.lock().unwrap() {
            return Err(HostError::new("store_down", "no"));
        }
        self.told.lock().unwrap().push(what);
        Ok(())
    }
}

#[async_trait::async_trait]
impl PreviewSink for Sink {
    async fn preview_ready(&self, run: &str, url: &str, reviewed: bool) -> Result<(), HostError> {
        self.tell(format!("{run} ready {url} {reviewed}"))
    }
    async fn preview_missed(&self, run: &str) -> Result<(), HostError> {
        self.tell(format!("{run} missed"))
    }
}

/// A host of runs on the far side of the wire.
pub struct Host {
    pub transport: Arc<InProcess>,
    pub remote: Arc<RemoteWorkspace>,
    pub sink: Arc<Sink>,
    /// What the host's clock reads.
    pub clock: Arc<ManualClock>,
}

impl Host {
    pub fn new(service: &Arc<Service>, secret: &str) -> Host {
        Host::reaching(service, secret, INSTANCE, None)
    }

    /// A host whose requests run on `tokio`, with [`World::service_on`].
    pub fn on(service: &Arc<Service>, tokio: &Tokio) -> Host {
        Host::reaching(service, TOKEN, INSTANCE, Some(tokio.handle().clone()))
    }

    /// Another instance, holding the same secret.
    pub fn of_instance(service: &Arc<Service>, instance: &str) -> Host {
        Host::reaching(service, TOKEN, instance, None)
    }

    fn reaching(service: &Arc<Service>, secret: &str, instance: &str, on: Option<Handle>) -> Host {
        let token = BearerToken::new(TOKEN.to_string()).unwrap();
        let transport = Arc::new(InProcess {
            router: router(service.clone(), token, INSTANCE.to_string()),
            secret: secret.to_string(),
            on,
            sent: AtomicUsize::new(0),
            down: AtomicBool::new(false),
        });
        let clock = Arc::new(ManualClock::at("2026-11-06T12:00:00Z"));
        let remote = RemoteWorkspace::new(transport.clone(), Arc::new(Forgets), None, instance)
            .with_preview_hosts(LinkHosts::parse("*.preview.example"))
            .with_forge_hosts(LinkHosts::parse("forge.example"))
            .with_clock(clock.clone());
        let remote = Arc::new(remote);
        let sink = Arc::new(Sink::default());
        remote.previews(sink.clone());
        Host {
            transport,
            remote,
            sink,
            clock,
        }
    }

    /// A request as written here, with `headers` alone: nothing is added.
    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Value) {
        let mut b = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            b = b.header(*name, *value);
        }
        let (status, body) = self.transport.call(b.body(Body::from(body)).unwrap()).await;
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// A request as a host in good standing sends it.
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bearer = format!("Bearer {TOKEN}");
        let wire = WIRE_VERSION.to_string();
        let headers = [
            ("authorization", bearer.as_str()),
            (WIRE_HEADER, wire.as_str()),
            (WIRE_INSTANCE_HEADER, INSTANCE),
            ("content-type", "application/json"),
        ];
        let body = body.map_or_else(Vec::new, |b| b.to_string().into_bytes());
        self.raw(method, path, &headers, body).await
    }

    pub fn sent(&self) -> usize {
        self.transport.sent.load(Ordering::SeqCst)
    }
}

/// A [`clax_bot_remote::wire::JobRequest`]'s body for command `command`.
pub fn job(key: &str, run: &str, command: &str) -> Value {
    json!({
        "key": key,
        "run": {"id": run, "thread_ids": ["thread1"]},
        "kind": "exec",
        "queue": null,
        "command": command,
        "cwd": null,
        "timeout_secs": 60,
    })
}

/// Run `id`, as a host holds it.
pub fn run(id: &str) -> Run {
    clax_bot_remote::wire::RunRef {
        id: id.to_string(),
        thread_ids: vec!["thread1".to_string()],
    }
    .to_run()
}

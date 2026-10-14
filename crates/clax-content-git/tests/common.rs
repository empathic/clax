//! Shared helpers: stores over clax-git's in-memory host, a host whose
//! repositories run a hook before each ref update (to race a writer
//! deterministically), and the smart-HTTP server with real git.

use std::path::Path;
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use clax_content_git::GitContent;
use clax_core::content::memory::MemContent;
use clax_core::content::{NewFile, VersionWrite};
use clax_core::working::ManualClock;
use clax_git::memory::{MemoryHost, MemoryRepo};
use clax_git::server::{Access, GitRequest, ServeOptions, serve, split_git_path};
use clax_git::{CommitMeta, Object, ObjectId, Person, RefStatus, RefUpdate, Repo, RepoHost};

/// A bound for a hung git, not a time budget.
pub const HANG: Duration = Duration::from_secs(120);

pub fn committer() -> Person {
    Person {
        name: "Clax".into(),
        email: "clax@users.clax.dev".into(),
    }
}

pub fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::at("2026-10-13T12:00:00Z"))
}

/// A store over `host`, with its own in-memory assets.
pub fn store_on<H: RepoHost>(host: H) -> GitContent<H, MemContent> {
    GitContent::new(host, MemContent::new(), committer(), clock())
}

/// A version write of `files`, carrying `carry`.
pub fn write<'a>(carry: &[&'a str], files: &[(&'a str, &'a [u8])]) -> VersionWrite<'a> {
    VersionWrite {
        carry: carry.to_vec(),
        put: files
            .iter()
            .map(|(path, bytes)| NewFile {
                path,
                bytes,
                content_type: "text/plain",
            })
            .collect(),
        ..Default::default()
    }
}

type Hook = Box<dyn FnOnce() + Send>;

/// A [`MemoryHost`] whose repositories run the hook set with
/// [`HookHost::before_next_apply`] once, just before the next ref update:
/// another writer acting between a store's read of the refs and its update.
#[derive(Clone, Default)]
pub struct HookHost {
    pub inner: MemoryHost,
    hook: Arc<Mutex<Option<Hook>>>,
}

impl HookHost {
    pub fn before_next_apply(&self, f: impl FnOnce() + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }
}

pub struct HookRepo {
    inner: MemoryRepo,
    hook: Arc<Mutex<Option<Hook>>>,
}

impl Repo for HookRepo {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        self.inner.read_object(id).await
    }
    async fn read_commit_meta(&self, id: &ObjectId) -> clax_git::Result<Option<CommitMeta>> {
        self.inner.read_commit_meta(id).await
    }
    async fn refs(&self) -> clax_git::Result<std::collections::BTreeMap<String, ObjectId>> {
        self.inner.refs().await
    }
    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> clax_git::Result<Vec<RefStatus>> {
        let hook = self.hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        self.inner.apply(objects, updates, atomic).await
    }
}

impl RepoHost for HookHost {
    type Repo = HookRepo;
    async fn open(&self, name: &str) -> clax_git::Result<Option<HookRepo>> {
        Ok(self.inner.repo(name).map(|inner| HookRepo {
            inner,
            hook: self.hook.clone(),
        }))
    }
    async fn create(&self, name: &str) -> clax_git::Result<()> {
        self.inner.create(name).await
    }
    async fn fork(&self, from: &str, to: &str) -> clax_git::Result<()> {
        self.inner.fork(from, to).await
    }
    async fn delete(&self, name: &str) -> clax_git::Result<()> {
        self.inner.delete(name).await
    }
    fn can_prune(&self) -> bool {
        self.inner.can_prune()
    }

    async fn prune(&self, name: &str) -> clax_git::Result<()> {
        self.inner.prune(name).await
    }
}

// ── git ─────────────────────────────────────────────────────────────────────

/// A home for git with no configuration of the person running the tests.
pub struct Git {
    pub home: tempfile::TempDir,
}

impl Git {
    pub fn new() -> Git {
        Git {
            home: tempfile::Builder::new()
                .prefix("clax-content-git-home-")
                .tempdir()
                .expect("tempdir"),
        }
    }

    /// Runs git in `cwd` without blocking the runtime; answers its output.
    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut c = tokio::process::Command::new("git");
        c.current_dir(cwd)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("TMPDIR", std::env::temp_dir())
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        tokio::time::timeout(HANG, c.output())
            .await
            .unwrap_or_else(|_| panic!("git {args:?} hung"))
            .expect("run git")
    }

    /// Runs git, which must succeed; answers its trimmed stdout.
    pub async fn text(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args).await;
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    /// Runs git with `input` on its stdin, which must succeed; answers its
    /// trimmed stdout.
    pub async fn text_in(&self, cwd: &Path, args: &[&str], input: &[u8]) -> String {
        let file = tempfile::tempfile().expect("tempfile");
        std::io::Write::write_all(&mut &file, input).expect("write the input");
        std::io::Seek::rewind(&mut &file).expect("rewind the input");
        let mut c = tokio::process::Command::new("git");
        c.current_dir(cwd)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::from(file));
        let out = tokio::time::timeout(HANG, c.output())
            .await
            .unwrap_or_else(|_| panic!("git {args:?} hung"))
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    /// Runs git, which must succeed; answers its stdout as bytes.
    pub async fn bytes(&self, cwd: &Path, args: &[&str]) -> Vec<u8> {
        let out = self.run(cwd, args).await;
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
}

/// clax-git's smart-HTTP server over `host` at `/git/<repo>.git/…`, with
/// write access for every request, on a free local port; stopped when
/// dropped.
pub struct Server {
    pub base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    pub async fn start(host: MemoryHost) -> Server {
        let router = axum::Router::new()
            .fallback(route)
            .layer(DefaultBodyLimit::disable())
            .with_state(host);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        Server { base, task }
    }

    pub fn url(&self, repo: &str) -> String {
        format!("{}/git/{repo}.git", self.base)
    }
}

async fn route(
    State(host): State<MemoryHost>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some((name, rest)) = split_git_path(uri.path()) else {
        return (StatusCode::NOT_FOUND, "not a git path\n").into_response();
    };
    let Some(repo) = host.repo(name) else {
        return (StatusCode::NOT_FOUND, "no repository\n").into_response();
    };
    let request = GitRequest {
        method: method.as_str(),
        rest,
        query: uri.query(),
        content_encoding: headers
            .get("content-encoding")
            .and_then(|v| v.to_str().ok()),
        body: &body,
    };
    let served = serve(&repo, Access::Write, &request, &ServeOptions::default()).await;
    let mut b = Response::builder().status(served.response.status);
    for (k, v) in &served.response.headers {
        b = b.header(k, v);
    }
    b.body(axum::body::Body::from(served.response.body))
        .expect("response")
}

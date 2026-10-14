//! Shared test helpers: git and jj with a clean environment, an HTTP
//! server around `git http-backend`, an HTTP server around
//! `clax_git::server::serve` over a `MemoryHost`, and a reqwest transport.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use clax_git::GitError;
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::memory::MemoryHost;
use clax_git::server::{
    Access, GitRequest, ServeOptions, bearer_token, serve, split_git_path, too_large, unauthorized,
};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

/// A bound for a hung git or jj, not a time budget.
pub const HANG: Duration = Duration::from_secs(120);

/// A scratch directory removed when dropped.
pub fn temp(label: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(&format!("clax-git-{label}-"))
        .tempdir()
        .expect("tempdir")
}

/// A home for git and jj with no configuration of the person running the
/// tests (an empty `jj.toml` included).
pub struct Home {
    pub dir: TempDir,
}

impl Home {
    pub fn new() -> Home {
        let dir = temp("home");
        std::fs::write(dir.path().join("jj.toml"), "").expect("jj.toml");
        Home { dir }
    }

    /// The environment for git and jj: PATH and this home, nothing else.
    pub fn env(&self) -> Vec<(String, String)> {
        let home = self.dir.path().to_string_lossy().into_owned();
        vec![
            ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".into(), home.clone()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("JJ_USER".into(), "Rae".into()),
            ("JJ_EMAIL".into(), "rae@example.com".into()),
            ("JJ_CONFIG".into(), format!("{home}/jj.toml")),
            (
                "TMPDIR".into(),
                std::env::temp_dir().to_string_lossy().into_owned(),
            ),
        ]
    }

    fn command(&self, program: &str, cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> Command {
        let mut c = Command::new(program);
        c.current_dir(cwd).args(args).env_clear();
        for (k, v) in self.env() {
            c.env(k, v);
        }
        for (k, v) in extra {
            c.env(k, v);
        }
        c
    }

    /// Runs git, which must succeed; answers stdout.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> Vec<u8> {
        self.git_with(cwd, args, &[], None)
    }

    /// Runs git with extra environment and stdin, which must succeed.
    pub fn git_with(
        &self,
        cwd: &Path,
        args: &[&str],
        extra: &[(&str, &str)],
        input: Option<&[u8]>,
    ) -> Vec<u8> {
        let out = self.try_git(cwd, args, extra, input);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// Runs git and answers its output, whatever its exit.
    pub fn try_git(
        &self,
        cwd: &Path,
        args: &[&str],
        extra: &[(&str, &str)],
        input: Option<&[u8]>,
    ) -> Output {
        let mut c = self.command("git", cwd, args, extra);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().expect("spawn git");
        if let Some(input) = input {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(input)
                .expect("write stdin");
        }
        drop(child.stdin.take());
        child.wait_with_output().expect("git output")
    }

    /// git's stdout as trimmed text.
    pub fn git_text(&self, cwd: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git(cwd, args))
            .expect("utf8")
            .trim()
            .to_string()
    }

    /// git's stdout as trimmed text, with `input` on stdin.
    pub fn git_text_input(&self, cwd: &Path, args: &[&str], input: &[u8]) -> String {
        String::from_utf8(self.git_with(cwd, args, &[], Some(input)))
            .expect("utf8")
            .trim()
            .to_string()
    }

    /// Runs `program` without blocking the runtime (for commands that talk
    /// to a server in this process); answers its output, whatever its exit.
    pub async fn run_async(
        &self,
        program: &str,
        cwd: &Path,
        args: &[&str],
        extra: &[(&str, &str)],
    ) -> Output {
        let mut c = tokio::process::Command::from(self.command(program, cwd, args, extra));
        c.kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = c.spawn().unwrap_or_else(|e| panic!("spawn {program}: {e}"));
        tokio::time::timeout(HANG, child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("{program} {args:?} hung"))
            .expect("output")
    }

    /// Like [`Home::run_async`], and the command must succeed; answers stderr.
    pub async fn ok_async(
        &self,
        program: &str,
        cwd: &Path,
        args: &[&str],
        extra: &[(&str, &str)],
    ) -> String {
        let out = self.run_async(program, cwd, args, extra).await;
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{program} {args:?} failed: {stderr}");
        stderr
    }
}

/// `-c http.extraHeader=Authorization: Bearer <token>`, as git arguments.
pub fn bearer_args(token: &str) -> [String; 2] {
    [
        "-c".into(),
        format!("http.extraHeader=Authorization: Bearer {token}"),
    ]
}

/// The environment that sets `http.extraHeader` to carry `token`, as
/// `jj git clone` and `jj git fetch` take it.
pub fn bearer_env(token: &str) -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "http.extraHeader".into()),
        (
            "GIT_CONFIG_VALUE_0".into(),
            format!("Authorization: Bearer {token}"),
        ),
    ]
}

/// jj from `CLAX_TEST_JJ`, else on PATH, else none.
pub fn jj() -> Option<String> {
    let candidates = [std::env::var("CLAX_TEST_JJ").ok(), Some("jj".to_string())];
    candidates.into_iter().flatten().find(|p| {
        Command::new(p)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Every file below `dir` (skipping `.git` and `.jj`), as path to bytes.
pub fn files_of(dir: &Path) -> HashMap<String, Vec<u8>> {
    let mut out = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read_dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            if name == ".git" || name == ".jj" {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(dir)
                    .expect("prefix")
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&p).expect("read"));
            }
        }
    }
    out
}

// ── transport ───────────────────────────────────────────────────────────────

/// A reqwest transport, with a hook run before each request that may
/// rewrite it or answer it instead.
#[derive(Clone)]
pub struct TestTransport {
    client: reqwest::Client,
    #[allow(clippy::type_complexity)]
    hook: Arc<Mutex<Box<dyn FnMut(&mut HttpRequest) -> Option<HttpResponse> + Send>>>,
}

impl TestTransport {
    pub fn new() -> TestTransport {
        TestTransport::hooked(|_| None)
    }

    pub fn hooked(
        hook: impl FnMut(&mut HttpRequest) -> Option<HttpResponse> + Send + 'static,
    ) -> TestTransport {
        TestTransport {
            client: reqwest::Client::new(),
            hook: Arc::new(Mutex::new(Box::new(hook))),
        }
    }
}

impl Transport for TestTransport {
    async fn send(&self, mut request: HttpRequest) -> clax_git::Result<HttpResponse> {
        if let Some(answer) = (self.hook.lock().unwrap())(&mut request) {
            return Ok(answer);
        }
        let method = if request.method == "POST" {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        };
        let mut b = self.client.request(method, &request.url).body(request.body);
        for (k, v) in &request.headers {
            b = b.header(*k, v);
        }
        let r = b
            .send()
            .await
            .map_err(|e| GitError::RepoUnavailable(e.to_string()))?;
        let status = r.status().as_u16();
        let headers = r
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = r
            .bytes()
            .await
            .map_err(|e| GitError::RepoUnavailable(e.to_string()))?
            .to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

// ── servers ─────────────────────────────────────────────────────────────────

/// An HTTP server on a free local port, stopped when dropped.
pub struct Server {
    pub base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn listen(router: axum::Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    Server { base, task }
}

#[derive(Clone)]
struct BackendState {
    root: PathBuf,
    token: String,
    env: Vec<(String, String)>,
}

/// `git http-backend` serving the bare repositories under `root`, behind a
/// check for `Authorization: Bearer <token>`.
pub async fn http_backend(home: &Home, root: &Path, token: &str) -> Server {
    let state = BackendState {
        root: root.to_path_buf(),
        token: token.to_string(),
        env: home.env(),
    };
    let router = axum::Router::new()
        .fallback(cgi)
        .layer(DefaultBodyLimit::disable())
        .with_state(state);
    listen(router).await
}

async fn cgi(
    State(s): State<BackendState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != format!("Bearer {}", s.token) {
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Bearer realm=\"test\"")],
            "",
        )
            .into_response();
    }
    let header = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let mut c = tokio::process::Command::new("git");
    c.arg("http-backend").env_clear().kill_on_drop(true);
    for (k, v) in &s.env {
        c.env(k, v);
    }
    c.env("GIT_PROJECT_ROOT", &s.root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", uri.path())
        .env("QUERY_STRING", uri.query().unwrap_or(""))
        .env("REQUEST_METHOD", method.as_str())
        .env("CONTENT_TYPE", header("content-type"))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("REMOTE_USER", "token");
    let encoding = header("content-encoding");
    if !encoding.is_empty() {
        c.env("HTTP_CONTENT_ENCODING", encoding);
    }
    let protocol = header("git-protocol");
    if !protocol.is_empty() {
        c.env("GIT_PROTOCOL", protocol);
    }
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("spawn http-backend");
    let mut stdin = child.stdin.take().expect("stdin");
    let write = async move {
        stdin.write_all(&body).await.ok();
        drop(stdin);
    };
    let (_, out) = tokio::join!(write, child.wait_with_output());
    let all = out.expect("http-backend output").stdout;
    let (split, sep) = match find(&all, b"\r\n\r\n") {
        Some(i) => (i, 4),
        None => (find(&all, b"\n\n").expect("CGI headers"), 2),
    };
    let mut status = StatusCode::OK;
    let mut response = Response::builder();
    for line in String::from_utf8_lossy(&all[..split]).lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "status" {
            status = StatusCode::from_u16(v[..3].parse().expect("status")).expect("status");
        } else {
            response = response.header(k, v);
        }
    }
    response
        .status(status)
        .body(axum::body::Body::from(all[split + sep..].to_vec()))
        .expect("response")
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[derive(Clone)]
struct ClaxState {
    host: MemoryHost,
    tokens: HashMap<String, Access>,
    options: ServeOptions,
    /// The object count of each upload-pack pack served, newest last; and
    /// how many requests came gzip-encoded.
    stats: Arc<Mutex<Stats>>,
}

/// What [`clax_server`] observed.
#[derive(Default)]
pub struct Stats {
    pub packs: Vec<u32>,
    pub gzip_requests: usize,
    pub pushes: Vec<Vec<clax_git::RefUpdate>>,
}

/// The crate's smart-HTTP server over `host` at `/git/<repo>.git/…`, with a
/// token for each access.
pub async fn clax_server(
    host: &MemoryHost,
    tokens: &[(&str, Access)],
    options: ServeOptions,
) -> (Server, Arc<Mutex<Stats>>) {
    let stats = Arc::new(Mutex::new(Stats::default()));
    let state = ClaxState {
        host: host.clone(),
        tokens: tokens.iter().map(|(t, a)| (t.to_string(), *a)).collect(),
        options,
        stats: stats.clone(),
    };
    let router = axum::Router::new()
        .fallback(clax_git_route)
        .layer(DefaultBodyLimit::disable())
        .with_state(state);
    (listen(router).await, stats)
}

async fn clax_git_route(
    State(s): State<ClaxState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let to_axum = |r: HttpResponse| {
        let mut b = Response::builder().status(r.status);
        for (k, v) in &r.headers {
            b = b.header(k, v);
        }
        b.body(axum::body::Body::from(r.body)).expect("response")
    };
    let Some((repo_name, rest)) = split_git_path(uri.path()) else {
        return (StatusCode::NOT_FOUND, "not a git smart-HTTP path\n").into_response();
    };
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_token);
    let access = token.and_then(|t| s.tokens.get(t)).copied();
    let repo = s.host.repo(repo_name);
    let (Some(access), Some(repo)) = (access, repo) else {
        return to_axum(unauthorized());
    };
    let limit = s.options.body_limit(rest);
    if body.len() > limit {
        return to_axum(too_large(limit));
    }
    let encoding = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok());
    if encoding == Some("gzip") {
        s.stats.lock().unwrap().gzip_requests += 1;
    }
    let request = GitRequest {
        method: method.as_str(),
        rest,
        query: uri.query(),
        content_encoding: encoding,
        body: &body,
    };
    let served = serve(&repo, access, &request, &s.options).await;
    {
        let mut stats = s.stats.lock().unwrap();
        if rest == "git-upload-pack"
            && let Some(i) = find(&served.response.body, b"PACK")
        {
            let n = u32::from_be_bytes(served.response.body[i + 8..i + 12].try_into().unwrap());
            stats.packs.push(n);
        }
        if !served.pushed.is_empty() {
            stats.pushes.push(served.pushed.clone());
        }
    }
    to_axum(served.response)
}

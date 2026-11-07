//! What the repository tests share: a seeded bare repository, a real `git`
//! with no machine configuration, and a small HTTP server in front of
//! `git http-backend` that demands credentials.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use base64::Engine;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;

/// `git` with `args` in `dir`; panics on failure and answers stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare repository `<root>/repo.git` whose `main` holds `app.txt`,
/// `tool.sh`, `.github/ci.yml`, and `outside`, a symlink out of the tree.
/// Pushed rather than copied, so it shares no file with anything.
pub fn seed(root: &Path) -> PathBuf {
    let bare = root.join("repo.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(
        &bare,
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let work = root.join("seed");
    std::fs::create_dir_all(work.join(".github")).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("app.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(work.join("tool.sh"), "echo tool\n").unwrap();
    std::fs::write(work.join(".github/ci.yml"), "on: push\n").unwrap();
    std::os::unix::fs::symlink("/tmp", work.join("outside")).unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "seed"]);
    git(&work, &["push", "--quiet", bare.to_str().unwrap(), "main"]);
    bare
}

/// The `Authorization` value git sends for `token`.
pub fn basic(token: &str) -> String {
    let b = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    format!("Basic {b}")
}

/// What a [`Remote`] saw.
#[derive(Default)]
pub struct Seen {
    /// The `Authorization` of each request, when it had one.
    pub authorizations: Vec<String>,
}

#[derive(Clone)]
struct Backend {
    root: PathBuf,
    token: String,
    /// Where `/moved/…` redirects.
    moved_to: Option<String>,
    seen: Arc<Mutex<Seen>>,
}

/// A served repository root.
pub struct Remote {
    pub base: String,
    pub seen: Arc<Mutex<Seen>>,
}

impl Remote {
    /// The URL of `repo.git`.
    pub fn url(&self) -> String {
        format!("{}/repo.git", self.base)
    }
}

/// Serves the repositories under `root` to whoever sends `token`, as a forge
/// does: anyone else gets 401. An empty `token` serves anyone. `/echo/…`
/// answers 403 with the request's
/// `Authorization` in its text, as a careless proxy might, and `/moved/…`
/// redirects to `moved_to`.
pub async fn serve(root: &Path, token: &str, moved_to: Option<String>) -> Remote {
    serve_on("127.0.0.1", root, token, moved_to).await
}

/// As [`serve`], on interface `host`.
pub async fn serve_on(host: &str, root: &Path, token: &str, moved_to: Option<String>) -> Remote {
    let listener = tokio::net::TcpListener::bind((host, 0)).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let app = Router::new().fallback(handle).with_state(Backend {
        root: root.to_path_buf(),
        token: token.to_string(),
        moved_to,
        seen: seen.clone(),
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Remote { base, seen }
}

fn plain(status: StatusCode, text: String) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(text))
        .unwrap()
}

async fn handle(State(b): State<Backend>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let auth = parts
        .headers
        .get(header::AUTHORIZATION)
        .map(|v| v.to_str().unwrap().to_string());
    if let Some(a) = &auth {
        b.seen.lock().unwrap().authorizations.push(a.clone());
    }
    let path = parts.uri.path();
    if path.starts_with("/echo/") {
        let told = auth.unwrap_or_default();
        return plain(StatusCode::FORBIDDEN, format!("rejected: {told}"));
    }
    if let Some(rest) = path.strip_prefix("/moved/")
        && let Some(to) = &b.moved_to
    {
        let query = parts.uri.query().unwrap_or_default();
        return Response::builder()
            .status(StatusCode::MOVED_PERMANENTLY)
            .header(header::LOCATION, format!("{to}/{rest}?{query}"))
            .body(Body::empty())
            .unwrap();
    }
    if !b.token.is_empty() && auth.as_deref() != Some(basic(&b.token).as_str()) {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"git\"")
            .body(Body::empty())
            .unwrap();
    }
    http_backend(&b.root, &parts.method, &parts.uri, &parts.headers, body).await
}

/// One request through `git http-backend`, by the CGI rules it speaks.
pub async fn http_backend(
    root: &Path,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let header_of = |name: header::HeaderName| {
        headers
            .get(name)
            .map_or("", |v| v.to_str().unwrap())
            .to_string()
    };
    let mut child = tokio::process::Command::new("git")
        .arg("http-backend")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "http.receivepack")
        .env("GIT_CONFIG_VALUE_0", "true")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REMOTE_USER", "x-access-token")
        .env("REQUEST_METHOD", method.as_str())
        .env("PATH_INFO", uri.path())
        .env("QUERY_STRING", uri.query().unwrap_or_default())
        .env("CONTENT_TYPE", header_of(header::CONTENT_TYPE))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("HTTP_CONTENT_ENCODING", header_of(header::CONTENT_ENCODING))
        .env(
            "GIT_PROTOCOL",
            headers
                .get("git-protocol")
                .map_or("", |v| v.to_str().unwrap()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let feed = async move {
        stdin.write_all(&body).await.unwrap();
        stdin.shutdown().await.unwrap();
    };
    let (out, ()) = tokio::join!(child.wait_with_output(), feed);
    cgi_response(&out.unwrap().stdout)
}

fn cgi_response(out: &[u8]) -> Response {
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a CGI answer has headers");
    let head = String::from_utf8_lossy(&out[..split]);
    let mut r = Response::builder();
    for line in head.lines() {
        let (name, value) = line.split_once(": ").unwrap();
        if name.eq_ignore_ascii_case("status") {
            let code: u16 = value.split(' ').next().unwrap().parse().unwrap();
            r = r.status(code);
        } else {
            r = r.header(name, value);
        }
    }
    r.body(Body::from(out[split + 4..].to_vec())).unwrap()
}

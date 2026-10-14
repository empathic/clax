//! The Cloudflare Artifacts host writing to real git: each repository is a
//! bare repository served by `git http-backend` (git's own receive-pack),
//! read by a fake binding through `git cat-file`. Versions written through
//! the content store are what `git log` shows, read back through the
//! binding; a version removed after a failed publish is gone; a refused
//! token is replaced.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::content::memory::MemContent;
use clax_core::content::{ContentStore, wait};
use clax_core::working::{Clock, ManualClock};
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::object::parse_tree;
use clax_git::versions::repo_name;
use clax_git::{Commit, CommitMeta, GitError, ObjectId, TreeEntry};

use super::{Home, clock, committer, write};
use crate::repos::artifacts::{
    ApiToken, ArtifactsApi, ArtifactsHost, ArtifactsRepoApi, CreatedRepo,
};
use crate::repos::tokens::Scope;

const ORIGIN: &str = "http://backend.test";

struct Inner {
    home: Home,
    clock: Arc<ManualClock>,
    valid: Mutex<HashSet<String>>,
    seed_valid: bool,
    minted: AtomicUsize,
}

/// Bare repositories under a temporary root, a fake binding over them and
/// `git http-backend` as their remote.
#[derive(Clone)]
struct Backend(Arc<Inner>);

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .expect("run git")
}

fn git_ok(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

impl Backend {
    fn new(seed_valid: bool) -> Backend {
        Backend(Arc::new(Inner {
            home: Home::new(),
            clock: clock(),
            valid: Mutex::default(),
            seed_valid,
            minted: AtomicUsize::new(0),
        }))
    }

    fn root(&self) -> &Path {
        self.0.home.dir.path()
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root().join(format!("{name}.git"))
    }

    fn store(&self) -> GitContent<ArtifactsHost<Backend, Backend>, MemContent> {
        let clock: Arc<dyn Clock> = self.0.clock.clone();
        GitContent::new(
            ArtifactsHost::new(self.clone(), self.clone(), clock.clone()),
            MemContent::new(),
            committer(),
            clock,
        )
    }

    fn expiry(&self) -> i64 {
        self.0.clock.now().timestamp() + 3600
    }
}

struct Handle {
    dir: PathBuf,
    name: String,
    backend: Backend,
}

impl Handle {
    /// The object `id` when it is of type `kind`.
    fn object(&self, id: &ObjectId, kind: &str) -> Option<Vec<u8>> {
        let t = git(&self.dir, &["cat-file", "-t", &id.to_hex()]);
        if !t.status.success() || String::from_utf8_lossy(&t.stdout).trim() != kind {
            return None;
        }
        Some(git_ok(&self.dir, &["cat-file", kind, &id.to_hex()]))
    }
}

impl ArtifactsApi for Backend {
    type Handle = Handle;

    async fn create(&self, name: &str) -> clax_git::Result<CreatedRepo> {
        let dir = self.dir(name);
        if dir.exists() {
            return Err(GitError::InvalidArgument(format!("{name} exists")));
        }
        git_ok(
            self.root(),
            &["init", "-q", "--bare", "-b", "main", dir.to_str().unwrap()],
        );
        git_ok(&dir, &["config", "http.receivepack", "true"]);
        let token = format!("art_v1_seed?expires={}", self.expiry());
        if self.0.seed_valid {
            self.0.valid.lock().unwrap().insert(token.clone());
        }
        Ok(CreatedRepo {
            remote: format!("{ORIGIN}/git/{name}.git"),
            token: Some(token),
        })
    }

    async fn get(&self, name: &str) -> clax_git::Result<Option<Handle>> {
        let dir = self.dir(name);
        Ok(dir.exists().then(|| Handle {
            dir,
            name: name.to_string(),
            backend: self.clone(),
        }))
    }

    async fn delete(&self, name: &str) -> clax_git::Result<bool> {
        let dir = self.dir(name);
        let had = dir.exists();
        if had {
            std::fs::remove_dir_all(dir).unwrap();
        }
        Ok(had)
    }

    async fn pause(&self, _ms: u32) {}
}

impl ArtifactsRepoApi for Handle {
    async fn remote(&self) -> clax_git::Result<String> {
        Ok(format!("{ORIGIN}/git/{}.git", self.name))
    }

    async fn create_token(&self, _scope: Scope, ttl_s: u32) -> clax_git::Result<ApiToken> {
        let n = self.backend.0.minted.fetch_add(1, Ordering::SeqCst) + 1;
        let expires_s = self.backend.0.clock.now().timestamp() + i64::from(ttl_s);
        let plaintext = format!("art_v1_fresh{n}?expires={expires_s}");
        self.backend
            .0
            .valid
            .lock()
            .unwrap()
            .insert(plaintext.clone());
        Ok(ApiToken {
            id: format!("t{n}"),
            plaintext,
            expires_ms: expires_s * 1000,
        })
    }

    async fn revoke_token(&self, _id: &str) -> clax_git::Result<bool> {
        unimplemented!("not used here")
    }

    async fn fork(&self, to: &str) -> clax_git::Result<CreatedRepo> {
        let dst = self.backend.dir(to);
        git_ok(
            self.backend.root(),
            &[
                "clone",
                "-q",
                "--mirror",
                self.dir.to_str().unwrap(),
                dst.to_str().unwrap(),
            ],
        );
        git_ok(&dst, &["config", "http.receivepack", "true"]);
        Ok(CreatedRepo {
            remote: format!("{ORIGIN}/git/{to}.git"),
            token: None,
        })
    }

    async fn read_commit(&self, id: &ObjectId) -> clax_git::Result<Option<CommitMeta>> {
        match self.object(id, "commit") {
            None => Ok(None),
            Some(data) => Ok(Some(CommitMeta::from_commit(*id, Commit::parse(&data)?))),
        }
    }

    async fn read_tree(&self, id: &ObjectId) -> clax_git::Result<Option<Vec<TreeEntry>>> {
        self.object(id, "tree").map(|d| parse_tree(&d)).transpose()
    }

    async fn read_blob(&self, id: &ObjectId) -> clax_git::Result<Option<Vec<u8>>> {
        Ok(self.object(id, "blob"))
    }

    async fn read_file(&self, rev: &str, path: &str) -> clax_git::Result<Option<Vec<u8>>> {
        let out = git(&self.dir, &["cat-file", "blob", &format!("{rev}:{path}")]);
        Ok(out.status.success().then_some(out.stdout))
    }
}

/// Runs `git http-backend` as CGI for one request.
fn http_backend(root: &Path, request: &HttpRequest) -> HttpResponse {
    let (path, query) = match request.url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (request.url.as_str(), ""),
    };
    let path_info = path
        .strip_prefix(ORIGIN)
        .and_then(|p| p.strip_prefix("/git"))
        .expect("a backend URL");
    let content_type = request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let mut child = Command::new("git")
        .arg("http-backend")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REMOTE_USER", "clax")
        .env("REQUEST_METHOD", request.method)
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", content_type)
        .env("CONTENT_LENGTH", request.body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git http-backend");
    let mut stdin = child.stdin.take().unwrap();
    let body = request.body.clone();
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(&body);
    });
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    let split = out
        .stdout
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("CGI headers");
    let head = String::from_utf8_lossy(&out.stdout[..split]).into_owned();
    let mut status = 200;
    let mut headers = Vec::new();
    for line in head.split("\r\n") {
        let Some((k, v)) = line.split_once(": ") else {
            continue;
        };
        if k.eq_ignore_ascii_case("status") {
            status = v[..3].parse().unwrap();
        } else {
            headers.push((k.to_string(), v.to_string()));
        }
    }
    HttpResponse {
        status,
        headers,
        body: out.stdout[split + 4..].to_vec(),
    }
}

impl Transport for Backend {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let token = request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .and_then(|(_, v)| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !self.0.valid.lock().unwrap().contains(token) {
            return Ok(HttpResponse::new(401, "no"));
        }
        Ok(http_backend(self.root(), &request))
    }
}

#[test]
fn versions_written_over_receive_pack_are_what_git_log_shows_and_read_back() {
    let b = Backend::new(false);
    let c = b.store();
    let id = ArtifactId::generate();
    wait(c.write_version(
        &id,
        1,
        &write(
            &[],
            &[("index.html", b"<h1>one</h1>\n"), ("js/app.js", b"go()\n")],
        ),
    ))
    .unwrap();
    wait(c.write_version(
        &id,
        2,
        &write(&["js/app.js"], &[("index.html", b"<h1>two</h1>\n")]),
    ))
    .unwrap();
    assert_eq!(
        b.0.minted.load(Ordering::SeqCst),
        1,
        "the refused seed token was replaced once"
    );

    let dir = b.dir(&repo_name(id.as_str()));
    let log = String::from_utf8(git_ok(
        &dir,
        &[
            "log",
            "--format=%s|%(trailers:key=Clax-Version,valueonly)",
            "main",
        ],
    ))
    .unwrap();
    assert_eq!(
        log.split_whitespace().collect::<Vec<_>>(),
        ["Version", "2|2", "Version", "1|1"]
    );
    assert_eq!(
        String::from_utf8(git_ok(&dir, &["tag"])).unwrap(),
        "v1\nv2\n"
    );
    assert_eq!(git_ok(&dir, &["show", "v2:js/app.js"]), b"go()\n");

    assert_eq!(
        wait(c.read_file(&id, 1, "index.html")).unwrap().unwrap(),
        b"<h1>one</h1>\n"
    );
    assert_eq!(
        wait(c.read_file(&id, 2, "index.html")).unwrap().unwrap(),
        b"<h1>two</h1>\n"
    );
    assert_eq!(
        wait(c.read_file(&id, 2, "js/app.js")).unwrap().unwrap(),
        b"go()\n"
    );

    // A publish whose recording failed is taken away; the next write builds on.
    wait(c.write_version(&id, 3, &write(&[], &[("index.html", b"<h1>three</h1>\n")]))).unwrap();
    wait(c.remove_version(&id, 3)).unwrap();
    assert_eq!(wait(c.read_file(&id, 3, "index.html")).unwrap(), None);
    assert_eq!(
        String::from_utf8(git_ok(&dir, &["tag"])).unwrap(),
        "v1\nv2\n"
    );
    wait(c.write_version(&id, 3, &write(&[], &[("index.html", b"<h1>again</h1>\n")]))).unwrap();
    assert_eq!(
        git_ok(&dir, &["show", "v3:index.html"]),
        b"<h1>again</h1>\n"
    );
    git_ok(&dir, &["fsck", "--strict", "--no-progress"]);

    // A fork through the binding keeps the history under version 1.
    let to = ArtifactId::generate();
    wait(c.fork(
        &id,
        3,
        &["index.html"],
        &to,
        &clax_core::content::VersionMeta::default(),
    ))
    .unwrap();
    assert_eq!(
        wait(c.read_file(&to, 1, "index.html")).unwrap().unwrap(),
        b"<h1>again</h1>\n"
    );
    let fork = b.dir(&repo_name(to.as_str()));
    assert_eq!(String::from_utf8(git_ok(&fork, &["tag"])).unwrap(), "v1\n");
    assert_eq!(
        String::from_utf8(git_ok(&fork, &["rev-list", "--count", "main"]))
            .unwrap()
            .trim(),
        "4"
    );
}

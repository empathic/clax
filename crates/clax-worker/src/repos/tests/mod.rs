//! Native tests of the repository hosts: the stand-in's database and its
//! internal API under the content store contract, its tokens and
//! must-revoke list, its smart HTTP with real `git` and `jj`, and the
//! Cloudflare Artifacts host over a fake binding, against clax-git's
//! server in process and against `git http-backend`.

mod artifacts_fake;
mod artifacts_http_backend;
mod assets;
mod smart_http;
mod stand_in;

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use clax_core::content::{NewFile, VersionWrite};
use clax_core::working::ManualClock;
use clax_git::Person;
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use rusqlite::Connection;

use super::api::{InternalRequest, RepoService};
use super::local::LocalHost;
use super::tokens::{LocalTokens, TokenHost};

pub fn committer() -> Person {
    Person {
        name: "Clax".into(),
        email: "clax@users.clax.dev".into(),
    }
}

pub fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::at("2026-10-13T12:00:00Z"))
}

/// A stand-in database in memory.
pub fn local_host() -> LocalHost<Connection> {
    LocalHost::new(Connection::open_in_memory().unwrap()).unwrap()
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

/// `RepoDO`'s internal API served in process: what a stub's `fetch`
/// reaches in a Worker.
pub struct InProc<T> {
    pub service: Arc<RepoService<Connection, T>>,
}

impl<T> Clone for InProc<T> {
    fn clone(&self) -> Self {
        InProc {
            service: self.service.clone(),
        }
    }
}

impl<T: TokenHost + Send + Sync> Transport for InProc<T> {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let rest = request
            .url
            .strip_prefix(super::api::BASE)
            .expect("an internal URL");
        let (op, query) = match rest.split_once('?') {
            Some((op, q)) => (op, Some(q)),
            None => (rest, None),
        };
        Ok(self
            .service
            .serve(&InternalRequest {
                method: request.method,
                op,
                query,
                body: &request.body,
            })
            .await)
    }
}

/// The stand-in's service over a fresh database, with its own tokens and
/// `clock`, served in process.
pub fn stand_in(clock: Arc<ManualClock>) -> InProc<LocalTokens<Connection>> {
    let host = local_host();
    InProc {
        service: Arc::new(RepoService {
            tokens: LocalTokens {
                host: host.clone(),
                public_origin: "http://clax.test".into(),
            },
            host,
            clock,
        }),
    }
}

/// A home for git and jj with no configuration of the person running the
/// tests.
pub struct Home {
    pub dir: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Home {
        Home {
            dir: tempfile::Builder::new()
                .prefix("clax-worker-repos-")
                .tempdir()
                .unwrap(),
        }
    }

    /// Runs `program` in `cwd` with this home and `env`.
    pub fn run(&self, program: &str, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(program);
        c.current_dir(cwd)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("JJ_CONFIG", self.dir.path().join("jj.toml"))
            .env("JJ_USER", "Test")
            .env("JJ_EMAIL", "test@clax.test")
            .env("TMPDIR", std::env::temp_dir())
            .stdin(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap_or_else(|e| panic!("run {program}: {e}"))
    }

    /// Runs `program`, which must succeed; answers its trimmed stdout.
    pub fn ok(&self, program: &str, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
        let out = self.run(program, cwd, args, env);
        assert!(
            out.status.success(),
            "{program} {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

/// The jj to run: `CLAX_TEST_JJ` (which must run when set), else `jj` on
/// `PATH` when there is one.
pub fn jj() -> Option<String> {
    let runs = |p: &str| {
        Command::new(p)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if let Ok(p) = std::env::var("CLAX_TEST_JJ") {
        assert!(runs(&p), "CLAX_TEST_JJ={p} does not run");
        return Some(p);
    }
    runs("jj").then(|| "jj".to_string())
}

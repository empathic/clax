//! A GitHub App's tokens and commits, against a fake GitHub that checks what
//! the real one is documented to: a JWT signed by the App's key, tokens
//! narrowed as asked, and the Git Data routes, served over a real bare
//! repository so that what lands is what `git` says it is. Whether GitHub
//! then marks a commit verified is not something a fake can show.

use crate::fixture::{basic, git, http_backend};
use crate::recovery::{World, another_proposal, edit, file_at, proposal, run, sh};
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use clax_bot::workspace::{CODE_CHANGE_REFUSED, Workspace};
use clax_bot_workspace::forge::{Forge, GitHub, NewPullRequest, UnsignedCommits, VERIFY_ATTEMPTS};
use clax_bot_workspace::github_app::{GitHubApp, GitHubAppAccess};
use clax_bot_workspace::repo::{Redactor, Remembered, RepoAccess, StaticAccess, Token};
use clax_bot_workspace::runtime::ContainerSpec;
use clax_bot_workspace::workspace::{ContainerWorkspace, Target};
use ring::signature::{self, KeyPair};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Made for these tests and nothing else.
pub(crate) const KEY: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEpQIBAAKCAQEAq3WZuIsnf9SqGWJ0K7IqcSusFxMW7HwcLwI4TxYI+OR3viPN
NHvwLqP0h6ZPV5vyVdrIeFd3Ml5XqP88/C2jRwXEsr5PfNwT9qQG3eiSAs7i1o15
Vc5MuuslZ+D3KmIcJY3kk+q7ok6SpmFC90fZ6uur2UT0if5E2nBiSZBmZB++xs6X
euZbRZfZ8qVe/TdugHXzJrdI+Vtd5rpnSWSGxJWvR/kGxULBi8YydGV516vUwT+m
FBbVLabRxnnH/Hz8Meqw4UoLQYM4dGU6g4CHZbCUoR3aNAh6qUpJicWxi6Y/+AN4
7a1B2oLFswkxltImL7wi8MhYKow+S7schSPEWwIDAQABAoIBAQCXeASc6xGx9AVw
93D5SSQFZjdZCxwtPlcHkEZ+V1EFz4+ZAbe+rd641U6op1tgYfvrrKN87If4jy+R
tCoD/a1IiOgDn+xIuYNJWA5Y66RRpsLuF30hOGbAmgspgavYXWPQKVHM+olYEWGs
ai/mlCsHRbtC7Y+6eE4LS00I/+/0ckTD2dpVkgQ5BUPvQJjO3NpireeDqx2nfVRx
40i7zuAtMHjjzdrlsqqlVSX+gayAd0NDaxK+YwrbyoUFEZfh5WS/SazlQa5ZDRZE
vbh6Iel9vb1HyWTPnrNwus3rpSZkAwcLiwe8W47qnMSKmZSJGHwcjTCjroH53qc1
OZokWC3BAoGBANHU8YKwNiiYjhcg1PxGdk6blnTku3NFPunbRxdWl/2VmlhwoD1L
gE7rLrEeQVBRZ2ngyrQ7HDZoWGVIRR9Pvu+cBOG1Z/MWZbZP82qvf+ZIwdp2lP3B
KiUQ5Z51cESVSwOq2yIS3JIAJJ52ATGxB8mKb4BkPY0bTkqOfjvEVx11AoGBANEv
R1ESOu0MApf2Q6sgWv63Kzf/nGP3IZm4pEIM8SFO4tVszns2j2biGt5jg5ZcMPIH
Ms3LlQVuOkrjxRp9/YQFpoHQTojSYfWqPvTmhxq6xhHDNrPw9eE31KHajgPBblLp
W+ZVXukPe5+VMOU/1HStdwZw0BnMxZsTTHH4txCPAoGAVLyr81z54hcJeqmVmXNT
H5eRgQ9F690Q9RvKb9BWfN/HfuOsslQlMrmjF3/iXG8jVgyjj3k3gXlOCUjQMT7h
Gp5lEklR+0cWODO9V14cRoEnV/P/YyJ4xJM4R2+hsPChZI/BAtefH47TOvLotIqL
XIZ9VNomSS+3xEAbFkPJ8mkCgYEAiyo49+Z47VsgHpjnS+Gqp+qUBh03ngm7gNkl
7gtOICqrHfHVLIOPw5KI7Yqm8FFE/r9rPR5DdelvPoNpjcZj8oEP1S0QElZkdVtd
pu/20gGP5CFLM0sLSVorS8Jq2HTHQMGfMg15pvN1+yT18o4UecstdEt5JrDkhdbA
9NHSeGECgYEAjebvk4v0qfKVs45pZevWLu90hzGS/aEwBe1cbO/Znt5EmDpDZVmG
Vuvd4h/iBTBvDwyTcLrKgCN91rrbw5DibOpHlikiXHHGNkH7splQz04F/mipJwFC
2Jm3PRCKsIoa1HDqL+9xNHSNXC20i9FpmTz60kTG/Do6GP7xC6bPoDo=
-----END RSA PRIVATE KEY-----";

pub(crate) const APP_ID: u64 = 4242;
pub(crate) const INSTALLATION: u64 = 7;
const START: u64 = 1_800_000_000;
const BRANCH: &str = "claxbot/thread1";

/// What the fake saw, and how it misbehaves next.
#[derive(Default)]
struct Saw {
    calls: usize,
    /// The body of each request for a token.
    minted: Vec<Value>,
    /// Tokens that may write.
    writers: Vec<String>,
    blobs: usize,
    /// The body of each commit made.
    commits: Vec<Value>,
    pulls: Vec<Value>,
    /// What each new pull request was opened with.
    opened: Vec<Value>,
    /// What each pull request was changed to, by route.
    described: Vec<(String, Value)>,
    /// Moves that many refs, and answers as though they failed.
    lost_answers: usize,
    /// Fails that many ref moves.
    failed_moves: usize,
    /// Answers every call so, quoting its `Authorization` at `{auth}`.
    canned: Option<(u16, String)>,
    /// Tokens ended, in order.
    revoked: Vec<String>,
    /// Answers a revocation with a server's error.
    revoke_fails: bool,
    /// Answers that many forge calls 401.
    unauthorized: usize,
    /// How many reads of a commit go by before it is verified; `None` is
    /// verified at once, and a new commit's answer then has no `verification`.
    verified_after: Option<usize>,
    /// How many times a commit was read.
    reads: usize,
    /// Answers a tree with 422.
    refuses_trees: bool,
    /// Answers a tree with another tree's ID.
    lies_about_trees: bool,
    /// `/o/r.git` serves anyone, as a public repository does.
    open_git: bool,
}

#[derive(Clone)]
pub(crate) struct Fake {
    bare: PathBuf,
    pub(crate) base: String,
    public_key: Arc<Vec<u8>>,
    now: Arc<AtomicU64>,
    saw: Arc<Mutex<Saw>>,
}

fn der_of(pem: &str) -> Vec<u8> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    STANDARD.decode(body).unwrap()
}

impl Fake {
    pub(crate) async fn start(bare: &Path) -> Fake {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let key = signature::RsaKeyPair::from_der(&der_of(KEY)).unwrap();
        let fake = Fake {
            bare: bare.to_path_buf(),
            base: format!("http://{}", listener.local_addr().unwrap()),
            public_key: Arc::new(key.public_key().as_ref().to_vec()),
            now: Arc::new(AtomicU64::new(START)),
            saw: Arc::default(),
        };
        let app = Router::new().fallback(handle).with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        fake
    }

    fn saw<T>(&self, f: impl FnOnce(&mut Saw) -> T) -> T {
        f(&mut self.saw.lock().unwrap())
    }

    /// This fake, keeping the time a client that reads the clock does.
    pub(crate) fn on_the_system_clock(&self) {
        let now = std::time::SystemTime::now();
        let now = now.duration_since(std::time::UNIX_EPOCH).unwrap();
        self.now.store(now.as_secs(), Ordering::SeqCst);
    }

    pub(crate) fn commits_made(&self) -> usize {
        self.saw(|s| s.commits.len())
    }

    /// Every token minted: the read ones, then the write ones.
    pub(crate) fn tokens(&self) -> (Vec<String>, Vec<String>) {
        self.saw(|s| {
            let all = (0..s.minted.len()).map(|n| format!("ghs_fake_{n}"));
            all.partition(|t| !s.writers.contains(t))
        })
    }

    pub(crate) fn revoked(&self) -> Vec<String> {
        self.saw(|s| s.revoked.clone())
    }

    pub(crate) fn never_verifies(&self) {
        self.saw(|s| s.verified_after = Some(usize::MAX));
    }

    pub(crate) fn opens_git_to_anyone(&self) {
        self.saw(|s| s.open_git = true);
    }

    /// Whether `authorization` is a token minted and not ended.
    fn may_read(&self, authorization: &str) -> bool {
        self.saw(|s| {
            s.open_git
                || (0..s.minted.len())
                    .map(|n| format!("ghs_fake_{n}"))
                    .any(|t| authorization == basic(&t) && !s.revoked.contains(&t))
        })
    }

    fn app(&self) -> GitHubApp {
        let now = self.now.clone();
        GitHubApp::new(&self.base, APP_ID, KEY)
            .unwrap()
            .with_clock(Arc::new(move || now.load(Ordering::SeqCst)))
    }

    fn access(&self, public: bool) -> GitHubAppAccess {
        GitHubAppAccess::new(self.app(), INSTALLATION, "o/r", public).unwrap()
    }

    fn now(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    /// Why the JWT in `authorization` would be refused.
    fn jwt_refusal(&self, authorization: &str) -> Option<String> {
        let Some(jwt) = authorization.strip_prefix("Bearer ") else {
            return Some("no bearer".to_string());
        };
        let parts: Vec<&str> = jwt.split('.').collect();
        let [head, claims, signed] = parts[..] else {
            return Some("not three parts".to_string());
        };
        let read = |part: &str| -> Value {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
        };
        if read(head)["alg"] != "RS256" {
            return Some("not RS256".to_string());
        }
        let key = signature::UnparsedPublicKey::new(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            self.public_key.as_slice(),
        );
        let sig = URL_SAFE_NO_PAD.decode(signed).unwrap();
        let message = format!("{head}.{claims}");
        if key.verify(message.as_bytes(), &sig).is_err() {
            return Some("a bad signature".to_string());
        }
        let c = read(claims);
        let (iat, exp) = (c["iat"].as_u64().unwrap(), c["exp"].as_u64().unwrap());
        let now = self.now();
        if c["iss"].as_str() != Some(APP_ID.to_string().as_str()) {
            return Some(format!("iss is {}", c["iss"]));
        }
        // Backdated for clock drift, and no more than ten minutes ahead.
        if iat + 30 > now || exp <= now || exp > now + 600 {
            return Some(format!("iat {iat} exp {exp} at {now}"));
        }
        None
    }

    fn mint(&self, body: Value) -> Response {
        let token = self.saw(|s| {
            let token = format!("ghs_fake_{}", s.minted.len());
            if body["permissions"]["contents"] == "write" {
                s.writers.push(token.clone());
            }
            s.minted.push(body.clone());
            token
        });
        let expires = i64::try_from(self.now() + 3600).unwrap();
        let at = chrono::DateTime::from_timestamp(expires, 0).unwrap();
        let at = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        answer(201, json!({"token": token, "expires_at": at}))
    }

    fn blob(&self, body: &Value) -> Response {
        assert_eq!(body["encoding"], "base64");
        let bytes = STANDARD.decode(body["content"].as_str().unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("blob");
        std::fs::write(&file, bytes).unwrap();
        let sha = git(
            &self.bare,
            &["hash-object", "-w", "--no-filters", file.to_str().unwrap()],
        );
        self.saw(|s| s.blobs += 1);
        answer(201, json!({"sha": sha}))
    }

    fn tree(&self, body: &Value) -> Response {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("index");
        let with_index = |args: &[&str]| -> String {
            let out = Command::new("git")
                .args(args)
                .current_dir(&self.bare)
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_WORK_TREE", dir.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        with_index(&["read-tree", body["base_tree"].as_str().unwrap()]);
        for entry in body["tree"].as_array().unwrap() {
            let path = entry["path"].as_str().unwrap();
            let mode = entry["mode"].as_str().unwrap();
            assert_eq!(entry["type"], "blob");
            assert!(["100644", "100755"].contains(&mode), "{mode}");
            match entry["sha"].as_str() {
                Some(sha) => {
                    let info = format!("{mode},{sha},{path}");
                    with_index(&["update-index", "--add", "--cacheinfo", &info]);
                }
                None => {
                    with_index(&["update-index", "--force-remove", "--", path]);
                }
            }
        }
        let (refuses, lies) = self.saw(|s| (s.refuses_trees, s.lies_about_trees));
        if refuses {
            return answer(422, json!({"message": "tree.sha is not valid"}));
        }
        let sha = with_index(&["write-tree"]);
        let sha = if lies { "0".repeat(40) } else { sha };
        answer(201, json!({"sha": sha}))
    }

    fn commit(&self, body: Value) -> Response {
        let tree = body["tree"].as_str().unwrap();
        let parent = body["parents"][0].as_str().unwrap();
        let message = body["message"].as_str().unwrap();
        let args = ["commit-tree", tree, "-p", parent, "-m", message];
        let sha = git(&self.bare, &args);
        self.saw(|s| s.commits.push(body.clone()));
        if self.saw(|s| s.verified_after.is_some()) {
            return answer(201, json!({"sha": sha}));
        }
        let verification = json!({"verified": true, "reason": "valid"});
        answer(201, json!({"sha": sha, "verification": verification}))
    }

    fn read_commit(&self, sha: &str) -> Response {
        let verified = self.saw(|s| {
            s.reads += 1;
            s.verified_after.is_none_or(|n| s.reads > n)
        });
        let verification = json!({"verified": verified, "reason": "unsigned"});
        answer(200, json!({"sha": sha, "verification": verification}))
    }

    /// Ends the token that asks, as `DELETE /installation/token` does.
    fn revoke(&self, authorization: &str) -> Response {
        let token = authorization.strip_prefix("Bearer ").unwrap().to_string();
        let fails = self.saw(|s| {
            s.revoked.push(token);
            s.revoke_fails
        });
        let status = if fails { 500 } else { 204 };
        Response::builder()
            .status(status)
            .body(Body::empty())
            .unwrap()
    }

    /// Moves a ref as GitHub does: a new one must be new, and an old one
    /// moves forward only.
    fn move_ref(&self, name: &str, sha: &str, new: bool) -> Response {
        let exists = ok(&self.bare, &["rev-parse", "--verify", "--quiet", name]);
        let forward = new || ok(&self.bare, &["merge-base", "--is-ancestor", name, sha]);
        if exists == new || !forward {
            return answer(422, json!({"message": "Reference update failed"}));
        }
        let (failed, lost) = self.saw(|s| {
            let failed = s.failed_moves > 0;
            s.failed_moves = s.failed_moves.saturating_sub(1);
            let lost = !failed && s.lost_answers > 0;
            s.lost_answers -= usize::from(lost);
            (failed, lost)
        });
        if !failed {
            git(&self.bare, &["update-ref", name, sha]);
        }
        if failed || lost {
            return answer(502, json!({"message": "Bad gateway"}));
        }
        answer(200, json!({"ref": name}))
    }
}

fn ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
        .status
        .success()
}

fn answer(status: u16, body: Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn handle(State(f): State<Fake>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let auth = parts
        .headers
        .get(header::AUTHORIZATION)
        .map_or("", |v| v.to_str().unwrap())
        .to_string();
    let path = parts.uri.path().to_string();
    let canned = f.saw(|s| {
        s.calls += 1;
        s.canned.clone()
    });
    if let Some((status, text)) = canned {
        return Response::builder()
            .status(status)
            .header(header::LOCATION, format!("{}/elsewhere", f.base))
            .body(Body::from(text.replace("{auth}", &auth)))
            .unwrap();
    }
    if parts.method == Method::DELETE && path == "/installation/token" {
        return f.revoke(&auth);
    }
    if let Some(rest) = path.strip_prefix("/o/r.git/") {
        if !f.may_read(&auth) {
            return Response::builder()
                .status(401)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"git\"")
                .body(Body::empty())
                .unwrap();
        }
        let query = parts.uri.query().unwrap_or_default();
        let uri = format!("/repo.git/{rest}?{query}").parse().unwrap();
        let root = f.bare.parent().unwrap();
        return http_backend(root, &parts.method, &uri, &parts.headers, bytes).await;
    }
    if path.starts_with("/app/") || path.ends_with("/installation") {
        if let Some(why) = f.jwt_refusal(&auth) {
            return answer(401, json!({"message": why}));
        }
        let tokens = format!("/app/installations/{INSTALLATION}/access_tokens");
        return match (&parts.method, path.as_str()) {
            (&Method::POST, p) if p == tokens => f.mint(body),
            (&Method::GET, "/repos/o/r/installation") => {
                answer(200, json!({"id": INSTALLATION, "account": {"login": "o"}}))
            }
            _ => answer(404, json!({"message": "Not Found"})),
        };
    }
    let writer = auth
        .strip_prefix("Bearer ")
        .is_some_and(|t| f.saw(|s| s.writers.iter().any(|w| w == t)));
    if !writer {
        return answer(403, json!({"message": "Resource not accessible"}));
    }
    let stale = f.saw(|s| {
        let stale = s.unauthorized > 0;
        s.unauthorized = s.unauthorized.saturating_sub(1);
        stale
    });
    if stale {
        return answer(401, json!({"message": "Bad credentials"}));
    }
    let sha = body["sha"].as_str().unwrap_or_default();
    match (&parts.method, path.strip_prefix("/repos/o/r/").unwrap()) {
        (&Method::POST, "git/blobs") => f.blob(&body),
        (&Method::POST, "git/trees") => f.tree(&body),
        (&Method::POST, "git/commits") => f.commit(body),
        (&Method::GET, p) if p.starts_with("git/commits/") => {
            f.read_commit(p.strip_prefix("git/commits/").unwrap())
        }
        (&Method::POST, "git/refs") => f.move_ref(body["ref"].as_str().unwrap(), sha, true),
        (&Method::PATCH, p) if p.starts_with("git/refs/") => {
            assert_ne!(body["force"], true, "a ref is never forced");
            let name = format!("refs/{}", p.strip_prefix("git/refs/").unwrap());
            f.move_ref(&name, sha, false)
        }
        (&Method::GET, "pulls") => answer(200, json!(f.saw(|s| s.pulls.clone()))),
        (&Method::POST, "pulls") => {
            let number = f.saw(|s| s.pulls.len()) + 1;
            let pr = json!({
                "number": number,
                "html_url": format!("https://forge.example/pr/{number}"),
                "head": {"ref": body["head"]},
            });
            f.saw(|s| {
                s.pulls.push(pr.clone());
                s.opened.push(body.clone());
            });
            answer(201, pr)
        }
        (&Method::GET, p) if p.starts_with("pulls/") => {
            answer(200, json!({"body": "as it stands"}))
        }
        (&Method::PATCH, p) if p.starts_with("pulls/") => {
            f.saw(|s| s.described.push((p.to_string(), body.clone())));
            answer(200, json!({}))
        }
        _ => answer(404, json!({"message": "Not Found"})),
    }
}

async fn alone() -> Fake {
    Fake::start(Path::new("/nonexistent")).await
}

#[tokio::test]
async fn a_read_token_is_signed_for_by_the_app_and_narrowed_to_reading_one_repository() {
    let f = alone().await;
    let token = f.access(false).read_token().await.unwrap().unwrap();
    assert_eq!(token.expose(), "ghs_fake_0");
    assert_eq!(
        f.saw(|s| s.minted.clone()),
        [json!({"repositories": ["r"], "permissions": {"contents": "read"}})]
    );
}

#[tokio::test]
async fn a_write_token_may_write_contents_and_pull_requests_and_never_workflows() {
    let f = alone().await;
    f.access(false).write_token().await.unwrap().unwrap();
    assert_eq!(
        f.saw(|s| s.minted.clone()),
        [json!({
            "repositories": ["r"],
            "permissions": {"contents": "write", "pull_requests": "write"},
        })]
    );
}

#[tokio::test]
async fn a_public_repository_gives_a_container_no_token_at_all() {
    let f = alone().await;
    assert!(f.access(true).read_token().await.unwrap().is_none());
    assert_eq!(f.saw(|s| s.calls), 0);
}

#[tokio::test]
async fn a_token_is_kept_until_its_margin_before_expiry_and_each_kind_has_its_own() {
    let f = alone().await;
    let access = f.access(false);
    let first = access.write_token().await.unwrap();
    assert_eq!(access.write_token().await.unwrap(), first);
    assert_eq!(f.saw(|s| s.minted.len()), 1);

    // A git may run for ten minutes on a token it was given at the start.
    f.now.store(START + 3600 - 601, Ordering::SeqCst);
    assert_eq!(access.write_token().await.unwrap(), first);
    f.now.store(START + 3600 - 600, Ordering::SeqCst);
    assert_ne!(access.write_token().await.unwrap(), first);
    assert_eq!(f.saw(|s| s.minted.len()), 2);
}

#[tokio::test]
async fn a_read_token_is_never_kept() {
    let f = alone().await;
    let access = f.access(false);
    let first = access.read_token().await.unwrap();
    assert_ne!(access.read_token().await.unwrap(), first);
    assert_eq!(f.saw(|s| s.minted.len()), 2);
}

#[tokio::test]
async fn a_token_the_forge_refuses_is_not_handed_out_again() {
    let f = alone().await;
    let access = Arc::new(f.access(false));
    let forge = GitHub::new(&f.base, "o/r", access.clone());
    let first = access.write_token().await.unwrap();
    f.saw(|s| s.unauthorized = 1);
    forge.find_open("b").await.unwrap_err();
    assert_ne!(access.write_token().await.unwrap(), first);
}

#[tokio::test]
async fn a_forge_given_a_bare_access_quotes_no_token_of_its_own() {
    let f = alone().await;
    f.saw(|s| s.canned = Some((422, "you sent {auth}".to_string())));
    let access = StaticAccess {
        read: None,
        write: Some(Token::new("s3cret-forge-token")),
    };
    let forge = GitHub::new(&f.base, "o/r", Arc::new(access));
    let e = forge.find_open("b").await.unwrap_err();
    let text = format!("{e:#}");
    assert!(text.contains("422"), "{text}");
    assert!(!text.contains("s3cret-forge-token"), "{text}");
}

#[tokio::test]
async fn what_is_issued_reaches_the_redactor() {
    let f = alone().await;
    let secrets = Arc::new(Redactor::default());
    let access = Remembered {
        inner: Arc::new(f.access(false)),
        redactor: secrets.clone(),
    };
    let token = access.write_token().await.unwrap().unwrap();
    let text = format!("saw {}", token.expose());
    assert_eq!(secrets.clean(&text), "saw [redacted]");
}

#[tokio::test]
async fn a_refusal_from_the_forge_names_no_secret() {
    let f = alone().await;
    f.saw(|s| s.canned = Some((401, "you sent {auth}".to_string())));
    let e = f.access(false).write_token().await.unwrap_err();
    let text = format!("{e:#} {e:?}");
    assert!(text.contains("401"), "{text}");
    assert!(!text.contains("eyJ"), "the JWT leaked: {text}");
}

#[tokio::test]
async fn a_redirect_is_not_followed_with_the_apps_signature() {
    let f = alone().await;
    f.saw(|s| s.canned = Some((302, String::new())));
    assert!(f.access(false).write_token().await.is_err());
    assert_eq!(f.saw(|s| s.calls), 1);
}

#[test]
fn a_key_that_will_not_load_is_not_quoted() {
    let bad = "-----BEGIN RSA PRIVATE KEY-----\nc2VjcmV0\n-----END RSA PRIVATE KEY-----\n";
    for pem in [bad, "c2VjcmV0", ""] {
        let Err(e) = GitHubApp::new("http://127.0.0.1:1", APP_ID, pem) else {
            panic!("a key that is no key loaded");
        };
        assert!(!format!("{e:#}").contains("c2VjcmV0"));
    }
}

#[test]
fn a_repository_needs_an_owner() {
    let app = GitHubApp::new("http://127.0.0.1:1", APP_ID, KEY).unwrap();
    assert!(GitHubAppAccess::new(app, INSTALLATION, "no-owner", false).is_err());
}

#[tokio::test]
async fn an_installation_is_found_for_a_repository() {
    let f = alone().await;
    let found = f.app().installation_of("o/r").await.unwrap().unwrap();
    assert_eq!((found.id, found.account.as_str()), (INSTALLATION, "o"));
    assert_eq!(
        f.app().installation_of("o/not-installed").await.unwrap(),
        None
    );
}

/// `world`'s target, committing through `f`.
fn committing(world: &World, f: &Fake) -> Target {
    let access: Arc<dyn RepoAccess> = Arc::new(f.access(false));
    let forge = GitHub::new(&f.base, "o/r", access.clone());
    Target {
        access,
        forge: Arc::new(forge.committing().with_verify_delay(Duration::ZERO)),
        unsigned: UnsignedCommits::Refuse,
        ..world.target()
    }
}

fn daemon(world: &World, target: Target) -> ContainerWorkspace {
    ContainerWorkspace::keeping(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        vec![target],
        false,
        world.root.path().join("ledger"),
        world.secrets.clone(),
    )
}

async fn started() -> (World, Fake, ContainerWorkspace) {
    let world = World::new();
    let f = Fake::start(&world.bare).await;
    let w = daemon(&world, committing(&world, &f));
    (world, f, w)
}

fn bytes_at(bare: &Path, rev: &str, path: &str) -> Vec<u8> {
    let out = Command::new("git")
        .args(["cat-file", "blob", &format!("{rev}:{path}")])
        .current_dir(bare)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    out.stdout
}

#[tokio::test]
async fn a_change_is_committed_by_the_forge_with_nothing_of_ours_on_the_commit() {
    let (world, f, w) = started().await;
    let r = run("r1");
    let change = "printf 'fixed\\n' > app.txt && rm tool.sh && printf '\\000\\377\\001' > logo.bin \
                  && printf '#!/bin/sh\\n' > run.sh && chmod +x run.sh && mkdir -p a/b \
                  && printf 'deep\\n' > 'a/b/sp ace.txt'";
    assert_eq!(sh(&w, &r, change).await.unwrap()["exit_code"], 0);
    let done = w.propose(&r, &proposal()).await.unwrap();

    assert_eq!(done["pr"], 1);
    assert_eq!(done["commit"], world.pushed(BRANCH).as_str());
    assert_eq!(world.pushed(&format!("{BRANCH}~1")), world.pushed("main"));
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "fixed");
    assert_eq!(file_at(&world.bare, BRANCH, "a/b/sp ace.txt"), "deep");
    assert_eq!(bytes_at(&world.bare, BRANCH, "logo.bin"), [0, 0xff, 1]);
    let listed = git(&world.bare, &["ls-tree", "-r", BRANCH]);
    assert!(!listed.contains("tool.sh"), "{listed}");
    assert!(listed.contains("100755 blob"), "{listed}");
    assert!(listed.contains(".github/ci.yml"), "{listed}");
    assert!(listed.contains("120000 blob"), "the rest stays: {listed}");

    // An author, committer or signature of ours would lose GitHub's.
    let commits = f.saw(|s| s.commits.clone());
    let [commit] = &commits[..] else {
        panic!("{commits:?}")
    };
    let mut keys: Vec<&String> = commit.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["message", "parents", "tree"]);
    assert_eq!(f.saw(|s| s.blobs), 4, "a deletion uploads nothing");
}

#[tokio::test]
async fn a_commits_subject_is_the_title_as_it_was_written_but_for_an_issues_number() {
    let (_world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    let title = "fix: Option<String> for https://example.com #12";
    let mut p = proposal();
    p.title = title.to_string();
    w.propose(&r, &p).await.unwrap();
    let message = f.saw(|s| s.commits[0]["message"].as_str().unwrap().to_string());
    let subject = message.lines().next().unwrap();
    assert_eq!(subject, title.replace("#1", "#\u{200b}1"));
}

#[tokio::test]
async fn a_second_proposal_continues_from_the_commit_the_forge_made() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    sh(&w, &r, "printf 'gone soon\\n' > extra.txt")
        .await
        .unwrap();
    let first = w.propose(&r, &proposal()).await.unwrap();
    edit(&w, &r, "second\n").await;
    sh(&w, &r, "rm extra.txt").await.unwrap();
    let second = w.propose(&r, &another_proposal()).await.unwrap();

    assert_ne!(first["commit"], second["commit"]);
    assert_eq!(second["commit"], world.pushed(BRANCH).as_str());
    let parent = world.pushed(&format!("{BRANCH}~1"));
    assert_eq!(first["commit"], parent.as_str());
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "second");
    let listed = git(&world.bare, &["ls-tree", "-r", BRANCH]);
    assert!(!listed.contains("extra.txt"), "{listed}");
    assert_eq!(f.saw(|s| s.commits.len()), 2);
    assert_eq!(f.saw(|s| s.pulls.len()), 1);
}

#[tokio::test]
async fn proposing_again_under_a_key_answers_as_before_and_commits_nothing() {
    let (_world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    let first = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(w.propose(&r, &proposal()).await.unwrap(), first);
    assert_eq!(f.saw(|s| s.commits.len()), 1);
}

#[tokio::test]
async fn a_ref_that_moved_though_its_answer_was_lost_is_not_committed_again() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.lost_answers = 1);
    w.propose(&r, &proposal()).await.unwrap_err();
    let tip = world.pushed(BRANCH);

    let done = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["commit"], tip.as_str());
    assert_eq!(world.pushed(BRANCH), tip);
    assert_eq!(f.saw(|s| s.commits.len()), 1);
}

#[tokio::test]
async fn a_ref_that_did_not_move_takes_the_commit_already_made() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.failed_moves = 1);
    w.propose(&r, &proposal()).await.unwrap_err();
    let args = ["rev-parse", "--verify", "--quiet", BRANCH];
    assert!(!ok(&world.bare, &args));

    let done = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["commit"], world.pushed(BRANCH).as_str());
    assert_eq!(f.saw(|s| s.commits.len()), 1);
}

#[tokio::test]
async fn a_failed_move_under_another_key_takes_the_commit_already_made() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.failed_moves = 1);
    w.propose(&r, &proposal()).await.unwrap_err();

    let done = w.propose(&r, &another_proposal()).await.unwrap();
    assert_eq!(done["commit"], world.pushed(BRANCH).as_str());
    assert_eq!(world.pushed(&format!("{BRANCH}~1")), world.pushed("main"));
    assert_eq!(f.saw(|s| s.commits.len()), 1);
}

#[tokio::test]
async fn a_commit_verified_only_on_reading_it_again_is_published() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.verified_after = Some(1));
    let done = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["commit"], world.pushed(BRANCH).as_str());
    assert_eq!(f.saw(|s| s.reads), 2);
}

/// Fails unless `e` refuses for good, and does not invite another try.
fn refused_for_good(e: &clax_bot_sandbox::HostError) {
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(
        !e.message.to_lowercase().contains("try again"),
        "{}",
        e.message
    );
}

fn no_branch(world: &World) {
    let args = ["rev-parse", "--verify", "--quiet", BRANCH];
    assert!(!ok(&world.bare, &args), "the branch was made");
}

#[tokio::test]
async fn a_commit_never_verified_stays_on_no_branch() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.verified_after = Some(usize::MAX));
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    refused_for_good(&e);
    assert!(e.message.contains("sign"), "{}", e.message);
    no_branch(&world);
    assert_eq!(f.saw(|s| s.reads), VERIFY_ATTEMPTS as usize);

    // Nothing unverified is kept for another proposal to take.
    let e = w.propose(&r, &another_proposal()).await.unwrap_err();
    refused_for_good(&e);
    no_branch(&world);
}

#[tokio::test]
async fn a_target_that_allows_unsigned_commits_publishes_an_unverified_one() {
    let world = World::new();
    let f = Fake::start(&world.bare).await;
    let target = Target {
        unsigned: UnsignedCommits::Allow,
        ..committing(&world, &f)
    };
    let w = daemon(&world, target);
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.verified_after = Some(usize::MAX));
    let done = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["commit"], world.pushed(BRANCH).as_str());
}

#[tokio::test]
async fn a_tree_the_forge_refuses_or_gets_wrong_is_refused_for_good() {
    for lies in [false, true] {
        let (world, f, w) = started().await;
        let r = run("r1");
        edit(&w, &r, "fixed\n").await;
        f.saw(|s| {
            s.refuses_trees = !lies;
            s.lies_about_trees = lies;
        });
        refused_for_good(&w.propose(&r, &proposal()).await.unwrap_err());
        no_branch(&world);
        assert_eq!(f.saw(|s| s.commits.len()), 0);
    }
}

#[tokio::test]
async fn each_clone_has_a_read_token_of_its_own_ended_when_it_returns() {
    let (_world, f, w) = started().await;
    for id in ["r1", "r2"] {
        sh(&w, &run(id), "true").await.unwrap();
    }
    let minted = f.saw(|s| s.minted.clone());
    let read = |m: &&Value| m["permissions"]["contents"] == "read";
    assert_eq!(minted.iter().filter(read).count(), 2);
    let revoked = f.saw(|s| s.revoked.clone());
    assert_eq!(revoked.len(), 2, "{revoked:?}");
    assert_ne!(revoked[0], revoked[1]);
    let writers = f.saw(|s| s.writers.clone());
    assert!(!revoked.iter().any(|t| writers.contains(t)), "{revoked:?}");
}

#[tokio::test]
async fn a_clone_that_fails_still_ends_its_token() {
    let world = World::new();
    let f = Fake::start(&world.bare).await;
    let target = Target {
        container_repo_url: "/nonexistent/repo.git".to_string(),
        ..committing(&world, &f)
    };
    let w = daemon(&world, target);
    sh(&w, &run("r1"), "true").await.unwrap_err();
    assert_eq!(f.saw(|s| s.revoked.len()), 1);
}

#[tokio::test]
async fn a_revocation_that_fails_does_not_stop_the_run() {
    let (_world, f, w) = started().await;
    f.saw(|s| s.revoke_fails = true);
    let answer = sh(&w, &run("r1"), "true").await.unwrap();
    assert_eq!(answer["exit_code"], 0);
    assert_eq!(f.saw(|s| s.revoked.len()), 1);
}

#[tokio::test]
async fn a_restart_after_a_lost_answer_knows_the_commit_as_its_own() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    f.saw(|s| s.lost_answers = 1);
    w.propose(&r, &proposal()).await.unwrap_err();
    drop(w);

    let again = daemon(&world, committing(&world, &f));
    let done = again.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["commit"], world.pushed(BRANCH).as_str());
    assert_eq!(f.saw(|s| s.commits.len()), 1);
}

#[tokio::test]
async fn a_commit_of_someone_elses_on_the_branch_is_left_there() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    world.someone_pushes(BRANCH, BRANCH);
    let theirs = world.pushed(BRANCH);

    edit(&w, &r, "second\n").await;
    let e = w.propose(&r, &another_proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert_eq!(world.pushed(BRANCH), theirs);
    assert_eq!(f.saw(|s| s.commits.len()), 1);
}

#[tokio::test]
async fn a_title_notifies_nobody_any_more_than_a_body_does() {
    let f = alone().await;
    let forge = GitHub::new(&f.base, "o/r", Arc::new(f.access(false)));
    let pr = NewPullRequest {
        title: "for @someone".to_string(),
        body: "and @another".to_string(),
        head: "claxbot/a".to_string(),
        base: "main".to_string(),
    };
    forge.open(&pr).await.unwrap();
    let opened = f.saw(|s| s.opened[0].clone());
    assert_eq!(opened["title"], "for @\u{200b}someone");
    assert_eq!(opened["body"], "and @\u{200b}another");
}

#[tokio::test]
async fn a_pull_requests_text_is_read_and_replaced_as_it_is_given() {
    let f = alone().await;
    let forge = GitHub::new(&f.base, "o/r", Arc::new(f.access(false)));
    assert_eq!(forge.body(7).await.unwrap(), "as it stands");
    forge.describe(7, "a person's @someone").await.unwrap();
    let (route, body) = f.saw(|s| s.described[0].clone());
    assert_eq!(route, "pulls/7");
    assert_eq!(body["body"], "a person's @someone");
}

#[tokio::test]
async fn only_branches_under_the_prefix_count_as_open() {
    let f = alone().await;
    let forge = GitHub::new(&f.base, "o/r", Arc::new(f.access(false)));
    for head in ["claxbot/a", "claxbot/b-2", "someones-branch"] {
        let pr = NewPullRequest {
            title: "t".to_string(),
            body: "b".to_string(),
            head: head.to_string(),
            base: "main".to_string(),
        };
        forge.open(&pr).await.unwrap();
    }
    assert_eq!(forge.open_under("claxbot/").await.unwrap(), 2);
}

#[tokio::test]
async fn the_policy_judges_a_change_before_the_forge_hears_of_it() {
    let (_world, f, w) = started().await;
    let r = run("r1");
    sh(&w, &r, "printf 'on: x\\n' > .github/ci.yml")
        .await
        .unwrap();
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(e.message.contains("protected"), "{}", e.message);
    assert_eq!(f.saw(|s| (s.blobs, s.commits.len())), (0, 0));
}

#[tokio::test]
async fn what_a_forge_commit_would_upload_is_bounded() {
    let world = World::new();
    let f = Fake::start(&world.bare).await;
    let mut target = committing(&world, &f);
    target.policy.max_change_bytes = 4;
    let w = daemon(&world, target);
    let r = run("r1");
    edit(&w, &r, "longer than four\n").await;
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(e.message.contains("limit"), "{}", e.message);
    assert_eq!(f.saw(|s| (s.blobs, s.commits.len())), (0, 0));
}

#[tokio::test]
async fn a_container_sees_the_read_token_alone_and_never_the_key() {
    let (world, f, w) = started().await;
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    w.propose(&r, &proposal()).await.unwrap();

    let told: Vec<String> = world.runtime.with(|b| {
        let each = b.execs.iter();
        each.flat_map(|e| e.envs.values().chain([&e.command]).cloned())
            .collect()
    });
    let told = told.join("\n");
    let encoded = |token: &str| basic(token).trim_start_matches("Basic ").to_string();
    let writers = f.saw(|s| s.writers.clone());
    assert!(!writers.is_empty());
    for token in &writers {
        assert!(!told.contains(token.as_str()), "{told}");
        assert!(!told.contains(&encoded(token)), "{told}");
    }
    assert!(!told.contains("PRIVATE KEY"), "{told}");
    // The host's clone asks first, so the container's is the second minted.
    assert!(told.contains(&encoded("ghs_fake_1")), "{told}");
}

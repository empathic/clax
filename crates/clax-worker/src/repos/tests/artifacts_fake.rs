//! The Cloudflare Artifacts host over a fake binding: a namespace on
//! clax-git's in-memory host, whose git remote is clax-git's server in
//! process, taking only the tokens the fake minted and still holds valid.
//! The store keeps the content contract; writes use `create`'s token, then
//! a minted one, retried once on a refusal; tokens for people are
//! Artifacts' own, revoked through the must-revoke list.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::content::memory::MemContent;
use clax_core::content::{ContentStore, contract, wait};
use clax_core::working::{Clock, ManualClock};
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::memory::{MemoryHost, MemoryRepo};
use clax_git::object::parse_tree;
use clax_git::server::{Access, GitRequest, ReceiveOptions, ServeOptions, serve, split_git_path};
use clax_git::versions::repo_name;
use clax_git::{CommitMeta, GitError, ObjectId, ObjectKind, Repo, RepoHost, TreeEntry};

use super::{clock, committer, local_host, write};
use crate::repos::api::RepoService;
use crate::repos::artifacts::{
    ApiToken, ArtifactsApi, ArtifactsHost, ArtifactsRepoApi, ArtifactsTokens, CreatedRepo,
    READY_WAITS_MS, WRITE_TOKEN_MARGIN_MS, WRITE_TOKEN_TTL_S, error_for_code,
};
use crate::repos::db;
use crate::repos::tokens::{self, Scope};

const ORIGIN: &str = "http://artifacts.test";

#[derive(Default)]
struct State {
    /// Tokens the remote takes.
    valid: HashSet<String>,
    /// Token ID to plaintext.
    ids: HashMap<String, String>,
    /// `<method> <token>` of every request the remote was sent.
    seen: Vec<String>,
    /// How many more `get`s of a fork answer `FORK_IN_PROGRESS`.
    forking: HashMap<String, usize>,
    /// How many `get`s each new fork answers `FORK_IN_PROGRESS` to.
    fork_delay: usize,
    /// How many more `get`s of a new repository answer
    /// `CREATE_IN_PROGRESS`.
    creating: HashMap<String, usize>,
    /// How many `get`s each new repository answers `CREATE_IN_PROGRESS` to.
    create_delay: usize,
    /// Every pause the host asked for, in milliseconds.
    paused: Vec<u32>,
}

struct Inner {
    host: MemoryHost,
    clock: Arc<ManualClock>,
    /// Whether `create`'s initial token and minted tokens are taken.
    seed_valid: bool,
    minted_valid: bool,
    minted: AtomicUsize,
    /// Revocations to fail before one succeeds.
    revoke_failures: AtomicUsize,
    /// Token IDs whose revocation answers `NOT_FOUND`.
    not_found_ids: Mutex<HashSet<String>>,
    state: Mutex<State>,
}

/// A fake `ARTIFACTS` namespace and its git remote.
#[derive(Clone)]
struct Fake(Arc<Inner>);

impl Fake {
    fn new(clock: Arc<ManualClock>, seed_valid: bool, minted_valid: bool) -> Fake {
        Fake(Arc::new(Inner {
            host: MemoryHost::new(),
            clock,
            seed_valid,
            minted_valid,
            minted: AtomicUsize::new(0),
            revoke_failures: AtomicUsize::new(0),
            not_found_ids: Mutex::default(),
            state: Mutex::default(),
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap()
    }

    fn minted(&self) -> usize {
        self.0.minted.load(Ordering::SeqCst)
    }

    fn seen(&self) -> Vec<String> {
        self.state().seen.clone()
    }

    fn now_s(&self) -> i64 {
        self.0.clock.now().timestamp()
    }

    fn remote(name: &str) -> String {
        format!("{ORIGIN}/git/{name}.git")
    }

    fn store(&self) -> GitContent<ArtifactsHost<Fake, Fake>, MemContent> {
        let clock: Arc<dyn Clock> = self.0.clock.clone();
        GitContent::new(
            ArtifactsHost::new(self.clone(), self.clone(), clock.clone()),
            MemContent::new(),
            committer(),
            clock,
        )
    }
}

struct Handle {
    fake: Fake,
    name: String,
    repo: MemoryRepo,
}

impl ArtifactsApi for Fake {
    type Handle = Handle;

    async fn create(&self, name: &str) -> clax_git::Result<CreatedRepo> {
        self.0.host.create(name).await?;
        let mut st = self.state();
        let delay = st.create_delay;
        st.creating.insert(name.to_string(), delay);
        drop(st);
        let token = format!("art_v1_seed?expires={}", self.now_s() + 3600);
        if self.0.seed_valid {
            self.state().valid.insert(token.clone());
        }
        Ok(CreatedRepo {
            remote: Fake::remote(name),
            token: Some(token),
        })
    }

    async fn get(&self, name: &str) -> clax_git::Result<Option<Handle>> {
        if let Some(n) = self.state().creating.get_mut(name).filter(|n| **n > 0) {
            *n -= 1;
            return Err(error_for_code(
                Some("CREATE_IN_PROGRESS"),
                format!("{name} is being created"),
            ));
        }
        if let Some(n) = self.state().forking.get_mut(name).filter(|n| **n > 0) {
            *n -= 1;
            return Err(error_for_code(
                Some("FORK_IN_PROGRESS"),
                format!("{name} is being forked"),
            ));
        }
        Ok(self.0.host.repo(name).map(|repo| Handle {
            fake: self.clone(),
            name: name.to_string(),
            repo,
        }))
    }

    async fn delete(&self, name: &str) -> clax_git::Result<bool> {
        let had = self.0.host.repo(name).is_some();
        self.0.host.delete(name).await?;
        Ok(had)
    }

    async fn pause(&self, ms: u32) {
        self.state().paused.push(ms);
    }
}

impl ArtifactsRepoApi for Handle {
    async fn remote(&self) -> clax_git::Result<String> {
        Ok(Fake::remote(&self.name))
    }

    async fn create_token(&self, scope: Scope, ttl_s: u32) -> clax_git::Result<ApiToken> {
        assert_eq!(scope, Scope::Write, "the store mints write tokens");
        let n = self.fake.0.minted.fetch_add(1, Ordering::SeqCst) + 1;
        let expires_s = self.fake.now_s() + i64::from(ttl_s);
        let plaintext = format!("art_v1_fresh{n}?expires={expires_s}");
        let mut st = self.fake.state();
        if self.fake.0.minted_valid {
            st.valid.insert(plaintext.clone());
        }
        st.ids.insert(format!("t{n}"), plaintext.clone());
        Ok(ApiToken {
            id: format!("t{n}"),
            plaintext,
            expires_ms: expires_s * 1000,
        })
    }

    async fn revoke_token(&self, id: &str) -> clax_git::Result<bool> {
        let f = &self.fake.0.revoke_failures;
        if f.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(GitError::RepoUnavailable("UPSTREAM_UNAVAILABLE".into()));
        }
        if self.fake.0.not_found_ids.lock().unwrap().contains(id) {
            return Err(GitError::NotFound(format!("No token {id}")));
        }
        let mut st = self.fake.state();
        let Some(t) = st.ids.get(id).cloned() else {
            return Ok(false);
        };
        Ok(st.valid.remove(&t))
    }

    async fn fork(&self, to: &str) -> clax_git::Result<CreatedRepo> {
        self.fake.0.host.fork(&self.name, to).await?;
        let mut st = self.fake.state();
        let delay = st.fork_delay;
        st.forking.insert(to.to_string(), delay);
        drop(st);
        Ok(CreatedRepo {
            remote: Fake::remote(to),
            token: None,
        })
    }

    async fn read_commit(&self, id: &ObjectId) -> clax_git::Result<Option<CommitMeta>> {
        self.repo.read_commit_meta(id).await
    }

    async fn read_tree(&self, id: &ObjectId) -> clax_git::Result<Option<Vec<TreeEntry>>> {
        match self.repo.read_object(id).await? {
            Some(o) if o.kind == ObjectKind::Tree => Ok(Some(parse_tree(&o.data)?)),
            _ => Ok(None),
        }
    }

    async fn read_blob(&self, id: &ObjectId) -> clax_git::Result<Option<Vec<u8>>> {
        match self.repo.read_object(id).await? {
            Some(o) if o.kind == ObjectKind::Blob => Ok(Some(o.data)),
            _ => Ok(None),
        }
    }

    async fn read_file(&self, rev: &str, path: &str) -> clax_git::Result<Option<Vec<u8>>> {
        let commit = match ObjectId::from_hex(rev) {
            Some(id) => Some(id),
            None => self.repo.refs().await?.get(rev).copied(),
        };
        let Some(meta) = (match commit {
            Some(c) => self.repo.read_commit_meta(&c).await?,
            None => None,
        }) else {
            return Ok(None);
        };
        let mut at = meta.tree;
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            let Some(entries) = self.read_tree(&at).await? else {
                return Ok(None);
            };
            let Some(e) = entries.into_iter().find(|e| e.name == *part) else {
                return Ok(None);
            };
            if i + 1 == parts.len() {
                return self.read_blob(&e.id).await;
            }
            at = e.id;
        }
        Ok(None)
    }
}

impl Transport for Fake {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let token = request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .and_then(|(_, v)| v.strip_prefix("Bearer "))
            .unwrap_or("")
            .to_string();
        let ok = {
            let mut st = self.state();
            st.seen.push(format!("{} {token}", request.method));
            st.valid.contains(&token)
        };
        if !ok {
            return Ok(HttpResponse::new(401, "no"));
        }
        let (path, query) = match request.url.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (request.url.as_str(), None),
        };
        let path = path.strip_prefix(ORIGIN).expect("the fake's URL");
        let (name, rest) = split_git_path(path).expect("a git path");
        let Some(repo) = self.0.host.repo(name) else {
            return Ok(HttpResponse::new(404, "no repository"));
        };
        let options = ServeOptions {
            // Artifacts' own tag rules are not known; Clax's tags are its own.
            receive: ReceiveOptions {
                protect_version_tags: false,
                ..ReceiveOptions::default()
            },
            ..ServeOptions::default()
        };
        let req = GitRequest {
            method: request.method,
            rest,
            query,
            content_encoding: None,
            body: &request.body,
        };
        Ok(serve(&repo, Access::Write, &req, &options).await.response)
    }
}

#[test]
fn the_contract_holds_over_the_artifacts_host() {
    let f = Fake::new(clock(), true, true);
    wait(contract::run(&f.store()));
}

#[test]
fn a_write_pushes_with_creates_token_while_the_remote_takes_it() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    wait(c.write_version(&id, 2, &write(&[], &[("index.html", b"two\n")]))).unwrap();
    assert_eq!(f.minted(), 0);
    assert!(
        f.seen()
            .iter()
            .all(|s| s.ends_with("art_v1_seed?expires=1791896400")),
        "{:?}",
        f.seen()
    );
    assert_eq!(
        wait(c.read_file(&id, 1, "index.html")).unwrap().unwrap(),
        b"one\n"
    );
    assert_eq!(
        wait(c.read_file(&id, 2, "index.html")).unwrap().unwrap(),
        b"two\n"
    );
}

#[test]
fn a_refused_token_is_replaced_by_a_minted_one_and_the_request_sent_again() {
    let f = Fake::new(clock(), false, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    assert_eq!(f.minted(), 1);
    let seen = f.seen();
    let fresh = format!(
        "art_v1_fresh1?expires={}",
        1_791_892_800 + i64::from(WRITE_TOKEN_TTL_S)
    );
    assert_eq!(seen[0], "GET art_v1_seed?expires=1791896400");
    assert_eq!(seen[1], format!("GET {fresh}"));
    assert!(seen[2..].iter().all(|s| s.ends_with(&fresh)), "{seen:?}");
    assert!(seen.iter().any(|s| s.starts_with("POST")), "{seen:?}");
}

#[test]
fn a_write_gives_up_after_one_retry() {
    let f = Fake::new(clock(), false, false);
    let c = f.store();
    let id = ArtifactId::generate();
    let e = wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap_err();
    assert!(e.to_string().contains("refused the token"), "{e}");
    assert_eq!(f.minted(), 1);
    assert_eq!(f.seen().len(), 2);
}

#[test]
fn a_cached_token_is_replaced_shortly_before_it_expires() {
    let clock = clock();
    let f = Fake::new(clock.clone(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    assert_eq!(f.minted(), 0);
    clock.advance(3600 - WRITE_TOKEN_MARGIN_MS / 1000);
    wait(c.write_version(&id, 2, &write(&[], &[("index.html", b"two\n")]))).unwrap();
    assert_eq!(f.minted(), 1);
}

#[test]
fn missing_files_versions_and_repositories_read_as_none() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    assert_eq!(wait(c.read_file(&id, 1, "index.html")).unwrap(), None);
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    assert_eq!(wait(c.read_file(&id, 2, "index.html")).unwrap(), None);
    assert_eq!(wait(c.read_file(&id, 1, "gone.html")).unwrap(), None);
    // And through the binding's readFile, by tag.
    let repo = wait(c.host().open(&repo_name(id.as_str())))
        .unwrap()
        .unwrap();
    assert_eq!(
        wait(repo.read_file_at("refs/tags/v1", "index.html"))
            .unwrap()
            .unwrap(),
        b"one\n"
    );
    assert_eq!(
        wait(repo.read_file_at("refs/tags/v1", "gone.html")).unwrap(),
        None
    );
}

#[test]
fn a_fork_through_the_binding_keeps_the_history() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    wait(c.write_version(&id, 2, &write(&[], &[("index.html", b"two\n")]))).unwrap();
    wait(c.write_version(&id, 3, &write(&[], &[("index.html", b"three\n")]))).unwrap();
    let to = ArtifactId::generate();
    wait(c.fork(
        &id,
        2,
        &["index.html"],
        &to,
        &clax_core::content::VersionMeta::default(),
    ))
    .unwrap();
    assert_eq!(
        wait(c.read_file(&to, 1, "index.html")).unwrap().unwrap(),
        b"two\n"
    );
    let repo = f.0.host.repo(&repo_name(to.as_str())).unwrap();
    let main = wait(repo.refs()).unwrap()[clax_git::MAIN];
    let log = wait(clax_git::GitVersions::new(f.0.host.clone()).log(to.as_str(), None, Some(10)))
        .unwrap();
    assert_eq!(
        log.len(),
        3,
        "the source's versions up to v2 and the fork's"
    );
    assert_eq!(log[0].commit, main);
    // Made as a new repository, not an Artifacts fork: nothing of the
    // source's v3 is in it.
    let source = f.0.host.repo(&repo_name(id.as_str())).unwrap();
    let v3 = wait(source.refs()).unwrap()["refs/tags/v3"];
    assert!(!wait(repo.has_object(&v3)).unwrap());
    assert!(f.state().forking.is_empty());
}

#[test]
fn tokens_for_people_are_artifacts_own_and_a_failed_revocation_is_retried() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    let s = RepoService {
        host: local_host(),
        tokens: ArtifactsTokens { api: f.clone() },
        clock: f.0.clock.clone(),
    };
    let repo = repo_name(id.as_str());
    let mint = |s: &RepoService<_, ArtifactsTokens<Fake>>| {
        wait(tokens::mint(
            &s.host,
            &s.tokens,
            &repo,
            Scope::Write,
            900,
            None,
            &Default::default(),
            s.now_ms(),
        ))
        .unwrap()
    };
    let t = mint(&s);
    assert_eq!(t.remote, Fake::remote(&repo));
    assert!(t.token.starts_with("art_v1_fresh"));
    assert!(f.state().valid.contains(&t.token));
    assert!(wait(tokens::revoke(&s.host, &s.tokens, &repo, &t.id, s.now_ms())).unwrap());
    assert!(!f.state().valid.contains(&t.token));

    let t = mint(&s);
    f.0.revoke_failures.store(1, Ordering::SeqCst);
    assert!(wait(tokens::revoke(&s.host, &s.tokens, &repo, &t.id, s.now_ms())).is_err());
    assert!(
        f.state().valid.contains(&t.token),
        "still valid at Artifacts"
    );
    assert_eq!(s.host.with(|c| db::due(c, s.now_ms())).unwrap().len(), 1);
    assert_eq!(
        wait(tokens::retry_due(&s.host, &s.tokens, s.now_ms())).unwrap(),
        0
    );
    assert!(!f.state().valid.contains(&t.token));
    assert!(s.host.with(|c| db::due(c, s.now_ms())).unwrap().is_empty());

    // A token of a deleted repository leaves the list: it went with it.
    let t = mint(&s);
    f.0.revoke_failures.store(1, Ordering::SeqCst);
    assert!(wait(tokens::revoke(&s.host, &s.tokens, &repo, &t.id, s.now_ms())).is_err());
    wait(c.delete_artifact(&id)).unwrap();
    assert_eq!(
        wait(tokens::retry_due(&s.host, &s.tokens, s.now_ms())).unwrap(),
        0
    );
}

#[test]
fn a_fork_waits_while_artifacts_is_still_making_the_copy() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    f.state().fork_delay = 3;
    let (from, to) = (
        repo_name(id.as_str()),
        repo_name(ArtifactId::generate().as_str()),
    );
    wait(c.host().fork(&from, &to)).unwrap();
    assert!(wait(c.host().open(&to)).unwrap().is_some());
    assert_eq!(f.state().paused, READY_WAITS_MS[..3]);
}

#[test]
fn a_first_version_waits_while_artifacts_is_still_creating_the_repository() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    f.state().create_delay = 2;
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    assert_eq!(f.state().paused, READY_WAITS_MS[..2]);
    assert_eq!(
        wait(c.read_file(&id, 1, "index.html")).unwrap().unwrap(),
        b"one\n"
    );
}

#[test]
fn a_repository_still_being_created_after_the_bounded_wait_is_unavailable() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    f.state().create_delay = READY_WAITS_MS.len() + 1;
    let id = ArtifactId::generate();
    let e = wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap_err();
    assert!(e.to_string().contains("still being made"), "{e}");
    assert_eq!(f.state().paused, READY_WAITS_MS);
}

#[test]
fn a_fork_still_in_progress_after_the_bounded_wait_deletes_its_copy_and_a_retry_succeeds() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    f.state().fork_delay = READY_WAITS_MS.len() + 1;
    let (from, to) = (
        repo_name(id.as_str()),
        repo_name(ArtifactId::generate().as_str()),
    );
    let e = wait(c.host().fork(&from, &to)).unwrap_err();
    assert!(e.to_string().contains("still being made"), "{e}");
    assert_eq!(
        f.state().paused,
        READY_WAITS_MS,
        "one try after each pause, no more"
    );
    assert!(
        f.0.host.repo(&to).is_none(),
        "the unfinished copy, holding the source's tags, is deleted"
    );
    f.state().fork_delay = 0;
    f.state().forking.clear();
    wait(c.host().fork(&from, &to)).unwrap();
    assert!(wait(c.host().open(&to)).unwrap().is_some());
}

#[test]
fn revoking_a_token_the_repository_does_not_know_takes_it_off_the_list() {
    let f = Fake::new(clock(), true, true);
    let c = f.store();
    let id = ArtifactId::generate();
    wait(c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))).unwrap();
    let s = RepoService {
        host: local_host(),
        tokens: ArtifactsTokens { api: f.clone() },
        clock: f.0.clock.clone(),
    };
    let repo = repo_name(id.as_str());
    // A repository of the name made after the token's was deleted: its
    // revokeToken answers NOT_FOUND for the old ID.
    s.host
        .with(|c| db::add_due(c, &repo, "t-old", s.now_ms() + 3_600_000))
        .unwrap();
    *f.0.not_found_ids.lock().unwrap() = HashSet::from(["t-old".to_string()]);
    assert_eq!(
        wait(tokens::retry_due(&s.host, &s.tokens, s.now_ms())).unwrap(),
        0
    );
    assert!(s.host.with(|c| db::due(c, s.now_ms())).unwrap().is_empty());
}

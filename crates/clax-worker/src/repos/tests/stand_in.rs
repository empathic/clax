//! The stand-in: its database and its internal API keep the content store
//! contract; its tokens are scoped, expire and are revoked; a failed
//! revocation stays in the must-revoke list until it succeeds or expires.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clax_content_git::GitContent;
use clax_core::content::memory::MemContent;
use clax_core::content::{ContentStore, contract, wait};
use clax_git::server::Access;
use clax_git::{GitError, RepoHost};
use rusqlite::Connection;

use super::{InProc, clock, committer, local_host, stand_in};
use crate::repos::api::{DoHost, DoTokens, RepoService};
use crate::repos::assets::{BucketAssets, MemObjects};
use crate::repos::db;
use crate::repos::local::LocalHost;
use crate::repos::tokens::{self, LocalTokens, Minted, Scope, TokenHost};

#[test]
fn the_contract_holds_over_the_stand_in_database() {
    let c = GitContent::new(local_host(), MemContent::new(), committer(), clock());
    wait(contract::run(&c));
}

#[test]
fn the_contract_holds_through_the_internal_api_with_assets_in_a_bucket() {
    let t = stand_in(clock());
    let c = GitContent::new(
        DoHost {
            transport: t.clone(),
        },
        BucketAssets {
            objects: MemObjects::new(2),
        },
        committer(),
        clock(),
    );
    wait(contract::run(&c));
}

#[test]
fn a_store_through_the_internal_api_is_a_shareable_content_store() {
    let t = stand_in(clock());
    let c: Arc<dyn clax_core::content::ContentStore> = Arc::new(GitContent::new(
        DoHost {
            transport: t.clone(),
        },
        BucketAssets {
            objects: MemObjects::new(1000),
        },
        committer(),
        clock(),
    ));
    let id = clax_core::ArtifactId::generate();
    std::thread::scope(|s| {
        s.spawn(|| assert_eq!(wait(c.read_file(&id, 1, "index.html")).unwrap(), None));
    });
}

#[test]
fn a_repository_reads_only_the_objects_it_holds_and_a_fork_shares_them() {
    let h = local_host();
    wait(async {
        h.create("a").await.unwrap();
        h.create("b").await.unwrap();
        let a = h.open("a").await.unwrap().unwrap();
        let blob = clax_git::Object::blob("secret\n");
        let id = blob.id();
        clax_git::Repo::apply(&a, vec![blob], &[], true)
            .await
            .unwrap();
        let b = h.open("b").await.unwrap().unwrap();
        assert!(
            clax_git::Repo::read_object(&b, &id)
                .await
                .unwrap()
                .is_none()
        );
        h.fork("a", "c").await.unwrap();
        let c = h.open("c").await.unwrap().unwrap();
        assert!(
            clax_git::Repo::read_object(&c, &id)
                .await
                .unwrap()
                .is_some()
        );
        // Deleting `a` keeps what the fork holds.
        h.delete("a").await.unwrap();
        assert!(
            clax_git::Repo::read_object(&c, &id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(h.open("a").await.unwrap().is_none());
    });
}

#[test]
fn a_fork_through_the_internal_api_holds_nothing_the_source_gained_after_the_forked_version() {
    let t = stand_in(clock());
    let c = GitContent::new(
        DoHost {
            transport: t.clone(),
        },
        MemContent::new(),
        committer(),
        clock(),
    );
    let id = clax_core::ArtifactId::generate();
    for (n, page) in [(1, b"one\n"), (2, b"two\n"), (3, b"tri\n"), (4, b"for\n")] {
        wait(c.write_version(&id, n, &super::write(&[], &[("index.html", page)]))).unwrap();
    }
    let to = clax_core::ArtifactId::generate();
    let meta = clax_core::content::VersionMeta::default();
    wait(c.fork(&id, 2, &["index.html"], &to, &meta)).unwrap();
    let h = &t.service.host;
    let (from, fork) = (
        clax_git::versions::repo_name(id.as_str()),
        clax_git::versions::repo_name(to.as_str()),
    );
    let src = h.with(|c| db::refs(c, &from)).unwrap();
    let refs = h.with(|c| db::refs(c, &fork)).unwrap();
    assert_eq!(refs.len(), 2, "main and v1: {refs:?}");
    for v in ["refs/tags/v3", "refs/tags/v4"] {
        let commit = src[v];
        let tree = wait(clax_git::Repo::read_commit_meta(
            &h.repo(&from).unwrap().unwrap(),
            &commit,
        ))
        .unwrap()
        .unwrap()
        .tree;
        for o in [commit, tree] {
            assert!(h.with(|c| db::read_object(c, &fork, &o)).unwrap().is_none());
            assert!(h.with(|c| db::read_object(c, &from, &o)).unwrap().is_some());
        }
    }
    let v2 = src["refs/tags/v2"];
    assert!(
        h.with(|c| db::read_object(c, &fork, &v2))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        wait(c.read_file(&to, 1, "index.html")).unwrap().unwrap(),
        b"two\n"
    );
}

fn tokens_of(t: &InProc<LocalTokens<Connection>>) -> DoTokens<InProc<LocalTokens<Connection>>> {
    DoTokens {
        transport: t.clone(),
    }
}

#[test]
fn a_token_opens_its_own_repository_for_its_scope_until_it_expires() {
    let clock = clock();
    let t = stand_in(clock.clone());
    let host = t.service.host.clone();
    wait(async {
        host.create("a-one").await.unwrap();
        host.create("a-two").await.unwrap();
        let read = tokens_of(&t)
            .mint("a-one", Scope::Read, 3600, Some("http://ignored.test"))
            .await
            .unwrap();
        // PUBLIC_ORIGIN wins over the request's origin.
        assert_eq!(read.remote, "http://clax.test/git/a-one.git");
        assert!(read.token.starts_with("cxm_read_"), "{}", read.token);
        assert!(read.id.starts_with("cxt_"), "{}", read.id);
        assert_eq!(read.expires_at, "2026-10-13T13:00:00.000Z");
        assert_eq!(
            read.clone_command,
            format!(
                "git -c http.extraHeader=\"Authorization: Bearer {}\" clone http://clax.test/git/a-one.git",
                read.token
            )
        );
        assert!(read.jj_clone_command.starts_with("GIT_CONFIG_COUNT=1 "));
        let write = tokens_of(&t)
            .mint("a-one", Scope::Write, 60, None)
            .await
            .unwrap();
        let now = t.service.now_ms();
        assert_eq!(
            tokens::check(&host, "a-one", &read.token, now).unwrap(),
            Some(Access::Read)
        );
        assert_eq!(
            tokens::check(&host, "a-one", &write.token, now).unwrap(),
            Some(Access::Write)
        );
        assert_eq!(
            tokens::check(&host, "a-two", &read.token, now).unwrap(),
            None
        );
        assert_eq!(
            tokens::check(&host, "a-one", "cxm_read_guess", now).unwrap(),
            None
        );
        clock.advance(61);
        let now = t.service.now_ms();
        assert_eq!(
            tokens::check(&host, "a-one", &write.token, now).unwrap(),
            None
        );
        assert_eq!(
            tokens::check(&host, "a-one", &read.token, now).unwrap(),
            Some(Access::Read)
        );
    });
}

#[test]
fn a_token_cannot_be_minted_for_a_missing_repository_or_an_unbounded_life() {
    let t = stand_in(clock());
    wait(async {
        let e = tokens_of(&t)
            .mint("a-none", Scope::Read, 3600, None)
            .await
            .unwrap_err();
        assert!(matches!(e, GitError::NotFound(_)), "{e:?}");
        t.service.host.create("a-one").await.unwrap();
        for ttl in [0, 59, 31_536_001] {
            let e = tokens_of(&t)
                .mint("a-one", Scope::Read, ttl, None)
                .await
                .unwrap_err();
            assert!(matches!(e, GitError::InvalidArgument(_)), "{ttl}: {e:?}");
        }
    });
}

#[test]
fn a_revoked_token_and_the_tokens_of_a_deleted_repository_open_nothing() {
    let t = stand_in(clock());
    let host = t.service.host.clone();
    wait(async {
        host.create("a-one").await.unwrap();
        let a = tokens_of(&t)
            .mint("a-one", Scope::Read, 3600, None)
            .await
            .unwrap();
        let b = tokens_of(&t)
            .mint("a-one", Scope::Write, 3600, None)
            .await
            .unwrap();
        assert!(
            !tokens_of(&t).revoke("a-two", &a.id).await.unwrap(),
            "another repository's"
        );
        assert!(tokens_of(&t).revoke("a-one", &a.id).await.unwrap());
        assert!(
            !tokens_of(&t).revoke("a-one", &a.id).await.unwrap(),
            "already revoked"
        );
        let now = t.service.now_ms();
        assert_eq!(tokens::check(&host, "a-one", &a.token, now).unwrap(), None);
        assert_eq!(
            tokens::check(&host, "a-one", &b.token, now).unwrap(),
            Some(Access::Write)
        );
        // A repository created again under the name does not take them.
        host.delete("a-one").await.unwrap();
        host.create("a-one").await.unwrap();
        assert_eq!(tokens::check(&host, "a-one", &b.token, now).unwrap(), None);
        assert!(host.with(|c| db::due(c, now)).unwrap().is_empty());
    });
}

/// A token host whose revocations fail while `failing` is above zero.
struct Flaky {
    failing: AtomicUsize,
    revoked: AtomicUsize,
}

impl TokenHost for Flaky {
    async fn mint(
        &self,
        _: &str,
        scope: Scope,
        ttl_s: u32,
        now_ms: i64,
    ) -> clax_git::Result<Minted> {
        let mut m = tokens::new_local_token(scope, ttl_s, now_ms)?;
        m.hash = None;
        Ok(m)
    }

    async fn revoke(&self, _: &str, _: &str, _: i64) -> clax_git::Result<bool> {
        if self
            .failing
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(GitError::RepoUnavailable("the host is down".into()));
        }
        self.revoked.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }

    async fn remote(&self, repo: &str, _: Option<&str>) -> clax_git::Result<String> {
        Ok(format!("https://artifacts.test/{repo}.git"))
    }
}

fn flaky(
    failing: usize,
    clock: Arc<clax_core::working::ManualClock>,
) -> RepoService<Connection, Flaky> {
    RepoService {
        host: local_host(),
        tokens: Flaky {
            failing: AtomicUsize::new(failing),
            revoked: AtomicUsize::new(0),
        },
        clock,
    }
}

fn mint(s: &RepoService<Connection, Flaky>, repo: &str, ttl: u32) -> tokens::GitToken {
    wait(tokens::mint(
        &s.host,
        &s.tokens,
        repo,
        Scope::Read,
        ttl,
        None,
        s.now_ms(),
    ))
    .unwrap()
}

fn due(h: &LocalHost<Connection>, now: i64) -> Vec<(String, String)> {
    h.with(|c| db::due(c, now)).unwrap()
}

#[test]
fn a_failed_revocation_stays_listed_until_a_retry_revokes_it() {
    let s = flaky(2, clock());
    let t = mint(&s, "a-one", 3600);
    let e = wait(tokens::revoke(
        &s.host,
        &s.tokens,
        "a-one",
        &t.id,
        s.now_ms(),
    ))
    .unwrap_err();
    assert!(matches!(e, GitError::RepoUnavailable(_)), "{e:?}");
    assert_eq!(
        due(&s.host, s.now_ms()),
        vec![("a-one".to_string(), t.id.clone())]
    );
    // The first retry fails too; the second revokes it.
    assert_eq!(
        wait(tokens::retry_due(&s.host, &s.tokens, s.now_ms())).unwrap(),
        1
    );
    assert_eq!(
        wait(tokens::retry_due(&s.host, &s.tokens, s.now_ms())).unwrap(),
        0
    );
    assert!(due(&s.host, s.now_ms()).is_empty());
    assert_eq!(s.tokens.revoked.load(Ordering::SeqCst), 1);
    let row = s.host.with(|c| db::token(c, &t.id)).unwrap().unwrap();
    assert!(row.revoked_ms.is_some());
    // Revoking it again asks nothing of the host.
    assert!(
        !wait(tokens::revoke(
            &s.host,
            &s.tokens,
            "a-one",
            &t.id,
            s.now_ms()
        ))
        .unwrap()
    );
    assert_eq!(s.tokens.revoked.load(Ordering::SeqCst), 1);
}

#[test]
fn a_listed_token_that_expires_leaves_the_list_without_a_revocation() {
    let clock = clock();
    let s = flaky(usize::MAX, clock.clone());
    let t = mint(&s, "a-one", 60);
    assert!(
        wait(tokens::revoke(
            &s.host,
            &s.tokens,
            "a-one",
            &t.id,
            s.now_ms()
        ))
        .is_err()
    );
    assert_eq!(due(&s.host, s.now_ms()).len(), 1);
    clock.advance(60);
    assert_eq!(
        wait(tokens::retry_due(&s.host, &s.tokens, s.now_ms())).unwrap(),
        0
    );
    assert!(due(&s.host, s.now_ms()).is_empty());
    assert_eq!(s.tokens.revoked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_write_over_the_bound_is_refused_and_writes_nothing() {
    let h = local_host();
    wait(async {
        h.create("a").await.unwrap();
        let a = h.open("a").await.unwrap().unwrap();
        let big = clax_git::Object::blob(vec![7u8; db::MAX_WRITE_BYTES + 1]);
        let id = big.id();
        let update = clax_git::RefUpdate::new("refs/heads/big", None, Some(id));
        let e = clax_git::Repo::apply(&a, vec![big], &[update], true)
            .await
            .unwrap_err();
        assert!(matches!(e, GitError::InvalidArgument(_)), "{e:?}");
        assert!(
            e.to_string().contains(&db::MAX_WRITE_BYTES.to_string()),
            "{e}"
        );
        assert!(clax_git::Repo::refs(&a).await.unwrap().is_empty());
        assert!(
            clax_git::Repo::read_object(&a, &id)
                .await
                .unwrap()
                .is_none()
        );
        let n: i64 = h.with(|c| {
            c.query_row("SELECT count(*) FROM git_objects", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(n, 0);
    });
}

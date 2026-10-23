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
    let h = &t.service.host;
    let from = clax_git::versions::repo_name(id.as_str());
    let src = h.with(|c| db::refs(c, &from)).unwrap();
    // A push the source's ledger names.
    let mia = crate::repos::tokens::Holder {
        person: Some("u_mia".into()),
        session: None,
        link: false,
    };
    let v2 = src["refs/tags/v2"];
    h.with(|c| db::record_push(c, &from, &v2, &mia, 1)).unwrap();
    let to = clax_core::ArtifactId::generate();
    let meta = clax_core::content::VersionMeta::default();
    wait(c.fork(&id, 2, &["index.html"], &to, &meta)).unwrap();
    let fork = clax_git::versions::repo_name(to.as_str());
    // The fork inherits none of the source's pushes.
    assert_eq!(
        h.with(|c| db::pusher(c, &from, &v2.to_hex())).unwrap(),
        Some(mia)
    );
    assert_eq!(
        h.with(|c| db::pusher(c, &fork, &v2.to_hex())).unwrap(),
        None
    );
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
            .mint(
                "a-one",
                Scope::Read,
                3600,
                Some("http://ignored.test"),
                &Default::default(),
            )
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
            .mint("a-one", Scope::Write, 60, None, &Default::default())
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
            .mint("a-none", Scope::Read, 3600, None, &Default::default())
            .await
            .unwrap_err();
        assert!(matches!(e, GitError::NotFound(_)), "{e:?}");
        t.service.host.create("a-one").await.unwrap();
        for ttl in [0, 59, 31_536_001] {
            let e = tokens_of(&t)
                .mint("a-one", Scope::Read, ttl, None, &Default::default())
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
            .mint("a-one", Scope::Read, 3600, None, &Default::default())
            .await
            .unwrap();
        let b = tokens_of(&t)
            .mint("a-one", Scope::Write, 3600, None, &Default::default())
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
        &Default::default(),
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
fn many_tiny_objects_count_their_rows_against_the_bound() {
    let h = local_host();
    wait(async {
        h.create("a").await.unwrap();
        let a = h.open("a").await.unwrap().unwrap();
        // Four bytes each: far under the bound in data, over it with rows.
        let n = db::MAX_WRITE_BYTES / (db::OBJECT_ROW_BYTES + 4) + 1;
        let objects: Vec<_> = (0..n as u32)
            .map(|i| clax_git::Object::blob(i.to_be_bytes().to_vec()))
            .collect();
        assert!(4 * n < db::MAX_WRITE_BYTES);
        let e = clax_git::Repo::apply(&a, objects, &[], true)
            .await
            .unwrap_err();
        assert!(matches!(e, GitError::InvalidArgument(_)), "{e:?}");
    });
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

#[test]
fn a_token_keeps_whom_it_was_handed_to_and_an_older_ledger_gains_the_columns() {
    let t = stand_in(clock());
    let host = t.service.host.clone();
    let holder = tokens::Holder {
        person: Some("p_rae".into()),
        session: Some("s1".into()),
        link: false,
    };
    wait(async {
        host.create("a-one").await.unwrap();
        let minted = tokens_of(&t)
            .mint("a-one", Scope::Write, 3600, None, &holder)
            .await
            .unwrap();
        let now = t.service.now_ms();
        assert_eq!(
            tokens::holder(&host, "a-one", &minted.token, now).unwrap(),
            Some(holder.clone())
        );
        assert_eq!(
            tokens::holder(&host, "a-two", &minted.token, now).unwrap(),
            None,
            "another repository's token is no one's there"
        );
        let owners = tokens_of(&t)
            .mint("a-one", Scope::Write, 3600, None, &Default::default())
            .await
            .unwrap();
        assert_eq!(
            tokens::holder(&host, "a-one", &owners.token, now).unwrap(),
            Some(tokens::Holder::default())
        );
    });

    let old = Connection::open_in_memory().unwrap();
    old.execute_batch(
        "CREATE TABLE git_tokens (
            id TEXT PRIMARY KEY, repo TEXT NOT NULL, scope TEXT NOT NULL,
            expires_ms INTEGER NOT NULL, revoked_ms INTEGER, hash BLOB UNIQUE);
         INSERT INTO git_tokens VALUES ('cxt_old', 'a-one', 'read', 9, NULL, NULL);",
    )
    .unwrap();
    db::init(&old).unwrap();
    db::init(&old).unwrap();
    let row = db::token(&old, "cxt_old").unwrap().unwrap();
    assert_eq!(row.holder, tokens::Holder::default());
}

#[test]
fn the_held_tokens_are_listed_without_secrets_and_revoked_by_session_or_person() {
    let clock = clock();
    let t = stand_in(clock.clone());
    let host = t.service.host.clone();
    let holder = |person: Option<&str>, session: Option<&str>| tokens::Holder {
        person: person.map(String::from),
        session: session.map(String::from),
        link: false,
    };
    wait(async {
        host.create("a-one").await.unwrap();
        host.create("a-two").await.unwrap();
        let api = tokens_of(&t);
        let rae_agent = api
            .mint(
                "a-one",
                Scope::Read,
                900,
                None,
                &holder(Some("p_rae"), Some("s1")),
            )
            .await
            .unwrap();
        let rae_agent_two = api
            .mint(
                "a-two",
                Scope::Read,
                900,
                None,
                &holder(Some("p_rae"), Some("s1")),
            )
            .await
            .unwrap();
        let rae = api
            .mint(
                "a-one",
                Scope::Write,
                3600,
                None,
                &holder(Some("p_rae"), None),
            )
            .await
            .unwrap();
        let owner_agent = api
            .mint("a-one", Scope::Read, 900, None, &holder(None, Some("s2")))
            .await
            .unwrap();
        let owners = api
            .mint("a-one", Scope::Write, 3600, None, &Default::default())
            .await
            .unwrap();
        let link = api
            .mint(
                "a-one",
                Scope::Read,
                900,
                None,
                &tokens::Holder {
                    link: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let held = api.held().await.unwrap();
        let ids: Vec<&str> = held.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                &rae_agent.id,
                &rae_agent_two.id,
                &rae.id,
                &owner_agent.id,
                &owners.id,
                &link.id
            ]
            .map(|s| s.as_str()),
            "the owner's own token is listed, and a page link's"
        );
        assert!(!held[4].link && held[4].person.is_none() && held[4].session.is_none());
        assert!(held[5].link && held[5].person.is_none() && held[5].session.is_none());
        assert!(!held[2].link);
        assert_eq!(held[2].scope, "write");
        assert_eq!(held[2].person.as_deref(), Some("p_rae"));
        assert_eq!(held[2].session, None);
        let listed = serde_json::to_string(&held).unwrap();
        assert!(!listed.contains(&rae.token), "{listed}");

        assert_eq!(api.revoke_held(None, None).await.unwrap(), 0);
        assert_eq!(api.revoke_held(None, Some("s1")).await.unwrap(), 2);
        let now = t.service.now_ms();
        assert_eq!(
            tokens::check(&host, "a-one", &rae_agent.token, now).unwrap(),
            None
        );
        assert_eq!(
            tokens::check(&host, "a-two", &rae_agent_two.token, now).unwrap(),
            None
        );
        assert_eq!(
            tokens::check(&host, "a-one", &rae.token, now).unwrap(),
            Some(Access::Write),
            "the person's own token outlives their agent's session"
        );
        assert_eq!(api.revoke_held(Some("p_rae"), None).await.unwrap(), 1);
        let now = t.service.now_ms();
        assert_eq!(
            tokens::check(&host, "a-one", &rae.token, now).unwrap(),
            None
        );
        assert_eq!(
            tokens::check(&host, "a-one", &owner_agent.token, now).unwrap(),
            Some(Access::Read)
        );
        assert_eq!(
            tokens::check(&host, "a-one", &owners.token, now).unwrap(),
            Some(Access::Write)
        );
        let left: Vec<String> = api
            .held()
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(
            left,
            [owner_agent.id.clone(), owners.id.clone(), link.id.clone()]
        );
        assert_eq!(
            tokens::check(&host, "a-one", &link.token, now).unwrap(),
            Some(Access::Read)
        );
        // An expired token is no longer held.
        clock.advance(901);
        let left: Vec<String> = api
            .held()
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(left, std::slice::from_ref(&owners.id));
    });
}

#[test]
fn a_push_to_main_is_entered_as_its_token_holders_and_the_ledger_keeps_the_newest() {
    let h = local_host();
    let id = |b: u8| clax_git::ObjectId::from_bytes(&[b; 20]).unwrap();
    let main = |b: u8| clax_git::RefUpdate::new(clax_git::MAIN, None, Some(id(b)));
    let path = "/git/a-7q3k9mzx2b4t.git/git-receive-pack";
    let note = |path: &str, pushed: &[clax_git::RefUpdate]| {
        tokens::note_push(&h, path, None, pushed, 1).unwrap()
    };
    assert_eq!(
        note(path, &[main(1)]),
        Some(("7q3k9mzx2b4t".to_string(), tokens::Holder::default()))
    );
    let feature = clax_git::RefUpdate::new("refs/heads/feature", None, Some(id(2)));
    assert_eq!(note(path, &[feature]), None, "main did not move");
    assert_eq!(
        note("/git/notes.git/git-receive-pack", &[main(3)]),
        None,
        "no artifact's repository"
    );
    let repo = "a-7q3k9mzx2b4t";
    let hex = id(1).to_hex();
    assert_eq!(
        h.with(|c| db::pusher(c, repo, &hex)).unwrap(),
        Some(tokens::Holder::default())
    );
    assert_eq!(
        h.with(|c| db::pusher(c, repo, &id(2).to_hex())).unwrap(),
        None
    );
    let rae = tokens::Holder {
        person: Some("u_rae".into()),
        session: None,
        link: false,
    };
    for _ in 0..db::MAX_PUSHES_KEPT {
        h.with(|c| db::record_push(c, repo, &id(9), &rae, 2))
            .unwrap();
    }
    assert_eq!(
        h.with(|c| db::pusher(c, repo, &hex)).unwrap(),
        None,
        "pushed out by newer pushes"
    );
    assert_eq!(
        h.with(|c| db::pusher(c, repo, &id(9).to_hex())).unwrap(),
        Some(rae)
    );
}

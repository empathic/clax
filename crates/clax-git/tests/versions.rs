//! `GitVersions` over the in-memory host: publish carries files forward,
//! reads by version, conflicts, forks with history, tags; and the pushed
//! event parsing.

use std::collections::BTreeMap;

use clax_git::events::{BatchPlan, PUSHED_EVENT, event_repo, plan_batch, pushed_artifact};
use clax_git::memory::MemoryHost;
use clax_git::tree::Changes;
use clax_git::versions::{fork_ref, person_email, repo_name, tag_ref, version_of_tag_ref};
use clax_git::{
    GitError, GitVersions, MAIN, Mode, ObjectId, Person, Publish, Repo, Rev, VersionMeta,
};
use serde_json::json;

use crate::common::{Home, temp};

const A: &str = "0123456789ab";
const B: &str = "bbbbbbbbbbbb";

fn meta(time: i64, agent: Option<&str>) -> VersionMeta {
    VersionMeta {
        author: Person {
            name: "Mia".into(),
            email: person_email("p1"),
        },
        time,
        note: String::new(),
        publisher: "p1".into(),
        agent: agent.map(String::from),
    }
}

fn changes(files: &[(&str, Option<&str>)]) -> Changes {
    files
        .iter()
        .map(|(p, c)| (p.to_string(), c.map(|c| c.as_bytes().to_vec())))
        .collect()
}

async fn publish(
    v: &GitVersions<MemoryHost>,
    n: u64,
    parent: Option<ObjectId>,
    files: &[(&str, Option<&str>)],
) -> ObjectId {
    let p = Publish {
        version: n,
        parent,
        changes: changes(files),
        meta: meta(1_000 + n as i64, None),
    };
    let r = v.publish(A, &p).await.unwrap();
    assert_eq!(r.tag_error, None);
    r.commit
}

#[tokio::test]
async fn publish_carries_files_forward_and_reads_by_version() {
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(A).await.unwrap();
    assert_eq!(v.head(A).await.unwrap(), None);
    let c1 = publish(
        &v,
        1,
        None,
        &[
            ("index.html", Some("one")),
            ("css/a.css", Some("a")),
            ("css/b.css", Some("b")),
        ],
    )
    .await;
    let c2 = publish(
        &v,
        2,
        Some(c1),
        &[
            ("index.html", Some("two")),
            ("css/b.css", None),
            ("img/x/y.txt", Some("y")),
        ],
    )
    .await;
    // A publish that changes nothing makes a version all the same.
    let c3 = publish(&v, 3, Some(c2), &[]).await;
    assert_eq!(v.head(A).await.unwrap(), Some(c3));
    assert_eq!(
        v.versions(A).await.unwrap(),
        BTreeMap::from([(1, c1), (2, c2), (3, c3)])
    );

    let read = |n, p: &'static str| {
        let v = v.clone();
        async move {
            v.read_file(A, &Rev::Version(n), p)
                .await
                .unwrap()
                .map(|b| String::from_utf8(b).unwrap())
        }
    };
    assert_eq!(read(1, "index.html").await.as_deref(), Some("one"));
    assert_eq!(read(2, "index.html").await.as_deref(), Some("two"));
    assert_eq!(read(2, "css/a.css").await.as_deref(), Some("a"));
    assert_eq!(read(2, "css/b.css").await, None);
    assert_eq!(read(1, "css/b.css").await.as_deref(), Some("b"));
    assert_eq!(read(2, "css").await, None, "a directory is no file");
    assert_eq!(read(2, "../index.html").await, None);
    assert_eq!(read(9, "index.html").await, None);
    assert_eq!(
        v.read_file(A, &Rev::Commit(c3), "img/x/y.txt")
            .await
            .unwrap(),
        Some(b"y".to_vec())
    );
    assert_eq!(
        v.read_file("zzzzzzzzzzzz", &Rev::Version(1), "index.html")
            .await
            .unwrap(),
        None
    );

    let files = v.list_files(A, &Rev::Version(2)).await.unwrap();
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        ["css/a.css", "img/x/y.txt", "index.html"]
    );
    assert_eq!(files["index.html"].size, 3);
    assert_eq!(files["index.html"].mode, Mode::FILE);
    assert_eq!(
        files["index.html"].id,
        clax_git::object::hash_object(clax_git::ObjectKind::Blob, b"two")
    );
    assert_eq!(
        v.list_files(A, &Rev::Version(9)).await.unwrap_err().code(),
        "not_found"
    );

    let log = v.log(A, None, None).await.unwrap();
    assert_eq!(
        log.iter().map(|c| c.commit).collect::<Vec<_>>(),
        [c3, c2, c1]
    );
    assert_eq!(log[2].trailer("Clax-Version"), Some("1"));
    assert_eq!(log[2].trailer("Clax-Publisher"), Some("p1"));
    assert_eq!(log[2].message, "Clax-Version: 1\nClax-Publisher: p1");
    assert_eq!(log[2].author.email, "p1@users.clax.dev");
    assert_eq!(log[2].committer.time, 1_001);
    assert_eq!(v.log(A, Some(c1), None).await.unwrap().len(), 2);
    assert_eq!(v.log(A, None, Some(1)).await.unwrap().len(), 1);
    // `since` off the chain: the whole chain.
    assert_eq!(
        v.log(A, Some(ObjectId([7; 20])), None).await.unwrap().len(),
        3
    );
}

#[tokio::test]
async fn publish_refuses_a_stale_parent_a_taken_tag_and_bad_paths_moving_nothing() {
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(A).await.unwrap();
    let c1 = publish(&v, 1, None, &[("a", Some("1"))]).await;
    let c2 = publish(&v, 2, Some(c1), &[("a", Some("2"))]).await;
    let p = |n, parent, files: &[(&str, Option<&str>)]| Publish {
        version: n,
        parent,
        changes: changes(files),
        meta: meta(5, None),
    };

    let e = v
        .publish(A, &p(3, Some(c1), &[("a", Some("x"))]))
        .await
        .unwrap_err();
    assert!(
        matches!(e, GitError::VersionConflict { head: Some(h), .. } if h == c2),
        "{e:?}"
    );
    let e = v
        .publish(A, &p(1, None, &[("a", Some("x"))]))
        .await
        .unwrap_err();
    assert!(
        matches!(e, GitError::VersionConflict { head: Some(h), .. } if h == c2),
        "{e:?}"
    );
    let e = v
        .publish(A, &p(2, Some(c2), &[("a", Some("x"))]))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e}");
    for bad in [".git/config", "a/../b", "", "x//y", "a\\b", "./a"] {
        let e = v
            .publish(A, &p(3, Some(c2), &[(bad, Some("x"))]))
            .await
            .unwrap_err();
        assert_eq!(e.code(), "invalid_argument", "{bad:?}");
    }
    let e = v
        .publish(A, &p(3, Some(c2), &[("d", Some("x")), ("d/e", Some("y"))]))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let e = v
        .publish(A, &p(3, Some(ObjectId([9; 20])), &[]))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "version_conflict");
    assert_eq!(v.head(A).await.unwrap(), Some(c2));
    assert_eq!(v.versions(A).await.unwrap().len(), 2);
    assert_eq!(
        v.publish("zzzzzzzzzzzz", &p(1, None, &[]))
            .await
            .unwrap_err()
            .code(),
        "not_found"
    );
}

#[tokio::test]
async fn a_file_replaces_a_directory_and_an_emptied_directory_disappears() {
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(A).await.unwrap();
    let c1 = publish(
        &v,
        1,
        None,
        &[("d/x", Some("x")), ("e/y", Some("y")), ("f", Some("f"))],
    )
    .await;
    let c2 = publish(
        &v,
        2,
        Some(c1),
        &[("d", Some("now a file")), ("e/y", None), ("f/g", Some("g"))],
    )
    .await;
    let files = v.list_files(A, &Rev::Commit(c2)).await.unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), ["d", "f/g"]);
}

#[tokio::test]
async fn fork_copies_history_drops_the_version_tags_and_commits_v1_on_top() {
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(A).await.unwrap();
    let c1 = publish(
        &v,
        1,
        None,
        &[("index.html", Some("one")), ("old.txt", Some("old"))],
    )
    .await;
    let c2 = publish(
        &v,
        2,
        Some(c1),
        &[("index.html", Some("two")), ("old.txt", None)],
    )
    .await;
    host.repo(&repo_name(A))
        .unwrap()
        .apply(
            vec![],
            &[clax_git::RefUpdate::new("refs/tags/keep", None, Some(c2))],
            false,
        )
        .await
        .unwrap();

    let objects_before = host.object_count();
    let forked = v
        .fork(A, B, &Rev::Version(1), 1, &meta(2_000, Some("claude")))
        .await
        .unwrap();
    assert_eq!(forked.tag_error, None);
    // A fork shares objects: only the new commit is stored.
    assert_eq!(host.object_count(), objects_before + 1);
    assert_eq!(
        v.versions(B).await.unwrap(),
        BTreeMap::from([(1, forked.commit)])
    );
    let refs = host.repo(&repo_name(B)).unwrap().refs().await.unwrap();
    assert_eq!(
        refs.get("refs/tags/keep"),
        Some(&c2),
        "tags other than v<n> stay"
    );
    let log = v.log(B, None, None).await.unwrap();
    assert_eq!(
        log.iter().map(|c| c.commit).collect::<Vec<_>>(),
        [forked.commit, c2, c1]
    );
    assert_eq!(
        log[0].message,
        format!(
            "Forked from {}\n\nClax-Version: 1\nClax-Publisher: p1\nClax-Agent: claude\nClax-Forked-From: {}",
            fork_ref(A, 1),
            fork_ref(A, 1)
        )
    );
    // v1 of the fork holds v1 of the original.
    assert_eq!(
        v.read_file(B, &Rev::Version(1), "old.txt").await.unwrap(),
        Some(b"old".to_vec())
    );
    assert_eq!(
        v.read_file(B, &Rev::Version(1), "index.html")
            .await
            .unwrap(),
        Some(b"one".to_vec())
    );
    // The original is untouched; the fork publishes on.
    assert_eq!(v.versions(A).await.unwrap().len(), 2);
    let p = Publish {
        version: 2,
        parent: Some(forked.commit),
        changes: changes(&[("index.html", Some("fork two"))]),
        meta: meta(3_000, None),
    };
    v.publish(B, &p).await.unwrap();
    assert_eq!(
        v.read_file(A, &Rev::Version(2), "index.html")
            .await
            .unwrap(),
        Some(b"two".to_vec())
    );
    assert_eq!(
        v.read_file(B, &Rev::Version(2), "index.html")
            .await
            .unwrap(),
        Some(b"fork two".to_vec())
    );

    assert_eq!(
        v.fork(A, B, &Rev::Version(1), 1, &meta(0, None))
            .await
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    assert_eq!(
        v.fork(A, "cccccccccccc", &Rev::Version(7), 7, &meta(0, None))
            .await
            .unwrap_err()
            .code(),
        "not_found"
    );
    v.delete(B).await.unwrap();
    assert!(host.repo(&repo_name(B)).is_none());
}

#[tokio::test]
async fn a_fork_clones_with_real_git_and_its_history() {
    use crate::common::{bearer_args, clax_server};
    use clax_git::server::{Access, ServeOptions};
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(A).await.unwrap();
    let c1 = publish(&v, 1, None, &[("index.html", Some("one"))]).await;
    publish(&v, 2, Some(c1), &[("index.html", Some("two"))]).await;
    v.fork(A, B, &Rev::Version(1), 1, &meta(2_000, None))
        .await
        .unwrap();
    let (server, _) = clax_server(&host, &[("t", Access::Read)], ServeOptions::default()).await;
    let home = Home::new();
    let work = temp("fork");
    let [c, h] = bearer_args("t");
    let url = format!("{}/git/{}.git", server.base, repo_name(B));
    home.ok_async(
        "git",
        work.path(),
        &[&c, &h, "clone", "-q", &url, "out"],
        &[],
    )
    .await;
    let out = work.path().join("out");
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "one"
    );
    assert_eq!(home.git_text(&out, &["tag", "--list"]), "v1");
    assert_eq!(home.git_text(&out, &["rev-list", "--count", "HEAD"]), "3");
    home.git(&out, &["fsck", "--strict", "--no-dangling"]);
}

#[tokio::test]
async fn set_tags_points_moves_and_deletes_tags() {
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(A).await.unwrap();
    let c1 = publish(&v, 1, None, &[("a", Some("1"))]).await;
    let c2 = publish(&v, 2, Some(c1), &[("a", Some("2"))]).await;
    let tags = |pairs: &[(&str, Option<ObjectId>)]| -> BTreeMap<String, Option<ObjectId>> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    };
    v.set_tags(A, &tags(&[("v1", Some(c2)), ("v2", None), ("v3", None)]))
        .await
        .unwrap();
    assert_eq!(v.versions(A).await.unwrap(), BTreeMap::from([(1, c2)]));
    assert_eq!(
        v.set_tags(A, &tags(&[("v1", Some(ObjectId([1; 20])))]))
            .await
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    assert_eq!(
        v.set_tags(A, &tags(&[("bad..name", None)]))
            .await
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    let refs = host.repo(&repo_name(A)).unwrap().refs().await.unwrap();
    assert_eq!(refs[MAIN], c2);
}

#[test]
fn names_and_tags() {
    assert_eq!(repo_name(A), "a-0123456789ab");
    assert_eq!(tag_ref("v12").unwrap(), "refs/tags/v12");
    for bad in ["", ".v", "v1.lock", "a..b", "v 1", "v/1", "-v"] {
        assert!(tag_ref(bad).is_err(), "{bad:?}");
    }
    assert_eq!(version_of_tag_ref("refs/tags/v3"), Some(3));
    for not in [
        "refs/tags/v0",
        "refs/tags/v01",
        "refs/tags/v",
        "refs/tags/v1a",
        "refs/heads/v1",
    ] {
        assert_eq!(version_of_tag_ref(not), None, "{not}");
    }
}

#[test]
fn pushed_events_name_the_artifact_by_any_of_the_documented_fields() {
    let id = A;
    let cases = [
        json!({"type": PUSHED_EVENT, "source": {"type": "artifacts.repo", "namespace": "n", "repoName": format!("a-{id}")}, "payload": {"ref": "refs/heads/main"}}),
        json!({"type": PUSHED_EVENT, "payload": {"repo_name": format!("a-{id}")}}),
        json!({"type": PUSHED_EVENT, "payload": {"repo": {"name": format!("a-{id}")}}}),
        json!({"type": PUSHED_EVENT, "repo": format!("a-{id}")}),
        json!({"type": PUSHED_EVENT, "name": format!("a-{id}")}),
    ];
    for c in &cases {
        assert_eq!(pushed_artifact(c), Some(id), "{c}");
    }
    let not = [
        json!({"type": "cf.artifacts.repo.created", "repo": format!("a-{id}")}),
        json!({"type": PUSHED_EVENT, "repo": "someone-else"}),
        json!({"type": PUSHED_EVENT, "repo": "a-ILLEGAL00000"}),
        json!({"type": PUSHED_EVENT}),
        json!("a string"),
    ];
    for c in &not {
        assert_eq!(pushed_artifact(c), None, "{c}");
    }
    assert_eq!(
        event_repo(&json!({"payload": {"repoName": ""}, "source": {"repoName": "x"}})),
        Some("x")
    );
}

#[test]
fn a_batch_reconciles_each_artifact_once_and_acks_the_rest() {
    let push = |id: &str| json!({"type": PUSHED_EVENT, "source": {"repoName": format!("a-{id}")}});
    let bodies = [push(A), json!({"type": "other"}), push(B), push(A)];
    let plan = plan_batch(&bodies);
    assert_eq!(
        plan,
        BatchPlan {
            ack: vec![1],
            reconcile: BTreeMap::from([(A.to_string(), vec![0, 3]), (B.to_string(), vec![2])])
        }
    );
}

/// Over a host whose futures are `Send`, so are the store's and the
/// server's, so a native daemon can hold them across a spawned task.
#[test]
fn futures_over_the_memory_host_are_send() {
    fn send<T: Send>(_: T) {}
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    let p = Publish {
        version: 1,
        parent: None,
        changes: Changes::new(),
        meta: meta(0, None),
    };
    send(v.publish(A, &p));
    send(v.fork(A, B, &Rev::Version(1), 1, &p.meta));
    send(v.read_file(A, &Rev::Version(1), "a"));
    send(v.list_files(A, &Rev::Version(1)));
    if let Some(repo) = host.repo("x") {
        let options = clax_git::server::ServeOptions::default();
        let request = clax_git::server::GitRequest {
            method: "GET",
            rest: "info/refs",
            query: None,
            content_encoding: None,
            body: &[],
        };
        send(clax_git::server::serve(
            &repo,
            clax_git::server::Access::Read,
            &request,
            &options,
        ));
    }
}

/// Over any host, natively: the traits' futures are `Send`, so generic
/// code (a content store over a host chosen later) is `Send` too.
#[test]
fn futures_over_any_host_are_send() {
    fn send<T: Send>(_: T) {}
    fn generic<H: clax_git::RepoHost>(v: &GitVersions<H>, p: &Publish) {
        send(v.publish(A, p));
        send(v.read_file(A, &Rev::Version(1), "a"));
        send(v.log(A, None, None));
    }
    let p = Publish {
        version: 1,
        parent: None,
        changes: Changes::new(),
        meta: meta(0, None),
    };
    generic(&GitVersions::new(MemoryHost::new()), &p);
}

/// A host whose repositories answer commits only as metadata, as the
/// Cloudflare Artifacts binding does: `read_object` has no commits.
#[derive(Clone)]
struct MetaOnlyHost(MemoryHost);

struct MetaOnly(clax_git::memory::MemoryRepo);

impl Repo for MetaOnly {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<clax_git::Object>> {
        Ok(self
            .0
            .read_object(id)
            .await?
            .filter(|o| o.kind != clax_git::ObjectKind::Commit))
    }
    async fn read_commit_meta(
        &self,
        id: &ObjectId,
    ) -> clax_git::Result<Option<clax_git::versions::CommitMeta>> {
        self.0.read_commit_meta(id).await
    }
    async fn refs(&self) -> clax_git::Result<BTreeMap<String, ObjectId>> {
        self.0.refs().await
    }
    async fn apply(
        &self,
        objects: Vec<clax_git::Object>,
        updates: &[clax_git::RefUpdate],
        atomic: bool,
    ) -> clax_git::Result<Vec<clax_git::RefStatus>> {
        self.0.apply(objects, updates, atomic).await
    }
}

impl clax_git::RepoHost for MetaOnlyHost {
    type Repo = MetaOnly;
    async fn open(&self, name: &str) -> clax_git::Result<Option<MetaOnly>> {
        Ok(self.0.repo(name).map(MetaOnly))
    }
    async fn create(&self, name: &str) -> clax_git::Result<()> {
        self.0.create(name).await
    }
    async fn fork(&self, from: &str, to: &str) -> clax_git::Result<()> {
        self.0.fork(from, to).await
    }
    async fn delete(&self, name: &str) -> clax_git::Result<()> {
        self.0.delete(name).await
    }
}

#[tokio::test]
async fn versions_read_commits_through_the_metadata_path() {
    let v = GitVersions::new(MetaOnlyHost(MemoryHost::new()));
    v.create(A).await.unwrap();
    let mut p = Publish {
        version: 1,
        parent: None,
        changes: changes(&[("index.html", Some("one")), ("a.js", Some("a"))]),
        meta: meta(1_000, None),
    };
    let c1 = v.publish(A, &p).await.unwrap().commit;
    p.version = 2;
    p.parent = Some(c1);
    p.changes = changes(&[("index.html", Some("two"))]);
    let c2 = v.publish(A, &p).await.unwrap().commit;
    let (r1, r2) = (Rev::Version(1), Rev::Version(2));
    let read = |r, path| v.read_file(A, r, path);
    assert_eq!(read(&r2, "a.js").await.unwrap().unwrap(), b"a");
    assert_eq!(read(&r2, "index.html").await.unwrap().unwrap(), b"two");
    assert_eq!(read(&r1, "index.html").await.unwrap().unwrap(), b"one");
    assert_eq!(v.list_files(A, &Rev::Version(2)).await.unwrap().len(), 2);
    let log = v.log(A, None, None).await.unwrap();
    assert_eq!(log.iter().map(|c| c.commit).collect::<Vec<_>>(), [c2, c1]);
    assert_eq!(log[0].trailer("Clax-Version"), Some("2"));
    let f = v.fork(A, B, &Rev::Version(1), 1, &meta(2_000, None)).await;
    assert_eq!(
        v.read_file(B, &Rev::Version(1), "index.html")
            .await
            .unwrap()
            .unwrap(),
        b"one",
        "{f:?}"
    );
}

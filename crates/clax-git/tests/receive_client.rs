//! The receive-pack client against a real `git http-backend` that accepts
//! only `Authorization: Bearer <token>`: versions published through
//! `GitVersions` over a `PushingRepo` (objects read with `git cat-file`,
//! writes pushed) pass `git fsck --strict` and clone back with the same
//! files, and every refusal maps to the error a publish reports.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clax_git::client::{ObjectReader, PushingRepo, Remote, advertised_refs, apply, set_refs};
use clax_git::http::HttpResponse;
use clax_git::pktline;
use clax_git::tree::{Changes, build_tree};
use clax_git::versions::{Publish, VersionMeta, commit_refs};
use clax_git::{
    Commit, GitError, GitVersions, MAIN, Object, ObjectId, ObjectKind, Person, RefUpdate, Repo,
    RepoHost, Rev, Signature,
};

use crate::common::{Home, Server, TestTransport, bearer_args, files_of, http_backend, temp};

const TOKEN: &str = "test-token";
const ARTIFACT: &str = "0123456789ab";
const REPO: &str = "a-0123456789ab";

/// Reads objects of a repository on disk with `git cat-file`.
#[derive(Clone)]
struct DiskReader {
    env: Vec<(String, String)>,
    dir: PathBuf,
}

impl ObjectReader for DiskReader {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        let run = |args: &[&str]| {
            let mut c = std::process::Command::new("git");
            c.current_dir(&self.dir).args(args).env_clear();
            for (k, v) in &self.env {
                c.env(k, v);
            }
            c.output().expect("git cat-file")
        };
        let t = run(&["cat-file", "-t", &id.to_hex()]);
        if !t.status.success() {
            return Ok(None);
        }
        let kind = match String::from_utf8_lossy(&t.stdout).trim() {
            "commit" => ObjectKind::Commit,
            "tree" => ObjectKind::Tree,
            "blob" => ObjectKind::Blob,
            _ => ObjectKind::Tag,
        };
        let data = run(&["cat-file", kind.name(), &id.to_hex()]);
        Ok(Some(Object::new(kind, data.stdout)))
    }
}

/// The bare repository behind git http-backend, as a host of one.
struct BackendHost {
    reader: DiskReader,
    remote: Remote,
    transport: TestTransport,
}

impl RepoHost for BackendHost {
    type Repo = PushingRepo<DiskReader, TestTransport>;

    async fn open(&self, name: &str) -> clax_git::Result<Option<Self::Repo>> {
        assert_eq!(name, REPO);
        Ok(Some(PushingRepo {
            reader: self.reader.clone(),
            remote: self.remote.clone(),
            transport: self.transport.clone(),
        }))
    }
    async fn create(&self, _: &str) -> clax_git::Result<()> {
        Ok(())
    }
    async fn fork(&self, _: &str, _: &str) -> clax_git::Result<()> {
        Err(GitError::InvalidArgument("not supported".into()))
    }
    async fn delete(&self, _: &str) -> clax_git::Result<()> {
        Ok(())
    }
    fn can_prune(&self) -> bool {
        false
    }

    async fn prune(&self, _: &str) -> clax_git::Result<()> {
        Ok(())
    }
}

struct Fixture {
    home: Arc<Home>,
    bare: PathBuf,
    server: Server,
    _root: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Fixture {
        let home = Arc::new(Home::new());
        let root = temp("http");
        let bare = root.path().join(format!("{REPO}.git"));
        home.git(
            root.path(),
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        home.git(&bare, &["config", "http.receivepack", "true"]);
        home.git(&bare, &["config", "receive.denyNonFastForwards", "true"]);
        let server = http_backend(&home, root.path(), TOKEN).await;
        Fixture {
            home,
            bare,
            server,
            _root: root,
        }
    }

    fn remote_url(&self) -> String {
        format!("{}/{REPO}.git", self.server.base)
    }

    fn remote(&self, token: &str) -> Remote {
        Remote::new(self.remote_url(), token)
    }

    fn reader(&self) -> DiskReader {
        DiskReader {
            env: self.home.env(),
            dir: self.bare.clone(),
        }
    }

    fn versions_with(&self, token: &str, transport: TestTransport) -> GitVersions<BackendHost> {
        GitVersions::new(BackendHost {
            reader: self.reader(),
            remote: self.remote(token),
            transport,
        })
    }

    fn versions(&self) -> GitVersions<BackendHost> {
        self.versions_with(TOKEN, TestTransport::new())
    }

    fn repo(&self, transport: TestTransport) -> PushingRepo<DiskReader, TestTransport> {
        PushingRepo {
            reader: self.reader(),
            remote: self.remote(TOKEN),
            transport,
        }
    }

    fn rev(&self, name: &str) -> ObjectId {
        ObjectId::from_hex(&self.home.git_text(&self.bare, &["rev-parse", name])).unwrap()
    }

    fn fsck(&self) {
        self.home
            .git(&self.bare, &["fsck", "--strict", "--no-dangling"]);
    }

    /// A transport that, just before the first receive-pack POST, makes
    /// `name` point at `at` on the server.
    fn set_ref_before_post(&self, name: &str, at: ObjectId) -> TestTransport {
        let (home, bare, name) = (self.home.clone(), self.bare.clone(), name.to_string());
        let mut done = false;
        TestTransport::hooked(move |req| {
            if !done && req.method == "POST" {
                done = true;
                home.git(&bare, &["update-ref", &name, &at.to_hex()]);
            }
            None
        })
    }
}

fn meta(note: &str, time: i64) -> VersionMeta {
    VersionMeta {
        author: Person {
            name: "Mia Example".into(),
            email: "mia@clax.dev".into(),
        },
        time,
        note: note.into(),
        publisher: "0123456789ab".into(),
        agent: None,
    }
}

fn changes(files: &[(&str, Option<&[u8]>)]) -> Changes {
    files
        .iter()
        .map(|(p, c)| (p.to_string(), c.map(<[u8]>::to_vec)))
        .collect()
}

fn publish(
    version: u64,
    parent: Option<ObjectId>,
    files: &[(&str, Option<&[u8]>)],
    note: &str,
) -> Publish {
    Publish {
        version,
        parent,
        changes: changes(files),
        meta: meta(note, 1_760_176_800 + version as i64),
    }
}

/// Publishes v1 and v2 and answers their commits.
async fn two_versions(f: &Fixture) -> (ObjectId, ObjectId) {
    let v = f.versions();
    let c1 = v
        .publish(
            ARTIFACT,
            &publish(1, None, &[("index.html", Some(b"<h1>v1</h1>\n"))], "First"),
        )
        .await
        .unwrap();
    let c2 = v
        .publish(
            ARTIFACT,
            &publish(
                2,
                Some(c1.commit),
                &[("index.html", Some(b"<h1>v2</h1>\n"))],
                "",
            ),
        )
        .await
        .unwrap();
    (c1.commit, c2.commit)
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_two_versions_that_fsck_log_and_clone_accept() {
    let f = Fixture::new().await;
    let v = f.versions();
    let big = format!("<p>{}</p>\n", "lorem ipsum ".repeat(9000));
    let c1 = v
        .publish(
            ARTIFACT,
            &Publish {
                version: 1,
                parent: None,
                changes: changes(&[
                    ("index.html", Some(b"<h1>v1</h1>\n")),
                    ("css/site.css", Some(b"body { color: red }\n")),
                    ("css/old.css", Some(b"old\n")),
                    ("docs/big.html", Some(big.as_bytes())),
                    ("empty.txt", Some(b"")),
                ]),
                meta: meta("First version", 1_760_176_800),
            },
        )
        .await
        .unwrap();
    assert_eq!(c1.tag_error, None);
    let c2 = v
        .publish(
            ARTIFACT,
            &Publish {
                version: 2,
                parent: Some(c1.commit),
                changes: changes(&[
                    ("index.html", Some(b"<h1>v2</h1>\n")),
                    ("css/old.css", None),
                    ("img/a/b/note.txt", Some(b"nested\n")),
                ]),
                meta: VersionMeta {
                    agent: Some("claude".into()),
                    ..meta("", 1_760_180_400)
                },
            },
        )
        .await
        .unwrap();

    f.fsck();
    let log = f.home.git_text(&f.bare, &["log", "--format=%H", "main"]);
    assert_eq!(log, format!("{}\n{}", c2.commit, c1.commit));
    assert_eq!(f.rev("v1"), c1.commit);
    assert_eq!(f.rev("v2"), c2.commit);
    assert_eq!(
        f.home.git_text(
            &f.bare,
            &[
                "log",
                "-1",
                "--format=%an <%ae> %at %cn <%ce> %ct%n%B",
                "main"
            ]
        ),
        "Mia Example <mia@clax.dev> 1760180400 Mia Example <mia@clax.dev> 1760180400\n\
         Clax-Version: 2\nClax-Publisher: 0123456789ab\nClax-Agent: claude"
    );
    assert_eq!(
        f.home
            .git_text(&f.bare, &["log", "-1", "--format=%B", "v1"]),
        "First version\n\nClax-Version: 1\nClax-Publisher: 0123456789ab"
    );

    // Reads through the same interface.
    assert_eq!(
        v.read_file(ARTIFACT, &Rev::Version(1), "css/old.css")
            .await
            .unwrap(),
        Some(b"old\n".to_vec())
    );
    assert_eq!(
        v.read_file(ARTIFACT, &Rev::Version(2), "css/old.css")
            .await
            .unwrap(),
        None
    );
    let log = v.log(ARTIFACT, None, None).await.unwrap();
    assert_eq!(
        log.iter().map(|c| c.commit).collect::<Vec<_>>(),
        [c2.commit, c1.commit]
    );
    assert_eq!(log[0].trailer("Clax-Agent"), Some("claude"));

    let work = temp("clone");
    let [c, h] = bearer_args(TOKEN);
    let out = work.path().join("c");
    f.home
        .ok_async(
            "git",
            work.path(),
            &[
                &c,
                &h,
                "clone",
                "-q",
                &f.remote_url(),
                out.to_str().unwrap(),
            ],
            &[],
        )
        .await;
    let files = files_of(&out);
    let expect: Vec<(&str, &[u8])> = vec![
        ("index.html", b"<h1>v2</h1>\n"),
        ("css/site.css", b"body { color: red }\n"),
        ("docs/big.html", big.as_bytes()),
        ("empty.txt", b""),
        ("img/a/b/note.txt", b"nested\n"),
    ];
    assert_eq!(files.len(), expect.len());
    for (p, c) in expect {
        assert_eq!(files[p], c, "{p}");
    }
    f.home.git(&out, &["checkout", "-q", "v1"]);
    assert_eq!(std::fs::read(out.join("css/old.css")).unwrap(), b"old\n");
    assert_eq!(f.home.git_text(&out, &["tag", "--list"]), "v1\nv2");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_parent_is_version_conflict_and_a_taken_tag_invalid_argument_before_pushing() {
    let f = Fixture::new().await;
    let (c1, c2) = two_versions(&f).await;
    let v = f.versions();
    let e = v
        .publish(
            ARTIFACT,
            &publish(3, Some(c1), &[("index.html", Some(b"stale"))], "stale"),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "version_conflict");
    assert!(matches!(e, GitError::VersionConflict { head: Some(h), .. } if h == c2));
    let e = v
        .publish(ARTIFACT, &publish(2, Some(c2), &[("x", Some(b"x"))], "m"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    assert_eq!(f.rev("main"), c2);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_remotes_non_fast_forward_refusal_is_version_conflict() {
    let f = Fixture::new().await;
    let (c1, c2) = two_versions(&f).await;
    let repo = f.repo(TestTransport::new());
    // A commit whose parent is v1: `old` is main, but it does not descend from it.
    let parent_tree = Commit::parse(&repo.read_object(&c1).await.unwrap().unwrap().data)
        .unwrap()
        .tree;
    let (tree, mut objects) =
        build_tree(&repo, Some(parent_tree), &changes(&[("f", Some(b"side"))]))
            .await
            .unwrap();
    let sig = Signature::utc(&meta("", 0).author, 0);
    let side = Commit {
        tree,
        parents: vec![c1],
        author: sig.clone(),
        committer: sig,
        message: "side".into(),
    }
    .to_object()
    .unwrap();
    let side_id = side.id();
    objects.push(side);
    let updates = [
        RefUpdate::new(MAIN, Some(c2), Some(side_id)),
        RefUpdate::new("refs/tags/side", None, Some(side_id)),
    ];
    let e = commit_refs(&repo, objects, &updates).await.unwrap_err();
    assert_eq!(e.code(), "version_conflict");
    assert!(e.to_string().contains("non-fast-forward"), "{e}");
    assert_eq!(f.rev("main"), c2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_token_is_an_auth_error() {
    let f = Fixture::new().await;
    let e = f
        .versions_with("wrong", TestTransport::new())
        .publish(ARTIFACT, &publish(1, None, &[("f", Some(b"x"))], "m"))
        .await
        .unwrap_err();
    assert!(matches!(e, GitError::Auth { status: 401, .. }), "{e:?}");
    assert_eq!(e.code(), "repo_unavailable");
}

#[tokio::test(flavor = "multi_thread")]
async fn creates_a_branch_from_nothing() {
    let f = Fixture::new().await;
    let repo = f.repo(TestTransport::new());
    let (tree, mut objects) = build_tree(&repo, None, &changes(&[("r", Some(b"r"))]))
        .await
        .unwrap();
    let sig = Signature::utc(&meta("", 0).author, 0);
    let c = Commit {
        tree,
        parents: vec![],
        author: sig.clone(),
        committer: sig,
        message: "root".into(),
    }
    .to_object()
    .unwrap();
    let id = c.id();
    objects.push(c);
    let statuses = apply(
        &f.remote(TOKEN),
        &TestTransport::new(),
        &objects,
        &[RefUpdate::new("refs/heads/other", None, Some(id))],
        true,
    )
    .await
    .unwrap();
    assert_eq!(statuses, [Ok(())]);
    assert_eq!(f.rev("other"), id);
    f.fsck();
}

#[tokio::test(flavor = "multi_thread")]
async fn atomically_a_tag_taken_during_the_push_moves_nothing_and_is_invalid_argument() {
    let f = Fixture::new().await;
    let (_, c2) = two_versions(&f).await;
    let v = f.versions_with(TOKEN, f.set_ref_before_post("refs/tags/v3", c2));
    let e = v
        .publish(
            ARTIFACT,
            &publish(3, Some(c2), &[("race", Some(b"1"))], "m"),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e}");
    assert_eq!(f.rev("main"), c2);
}

#[tokio::test(flavor = "multi_thread")]
async fn atomically_main_moved_during_the_push_is_version_conflict_with_the_new_head() {
    let f = Fixture::new().await;
    let (c1, c2) = two_versions(&f).await;
    let v = f.versions_with(TOKEN, f.set_ref_before_post(MAIN, c1));
    let e = v
        .publish(
            ARTIFACT,
            &publish(3, Some(c2), &[("race", Some(b"0"))], "m"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(e, GitError::VersionConflict { head: Some(h), .. } if h == c1),
        "{e:?}"
    );
    assert_eq!(f.home.git_text(&f.bare, &["tag", "--list", "v3"]), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_atomic_pushes_main_then_the_tag_and_reports_a_tag_refused_after_main_moved() {
    let f = Fixture::new().await;
    let (_, c2) = two_versions(&f).await;
    f.home
        .git(&f.bare, &["config", "receive.advertiseAtomic", "false"]);
    let v = f.versions_with(TOKEN, f.set_ref_before_post("refs/tags/v3", c2));
    let r = v
        .publish(
            ARTIFACT,
            &publish(3, Some(c2), &[("race", Some(b"2"))], "m"),
        )
        .await
        .unwrap();
    assert!(
        r.tag_error
            .as_deref()
            .unwrap_or("")
            .contains("refs/tags/v3"),
        "{r:?}"
    );
    assert_eq!(f.rev("main"), r.commit);
    assert_eq!(f.rev("v3"), c2);

    let ok = f
        .versions()
        .publish(
            ARTIFACT,
            &publish(4, Some(r.commit), &[("race", Some(b"3"))], "m"),
        )
        .await
        .unwrap();
    assert_eq!(ok.tag_error, None);
    assert_eq!(f.rev("main"), ok.commit);
    assert_eq!(f.rev("v4"), ok.commit);
    f.fsck();
}

#[tokio::test(flavor = "multi_thread")]
async fn without_atomic_a_401_on_the_tag_request_is_a_tag_error_after_main_moved() {
    let f = Fixture::new().await;
    let (_, c2) = two_versions(&f).await;
    f.home
        .git(&f.bare, &["config", "receive.advertiseAtomic", "false"]);
    let mut posts = 0;
    let t = TestTransport::hooked(move |req| {
        if req.method == "POST" {
            posts += 1;
            if posts == 2 {
                for (k, v) in &mut req.headers {
                    if *k == "authorization" {
                        *v = "Bearer expired".into();
                    }
                }
            }
        }
        None
    });
    let r = f
        .versions_with(TOKEN, t)
        .publish(
            ARTIFACT,
            &publish(3, Some(c2), &[("auth", Some(b"a"))], "m"),
        )
        .await
        .unwrap();
    assert!(
        r.tag_error.as_deref().unwrap_or("").contains("401"),
        "{r:?}"
    );
    assert_eq!(f.rev("main"), r.commit);
    assert_eq!(f.home.git_text(&f.bare, &["tag", "--list", "v3"]), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_nul_in_a_commit_message() {
    let f = Fixture::new().await;
    let (_, c2) = two_versions(&f).await;
    let e = f
        .versions()
        .publish(ARTIFACT, &publish(3, Some(c2), &[], "a\0b"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
}

#[tokio::test(flavor = "multi_thread")]
async fn atomically_a_tag_refused_for_itself_is_repo_unavailable_naming_its_reason() {
    // receive-pack refuses a malformed ref name ("funny refname") on its own.
    let f = Fixture::new().await;
    let (_, c2) = two_versions(&f).await;
    let repo = f.repo(TestTransport::new());
    let tree = Commit::parse(&repo.read_object(&c2).await.unwrap().unwrap().data)
        .unwrap()
        .tree;
    let (tree, mut objects) = build_tree(&repo, Some(tree), &changes(&[("funny", Some(b"f"))]))
        .await
        .unwrap();
    let sig = Signature::utc(&meta("", 0).author, 0);
    let c = Commit {
        tree,
        parents: vec![c2],
        author: sig.clone(),
        committer: sig,
        message: "m".into(),
    }
    .to_object()
    .unwrap();
    let id = c.id();
    objects.push(c);
    let updates = [
        RefUpdate::new(MAIN, Some(c2), Some(id)),
        RefUpdate::new("refs/tags/v6..bad", None, Some(id)),
    ];
    let e = commit_refs(&repo, objects, &updates).await.unwrap_err();
    assert_eq!(e.code(), "repo_unavailable");
    assert!(
        e.to_string().contains("refs/tags/v6..bad: funny refname"),
        "{e}"
    );
    assert_eq!(f.rev("main"), c2);
}

#[tokio::test(flavor = "multi_thread")]
async fn atomically_a_failed_re_read_after_a_refused_push_propagates() {
    let f = Fixture::new().await;
    let (_, c2) = two_versions(&f).await;
    let (home, bare) = (f.home.clone(), f.bare.clone());
    let mut posted = false;
    let t = TestTransport::hooked(move |req| {
        if req.method == "POST" {
            posted = true;
            home.git(&bare, &["update-ref", "refs/tags/v3", &c2.to_hex()]);
        } else if posted {
            return Some(HttpResponse::new(503, "down"));
        }
        None
    });
    let e = f
        .versions_with(TOKEN, t)
        .publish(
            ARTIFACT,
            &publish(3, Some(c2), &[("reread", Some(b"r"))], "m"),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "repo_unavailable");
    assert!(e.to_string().contains("503"), "{e}");
    assert_eq!(f.rev("main"), c2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_that_says_atomic_but_takes_main_and_refuses_the_tag_gives_a_tag_error() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let t = TestTransport::hooked(move |req| {
        seen.lock().unwrap().push(req.method);
        let mut body = Vec::new();
        if req.method == "POST" {
            for l in [
                "unpack ok\n",
                "ok refs/heads/main\n",
                "ng refs/tags/v1 tag storage full\n",
            ] {
                pktline::write(&mut body, l.as_bytes());
            }
        } else {
            pktline::write(&mut body, b"# service=git-receive-pack\n");
            body.extend_from_slice(pktline::FLUSH);
            pktline::write(
                &mut body,
                format!(
                    "{} capabilities^{{}}\0report-status atomic\n",
                    ObjectId::ZERO
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(pktline::FLUSH);
        Some(HttpResponse::new(200, body))
    });
    let host = BackendHost {
        reader: DiskReader {
            env: vec![],
            dir: std::env::temp_dir(),
        },
        remote: Remote::new("http://remote.test/a-0123456789ab.git", "t"),
        transport: t,
    };
    let r = GitVersions::new(host)
        .publish(ARTIFACT, &publish(1, None, &[("f", Some(b"f"))], "m"))
        .await
        .unwrap();
    assert_eq!(
        r.tag_error.as_deref(),
        Some("The git remote refused refs/tags/v1: tag storage full")
    );
    assert_eq!(*calls.lock().unwrap(), ["GET", "POST"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_refs_creates_moves_and_deletes_tags_in_one_request_each() {
    let f = Fixture::new().await;
    let (c1, c2) = two_versions(&f).await;
    let t = TestTransport::new();
    let remote = f.remote(TOKEN);
    let refs = |pairs: &[(&str, Option<ObjectId>)]| -> BTreeMap<String, Option<ObjectId>> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    };
    let changed = set_refs(
        &remote,
        &t,
        &refs(&[("refs/tags/v1", Some(c2)), ("refs/tags/v9", Some(c2))]),
    )
    .await
    .unwrap();
    assert_eq!(changed, ["refs/tags/v1", "refs/tags/v9"]);
    assert_eq!(f.rev("refs/tags/v1"), c2);
    // Already there, and a missing deletion: nothing sent.
    assert!(
        set_refs(
            &remote,
            &t,
            &refs(&[("refs/tags/v1", Some(c2)), ("refs/tags/none", None)])
        )
        .await
        .unwrap()
        .is_empty()
    );
    let changed = set_refs(
        &remote,
        &t,
        &refs(&[("refs/tags/v1", Some(c1)), ("refs/tags/v9", None)]),
    )
    .await
    .unwrap();
    assert_eq!(changed, ["refs/tags/v1", "refs/tags/v9"]);
    let adv = advertised_refs(&remote, &t).await.unwrap();
    assert_eq!(adv.refs.get("refs/tags/v1"), Some(&c1));
    assert!(!adv.refs.contains_key("refs/tags/v9"));
    f.fsck();
}

//! Commits pushed with real git through clax-git's smart-HTTP server become
//! versions in a store over the git content store: published by the
//! pusher (named by the push ledger, whichever reconcile records them),
//! with publish's path rules, only while the pusher may still publish, and
//! after the tokens of agents who may not see them are revoked; a publish
//! that did not see them is a conflict.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clax_content_git::MemGitContent;
use clax_core::RegisterSession;
use clax_core::audit::AuditCtx;
use clax_core::content::{ContentStore, PushLedger, RepoToken};
use clax_core::publish::{PublishRequest, ValidatedPublish, validate};
use clax_core::rights::{PRESET_COLLABORATOR, Rights};
use clax_store::people::{GrantStart, InvitePerson, NewGrant};
use clax_store::pushes::Pusher;
use clax_core::{ArtifactId, CoreError, Home, Secret, Store};
use clax_git::versions::repo_name;

use crate::common::{Git, Server, clock, committer};

const DAEMON: &AuditCtx = &AuditCtx::DAEMON;

/// A push ledger in memory: the pusher of each commit a push left `main`
/// at, and tokens, whose revocation can be made to fail.
#[derive(Default)]
struct Ledger {
    pushers: Mutex<HashMap<String, Pusher>>,
    tokens: Mutex<Vec<RepoToken>>,
    revoked: Mutex<Vec<String>>,
    fail_revoke: AtomicBool,
}

#[async_trait::async_trait]
impl PushLedger for Ledger {
    async fn pushers(
        &self,
        _id: &ArtifactId,
        commits: &[String],
    ) -> clax_core::Result<Vec<Option<Pusher>>> {
        let known = self.pushers.lock().unwrap();
        Ok(commits.iter().map(|c| known.get(c).cloned()).collect())
    }

    async fn tokens(&self, _id: &ArtifactId) -> clax_core::Result<Vec<RepoToken>> {
        let revoked = self.revoked.lock().unwrap();
        Ok(self
            .tokens
            .lock()
            .unwrap()
            .iter()
            .filter(|t| !revoked.contains(&t.id))
            .cloned()
            .collect())
    }

    async fn revoke(&self, _id: &ArtifactId, token: &str) -> clax_core::Result<()> {
        if self.fail_revoke.load(Ordering::SeqCst) {
            return Err(CoreError::Io(std::io::Error::other(
                "the token host is down",
            )));
        }
        self.revoked.lock().unwrap().push(token.to_string());
        Ok(())
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    st: Arc<Store>,
    content: Arc<MemGitContent>,
    ledger: Arc<Ledger>,
    id: ArtifactId,
    _server: Server,
    git: Git,
    _work: tempfile::TempDir,
    clone: PathBuf,
}

fn publish(html: &str, if_version: Option<u32>) -> ValidatedPublish {
    let req: PublishRequest = serde_json::from_value(serde_json::json!({
        "if_version": if_version,
        "files": {"index.html": {"content": html}},
    }))
    .unwrap();
    validate(req).unwrap()
}

/// A connected person named `name`: their public ID.
fn person(st: &Store, name: &str) -> String {
    person_with_grant(st, name, 7).0
}

/// A connected person named `name`, connected with key `[key; 32]`: their
/// public ID and their grant's ID.
fn person_with_grant(st: &Store, name: &str, key: u8) -> (String, String) {
    let (_, code) = st
        .create_invite(DAEMON, InvitePerson::New(name.into()), None)
        .unwrap();
    let start = st
        .request_grant(
            DAEMON,
            NewGrant {
                key: [key; 32],
                client_name: "laptop".into(),
                peer_ip: "192.168.1.20".into(),
                access: Rights::preset(PRESET_COLLABORATOR).unwrap(),
                invite: Some(Secret::new(code.expose().to_string())),
            },
        )
        .unwrap();
    let GrantStart::Approved { grant, .. } = start else {
        panic!("{start:?}")
    };
    (grant.person.unwrap(), grant.id)
}

/// Lets person `pid` publish `id` as well as what a collaborator may.
fn may_publish(st: &Store, pid: &str, id: &ArtifactId) {
    let rights = Rights::parse(&serde_json::json!([
        PRESET_COLLABORATOR,
        {"type": "clax-artifacts", "actions": ["read", "update"], "locations": [id.as_str()]}
    ]))
    .unwrap();
    st.set_person_rights(DAEMON, pid, Some(rights)).unwrap();
}

/// A live agent session of grant `gid`: its ID.
fn agent(st: &Store, gid: &str) -> String {
    st.register_session(
        DAEMON,
        RegisterSession {
            harness: "claude".into(),
            cwd: "/w".into(),
            grant_id: Some(gid.into()),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn as_person(pid: &str) -> Pusher {
    Pusher {
        person: Some(pid.into()),
        session_id: None,
    }
}

/// A store over an in-memory git host holding one artifact at version 1,
/// and a clone of its repository.
async fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let ledger = Arc::new(Ledger::default());
    let content =
        Arc::new(MemGitContent::in_memory(committer(), clock()).with_ledger(ledger.clone()));
    let st = Arc::new(
        Store::open(&Home::at(home.path().join("clax")))
            .unwrap()
            .with_content(content.clone()),
    );
    let (a, _) = st
        .create_artifact_async(DAEMON, publish("<p>one</p>", None), None)
        .await
        .unwrap();
    let id = ArtifactId::parse(&a.id).unwrap();
    let server = Server::start(content.host().clone()).await;
    let git = Git::new();
    let work = tempfile::tempdir().unwrap();
    let clone = work.path().join("clone");
    git.text(
        work.path(),
        &[
            "clone",
            "-q",
            &server.url(&repo_name(id.as_str())),
            clone.to_str().unwrap(),
        ],
    )
    .await;
    Fixture {
        _home: home,
        st,
        content,
        ledger,
        id,
        _server: server,
        git,
        _work: work,
        clone,
    }
}

impl Fixture {
    async fn commit(&self, message: &str) {
        let dir: &Path = &self.clone;
        self.git.text(dir, &["add", "-A"]).await;
        self.git
            .text(
                dir,
                &[
                    "-c",
                    "user.name=Rae",
                    "-c",
                    "user.email=rae@example.com",
                    "commit",
                    "-qm",
                    message,
                ],
            )
            .await;
    }

    fn write(&self, path: &str, body: &str) {
        let p = self.clone.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    async fn push(&self) {
        self.git
            .text(&self.clone, &["push", "-q", "origin", "HEAD:main"])
            .await;
    }

    /// Pushes `HEAD` to `main` as `pusher`, as the push ledger records it.
    async fn push_as(&self, pusher: Pusher) {
        self.push().await;
        let head = self.git.text(&self.clone, &["rev-parse", "HEAD"]).await;
        self.ledger.pushers.lock().unwrap().insert(head, pusher);
    }

    /// The `version.push_refused` events recorded: each one's commit and
    /// reason.
    fn refusals(&self) -> Vec<(String, String)> {
        self.st
            .events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "version.push_refused")
            .map(|e| {
                let b: serde_json::Value = serde_json::from_str(&e.body).unwrap();
                assert_eq!(e.ids.artifact.as_deref(), Some(self.id.as_str()));
                (
                    b["commit"].as_str().unwrap().to_string(),
                    b["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    fn versions(&self) -> Vec<clax_core::model::Version> {
        self.st.list_versions(&self.id).unwrap()
    }

    async fn read(&self, n: u32, path: &str) -> Option<String> {
        self.st
            .read_file_async(&self.id, n, path)
            .await
            .unwrap()
            .map(|(b, _)| String::from_utf8(b).unwrap())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_pushed_commits_become_versions_published_by_the_pusher() {
    let f = fixture().await;
    let rae = person(&f.st, "Rae");
    may_publish(&f.st, &rae, &f.id);
    f.write("index.html", "<p>two</p>");
    f.write("js/app.js", "go()");
    f.commit("Second take").await;
    f.write("styles.css", "p{}");
    f.commit("Add styles").await;
    f.push().await;

    let recorded =
        f.st.reconcile_pushes_async(
            DAEMON,
            &f.id,
            &Pusher {
                person: Some(rae.clone()),
                session_id: None,
            },
        )
        .await
        .unwrap();
    let ns: Vec<u32> = recorded.iter().map(|(_, v)| v.n).collect();
    assert_eq!(ns, [2, 3]);
    assert_eq!(recorded[1].0.current_version, 3);
    let versions = f.st.list_versions(&f.id).unwrap();
    assert_eq!(versions.len(), 3);
    assert_eq!(versions[0].authors.person, None, "the owner published v1");
    for v in &versions[1..] {
        assert_eq!(v.authors.person.as_deref(), Some(rae.as_str()));
    }
    assert_eq!(versions[1].note.as_deref(), Some("Second take"));
    assert_eq!(versions[2].note.as_deref(), Some("Add styles"));
    let v2: Vec<&str> = versions[1].files.keys().map(String::as_str).collect();
    assert_eq!(v2, ["index.html", "js/app.js"]);
    let v3: Vec<&str> = versions[2].files.keys().map(String::as_str).collect();
    assert_eq!(v3, ["index.html", "js/app.js", "styles.css"]);
    assert_eq!(
        versions[2].files["styles.css"].content_type, "text/css",
        "content types come from the path"
    );
    assert_eq!(f.read(2, "index.html").await.as_deref(), Some("<p>two</p>"));
    assert_eq!(f.read(3, "styles.css").await.as_deref(), Some("p{}"));
    // A carried file keeps its authors; the files the push wrote are Rae's.
    let authors = versions[2].authors.files.as_ref().unwrap();
    assert_eq!(authors["styles.css"], [Some(rae.clone())]);
    assert_eq!(authors["index.html"], [Some(rae.clone())]);

    // Nothing more to record, and the next publish builds on v3.
    assert!(
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap()
            .is_empty()
    );
    let (a, v) =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>four</p>", Some(3)), None)
            .await
            .unwrap();
    assert_eq!((a.current_version, v.n), (4, 4));
    assert_eq!(f.read(4, "styles.css").await.as_deref(), Some("p{}"));
    f.git
        .text(&f.clone, &["pull", "-q", "--tags", "origin", "main"])
        .await;
    assert_eq!(
        f.git.text(&f.clone, &["tag", "--list"]).await,
        "v1\nv2\nv3\nv4"
    );
    assert_eq!(
        f.git.text(&f.clone, &["rev-parse", "v3"]).await,
        f.git.text(&f.clone, &["rev-parse", "HEAD~1"]).await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_path_publish_refuses_is_left_out_of_the_pushed_version_and_named_in_its_note() {
    let f = fixture().await;
    f.write("bad\tname.html", "x");
    f.write("ok.txt", "fine");
    f.commit("Odd names").await;
    // Case twins, which this machine's file system may not hold apart.
    for name in ["Notes.txt", "notes.txt"] {
        let blob = f.git.text(&f.clone, &["hash-object", "-w", "ok.txt"]).await;
        f.git
            .text(
                &f.clone,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{name}"),
                ],
            )
            .await;
    }
    f.git
        .text(
            &f.clone,
            &[
                "-c",
                "user.name=Rae",
                "-c",
                "user.email=rae@example.com",
                "commit",
                "--amend",
                "-qm",
                "Odd names",
            ],
        )
        .await;
    f.push().await;
    let recorded =
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap();
    assert_eq!(recorded.len(), 1);
    let v = &recorded[0].1;
    let files: Vec<&str> = v.files.keys().map(String::as_str).collect();
    // `Notes.txt` sorts first and is kept; `notes.txt` then collides.
    assert_eq!(files, ["Notes.txt", "index.html", "ok.txt"]);
    let note = v.note.as_deref().unwrap();
    assert!(note.starts_with("2 files were not taken"), "{note}");
    assert!(note.contains("bad name.html, notes.txt."), "{note}");
    assert!(note.ends_with("Odd names"), "{note}");
    assert_eq!(v.authors.person, None, "an unknown pusher is the owner");
    // Still in git.
    let commit = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    assert_eq!(f.content.pushed_commits(&f.id, 2).await.unwrap().len(), 0);
    assert!(
        f.git
            .text(&f.clone, &["ls-tree", "--name-only", &commit])
            .await
            .contains("bad")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_that_did_not_see_a_push_is_a_conflict_and_the_push_a_version() {
    let f = fixture().await;
    f.write("index.html", "<p>pushed</p>");
    f.commit("Pushed").await;
    f.push().await;
    // A publish with no reconcile before it records the push itself.
    let stale =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>mine</p>", Some(1)), None)
            .await;
    assert!(
        matches!(stale, Err(CoreError::Conflict { current: 2 })),
        "{stale:?}"
    );
    assert_eq!(
        f.read(2, "index.html").await.as_deref(),
        Some("<p>pushed</p>")
    );

    // A push racing a publish and its reconcile: the content lock orders
    // them, the publish that did not see the push conflicts, and the push
    // is recorded once.
    f.write("index.html", "<p>pushed again</p>");
    f.commit("Pushed again").await;
    f.push().await;
    let (st1, st2) = (f.st.clone(), f.st.clone());
    let id1 = f.id.clone();
    let id2 = f.id.clone();
    let publishing = tokio::spawn(async move {
        st1.publish_version_async(DAEMON, &id1, publish("<p>mine</p>", Some(2)), None)
            .await
    });
    let reconciling = tokio::spawn(async move {
        st2.reconcile_pushes_async(DAEMON, &id2, &Pusher::default())
            .await
    });
    let published = publishing.await.unwrap();
    let reconciled = reconciling.await.unwrap().unwrap();
    assert!(
        matches!(published, Err(CoreError::Conflict { current: 3 })),
        "{published:?}"
    );
    assert!(reconciled.len() <= 1);
    let ns: Vec<u32> =
        f.st.list_versions(&f.id)
            .unwrap()
            .iter()
            .map(|v| v.n)
            .collect();
    assert_eq!(ns, [1, 2, 3]);
    assert_eq!(
        f.read(3, "index.html").await.as_deref(),
        Some("<p>pushed again</p>")
    );
    // Re-read and publish again: it lands.
    let (_, v) =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>mine</p>", Some(3)), None)
            .await
            .unwrap();
    assert_eq!(v.n, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn a_store_on_one_connection_records_a_push_before_a_publish() {
    let home = tempfile::tempdir().unwrap();
    let content = Arc::new(MemGitContent::in_memory(committer(), clock()));
    let h = Home::at(home.path().join("clax"));
    h.ensure_dirs().unwrap();
    let conn = rusqlite::Connection::open(h.db_path()).unwrap();
    let st = Arc::new(
        Store::open_on(
            &h,
            conn,
            Arc::new(clax_core::working::SystemClock),
            content.clone(),
        )
        .unwrap(),
    );
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tell = heard.clone();
    st.set_pushed_listener(move |_, v| tell.lock().unwrap().push(v.n));
    let (a, _) = st
        .call_async(|st| async move {
            st.create_artifact_async(DAEMON, publish("<p>one</p>", None), None)
                .await
        })
        .await
        .unwrap();
    let id = ArtifactId::parse(&a.id).unwrap();
    let server = Server::start(content.host().clone()).await;
    let git = Git::new();
    let work = tempfile::tempdir().unwrap();
    let clone = work.path().join("clone");
    git.text(
        work.path(),
        &[
            "clone",
            "-q",
            &server.url(&repo_name(id.as_str())),
            clone.to_str().unwrap(),
        ],
    )
    .await;
    std::fs::write(clone.join("index.html"), "<p>pushed</p>").unwrap();
    git.text(&clone, &["add", "-A"]).await;
    git.text(
        &clone,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "commit",
            "-qm",
            "Pushed",
        ],
    )
    .await;
    git.text(&clone, &["push", "-q", "origin", "HEAD:main"])
        .await;
    let id2 = id.clone();
    let stale = st
        .call_async(move |st| async move {
            st.publish_version_async(DAEMON, &id2, publish("<p>mine</p>", Some(1)), None)
                .await
        })
        .await;
    assert!(
        matches!(stale, Err(CoreError::Conflict { current: 2 })),
        "{stale:?}"
    );
    assert_eq!(*heard.lock().unwrap(), [2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_reconcile_credits_a_pushed_commit_to_whoever_pushed_it() {
    let f = fixture().await;
    let rae = person(&f.st, "Rae");
    may_publish(&f.st, &rae, &f.id);
    f.write("index.html", "<p>rae</p>");
    f.commit("Rae's take").await;
    f.push_as(as_person(&rae)).await;
    f.write("a.txt", "a");
    f.commit("Owner one").await;
    f.write("b.txt", "b");
    f.commit("Owner two").await;
    f.push_as(Pusher::default()).await;

    // No push's own reconcile ran: the publish's records them, each as
    // its pusher's, and conflicts.
    let stale =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>mine</p>", Some(1)), None)
            .await;
    assert!(
        matches!(stale, Err(CoreError::Conflict { current: 4 })),
        "{stale:?}"
    );
    let by: Vec<Option<String>> = f
        .versions()
        .iter()
        .map(|v| v.authors.person.clone())
        .collect();
    assert_eq!(by, [None, Some(rae.clone()), None, None]);
    let authors = f.versions()[3].authors.files.clone().unwrap();
    assert_eq!(
        authors["index.html"],
        [Some(rae.clone())],
        "carried with its author"
    );
    assert_eq!(authors["b.txt"], [Some("*owner".to_string())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_push_by_someone_who_may_no_longer_publish_is_not_recorded_and_is_audited() {
    let f = fixture().await;
    let rae = person(&f.st, "Rae");
    may_publish(&f.st, &rae, &f.id);
    let (sam, sam_grant) = person_with_grant(&f.st, "Sam", 8);
    let sam_sid = agent(&f.st, &sam_grant);
    f.ledger.tokens.lock().unwrap().push(RepoToken {
        id: "t_sam".into(),
        write: false,
        holder: Pusher {
            person: Some(sam),
            session_id: Some(sam_sid),
        },
    });
    f.write("index.html", "<p>rae</p>");
    f.commit("Rae's take").await;
    f.push_as(as_person(&rae)).await;
    let refused = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    f.st.revoke_person(DAEMON, &rae).unwrap();

    for _ in 0..2 {
        assert!(
            f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(f.versions().len(), 1);
    assert_eq!(
        *f.ledger.revoked.lock().unwrap(),
        ["t_sam"],
        "a clone reaches the refused commit: Sam's agent may not see Rae's work"
    );
    assert_eq!(
        f.refusals(),
        [(refused.clone(), "person_revoked".to_string())],
        "named once, however often it is reconciled"
    );
    assert_eq!(
        f.content.pushed_commits(&f.id, 1).await.unwrap().len(),
        1,
        "untagged: still a pushed commit"
    );
    assert_eq!(f.read(2, "index.html").await, None);
    let blocked =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>mine</p>", Some(1)), None)
            .await;
    assert!(
        matches!(
            &blocked,
            Err(CoreError::Invalid {
                code: "push_refused",
                ..
            })
        ),
        "{blocked:?}"
    );

    // The owner pushes over it: that commit is the next version, and says
    // which commits are not versions.
    f.write("index.html", "<p>owner</p>");
    f.commit("Owner's fix").await;
    f.push_as(Pusher::default()).await;
    let recorded =
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap();
    assert_eq!(recorded.len(), 1);
    let v = &recorded[0].1;
    assert_eq!((v.n, v.authors.person.as_deref()), (2, None));
    let note = v.note.as_deref().unwrap();
    assert!(
        note.starts_with("Not versions (pushed by someone who may not publish here;")
            && note.contains(&refused[..12])
            && note.ends_with("Owner's fix"),
        "{note}"
    );
    let (_, v3) =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>three</p>", Some(2)), None)
            .await
            .unwrap();
    assert_eq!(v3.n, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_is_rechecked_against_its_rights_as_they_are_when_recorded() {
    let f = fixture().await;
    let (rae, gid) = person_with_grant(&f.st, "Rae", 7);
    may_publish(&f.st, &rae, &f.id);
    let sid = agent(&f.st, &gid);
    let rae_agent = Pusher {
        person: Some(rae.clone()),
        session_id: Some(sid.clone()),
    };
    let reader = Rights::preset(clax_core::rights::PRESET_READER).unwrap();
    f.st.narrow_agent(DAEMON, &sid, Some(reader)).unwrap();
    f.write("index.html", "<p>agent</p>");
    f.commit("Agent's take").await;
    f.push_as(rae_agent.clone()).await;
    let first = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    assert!(
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap()
            .is_empty(),
        "narrowed to reading"
    );
    assert_eq!(f.refusals(), [(first, "no_right".to_string())]);

    // Its narrowing lifted, the same commit is recorded as its own.
    f.st.narrow_agent(DAEMON, &sid, None).unwrap();
    let recorded =
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap();
    let v = &recorded[0].1;
    assert_eq!(v.authors.person.as_deref(), Some(rae.as_str()));
    assert_eq!(v.session_id.as_deref(), Some(sid.as_str()));

    // A token naming another person's session is not that agent's.
    f.write("x.txt", "x");
    f.commit("Not mine").await;
    f.push_as(Pusher {
        person: None,
        session_id: Some(sid.clone()),
    })
    .await;
    let foreign = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    assert!(
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap()
            .is_empty()
    );
    // Once the session ends, its pushes are refused too.
    f.st.end_session(DAEMON, &sid).unwrap();
    f.write("y.txt", "y");
    f.commit("After the end").await;
    f.push_as(rae_agent).await;
    let ended = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    assert!(
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap()
            .is_empty()
    );
    let reasons: Vec<(String, String)> = f.refusals().into_iter().skip(1).collect();
    assert_eq!(
        reasons,
        [
            (foreign, "foreign_session".to_string()),
            (ended, "session_ended".to_string())
        ]
    );
    assert_eq!(f.versions().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_token_that_may_not_see_a_pushed_version_is_revoked_before_it_is_recorded() {
    let f = fixture().await;
    let (rae, rae_grant) = person_with_grant(&f.st, "Rae", 7);
    let (sam, sam_grant) = person_with_grant(&f.st, "Sam", 8);
    may_publish(&f.st, &rae, &f.id);
    let rae_sid = agent(&f.st, &rae_grant);
    let sam_sid = agent(&f.st, &sam_grant);
    let token = |id: &str, person: &str, sid: &str| RepoToken {
        id: id.into(),
        write: false,
        holder: Pusher {
            person: Some(person.into()),
            session_id: Some(sid.into()),
        },
    };
    *f.ledger.tokens.lock().unwrap() = vec![
        token("t_rae", &rae, &rae_sid),
        token("t_sam", &sam, &sam_sid),
        RepoToken {
            id: "t_owner".into(),
            write: true,
            holder: Pusher::default(),
        },
    ];
    f.write("index.html", "<p>rae</p>");
    f.commit("Rae's take").await;
    f.push_as(as_person(&rae)).await;

    // Sam has not let Rae's work reach Sam's agents: while Sam's agent's
    // token cannot be revoked, nothing is recorded.
    f.ledger.fail_revoke.store(true, Ordering::SeqCst);
    assert!(
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .is_err()
    );
    assert_eq!(f.versions().len(), 1);
    assert!(f.ledger.revoked.lock().unwrap().is_empty());
    f.ledger.fail_revoke.store(false, Ordering::SeqCst);
    let recorded =
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        *f.ledger.revoked.lock().unwrap(),
        ["t_sam"],
        "Rae's own agent sees Rae's work; the owner's token is no agent's"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_revokes_the_agents_tokens_blind_to_its_publisher_before_its_commit() {
    let f = fixture().await;
    let (rae, rae_grant) = person_with_grant(&f.st, "Rae", 7);
    let (sam, sam_grant) = person_with_grant(&f.st, "Sam", 8);
    may_publish(&f.st, &rae, &f.id);
    let rae_sid = agent(&f.st, &rae_grant);
    let sam_sid = agent(&f.st, &sam_grant);
    let token = |id: &str, person: &str, sid: &str| RepoToken {
        id: id.into(),
        write: false,
        holder: Pusher {
            person: Some(person.into()),
            session_id: Some(sid.into()),
        },
    };
    *f.ledger.tokens.lock().unwrap() = vec![
        token("t_rae", &rae, &rae_sid),
        token("t_sam", &sam, &sam_sid),
    ];
    let before = f.git.text(&f.clone, &["ls-remote", "origin"]).await;
    // Rae's agent publishes: Sam's agent may not see Rae's work. While its
    // token cannot be revoked, nothing is committed or recorded.
    f.ledger.fail_revoke.store(true, Ordering::SeqCst);
    let e =
        f.st.publish_version_async(
            DAEMON,
            &f.id,
            publish("<p>rae</p>", Some(1)),
            Some(&rae_sid),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            CoreError::Invalid {
                code: "repo_unavailable",
                ..
            }
        ),
        "{e:?}"
    );
    assert_eq!(f.versions().len(), 1);
    assert_eq!(
        f.git.text(&f.clone, &["ls-remote", "origin"]).await,
        before,
        "no commit was written"
    );
    f.ledger.fail_revoke.store(false, Ordering::SeqCst);
    f.st.publish_version_async(
        DAEMON,
        &f.id,
        publish("<p>rae</p>", Some(1)),
        Some(&rae_sid),
    )
    .await
    .unwrap();
    assert_eq!(f.versions().len(), 2);
    assert_eq!(
        *f.ledger.revoked.lock().unwrap(),
        ["t_sam"],
        "Rae's own agent sees Rae's work"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pushed_commit_already_tagged_as_the_next_version_is_adopted_and_tagged_once() {
    let f = fixture().await;
    f.write("index.html", "<p>pushed</p>");
    f.commit("Pushed").await;
    f.push_as(Pusher::default()).await;
    let head = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    // An earlier attempt tagged it and was never recorded.
    assert!(f.content.tag_pushed(&f.id, 2, &head).await.unwrap());
    assert!(
        !f.content.tag_pushed(&f.id, 2, &head).await.unwrap(),
        "already version 2: nothing made"
    );
    let recorded =
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        f.read(2, "index.html").await.as_deref(),
        Some("<p>pushed</p>")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fork_holds_none_of_the_sources_pushes_beyond_the_forked_version() {
    let f = fixture().await;
    let rae = person(&f.st, "Rae");
    may_publish(&f.st, &rae, &f.id);
    // A push above v1, not yet recorded as a version.
    f.write("index.html", "<p>pushed</p>");
    f.commit("Pushed").await;
    f.push_as(as_person(&rae)).await;
    let pushed = f.git.text(&f.clone, &["rev-parse", "HEAD"]).await;
    assert_eq!(f.content.pushed_commits(&f.id, 1).await.unwrap().len(), 1);

    let (a, v) =
        f.st.fork_artifact_async(DAEMON, &f.id, Some(1), None, None)
            .await
            .unwrap();
    assert_eq!(v.n, 1);
    let fork = ArtifactId::parse(&a.id).unwrap();
    assert!(f.content.pushed_commits(&fork, 1).await.unwrap().is_empty());
    assert!(
        f.st.reconcile_pushes_async(DAEMON, &fork, &as_person(&rae))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.st.list_versions(&fork).unwrap().len(), 1);
    let repo = f.content.host().repo(&repo_name(fork.as_str())).unwrap();
    let id = clax_git::ObjectId::from_hex(&pushed).unwrap();
    assert!(!clax_git::Repo::has_object(&repo, &id).await.unwrap());
    // The source's push is still its own to record.
    assert_eq!(f.content.pushed_commits(&f.id, 1).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_blob_at_many_paths_is_read_once_and_files_past_the_read_bytes_are_left_out() {
    let f = fixture().await;
    // A 1 MiB file, named at 40 paths: a small push that stands for 40 MiB.
    f.write("big.txt", &"a".repeat(1 << 20));
    let blob = f
        .git
        .text(&f.clone, &["hash-object", "-w", "big.txt"])
        .await;
    std::fs::remove_file(f.clone.join("big.txt")).unwrap();
    for i in 0..40 {
        f.git
            .text(
                &f.clone,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},p{i:02}.txt"),
                ],
            )
            .await;
    }
    // The index as it is: the copies are in no working tree.
    f.git
        .text(
            &f.clone,
            &[
                "-c",
                "user.name=Rae",
                "-c",
                "user.email=rae@example.com",
                "commit",
                "-qm",
                "Many copies",
            ],
        )
        .await;
    f.push().await;
    let recorded =
        f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
            .await
            .unwrap();
    assert_eq!(recorded.len(), 1);
    let v = &recorded[0].1;
    let copies = v.files.keys().filter(|p| p.starts_with('p')).count();
    assert_eq!(
        copies,
        16,
        "16 MiB of the 40 are read: {:?}",
        v.files.keys()
    );
    let note = v.note.as_deref().unwrap();
    assert!(note.starts_with("24 files were not taken"), "{note}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_whose_trees_name_too_many_paths_is_refused_unread() {
    let f = fixture().await;
    let dir = f.clone.clone();
    let blob = f
        .git
        .text_in(&dir, &["hash-object", "-w", "--stdin"], b"x")
        .await;
    // Two small trees: 120 names of a tree of 100 files, 12,000 paths.
    let leaf: String = (0..100)
        .map(|i| format!("100644 blob {blob}\tf{i}\n"))
        .collect();
    let leaf = f.git.text_in(&dir, &["mktree"], leaf.as_bytes()).await;
    let mid: String = (0..120)
        .map(|i| format!("040000 tree {leaf}\td{i}\n"))
        .collect();
    let mid = f.git.text_in(&dir, &["mktree"], mid.as_bytes()).await;
    let index = f
        .git
        .text_in(&dir, &["hash-object", "-w", "--stdin"], b"<p>many</p>")
        .await;
    let root = format!("100644 blob {index}\tindex.html\n040000 tree {mid}\tx\n");
    let root = f.git.text_in(&dir, &["mktree"], root.as_bytes()).await;
    let commit = f
        .git
        .text(
            &dir,
            &[
                "-c",
                "user.name=Rae",
                "-c",
                "user.email=rae@example.com",
                "commit-tree",
                &root,
                "-p",
                "HEAD",
                "-m",
                "Many paths",
            ],
        )
        .await;
    f.git
        .text(&dir, &["push", "-q", "origin", &format!("{commit}:main")])
        .await;

    for _ in 0..2 {
        assert!(
            f.st.reconcile_pushes_async(DAEMON, &f.id, &Pusher::default())
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(f.versions().len(), 1);
    assert_eq!(
        f.refusals(),
        [(commit, "push_too_large".to_string())],
        "named once, however often it is reconciled"
    );
    let blocked =
        f.st.publish_version_async(DAEMON, &f.id, publish("<p>mine</p>", Some(1)), None)
            .await;
    assert!(
        matches!(
            &blocked,
            Err(CoreError::Invalid {
                code: "push_refused",
                ..
            })
        ),
        "{blocked:?}"
    );
}

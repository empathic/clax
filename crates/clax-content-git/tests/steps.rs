//! A job's publish step whose process is killed after its version's content
//! is written and before its transaction commits, resumed, over both content
//! stores (the home's files and git): the resumed step replaces the version
//! its own write left and commits once, with its document writes and its
//! step; a version no step marked, or another step marked, is never
//! replaced, and a pushed commit whose message copies or guesses a mark is
//! a push.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clax_content_git::MemGitContent;
use clax_core::audit::AuditCtx;
use clax_core::content::fs::FsContent;
use clax_core::content::{
    AssetKey, ContentStore, NewFile, PushedCommit, PushedFile, Pusher, RepoToken, VersionMeta,
    VersionWrite,
};
use clax_core::db::{Caller, Level};
use clax_core::model::FileMeta;
use clax_core::publish::{PublishRequest, ValidatedPublish, validate};
use clax_core::{ArtifactId, CoreError, Home};
use clax_git::versions::repo_name;
use clax_git::{MAIN, RefUpdate, Repo};
use clax_store::Store;
use clax_store::docs::{BatchOp, BatchWrite};
use clax_store::jobs::{JobPatch, JobState, NewJob, PublishStep, StepError};
use serde_json::json;

use crate::common::{clock, committer};

const DAEMON: &AuditCtx = &AuditCtx::DAEMON;

/// A content store whose next version write, once [`Dying::kill_next`] is
/// called, writes the version and then fails as the process would stop:
/// nothing after the write runs (no transaction, no removal).
struct Dying<C> {
    inner: C,
    die: AtomicBool,
    refuse: AtomicBool,
    /// The artifact of each version write, in order.
    wrote: std::sync::Mutex<Vec<String>>,
}

impl<C> Dying<C> {
    fn kill_next(&self) {
        self.die.store(true, Ordering::SeqCst);
    }

    /// The next version write fails before it writes, as a process killed
    /// just before it would.
    fn kill_before_next(&self) {
        self.refuse.store(true, Ordering::SeqCst);
    }

    /// The artifact the last version write wrote.
    fn last_written(&self) -> Option<String> {
        self.wrote.lock().unwrap().last().cloned()
    }
}

#[async_trait::async_trait]
impl<C: ContentStore> ContentStore for Dying<C> {
    async fn read_file(
        &self,
        id: &ArtifactId,
        n: u32,
        path: &str,
    ) -> clax_core::Result<Option<Vec<u8>>> {
        self.inner.read_file(id, n, path).await
    }
    async fn write_version(
        &self,
        id: &ArtifactId,
        n: u32,
        write: &VersionWrite<'_>,
    ) -> clax_core::Result<()> {
        if self.refuse.swap(false, Ordering::SeqCst) {
            return Err(CoreError::invalid("killed", "the process was killed"));
        }
        self.inner.write_version(id, n, write).await?;
        self.wrote.lock().unwrap().push(id.as_str().to_string());
        if self.die.swap(false, Ordering::SeqCst) {
            return Err(CoreError::invalid("killed", "the process was killed"));
        }
        Ok(())
    }
    async fn copy_version(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        to_n: u32,
    ) -> clax_core::Result<()> {
        self.inner.copy_version(from, from_n, paths, to, to_n).await
    }
    async fn fork(
        &self,
        from: &ArtifactId,
        from_n: u32,
        paths: &[&str],
        to: &ArtifactId,
        meta: &VersionMeta,
    ) -> clax_core::Result<()> {
        self.inner.fork(from, from_n, paths, to, meta).await
    }
    async fn remove_version(&self, id: &ArtifactId, n: u32) -> clax_core::Result<()> {
        self.inner.remove_version(id, n).await
    }
    async fn unrecorded_step(&self, id: &ArtifactId, n: u32) -> clax_core::Result<Option<String>> {
        self.inner.unrecorded_step(id, n).await
    }
    async fn delete_artifact(&self, id: &ArtifactId) -> clax_core::Result<()> {
        self.inner.delete_artifact(id).await
    }
    async fn put_asset(&self, key: &AssetKey, bytes: &[u8]) -> clax_core::Result<()> {
        self.inner.put_asset(key, bytes).await
    }
    async fn read_asset(&self, key: &AssetKey) -> clax_core::Result<Option<Vec<u8>>> {
        self.inner.read_asset(key).await
    }
    async fn delete_asset(&self, key: &AssetKey) -> clax_core::Result<()> {
        self.inner.delete_asset(key).await
    }
    async fn put_clip(&self, id: &ArtifactId, thread: &str, bytes: &[u8]) -> clax_core::Result<()> {
        self.inner.put_clip(id, thread, bytes).await
    }
    async fn read_clip(&self, id: &ArtifactId, thread: &str) -> clax_core::Result<Option<Vec<u8>>> {
        self.inner.read_clip(id, thread).await
    }
    async fn delete_clip(&self, id: &ArtifactId, thread: &str) -> clax_core::Result<()> {
        self.inner.delete_clip(id, thread).await
    }
    async fn pushed_commits(
        &self,
        id: &ArtifactId,
        after: u32,
    ) -> clax_core::Result<Vec<PushedCommit>> {
        self.inner.pushed_commits(id, after).await
    }
    async fn pushed_files(
        &self,
        id: &ArtifactId,
        commit: &str,
        base: u32,
        published: &BTreeMap<String, FileMeta>,
    ) -> clax_core::Result<Vec<PushedFile>> {
        self.inner.pushed_files(id, commit, base, published).await
    }
    async fn tag_pushed(&self, id: &ArtifactId, n: u32, commit: &str) -> clax_core::Result<bool> {
        self.inner.tag_pushed(id, n, commit).await
    }
    async fn untag_pushed(&self, id: &ArtifactId, n: u32, commit: &str) -> clax_core::Result<()> {
        self.inner.untag_pushed(id, n, commit).await
    }
    async fn pushers(
        &self,
        id: &ArtifactId,
        commits: &[String],
    ) -> clax_core::Result<Vec<Option<Pusher>>> {
        self.inner.pushers(id, commits).await
    }
    async fn repo_tokens(&self, id: &ArtifactId) -> clax_core::Result<Vec<RepoToken>> {
        self.inner.repo_tokens(id).await
    }
    async fn revoke_repo_token(&self, id: &ArtifactId, token: &str) -> clax_core::Result<()> {
        self.inner.revoke_repo_token(id, token).await
    }
}

/// A store over `content` holding one artifact at version 1 that declares
/// documents, and a change of it, `j1`, running.
struct Fixture<C> {
    _home: tempfile::TempDir,
    st: Store,
    content: Arc<Dying<C>>,
    id: ArtifactId,
}

fn publish(html: &str, if_version: Option<u32>) -> ValidatedPublish {
    let req: PublishRequest = serde_json::from_value(json!({
        "title": "Budget",
        "if_version": if_version,
        "capabilities": {"db": {}},
        "files": {"index.html": {"content": html}},
    }))
    .unwrap();
    validate(req).unwrap()
}

fn fixture<C: ContentStore + 'static>(content: impl FnOnce(&Home) -> C) -> Fixture<C> {
    let home = tempfile::tempdir().unwrap();
    let h = Home::at(home.path().join("clax"));
    let content = Arc::new(Dying {
        inner: content(&h),
        die: AtomicBool::new(false),
        refuse: AtomicBool::new(false),
        wrote: Default::default(),
    });
    let st = Store::open(&h).unwrap().with_content(content.clone());
    let (a, _) = st
        .create_artifact(DAEMON, publish("<p>one</p>", None), None)
        .unwrap();
    let id = ArtifactId::parse(&a.id).unwrap();
    st.create_job(NewJob {
        id: "j1".into(),
        client: "app".into(),
        kind: "change".into(),
        artifact_id: Some(id.as_str().into()),
        title: None,
        input: json!({"request": "make it green"}),
        plan: None,
    })
    .unwrap();
    st.patch_job(
        "j1",
        JobPatch {
            if_rev: 1,
            state: Some(JobState::Running),
            ..JobPatch::default()
        },
    )
    .unwrap();
    Fixture {
        _home: home,
        st,
        content,
        id,
    }
}

fn the_store_on_files(home: &Home) -> FsContent {
    FsContent::new(home.clone())
}

fn the_store_in_git(_: &Home) -> MemGitContent {
    MemGitContent::in_memory(committer(), clock())
}

impl<C: ContentStore> Fixture<C> {
    /// Publish step `key` of `j1` on version 1, with one document write.
    fn step(&self, html: &str) -> PublishStep {
        PublishStep {
            artifact_id: Some(self.id.as_str().into()),
            base: 1,
            request: publish(html, None),
            docs: vec![BatchWrite {
                path: "notes/n1".into(),
                op: BatchOp::Set(json!({"t": 1})),
                if_version: None,
            }],
            draft: None,
            session_id: None,
            done: false,
        }
    }

    fn commit(
        &self,
        key: &str,
        html: &str,
    ) -> Result<clax_store::jobs::StepCommit<clax_store::jobs::Published>, StepError> {
        self.st
            .commit_publish_step(DAEMON, "j1", key, self.step(html))
    }

    fn versions(&self) -> Vec<u32> {
        self.st
            .list_versions(&self.id)
            .unwrap()
            .iter()
            .map(|v| v.n)
            .collect()
    }

    fn html(&self, n: u32) -> Option<String> {
        clax_core::content::wait(self.content.read_file(&self.id, n, "index.html"))
            .unwrap()
            .map(|b| String::from_utf8(b).unwrap())
    }

    fn doc(&self) -> Option<serde_json::Value> {
        self.st
            .doc_get(&self.id, "notes/n1", &Caller::new(Level::Owner, None))
            .unwrap()
            .map(|d| d.data)
    }

    fn steps(&self) -> usize {
        self.st.job("j1").unwrap().unwrap().steps.len()
    }

    /// Writes version 2's content as another writer would, marked `step`,
    /// with no row recording it.
    fn stray(&self, html: &str, step: Option<String>) {
        let w = VersionWrite {
            carry: Vec::new(),
            put: vec![NewFile {
                path: "index.html",
                bytes: html.as_bytes(),
                content_type: "text/html",
            }],
            meta: VersionMeta {
                step,
                ..Default::default()
            },
        };
        clax_core::content::wait(self.content.inner.write_version(&self.id, 2, &w)).unwrap();
    }
}

fn a_killed_step_resumes_once<C: ContentStore + 'static>(content: impl FnOnce(&Home) -> C) {
    let f = fixture(content);
    f.content.kill_next();
    let e = f.commit("publish:1", "<p>two</p>").unwrap_err();
    assert!(
        matches!(
            e,
            StepError::Store(CoreError::Invalid { code: "killed", .. })
        ),
        "{e:?}"
    );
    // What the kill leaves: version 2's content, no row, no step, no doc.
    assert_eq!(f.versions(), [1]);
    assert_eq!(f.html(2).as_deref(), Some("<p>two</p>"));
    assert_eq!(f.steps(), 0);
    assert_eq!(f.doc(), None);
    // The resume: one version, with its document write and its step.
    let c = f.commit("publish:1", "<p>two</p>").unwrap();
    assert!(!c.replayed);
    assert_eq!(c.step.version_n, Some(2));
    assert_eq!(c.made.unwrap().docs.len(), 1);
    assert_eq!(f.versions(), [1, 2]);
    assert_eq!(f.html(2).as_deref(), Some("<p>two</p>"));
    assert_eq!(f.doc(), Some(json!({"t": 1})));
    assert_eq!(f.steps(), 1);
    let again = f.commit("publish:1", "<p>two</p>").unwrap();
    assert!(again.replayed);
    assert_eq!(f.versions(), [1, 2]);
    // The next version follows it.
    let (_, v) =
        f.st.publish_version(DAEMON, &f.id, publish("<p>three</p>", Some(2)), None)
            .unwrap();
    assert_eq!(v.n, 3);
}

/// A make's publish step, which makes a new app, killed after version 1's
/// content is written and resumed, makes one artifact: the resume writes
/// the ID the killed attempt wrote, replacing what it left, so no content
/// or row is left under another ID.
fn a_killed_new_apps_step_resumes_as_one_artifact<C: ContentStore + 'static>(
    content: impl FnOnce(&Home) -> C,
) {
    let f = fixture(content);
    f.st.create_job(NewJob {
        id: "j2".into(),
        client: "app".into(),
        kind: "make".into(),
        artifact_id: None,
        title: None,
        input: json!({"request": "a budget"}),
        plan: None,
    })
    .unwrap();
    f.st.patch_job(
        "j2",
        JobPatch {
            if_rev: 1,
            state: Some(JobState::Running),
            ..JobPatch::default()
        },
    )
    .unwrap();
    let step = |html: &str| PublishStep {
        artifact_id: None,
        base: 0,
        request: publish(html, None),
        docs: Vec::new(),
        draft: None,
        session_id: None,
        done: true,
    };
    f.content.kill_next();
    assert!(
        f.st.commit_publish_step(DAEMON, "j2", "publish:0", step("<p>first</p>"))
            .is_err()
    );
    // What the kill leaves: version 1's content under the step's new ID.
    let killed = f.content.last_written().unwrap();
    let aid = ArtifactId::parse(&killed).unwrap();
    let html = |aid: &ArtifactId| {
        clax_core::content::wait(f.content.read_file(aid, 1, "index.html"))
            .unwrap()
            .map(|b| String::from_utf8(b).unwrap())
    };
    assert_eq!(html(&aid).as_deref(), Some("<p>first</p>"));
    let c =
        f.st.commit_publish_step(DAEMON, "j2", "publish:0", step("<p>again</p>"))
            .unwrap();
    let made = c.made.unwrap().artifact;
    assert_eq!(made.id, killed, "the resume writes the killed attempt's ID");
    assert_eq!(c.job.artifact_id.as_deref(), Some(made.id.as_str()));
    assert!(f.st.zero_version_artifacts().unwrap().is_empty());
    assert_eq!(f.st.list_artifacts().unwrap().len(), 2);
    assert_eq!(html(&aid).as_deref(), Some("<p>again</p>"));
}

/// How `j1` ends after its step was killed.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// Cancelled.
    Cancel,
    /// Done by a later step that writes no version.
    Done,
}

/// A step killed after its content write, whose job then ends, can never
/// commit: the next write of the version, whoever makes it, removes what
/// it left. `clax doctor`'s list names it, dead, until then.
fn a_dead_steps_stray_is_removed_by_the_next_write<C: ContentStore + 'static>(
    content: impl FnOnce(&Home) -> C,
    ending: Ending,
) {
    let f = fixture(content);
    f.content.kill_next();
    assert!(f.commit("publish:1", "<p>killed</p>").is_err());
    let strays = f.st.marked_strays().unwrap();
    assert_eq!(strays.len(), 1, "{ending:?}");
    assert_eq!(
        (
            strays[0].n,
            strays[0].job_id.as_str(),
            strays[0].key.as_str(),
            strays[0].live
        ),
        (2, "j1", "publish:1", true),
        "{ending:?}"
    );
    match ending {
        Ending::Cancel => {
            let rev = f.st.job("j1").unwrap().unwrap().job.rev;
            f.st.patch_job(
                "j1",
                JobPatch {
                    if_rev: rev,
                    state: Some(JobState::Cancelled),
                    ..JobPatch::default()
                },
            )
            .unwrap();
        }
        Ending::Done => {
            f.st.commit_step("j1", "reply:1", json!({}), true).unwrap();
        }
    }
    assert!(!f.st.marked_strays().unwrap()[0].live, "{ending:?}");
    let (_, v) =
        f.st.publish_version(DAEMON, &f.id, publish("<p>two</p>", Some(1)), None)
            .unwrap();
    assert_eq!(v.n, 2, "{ending:?}");
    assert_eq!(f.versions(), [1, 2], "{ending:?}");
    assert_eq!(f.html(2).as_deref(), Some("<p>two</p>"), "{ending:?}");
    assert!(f.st.marked_strays().unwrap().is_empty(), "{ending:?}");
}

/// A live step's stray refuses any other write of its version, and is
/// listed live.
fn a_live_steps_stray_refuses_another_write<C: ContentStore + 'static>(
    content: impl FnOnce(&Home) -> C,
) {
    let f = fixture(content);
    f.content.kill_next();
    assert!(f.commit("publish:1", "<p>killed</p>").is_err());
    assert!(
        f.st.publish_version(DAEMON, &f.id, publish("<p>two</p>", Some(1)), None)
            .is_err()
    );
    assert_eq!(f.versions(), [1]);
    assert_eq!(f.html(2).as_deref(), Some("<p>killed</p>"));
    assert!(f.st.marked_strays().unwrap()[0].live);
}

fn a_stray_no_step_marked_is_not_replaced<C: ContentStore + 'static>(
    content: impl FnOnce(&Home) -> C,
) {
    let f = fixture(content);
    f.stray("<p>stray</p>", None);
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    assert_eq!(f.steps(), 0);
    assert_eq!(f.doc(), None);
    assert_eq!(f.html(2).as_deref(), Some("<p>stray</p>"));
}

fn a_stray_another_step_marked_is_not_replaced<C: ContentStore + 'static>(
    content: impl FnOnce(&Home) -> C,
) {
    let f = fixture(content);
    // Another live step of the job, killed after its content write.
    f.content.kill_next();
    assert!(f.commit("publish:other", "<p>stray</p>").is_err());
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    assert_eq!(f.steps(), 0);
    assert_eq!(f.doc(), None);
    assert_eq!(f.versions(), [1], "nor taken for a push");
    assert_eq!(f.html(2).as_deref(), Some("<p>stray</p>"));
    // Its own resume replaces it.
    let c = f.commit("publish:other", "<p>other</p>").unwrap();
    assert_eq!(c.step.version_n, Some(2));
    assert_eq!(f.html(2).as_deref(), Some("<p>other</p>"));
}

/// A killed step's attempt on another base is refused, and removes the
/// stray the killed attempt left: the step writes elsewhere now, and its
/// later attempt on the first base writes that version.
fn a_step_on_another_base_removes_its_stray<C: ContentStore + 'static>(
    content: impl FnOnce(&Home) -> C,
) {
    let f = fixture(content);
    f.content.kill_next();
    assert!(f.commit("publish:1", "<p>killed</p>").is_err());
    let mut other = f.step("<p>two</p>");
    other.base = 0;
    let e =
        f.st.commit_publish_step(DAEMON, "j1", "publish:1", other)
            .unwrap_err();
    assert!(matches!(e, StepError::VersionMoved { current: 1 }), "{e:?}");
    assert_eq!(f.html(2), None);
    assert!(f.st.marked_strays().unwrap().is_empty());
    let c = f.commit("publish:1", "<p>two</p>").unwrap();
    assert_eq!(c.step.version_n, Some(2));
    assert_eq!(f.html(2).as_deref(), Some("<p>two</p>"));
}

/// Pushes a commit onto `main` of `f`'s repository, as someone with a
/// clone does: on the commit `main` is at, with its tree, and `message`.
/// Its ID.
fn push(f: &Fixture<MemGitContent>, message: &str) -> clax_git::ObjectId {
    let repo = f
        .content
        .inner
        .host()
        .repo(&repo_name(f.id.as_str()))
        .unwrap();
    let main = clax_core::content::wait(repo.refs()).unwrap()[MAIN];
    let meta = clax_core::content::wait(repo.read_commit_meta(&main))
        .unwrap()
        .unwrap();
    let sig = clax_git::Signature::utc(
        &clax_git::Person {
            name: "Rae".into(),
            email: "rae@example.com".into(),
        },
        1_760_000_000,
    );
    let pushed = clax_git::Commit {
        tree: meta.tree,
        parents: vec![main],
        author: sig.clone(),
        committer: sig,
        message: message.into(),
    }
    .to_object()
    .unwrap();
    let id = pushed.id();
    clax_core::content::wait(repo.apply(
        vec![pushed],
        &[RefUpdate::new(MAIN, Some(main), Some(id))],
        true,
    ))
    .unwrap()[0]
        .clone()
        .unwrap();
    id
}

/// Pushes a commit onto version 1, its message carrying `mark` as a
/// `Clax-Step` trailer, and tags it `v2` as the push reconcile tags a
/// commit it adopts before the transaction recording it (a process killed
/// between the two leaves it so). Its ID.
fn push_forging(f: &Fixture<MemGitContent>, mark: &str) -> clax_git::ObjectId {
    let id = push(
        f,
        &format!("pushed\n\nClax-Version: 2\nClax-Step: {mark}\n"),
    );
    assert!(clax_core::content::wait(f.content.tag_pushed(&f.id, 2, &id.to_hex())).unwrap());
    id
}

/// A commit pushed onto a step's version that its killed process left
/// unrecorded makes that version a push: the reconcile records both as
/// versions, the step's resume finds the artifact moved, and the next
/// publish follows them.
#[test]
fn a_push_built_on_a_killed_steps_version_records_it_as_a_push() {
    let f = fixture(the_store_in_git);
    f.content.kill_next();
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    let pushed = push(&f, "pushed on top\n");
    let e = f.commit("publish:1", "<p>two</p>").unwrap_err();
    assert!(matches!(e, StepError::VersionMoved { current: 3 }), "{e:?}");
    assert_eq!(f.versions(), [1, 2, 3]);
    assert_eq!(f.html(2).as_deref(), Some("<p>two</p>"));
    assert_eq!(f.html(3).as_deref(), Some("<p>two</p>"));
    assert_eq!(f.steps(), 0);
    let repo = f
        .content
        .inner
        .host()
        .repo(&repo_name(f.id.as_str()))
        .unwrap();
    assert_eq!(clax_core::content::wait(repo.refs()).unwrap()[MAIN], pushed);
    // The step plans again on version 3, and the artifact publishes on.
    let mut again = f.step("<p>four</p>");
    again.base = 3;
    let c =
        f.st.commit_publish_step(DAEMON, "j1", "publish:3", again)
            .unwrap();
    assert_eq!(c.step.version_n, Some(4));
    let (_, v) =
        f.st.publish_version(DAEMON, &f.id, publish("<p>five</p>", Some(4)), None)
            .unwrap();
    assert_eq!(v.n, 5);
}

/// A pushed commit that the reconcile tagged, whose message names a live
/// step of the job by its guessable job ID and key, is a push: it becomes
/// version 2 as the push it is, stays on main, and the step finds the
/// artifact moved.
#[test]
fn a_pushed_commit_forging_a_step_mark_is_a_push() {
    for mark in [
        r#"["j1","publish:1"]"#.to_string(),
        format!(r#"["j1","publish:1","{}"]"#, "0".repeat(32)),
    ] {
        let f = fixture(the_store_in_git);
        let pushed = push_forging(&f, &mark);
        let e = f.commit("publish:1", "<p>two</p>").unwrap_err();
        assert!(
            matches!(e, StepError::VersionMoved { current: 2 }),
            "{mark}: {e:?}"
        );
        assert_eq!(f.versions(), [1, 2], "{mark}");
        assert_eq!(f.html(2).as_deref(), Some("<p>one</p>"), "{mark}");
        let repo = f
            .content
            .inner
            .host()
            .repo(&repo_name(f.id.as_str()))
            .unwrap();
        let refs = clax_core::content::wait(repo.refs()).unwrap();
        assert_eq!(refs[MAIN], pushed, "{mark}: the push stays on main");
        assert_eq!(refs["refs/tags/v2"], pushed, "{mark}");
        assert_eq!(f.steps(), 0, "{mark}");
    }
}

/// A host that takes a push that is not a fast forward: main moves off a
/// killed step's version to a commit on version 1, which builds nothing on
/// the step's version. The step's resume removes its version, and the
/// push becomes version 2.
#[test]
fn a_push_beside_a_killed_steps_version_is_recorded_once_the_step_resumes() {
    let f = fixture(the_store_in_git);
    f.content.kill_next();
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    let repo = f
        .content
        .inner
        .host()
        .repo(&repo_name(f.id.as_str()))
        .unwrap();
    let refs = clax_core::content::wait(repo.refs()).unwrap();
    let (stray, v1) = (refs[MAIN], refs["refs/tags/v1"]);
    let meta = clax_core::content::wait(repo.read_commit_meta(&v1))
        .unwrap()
        .unwrap();
    let sig = clax_git::Signature::utc(
        &clax_git::Person {
            name: "Rae".into(),
            email: "rae@example.com".into(),
        },
        1_760_000_000,
    );
    let beside = clax_git::Commit {
        tree: meta.tree,
        parents: vec![v1],
        author: sig.clone(),
        committer: sig,
        message: "forced over the step\n".into(),
    }
    .to_object()
    .unwrap();
    let pushed = beside.id();
    clax_core::content::wait(repo.apply(
        vec![beside],
        &[RefUpdate::new(MAIN, Some(stray), Some(pushed))],
        true,
    ))
    .unwrap()[0]
        .clone()
        .unwrap();
    assert_eq!(f.st.marked_strays().unwrap().len(), 1);
    let e = f.commit("publish:1", "<p>two</p>").unwrap_err();
    assert!(matches!(e, StepError::VersionMoved { current: 2 }), "{e:?}");
    assert_eq!(f.versions(), [1, 2]);
    assert_eq!(f.html(2).as_deref(), Some("<p>one</p>"));
    let refs = clax_core::content::wait(repo.refs()).unwrap();
    assert_eq!((refs[MAIN], refs["refs/tags/v2"]), (pushed, pushed));
    assert!(f.st.marked_strays().unwrap().is_empty());
}

/// The step mark version `n` of `f`'s artifact carries, read from its
/// commit as anyone who fetches the repository reads it.
fn mark_on(f: &Fixture<MemGitContent>, n: u32) -> String {
    let repo = f
        .content
        .inner
        .host()
        .repo(&repo_name(f.id.as_str()))
        .unwrap();
    let commit = clax_core::content::wait(repo.refs()).unwrap()[&format!("refs/tags/v{n}")];
    clax_core::content::wait(repo.read_commit_meta(&commit))
        .unwrap()
        .unwrap()
        .trailer("Clax-Step")
        .unwrap()
        .to_string()
}

/// A step's mark, read from the stray its killed attempt left, is no
/// longer the step's once its resume removed that stray: a push that copies
/// it, tagged at the step's version, is recorded as the push it is and
/// stays on main, even when the resume was killed before its own write.
#[test]
fn a_mark_copied_from_a_removed_stray_is_a_push() {
    let f = fixture(the_store_in_git);
    f.content.kill_next();
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    let copied = mark_on(&f, 2);
    // The resume removes the stray and is killed before it writes.
    f.content.kill_before_next();
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    assert_eq!(f.html(2), None);
    let pushed = push_forging(&f, &copied);
    let e = f.commit("publish:1", "<p>two</p>").unwrap_err();
    assert!(matches!(e, StepError::VersionMoved { current: 2 }), "{e:?}");
    assert_eq!(f.versions(), [1, 2]);
    let repo = f
        .content
        .inner
        .host()
        .repo(&repo_name(f.id.as_str()))
        .unwrap();
    assert_eq!(clax_core::content::wait(repo.refs()).unwrap()[MAIN], pushed);
}

#[test]
fn a_killed_step_resumes_once_on_files() {
    a_killed_step_resumes_once(the_store_on_files);
}

#[test]
fn a_killed_step_resumes_once_in_git() {
    a_killed_step_resumes_once(the_store_in_git);
}

#[test]
fn a_stray_no_step_marked_is_not_replaced_on_files() {
    a_stray_no_step_marked_is_not_replaced(the_store_on_files);
}

#[test]
fn a_stray_no_step_marked_is_not_replaced_in_git() {
    a_stray_no_step_marked_is_not_replaced(the_store_in_git);
}

#[test]
fn a_stray_another_step_marked_is_not_replaced_on_files() {
    a_stray_another_step_marked_is_not_replaced(the_store_on_files);
}

#[test]
fn a_stray_another_step_marked_is_not_replaced_in_git() {
    a_stray_another_step_marked_is_not_replaced(the_store_in_git);
}

#[test]
fn a_killed_new_apps_step_resumes_as_one_artifact_on_files() {
    a_killed_new_apps_step_resumes_as_one_artifact(the_store_on_files);
}

#[test]
fn a_killed_new_apps_step_resumes_as_one_artifact_in_git() {
    a_killed_new_apps_step_resumes_as_one_artifact(the_store_in_git);
}

#[test]
fn a_cancelled_steps_stray_is_removed_by_the_next_write_on_files() {
    a_dead_steps_stray_is_removed_by_the_next_write(the_store_on_files, Ending::Cancel);
}

#[test]
fn a_cancelled_steps_stray_is_removed_by_the_next_write_in_git() {
    a_dead_steps_stray_is_removed_by_the_next_write(the_store_in_git, Ending::Cancel);
}

#[test]
fn a_done_jobs_stray_is_removed_by_the_next_write_on_files() {
    a_dead_steps_stray_is_removed_by_the_next_write(the_store_on_files, Ending::Done);
}

#[test]
fn a_done_jobs_stray_is_removed_by_the_next_write_in_git() {
    a_dead_steps_stray_is_removed_by_the_next_write(the_store_in_git, Ending::Done);
}

#[test]
fn a_live_steps_stray_refuses_another_write_on_files() {
    a_live_steps_stray_refuses_another_write(the_store_on_files);
}

#[test]
fn a_live_steps_stray_refuses_another_write_in_git() {
    a_live_steps_stray_refuses_another_write(the_store_in_git);
}

#[test]
fn a_step_on_another_base_removes_its_stray_on_files() {
    a_step_on_another_base_removes_its_stray(the_store_on_files);
}

#[test]
fn a_step_on_another_base_removes_its_stray_in_git() {
    a_step_on_another_base_removes_its_stray(the_store_in_git);
}

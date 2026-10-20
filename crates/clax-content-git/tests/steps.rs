//! A job's publish step whose process is killed after its version's content
//! is written and before its transaction commits, resumed, over both content
//! stores (the home's files and git): the resumed step replaces the version
//! its own write left and commits once, with its document writes and its
//! step; a version no step marked, or another step marked, is never
//! replaced.

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
use clax_store::Store;
use clax_store::docs::{BatchOp, BatchWrite};
use clax_store::jobs::{JobPatch, JobState, NewJob, PublishStep, StepError, step_mark};
use serde_json::json;

use crate::common::{clock, committer};

const DAEMON: &AuditCtx = &AuditCtx::DAEMON;

/// A content store whose next version write, once [`Dying::kill_next`] is
/// called, writes the version and then fails as the process would stop:
/// nothing after the write runs (no transaction, no removal).
struct Dying<C> {
    inner: C,
    die: AtomicBool,
}

impl<C> Dying<C> {
    fn kill_next(&self) {
        self.die.store(true, Ordering::SeqCst);
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
        self.inner.write_version(id, n, write).await?;
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

    fn commit(&self, key: &str, html: &str) -> Result<clax_store::jobs::StepCommit<clax_store::jobs::Published>, StepError> {
        self.st.commit_publish_step(DAEMON, "j1", key, self.step(html))
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
        matches!(e, StepError::Store(CoreError::Invalid { code: "killed", .. })),
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
    let (_, v) = f
        .st
        .publish_version(DAEMON, &f.id, publish("<p>three</p>", Some(2)), None)
        .unwrap();
    assert_eq!(v.n, 3);
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
    f.stray("<p>stray</p>", Some(step_mark("j1", "publish:other")));
    assert!(f.commit("publish:1", "<p>two</p>").is_err());
    assert_eq!(f.steps(), 0);
    assert_eq!(f.doc(), None);
    assert_eq!(f.versions(), [1], "nor taken for a push");
    assert_eq!(f.html(2).as_deref(), Some("<p>stray</p>"));
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

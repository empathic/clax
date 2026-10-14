//! A store kept in git forks an artifact into a repository holding the
//! source's history behind the fork's version 1, which names its source
//! (`Clax-Forked-From`) and its forking person (`Clax-Publisher`): on one
//! connection, as a Durable Object opens the store, and on the pool.

use std::sync::Arc;

use clax_content_git::MemGitContent;
use clax_core::audit::AuditCtx;
use clax_core::content::wait;
use clax_core::{ArtifactId, Home, Store};
use clax_git::versions::repo_name;
use clax_git::{MAIN, Repo};

use crate::common::{clock, committer};

const DAEMON: &AuditCtx = &AuditCtx::DAEMON;
const MIA: &str = "p_mia0000000000";

fn publish(st: &Store, id: Option<&ArtifactId>, html: &str) -> ArtifactId {
    let mut req = serde_json::json!({
        "title": "Quarterly Review",
        "files": {"index.html": {"content": html, "encoding": "utf8"}}
    });
    if let Some(id) = id {
        req["if_version"] = st.get_artifact(id).unwrap().unwrap().current_version.into();
    }
    let p = clax_core::publish::validate(serde_json::from_value(req).unwrap()).unwrap();
    let a = match id {
        None => st.create_artifact(DAEMON, p, None).unwrap().0,
        Some(id) => st.publish_version(DAEMON, id, p, None).unwrap().0,
    };
    ArtifactId::parse(&a.id).unwrap()
}

fn forks_with_history(st: &Store, git: &MemGitContent) {
    let src = publish(st, None, "<p>one");
    publish(st, Some(&src), "<p>two");
    publish(st, Some(&src), "<p>three");
    let (a, v) = wait(st.fork_artifact_async(DAEMON, &src, Some(2), Some(MIA), None)).unwrap();
    let fork = ArtifactId::parse(&a.id).unwrap();
    assert_eq!(
        v.note.as_deref(),
        Some(format!("Forked from {src}@v2").as_str())
    );
    let (bytes, _) = st.read_file(&fork, 1, "index.html").unwrap().unwrap();
    assert_eq!(bytes, b"<p>two");

    let host = git.host();
    let source = host.repo(&repo_name(src.as_str())).unwrap();
    let repo = host.repo(&repo_name(fork.as_str())).unwrap();
    let refs = wait(repo.refs()).unwrap();
    let tags: Vec<&String> = refs
        .keys()
        .filter(|k| k.starts_with("refs/tags/"))
        .collect();
    assert_eq!(
        tags,
        ["refs/tags/v1"],
        "the source's tags are not the fork's"
    );
    let v1 = refs["refs/tags/v1"];
    assert_eq!(refs[MAIN], v1);
    let meta = wait(repo.read_commit_meta(&v1)).unwrap().unwrap();
    let reference = format!("{src}@v2");
    assert_eq!(meta.trailer("Clax-Forked-From"), Some(reference.as_str()));
    assert_eq!(meta.trailer("Clax-Version"), Some("1"));
    assert_eq!(meta.trailer("Clax-Publisher"), Some(MIA));
    // Behind v1, the source's history up to the forked version, back to its
    // first; nothing of the source's later version.
    let source_refs = wait(source.refs()).unwrap();
    let source_main = source_refs[MAIN];
    let source_v2 = source_refs["refs/tags/v2"];
    assert_eq!(meta.parents, [source_v2]);
    assert!(!wait(repo.has_object(&source_main)).unwrap());
    let v3_tree = wait(source.read_commit_meta(&source_main))
        .unwrap()
        .unwrap()
        .tree;
    assert!(!wait(repo.has_object(&v3_tree)).unwrap());
    let mut at = Some(source_v2);
    let mut versions = Vec::new();
    while let Some(c) = at {
        let m = wait(repo.read_commit_meta(&c)).unwrap().unwrap();
        versions.push(m.trailer("Clax-Version").unwrap().to_string());
        at = m.parents.first().copied();
    }
    assert_eq!(versions, ["2", "1"]);
    // The fork goes on in its own repository.
    publish(st, Some(&fork), "<p>mine");
    let refs = wait(repo.refs()).unwrap();
    assert!(refs.contains_key("refs/tags/v2"));
    assert_eq!(wait(source.refs()).unwrap()[MAIN], source_main);
}

#[test]
fn a_store_on_one_connection_forks_with_the_source_history() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    let conn = rusqlite::Connection::open(home.db_path()).unwrap();
    conn.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA locking_mode=EXCLUSIVE;")
        .unwrap();
    let git = Arc::new(MemGitContent::in_memory(committer(), clock()));
    let st = Store::open_on(&home, conn, clock(), git.clone()).unwrap();
    forks_with_history(&st, &git);
}

#[test]
fn a_pooled_store_forks_with_the_source_history() {
    let dir = tempfile::tempdir().unwrap();
    let git = Arc::new(MemGitContent::in_memory(committer(), clock()));
    let st = Store::open(&Home::at(dir.path().join("ax")))
        .unwrap()
        .with_content(git.clone());
    forks_with_history(&st, &git);
}

/// A fork whose rows cannot be recorded leaves no repository behind: the
/// copy the content store made is deleted, and only the source's remains.
#[test]
fn a_fork_that_cannot_be_recorded_deletes_its_repository() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let git = Arc::new(MemGitContent::in_memory(committer(), clock()));
    let st = Store::open(&home).unwrap().with_content(git.clone());
    let src = publish(&st, None, "<p>one");
    let names = git.host().names();
    assert_eq!(names, [repo_name(src.as_str())]);
    // The database refuses every fork's row, after the repository is made.
    rusqlite::Connection::open(home.db_path())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_forks BEFORE INSERT ON artifacts
             WHEN NEW.forked_from_id IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'no forks here'); END;",
        )
        .unwrap();
    assert!(wait(st.fork_artifact_async(DAEMON, &src, None, Some(MIA), None)).is_err());
    assert_eq!(git.host().names(), names, "the fork's copy is gone");
    assert_eq!(st.list_artifacts().unwrap().len(), 1);
}

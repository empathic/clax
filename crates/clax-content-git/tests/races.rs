//! Writers racing over one repository, resolved by the conditional ref
//! updates: a second writer of a version, a writer whose `main` moved, and
//! a remove that would discard another writer's commit, each refused and
//! changing nothing.

use std::collections::BTreeMap;

use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::content::memory::MemContent;
use clax_core::content::{ContentStore, wait};
use clax_git::memory::MemoryHost;
use clax_git::versions::repo_name;
use clax_git::{MAIN, ObjectId, Repo};

use crate::common::{HookHost, store_on, write};

fn refs(host: &MemoryHost, id: &ArtifactId) -> BTreeMap<String, ObjectId> {
    wait(host.repo(&repo_name(id.as_str())).unwrap().refs()).unwrap()
}

fn read(c: &impl ContentStore, id: &ArtifactId, n: u32, path: &str) -> Option<Vec<u8>> {
    wait(c.read_file(id, n, path)).unwrap()
}

/// `racer` writes through the hooked host's storage directly, so its own
/// updates run no hook.
fn pair() -> (
    HookHost,
    GitContent<HookHost, MemContent>,
    GitContent<MemoryHost, MemContent>,
) {
    let host = HookHost::default();
    let store = store_on(host.clone());
    let racer = store_on(host.inner.clone());
    (host, store, racer)
}

#[test]
fn a_second_writer_of_a_version_is_refused_and_changes_nothing() {
    let (host, store, racer) = pair();
    let racer = std::sync::Arc::new(racer);
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    let (rid, r) = (id.clone(), racer.clone());
    host.before_next_apply(move || {
        wait(r.write_version(&rid, 2, &write(&["index.html"], &[("b.js", b"theirs")]))).unwrap();
    });
    let before_mine = refs(&host.inner, &id);
    assert_eq!(before_mine.len(), 2, "main and v1");
    let mine = wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"mine")])));
    assert!(mine.is_err(), "both writers of version 2 succeeded");
    assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"one");
    assert_eq!(read(&store, &id, 2, "b.js").unwrap(), b"theirs");
    let after = refs(&host.inner, &id);
    assert_eq!(after[MAIN], after["refs/tags/v2"]);
    // The loser goes on from the winner's version.
    wait(store.write_version(&id, 3, &write(&["b.js"], &[("index.html", b"three")]))).unwrap();
    assert_eq!(read(&store, &id, 3, "b.js").unwrap(), b"theirs");
}

#[test]
fn a_writer_whose_main_moved_is_refused_and_changes_nothing() {
    let (host, store, _) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    // Someone else's commit lands on main (a push), untagged.
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let v1 = refs(&host.inner, &id)["refs/tags/v1"];
    let pushed = pushed_commit(&repo, v1);
    host.before_next_apply(move || {
        wait(repo.apply(
            vec![pushed.clone()],
            &[clax_git::RefUpdate::new(MAIN, Some(v1), Some(pushed.id()))],
            true,
        ))
        .unwrap()[0]
            .clone()
            .unwrap();
    });
    let before = wait(store.write_version(&id, 2, &write(&["index.html"], &[])));
    assert!(
        before.is_err(),
        "a write discarded or skipped a pushed commit"
    );
    let now = refs(&host.inner, &id);
    assert!(!now.contains_key("refs/tags/v2"));
    assert_ne!(now[MAIN], v1, "the pushed commit stays on main");
    assert_eq!(read(&store, &id, 2, "index.html"), None);
}

/// A commit with `parent`'s tree, as a push would put on `main`.
fn pushed_commit(repo: &clax_git::memory::MemoryRepo, parent: ObjectId) -> clax_git::Object {
    let meta = wait(repo.read_commit_meta(&parent)).unwrap().unwrap();
    let sig = clax_git::Signature::utc(
        &clax_git::Person {
            name: "Rae".into(),
            email: "rae@example.com".into(),
        },
        1_760_000_000,
    );
    clax_git::Commit {
        tree: meta.tree,
        parents: vec![parent],
        author: sig.clone(),
        committer: sig,
        message: "pushed\n".into(),
    }
    .to_object()
    .unwrap()
}

#[test]
fn a_remove_never_discards_a_commit_another_writer_put_on_top() {
    let (host, store, racer) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"unrecorded")]))).unwrap();
    // Between the remove's read of the refs and its update, another writer
    // commits version 3 on top of version 2.
    let rid = id.clone();
    host.before_next_apply(move || {
        wait(racer.write_version(&rid, 3, &write(&["index.html"], &[]))).unwrap();
    });
    assert!(wait(store.remove_version(&id, 2)).is_err());
    let now = refs(&host.inner, &id);
    assert_eq!(now[MAIN], now["refs/tags/v3"]);
    assert!(now.contains_key("refs/tags/v2"));
    assert_eq!(read(&store, &id, 3, "index.html").unwrap(), b"unrecorded");
    // With main moved past it, a remove is refused before any update.
    assert!(wait(store.remove_version(&id, 2)).is_err());
    assert_eq!(refs(&host.inner, &id), now);
}

#[test]
fn a_remove_is_refused_when_main_holds_a_pushed_commit_on_top() {
    let (host, store, _) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let v1 = refs(&host.inner, &id)["refs/tags/v1"];
    let pushed = pushed_commit(&repo, v1);
    let id_pushed = pushed.id();
    wait(repo.apply(
        vec![pushed],
        &[clax_git::RefUpdate::new(MAIN, Some(v1), Some(id_pushed))],
        true,
    ))
    .unwrap();
    assert!(wait(store.remove_version(&id, 1)).is_err());
    let now = refs(&host.inner, &id);
    assert_eq!(now[MAIN], id_pushed);
    assert_eq!(now["refs/tags/v1"], v1);
}

#[test]
fn a_tag_taken_between_read_and_update_refuses_the_write() {
    let (host, store, _) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let v1 = refs(&host.inner, &id)["refs/tags/v1"];
    host.before_next_apply(move || {
        repo.force_ref("refs/tags/v2", Some(v1)).unwrap();
    });
    assert!(wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"two")]))).is_err());
    let now = refs(&host.inner, &id);
    assert_eq!(now[MAIN], v1, "main did not move");
    assert_eq!(now["refs/tags/v2"], v1);
}

#[test]
fn of_many_threads_writing_one_version_exactly_one_wins() {
    let host = MemoryHost::new();
    let stores: Vec<_> = (0..8).map(|_| store_on(host.clone())).collect();
    let id = ArtifactId::generate();
    wait(stores[0].write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    let bodies: Vec<Vec<u8>> = (0..8).map(|i| format!("writer {i}").into_bytes()).collect();
    let results: Vec<bool> = std::thread::scope(|s| {
        let handles: Vec<_> = stores
            .iter()
            .zip(&bodies)
            .map(|(store, body)| {
                let id = &id;
                s.spawn(move || {
                    wait(store.write_version(id, 2, &write(&[], &[("index.html", body)]))).is_ok()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results.iter().filter(|ok| **ok).count(), 1, "{results:?}");
    let winner = results.iter().position(|ok| *ok).unwrap();
    assert_eq!(
        read(&stores[0], &id, 2, "index.html").unwrap(),
        bodies[winner]
    );
    let now = refs(&host, &id);
    assert_eq!(now[MAIN], now["refs/tags/v2"]);
}

#[test]
fn a_remove_left_half_done_finishes_on_a_retry() {
    let (host, store, _) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"two")]))).unwrap();
    // A repository without atomic updates took main back, not the tag.
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let v1 = refs(&host.inner, &id)["refs/tags/v1"];
    repo.force_ref(MAIN, Some(v1)).unwrap();
    wait(store.remove_version(&id, 2)).unwrap();
    let now = refs(&host.inner, &id);
    assert!(!now.contains_key("refs/tags/v2"));
    assert_eq!(now[MAIN], v1);
    wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"again")]))).unwrap();
    assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"again");
}

#[test]
fn removing_a_first_version_keeps_main_and_the_next_first_write_builds_on_it() {
    let (host, store, _) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"unrecorded")]))).unwrap();
    let v1 = refs(&host.inner, &id)["refs/tags/v1"];
    wait(store.remove_version(&id, 1)).unwrap();
    // Main is not deleted (a remote may refuse deleting its current
    // branch); the version is gone for every read.
    let now = refs(&host.inner, &id);
    assert_eq!(now.keys().collect::<Vec<_>>(), [MAIN]);
    assert_eq!(now[MAIN], v1);
    assert_eq!(read(&store, &id, 1, "index.html"), None);
    // Removing it again is a no-op.
    wait(store.remove_version(&id, 1)).unwrap();
    assert_eq!(refs(&host.inner, &id), now);
    // The next first write goes on top of the untagged commit (a fast
    // forward), with its own files only.
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"again")]))).unwrap();
    let now = refs(&host.inner, &id);
    assert_eq!(now[MAIN], now["refs/tags/v1"]);
    assert_ne!(now[MAIN], v1);
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let meta = wait(repo.read_commit_meta(&now[MAIN])).unwrap().unwrap();
    assert_eq!(meta.parents, [v1]);
    assert_eq!(read(&store, &id, 1, "index.html").unwrap(), b"again");
    wait(store.write_version(&id, 2, &write(&["index.html"], &[]))).unwrap();
    assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"again");
}

#[test]
fn a_first_write_is_refused_while_main_holds_a_push_on_a_removed_first_version() {
    let (host, store, _) = pair();
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"unrecorded")]))).unwrap();
    let v1 = refs(&host.inner, &id)["refs/tags/v1"];
    wait(store.remove_version(&id, 1)).unwrap();
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let pushed = pushed_commit(&repo, v1);
    let pushed_id = pushed.id();
    wait(repo.apply(
        vec![pushed],
        &[clax_git::RefUpdate::new(MAIN, Some(v1), Some(pushed_id))],
        true,
    ))
    .unwrap();
    assert!(wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).is_err());
    let now = refs(&host.inner, &id);
    assert_eq!(now.keys().collect::<Vec<_>>(), [MAIN]);
    assert_eq!(now[MAIN], pushed_id, "the push is kept");
}

#[test]
fn a_fork_whose_update_is_refused_leaves_no_repository_and_a_retry_succeeds() {
    let (host, store, _) = pair();
    let from = ArtifactId::generate();
    wait(store.write_version(&from, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    wait(store.write_version(&from, 2, &write(&[], &[("index.html", b"two")]))).unwrap();
    let to = ArtifactId::generate();
    // Between the copy and its update, the copy's main moves.
    let (inner, name) = (host.inner.clone(), repo_name(to.as_str()));
    host.before_next_apply(move || {
        let repo = inner.repo(&name).unwrap();
        let v1 = wait(repo.refs()).unwrap()["refs/tags/v1"];
        repo.force_ref(MAIN, Some(v1)).unwrap();
    });
    assert!(wait(store.fork(&from, 2, &["index.html"], &to)).is_err());
    assert!(
        host.inner.repo(&repo_name(to.as_str())).is_none(),
        "the copy is deleted"
    );
    wait(store.fork(&from, 2, &["index.html"], &to)).unwrap();
    assert_eq!(read(&store, &to, 1, "index.html").unwrap(), b"two");
    assert_eq!(read(&store, &to, 2, "index.html"), None);
    let now = refs(&host.inner, &to);
    assert_eq!(now.len(), 2, "main and v1: {now:?}");
    assert_eq!(read(&store, &from, 2, "index.html").unwrap(), b"two");
}

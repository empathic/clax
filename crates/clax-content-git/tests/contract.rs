//! The ContentStore contract, over clax-git's in-memory host.

use clax_content_git::MemGitContent;
use clax_core::content::{contract, wait};

use crate::common::{clock, committer, store_on};

#[test]
fn the_contract_holds_over_the_memory_host() {
    let c = MemGitContent::in_memory(committer(), clock());
    wait(contract::run(&c));
}

#[test]
fn the_contract_holds_when_ref_updates_go_through_a_wrapping_host() {
    let c = store_on(crate::common::HookHost::default());
    wait(contract::run(&c));
}

/// Natively the store's futures are `Send`, so the daemon can hold it as
/// an `Arc<dyn ContentStore>` shared across threads.
#[test]
fn the_store_is_a_shareable_content_store() {
    let c: std::sync::Arc<dyn clax_core::content::ContentStore> =
        std::sync::Arc::new(MemGitContent::in_memory(committer(), clock()));
    let id = clax_core::ArtifactId::generate();
    std::thread::scope(|s| {
        s.spawn(|| {
            assert_eq!(
                wait(c.read_file(&id, 1, "index.html")).unwrap(),
                None,
                "nothing written"
            );
        });
    });
}

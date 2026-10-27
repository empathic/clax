//! `clax-fleet` against a PostgreSQL server: the schema, the coordinator,
//! the page store under the Clax store, the fence, and the content store's
//! contract. Each test needs `CLAX_TEST_PG_URL` (a database the tests may
//! fill; each test keeps to instances of its own) and is ignored without
//! it: `just fleet-pg-test` runs them.

use std::sync::Arc;
use std::time::Duration;

use clax_fleet::coord::{Coordinator, PgCoordinator};
use clax_fleet::pg::{Fence, Pool, Tls};

fn pool() -> Arc<Pool> {
    let url = std::env::var("CLAX_TEST_PG_URL")
        .expect("CLAX_TEST_PG_URL names the PostgreSQL server these tests use");
    let pool = Pool::connect(&url, &Tls::Off, 4).unwrap();
    clax_fleet::pg::migrate(&pool).unwrap();
    pool
}

/// A name no other test run uses.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", clax_core::new_ulid().to_lowercase())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn migrating_twice_and_concurrently_is_harmless() {
    let p = pool();
    let runs: Vec<_> = (0..4)
        .map(|_| {
            let p = p.clone();
            std::thread::spawn(move || clax_fleet::pg::migrate(&p))
        })
        .collect();
    for r in runs {
        r.join().unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn leases_and_members_keep_the_coordinator_rules() {
    let c = PgCoordinator::new(pool());
    let inst = unique("lease");
    let (a, b) = (unique("pod-a"), unique("pod-b"));
    assert_eq!(c.acquire(&inst, &a, "a:1").await.unwrap(), Some(1));
    assert_eq!(c.acquire(&inst, &b, "b:1").await.unwrap(), None);
    assert!(c.renew(&inst, &a, 1).await.unwrap());
    assert!(!c.renew(&inst, &b, 1).await.unwrap());
    let l = c.lease(&inst).await.unwrap().unwrap();
    assert_eq!(
        (l.holder.as_str(), l.addr.as_str(), l.epoch),
        (a.as_str(), "a:1", 1)
    );
    c.release(&inst, &a, 1).await.unwrap();
    assert!(c.lease(&inst).await.unwrap().is_none());
    assert_eq!(c.acquire(&inst, &b, "b:1").await.unwrap(), Some(2));
    // Taking it again while holding it is a new hold.
    assert_eq!(c.acquire(&inst, &b, "b:1").await.unwrap(), Some(3));
    assert!(!c.renew(&inst, &b, 2).await.unwrap());

    c.heartbeat(&a, "a:1").await.unwrap();
    c.heartbeat(&a, "a:2").await.unwrap();
    let m = c.members().await.unwrap();
    let me = m.iter().find(|m| m.pod == a).unwrap();
    assert_eq!(me.addr, "a:2");
    assert!(me.age < Duration::from_secs(5));
    c.leave(&a).await.unwrap();
    assert!(!c.members().await.unwrap().iter().any(|m| m.pod == a));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn the_clax_store_runs_on_pages_in_postgresql_and_reopens() {
    let p = pool();
    let c = PgCoordinator::new(p.clone());
    let inst = unique("store");
    let pod = unique("pod");
    let epoch = c.acquire(&inst, &pod, "x:1").await.unwrap().unwrap();
    let fence = Fence::new(&inst, &pod, epoch);
    let dir = tempfile::tempdir().unwrap();
    let home = clax_core::Home::at(dir.path().join("home"));
    let open = |fence: Arc<Fence>| {
        let p = p.clone();
        let home = home.clone();
        std::thread::spawn(move || {
            let pages: Arc<dyn clax_pagevfs::PageStore> =
                Arc::new(clax_fleet::pages::PgPages::new(p.clone(), fence.clone()));
            let (conn, reg) = clax_pagevfs::open("db", pages).unwrap().into_parts();
            let content = Arc::new(clax_fleet::content::PgContent::new(p, fence));
            let store = clax_store::Store::open_on(
                &home,
                conn,
                Arc::new(clax_core::working::SystemClock),
                content,
            )
            .unwrap();
            (store, reg)
        })
        .join()
        .unwrap()
    };
    let (store, reg) = open(fence.clone());
    assert_eq!(store.integrity_check().unwrap(), "ok");
    drop(store);
    drop(reg);
    // Opened again from the database alone.
    let (store, reg) = open(fence.clone());
    assert_eq!(store.integrity_check().unwrap(), "ok");
    drop(store);
    drop(reg);
}

/// A process that lost the lease cannot commit: its page commits and
/// content writes fail, and the new holder's succeed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn every_write_of_a_lost_lease_is_refused() {
    use clax_pagevfs::PageStore;
    let p = pool();
    let c = PgCoordinator::new(p.clone());
    let inst = unique("fence");
    let (old, new) = (unique("old"), unique("new"));
    let e1 = c.acquire(&inst, &old, "o:1").await.unwrap().unwrap();
    let stale = Fence::new(&inst, &old, e1);
    let pages = clax_fleet::pages::PgPages::new(p.clone(), stale.clone());
    let chunk =
        |b: u8| std::collections::BTreeMap::from([(0u64, vec![b; clax_pagevfs::CHUNK_SIZE])]);
    let (pages, size) = tokio::task::spawn_blocking(move || {
        pages.commit("db", 4096, &chunk(1), 1).unwrap();
        let size = pages.size("db").unwrap();
        (pages, size)
    })
    .await
    .unwrap();
    assert_eq!(size, Some(4096));

    c.release(&inst, &old, e1).await.unwrap();
    let e2 = c.acquire(&inst, &new, "n:1").await.unwrap().unwrap();
    assert!(e2 > e1);
    let fresh = clax_fleet::pages::PgPages::new(p.clone(), Fence::new(&inst, &new, e2));
    let stale_content = clax_fleet::content::PgContent::new(p.clone(), stale.clone());
    tokio::task::spawn_blocking(move || {
        let err = pages.commit("db", 4096, &chunk(2), 1).unwrap_err();
        assert!(err.contains("lease_lost"), "{err}");
        fresh.commit("db", 4096, &chunk(3), 1).unwrap();
        assert_eq!(fresh.read_chunk("db", 0).unwrap().unwrap()[0], 3);
        let id = clax_core::ArtifactId::generate();
        let w = clax_core::content::VersionWrite {
            put: vec![clax_core::content::NewFile {
                path: "index.html",
                bytes: b"x",
                content_type: "text/html",
            }],
            ..Default::default()
        };
        use clax_core::content::ContentStore;
        let err = clax_core::content::wait(stale_content.write_version(&id, 1, &w)).unwrap_err();
        assert!(err.to_string().contains("lease_lost"), "{err}");
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn the_content_store_keeps_the_content_contract() {
    let p = pool();
    let c = PgCoordinator::new(p.clone());
    let inst = unique("content");
    let pod = unique("pod");
    let epoch = c.acquire(&inst, &pod, "x:1").await.unwrap().unwrap();
    let content = clax_fleet::content::PgContent::new(p, Fence::new(&inst, &pod, epoch));
    tokio::task::spawn_blocking(move || {
        clax_core::content::wait(clax_core::content::contract::run(&content));
    })
    .await
    .unwrap();
}

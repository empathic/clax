//! The PostgreSQL backend's placement, on an in-memory coordinator: an
//! instance runs where its lease is held, goes where rendezvous hashing
//! prefers when no lease is live, and moves when a member settles in or a
//! process drains.

use std::sync::Arc;
use std::time::Duration;

use clax_fleet::coord::{self, Coordinator, MemCoordinator, Member};
use clax_fleet::fleet::Placement;

use crate::common::{Process, catalog, healthz, listener, process, write_catalog};

fn member(pod: &str) -> Member {
    Member {
        pod: pod.into(),
        addr: String::new(),
        age: Duration::ZERO,
    }
}

/// An instance name rendezvous hashing gives `want` among `a` and `b`.
fn preferring(want: &str) -> String {
    let both = [member("a"), member("b")];
    (0..)
        .map(|i| format!("inst{i}"))
        .find(|n| coord::preferred(n, &both).unwrap().pod == want)
        .unwrap()
}

async fn pair(dir: &std::path::Path, names: &[&str]) -> (Arc<MemCoordinator>, Process, Process) {
    let cat = write_catalog(
        dir,
        &catalog(&names.iter().map(|n| (*n, None)).collect::<Vec<_>>()),
    );
    let c = MemCoordinator::new();
    let leases = |c: &Arc<MemCoordinator>| Placement::Leases(c.clone() as Arc<dyn Coordinator>);
    // One data directory, as the processes share PostgreSQL: an instance
    // that moves finds its home.
    let data = dir.join("data");
    let a = process("a", listener().await, &data, &cat, None, leases(&c)).await;
    let b = process("b", listener().await, &data, &cat, None, leases(&c)).await;
    (c, a, b)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_instance_goes_where_hashing_prefers_and_moves_once_a_member_settles() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("b");
    let host = format!("{name}.test");
    let (c, a, b) = pair(dir.path(), &[&name]).await;

    // a alone: the instance is a's.
    a.fleet.tick().await;
    let (status, first) = healthz(a.addr, &host).await;
    assert_eq!(status, 200);
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));
    assert_eq!(c.lease(&name).await.unwrap().unwrap().holder, "a");

    // b joins: a's live lease keeps it on a, and b forwards to a.
    b.fleet.tick().await;
    assert_eq!(healthz(b.addr, &host).await, (200, first.clone()));
    assert!(b.fleet.running().is_empty());

    // Once b has been alive for the settling time, a hands the instance
    // over at its next rebalance; its lease is released, not expired.
    for _ in 0..6 {
        c.advance(Duration::from_secs(10));
        a.fleet.tick().await;
        b.fleet.tick().await;
    }
    assert!(a.fleet.running().is_empty(), "{:?}", a.fleet.running());
    assert!(c.lease(&name).await.unwrap().is_none());

    // The next request, at a, goes to b, which starts it under a new epoch.
    let (status, second) = healthz(a.addr, &host).await;
    assert_eq!(status, 200);
    assert_ne!(first, second);
    assert_eq!(b.fleet.running(), std::slice::from_ref(&name));
    let lease = c.lease(&name).await.unwrap().unwrap();
    assert_eq!((lease.holder.as_str(), lease.epoch), ("b", 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draining_process_hands_its_instances_to_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (c, a, b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    b.fleet.tick().await;

    // Asked at b, the instance starts on a, which hashing prefers.
    assert_eq!(healthz(b.addr, &host).await.0, 200);
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));
    assert!(a.fleet.ready());

    a.fleet.drain().await;
    assert!(!a.fleet.ready());
    assert!(a.fleet.running().is_empty());
    assert!(c.lease(&name).await.unwrap().is_none());
    // a still answers while it drains, by sending the request to b.
    assert_eq!(healthz(a.addr, &host).await.0, 200);
    assert_eq!(b.fleet.running(), std::slice::from_ref(&name));
    assert_eq!(c.lease(&name).await.unwrap().unwrap().holder, "b");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_lease_is_never_taken_and_a_request_out_of_hops_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (_c, a, b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    b.fleet.tick().await;
    assert_eq!(healthz(a.addr, &host).await.0, 200);

    let resp = reqwest::Client::new()
        .get(format!("http://{}/healthz", b.addr))
        .header("host", &host)
        .header(
            clax_fleet::proxy::HOP_HEADER,
            clax_fleet::fleet::MAX_HOPS.to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 503);
    assert!(b.fleet.running().is_empty());
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));
}

/// A process whose lease was taken over (it was cut off past the lease's
/// expiry) stops the instance at its next tick, and never renews it.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_lease_stops_the_instance() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (c, a, _b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    assert_eq!(healthz(a.addr, &host).await.0, 200);

    c.advance(coord::LEASE_TTL + Duration::from_secs(1));
    assert_eq!(c.acquire(&name, "z", "z:1").await.unwrap(), Some(2));
    a.fleet.tick().await;
    assert!(a.fleet.running().is_empty());
    assert_eq!(c.lease(&name).await.unwrap().unwrap().holder, "z");
}

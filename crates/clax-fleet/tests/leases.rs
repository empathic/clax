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
        store_schema: coord::STORE_SCHEMA,
    }
}

/// An instance name rendezvous hashing gives `want` among `a` and `b`.
fn preferring(want: &str) -> String {
    let both = [member("a"), member("b")];
    (0..)
        .map(|i| format!("inst{i}"))
        .find(|n| coord::preferred(n, 0, &both).unwrap().pod == want)
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
    let a = process("a", listener().await, &data, &cat, leases(&c)).await;
    let b = process("b", listener().await, &data, &cat, leases(&c)).await;
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
        .header(clax_fleet::proxy::INSTANCE_HEADER, &name)
        .header(clax_fleet::proxy::KEY_HEADER, crate::common::PEER_KEY)
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
    assert_eq!(
        c.acquire(&name, "z", "z:1", coord::STORE_SCHEMA)
            .await
            .unwrap(),
        Some(2)
    );
    a.fleet.tick().await;
    assert!(a.fleet.running().is_empty());
    assert_eq!(c.lease(&name).await.unwrap().unwrap().holder, "z");
}

/// A request out of hops (or one a client marked so) never starts an
/// instance away from the member hashing prefers.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_out_of_hops_never_places_an_instance_off_its_preferred_member() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (c, a, b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    b.fleet.tick().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/healthz", b.addr))
        .header("host", &host)
        .header(
            clax_fleet::proxy::HOP_HEADER,
            clax_fleet::fleet::MAX_HOPS.to_string(),
        )
        .header(clax_fleet::proxy::INSTANCE_HEADER, &name)
        .header(clax_fleet::proxy::KEY_HEADER, crate::common::PEER_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 503);
    assert!(b.fleet.running().is_empty());
    assert!(c.lease(&name).await.unwrap().is_none());
}

/// A client's hop and instance headers, without the peer key, are not
/// believed: the request is routed as any other.
#[tokio::test(flavor = "multi_thread")]
async fn a_clients_hop_headers_are_ignored_without_the_peer_key() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (_c, a, b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    b.fleet.tick().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/healthz", b.addr))
        .header("host", &host)
        .header(
            clax_fleet::proxy::HOP_HEADER,
            clax_fleet::fleet::MAX_HOPS.to_string(),
        )
        .header(clax_fleet::proxy::INSTANCE_HEADER, "someone-else")
        .header(clax_fleet::proxy::KEY_HEADER, "a guess")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));
}

/// While the database is out of reach a held instance keeps serving, and
/// the process answers not ready; once renewals have failed RENEW_FAILURES
/// ticks in a row, the instance stops.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_stops_once_its_renewals_have_failed_long_enough() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (c, a, _b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    assert_eq!(healthz(a.addr, &host).await.0, 200);
    c.fail(true);
    for i in 1..clax_fleet::fleet::RENEW_FAILURES {
        a.fleet.tick().await;
        assert!(!a.fleet.ready(), "tick {i}");
        assert_eq!(a.fleet.running(), std::slice::from_ref(&name), "tick {i}");
    }
    a.fleet.tick().await;
    assert!(a.fleet.running().is_empty());
    c.fail(false);
    a.fleet.tick().await;
    assert!(a.fleet.ready());
}

/// A paused instance answers 503 on every process, never the default
/// instance in its place, and a running one stops at the next tick.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_instance_answers_503_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let name = preferring("a");
    let host = format!("{name}.test");
    let (_c, a, b) = pair(dir.path(), &[&name]).await;
    a.fleet.tick().await;
    b.fleet.tick().await;
    assert_eq!(healthz(b.addr, &host).await.0, 200);
    let paused = catalog(&[(name.as_str(), None)]).replace(
        &format!("name = \"{name}\"\n"),
        &format!("name = \"{name}\"\npaused = true\n"),
    );
    write_catalog(dir.path(), &paused);
    a.fleet.tick().await;
    b.fleet.tick().await;
    assert!(a.fleet.running().is_empty());
    assert_eq!(healthz(a.addr, &host).await.0, 503);
    assert_eq!(healthz(b.addr, &host).await.0, 503);
}

/// A member that settles in takes its share a few instances per handover
/// tick, never all at once: their clients do not all reconnect, nor their
/// instances all start, in one tick.
#[tokio::test(flavor = "multi_thread")]
async fn a_handover_moves_a_few_instances_per_tick() {
    let dir = tempfile::tempdir().unwrap();
    let per_tick = clax_fleet::fleet::HANDOVERS_PER_TICK;
    let both = [member("a"), member("b")];
    let names: Vec<String> = (0..)
        .map(|i| format!("inst{i}"))
        .filter(|n| coord::preferred(n, 0, &both).unwrap().pod == "b")
        .take(per_tick + 2)
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (c, a, b) = pair(dir.path(), &refs).await;
    a.fleet.tick().await;
    for n in &names {
        assert_eq!(healthz(a.addr, &format!("{n}.test")).await.0, 200, "{n}");
    }
    assert_eq!(a.fleet.running().len(), names.len());
    b.fleet.tick().await;

    let mut counts = Vec::new();
    for _ in 0..12 {
        c.advance(Duration::from_secs(10));
        a.fleet.tick().await;
        b.fleet.tick().await;
        let n = a.fleet.running().len();
        if counts.last() != Some(&n) {
            counts.push(n);
        }
    }
    assert_eq!(
        counts,
        [names.len(), names.len() - per_tick, 0],
        "{counts:?}"
    );
    assert!(
        a.fleet
            .metrics()
            .contains("clax_fleet_handovers_deferred_total 2\n"),
        "{}",
        a.fleet.metrics()
    );
}

/// An instance this build opened never goes to a member of an older build,
/// which would refuse its data, even where hashing prefers that member: not
/// on a request, nor at a handover once it has settled. One a newer build
/// opened starts nowhere until a member of that build runs.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_never_goes_to_a_member_whose_build_cannot_open_it() {
    let dir = tempfile::tempdir().unwrap();
    let all = [member("a"), member("b"), member("z")];
    let name = (0..)
        .map(|i| format!("inst{i}"))
        .find(|n| {
            coord::preferred(n, 0, &all).unwrap().pod == "z"
                && coord::preferred(n, 0, &all[..2]).unwrap().pod == "a"
        })
        .unwrap();
    let newer = "newer";
    let (c, a, b) = pair(dir.path(), &[&name, newer]).await;
    // z, an older build's member, never answers at its address.
    let old = coord::STORE_SCHEMA - 1;
    c.heartbeat("z", "127.0.0.1:1", "z-run", old).await.unwrap();
    a.fleet.tick().await;
    b.fleet.tick().await;
    // This build opened the one, a newer build the other.
    for (inst, schema) in [
        (name.as_str(), coord::STORE_SCHEMA),
        (newer, coord::STORE_SCHEMA + 1),
    ] {
        let e = c.acquire(inst, "x", "x:1", schema).await.unwrap().unwrap();
        c.release(inst, "x", e).await.unwrap();
    }

    // Asked at b, it goes to a, which hashing prefers among the members
    // that can open it.
    assert_eq!(healthz(b.addr, &format!("{name}.test")).await.0, 200);
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));
    // z settles, and a keeps the instance through every handover.
    for _ in 0..6 {
        c.advance(Duration::from_secs(10));
        c.heartbeat("z", "127.0.0.1:1", "z-run", old).await.unwrap();
        a.fleet.tick().await;
        b.fleet.tick().await;
    }
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));

    assert_eq!(healthz(a.addr, &format!("{newer}.test")).await.0, 503);
    assert_eq!(healthz(b.addr, &format!("{newer}.test")).await.0, 503);
    assert_eq!(a.fleet.running(), std::slice::from_ref(&name));
    assert!(b.fleet.running().is_empty());
    assert!(c.lease(newer).await.unwrap().is_none());
}

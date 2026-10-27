//! The SQLite backend's placement: each instance on the process of its
//! shard, every request reaching it from any process.

use clax_fleet::fleet::{Peers, Placement};

use crate::common::{catalog, healthz, listener, process, write_catalog};

#[tokio::test(flavor = "multi_thread")]
async fn every_process_answers_every_instance_and_each_runs_its_own_shard() {
    let dir = tempfile::tempdir().unwrap();
    let cat = write_catalog(
        dir.path(),
        &catalog(&[("acme", Some(0)), ("globex", Some(1)), ("initech", Some(1))]),
    );
    let (l0, l1) = (listener().await, listener().await);
    let peers = vec![l0.1.to_string(), l1.1.to_string()];
    let p0 = process(
        "clax-0",
        l0,
        &dir.path().join("data-0"),
        &cat,
        Some(2),
        Placement::Shards {
            ordinal: 0,
            peers: Peers::List(peers.clone()),
        },
    )
    .await;
    let p1 = process(
        "clax-1",
        l1,
        &dir.path().join("data-1"),
        &cat,
        Some(2),
        Placement::Shards {
            ordinal: 1,
            peers: Peers::List(peers),
        },
    )
    .await;

    // Each instance answers through either process, as one instance: the
    // same start time both ways.
    for host in ["acme.test", "globex.test", "initech.test"] {
        let (s0, a) = healthz(p0.addr, host).await;
        let (s1, b) = healthz(p1.addr, host).await;
        assert_eq!((s0, s1), (200, 200), "{host}");
        assert!(a.is_some() && a == b, "{host}: {a:?} and {b:?}");
    }
    assert_eq!(p0.fleet.running(), ["acme"]);
    assert_eq!(p1.fleet.running(), ["globex", "initech"]);
    // Each home is on its own shard's disk alone.
    assert!(
        dir.path()
            .join("data-0/instances/acme/config.toml")
            .exists()
    );
    assert!(!dir.path().join("data-0/instances/globex").exists());
    assert!(
        dir.path()
            .join("data-1/instances/globex/config.toml")
            .exists()
    );

    // A host no instance answers at, with no default, is 404.
    assert_eq!(healthz(p0.addr, "nobody.test").await.0, 404);
}

/// A request that has made its hops is answered where it lands: a process
/// never forwards one again, so a misconfigured peer cannot make requests
/// circle.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_made_its_hops_is_not_forwarded_again() {
    let dir = tempfile::tempdir().unwrap();
    let cat = write_catalog(dir.path(), &catalog(&[("acme", Some(1))]));
    let l0 = listener().await;
    // Shard 1's peer is this same process: forwarding would circle.
    let self_addr = l0.1.to_string();
    let p0 = process(
        "clax-0",
        l0,
        dir.path(),
        &cat,
        Some(2),
        Placement::Shards {
            ordinal: 0,
            peers: Peers::List(vec![self_addr.clone(), self_addr]),
        },
    )
    .await;
    let (status, _) = healthz(p0.addr, "acme.test").await;
    assert_eq!(status, 503);
    assert!(p0.fleet.running().is_empty());
}

/// A changed catalog takes effect at the next tick: an instance whose config
/// changed restarts with it, and one removed stops.
#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_reloads_and_stops_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let cat = write_catalog(
        dir.path(),
        &catalog(&[("acme", Some(0)), ("globex", Some(0))]),
    );
    let l0 = listener().await;
    let peers = vec![l0.1.to_string()];
    let p0 = process(
        "clax-0",
        l0,
        dir.path(),
        &cat,
        Some(1),
        Placement::Shards {
            ordinal: 0,
            peers: Peers::List(peers),
        },
    )
    .await;
    assert_eq!(healthz(p0.addr, "acme.test").await.0, 200);
    assert_eq!(healthz(p0.addr, "globex.test").await.0, 200);
    assert_eq!(p0.fleet.running(), ["acme", "globex"]);

    let changed = catalog(&[("acme", Some(0))]).replace(
        "site_domain = \"acme-sites.test\"",
        "site_domain = \"acme-pages.test\"",
    );
    write_catalog(dir.path(), &changed);
    p0.fleet.tick().await;
    assert!(p0.fleet.running().is_empty(), "{:?}", p0.fleet.running());
    assert_eq!(healthz(p0.addr, "globex.test").await.0, 404);
    // Started again, with the new config written to its home.
    assert_eq!(healthz(p0.addr, "acme.test").await.0, 200);
    assert_eq!(p0.fleet.running(), ["acme"]);
    let config = std::fs::read_to_string(dir.path().join("instances/acme/config.toml")).unwrap();
    assert!(config.contains("acme-pages.test"), "{config}");

    // A catalog that does not parse leaves the one in effect.
    write_catalog(dir.path(), "[[instance]]\nname = 1\n");
    p0.fleet.tick().await;
    assert_eq!(p0.fleet.running(), ["acme"]);
    assert_eq!(healthz(p0.addr, "acme.test").await.0, 200);
}

/// A forward names its instance: a process whose catalog does not have it
/// yet answers 503, never its default instance in its place.
#[tokio::test(flavor = "multi_thread")]
async fn a_forward_for_an_instance_the_owner_lacks_is_refused_not_defaulted() {
    let dir = tempfile::tempdir().unwrap();
    let (d0, d1) = (dir.path().join("0"), dir.path().join("1"));
    std::fs::create_dir_all(&d0).unwrap();
    std::fs::create_dir_all(&d1).unwrap();
    // Shard 0 has acme, on shard 1; shard 1's catalog has not caught up
    // and has a default instance of its own.
    let cat0 = write_catalog(&d0, &catalog(&[("acme", Some(1)), ("other", Some(1))]));
    let behind = catalog(&[("other", Some(1))])
        .replace("name = \"other\"\n", "name = \"other\"\ndefault = true\n");
    let cat1 = write_catalog(&d1, &behind);
    let (l0, l1) = (listener().await, listener().await);
    let peers = vec![l0.1.to_string(), l1.1.to_string()];
    let shards = |ordinal| Placement::Shards {
        ordinal,
        peers: Peers::List(peers.clone()),
    };
    let p0 = process("clax-0", l0, &d0, &cat0, Some(2), shards(0)).await;
    let p1 = process("clax-1", l1, &d1, &cat1, Some(2), shards(1)).await;
    assert_eq!(healthz(p0.addr, "acme.test").await.0, 503);
    assert!(p1.fleet.running().is_empty(), "{:?}", p1.fleet.running());
}

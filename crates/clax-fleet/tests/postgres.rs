//! `clax-fleet` against a PostgreSQL server: the schema, the coordinator,
//! the page store under the Clax store, the fence, and the content store's
//! contract. Each test needs `CLAX_TEST_PG_URL` (a database the tests may
//! fill; each test keeps to instances of its own) and is ignored without
//! it: `just fleet-pg-test` runs them.

use std::sync::Arc;
use std::time::Duration;

use clax_fleet::coord::{Coordinator, PgCoordinator};
use clax_fleet::pg::{Fence, Pool, Tls};

fn url() -> String {
    std::env::var("CLAX_TEST_PG_URL")
        .expect("CLAX_TEST_PG_URL names the PostgreSQL server these tests use")
}

fn pool() -> Arc<Pool> {
    let config = clax_fleet::pg::config(&url(), &Tls::Off).unwrap();
    let pool = Pool::connect("test", config, &Tls::Off, 4).unwrap();
    clax_fleet::pg::migrate(&pool).unwrap();
    pool
}

/// Time passing on the database for the named rows alone: their expiry,
/// last heartbeat and joining move back.
struct PgTime(Arc<Pool>);

#[async_trait::async_trait]
impl clax_fleet::coord::contract::Time for PgTime {
    async fn pass(&self, d: Duration, instances: &[&str], pods: &[&str]) {
        let instances: Vec<String> = instances.iter().map(|s| s.to_string()).collect();
        let pods: Vec<String> = pods.iter().map(|s| s.to_string()).collect();
        let secs = d.as_secs_f64();
        self.0
            .call(move |c| {
                c.execute(
                    "UPDATE clax_fleet_leases SET expires = expires - make_interval(secs => $1)
                     WHERE instance = ANY($2)",
                    &[&secs, &instances],
                )
                .map_err(clax_fleet::pg::err)?;
                c.execute(
                    "UPDATE clax_fleet_members
                     SET seen = seen - make_interval(secs => $1),
                         joined = joined - make_interval(secs => $1)
                     WHERE pod = ANY($2)",
                    &[&secs, &pods],
                )
                .map_err(clax_fleet::pg::err)?;
                Ok(())
            })
            .await
            .unwrap();
    }
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
    assert_eq!(c.acquire(&inst, &a, "a:1", 1).await.unwrap(), Some(1));
    assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), None);
    assert!(c.renew(&inst, &a, 1).await.unwrap());
    assert!(!c.renew(&inst, &b, 1).await.unwrap());
    let l = c.lease(&inst).await.unwrap().unwrap();
    assert_eq!(
        (l.holder.as_str(), l.addr.as_str(), l.epoch),
        (a.as_str(), "a:1", 1)
    );
    c.release(&inst, &a, 1).await.unwrap();
    assert!(c.lease(&inst).await.unwrap().is_none());
    assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), Some(2));
    // Taking it again while holding it is a new hold.
    assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), Some(3));
    assert!(!c.renew(&inst, &b, 2).await.unwrap());

    c.heartbeat(&a, "a:1", "run", 1).await.unwrap();
    c.heartbeat(&a, "a:2", "run", 1).await.unwrap();
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
    let epoch = c.acquire(&inst, &pod, "x:1", 1).await.unwrap().unwrap();
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
    let e1 = c.acquire(&inst, &old, "o:1", 1).await.unwrap().unwrap();
    let stale = Fence::new(&inst, &old, e1);
    let pages = clax_fleet::pages::PgPages::new(p.clone(), stale.clone());
    let chunk = |b: u8, stored: u64| clax_pagevfs::Commit {
        size: 4096,
        stored_size: Some(stored),
        chunks: std::sync::Arc::new(std::collections::BTreeMap::from([(
            0u64,
            vec![b; clax_pagevfs::CHUNK_SIZE],
        )])),
        keep_below: 1,
    };
    let (pages, size) = tokio::task::spawn_blocking(move || {
        pages.commit("db", &chunk(1, 0)).unwrap();
        let size = pages.size("db").unwrap();
        (pages, size)
    })
    .await
    .unwrap();
    assert_eq!(size, Some(4096));

    c.release(&inst, &old, e1).await.unwrap();
    let e2 = c.acquire(&inst, &new, "n:1", 1).await.unwrap().unwrap();
    assert!(e2 > e1);
    let fresh = clax_fleet::pages::PgPages::new(p.clone(), Fence::new(&inst, &new, e2));
    let stale_content = clax_fleet::content::PgContent::new(p.clone(), stale.clone());
    tokio::task::spawn_blocking(move || {
        let err = pages.commit("db", &chunk(2, 4096)).unwrap_err();
        assert!(err.contains("lease_lost"), "{err}");
        fresh.commit("db", &chunk(3, 4096)).unwrap();
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
    let epoch = c.acquire(&inst, &pod, "x:1", 1).await.unwrap().unwrap();
    let content = clax_fleet::content::PgContent::new(p, Fence::new(&inst, &pod, epoch));
    tokio::task::spawn_blocking(move || {
        clax_core::content::wait(clax_core::content::contract::run(&content));
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn the_postgresql_coordinator_keeps_the_contract() {
    let p = pool();
    let c = PgCoordinator::new(p.clone());
    clax_fleet::coord::contract::run(&c, &|p| unique(p)).await;
    clax_fleet::coord::contract::expiry(&c, &PgTime(p), &|p| unique(p)).await;
}

/// A write holding its instance's fence leaves the lease row free, so the
/// pod's renewals never wait for it, while a takeover of the fence waits.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn a_write_in_flight_holds_the_fence_and_never_the_lease() {
    let c = PgCoordinator::new(pool());
    let (inst, pod) = (unique("fenced"), unique("pod"));
    let epoch = c.acquire(&inst, &pod, "x:1", 1).await.unwrap().unwrap();
    let fence = Fence::new(&inst, &pod, epoch);
    let config = clax_fleet::pg::config(&url(), &Tls::Off).unwrap();
    tokio::task::spawn_blocking(move || {
        let mut writer = config.connect(postgres::NoTls).unwrap();
        let mut other = config.connect(postgres::NoTls).unwrap();
        let mut tx = writer.transaction().unwrap();
        fence.check(&mut tx).unwrap();
        // What renew_all locks: free.
        other
            .query(
                "SELECT 1 FROM clax_fleet_leases WHERE instance = $1 FOR NO KEY UPDATE NOWAIT",
                &[&inst],
            )
            .unwrap();
        // What a takeover rewrites: held until the write ends.
        let e = other
            .query(
                "SELECT 1 FROM clax_fleet_fences WHERE instance = $1 FOR NO KEY UPDATE NOWAIT",
                &[&inst],
            )
            .unwrap_err();
        assert_eq!(
            e.code(),
            Some(&postgres::error::SqlState::LOCK_NOT_AVAILABLE),
            "{e}"
        );
        tx.commit().unwrap();
        other
            .query(
                "SELECT 1 FROM clax_fleet_fences WHERE instance = $1 FOR NO KEY UPDATE NOWAIT",
                &[&inst],
            )
            .unwrap();
    })
    .await
    .unwrap();
}

/// Every session runs with the fleet's timeouts, names its pool, and the
/// pool's metrics count its jobs.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn sessions_carry_the_timeouts_and_their_name() {
    let mut config = clax_fleet::pg::config(&url(), &Tls::Off).unwrap();
    clax_fleet::pg::name_sessions(&mut config, "clax-fleet/test/data");
    let pool = Pool::connect("data", config, &Tls::Off, 1).unwrap();
    let got: Vec<String> = pool
        .call(|c| {
            let r = c
                .query_one(
                    "SELECT current_setting('statement_timeout'),
                            current_setting('idle_in_transaction_session_timeout'),
                            current_setting('application_name')",
                    &[],
                )
                .map_err(clax_fleet::pg::err)?;
            Ok(vec![r.get(0), r.get(1), r.get(2)])
        })
        .await
        .unwrap();
    assert_eq!(got, ["2min", "20s", "clax-fleet/test/data"]);
    let text = clax_fleet::pg::metrics(&[&pool]);
    assert!(
        text.contains("clax_fleet_pg_job_wait_seconds_count{pool=\"data\"} 1"),
        "{text}"
    );
}

/// TLS on is required: a URL that turns it off is refused, and with TLS off
/// a URL that asks for it is refused too.
#[test]
fn tls_on_never_falls_back_to_plain_text() {
    let on = Tls::On { ca_file: None };
    assert!(clax_fleet::pg::config("postgresql://u@h/db?sslmode=disable", &on).is_err());
    assert!(clax_fleet::pg::config("postgresql://u@h/db?sslmode=require", &Tls::Off).is_err());
    let c = clax_fleet::pg::config("postgresql://u@h/db", &on).unwrap();
    assert_eq!(c.get_ssl_mode(), postgres::config::SslMode::Require);
    let c = clax_fleet::pg::config("postgresql://u@h/db", &Tls::Off).unwrap();
    assert_eq!(c.get_ssl_mode(), postgres::config::SslMode::Disable);
}

/// The fleet's server settings go first in the startup options and the
/// URL's own follow, so they are kept and override the fleet's; the
/// keepalives and the TCP user timeout bound a lost server, unless the
/// URL sets its own.
#[test]
fn the_urls_options_are_kept_after_the_fleets() {
    let c = clax_fleet::pg::config(
        "postgresql://u@h/db?options=-c%20search_path%3Dclax%20-c%20statement_timeout%3D5000",
        &Tls::Off,
    )
    .unwrap();
    let options = c.get_options().unwrap();
    let ours = options.find("-c statement_timeout=120000").unwrap();
    let theirs = options.find("-c statement_timeout=5000").unwrap();
    assert!(ours < theirs, "{options}");
    assert!(options.contains("-c search_path=clax"), "{options}");
    assert!(
        options.contains("-c idle_in_transaction_session_timeout=20000"),
        "{options}"
    );
    assert_eq!(
        c.get_keepalives_interval(),
        Some(clax_fleet::pg::KEEPALIVE_INTERVAL)
    );
    assert_eq!(
        c.get_keepalives_retries(),
        Some(clax_fleet::pg::KEEPALIVE_RETRIES)
    );
    assert_eq!(c.get_tcp_user_timeout(), Some(&clax_fleet::pg::LOST_SERVER));
    let c = clax_fleet::pg::config(
        "postgresql://u@h/db?tcp_user_timeout=7&keepalives_retries=9",
        &Tls::Off,
    )
    .unwrap();
    assert_eq!(c.get_tcp_user_timeout(), Some(&Duration::from_secs(7)));
    assert_eq!(c.get_keepalives_retries(), Some(9));
}

/// Sessions are named for the pool unless the URL names them.
#[test]
fn sessions_are_named_unless_the_url_names_them() {
    let mut c = clax_fleet::pg::config("postgresql://u@h/db", &Tls::Off).unwrap();
    clax_fleet::pg::name_sessions(&mut c, "clax-fleet/pod-0/data");
    assert_eq!(c.get_application_name(), Some("clax-fleet/pod-0/data"));
    let mut c =
        clax_fleet::pg::config("postgresql://u@h/db?application_name=mine", &Tls::Off).unwrap();
    clax_fleet::pg::name_sessions(&mut c, "clax-fleet/pod-0/data");
    assert_eq!(c.get_application_name(), Some("mine"));
}

/// The catalog file of one instance named `name` at `origin`.
fn one_instance(name: &str, origin: &str) -> String {
    format!(
        "[[instance]]\nname = \"{name}\"\nconfig = \"\"\"\n[hosted]\npublic_origin = \"{origin}\"\n\"\"\"\n"
    )
}

/// Two processes on PostgreSQL storage: an instance started on one starts
/// on the other from the same data, and a name given to another origin
/// does not open it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CLAX_TEST_PG_URL"]
async fn an_instance_moves_between_processes_with_its_data() {
    use clax_core::embedded::MemTree;
    use clax_fleet::fleet::{Fleet, FleetConfig, Placement, Route, Storage};
    let p = pool();
    let coord: Arc<dyn Coordinator> = Arc::new(PgCoordinator::new(p.clone()));
    let dir = tempfile::tempdir().unwrap();
    let catalog = dir.path().join("fleet.toml");
    std::fs::write(
        &catalog,
        one_instance("placeholder", "https://placeholder.test"),
    )
    .unwrap();
    let fleet = |pod: String, data: &str| {
        Fleet::new(FleetConfig {
            pod,
            advertise: "127.0.0.1:1".into(),
            data_dir: dir.path().join(data),
            catalog: catalog.clone(),
            version: "test",
            web: Arc::new(MemTree(Default::default())),
            placement: Placement::Leases(coord.clone()),
            storage: Storage::Postgres(p.clone()),
            kek: Some([3; 32]),
            peer_key: "k".into(),
            db_limits: clax_pagevfs::Limits::default(),
            port: 1,
            clock: Arc::new(clax_core::working::SystemClock),
        })
        .unwrap()
    };
    let (pa, pb) = (unique("a"), unique("b"));
    let (a, b) = (fleet(pa.clone(), "a"), fleet(pb.clone(), "b"));
    a.tick().await;
    b.tick().await;
    // A name hashing gives a, and b once a has left: other test runs may
    // be members too.
    let members = coord.members().await.unwrap();
    let without_a: Vec<_> = members.iter().filter(|m| m.pod != pa).cloned().collect();
    let name = (0..)
        .map(|i| unique(&format!("moving{i}")))
        .find(|n| {
            clax_fleet::coord::preferred(n, 0, &members).is_some_and(|m| m.pod == pa)
                && clax_fleet::coord::preferred(n, 0, &without_a).is_some_and(|m| m.pod == pb)
        })
        .unwrap();
    std::fs::write(
        &catalog,
        one_instance(&name, &format!("https://{name}.test")),
    )
    .unwrap();
    a.tick().await;
    b.tick().await;
    let inst = a.catalog().get(&name).unwrap().clone();
    assert!(matches!(a.route(&inst, 2).await, Route::Local(_)));
    a.drain().await;
    // b reads the members again, without a.
    b.tick().await;
    let inst = b.catalog().get(&name).unwrap().clone();
    assert!(matches!(b.route(&inst, 2).await, Route::Local(_)));
    b.stop(&name, "test").await;
    // The same name for another origin: its data is not opened.
    std::fs::write(
        &catalog,
        one_instance(&name, &format!("https://other-{name}.test")),
    )
    .unwrap();
    b.tick().await;
    let inst = b.catalog().get(&name).unwrap().clone();
    assert!(matches!(b.route(&inst, 2).await, Route::Unavailable(_)));
}

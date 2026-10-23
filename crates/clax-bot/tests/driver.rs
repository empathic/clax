//! The driver: runs to the end within a budget, one person's runs one at
//! a time, a stalled run waiting with backoff, a cancel stopping a run.

use crate::common::*;
use async_trait::async_trait;
use clax_bot::Driver;
use clax_bot::run::Next;
use clax_bot_sandbox::{CancelToken, Host, Limits, Sandbox, Script, ScriptOutcome};
use clax_store::bot_runs::{DriverId, RunStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A sandbox that records the most scripts it ran at once.
struct Counting {
    inner: Arc<dyn Sandbox>,
    now: AtomicUsize,
    most: AtomicUsize,
}

#[async_trait]
impl Sandbox for Counting {
    async fn run(&self, s: &Script, h: &dyn Host, l: &Limits, c: &CancelToken) -> ScriptOutcome {
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(n, Ordering::SeqCst);
        let out = self.inner.run(s, h, l, c).await;
        self.now.fetch_sub(1, Ordering::SeqCst);
        out
    }
}

#[tokio::test]
async fn the_driver_runs_one_persons_runs_one_at_a_time_to_the_end() {
    let env = Env::new().await;
    for _ in 0..2 {
        env.api
            .push(env.script_turn("tu_1", "Count", "return (await clax.files.list()).length;"));
    }
    for _ in 0..2 {
        env.api.push(finish_turn("done"));
    }
    let (a, b) = (env.run(), env.run());
    let counting = Arc::new(Counting {
        inner: env.sandbox.clone(),
        now: AtomicUsize::new(0),
        most: AtomicUsize::new(0),
    });
    let driver = Driver::new(
        DriverId::mint(),
        Arc::new(env.deps_with(counting.clone())),
        env.hub.clone(),
    );
    assert_eq!(driver.resume(true).unwrap(), 2);
    let report = tokio::time::timeout(PROMPTLY, driver.run_until_idle(Duration::from_secs(600)))
        .await
        .unwrap();
    assert_eq!(report.next_wake, None);
    for r in [&a, &b] {
        assert_eq!(env.run_row(r).status, RunStatus::Finished);
        assert!(env.run_row(r).claimed_by.is_none());
    }
    assert_eq!(counting.most.load(Ordering::SeqCst), 1);
    assert_eq!(report.ticks.len(), 8);
    assert!(matches!(report.ticks.last().unwrap().1.next, Next::Done(_)));
}

/// A sandbox that can never take a script.
struct Unavailable;

#[async_trait]
impl Sandbox for Unavailable {
    async fn run(&self, _: &Script, _: &dyn Host, _: &Limits, _: &CancelToken) -> ScriptOutcome {
        ScriptOutcome::failed(clax_bot_sandbox::ErrorKind::Unavailable, "busy")
    }
}

#[tokio::test]
async fn a_stalled_run_waits_and_a_cancel_wakes_it_to_stop() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let driver = Driver::new(
        DriverId::mint(),
        Arc::new(env.deps_with(Arc::new(Unavailable))),
        env.hub.clone(),
    );
    driver.wake(&run);
    let report = driver.run_until_idle(Duration::from_secs(600)).await;
    let wait = report.next_wake.expect("the stalled run waits");
    assert!(
        wait > Duration::ZERO && wait <= Duration::from_secs(1),
        "{wait:?}"
    );
    assert!(
        env.run_row(&run).claimed_by.is_none(),
        "released while it waits"
    );
    env.store
        .cancel_run(&clax_bot::run::bot_ctx("*owner"), &run)
        .unwrap();
    driver.cancel(&run);
    let report = driver.run_until_idle(Duration::from_secs(600)).await;
    assert_eq!(report.next_wake, None);
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::Stopped, Some("cancelled"))
    );
}

#[tokio::test]
async fn a_run_woken_while_another_streams_starts_at_once() {
    let env = Arc::new(Env::new().await);
    env.set(|s| s.per_person_concurrency = 2);
    env.api.push(Reply::StartThenHang((10, 0, 0, 0)));
    let a = env.run();
    let driver = Arc::new(Driver::new(
        DriverId::mint(),
        Arc::new(env.deps()),
        env.hub.clone(),
    ));
    driver.wake(&a);
    let task = {
        let driver = driver.clone();
        tokio::spawn(async move { driver.run_until_idle(Duration::from_secs(600)).await })
    };
    tokio::time::timeout(PROMPTLY, env.api.arrived.notified())
        .await
        .expect("run a streams");
    env.api.push(text_turn("hello"));
    env.api.push(text_turn("still"));
    let b = env.run();
    driver.wake(&b);
    tokio::time::timeout(PROMPTLY, env.api.arrived.notified())
        .await
        .expect("run b starts while a streams");
    assert_eq!(env.run_row(&a).status, RunStatus::Running);
    env.store
        .cancel_run(&clax_bot::run::bot_ctx("*owner"), &a)
        .unwrap();
    driver.cancel(&a);
    let report = tokio::time::timeout(PROMPTLY, task).await.unwrap().unwrap();
    assert_eq!(report.next_wake, None);
    assert_eq!(env.run_row(&a).status, RunStatus::Stopped);
    assert_eq!(env.run_row(&b).status, RunStatus::GaveUp);
}

#[tokio::test]
async fn a_driver_starting_sweeps_artifacts_a_crash_left_without_a_version() {
    let env = Env::new().await;
    // Rows a crash left between an artifact's row and its first version,
    // written as such a crash leaves them, and a live page, which has no
    // version until its first snapshot.
    let db = clax_core::Home::at(env._dir.path().join("ax")).db_path();
    let open = || {
        let c = rusqlite::Connection::open(&db).unwrap();
        c.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        c
    };
    let orphan = |id: &str, kind: &str| {
        let made = clax_core::time::stamp(env.clock_now());
        open()
            .execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version,
                    contract_version, kind) VALUES (?1, 'x', ?2, ?2, 0, '1', ?3)",
                rusqlite::params![id, made, kind],
            )
            .unwrap();
    };
    orphan("zzzzzzzzzzzz", "html");
    orphan("llllllllllll", "live");
    // Older than a claim's lease by the time a driver starts; the next is
    // young, a create that may still be under way.
    env.clock.advance(3600);
    orphan("yyyyyyyyyyyy", "html");
    let driver = Driver::new(DriverId::mint(), Arc::new(env.deps()), env.hub.clone());
    driver.start(true).await.unwrap();
    assert_eq!(
        env.store.zero_version_artifacts().unwrap(),
        ["llllllllllll", "yyyyyyyyyyyy"]
    );
    // The run's artifact, which has a version, is kept.
    assert!(
        env.store
            .get_artifact(&clax_core::ArtifactId::parse(&env.aid).unwrap())
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_wait_outlives_its_driver_and_the_next_driver_honours_it() {
    let env = Env::new().await;
    env.api.push(env.script_turn("tu_1", "Edit", "return 1;"));
    let run = env.run();
    let a = Driver::new(
        DriverId::mint(),
        Arc::new(env.deps_with(Arc::new(Unavailable))),
        env.hub.clone(),
    );
    a.wake(&run);
    let report = a.run_until_idle(Duration::from_secs(600)).await;
    assert!(report.next_wake.is_some(), "the stalled run waits");
    assert!(env.run_row(&run).due_at.is_some(), "its wait is recorded");

    // The driver goes (a restart, an eviction); the next one waits it out.
    drop(a);
    let b = Driver::new(
        DriverId::mint(),
        Arc::new(env.deps_with(Arc::new(Unavailable))),
        env.hub.clone(),
    );
    assert_eq!(b.resume(true).unwrap(), 1);
    let report = b.run_until_idle(Duration::from_secs(600)).await;
    assert!(
        report.ticks.is_empty(),
        "not ticked early: {:?}",
        report.ticks
    );
    let wait = report.next_wake.expect("still waiting");
    assert!(
        wait > Duration::ZERO && wait <= Duration::from_secs(1),
        "{wait:?}"
    );

    // Once due, it is ticked, and its new wait recorded.
    env.clock.advance(2);
    let report = b.run_until_idle(Duration::from_secs(600)).await;
    assert_eq!(report.ticks.len(), 1, "{:?}", report.ticks);
    assert!(env.run_row(&run).due_at.is_some());

    // A cancel is not held back by a recorded wait, and running clears it.
    env.store
        .cancel_run(&clax_bot::run::bot_ctx("*owner"), &run)
        .unwrap();
    let c = Driver::new(
        DriverId::mint(),
        Arc::new(env.deps_with(Arc::new(Unavailable))),
        env.hub.clone(),
    );
    c.resume(true).unwrap();
    let report = c.run_until_idle(Duration::from_secs(600)).await;
    assert_eq!(report.next_wake, None);
    let r = env.run_row(&run);
    assert_eq!(r.status, RunStatus::Stopped);
    assert!(r.due_at.is_none(), "cleared once it ran");
}

#[tokio::test]
async fn a_failed_sweep_does_not_stop_a_driver_resuming_its_runs() {
    let env = Env::new().await;
    let run = env.run();
    // The sweep's query fails (the column it reads is renamed for the
    // moment); the run is still queued.
    let db = clax_core::Home::at(env._dir.path().join("ax")).db_path();
    let c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    c.execute_batch("ALTER TABLE artifacts RENAME COLUMN kind TO kind_gone")
        .unwrap();
    let driver = Driver::new(DriverId::mint(), Arc::new(env.deps()), env.hub.clone());
    let queued = driver.start(true).await.unwrap();
    c.execute_batch("ALTER TABLE artifacts RENAME COLUMN kind_gone TO kind")
        .unwrap();
    assert_eq!(queued, 1, "{run}");
}

//! Jobs: a key joins, a poll is held, a run's end stops what it has running.

use crate::fixture::{FLOOD, HOLD, QUEUE, World, job, run};
use clax_bot::workspace::{CODE_SHELL_UNAVAILABLE, MAX_OUTPUT_BYTES, Offer, Proposal, Workspace};
use clax_bot_remote::lapsed_offer;
use clax_bot_remote::wire::job_path;
use clax_bot_workspace::ledger::{Entry, Ledger};
use clax_bot_workspace_service::Limits;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn a_repeated_key_joins_a_running_job_and_runs_it_once() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    let first = h.remote.exec_keyed(&r, None, "r1:1:0", HOLD, None, 60);
    let again = h.remote.exec_keyed(&r, None, "r1:1:0", HOLD, None, 60);
    let release = async {
        w.runtime.entered.notified().await;
        w.runtime.release();
    };
    let (first, again, ()) = tokio::join!(first, again, release);
    assert_eq!(first.unwrap()["output"], "held");
    assert_eq!(again.unwrap()["output"], "held");
    assert_eq!(w.runtime.ran(HOLD), 1);
}

#[tokio::test(start_paused = true)]
async fn a_repeated_key_after_its_job_ended_answers_as_it_did() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    let first = h.remote.exec_keyed(&r, None, "r1:1:0", "date", None, 60);
    let first = first.await.unwrap();
    let again = h.remote.exec_keyed(&r, None, "r1:1:0", "date", None, 60);
    assert_eq!(again.await.unwrap(), first);
    let joined = h.remote.resume_exec(&r, "r1:1:0").await.unwrap();
    assert_eq!(joined, Some(first));
    assert_eq!(w.runtime.ran("date"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_poll_is_held_for_its_wait_and_no_longer_than_the_most() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let (status, _) = h
        .call("POST", "/v1/jobs", Some(job("k1", "r1", HOLD)))
        .await;
    assert_eq!(status, 200);
    w.runtime.entered.notified().await;

    let at = Instant::now();
    let (status, answer) = h.call("GET", "/v1/jobs/k1?wait=3", None).await;
    assert_eq!((status, &answer["state"]), (200, &json!("running")));
    assert_eq!(at.elapsed().as_secs(), 3);

    let at = Instant::now();
    h.call("GET", "/v1/jobs/k1?wait=3600", None).await;
    assert_eq!(at.elapsed().as_secs(), 20);

    let at = Instant::now();
    h.call("GET", "/v1/jobs/k1", None).await;
    assert_eq!(at.elapsed().as_secs(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_held_poll_answers_as_soon_as_its_job_ends() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.call("POST", "/v1/jobs", Some(job("k1", "r1", HOLD)))
        .await;
    w.runtime.entered.notified().await;
    let at = Instant::now();
    let release = async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        w.runtime.release();
    };
    let path = job_path("k1");
    let ((status, answer), ()) = tokio::join!(h.call("GET", &path, None), release);
    assert_eq!((status, &answer["state"]), (200, &json!("done")));
    assert_eq!(at.elapsed().as_secs(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_host_waiting_on_a_long_command_asks_a_few_times_and_does_not_spin() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    let call = h.remote.exec_keyed(&r, None, "r1:1:0", HOLD, None, 600);
    let release = async {
        tokio::time::sleep(Duration::from_secs(100)).await;
        w.runtime.release();
    };
    let (done, ()) = tokio::join!(call, release);
    assert_eq!(done.unwrap()["output"], "held");
    // The post, and a poll for each 20 seconds of the hundred.
    assert!(h.sent() <= 7, "{} requests", h.sent());
}

#[tokio::test(start_paused = true)]
async fn a_key_nobody_began_is_gone_and_starts_nothing() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let (status, _) = h.call("GET", "/v1/jobs/r1:9:9", None).await;
    assert_eq!(status, 404);
    let joined = h.remote.resume_exec(&run("r1"), "r1:9:9").await;
    assert_eq!(joined.unwrap(), None);
    assert_eq!(w.runtime.made(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_jobs_answer_goes_with_its_run() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote
        .exec_keyed(&r, None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    h.remote.finish(&r).await;
    assert_eq!(h.remote.resume_exec(&r, "r1:1:0").await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn a_restart_leaves_a_job_gone_and_the_container_to_be_taken_back() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote
        .exec_keyed(&r, None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();

    let service = w.service();
    let h = w.host(&service);
    assert_eq!(h.remote.resume_exec(&r, "r1:1:0").await.unwrap(), None);
    h.remote
        .exec_keyed(&r, None, "r1:2:0", "more", None, 60)
        .await
        .unwrap();
    assert_eq!(w.runtime.made(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_run_whose_container_is_gone_is_told_it_was_lost() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote
        .exec_keyed(&r, None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    w.runtime.lose_all();

    let service = w.service();
    let h = w.host(&service);
    let e = h.remote.exec_keyed(&r, None, "r1:2:0", "more", None, 60);
    let e = e.await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.message.contains("lost"), "{}", e.message);
    assert_eq!(w.runtime.made(), 1);
}

#[tokio::test(start_paused = true)]
async fn stop_aborts_a_running_job_and_ends_the_container_without_waiting_for_it() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.call("POST", "/v1/jobs", Some(job("r1:1:0", "r1", HOLD)))
        .await;
    w.runtime.entered.notified().await;

    let at = Instant::now();
    h.remote.finish(&r).await;
    assert_eq!(at.elapsed().as_secs(), 0);
    assert_eq!(w.runtime.cut(), 1);
    assert_eq!(w.runtime.live(), 0);
    assert_eq!(h.remote.resume_exec(&r, "r1:1:0").await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn a_poll_held_on_a_job_stop_aborts_answers_at_once() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.call("POST", "/v1/jobs", Some(job("r1:1:0", "r1", HOLD)))
        .await;
    w.runtime.entered.notified().await;
    let at = Instant::now();
    let stop = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        h.remote.finish(&run("r1")).await;
    };
    let path = job_path("r1:1:0");
    let ((status, answer), ()) = tokio::join!(h.call("GET", &path, None), stop);
    assert_eq!((status, &answer["state"]), (200, &json!("failed")));
    assert_eq!(answer["code"], "shell_interrupted");
    assert_eq!(at.elapsed().as_secs(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_request_that_arrives_after_its_run_ended_makes_nothing() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote.finish(&r).await;
    let late = h.remote.exec_keyed(&r, None, "r1:1:0", "date", None, 60);
    assert_eq!(late.await.unwrap_err().code, CODE_SHELL_UNAVAILABLE);
    assert_eq!(w.runtime.made(), 0);
}

#[tokio::test(start_paused = true)]
async fn ending_a_run_twice_does_no_harm() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote.finish(&r).await;
    let (status, _) = h.call("DELETE", "/v1/runs/r1", None).await;
    assert_eq!(status, 200);
}

#[tokio::test(start_paused = true)]
async fn a_key_of_another_run_is_refused_and_reads_nothing_of_it() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.remote
        .exec_keyed(&run("r1"), None, "shared", "secret", None, 60)
        .await
        .unwrap();
    let (status, answer) = h
        .call("POST", "/v1/jobs", Some(job("shared", "r2", "x")))
        .await;
    assert_eq!(status, 409);
    assert!(answer["value"].is_null(), "{answer}");
    assert_eq!(w.runtime.ran("x"), 0);
}

#[tokio::test(start_paused = true)]
async fn what_a_command_answers_is_clipped_for_a_script() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let done = h
        .remote
        .exec_keyed(&run("r1"), None, "r1:1:0", FLOOD, None, 60)
        .await
        .unwrap();
    let output = done["output"].as_str().unwrap();
    assert!(output.len() < MAX_OUTPUT_BYTES + 100, "{}", output.len());
    assert!(output.contains("bytes cut"));
}

#[tokio::test(start_paused = true)]
async fn a_reap_ends_a_run_this_process_holds_and_leaves_the_active() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let active = run("r2");
    h.remote
        .exec_keyed(&active, None, "r2:1:0", "date", None, 60)
        .await
        .unwrap();
    h.call("POST", "/v1/jobs", Some(job("r1:1:0", "r1", HOLD)))
        .await;
    w.runtime.entered.notified().await;

    h.remote.reap(&["r2".to_string()]).await;

    assert_eq!(w.runtime.cut(), 1);
    assert_eq!(w.runtime.live(), 1);
    assert_eq!(
        h.remote.resume_exec(&run("r1"), "r1:1:0").await.unwrap(),
        None
    );
    let kept = h.remote.resume_exec(&active, "r2:1:0").await.unwrap();
    assert_eq!(kept.unwrap()["output"], "ran date");
}

#[tokio::test(start_paused = true)]
async fn what_a_host_offers_is_what_the_workspace_says() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let (fresh, ws) = (run("fresh"), w.workspace());
    assert_eq!(h.remote.offer(&fresh, Some(QUEUE)), lapsed_offer());

    h.remote.reap(&[]).await;

    let on_queue = ws.offer(&fresh, Some(QUEUE));
    assert!(
        on_queue.change && on_queue.reviewed_previews,
        "{on_queue:?}"
    );
    assert_eq!(h.remote.offer(&fresh, Some(QUEUE)), on_queue);
    assert_eq!(h.remote.offer(&fresh, None), Offer::SHELL);
    assert_eq!(h.remote.offer(&fresh, Some("other")), Offer::SHELL);
}

#[tokio::test(start_paused = true)]
async fn a_run_keeps_the_offer_it_began_with_and_loses_it_when_it_ends() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote
        .exec_keyed(&r, None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    let (_, answer) = h.call("GET", "/v1/jobs/r1:1:0", None).await;
    assert_eq!(answer["snapshot"]["runs"]["r1"]["shell"], true);

    h.remote.finish(&r).await;
    assert_eq!(service.snapshot().runs.get("r1"), None);
}

/// Real `git`, which paused time would hurry past, so nothing here waits on
/// the clock.
#[tokio::test]
async fn a_repeated_proposal_joins_and_pushes_once() {
    let w = World::with_repository();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    let edit = "echo two >> app.txt";
    let done = h
        .remote
        .exec_keyed(&r, Some(QUEUE), "r1:1:0", edit, None, 60)
        .await
        .unwrap();
    assert_eq!(done["exit_code"], 0, "{done}");
    let proposal = Proposal {
        title: "Add two".to_string(),
        summary: "It lacked two.".to_string(),
        key: "r1:2:0".to_string(),
    };
    let first = h.remote.propose(&r, &proposal).await.unwrap();
    let again = h.remote.propose(&r, &proposal).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(first["url"], "https://forge.example/pr/1");
    assert!(first["verify_url"].is_null(), "{first}");
    assert_eq!(w.forge.opened.lock().unwrap().len(), 1);
    assert!(service.snapshot().runs["r1"].change);
}

#[tokio::test(start_paused = true)]
async fn a_restart_between_two_sends_of_one_post_runs_the_command_once() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let body = job("r1:1:0", "r1", "date");
    h.call("POST", "/v1/jobs", Some(body.clone())).await;
    h.call("GET", "/v1/jobs/r1:1:0?wait=20", None).await;
    assert_eq!(w.runtime.ran("date"), 1);

    let service = w.service();
    let h = w.host(&service);
    let (status, answer) = h.call("POST", "/v1/jobs", Some(body)).await;
    assert_eq!((status, &answer["state"]), (200, &json!("failed")));
    assert_eq!(answer["code"], "shell_interrupted");
    tokio::task::yield_now().await;
    assert_eq!(w.runtime.ran("date"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_key_begun_before_a_restart_is_still_another_runs() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.call("POST", "/v1/jobs", Some(job("shared", "r1", "date")))
        .await;
    let service = w.service();
    let h = w.host(&service);
    let (status, _) = h
        .call("POST", "/v1/jobs", Some(job("shared", "r2", "date")))
        .await;
    assert_eq!(status, 409);
}

#[tokio::test(start_paused = true)]
async fn a_torn_line_in_the_begun_keys_loses_only_itself() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let body = job("r1:1:0", "r1", "date");
    h.call("POST", "/v1/jobs", Some(body.clone())).await;
    let path = w.root.path().join("workspace-jobs.log");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"key\":\"r1:2");
    std::fs::write(&path, text).unwrap();

    let service = w.service();
    let h = w.host(&service);
    let (_, answer) = h.call("POST", "/v1/jobs", Some(body)).await;
    assert_eq!(answer["code"], "shell_interrupted");
}

#[tokio::test(start_paused = true)]
async fn a_runs_end_forgets_its_begun_keys() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote
        .exec_keyed(&r, None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    h.remote.finish(&r).await;
    let kept = std::fs::read_to_string(w.root.path().join("workspace-jobs.log")).unwrap();
    assert!(!kept.contains("r1:1:0"), "{kept}");
}

#[tokio::test(start_paused = true)]
async fn a_reap_after_a_restart_forgets_the_keys_of_runs_that_are_over() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.remote
        .exec_keyed(&run("r1"), None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    let service = w.service();
    let h = w.host(&service);
    h.remote.reap(&[]).await;
    let kept = std::fs::read_to_string(w.root.path().join("workspace-jobs.log")).unwrap();
    assert!(!kept.contains("r1:1:0"), "{kept}");
}

#[tokio::test(start_paused = true)]
async fn a_command_past_a_targets_limit_is_refused_and_may_ask_again() {
    let mut w = World::new();
    w.limits = Limits {
        running_per_target: 1,
        ..Limits::default()
    };
    let service = w.service();
    let h = w.host(&service);
    h.call("POST", "/v1/jobs", Some(job("r1:1:0", "r1", HOLD)))
        .await;
    w.runtime.entered.notified().await;

    let (status, answer) = h
        .call("POST", "/v1/jobs", Some(job("r2:1:0", "r2", "date")))
        .await;
    assert_eq!((status, &answer["state"]), (200, &json!("failed")));
    assert_eq!(answer["code"], CODE_SHELL_UNAVAILABLE);
    assert_eq!(answer["retryable"], true);
    let (status, _) = h.call("GET", "/v1/jobs/r2:1:0", None).await;
    assert_eq!(status, 404);

    let mut elsewhere = job("r3:1:0", "r3", "date");
    elsewhere["queue"] = json!("elsewhere");
    let (_, answer) = h.call("POST", "/v1/jobs", Some(elsewhere)).await;
    assert_eq!(answer["state"], "running");

    w.runtime.release();
    h.call("GET", "/v1/jobs/r1:1:0?wait=20", None).await;
    let (_, answer) = h
        .call("POST", "/v1/jobs", Some(job("r2:1:0", "r2", "date")))
        .await;
    assert_eq!(answer["state"], "running");
}

#[tokio::test(start_paused = true)]
async fn a_run_holds_only_its_latest_answers() {
    let mut w = World::new();
    w.limits = Limits {
        answers_per_run: 2,
        ..Limits::default()
    };
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    for i in 0..4 {
        h.remote
            .exec_keyed(&r, None, &format!("r1:{i}:0"), "date", None, 60)
            .await
            .unwrap();
    }
    assert_eq!(h.remote.resume_exec(&r, "r1:0:0").await.unwrap(), None);
    assert!(h.remote.resume_exec(&r, "r1:3:0").await.unwrap().is_some());
    assert!(h.remote.resume_exec(&r, "r1:2:0").await.unwrap().is_some());
    // What went is still known as begun: it runs no command again.
    let lost = h
        .remote
        .exec_keyed(&r, None, "r1:0:0", "date", None, 60)
        .await;
    assert_eq!(lost.unwrap_err().code, "shell_interrupted");
    assert_eq!(w.runtime.ran("date"), 4);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_is_read_again_only_when_something_changed() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    assert_eq!(service.snapshot().runs.len(), 0);
    let entry = Entry {
        offer: Some(Offer::SHELL),
        ..Entry::default()
    };
    Ledger::at(w.ledger_dir()).write("written", &entry).unwrap();
    assert_eq!(service.snapshot().runs.len(), 0);

    h.remote
        .exec_keyed(&run("r1"), None, "r1:1:0", "date", None, 60)
        .await
        .unwrap();
    assert_eq!(service.snapshot().runs.len(), 2);
}

/// A run with a change, its proposal stopped at opening the pull request.
async fn proposing(w: &World, h: &crate::fixture::Host) {
    let r = run("r1");
    w.forge.gated.store(true, Ordering::SeqCst);
    let edit = "echo two >> app.txt";
    h.remote
        .exec_keyed(&r, Some(QUEUE), "r1:1:0", edit, None, 60)
        .await
        .unwrap();
    let body = json!({
        "key": "r1:2:0",
        "run": {"id": "r1", "thread_ids": ["thread1"]},
        "kind": "propose",
        "title": "Add two",
        "summary": "It lacked two.",
    });
    let (status, _) = h.call("POST", "/v1/jobs", Some(body)).await;
    assert_eq!(status, 200);
    w.forge.opening.notified().await;
    // Real `git` is over: from here time is the test's.
    tokio::time::pause();
}

#[tokio::test]
async fn stop_lets_a_proposal_under_way_open_its_pull_request_then_answers() {
    let w = World::with_repository();
    let service = w.service();
    let h = w.host(&service);
    proposing(&w, &h).await;

    let at = Instant::now();
    let release = async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(w.runtime.live(), 1);
        w.forge.release();
    };
    let ((status, _), ()) = tokio::join!(h.call("DELETE", "/v1/runs/r1", None), release);
    assert_eq!(status, 200);
    assert_eq!(at.elapsed().as_secs(), 3);
    assert_eq!(w.forge.opened(), 1);
    assert_eq!(w.runtime.live(), 0);
}

#[tokio::test]
async fn stop_answers_within_its_bound_and_the_proposal_still_finishes() {
    let w = World::with_repository();
    let service = w.service();
    let h = w.host(&service);
    proposing(&w, &h).await;

    let at = Instant::now();
    let (status, _) = h.call("DELETE", "/v1/runs/r1", None).await;
    assert_eq!(status, 200);
    assert_eq!(at.elapsed().as_secs(), 25);
    assert_eq!(w.runtime.live(), 1);

    w.forge.release();
    while w.runtime.live() > 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(w.forge.opened(), 1);
}

#[tokio::test]
async fn a_proposal_stuck_past_the_bound_is_aborted_and_the_run_ended() {
    let mut w = World::with_repository();
    w.limits = Limits {
        proposal_settle: Duration::from_secs(10),
        ..Limits::default()
    };
    let service = w.service();
    let h = w.host(&service);
    proposing(&w, &h).await;

    let at = Instant::now();
    let (status, _) = h.call("DELETE", "/v1/runs/r1", None).await;
    assert_eq!(status, 200);
    assert_eq!(at.elapsed().as_secs(), 10);
    assert_eq!(w.forge.opened(), 0);
    assert_eq!(w.runtime.live(), 0);
}

#[tokio::test]
async fn stop_aborts_a_proposal_that_has_not_reached_its_push() {
    let w = World::with_repository();
    let service = w.service();
    let h = w.host(&service);
    let edit = "echo two >> app.txt";
    h.remote
        .exec_keyed(&run("r1"), Some(QUEUE), "r1:1:0", edit, None, 60)
        .await
        .unwrap();
    w.runtime.patches_held.store(true, Ordering::SeqCst);
    let body = json!({
        "key": "r1:2:0",
        "run": {"id": "r1", "thread_ids": ["thread1"]},
        "kind": "propose",
        "title": "Add two",
        "summary": "It lacked two.",
    });
    let (status, _) = h.call("POST", "/v1/jobs", Some(body)).await;
    assert_eq!(status, 200);
    w.runtime.entered.notified().await;
    tokio::time::pause();

    let at = Instant::now();
    let (status, _) = h.call("DELETE", "/v1/runs/r1", None).await;
    assert_eq!(status, 200);
    assert_eq!(at.elapsed().as_secs(), 0, "it waits for no push");
    assert_eq!(w.runtime.cut(), 1);
    assert_eq!(w.runtime.live(), 0);
    assert_eq!(w.forge.opened(), 0);
}

#[tokio::test(start_paused = true)]
async fn stop_never_waits_for_a_command() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    h.call("POST", "/v1/jobs", Some(job("r1:1:0", "r1", HOLD)))
        .await;
    w.runtime.entered.notified().await;
    let at = Instant::now();
    let (status, _) = h.call("DELETE", "/v1/runs/r1", None).await;
    assert_eq!(status, 200);
    assert_eq!(at.elapsed().as_secs(), 0);
}

#[tokio::test]
async fn a_proposal_that_failed_retryably_may_be_sent_again_under_its_key() {
    let w = World::with_repository();
    let service = w.service();
    let h = w.host(&service);
    let r = run("r1");
    h.remote
        .exec_keyed(&r, Some(QUEUE), "r1:1:0", "echo two >> app.txt", None, 60)
        .await
        .unwrap();
    let proposal = Proposal {
        title: "Add two".to_string(),
        summary: "It lacked two.".to_string(),
        key: "r1:2:0".to_string(),
    };
    w.runtime.patches_fail.store(true, Ordering::SeqCst);
    let failed = h.remote.propose(&r, &proposal).await.unwrap_err();
    assert!(failed.retryable, "{failed:?}");

    w.runtime.patches_fail.store(false, Ordering::SeqCst);
    let done = h.remote.propose(&r, &proposal).await.unwrap();
    assert_eq!(done["url"], "https://forge.example/pr/1");
    assert_eq!(w.forge.opened(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_later_snapshot_never_has_a_lesser_number_across_a_restart() {
    let w = World::new();
    let service = w.service();
    w.now_ms.store(5_000, Ordering::SeqCst);
    let first = service.snapshot().seq;
    w.now_ms.store(4_000, Ordering::SeqCst);
    let second = service.snapshot().seq;
    assert!(second >= first, "{second} after {first}");

    w.now_ms.store(9_000, Ordering::SeqCst);
    let service = w.service();
    assert!(service.snapshot().seq >= 9_000);
}

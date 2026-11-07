//! What became of a preview: on disk until the host says it told it.

use crate::fixture::{QUEUE, World, run};
use clax_bot::workspace::{PreviewSink, Proposal, Workspace};
use clax_bot_remote::wire::{MAX_PREVIEW_EVENTS, PreviewTold};
use clax_bot_workspace::ledger::{Awaiting, Entry, Ledger};
use clax_bot_workspace_service::Service;
use clax_bot_workspace_service::jobs::PREVIEW_GRACE_MS;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const LINK: &str = "https://pr-1.preview.example";

/// Until `service` holds an event: a task of the workspace's tells it.
async fn told(service: &Arc<Service>) {
    while service.previews().is_empty() {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_word_reaches_the_host_once_and_is_forgotten_when_acknowledged() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    service
        .previews()
        .preview_ready("r1", LINK, true)
        .await
        .unwrap();
    service.previews().preview_missed("r2").await.unwrap();

    assert_eq!(h.remote.poll_previews().await.unwrap(), 2);
    assert_eq!(
        *h.sink.told.lock().unwrap(),
        [format!("r1 ready {LINK} true"), "r2 missed".to_string()]
    );
    assert!(service.previews().is_empty());
    assert_eq!(h.remote.poll_previews().await.unwrap(), 0);
}

#[tokio::test]
async fn a_word_not_told_is_delivered_again_after_a_restart_and_not_after_its_ack() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    service
        .previews()
        .preview_ready("r1", LINK, false)
        .await
        .unwrap();
    *h.sink.down.lock().unwrap() = true;
    assert_eq!(h.remote.poll_previews().await.unwrap(), 0);
    assert_eq!(service.previews().events().len(), 1);

    let service = w.service();
    let h = w.host(&service);
    assert_eq!(h.remote.poll_previews().await.unwrap(), 1);
    assert_eq!(h.sink.told.lock().unwrap().len(), 1);

    let service = w.service();
    let h = w.host(&service);
    assert_eq!(h.remote.poll_previews().await.unwrap(), 0);
    assert!(service.previews().is_empty());
}

#[tokio::test]
async fn a_word_acknowledged_before_a_restart_leaves_its_id_used() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    service.previews().preview_missed("r1").await.unwrap();
    let first = service.previews().events()[0].id.clone();
    h.remote.poll_previews().await.unwrap();

    let service = w.service();
    service.previews().preview_missed("r2").await.unwrap();
    assert_ne!(service.previews().events()[0].id, first);
}

#[tokio::test]
async fn the_same_word_twice_is_kept_once_until_acknowledged() {
    let w = World::new();
    let service = w.service();
    service.previews().preview_missed("r1").await.unwrap();
    service.previews().preview_missed("r1").await.unwrap();
    assert_eq!(service.previews().events().len(), 1);
}

#[tokio::test]
async fn an_acknowledgement_of_nothing_known_is_no_failure() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    let ids = serde_json::json!({"ids": ["p99"]});
    let (status, _) = h.call("POST", "/v1/previews/ack", Some(ids)).await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn a_told_file_that_cannot_be_read_stops_the_service_rather_than_losing_it() {
    let w = World::new();
    std::fs::write(w.root.path().join("workspace-previews.json"), "{").unwrap();
    let opened = Service::open(w.workspace(), w.root.path());
    assert!(opened.is_err());
}

#[tokio::test]
async fn a_wait_past_its_end_at_a_restart_is_told_as_missed() {
    let w = World::new();
    let entry = Entry {
        awaiting: Some(Awaiting {
            pr: 1,
            commit: "abc".to_string(),
            until: 1,
        }),
        ..Entry::default()
    };
    Ledger::at(w.ledger_dir()).write("r1", &entry).unwrap();
    let service = w.service();
    told(&service).await;
    let events = service.previews().events();
    assert_eq!(events[0].run, "r1");
    assert_eq!(events[0].told, PreviewTold::Missed);
}

#[tokio::test]
async fn a_host_is_told_to_poll_past_a_wait_and_while_a_word_is_unheard() {
    let w = World::new();
    let service = w.service();
    assert_eq!(service.snapshot().previews_until_ms, None);

    service.previews().preview_missed("r1").await.unwrap();
    w.now_ms.store(5_000, Ordering::SeqCst);
    let unheard = service.snapshot().previews_until_ms;
    assert_eq!(unheard, Some(5_000 + PREVIEW_GRACE_MS));

    let until = 4_000_000_000;
    let entry = Entry {
        awaiting: Some(Awaiting {
            pr: 1,
            commit: "abc".to_string(),
            until,
        }),
        finished: true,
        ..Entry::default()
    };
    Ledger::at(w.ledger_dir()).write("r2", &entry).unwrap();
    let service = w.service();
    let waited = service.snapshot().previews_until_ms;
    assert_eq!(waited, Some(4_000_000_000_000 + PREVIEW_GRACE_MS));
}

/// Real `git`: nothing here waits on the clock.
#[tokio::test]
async fn a_proposal_whose_preview_never_comes_tells_the_host_after_its_run_ended() {
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
    h.remote.propose(&r, &proposal).await.unwrap();
    h.remote.finish(&r).await;
    told(&service).await;
    assert_eq!(h.remote.poll_previews().await.unwrap(), 1);
    assert_eq!(*h.sink.told.lock().unwrap(), ["r1 missed".to_string()]);
}

#[tokio::test]
async fn a_word_the_disk_will_not_take_is_kept_and_saved_when_it_will() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    *h.sink.down.lock().unwrap() = true;
    w.block_saves();
    let told = service.previews().preview_ready("r1", LINK, true).await;
    assert!(told.is_ok());
    assert_eq!(service.previews().events().len(), 1);
    assert_eq!(h.remote.poll_previews().await.unwrap(), 0);

    w.unblock_saves();
    assert_eq!(h.remote.poll_previews().await.unwrap(), 0);
    let service = w.service();
    assert_eq!(service.previews().events().len(), 1);
}

#[tokio::test]
async fn a_word_the_disk_will_not_take_still_reaches_the_host() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    w.block_saves();
    service.previews().preview_missed("r1").await.unwrap();
    assert_eq!(h.remote.poll_previews().await.unwrap_or(0), 0);
    assert_eq!(*h.sink.told.lock().unwrap(), ["r1 missed".to_string()]);

    w.unblock_saves();
    assert_eq!(h.remote.poll_previews().await.unwrap(), 1);
    assert!(service.previews().is_empty());
}

#[tokio::test]
async fn a_failed_acknowledgement_keeps_every_word_in_its_order() {
    let w = World::new();
    let service = w.service();
    for run in ["r1", "r2", "r3"] {
        service.previews().preview_missed(run).await.unwrap();
    }
    let ids: Vec<String> = service
        .previews()
        .events()
        .iter()
        .map(|e| e.id.clone())
        .collect();
    w.block_saves();
    assert!(service.previews().acknowledge(&ids[..1]).is_err());
    let runs: Vec<String> = service
        .previews()
        .events()
        .iter()
        .map(|e| e.run.clone())
        .collect();
    assert_eq!(runs, ["r1", "r2", "r3"]);

    w.unblock_saves();
    service.previews().acknowledge(&ids[..1]).unwrap();
    let runs: Vec<String> = service
        .previews()
        .events()
        .iter()
        .map(|e| e.run.clone())
        .collect();
    assert_eq!(runs, ["r2", "r3"]);
}

#[tokio::test]
async fn a_poll_answers_the_oldest_words_and_no_more_than_the_most() {
    let w = World::new();
    let service = w.service();
    let h = w.host(&service);
    for i in 0..MAX_PREVIEW_EVENTS + 5 {
        service
            .previews()
            .preview_missed(&format!("r{i}"))
            .await
            .unwrap();
    }
    let (status, answer) = h.call("GET", "/v1/previews", None).await;
    assert_eq!(status, 200);
    let events = answer["events"].as_array().unwrap();
    assert_eq!(events.len(), MAX_PREVIEW_EVENTS);
    assert_eq!(events[0]["run"], "r0");
}

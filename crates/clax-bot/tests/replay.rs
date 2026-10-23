//! A run's live events and their replay after a reconnect or a restart.

use crate::common::*;
use clax_bot::events::{DONE_SEQ, EventHub, EventId, RunEvent, Subscription};
use clax_store::bot_runs::DriverId;
use clax_store::roles::OWNER;

async fn all(mut sub: Subscription) -> Vec<RunEvent> {
    let mut out = Vec::new();
    while let Some(e) = tokio::time::timeout(PROMPTLY, sub.next())
        .await
        .expect("the run's events end")
        .expect("not lagged")
    {
        out.push(e);
    }
    out
}

fn strictly_increasing(events: &[RunEvent]) {
    for w in events.windows(2) {
        assert!(w[0].id < w[1].id, "{} then {}", w[0].id, w[1].id);
    }
}

#[tokio::test]
async fn live_events_then_replays_from_the_store_and_from_an_id() {
    let env = Env::new().await;
    env.api
        .push(env.script_turn("tu_1", "Edit", "await clax.files.list(); return 1;"));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let live = env.hub.subscribe(&env.store, &run, None).unwrap();
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    let live = all(live).await;
    strictly_increasing(&live);
    let names: Vec<&str> = live.iter().map(|e| e.event).collect();
    assert_eq!(names.first(), Some(&"step_started"));
    assert_eq!(names.last(), Some(&"run_end"));
    assert!(names.contains(&"turn_delta"));
    assert!(names.contains(&"script") && names.contains(&"call"));
    assert_eq!(names.iter().filter(|n| **n == "step_done").count(), 4);
    let first_delta = live.iter().find(|e| e.event == "turn_delta").unwrap();
    assert_eq!(first_delta.id.to_string(), "1.1:1");
    assert_eq!(first_delta.to_sse().lines().next(), Some("id: 1.1:1"));

    // After a restart: rebuilt from committed steps, final content only.
    let fresh = EventHub::new();
    let replay = all(fresh.subscribe(&env.store, &run, None).unwrap()).await;
    strictly_increasing(&replay);
    assert!(replay.iter().all(|e| e.event != "turn_delta"));
    assert_eq!(replay.last().unwrap().event, "run_end");
    let done = replay.iter().find(|e| e.event == "step_done").unwrap();
    assert_eq!(done.id.seq, DONE_SEQ);
    assert_eq!(done.data["summary"]["text"], "Let me look.");
    assert_eq!(done.data["summary"]["tool"]["name"], "run_script");
    // A script's calls replay with the IDs they had live.
    let live_calls: Vec<EventId> = live
        .iter()
        .filter(|e| e.event == "call")
        .map(|e| e.id)
        .collect();
    let replay_calls: Vec<EventId> = replay
        .iter()
        .filter(|e| e.event == "call")
        .map(|e| e.id)
        .collect();
    assert_eq!(live_calls, replay_calls);
    // Each step's end has one ID live and replayed.
    let ends = |es: &[RunEvent]| -> Vec<EventId> {
        es.iter()
            .filter(|e| e.event == "step_done" || e.event == "run_end")
            .map(|e| e.id)
            .collect()
    };
    assert_eq!(ends(&live), ends(&replay));

    // A client that saw part of step 1 live gets that step's end next.
    let partial = fresh
        .subscribe(&env.store, &run, Some(first_delta.id))
        .unwrap();
    let rest = all(partial).await;
    assert_eq!(rest[0].event, "step_done");
    assert_eq!(rest[0].id.to_string(), format!("1.1:{DONE_SEQ}"));
    assert_eq!(rest[1].id.to_string(), "2.1:0");
    // One that saw the run end gets nothing more.
    let end = replay.last().unwrap().id;
    assert!(
        all(fresh.subscribe(&env.store, &run, Some(end)).unwrap())
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn the_hub_keeps_no_channel_for_an_ended_run() {
    let env = Env::new().await;
    env.api.push(finish_turn("done"));
    let run = env.run();
    let live = env.hub.subscribe(&env.store, &run, None).unwrap();
    assert_eq!(env.hub.channels(), 1);
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(all(live).await.last().unwrap().event, "run_end");
    // An ended run's channel goes once no one listens, and replaying an
    // ended run makes none.
    for _ in 0..20 {
        let sub = env.hub.subscribe(&env.store, &run, None).unwrap();
        assert_eq!(all(sub).await.last().unwrap().event, "run_end");
    }
    assert_eq!(env.hub.channels(), 0);
    let _ = OWNER;
}

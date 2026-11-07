//! The wire as bytes: a service in another build reads these shapes.

use clax_bot::workspace::Offer;
use clax_bot_remote::wire::{
    JobAnswer, JobRequest, JobState, OfferForm, PreviewEvent, PreviewTold, RunRef, Snapshot,
    WIRE_VERSION, WireFailure, Work, job_path, run_path,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn snapshot() -> Snapshot {
    Snapshot {
        seq: 1,
        runs: BTreeMap::new(),
        queues: BTreeMap::new(),
        others: Offer::default(),
        private_runs: BTreeSet::new(),
        private_queues: BTreeSet::new(),
        previews_until_ms: None,
    }
}

fn run() -> RunRef {
    RunRef {
        id: "R1".into(),
        thread_ids: vec!["T1".into()],
    }
}

#[test]
fn a_command_job_reads_as_the_wire_says() {
    let job = JobRequest {
        key: "R1:2:0".into(),
        run: run(),
        work: Work::Exec {
            queue: Some("Q1".into()),
            command: "make".into(),
            cwd: None,
            timeout_secs: 120,
        },
    };
    let v = json!({
        "key": "R1:2:0",
        "run": {"id": "R1", "thread_ids": ["T1"]},
        "kind": "exec",
        "queue": "Q1",
        "command": "make",
        "cwd": null,
        "timeout_secs": 120,
    });
    assert_eq!(serde_json::to_value(&job).unwrap(), v);
    assert_eq!(serde_json::from_value::<JobRequest>(v).unwrap(), job);
}

#[test]
fn a_proposal_job_reads_as_the_wire_says() {
    let job = JobRequest {
        key: "R1:2:1".into(),
        run: run(),
        work: Work::Propose {
            title: "t".into(),
            summary: "s".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&job).unwrap(),
        json!({
            "key": "R1:2:1",
            "run": {"id": "R1", "thread_ids": ["T1"]},
            "kind": "propose",
            "title": "t",
            "summary": "s",
        })
    );
}

#[test]
fn a_jobs_answers_read_as_the_wire_says() {
    let of = |state| JobAnswer {
        state,
        snapshot: snapshot(),
    };
    let v = serde_json::to_value(of(JobState::Running)).unwrap();
    assert_eq!(v["state"], "running");
    let v = serde_json::to_value(of(JobState::Done {
        value: json!({"exit_code": 0}),
    }))
    .unwrap();
    assert_eq!(
        (&v["state"], &v["value"]["exit_code"]),
        (&json!("done"), &json!(0))
    );
    let failed = json!({"state": "failed", "code": "c", "message": "m",
                          "snapshot": serde_json::to_value(snapshot()).unwrap()});
    assert_eq!(
        serde_json::from_value::<JobAnswer>(failed).unwrap().state,
        JobState::Failed {
            code: "c".into(),
            message: "m".into(),
            retryable: false,
        }
    );
}

#[test]
fn a_preview_event_reads_as_the_wire_says() {
    let e = PreviewEvent {
        id: "E1".into(),
        run: "R1".into(),
        told: PreviewTold::Ready {
            verify_url: "https://pr-1.example".into(),
            reviewed: true,
        },
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({"id": "E1", "run": "R1", "told": "ready",
               "verify_url": "https://pr-1.example", "reviewed": true})
    );
    let missed = json!({"id": "E2", "run": "R1", "told": "missed"});
    assert_eq!(
        serde_json::from_value::<PreviewEvent>(missed).unwrap().told,
        PreviewTold::Missed
    );
}

#[test]
fn a_key_and_a_run_are_one_segment_of_a_path() {
    assert_eq!(job_path("R1:2/0"), "/v1/jobs/R1%3A2%2F0?wait=20");
    assert_eq!(run_path("../x"), "/v1/runs/..%2Fx");
}

#[test]
fn a_run_keeps_its_offer_before_its_queues_and_others_get_the_rest() {
    let s = Snapshot {
        runs: BTreeMap::from([("R1".to_string(), Offer::CHANGE.confidential())]),
        queues: BTreeMap::from([("Q1".to_string(), Offer::CHANGE)]),
        others: Offer::SHELL,
        ..snapshot()
    };
    assert_eq!(s.offer("R1", Some("Q1")), Offer::CHANGE.confidential());
    assert_eq!(s.offer("R2", Some("Q1")), Offer::CHANGE);
    assert_eq!(s.offer("R2", Some("Q2")), Offer::SHELL);
    assert_eq!(s.offer("R2", None), Offer::SHELL);
}

#[test]
fn a_targets_words_cross_the_wire_in_its_offer() {
    let told = Offer {
        guidance: Some("Say how.".into()),
        preview_gate: Some("press `{commit}`".into()),
        ..Offer::CHANGE
    };
    let snapshot = Snapshot {
        seq: 1,
        runs: BTreeMap::new(),
        queues: BTreeMap::from([("Q1".to_string(), told.clone())]),
        others: Offer::default(),
        private_runs: BTreeSet::new(),
        private_queues: BTreeSet::new(),
        previews_until_ms: None,
    };
    let v = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(v["queues"]["Q1"]["guidance"], "Say how.");
    assert_eq!(v["queues"]["Q1"]["preview_gate"], "press `{commit}`");
    let back: Snapshot = serde_json::from_value(v).unwrap();
    assert_eq!(back.offer("R1", Some("Q1")), told);
}

#[test]
fn a_run_made_from_the_wire_has_what_a_service_reads() {
    let r = run().to_run();
    assert_eq!(r.id, "R1");
    assert_eq!(r.thread_ids, ["T1"]);
    assert_eq!(RunRef::of(&r), run());
}

#[test]
fn a_skew_says_which_version_the_service_speaks() {
    let f = WireFailure::skew();
    assert_eq!(f.wire, Some(1));
    assert_eq!(f.code, "wire_skew");
}

/// An offer's fields on the wire, by the version that has them. A field added
/// to [`Offer`] takes a new version and a new entry here, and an entry already
/// released never changes.
const OFFER_FIELDS: &[(u32, &str)] = &[(
    1,
    r#"{"answers_only":true,"change":true,"confidential":true,"guidance":"g","no_follow_ups":true,"preview_gate":"p","reviewed_previews":true,"shell":true,"unsure":true,"wake_on_reply":true}"#,
)];

fn every_field() -> Offer {
    Offer {
        shell: true,
        change: true,
        confidential: true,
        reviewed_previews: true,
        wake_on_reply: true,
        no_follow_ups: true,
        answers_only: true,
        unsure: true,
        guidance: Some("g".into()),
        preview_gate: Some("p".into()),
    }
}

#[test]
fn an_offers_wire_form_is_the_one_this_version_says() {
    let (version, golden) = OFFER_FIELDS.last().unwrap();
    assert_eq!(*version, WIRE_VERSION, "add an entry for this version");
    let v = serde_json::to_value(OfferForm::from(every_field())).unwrap();
    let golden: serde_json::Value = serde_json::from_str(golden).unwrap();
    assert_eq!(v, golden, "a new field takes a new WIRE_VERSION");
    let form: OfferForm = serde_json::from_value(golden).unwrap();
    assert_eq!(Offer::from(form), every_field());
}

fn golden_without(field: &str) -> serde_json::Value {
    let mut v: serde_json::Value = serde_json::from_str(OFFER_FIELDS.last().unwrap().1).unwrap();
    v.as_object_mut().unwrap().remove(field);
    v
}

#[test]
fn an_offer_missing_any_field_but_confidentiality_is_refused() {
    let golden: serde_json::Value = serde_json::from_str(OFFER_FIELDS.last().unwrap().1).unwrap();
    for field in golden.as_object().unwrap().keys() {
        if field != "confidential" {
            let read = serde_json::from_value::<OfferForm>(golden_without(field));
            assert!(read.is_err(), "{field}");
        }
    }
}

#[test]
fn an_offer_with_no_word_on_confidentiality_is_confidential() {
    let v = golden_without("confidential");
    let form: OfferForm = serde_json::from_value(v).unwrap();
    assert!(form.confidential);
}

#[test]
fn an_offer_with_a_field_this_version_does_not_know_is_refused() {
    let mut v = golden_without("shell");
    v["shell"] = json!(true);
    v["new_right"] = json!(true);
    assert!(serde_json::from_value::<OfferForm>(v).is_err());
}

#[test]
fn a_snapshot_missing_a_field_is_refused() {
    let whole = serde_json::to_value(snapshot()).unwrap();
    for field in whole.as_object().unwrap().keys() {
        let mut v = whole.clone();
        v.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Snapshot>(v).is_err(), "{field}");
    }
    assert_eq!(
        serde_json::from_value::<Snapshot>(whole).unwrap(),
        snapshot()
    );
}

//! What [`RemoteWorkspace`] asks a service, and what it makes of the answers.

use clax_bot::Workspace;
use clax_bot::run::OffersLapse;
use clax_bot::workspace::{
    CODE_CHANGE_REFUSED, CODE_FIX_COMING, CODE_SHELL_INTERRUPTED, CODE_SHELL_UNAVAILABLE, Offer,
    PreviewSink, Proposal,
};
use clax_bot_remote::wire::{
    JobRequest, MAX_PREVIEW_EVENTS, MAX_RESPONSE_BYTES, PreviewEvent, PreviewTold, PreviewsAnswer,
    RunRef, Snapshot, WIRE_HEADER, WIRE_INSTANCE_HEADER, WIRE_VERSION, WireFailure,
};
use clax_bot_remote::{
    Keeper, Kept, LinkHosts, MAX_PREVIEW_TRIES, MAX_SNAPSHOT_AGE_MS, Method, RemoteWorkspace,
    Transport, Unreachable, WireRequest, WireResponse, lapsed_offer,
};
use clax_bot_sandbox::HostError;
use clax_core::working::ManualClock;
use clax_store::bot_runs::Run;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the next request gets.
enum Next {
    Answer(u16, Vec<u8>),
    Down,
    /// Never answers.
    Hang,
}

/// A service that answers from a script, and keeps what it was asked. With
/// nothing left to answer it is out of reach.
#[derive(Default)]
struct Service {
    next: Mutex<VecDeque<Next>>,
    seen: Mutex<Vec<WireRequest>>,
}

impl Service {
    fn says(&self, status: u16, body: Value) -> &Service {
        self.next
            .lock()
            .unwrap()
            .push_back(Next::Answer(status, body.to_string().into_bytes()));
        self
    }

    fn raw(&self, status: u16, body: &[u8]) -> &Service {
        self.next
            .lock()
            .unwrap()
            .push_back(Next::Answer(status, body.to_vec()));
        self
    }

    fn down(&self, times: usize) -> &Service {
        for _ in 0..times {
            self.next.lock().unwrap().push_back(Next::Down);
        }
        self
    }

    fn lines(&self) -> Vec<(Method, String)> {
        let seen = self.seen.lock().unwrap();
        seen.iter().map(|r| (r.method, r.path.clone())).collect()
    }

    fn body(&self, i: usize) -> Value {
        let seen = self.seen.lock().unwrap();
        serde_json::from_slice(seen[i].body.as_ref().unwrap()).unwrap()
    }
}

#[async_trait::async_trait]
impl Transport for Service {
    async fn send(&self, request: WireRequest) -> Result<WireResponse, Unreachable> {
        self.seen.lock().unwrap().push(request);
        let next = self.next.lock().unwrap().pop_front();
        match next {
            Some(Next::Answer(status, body)) => Ok(WireResponse { status, body }),
            Some(Next::Hang) => std::future::pending().await,
            Some(Next::Down) | None => Err(Unreachable),
        }
    }
}

#[derive(Default)]
struct Saved(Mutex<Vec<Kept>>);

impl Saved {
    fn snapshots(&self) -> usize {
        let saved = self.0.lock().unwrap();
        saved.iter().filter(|k| k.snapshot.is_some()).count()
    }
    fn last(&self) -> Kept {
        self.0.lock().unwrap().last().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Keeper for Saved {
    async fn save(&self, kept: &Kept) {
        self.0.lock().unwrap().push(kept.clone());
    }
}

#[derive(Default)]
struct Sink {
    told: Mutex<Vec<String>>,
    lost: Mutex<Vec<String>>,
    /// Runs telling of which fails with this code.
    fails: Mutex<BTreeMap<String, String>>,
}

impl Sink {
    fn tell(&self, run: &str, what: String) -> Result<(), HostError> {
        if let Some(code) = self.fails.lock().unwrap().get(run) {
            return Err(HostError::new(code.clone(), "no"));
        }
        self.told.lock().unwrap().push(what);
        Ok(())
    }
}

#[async_trait::async_trait]
impl PreviewSink for Sink {
    async fn preview_ready(&self, run: &str, url: &str, reviewed: bool) -> Result<(), HostError> {
        self.tell(run, format!("{run} ready {url} {reviewed}"))
    }
    async fn preview_missed(&self, run: &str) -> Result<(), HostError> {
        self.tell(run, format!("{run} missed"))
    }
    async fn preview_lost(&self, run: &str) -> Result<(), HostError> {
        self.lost.lock().unwrap().push(run.to_string());
        Ok(())
    }
}

const INSTANCE: &str = "I1";

struct Fixture {
    service: Arc<Service>,
    keeper: Arc<Saved>,
    clock: Arc<ManualClock>,
    remote: RemoteWorkspace,
}

fn fixture_from(kept: Option<Kept>) -> Fixture {
    let service = Arc::new(Service::default());
    let keeper = Arc::new(Saved::default());
    let clock = Arc::new(ManualClock::at("2026-11-06T12:00:00Z"));
    let remote = RemoteWorkspace::new(service.clone(), keeper.clone(), kept, INSTANCE)
        .with_preview_hosts(LinkHosts::parse("preview.example, *.pages.example"))
        .with_forge_hosts(LinkHosts::parse("forge.example"))
        .with_clock(clock.clone());
    Fixture {
        service,
        keeper,
        clock,
        remote,
    }
}

fn fixture() -> Fixture {
    fixture_from(None)
}

impl Fixture {
    fn clock_ms(&self) -> i64 {
        use clax_core::working::Clock;
        self.clock.now().timestamp_millis()
    }
}

fn run() -> Run {
    RunRef {
        id: "R1".into(),
        thread_ids: vec!["T1".into()],
    }
    .to_run()
}

fn offers() -> Snapshot {
    Snapshot {
        seq: 5,
        runs: BTreeMap::from([("R1".to_string(), Offer::SHELL)]),
        queues: BTreeMap::from([("Q1".to_string(), Offer::CHANGE)]),
        others: Offer::default(),
        private_runs: BTreeSet::new(),
        private_queues: BTreeSet::new(),
        previews_until_ms: None,
    }
}

fn wire(snapshot: &Snapshot) -> Value {
    serde_json::to_value(snapshot).unwrap()
}

fn running() -> Value {
    json!({"state": "running", "snapshot": wire(&offers())})
}

fn done(value: Value) -> Value {
    json!({"state": "done", "value": value, "snapshot": wire(&offers())})
}

fn no_such_job() -> Value {
    json!({"code": "no_such_job", "message": "no such job, begun or ended"})
}

fn proposal() -> Proposal {
    Proposal {
        title: "t".into(),
        summary: "s".into(),
        key: "R1:2:0".into(),
    }
}

async fn make(f: &Fixture) -> Result<Value, HostError> {
    f.remote
        .exec_keyed(&run(), Some("Q1"), "R1:2:0", "make", Some("/w"), 120)
        .await
}

const POLL: &str = "/v1/jobs/R1%3A2%3A0?wait=20";

#[tokio::test(start_paused = true)]
async fn a_command_is_posted_under_its_key_and_polled_to_its_answer() {
    let f = fixture();
    f.service
        .says(200, running())
        .says(200, running())
        .says(200, done(json!({"exit_code": 0, "output": "ok"})));
    let v = make(&f).await.unwrap();
    assert_eq!(v["output"], "ok");
    assert_eq!(
        f.service.lines(),
        [
            (Method::Post, "/v1/jobs".to_string()),
            (Method::Get, POLL.to_string()),
            (Method::Get, POLL.to_string()),
        ]
    );
    let job: JobRequest = serde_json::from_value(f.service.body(0)).unwrap();
    assert_eq!(job.key, "R1:2:0");
    assert_eq!(job.run, RunRef::of(&run()));
    assert_eq!(
        f.service.body(0)["command"],
        "make",
        "{}",
        f.service.body(0)
    );
}

#[tokio::test(start_paused = true)]
async fn every_request_says_the_version_of_the_wire() {
    let f = fixture();
    f.service.says(200, done(json!({})));
    make(&f).await.unwrap();
    let seen = f.service.seen.lock().unwrap();
    assert!(
        seen[0]
            .headers
            .contains(&(WIRE_HEADER, WIRE_VERSION.to_string()))
    );
    assert!(
        seen[0]
            .headers
            .contains(&(WIRE_INSTANCE_HEADER, INSTANCE.to_string()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_jobs_failure_is_its_own_error() {
    let f = fixture();
    f.service.says(
        200,
        json!({"state": "failed", "code": "shell_unavailable", "message": "no container",
               "retryable": true, "snapshot": wire(&offers())}),
    );
    let e = make(&f).await.unwrap_err();
    assert_eq!(e.code, "shell_unavailable");
    assert_eq!(e.message, "no container");
    assert!(e.retryable);
}

#[tokio::test(start_paused = true)]
async fn joining_a_command_only_polls_and_never_starts_one() {
    let f = fixture();
    f.service
        .says(200, running())
        .says(200, done(json!({"exit_code": 3, "output": ""})));
    let v = f.remote.resume_exec(&run(), "R1:2:0").await.unwrap();
    assert_eq!(v.unwrap()["exit_code"], 3);
    assert!(f.service.lines().iter().all(|(m, _)| *m == Method::Get));
}

#[tokio::test(start_paused = true)]
async fn joining_a_command_the_service_does_not_know_finds_none() {
    let f = fixture();
    f.service.says(404, no_such_job());
    assert_eq!(f.remote.resume_exec(&run(), "R1:2:0").await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn a_command_the_service_lost_while_it_ran_is_interrupted_and_not_sent_again() {
    let f = fixture();
    f.service.says(200, running()).says(404, no_such_job());
    let e = make(&f).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_INTERRUPTED);
    assert_eq!(f.service.lines().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_proposal_the_service_lost_is_sent_once_more_under_its_key() {
    let f = fixture();
    f.service
        .says(200, running())
        .says(404, no_such_job())
        .says(200, done(json!({"pr": 7, "verify_url": null})));
    let v = f.remote.propose(&run(), &proposal()).await.unwrap();
    assert_eq!(v["pr"], 7);
    assert_eq!(f.service.body(0), f.service.body(2));
    assert_eq!(f.service.body(2)["kind"], "propose");
    assert_eq!(f.service.body(2)["key"], "R1:2:0");

    let f = fixture();
    f.service
        .says(200, running())
        .says(404, no_such_job())
        .says(200, running())
        .says(404, no_such_job());
    let e = f.remote.propose(&run(), &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert_eq!(f.service.lines().len(), 4, "lost twice, it stops");
}

#[tokio::test(start_paused = true)]
async fn a_proposals_link_stays_only_on_an_allowed_host() {
    let f = fixture();
    f.service.says(
        200,
        done(json!({"verify_url": "https://pr-1.pages.example/"})),
    );
    let v = f.remote.propose(&run(), &proposal()).await.unwrap();
    assert_eq!(v["verify_url"], "https://pr-1.pages.example/");
    f.service.says(
        200,
        done(json!({"verify_url": "https://evil.example/", "pr": 1})),
    );
    let v = f.remote.propose(&run(), &proposal()).await.unwrap();
    assert_eq!(v["verify_url"], Value::Null);
    assert_eq!(v["pr"], 1);
}

#[test]
fn link_hosts_allow_https_on_the_hosts_they_name_alone() {
    let h = LinkHosts::parse(" preview.example , *.pages.example, ports.example:8443 ");
    assert!(h.allows("https://preview.example/x"));
    assert!(
        h.allows("https://preview.example:443/x"),
        "the default port"
    );
    assert!(h.allows("https://PR-1.Pages.Example/"));
    assert!(h.allows("https://a.b.pages.example/"), "whole labels");
    assert!(h.allows("https://ports.example:8443/"));
    assert!(
        !h.allows("https://pages.example/"),
        "a zone is not under itself"
    );
    assert!(!h.allows("http://preview.example/"));
    assert!(!h.allows("https://preview.example:8443/"), "another port");
    assert!(!h.allows("https://ports.example/"), "not its port");
    assert!(!h.allows("https://pr-1.pages.example:8443/"));
    assert!(!h.allows("https://preview.example.evil.example/"));
    assert!(!h.allows("https://evilpages.example/"));
    assert!(!h.allows("https://preview.example@evil.example/"));
    assert!(!h.allows("https://me@preview.example/"));
    assert!(!h.allows("not a link"));
    assert!(!LinkHosts::parse("").allows("https://preview.example/"));
}

#[test]
fn a_pattern_too_broad_or_malformed_allows_nothing() {
    for p in [
        "*",
        "*.",
        "*.example",
        "a.*.example",
        "*a.example",
        "x.example:port",
    ] {
        let h = LinkHosts::parse(p);
        assert!(!h.allows("https://a.example/"), "{p}");
        assert!(!h.allows("https://b.a.example/"), "{p}");
        assert!(!h.allows("https://x.example/"), "{p}");
    }
}

#[tokio::test(start_paused = true)]
async fn with_no_snapshot_the_offers_have_lapsed() {
    let f = fixture();
    assert_eq!(f.remote.offer(&run(), Some("Q1")), lapsed_offer());
    assert_eq!(f.remote.offer(&run(), None), lapsed_offer());
}

#[tokio::test(start_paused = true)]
async fn a_reap_sends_the_active_runs_and_brings_the_offers() {
    let f = fixture();
    let until = Snapshot {
        previews_until_ms: Some(99),
        ..offers()
    };
    f.service.says(200, wire(&until));
    f.remote.reap(&["R1".to_string()]).await;
    assert_eq!(f.service.lines(), [(Method::Post, "/v1/reap".to_string())]);
    assert_eq!(f.service.body(0), json!({"active": ["R1"]}));
    assert_eq!(f.remote.offer(&run(), Some("Q1")), Offer::SHELL);
    assert_eq!(f.remote.previews_until_ms(), Some(99));
    assert_eq!(f.keeper.last().snapshot, Some(until));
}

#[tokio::test(start_paused = true)]
async fn a_job_answer_brings_the_offers_and_the_same_ones_are_not_saved_again() {
    let f = fixture();
    f.service
        .says(200, running())
        .says(200, running())
        .says(200, done(json!({})));
    make(&f).await.unwrap();
    assert_eq!(f.remote.offer(&run(), None), Offer::SHELL);
    assert_eq!(f.keeper.snapshots(), 1);
}

fn kept_at(ms: i64) -> Kept {
    Kept {
        snapshot: Some(offers()),
        received_ms: ms,
        ..Kept::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_new_workspace_answers_from_what_it_was_given() {
    let f = fixture();
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    let g = fixture_from(Some(f.keeper.last()));
    assert_eq!(g.remote.offer(&run(), Some("Q1")), Offer::SHELL);
}

#[tokio::test(start_paused = true)]
async fn a_failed_reap_keeps_the_offers_it_had() {
    let f = fixture();
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    f.remote.reap(&[]).await;
    assert_eq!(f.service.lines().len(), 4);
    assert_eq!(f.remote.offer(&run(), None), Offer::SHELL);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_past_its_age_has_lapsed_until_a_new_one_comes() {
    let f = fixture();
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    f.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 - 1);
    assert_eq!(f.remote.offer(&run(), None), Offer::SHELL);
    f.clock.advance(2);
    assert_eq!(f.remote.offer(&run(), None), lapsed_offer());
    assert_eq!(f.remote.offer(&run(), Some("Q1")), lapsed_offer());
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    assert_eq!(f.remote.offer(&run(), None), Offer::SHELL);
}

#[tokio::test(start_paused = true)]
async fn a_target_is_private_only_on_the_services_own_word_and_a_fresh_snapshot() {
    let f = fixture();
    assert!(!f.remote.target_private(&run(), Some("Q1")), "no snapshot");
    // An offer that reads confidential says nothing of the target.
    let s = Snapshot {
        queues: BTreeMap::from([("Q1".to_string(), Offer::unsure())]),
        runs: BTreeMap::new(),
        ..offers()
    };
    f.service.says(200, wire(&s));
    f.remote.reap(&[]).await;
    assert!(f.remote.offer(&run(), Some("Q1")).confidential);
    assert!(!f.remote.target_private(&run(), Some("Q1")));

    let s = Snapshot {
        seq: 6,
        private_queues: BTreeSet::from(["Q1".to_string()]),
        private_runs: BTreeSet::from(["R2".to_string()]),
        ..offers()
    };
    f.service.says(200, wire(&s));
    f.remote.reap(&[]).await;
    assert!(f.remote.target_private(&run(), Some("Q1")));
    assert!(!f.remote.target_private(&run(), Some("Q2")));
    assert!(!f.remote.target_private(&run(), None));
    let mut other = run();
    other.id = "R2".into();
    assert!(f.remote.target_private(&other, None));

    f.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 + 1);
    assert!(!f.remote.target_private(&run(), Some("Q1")), "too old");
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_kept_too_long_has_lapsed_in_a_new_incarnation() {
    let f = fixture();
    let old = f.clock_ms() - MAX_SNAPSHOT_AGE_MS - 1;
    let g = fixture_from(Some(kept_at(old)));
    assert_eq!(g.remote.offer(&run(), None), lapsed_offer());
}

#[tokio::test(start_paused = true)]
async fn the_same_snapshot_is_saved_now_and_then_so_a_new_incarnation_reads_its_age() {
    let f = fixture();
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    f.clock.advance(120);
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    assert_eq!(f.keeper.snapshots(), 2);
    assert_eq!(f.keeper.last().received_ms, f.clock_ms());
}

#[tokio::test(start_paused = true)]
async fn an_earlier_snapshot_answering_late_is_not_kept() {
    let f = fixture();
    let later = Snapshot { seq: 9, ..offers() };
    f.service.says(200, wire(&later));
    f.remote.reap(&[]).await;
    let earlier = Snapshot {
        seq: 8,
        others: Offer::CHANGE,
        runs: BTreeMap::new(),
        ..offers()
    };
    f.service.says(200, wire(&earlier));
    f.remote.reap(&[]).await;
    assert_eq!(f.remote.offer(&run(), None), Offer::SHELL);
    // A service that lost count is believed once what it held is too old.
    f.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 + 1);
    f.service.says(200, wire(&earlier));
    f.remote.reap(&[]).await;
    assert_eq!(f.remote.offer(&run(), None), Offer::CHANGE);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_with_a_field_missing_is_not_read_and_the_call_fails_closed() {
    let f = fixture();
    let mut bare = wire(&offers());
    bare["others"]
        .as_object_mut()
        .unwrap()
        .remove("no_follow_ups");
    f.service.says(200, bare);
    f.remote.reap(&[]).await;
    assert_eq!(f.remote.offer(&run(), None), lapsed_offer());
}

#[tokio::test(start_paused = true)]
async fn finishing_ends_the_run_at_the_service() {
    let f = fixture();
    f.service.says(200, wire(&offers()));
    f.remote.finish(&run()).await;
    assert_eq!(
        f.service.lines(),
        [(Method::Delete, "/v1/runs/R1".to_string())]
    );
    assert!(f.keeper.last().unfinished.is_empty());
    assert_eq!(f.remote.next_due_ms(), None);
}

#[tokio::test(start_paused = true)]
async fn a_lost_finish_is_kept_and_retried_by_a_later_incarnation() {
    let f = fixture();
    f.remote.finish(&run()).await;
    assert_eq!(f.service.lines().len(), 3);
    let kept = f.keeper.last();
    assert!(kept.unfinished.contains("R1"));
    assert!(f.remote.next_due_ms().is_some());

    let g = fixture_from(Some(kept));
    assert!(g.remote.next_due_ms().is_some());
    g.service.says(200, wire(&offers()));
    g.remote.retry_finishes().await;
    assert_eq!(
        g.service.lines(),
        [(Method::Delete, "/v1/runs/R1".to_string())]
    );
    assert!(g.keeper.last().unfinished.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_reap_settles_the_ends_of_runs_no_longer_active() {
    let f = fixture();
    f.remote.finish(&run()).await;
    f.service.says(200, wire(&offers()));
    f.remote.reap(&[]).await;
    assert!(f.keeper.last().unfinished.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_service_out_of_reach_fails_closed_in_words_of_our_own() {
    let f = fixture();
    let e = make(&f).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.message.contains("could not be reached"), "{}", e.message);
    assert_eq!(f.service.lines().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn an_outage_while_polling_is_waited_out_however_long() {
    let f = fixture();
    f.service.says(200, running()).down(40);
    f.service
        .says(502, json!({}))
        .raw(200, b"<html>tunnel</html>");
    f.service.says(200, done(json!({"output": "ok"})));
    assert_eq!(make(&f).await.unwrap()["output"], "ok");
}

#[tokio::test(start_paused = true)]
async fn a_tunnels_own_404_is_not_the_service_losing_a_job() {
    let f = fixture();
    f.service.says(200, running()).raw(404, b"not found");
    f.service
        .says(404, json!({"code": "no_such_route", "message": "no"}));
    f.service.says(200, done(json!({"output": "ok"})));
    assert_eq!(make(&f).await.unwrap()["output"], "ok");
    let f = fixture();
    f.service.raw(404, b"not found").says(404, no_such_job());
    assert_eq!(f.remote.resume_exec(&run(), "R1:2:0").await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn an_outage_while_joining_never_answers_for_the_job() {
    let f = fixture();
    let r = run();
    let wait = f.remote.resume_exec(&r, "R1:2:0");
    assert!(
        tokio::time::timeout(Duration::from_secs(600), wait)
            .await
            .is_err()
    );
    assert!(
        f.service.lines().len() > 5,
        "and it backs off, then goes on"
    );
    assert!(f.service.lines().len() < 60);
}

#[tokio::test(start_paused = true)]
async fn a_service_that_ignores_the_wait_is_not_polled_in_a_spin() {
    let f = fixture();
    f.service.says(200, running());
    for _ in 0..4 {
        f.service.says(200, running());
    }
    f.service.says(200, done(json!({})));
    let started = tokio::time::Instant::now();
    make(&f).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn a_proposal_that_cannot_be_sent_is_tried_until_it_can() {
    let f = fixture();
    f.service.down(9).says(200, done(json!({"pr": 1})));
    let v = f.remote.propose(&run(), &proposal()).await.unwrap();
    assert_eq!(v["pr"], 1);
}

#[tokio::test(start_paused = true)]
async fn a_proposal_the_service_refuses_is_answered_at_once() {
    let f = fixture();
    f.service
        .says(403, json!({"code": "wrong_instance", "message": "no"}));
    let e = f.remote.propose(&run(), &proposal()).await.unwrap_err();
    assert!(e.message.contains("credentials"), "{}", e.message);
}

#[tokio::test(start_paused = true)]
async fn a_proposal_that_failed_and_may_be_tried_again_is_sent_once_more() {
    let failed = json!({"state": "failed", "code": "shell_unavailable", "message": "busy",
                        "retryable": true, "snapshot": wire(&offers())});
    let f = fixture();
    f.service
        .says(200, failed.clone())
        .says(200, done(json!({"pr": 2})));
    assert_eq!(
        f.remote.propose(&run(), &proposal()).await.unwrap()["pr"],
        2
    );
    assert_eq!(f.service.lines().len(), 2);

    let f = fixture();
    f.service
        .says(200, failed.clone())
        .says(200, failed.clone());
    assert!(f.remote.propose(&run(), &proposal()).await.is_err());
    assert_eq!(f.service.lines().len(), 2);

    let f = fixture();
    f.service.says(200, failed);
    assert!(make(&f).await.is_err(), "a command is not sent again");
    assert_eq!(f.service.lines().len(), 1);
}

fn refused_change() -> Value {
    json!({"state": "failed", "code": "change_refused", "message": "the branch moved",
           "snapshot": wire(&offers())})
}

#[tokio::test(start_paused = true)]
async fn a_proposal_refused_when_first_sent_says_so() {
    let f = fixture();
    f.service.says(200, refused_change());
    let e = f.remote.propose(&run(), &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
}

#[tokio::test(start_paused = true)]
async fn a_refusal_of_a_proposal_sent_again_does_not_read_as_nothing_having_left() {
    let check = |e: HostError| {
        assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
        assert_eq!(e.message, "the branch moved");
        assert!(!e.retryable);
    };

    // The first sending failed with its change out.
    let failed = json!({"state": "failed", "code": "shell_unavailable", "message": "pushed",
                        "retryable": true, "snapshot": wire(&offers())});
    let f = fixture();
    f.service.says(200, failed).says(200, refused_change());
    check(f.remote.propose(&run(), &proposal()).await.unwrap_err());

    // The service lost it while it ran.
    let f = fixture();
    f.service
        .says(200, running())
        .says(404, no_such_job())
        .says(200, refused_change());
    check(f.remote.propose(&run(), &proposal()).await.unwrap_err());

    // Sending it failed, and the service may have heard all the same.
    let f = fixture();
    f.service.down(1).says(200, refused_change());
    check(f.remote.propose(&run(), &proposal()).await.unwrap_err());
}

#[tokio::test(start_paused = true)]
async fn a_known_commands_key_with_no_job_answers_interrupted_by_the_service() {
    let f = fixture();
    let gone = json!({"state": "failed", "code": "shell_interrupted", "message": "gone",
                      "snapshot": wire(&offers())});
    f.service.says(200, gone);
    assert_eq!(make(&f).await.unwrap_err().code, CODE_SHELL_INTERRUPTED);
}

#[tokio::test(start_paused = true)]
async fn an_answer_past_the_limit_is_not_read() {
    let f = fixture();
    f.service.raw(200, &vec![b' '; MAX_RESPONSE_BYTES + 1]);
    assert!(make(&f).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn a_request_that_never_answers_is_given_up() {
    let f = fixture();
    for _ in 0..3 {
        f.service.next.lock().unwrap().push_back(Next::Hang);
    }
    let e = make(&f).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
}

#[tokio::test(start_paused = true)]
async fn a_skewed_service_says_so_clearly_and_is_not_asked_again() {
    let f = fixture();
    let skew = WireFailure {
        wire: Some(2),
        ..WireFailure::skew()
    };
    f.service.says(426, serde_json::to_value(skew).unwrap());
    let e = make(&f).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.message.contains("wire version 2"), "{}", e.message);
    assert!(e.message.contains("version 1"), "{}", e.message);
    assert_eq!(f.service.lines().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn refused_credentials_say_only_that() {
    let f = fixture();
    f.service
        .says(401, json!({"message": "Bearer s3cret is wrong"}));
    let e = make(&f).await.unwrap_err();
    assert!(e.message.contains("credentials"), "{}", e.message);
    assert!(!e.message.contains("s3cret"));
}

#[tokio::test(start_paused = true)]
async fn the_shell_of_the_trait_without_a_key_is_refused() {
    let f = fixture();
    let e = f
        .remote
        .exec(&run(), None, "ls", None, 1)
        .await
        .unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(f.service.lines().is_empty());
}

fn event(id: &str, run: &str, told: PreviewTold) -> PreviewEvent {
    PreviewEvent {
        id: id.into(),
        run: run.into(),
        told,
    }
}

fn ready(url: &str) -> PreviewTold {
    PreviewTold::Ready {
        verify_url: url.into(),
        reviewed: true,
    }
}

fn previews(events: Vec<PreviewEvent>) -> Value {
    serde_json::to_value(PreviewsAnswer {
        instance: INSTANCE.to_string(),
        events,
        snapshot: offers(),
    })
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn previews_are_told_acknowledged_and_bring_the_offers() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    f.remote.previews(sink.clone());
    f.service
        .says(
            200,
            previews(vec![
                event("E1", "R1", ready("https://preview.example/")),
                event("E2", "R2", PreviewTold::Missed),
            ]),
        )
        .says(204, json!({}));
    assert_eq!(f.remote.poll_previews().await.unwrap(), 2);
    assert_eq!(
        *sink.told.lock().unwrap(),
        ["R1 ready https://preview.example/ true", "R2 missed"]
    );
    assert_eq!(f.service.body(1), json!({"ids": ["E1", "E2"]}));
    assert_eq!(f.remote.offer(&run(), None), Offer::SHELL);
}

#[tokio::test(start_paused = true)]
async fn a_preview_on_no_allowed_host_counts_as_none() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    f.remote.previews(sink.clone());
    f.service
        .says(
            200,
            previews(vec![event("E1", "R1", ready("https://evil.example/"))]),
        )
        .says(204, json!({}));
    assert_eq!(f.remote.poll_previews().await.unwrap(), 1);
    assert_eq!(*sink.told.lock().unwrap(), ["R1 missed"]);
}

#[tokio::test(start_paused = true)]
async fn what_could_not_be_told_stays_with_the_service_unless_its_run_is_gone() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    sink.fails.lock().unwrap().extend([
        ("R1".to_string(), "busy".to_string()),
        ("R2".to_string(), "not_found".to_string()),
    ]);
    f.remote.previews(sink.clone());
    f.service
        .says(
            200,
            previews(vec![
                event("E1", "R1", PreviewTold::Missed),
                event("E2", "R2", PreviewTold::Missed),
            ]),
        )
        .says(204, json!({}));
    assert_eq!(f.remote.poll_previews().await.unwrap(), 1);
    assert_eq!(f.service.body(1), json!({"ids": ["E2"]}));
}

#[tokio::test(start_paused = true)]
async fn with_no_sink_previews_are_left_with_the_service() {
    let f = fixture();
    assert_eq!(f.remote.poll_previews().await.unwrap(), 0);
    assert!(f.service.lines().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_answer_for_another_instance_tells_nothing_and_acknowledges_nothing() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    sink.fails
        .lock()
        .unwrap()
        .insert("R1".to_string(), "not_found".to_string());
    f.remote.previews(sink.clone());
    let mut other = previews(vec![event("E1", "R1", PreviewTold::Missed)]);
    other["instance"] = json!("I2");
    f.service.says(200, other);
    assert!(f.remote.poll_previews().await.is_err());
    assert_eq!(f.service.lines().len(), 1);
    assert!(sink.told.lock().unwrap().is_empty());
    assert!(f.keeper.last().poll_failed);
}

#[tokio::test(start_paused = true)]
async fn a_word_waiting_for_its_fix_stays_uncounted_and_unacknowledged() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    sink.fails
        .lock()
        .unwrap()
        .insert("R1".to_string(), CODE_FIX_COMING.to_string());
    f.remote.previews(sink.clone());
    for _ in 0..MAX_PREVIEW_TRIES + 2 {
        f.service
            .says(200, previews(vec![event("E1", "R1", PreviewTold::Missed)]));
        assert_eq!(f.remote.poll_previews().await.unwrap(), 0);
    }
    assert_eq!(f.service.lines().len() as u32, MAX_PREVIEW_TRIES + 2);
    assert!(sink.lost.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_word_that_keeps_failing_is_given_up_with_a_note_and_acknowledged() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    sink.fails
        .lock()
        .unwrap()
        .insert("R1".to_string(), "busy".to_string());
    f.remote.previews(sink.clone());
    for _ in 0..MAX_PREVIEW_TRIES - 1 {
        f.service
            .says(200, previews(vec![event("E1", "R1", PreviewTold::Missed)]));
        assert_eq!(f.remote.poll_previews().await.unwrap(), 0);
    }
    assert!(f.remote.next_due_ms().is_some(), "it is still owed");
    // The count outlives an incarnation.
    let g = fixture_from(Some(f.keeper.last()));
    g.remote.previews(sink.clone());
    g.service
        .says(200, previews(vec![event("E1", "R1", PreviewTold::Missed)]))
        .says(204, json!({}));
    assert_eq!(g.remote.poll_previews().await.unwrap(), 1);
    assert_eq!(*sink.lost.lock().unwrap(), ["R1"]);
    assert_eq!(g.service.body(1), json!({"ids": ["E1"]}));
    assert!(g.keeper.last().preview_tries.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_poll_takes_a_bounded_number_of_events() {
    let f = fixture();
    let sink = Arc::new(Sink::default());
    f.remote.previews(sink.clone());
    let many = (0..MAX_PREVIEW_EVENTS + 5)
        .map(|i| event(&format!("E{i}"), "R1", PreviewTold::Missed))
        .collect();
    f.service.says(200, previews(many)).says(204, json!({}));
    assert_eq!(f.remote.poll_previews().await.unwrap(), MAX_PREVIEW_EVENTS);
}

#[tokio::test(start_paused = true)]
async fn a_failed_poll_is_owed_past_the_end_of_the_wait() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    assert_eq!(f.remote.next_due_ms(), None);
    assert!(f.remote.poll_previews().await.is_err());
    assert!(f.remote.next_due_ms().is_some());
    let g = fixture_from(Some(f.keeper.last()));
    g.remote.previews(Arc::new(Sink::default()));
    assert!(g.remote.next_due_ms().is_some(), "and the debt is kept");
    g.service.says(200, previews(Vec::new()));
    g.remote.tend().await;
    assert_eq!(g.remote.next_due_ms(), None);
}

#[tokio::test(start_paused = true)]
async fn a_pull_requests_link_stays_only_on_an_allowed_host() {
    let f = fixture();
    f.service.says(
        200,
        done(json!({"url": "https://forge.example/pr/1", "verify_url": null})),
    );
    let v = f.remote.propose(&run(), &proposal()).await.unwrap();
    assert_eq!(v["url"], "https://forge.example/pr/1");
    f.service
        .says(200, done(json!({"url": "javascript:alert(1)", "pr": 1})));
    let v = f.remote.propose(&run(), &proposal()).await.unwrap();
    assert_eq!(v["url"], Value::Null);
    assert_eq!(v["pr"], 1);
}

#[tokio::test(start_paused = true)]
async fn tending_brings_an_old_snapshot_up_to_date_and_leaves_a_new_one() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    f.service.says(200, previews(Vec::new()));
    f.remote.tend().await;
    assert_eq!(f.service.lines().len(), 1);
    f.remote.tend().await;
    assert_eq!(f.service.lines().len(), 1, "it is fresh");
    f.clock.advance(6 * 60);
    f.service.says(200, previews(Vec::new()));
    f.remote.tend().await;
    assert_eq!(f.service.lines().len(), 2);
}

fn refusal(code: &str) -> Value {
    json!({"code": code, "message": "no"})
}

/// `work`, which must end long before the run's time would.
async fn promptly<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3600), work)
        .await
        .expect("it ended at once")
}

#[tokio::test(start_paused = true)]
async fn a_refusal_the_service_wrote_ends_the_wait_for_a_begun_job() {
    for (status, code, says) in [
        (401, "unauthorized", "credentials"),
        (403, "wrong_instance", "credentials"),
        (426, "wire_skew", "version"),
        (400, "bad_id", "answered 400"),
    ] {
        let f = fixture();
        f.service.says(200, running()).says(status, refusal(code));
        let e = promptly(make(&f)).await.unwrap_err();
        assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
        assert!(e.message.contains(says), "{code}: {}", e.message);

        let f = fixture();
        f.service.says(status, refusal(code));
        let r = run();
        let joined = f.remote.resume_exec(&r, "R1:2:0");
        assert!(promptly(joined).await.is_err(), "{code}, joining");
    }
}

#[tokio::test(start_paused = true)]
async fn a_tunnels_own_refusal_is_no_answer_about_a_begun_job() {
    let f = fixture();
    f.service.says(200, running());
    f.service
        .raw(401, b"<html>sign in</html>")
        .says(403, json!({"error": "blocked"}))
        .says(426, refusal("not_ours"))
        .says(404, refusal("no_such_route"));
    f.service.says(200, done(json!({"output": "ok"})));
    assert_eq!(make(&f).await.unwrap()["output"], "ok");
}

#[tokio::test(start_paused = true)]
async fn only_a_refusal_the_service_wrote_answers_a_proposal() {
    let f = fixture();
    f.service
        .raw(403, b"<html>blocked</html>")
        .says(404, json!({}))
        .says(200, done(json!({"pr": 1})));
    assert_eq!(
        f.remote.propose(&run(), &proposal()).await.unwrap()["pr"],
        1
    );

    let f = fixture();
    f.service
        .raw(401, b"<html>sign in</html>")
        .says(409, refusal("key_taken"));
    let (r, p) = (run(), proposal());
    let e = promptly(f.remote.propose(&r, &p)).await.unwrap_err();
    assert!(e.message.contains("409"), "{}", e.message);
}

/// How long from now [`RemoteWorkspace::next_due_ms`] says, in seconds.
fn due_in(f: &Fixture) -> i64 {
    (f.remote.next_due_ms().unwrap() - f.clock_ms()) / 1000
}

#[tokio::test(start_paused = true)]
async fn a_tend_that_keeps_failing_backs_off_to_a_cap_and_a_success_clears_it() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    f.service.says(401, refusal("unauthorized"));
    let mut waits = Vec::new();
    for _ in 0..8 {
        f.remote.tend().await;
        waits.push(due_in(&f));
    }
    assert_eq!(waits, [30, 60, 120, 240, 480, 600, 600, 600]);

    f.clock.advance(601);
    f.service.says(200, previews(Vec::new()));
    f.remote.tend().await;
    assert_eq!(f.remote.next_due_ms(), None, "nothing is owed");
    assert_eq!(f.keeper.last().tend_failures, 0);
}

#[tokio::test(start_paused = true)]
async fn an_ending_refused_for_ever_backs_off_too() {
    let f = fixture();
    f.service.says(200, previews(Vec::new()));
    f.remote.previews(Arc::new(Sink::default()));
    f.remote.tend().await;
    f.remote.finish(&run()).await;
    assert_eq!(due_in(&f), 30, "a first ending lost holds nothing back");
    f.remote.tend().await;
    f.remote.tend().await;
    assert_eq!(due_in(&f), 60);
}

#[tokio::test(start_paused = true)]
async fn the_wait_between_failed_tends_outlives_an_incarnation() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    for _ in 0..3 {
        f.remote.tend().await;
    }
    let g = fixture_from(Some(f.keeper.last()));
    g.clock.advance(20);
    g.remote.previews(Arc::new(Sink::default()));
    assert_eq!(due_in(&g), 100);
}

#[tokio::test(start_paused = true)]
async fn a_tend_cut_off_by_its_deadline_counts_as_failed() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    f.service.next.lock().unwrap().push_back(Next::Hang);
    assert!(!f.remote.tend_within(Duration::from_secs(5)).await);
    assert_eq!(f.keeper.last().tend_failures, 1);
    f.service.says(200, previews(Vec::new()));
    assert!(f.remote.tend_within(Duration::from_secs(5)).await);
    assert_eq!(f.keeper.last().tend_failures, 0);
}

#[tokio::test(start_paused = true)]
async fn tends_held_back_are_scaled_by_the_hosts_own_interval() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    let every = Duration::from_secs(1);
    assert_eq!(f.remote.tend_held_until_ms(every), None);
    f.remote.tend().await;
    f.remote.tend().await;
    assert_eq!(
        f.remote.tend_held_until_ms(every),
        Some(f.clock_ms() + 2000)
    );
}

#[tokio::test(start_paused = true)]
async fn a_slow_ending_cannot_hold_up_new_offers() {
    let f = fixture();
    f.remote.previews(Arc::new(Sink::default()));
    f.service.says(200, previews(Vec::new()));
    f.remote.tend().await;
    f.remote.finish(&run()).await;
    f.clock.advance(6 * 60);
    let seen = f.service.lines().len();
    f.service.says(200, previews(Vec::new()));
    f.service.next.lock().unwrap().push_back(Next::Hang);
    assert!(!f.remote.tend_within(Duration::from_secs(5)).await);
    let lines = f.service.lines();
    assert_eq!(lines[seen].0, Method::Get, "{lines:?}");
    assert_eq!(lines[seen + 1].0, Method::Delete, "{lines:?}");
}

#[tokio::test(start_paused = true)]
async fn a_lapse_reaches_a_queue_with_a_target_a_kept_offer_and_any_queue_with_no_word_at_all() {
    let f = fixture();
    let kept = run();
    let plain = RunRef {
        id: "R2".into(),
        thread_ids: vec![],
    }
    .to_run();
    assert!(f.remote.lapse_reaches(&plain, Some("Q9")), "no word at all");
    assert!(!f.remote.lapse_reaches(&plain, None), "no queue, no offer");

    let word = Snapshot {
        runs: BTreeMap::from([(kept.id.clone(), Offer::CHANGE)]),
        queues: BTreeMap::from([("Q1".to_string(), Offer::CHANGE)]),
        ..offers()
    };
    f.service.says(200, wire(&word));
    f.remote.reap(&[]).await;
    f.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 + 1);
    assert!(f.remote.offers_lapsed());
    assert!(f.remote.lapse_reaches(&plain, Some("Q1")), "its target's");
    assert!(f.remote.lapse_reaches(&kept, None), "its own");
    assert!(
        !f.remote.lapse_reaches(&plain, Some("Q9")),
        "no target there"
    );
    assert!(!f.remote.lapse_reaches(&plain, None));
}

#[tokio::test(start_paused = true)]
async fn lapsed_offers_are_their_own_state_and_a_targets_doubt_is_not_one() {
    let f = fixture();
    assert!(f.remote.offers_lapsed(), "with none at all");
    assert_eq!(f.remote.offer(&run(), None), lapsed_offer());

    let doubt = Snapshot {
        queues: BTreeMap::from([("Q1".to_string(), Offer::unsure())]),
        ..offers()
    };
    f.service.says(200, wire(&doubt));
    f.remote.reap(&[]).await;
    assert!(!f.remote.offers_lapsed());
    let other = RunRef {
        id: "R2".into(),
        thread_ids: vec![],
    }
    .to_run();
    assert_eq!(f.remote.offer(&other, Some("Q1")), Offer::unsure());

    f.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 + 1);
    assert!(f.remote.offers_lapsed());
    assert_eq!(f.remote.offer(&other, Some("Q1")), lapsed_offer());
    let lapsed = lapsed_offer();
    assert!(lapsed.confidential && !lapsed.unsure && !lapsed.shell && !lapsed.change);
}

use super::*;
use crate::bot_runs::{NewRun, RequestKind};
use crate::bot_settings::BaseUrlPolicy;
use crate::credits::KeySource;
use crate::queues::{NewQueue, NewReport, ReportWriter, Verdict, Visibility};
use crate::secrets::SecretSlot;
use crate::test_util::{DAEMON, anchor, store, store_with_clock};
use clax_core::queue::ReportersSee;
use clax_core::report::NewFix;
use clax_core::working::ManualClock;
use serde_json::json;
use std::sync::Arc;

const KEY: &str = "zulip-api-key-CANARY-wxyz";

fn kek() -> Kek {
    Kek::from_bytes([7; 32])
}

fn queue(st: &Store) -> String {
    st.create_queue(
        DAEMON,
        None,
        NewQueue {
            name: "Bugs".into(),
            origins: vec![],
            reporters_see: ReportersSee::Own,
        },
    )
    .unwrap()
    .id
}

fn fields() -> ZulipFields {
    ZulipFields {
        site: "https://acme.zulipchat.com".into(),
        bot_email: "clax-bot@acme.zulipchat.com".into(),
        api_key: Some(KEY.into()),
        stream: "bugs".into(),
        people: BTreeMap::from([("Ana-Dev".to_string(), "ana@acme.dev".to_string())]),
        if_rev: None,
    }
}

fn set(st: &Store, qid: &str) -> ZulipNotifier {
    st.set_zulip_notifier(DAEMON, &kek(), qid, &fields(), true)
        .unwrap()
}

fn file(st: &Store, qid: &str) -> String {
    st.file_report(
        DAEMON,
        qid,
        None,
        crate::queues::ExternalScope::Instance,
        NewReport {
            external_id: None,
            url: "https://app.example.com/invoices/4821".into(),
            title: "Invoice 4821".into(),
            anchor: anchor(),
            body: "The total is wrong".into(),
            author_name: "Ana".into(),
            metadata: json!(null),
            revision: None,
            clip: None,
            context: None,
            server: None,
            diagnostics: None,
        },
    )
    .unwrap()
    .id
}

fn person() -> ReportWriter<'static> {
    ReportWriter::Person {
        name: "Ana".into(),
        public_id: None,
    }
}

fn linked(summary: &str) -> NewFix {
    NewFix {
        summary: summary.into(),
        url: None,
        commit: None,
        release: None,
        verify_url: Some("https://pr-7.acme.dev/".into()),
        verify_after: None,
    }
}

fn unlinked() -> NewFix {
    NewFix {
        verify_url: None,
        ..linked("Held")
    }
}

/// A run's proposal, held or not, under `key`.
fn by_run(run: &str, key: &str, held: bool) -> ReportWriter<'static> {
    ReportWriter::Run {
        run: run.into(),
        name: "ClaxBot".into(),
        key: key.into(),
        note: None,
        held,
        preview_url: None,
    }
}

/// A run on report `tid` of `qid`, not confidential.
fn run_on(st: &Store, qid: &str, tid: &str) -> String {
    let aid = st
        .report(qid, tid, Visibility::All)
        .unwrap()
        .unwrap()
        .artifact
        .id;
    st.create_run(
        DAEMON,
        NewRun {
            artifact_id: aid,
            person: crate::roles::OWNER.into(),
            request: RequestKind::Thread,
            thread_ids: vec![tid.into()],
            model: "claude-sonnet-5-5".into(),
            key_source: KeySource::Instance,
            base_version: 1,
            agent_grant: None,
            web: String::new(),
        },
    )
    .unwrap()
    .id
}

fn events(st: &Store) -> Vec<(u32, NoticeEvent)> {
    let mut v: Vec<_> = st
        .take_due_notices(50)
        .unwrap()
        .into_iter()
        .map(|n| (n.n, n.event))
        .collect();
    v.sort_by_key(|e| e.0);
    v
}

fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
    match r {
        Err(CoreError::Invalid { code, .. }) => code,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_key_is_write_only_and_opens_for_its_notifier_alone() {
    let (_d, st) = store();
    let qid = queue(&st);
    let n = set(&st, &qid);
    assert_eq!(n.key.as_ref().unwrap().last4, "wxyz");
    assert!(!format!("{n:?}").contains(KEY));
    assert_eq!(
        n.people["ana-dev"], "ana@acme.dev",
        "logins compare lowercase"
    );
    let slot = SecretSlot::NotifierZulip(qid.clone());
    assert!(
        st.open_for_use(&kek(), &slot, "https://acme.zulipchat.com")
            .is_err()
    );
    let opened = st.open_zulip_notifier(&kek(), &qid).unwrap().unwrap();
    assert_eq!(opened.api_key.expose(), KEY);
    assert!(!format!("{opened:?}").contains(KEY));
}

#[test]
fn a_write_names_the_rev_it_was_made_from() {
    let (_d, st) = store();
    let qid = queue(&st);
    let n = set(&st, &qid);
    let stale = ZulipFields {
        stream: "other".into(),
        ..fields()
    };
    assert_eq!(
        code(st.set_zulip_notifier(DAEMON, &kek(), &qid, &stale, true)),
        "notifier_changed"
    );
    assert_eq!(
        code(st.clear_zulip_notifier(DAEMON, &qid, "old")),
        "notifier_changed"
    );
    let moved = st
        .set_zulip_notifier(
            DAEMON,
            &kek(),
            &qid,
            &ZulipFields {
                if_rev: Some(n.rev.clone()),
                api_key: None,
                ..stale
            },
            true,
        )
        .unwrap();
    assert_eq!(moved.stream, "other");
    assert_ne!(moved.rev, n.rev);
    assert_eq!(moved.key.unwrap().last4, "wxyz", "no new key keeps the old");
    assert!(st.clear_zulip_notifier(DAEMON, &qid, &moved.rev).unwrap());
    assert!(st.zulip_notifier(&qid).unwrap().is_none());
    assert!(
        st.secret_meta(&SecretSlot::NotifierZulip(qid))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_new_site_needs_a_new_key_so_nobody_sends_one_somewhere_new() {
    let (_d, st) = store();
    let qid = queue(&st);
    let n = set(&st, &qid);
    let away = ZulipFields {
        site: "https://attacker.example.com".into(),
        api_key: None,
        if_rev: Some(n.rev),
        ..fields()
    };
    assert_eq!(
        code(st.set_zulip_notifier(DAEMON, &kek(), &qid, &away, true)),
        "api_key_required"
    );
    let first = ZulipFields {
        api_key: None,
        ..fields()
    };
    let other = queue(&st);
    assert_eq!(
        code(st.set_zulip_notifier(DAEMON, &kek(), &other, &first, true)),
        "api_key_required"
    );
}

#[test]
fn a_site_is_held_to_the_rule_of_who_sets_it() {
    let (_d, st) = store();
    let qid = queue(&st);
    let at = |site: &str| ZulipFields {
        site: site.into(),
        ..fields()
    };
    // Another admin on a local daemon: the host would call it for them.
    for bad in [
        "http://127.0.0.1:9",
        "https://10.0.0.1",
        "https://u:p@acme.dev",
        "ftp://acme.dev",
    ] {
        assert_eq!(
            code(st.set_zulip_notifier(DAEMON, &kek(), &qid, &at(bad), false)),
            "invalid_site",
            "{bad}"
        );
    }
    let n = st
        .set_zulip_notifier(DAEMON, &kek(), &qid, &at("http://127.0.0.1:9"), true)
        .unwrap();
    assert!(n.local_site);
    // An admin may save the rest of a form holding the owner's address.
    let same = ZulipFields {
        site: "http://127.0.0.1:9".into(),
        api_key: None,
        stream: "other".into(),
        if_rev: Some(n.rev),
        ..fields()
    };
    let kept = st
        .set_zulip_notifier(DAEMON, &kek(), &qid, &same, false)
        .unwrap();
    assert!(kept.local_site);
    st.set_base_url_policy(BaseUrlPolicy::Hosted);
    let other = queue(&st);
    assert_eq!(
        code(st.set_zulip_notifier(DAEMON, &kek(), &other, &at("http://127.0.0.1:9"), true)),
        "invalid_site"
    );
    let n = st
        .set_zulip_notifier(DAEMON, &kek(), &other, &fields(), true)
        .unwrap();
    assert!(!n.local_site, "hosted: public addresses alone");
}

/// A daemon that went hosted after its owner set a loopback site holds it to
/// the hosted rule from then on, whatever the row says.
#[test]
fn a_site_set_on_a_local_daemon_is_no_longer_local_once_the_daemon_is_hosted() {
    let (_d, st) = store();
    let qid = queue(&st);
    let f = ZulipFields {
        site: "http://127.0.0.1:9".into(),
        ..fields()
    };
    let n = st
        .set_zulip_notifier(DAEMON, &kek(), &qid, &f, true)
        .unwrap();
    assert!(n.local_site);
    st.set_base_url_policy(BaseUrlPolicy::Hosted);
    let n = st.zulip_notifier(&qid).unwrap().unwrap();
    assert!(!n.local_site);
    // An address literal reaches no resolver, so the rule is held here too.
    assert_eq!(code(st.open_zulip_notifier(&kek(), &qid)), "invalid_site");
    let kept = ZulipFields {
        site: "http://127.0.0.1:9".into(),
        api_key: None,
        if_rev: Some(n.rev),
        ..fields()
    };
    assert_eq!(
        code(st.set_zulip_notifier(DAEMON, &kek(), &qid, &kept, true)),
        "invalid_site"
    );
}

/// One queue's backlog takes no turn from another's first notice.
#[test]
fn a_taking_goes_round_the_queues() {
    let (_d, st) = store();
    let (busy, quiet) = (queue(&st), queue(&st));
    set(&st, &busy);
    set(&st, &quiet);
    for _ in 0..6 {
        let tid = file(&st, &busy);
        st.propose_fix(DAEMON, &busy, &tid, &person(), linked("One"))
            .unwrap();
    }
    let tid = file(&st, &quiet);
    st.propose_fix(DAEMON, &quiet, &tid, &person(), linked("One"))
        .unwrap();
    let taken = st.take_due_notices(2).unwrap();
    let queues: std::collections::BTreeSet<_> = taken.iter().map(|n| &n.queue_id).collect();
    assert_eq!(queues.len(), 2, "{taken:?}");
}

#[test]
fn what_went_is_forgotten_after_a_month_and_what_expired_is_still_counted() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    set(&st, &qid);
    let rows = |st: &Store| -> u32 {
        st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM queue_notices", [], |r| r.get(0))?))
            .unwrap()
    };
    let (a, b, c) = (file(&st, &qid), file(&st, &qid), file(&st, &qid));
    for tid in [&a, &b, &c] {
        st.propose_fix(DAEMON, &qid, tid, &person(), linked("One"))
            .unwrap();
    }
    let due = st.take_due_notices(3).unwrap();
    st.notice_sent(&due[0].id).unwrap();
    st.drop_notice(&due[1].id).unwrap();
    assert_eq!(rows(&st), 3);
    clock.advance(PRUNE_SECS + 1);
    assert!(st.take_due_notices(3).unwrap().is_empty());
    assert_eq!(rows(&st), 1);
    assert_eq!(st.zulip_notifier(&qid).unwrap().unwrap().expired, 1);
}

#[test]
fn deleting_a_queue_forgets_what_its_fixes_linked_to() {
    let (_d, st) = store();
    let qid = queue(&st);
    let tid = file(&st, &qid);
    let links = FixLinks {
        url: Some("https://github.com/a/b/pull/7".into()),
        ..FixLinks::default()
    };
    st.keep_fix_links(&tid, "k", &links).unwrap();
    st.delete_queue(DAEMON, &qid).unwrap();
    let left: u32 = st
        .with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM queue_fix_links", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn bad_fields_are_refused() {
    let (_d, st) = store();
    let qid = queue(&st);
    let people = |p: &[(&str, &str)]| ZulipFields {
        people: p
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..fields()
    };
    let cases = [
        (
            ZulipFields {
                bot_email: "no-at-sign".into(),
                ..fields()
            },
            "invalid_bot_email",
        ),
        (
            ZulipFields {
                stream: " ".into(),
                ..fields()
            },
            "invalid_stream",
        ),
        (
            ZulipFields {
                stream: "a\nb".into(),
                ..fields()
            },
            "invalid_stream",
        ),
        (
            ZulipFields {
                api_key: Some("short".into()),
                ..fields()
            },
            "invalid_key",
        ),
        (people(&[("not a login", "a@b.dev")]), "invalid_people"),
        (people(&[("ana", "a@b.dev,c@d.dev")]), "invalid_people"),
        (people(&[("ana", "a:b@c.dev")]), "invalid_people"),
        (
            ZulipFields {
                bot_email: "a:b@c.dev".into(),
                ..fields()
            },
            "invalid_bot_email",
        ),
        // Zulip's `to` takes a channel's name or its ID, and does not say
        // which an all-digit string is.
        (
            ZulipFields {
                stream: "1234".into(),
                ..fields()
            },
            "invalid_stream",
        ),
        (
            people(&[("Ana", "a@b.dev"), ("ana", "c@d.dev")]),
            "invalid_people",
        ),
    ];
    for (f, want) in &cases {
        assert_eq!(
            code(st.set_zulip_notifier(DAEMON, &kek(), &qid, f, true)),
            *want
        );
    }
    assert!(st.zulip_notifier(&qid).unwrap().is_none());
}

#[test]
fn a_fix_with_a_link_is_one_notice_from_any_proposer() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    let by = by_run("run-1", "k", false);
    st.propose_fix(DAEMON, &qid, &tid, &by, linked("Two"))
        .unwrap();
    // The same proposal again is the same attempt, and tells nobody more.
    st.propose_fix(DAEMON, &qid, &tid, &by, linked("Two"))
        .unwrap();
    assert_eq!(
        events(&st),
        vec![(1, NoticeEvent::FixToCheck), (2, NoticeEvent::FixToCheck)]
    );
}

#[test]
fn no_notifier_means_no_notice_and_a_fix_with_no_link_tells_no_reporter() {
    let (_d, st) = store();
    let qid = queue(&st);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    set(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), unlinked())
        .unwrap();
    assert!(events(&st).is_empty());
    assert!(st.notices_next_due().unwrap().is_none());
}

#[test]
fn a_held_fix_waiting_on_its_preview_alone_asks_nobody_until_its_wait_ends() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    let run = run_on(&st, &qid, &tid);
    st.propose_fix(DAEMON, &qid, &tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    assert!(events(&st).is_empty(), "the preview may yet come by itself");
    assert!(st.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixWaitEnded)]);
}

#[test]
fn a_held_fix_a_person_must_deploy_asks_at_its_proposal_and_tells_of_its_wait_ending_once() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    let run = run_on(&st, &qid, &tid);
    let links = FixLinks {
        preview_step: Some("To deploy its preview, press go.".into()),
        ..FixLinks::default()
    };
    st.keep_fix_links(&tid, "k", &links).unwrap();
    st.propose_fix(DAEMON, &qid, &tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixHeld)]);
    assert!(st.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(
        events(&st),
        vec![(1, NoticeEvent::FixWaitEnded)],
        "what the held notice said of the wait is no longer so"
    );
    assert!(!st.set_fix_preview_missed(&tid, &run).unwrap());
    assert!(events(&st).is_empty(), "a replay tells nobody twice");
}

/// A held fix a person must deploy, with its step kept, from a run on report
/// `tid`, whose ID this answers.
fn held_with_step(st: &Store, qid: &str, tid: &str) -> String {
    let run = run_on(st, qid, tid);
    let links = FixLinks {
        preview_step: Some("To deploy its preview, press go.".into()),
        ..FixLinks::default()
    };
    st.keep_fix_links(tid, "k", &links).unwrap();
    st.propose_fix(DAEMON, qid, tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    run
}

#[test]
fn a_preview_that_answers_in_time_ends_no_wait() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    let run = held_with_step(&st, &qid, &tid);
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixHeld)]);
    assert!(
        st.set_fix_verify_url(DAEMON, &qid, &tid, &run, "https://pr-7.acme.dev")
            .unwrap()
    );
    assert!(!st.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixToCheck)]);

    // Kept for the queue's members, a preview ends the wait the same way.
    let tid = file(&st, &qid);
    let run = held_with_step(&st, &qid, &tid);
    assert!(
        st.set_fix_preview(&tid, &run, "https://pr-8.acme.dev")
            .unwrap()
    );
    assert!(!st.set_fix_preview_missed(&tid, &run).unwrap());
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixHeld)]);
}

#[test]
fn a_notifier_set_after_the_proposal_is_told_nothing_of_its_wait() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    let tid = file(&st, &qid);
    let run = held_with_step(&st, &qid, &tid);
    clock.advance(60);
    set(&st, &qid);
    assert!(st.set_fix_preview_missed(&tid, &run).unwrap());
    assert!(events(&st).is_empty());
}

#[test]
fn a_notifier_saved_again_since_the_proposal_is_still_the_one_that_was_there() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    let n = set(&st, &qid);
    let tid = file(&st, &qid);
    clock.advance(60);
    let run = held_with_step(&st, &qid, &tid);
    clock.advance(60);
    let f = ZulipFields {
        api_key: None,
        if_rev: Some(n.rev),
        ..fields()
    };
    st.set_zulip_notifier(DAEMON, &kek(), &qid, &f, true)
        .unwrap();
    assert!(st.set_fix_preview_missed(&tid, &run).unwrap());
    let mut told = events(&st);
    told.sort_by_key(|e| e.1.as_str());
    assert_eq!(
        told,
        vec![(1, NoticeEvent::FixHeld), (1, NoticeEvent::FixWaitEnded)]
    );
}

#[test]
fn a_notifier_removed_and_set_again_counts_from_then() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    let n = set(&st, &qid);
    let tid = file(&st, &qid);
    let run = held_with_step(&st, &qid, &tid);
    clock.advance(60);
    assert!(st.clear_zulip_notifier(DAEMON, &qid, &n.rev).unwrap());
    set(&st, &qid);
    assert!(st.set_fix_preview_missed(&tid, &run).unwrap());
    assert!(events(&st).is_empty());
}

/// That a fix's link reached its reporter, or was rejected, is news of the
/// moment, for whichever notifier is there by then.
#[test]
fn a_notifier_set_after_the_proposal_hears_of_a_link_for_its_reporter() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    let tid = file(&st, &qid);
    let run = held_with_step(&st, &qid, &tid);
    clock.advance(60);
    set(&st, &qid);
    assert!(
        st.set_fix_verify_url(DAEMON, &qid, &tid, &run, "https://pr-7.acme.dev")
            .unwrap()
    );
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixToCheck)]);
}

#[test]
fn a_replay_of_a_waits_end_after_its_notice_was_forgotten_tells_nobody() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    let run = run_on(&st, &qid, &tid);
    st.propose_fix(DAEMON, &qid, &tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    assert!(st.set_fix_preview_missed(&tid, &run).unwrap());
    let due = st.take_due_notices(10).unwrap();
    st.notice_sent(&due[0].id).unwrap();
    clock.advance(PRUNE_SECS + 1);
    assert!(st.take_due_notices(10).unwrap().is_empty());
    assert!(!st.set_fix_preview_missed(&tid, &run).unwrap());
    assert!(st.notices_next_due().unwrap().is_none());
}

#[test]
fn a_rejected_fix_tells_the_queue_whoever_proposed_it_and_a_report_with_none_tells_nobody() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let reject = |tid: &str| {
        let by = Verdict::Person {
            name: "Ana".into(),
            public_id: None,
        };
        st.reopen_report(
            DAEMON,
            &qid,
            tid,
            by,
            "Still wrong".into(),
            None,
            None,
            None,
        )
    };
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), unlinked())
        .unwrap();
    reject(&tid).unwrap();
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixRejected)]);

    let run = run_on(&st, &qid, &tid);
    st.propose_fix(DAEMON, &qid, &tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    reject(&tid).unwrap();
    assert_eq!(events(&st), vec![(2, NoticeEvent::FixRejected)]);
}

#[test]
fn a_confidential_runs_held_fix_needs_a_person_at_once_and_says_so_once() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    let run = run_on(&st, &qid, &tid);
    st.mark_run_confidential(&run).unwrap();
    st.propose_fix(DAEMON, &qid, &tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    assert!(
        st.set_fix_preview(&tid, &run, "https://pr-7.acme.dev")
            .unwrap()
    );
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixHeld)]);
    assert!(st.fix_is_confidential(&tid, 1).unwrap());
}

#[test]
fn a_link_arriving_on_a_held_fix_tells_its_reporter() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    let run = run_on(&st, &qid, &tid);
    st.propose_fix(DAEMON, &qid, &tid, &by_run(&run, "k", true), unlinked())
        .unwrap();
    assert!(!st.fix_is_confidential(&tid, 1).unwrap());
    assert!(
        st.set_fix_verify_url(DAEMON, &qid, &tid, &run, "https://pr-7.acme.dev")
            .unwrap()
    );
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixToCheck)]);
}

#[test]
fn a_run_with_no_row_counts_as_confidential() {
    let (_d, st) = store();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &by_run("gone", "k", true), unlinked())
        .unwrap();
    assert!(st.fix_is_confidential(&tid, 1).unwrap());
    assert_eq!(events(&st), vec![(1, NoticeEvent::FixHeld)]);
}

#[test]
fn links_kept_under_a_proposal_key_are_read_by_its_fix() {
    let (_d, st) = store();
    let qid = queue(&st);
    let tid = file(&st, &qid);
    let links = FixLinks {
        url: Some("https://github.com/a/b/pull/7".into()),
        commit: Some("abc".into()),
        preview_step: Some("To deploy its preview, press go.".into()),
    };
    st.keep_fix_links(&tid, "k", &links).unwrap();
    st.propose_fix(DAEMON, &qid, &tid, &by_run("r", "k", true), unlinked())
        .unwrap();
    assert_eq!(st.fix_links(&tid, 1).unwrap(), Some(links));
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("Mine"))
        .unwrap();
    assert!(st.fix_links(&tid, 2).unwrap().is_none());
}

#[test]
fn saving_a_notifier_ends_the_wait_for_a_test_message() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    let n = set(&st, &qid);
    assert!(st.record_notifier_test(DAEMON, &qid).unwrap());
    assert!(!st.record_notifier_test(DAEMON, &qid).unwrap());
    clock.advance(1);
    let f = ZulipFields {
        if_rev: Some(n.rev),
        ..fields()
    };
    st.set_zulip_notifier(DAEMON, &kek(), &qid, &f, true)
        .unwrap();
    assert!(st.record_notifier_test(DAEMON, &qid).unwrap());
    assert!(!st.record_notifier_test(DAEMON, &qid).unwrap());
}

fn clocked() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-10-27T12:00:00Z"));
    let (d, st) = store_with_clock(clock.clone());
    (d, st, clock)
}

#[test]
fn a_taken_notice_is_nobody_elses_and_a_failure_backs_off() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    let due = st.take_due_notices(10).unwrap();
    assert_eq!(due.len(), 1);
    assert!(
        st.take_due_notices(10).unwrap().is_empty(),
        "held by its taker"
    );
    st.retry_notice(&due[0].id, None).unwrap();
    assert!(st.take_due_notices(10).unwrap().is_empty());
    clock.advance(61);
    let again = st.take_due_notices(10).unwrap();
    assert_eq!(again[0].attempts, 2);
    st.retry_notice(&again[0].id, None).unwrap();
    clock.advance(61);
    assert!(
        st.take_due_notices(10).unwrap().is_empty(),
        "twice as long now"
    );
    clock.advance(60);
    let third = st.take_due_notices(10).unwrap();
    // A server's own wait is honoured when it is the longer.
    st.retry_notice(&third[0].id, Some(1000)).unwrap();
    clock.advance(500);
    assert!(st.take_due_notices(10).unwrap().is_empty());
    clock.advance(501);
    let fourth = st.take_due_notices(10).unwrap();
    st.notice_sent(&fourth[0].id).unwrap();
    clock.advance(100_000);
    assert!(st.take_due_notices(10).unwrap().is_empty());
    assert!(st.notices_next_due().unwrap().is_none());
}

#[test]
fn a_taker_that_died_leaves_its_notice_to_the_next() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    assert_eq!(st.take_due_notices(10).unwrap().len(), 1);
    clock.advance(LEASE_SECS + 1);
    assert_eq!(st.take_due_notices(10).unwrap()[0].attempts, 2);
}

#[test]
fn a_notice_stops_trying_after_a_week_and_the_panel_counts_it() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st);
    set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    assert_eq!(st.zulip_notifier(&qid).unwrap().unwrap().pending, 1);
    clock.advance(KEEP_SECS + 1);
    assert!(st.take_due_notices(10).unwrap().is_empty());
    let n = st.zulip_notifier(&qid).unwrap().unwrap();
    assert_eq!((n.pending, n.expired), (0, 1));
}

#[test]
fn a_saved_notifier_tries_its_notices_at_once_and_forgets_what_failed() {
    let (_d, st) = clocked_store();
    let qid = queue(&st);
    let n = set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    let due = st.take_due_notices(10).unwrap();
    st.retry_notice(&due[0].id, Some(3000)).unwrap();
    st.note_notifier_failure(&qid, &n.rev, NoticeChannel::Stream, "It failed.")
        .unwrap();
    assert_eq!(
        st.zulip_notifier(&qid)
            .unwrap()
            .unwrap()
            .last_failure
            .as_deref(),
        Some("It failed.")
    );
    let f = ZulipFields {
        api_key: Some("a-better-key-0000".into()),
        if_rev: Some(n.rev.clone()),
        ..fields()
    };
    let saved = st
        .set_zulip_notifier(DAEMON, &kek(), &qid, &f, true)
        .unwrap();
    assert!(saved.last_failure.is_none());
    // What an older save was found to do says nothing of this one.
    st.note_notifier_failure(&qid, &n.rev, NoticeChannel::Stream, "It failed.")
        .unwrap();
    assert!(
        st.zulip_notifier(&qid)
            .unwrap()
            .unwrap()
            .last_failure
            .is_none()
    );
    assert_eq!(st.take_due_notices(10).unwrap().len(), 1);
}

fn clocked_store() -> (tempfile::TempDir, Store) {
    let (d, st, _) = clocked();
    (d, st)
}

#[test]
fn removing_a_notifier_or_its_queue_takes_its_notices_and_key() {
    let (_d, st) = store();
    let qid = queue(&st);
    let n = set(&st, &qid);
    let tid = file(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("One"))
        .unwrap();
    st.clear_zulip_notifier(DAEMON, &qid, &n.rev).unwrap();
    assert!(st.notices_next_due().unwrap().is_none());

    set(&st, &qid);
    st.propose_fix(DAEMON, &qid, &tid, &person(), linked("Two"))
        .unwrap();
    assert!(st.notices_next_due().unwrap().is_some());
    st.delete_queue(DAEMON, &qid).unwrap();
    assert!(st.notices_next_due().unwrap().is_none());
    assert!(
        st.secret_meta(&SecretSlot::NotifierZulip(qid))
            .unwrap()
            .is_none()
    );
}

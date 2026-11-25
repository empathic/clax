//! Fixes checked on preview deployments: grants limited to some origins,
//! fix tickets and what they may do, and the reports a preview offers.

use super::{DAEMON, ORIGIN, audit, bodies, clocked, file, fix, session, store, token};
use crate::Store;
use crate::queues::{ExternalScope, FixTicket, NewQueue, ReportWriter, Verdict, Visibility};
use clax_core::CoreError;
use clax_core::audit::{Actor, AuditCtx, Via};
use clax_core::gitctx::GitField;
use clax_core::queue::{FIX_TICKET_SECS, ReportersSee, State};
use clax_core::report::FixOutcome;
use clax_core::secret::Secret;
use serde_json::json;

const PATTERN: &str = "https://app-pr-*.fly.dev";
const PREVIEW: &str = "https://app-pr-538.fly.dev";

/// A queue for [`ORIGIN`] and [`PATTERN`].
fn previews_queue(st: &Store) -> String {
    st.create_queue(
        DAEMON,
        None,
        NewQueue {
            name: "App reports".into(),
            origins: vec![ORIGIN.into(), PATTERN.into()],
            reporters_see: ReportersSee::Own,
        },
    )
    .unwrap()
    .id
}

fn key() -> String {
    clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public())
}

fn ticket_ctx(t: &FixTicket) -> AuditCtx {
    AuditCtx {
        actor: Actor::FixTicket {
            queue_id: t.queue_id.clone(),
            ticket_id: t.id.clone(),
        },
        via: Via::Lan,
        git: GitField::Absent,
        call: None,
    }
}

/// A report of `emp-1` in `qid`, fixed with `preview` by a new agent
/// session: the report, the session and the ticket's token.
fn ticketed(st: &Store, qid: &str) -> (String, String, Secret) {
    let tid = file(st, qid, Some("emp-1"), None);
    let a = session(st, "claude", "fixer");
    st.claim(DAEMON, qid, &tid, &a).unwrap();
    let sa = st.get_session(&a).unwrap().unwrap();
    let (r, t) = st
        .propose_fix_ticketed(
            DAEMON,
            qid,
            &tid,
            &ReportWriter::Agent(&sa),
            fix("Clamp the month"),
            None,
            true,
        )
        .unwrap();
    assert_eq!(r.state, State::FixProposed);
    let t = t.expect("a preview fix is issued a ticket");
    (tid, a, t.token)
}

fn by_token(st: &Store, token: &Secret) -> clax_core::Result<Option<FixTicket>> {
    st.fix_ticket_by_token(&token.sha256_hex())
}

#[test]
fn a_grant_limited_to_the_preview_pattern_mints_for_previews_alone() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (grant, _, _) = st
        .create_service_grant_for(
            DAEMON,
            &qid,
            "previews",
            Some(&["https://APP-PR-*.fly.dev/".to_string()]),
        )
        .unwrap();
    assert_eq!(grant.origins.as_deref(), Some(&[PATTERN.to_string()][..]));
    st.mint_reporter_token(&grant, "emp-1", PREVIEW, &key(), None)
        .unwrap();
    assert!(matches!(
        st.mint_reporter_token(&grant, "emp-1", ORIGIN, &key(), None),
        Err(CoreError::WrongOrigin)
    ));
    // Listed and read back with its origins; the production grant has none.
    let (prod, _, _) = st.create_service_grant(DAEMON, &qid, "prod").unwrap();
    assert_eq!(prod.origins, None);
    st.mint_reporter_token(&prod, "emp-1", ORIGIN, &key(), None)
        .unwrap();
    let listed = st.service_grants(&qid).unwrap();
    assert_eq!(listed[0].origins, grant.origins);
    assert_eq!(listed[1].origins, None);
    assert_eq!(
        bodies(&st, "queue.update")
            .iter()
            .filter(|b| b["op"] == "grant")
            .map(|b| b.get("grant_origins").cloned())
            .collect::<Vec<_>>(),
        [Some(json!([PATTERN])), None]
    );
    // Once the queue drops the pattern, the grant mints for nothing.
    st.update_queue(
        DAEMON,
        &qid,
        crate::queues::QueuePatch {
            origins: Some(vec![ORIGIN.into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        st.mint_reporter_token(&grant, "emp-1", PREVIEW, &key(), None),
        Err(CoreError::WrongOrigin)
    ));
}

#[test]
fn a_grants_origins_are_among_its_queues() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    for bad in [
        vec![],
        vec!["https://other-*.fly.dev".to_string()],
        vec!["https://evil.example".to_string()],
        vec!["nope".to_string()],
    ] {
        let e = st
            .create_service_grant_for(DAEMON, &qid, "previews", Some(&bad))
            .unwrap_err();
        assert!(
            matches!(&e, CoreError::Invalid { code, .. } if *code == "invalid_origin"),
            "{bad:?}: {e:?}"
        );
    }
    assert!(st.service_grants(&qid).unwrap().is_empty());
}

#[test]
fn a_preview_fix_needs_an_origin_pattern_and_writes_nothing_without_one() {
    let (_d, st) = store();
    let qid = super::queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, Some("emp-1"), None);
    let e = st
        .propose_fix_ticketed(DAEMON, &qid, &tid, &super::ana(), fix("Clamp"), None, true)
        .unwrap_err();
    assert!(
        matches!(&e, CoreError::Invalid { code, .. } if *code == "no_preview_origins"),
        "{e:?}"
    );
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert!(r.fixes.is_empty());
    assert_eq!(r.state, State::Open);
}

#[test]
fn a_ticket_is_bound_to_its_fix_and_the_queues_patterns() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (tid, a, token) = ticketed(&st, &qid);
    let prefix = format!("cxf_{qid}.{tid}.1.");
    assert!(token.expose().starts_with(&prefix), "{}", token.expose());
    assert!(token.expose().len() > prefix.len() + 40);
    let t = by_token(&st, &token).unwrap().unwrap();
    assert_eq!(
        (t.queue_id.as_str(), t.thread_id.as_str(), t.n),
        (qid.as_str(), tid.as_str(), 1)
    );
    assert_eq!(t.origins, [PATTERN]);
    assert_eq!(t.issuer_name, "claude");
    assert_eq!(t.issuer_session_id.as_deref(), Some(a.as_str()));
    assert_eq!(t.issuer_person, None);
    // The fix's record names the ticket, never its token.
    let fixes = bodies(&st, "queue.fix");
    assert_eq!(fixes.last().unwrap()["ticket_id"], json!(t.id));
    assert!(
        !serde_json::to_string(&fixes)
            .unwrap()
            .contains(token.expose())
    );
    // Another token, or none, is no ticket.
    assert_eq!(
        by_token(&st, &Secret::new("cxf_nope".into())).unwrap(),
        None
    );
}

#[test]
fn attaching_sets_the_verify_url_notifies_and_records() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (tid, a, token) = ticketed(&st, &qid);
    let t = by_token(&st, &token).unwrap().unwrap();
    let done = st
        .attach_preview(
            &ticket_ctx(&t),
            &t,
            "https://APP-PR-538.fly.dev:443",
            Some("/orders/7?tab=lines"),
        )
        .unwrap();
    let url = format!("{PREVIEW}/orders/7?tab=lines");
    assert_eq!(done.url, url);
    let f = &done.report.fixes[0];
    assert_eq!(f.fix.verify_url.as_deref(), Some(url.as_str()));
    assert_eq!(f.outcome, FixOutcome::Pending);
    assert_eq!(done.report.state, State::FixProposed, "still waiting");
    // The notice is the issuing agent's comment, which the reporter reads.
    let c = done.report.comments.last().unwrap();
    assert_eq!(Some(&c.id), done.comment_id.as_ref());
    assert_eq!(c.author_name, "claude");
    assert!(c.body.contains(&url), "{}", c.body);
    let mine = st
        .report(&qid, &tid, Visibility::Reporter("emp-1"))
        .unwrap()
        .unwrap();
    assert!(mine.comments.iter().any(|x| x.id == c.id));
    let badge = st.reporter_badge(&qid, "emp-1").unwrap();
    assert_eq!(badge.report_ids, std::slice::from_ref(&tid));
    assert_eq!(
        bodies(&st, "queue.preview"),
        [json!({
            "queue_id": qid, "thread_id": tid, "n": 1, "origin": PREVIEW, "url": url,
            "ticket_id": t.id, "issued_by": "claude", "for_person": "owner",
        })]
    );
    let row = &audit(&st, "queue.preview")[0];
    assert_eq!(row["origin"], json!(PREVIEW));
    let _ = a;

    // The same URL again changes nothing; another replaces it.
    let again = st
        .attach_preview(&ticket_ctx(&t), &t, PREVIEW, Some("/orders/7?tab=lines"))
        .unwrap();
    assert_eq!(again.comment_id, None);
    assert_eq!(bodies(&st, "queue.preview").len(), 1);
    let other = st
        .attach_preview(&ticket_ctx(&t), &t, "https://app-pr-539.fly.dev", None)
        .unwrap();
    assert!(other.comment_id.is_some());
    assert_eq!(
        other.report.fixes[0].fix.verify_url.as_deref(),
        Some("https://app-pr-539.fly.dev/")
    );
    assert_eq!(bodies(&st, "queue.preview").len(), 2);
}

#[test]
fn a_ticket_attaches_only_an_origin_its_patterns_match() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (tid, _, token) = ticketed(&st, &qid);
    let t = by_token(&st, &token).unwrap().unwrap();
    let ctx = ticket_ctx(&t);
    // The production origin, another host, a deeper name, another scheme.
    for o in [
        ORIGIN,
        "https://evil.example",
        "https://app-pr-1.evil.fly.dev",
        "http://app-pr-538.fly.dev",
    ] {
        assert!(
            matches!(
                st.attach_preview(&ctx, &t, o, None),
                Err(CoreError::WrongOrigin)
            ),
            "{o}"
        );
    }
    // A pattern, a URL with a path, no origin: not an exact origin.
    for o in [
        PATTERN,
        "https://app-pr-538.fly.dev/x",
        "app-pr-538.fly.dev",
    ] {
        let e = st.attach_preview(&ctx, &t, o, None).unwrap_err();
        assert!(
            matches!(&e, CoreError::Invalid { code, .. } if *code == "invalid_origin"),
            "{o}: {e:?}"
        );
    }
    // A path that leaves the origin, or does not start with one /.
    for p in ["//evil.example/x", "x", "/\\evil.example", "@evil.example"] {
        let e = st.attach_preview(&ctx, &t, PREVIEW, Some(p)).unwrap_err();
        assert!(
            matches!(&e, CoreError::Invalid { code, .. } if *code == "invalid_args"),
            "{p}: {e:?}"
        );
    }
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!(r.fixes[0].fix.verify_url, None, "nothing was attached");
    assert!(bodies(&st, "queue.preview").is_empty());

    // A pattern the queue gains after the ticket is not the ticket's; one
    // the queue drops no longer holds.
    st.update_queue(
        DAEMON,
        &qid,
        crate::queues::QueuePatch {
            origins: Some(vec![ORIGIN.into(), "https://*.vercel.app".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        st.attach_preview(&ctx, &t, "https://app.vercel.app", None),
        Err(CoreError::WrongOrigin)
    ));
    assert!(matches!(
        st.attach_preview(&ctx, &t, PREVIEW, None),
        Err(CoreError::WrongOrigin)
    ));
}

#[test]
fn a_ticket_ends_after_fourteen_days() {
    let (_d, st, clock) = clocked();
    let qid = previews_queue(&st);
    let (_, _, token) = ticketed(&st, &qid);
    let t = by_token(&st, &token).unwrap().unwrap();
    clock.advance(FIX_TICKET_SECS - 1);
    by_token(&st, &token).unwrap().unwrap();
    clock.advance(1);
    assert!(matches!(
        by_token(&st, &token),
        Err(CoreError::TokenExpired)
    ));
    // One read before its end is refused once it has ended.
    assert!(matches!(
        st.attach_preview(&ticket_ctx(&t), &t, PREVIEW, None),
        Err(CoreError::TokenExpired)
    ));
}

#[test]
fn a_ticket_ends_at_its_fixs_verdict() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    // Verified.
    let (tid, _, token) = ticketed(&st, &qid);
    let t = by_token(&st, &token).unwrap().unwrap();
    st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-1"), None, None)
        .unwrap();
    assert!(matches!(
        by_token(&st, &token),
        Err(CoreError::TokenExpired)
    ));
    assert!(matches!(
        st.attach_preview(&ticket_ctx(&t), &t, PREVIEW, None),
        Err(CoreError::TokenExpired)
    ));
    // Rejected: the next fix's ticket is a new one, and the old one stays
    // ended.
    let (tid, a, token) = ticketed(&st, &qid);
    st.reopen_report(
        DAEMON,
        &qid,
        &tid,
        Verdict::Reporter("emp-1"),
        "Still jumps".into(),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(matches!(
        by_token(&st, &token),
        Err(CoreError::TokenExpired)
    ));
    st.claim(DAEMON, &qid, &tid, &a).unwrap();
    let sa = st.get_session(&a).unwrap().unwrap();
    let (_, next) = st
        .propose_fix_ticketed(
            DAEMON,
            &qid,
            &tid,
            &ReportWriter::Agent(&sa),
            fix("Clamp it again"),
            None,
            true,
        )
        .unwrap();
    let next = next.unwrap();
    assert_eq!(next.n, 2);
    assert!(next.token.expose().contains(&format!(".{tid}.2.")));
    by_token(&st, &next.token).unwrap().unwrap();
    assert!(matches!(
        by_token(&st, &token),
        Err(CoreError::TokenExpired)
    ));
}

#[test]
fn a_person_issues_a_ticket_and_its_notice_is_theirs() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let tid = file(&st, &qid, Some("emp-1"), None);
    let (_, t) = st
        .propose_fix_ticketed(DAEMON, &qid, &tid, &super::ana(), fix("Clamp"), None, true)
        .unwrap();
    let t = by_token(&st, &t.unwrap().token).unwrap().unwrap();
    assert_eq!(
        (t.issuer_name.as_str(), t.issuer_session_id.as_deref()),
        ("Ana", None)
    );
    assert_eq!(t.issuer_person.as_deref(), Some("u_ana"));
    let done = st
        .attach_preview(&ticket_ctx(&t), &t, PREVIEW, None)
        .unwrap();
    let c = done.report.comments.last().unwrap();
    assert_eq!(c.author_name, "Ana");
    assert_eq!(
        bodies(&st, "queue.preview")[0]["for_person"],
        json!("u_ana")
    );
}

#[test]
fn verifying_on_a_preview_records_its_origin() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (tid, _, token) = ticketed(&st, &qid);
    let t = by_token(&st, &token).unwrap().unwrap();
    st.attach_preview(&ticket_ctx(&t), &t, PREVIEW, None)
        .unwrap();
    let (r, _) = st
        .verify_report_from(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            None,
            None,
            Some(PREVIEW),
        )
        .unwrap();
    assert_eq!(r.state, State::Verified);
    assert_eq!(r.fixes[0].verified_origin.as_deref(), Some(PREVIEW));
    assert_eq!(
        bodies(&st, "queue.verify").last().unwrap()["origin"],
        json!(PREVIEW)
    );
    let triage = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!(triage.fixes[0].verified_origin.as_deref(), Some(PREVIEW));
}

#[test]
fn a_previews_reports_are_its_reporters_waiting_on_its_origin() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (here, _, token) = ticketed(&st, &qid);
    let t = by_token(&st, &token).unwrap().unwrap();
    st.attach_preview(&ticket_ctx(&t), &t, PREVIEW, Some("/orders"))
        .unwrap();
    // A fix of emp-1's checked elsewhere, and another reporter's here.
    let (elsewhere, _, other) = ticketed(&st, &qid);
    let o = by_token(&st, &other).unwrap().unwrap();
    st.attach_preview(&ticket_ctx(&o), &o, "https://app-pr-539.fly.dev", None)
        .unwrap();
    let theirs = {
        let tok = token_of(&qid, "emp-2");
        let tid = st
            .file_report(
                DAEMON,
                &qid,
                Some(&tok),
                ExternalScope::Instance,
                super::report(None, "The month jumps"),
            )
            .unwrap()
            .id;
        let (_, tk) = st
            .propose_fix_ticketed(DAEMON, &qid, &tid, &super::ana(), fix("x"), None, true)
            .unwrap();
        let tk = by_token(&st, &tk.unwrap().token).unwrap().unwrap();
        st.attach_preview(&ticket_ctx(&tk), &tk, PREVIEW, None)
            .unwrap();
        tid
    };
    let ids = |r: Vec<clax_core::queue::Report>| r.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(
        ids(st.preview_reports(&qid, "emp-1", PREVIEW).unwrap()),
        std::slice::from_ref(&here)
    );
    assert_eq!(
        ids(st
            .preview_reports(&qid, "emp-1", "https://app-pr-539.fly.dev")
            .unwrap()),
        [elsewhere]
    );
    assert_eq!(
        ids(st.preview_reports(&qid, "emp-2", PREVIEW).unwrap()),
        [theirs]
    );
    assert!(
        st.preview_reports(&qid, "emp-1", ORIGIN)
            .unwrap()
            .is_empty()
    );
    // Read as the reporter reads it.
    let r = &st.preview_reports(&qid, "emp-1", PREVIEW).unwrap()[0];
    assert!(r.reporter_profile.is_none());
    // Once verified it is no longer offered.
    st.verify_report(DAEMON, &qid, &here, Verdict::Reporter("emp-1"), None, None)
        .unwrap();
    assert!(
        st.preview_reports(&qid, "emp-1", PREVIEW)
            .unwrap()
            .is_empty()
    );
}

fn token_of(qid: &str, reporter: &str) -> crate::queues::ReporterToken {
    token(qid, reporter, None)
}

#[test]
fn a_grants_damaged_origin_limit_mints_for_nothing() {
    let (_d, st) = store();
    let qid = previews_queue(&st);
    let (grant, _, _) = st
        .create_service_grant_for(DAEMON, &qid, "previews", Some(&[PATTERN.to_string()]))
        .unwrap();
    st.with_tx(|tx| {
        tx.execute(
            "UPDATE queue_service_grants SET origins_json = 'not a list' WHERE id = ?1",
            [&grant.id],
        )?;
        Ok(())
    })
    .unwrap();
    for origin in [PREVIEW, ORIGIN] {
        assert!(
            matches!(
                st.mint_reporter_token(&grant, "emp-1", origin, &key(), None),
                Err(CoreError::WrongOrigin)
            ),
            "{origin}"
        );
    }
    assert_eq!(
        st.service_grants(&qid).unwrap()[0].origins,
        Some(Vec::new())
    );
}

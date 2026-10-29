use super::{
    Badge, ExternalScope, NewQueue, NewReport, ReportWriter, ReporterToken, Verdict, Visibility,
};
use crate::Store;
use crate::test_util::{DAEMON, anchor, session, store, store_with_clock};
use clax_core::CoreError;
use clax_core::queue::{LEASE_SECS, REPORTER_TOKEN_SECS, ReportersSee, Role, State, StateFilter};
use clax_core::report::{
    DiagLine, FixOutcome, NewFix, ReportContext, ReporterProfile, ResolveReason, ServerStamp,
};
use clax_core::working::ManualClock;
use serde_json::json;
use std::sync::Arc;

const ORIGIN: &str = "https://app.example.com";

fn queue(st: &Store, see: ReportersSee) -> String {
    st.create_queue(
        DAEMON,
        None,
        NewQueue {
            name: "App reports".into(),
            origins: vec![ORIGIN.into()],
            reporters_see: see,
        },
    )
    .unwrap()
    .id
}

fn report(external_id: Option<&str>, body: &str) -> NewReport {
    NewReport {
        external_id: external_id.map(str::to_string),
        url: format!("{ORIGIN}/invoices/4821?tab=lines#top"),
        title: "Invoice 4821".into(),
        anchor: anchor(),
        body: body.into(),
        author_name: "Ana".into(),
        metadata: json!({"build": "2026.10.27"}),
        revision: Some("r1".into()),
        clip: None,
        context: None,
        server: None,
        diagnostics: None,
    }
}

/// A reporter token of queue `qid` for `reporter_id`, carrying `profile`.
fn token(qid: &str, reporter_id: &str, profile: Option<ReporterProfile>) -> ReporterToken {
    ReporterToken {
        queue_id: qid.into(),
        service_grant_id: "g1".into(),
        reporter_id: reporter_id.into(),
        origin: ORIGIN.into(),
        public_key: String::new(),
        expires_at: "2026-10-27T12:15:00.000Z".into(),
        profile,
    }
}

fn file(st: &Store, qid: &str, reporter: Option<&str>, external_id: Option<&str>) -> String {
    let tok = reporter.map(|r| token(qid, r, None));
    st.file_report(
        DAEMON,
        qid,
        tok.as_ref(),
        ExternalScope::Instance,
        report(external_id, "The total is wrong"),
    )
    .unwrap()
    .id
}

fn clocked() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-10-27T12:00:00Z"));
    let (d, st) = store_with_clock(clock.clone());
    (d, st, clock)
}

fn audit(st: &Store, kind: &str) -> Vec<serde_json::Value> {
    st.with_read(|c| {
        let rows: Vec<String> = c
            .prepare("SELECT body FROM audit_events WHERE kind = ?1 ORDER BY seq")?
            .query_map([kind], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows
            .iter()
            .map(|b| serde_json::from_str(b).unwrap())
            .collect())
    })
    .unwrap()
}

#[test]
fn claim_then_expiry_frees_report() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, Some("emp-1"), Some("invoice:4821"));
    let a = session(&st, "claude", "a");
    let b = session(&st, "codex", "b");

    let lease = st.claim(DAEMON, &qid, &tid, &a).unwrap();
    assert_eq!(lease.fence, 1);
    assert_eq!(lease.report.state, State::Claimed);
    assert_eq!(st.thread_owner(&tid).unwrap().unwrap().session_id, a);
    // Another session is refused while the lease is live.
    assert!(matches!(
        st.claim(DAEMON, &qid, &tid, &b),
        Err(CoreError::ThreadTaken { .. })
    ));
    assert!(matches!(
        st.check_lease(&qid, &tid, &b),
        Err(CoreError::ThreadTaken { .. })
    ));
    st.check_lease(&qid, &tid, &a).unwrap();

    clock.advance(LEASE_SECS + 1);
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!(
        r.state,
        State::Open,
        "a lapsed lease leaves the report open"
    );
    assert!(r.claim.is_none());
    assert!(matches!(
        st.check_lease(&qid, &tid, &a),
        Err(CoreError::LeaseLost)
    ));
    assert!(
        st.thread_owner(&tid).unwrap().is_none(),
        "the lapsed holder no longer owns the thread"
    );

    let taken = st.claim(DAEMON, &qid, &tid, &b).unwrap();
    assert_eq!(taken.fence, 2, "a new lease has a larger fence");
    assert_eq!(st.thread_owner(&tid).unwrap().unwrap().session_id, b);
    // The first holder's late write is refused, not applied.
    assert!(matches!(
        st.check_lease(&qid, &tid, &a),
        Err(CoreError::LeaseLost)
    ));
    assert!(matches!(
        st.renew(&qid, &tid, &a),
        Err(CoreError::LeaseLost)
    ));
    st.check_lease(&qid, &tid, &b).unwrap();
    let claims = audit(&st, "queue.claim");
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[1]["session"], json!(b));
    assert_eq!(claims[1]["fence"], json!(2));
}

#[test]
fn renew_extends_lease() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, None, None);
    let a = session(&st, "claude", "a");
    let b = session(&st, "codex", "b");
    let first = st.claim(DAEMON, &qid, &tid, &a).unwrap();

    clock.advance(LEASE_SECS - 60);
    let renewed = st.renew(&qid, &tid, &a).unwrap();
    assert_eq!(renewed.lease_id, first.lease_id);
    assert_eq!(renewed.fence, first.fence);
    assert!(renewed.expires_at > first.expires_at);

    // Past the first expiry, inside the renewed one: still held.
    clock.advance(120);
    st.check_lease(&qid, &tid, &a).unwrap();
    assert!(matches!(
        st.claim(DAEMON, &qid, &tid, &b),
        Err(CoreError::ThreadTaken { .. })
    ));

    // `working` on the report renews it too.
    clock.advance(LEASE_SECS - 180);
    assert_eq!(
        st.renew_session_leases(&a, std::slice::from_ref(&tid))
            .unwrap(),
        1
    );
    assert_eq!(
        st.renew_session_leases(&b, std::slice::from_ref(&tid))
            .unwrap(),
        0
    );
    clock.advance(120);
    st.check_lease(&qid, &tid, &a).unwrap();

    // A release opens it to anyone at once.
    st.release(DAEMON, &qid, &tid, &a).unwrap();
    assert!(st.thread_owner(&tid).unwrap().is_none());
    assert_eq!(st.claim(DAEMON, &qid, &tid, &b).unwrap().fence, 2);
}

#[test]
fn an_ended_session_holds_no_lease() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, None, None);
    let a = session(&st, "claude", "a");
    let b = session(&st, "codex", "b");
    st.claim(DAEMON, &qid, &tid, &a).unwrap();
    st.end_session(DAEMON, &a).unwrap();
    assert_eq!(
        st.report(&qid, &tid, Visibility::All)
            .unwrap()
            .unwrap()
            .state,
        State::Open
    );
    st.claim(DAEMON, &qid, &tid, &b).unwrap();
}

#[test]
fn reporter_sees_only_own_under_own() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let mine = file(&st, &qid, Some("emp-1"), Some("a"));
    let theirs = file(&st, &qid, Some("emp-2"), Some("b"));

    let (seen, next) = st
        .reports(&qid, Visibility::Reporter("emp-1"), None, None)
        .unwrap();
    assert_eq!(next, None);
    assert_eq!(
        seen.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        [mine.as_str()]
    );
    assert!(
        st.report(&qid, &theirs, Visibility::Reporter("emp-1"))
            .unwrap()
            .is_none()
    );
    assert!(
        st.reporter_comment(DAEMON, &qid, &theirs, "emp-1", "me too")
            .is_err()
    );
    let (all, _) = st.reports(&qid, Visibility::All, None, None).unwrap();
    assert_eq!(all.len(), 2);

    // The reporter's comment is the reporter's.
    let r = st
        .reporter_comment(DAEMON, &qid, &mine, "emp-1", "Still wrong")
        .unwrap();
    let kinds: Vec<&str> = r.comments.iter().map(|c| c.author_kind.as_str()).collect();
    assert_eq!(kinds, ["reporter", "reporter"]);
    assert_eq!(r.reporter_id.as_deref(), Some("emp-1"));

    // Under `all`, every reporter reads every report.
    st.update_queue(
        DAEMON,
        &qid,
        super::QueuePatch {
            reporters_see: Some(ReportersSee::All),
            ..Default::default()
        },
    )
    .unwrap();
    let (seen, _) = st
        .reports(&qid, Visibility::Reporter("emp-1"), None, None)
        .unwrap();
    assert_eq!(seen.len(), 2);
}

#[test]
fn reports_list_by_state_and_page() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::All);
    let ids: Vec<String> = (0..clax_core::queue::REPORT_PAGE + 2)
        .map(|i| {
            clock.advance(1);
            file(&st, &qid, None, Some(&format!("r{i}")))
        })
        .collect();
    let (first, next) = st.reports(&qid, Visibility::All, None, None).unwrap();
    assert_eq!(first.len(), clax_core::queue::REPORT_PAGE);
    let (rest, end) = st
        .reports(&qid, Visibility::All, None, next.as_deref())
        .unwrap();
    assert_eq!(end, None);
    let all: Vec<String> = first.iter().chain(&rest).map(|r| r.id.clone()).collect();
    assert_eq!(all, ids, "in order of arrival");
    for bad in ["01J0000000000000000000000Z", "nope"] {
        assert!(matches!(
            st.reports(&qid, Visibility::All, None, Some(bad)),
            Err(CoreError::Invalid {
                code: "invalid_cursor",
                ..
            })
        ));
    }

    let a = session(&st, "claude", "a");
    st.claim(DAEMON, &qid, &ids[0], &a).unwrap();
    st.resolve_thread(DAEMON, &ids[1], "agent:claude").unwrap();
    let count = |s| {
        let (mut n, mut cur) = (0, None);
        loop {
            let (page, next) = st
                .reports(
                    &qid,
                    Visibility::All,
                    Some(StateFilter::One(s)),
                    cur.as_deref(),
                )
                .unwrap();
            n += page.len();
            if next.is_none() {
                return n;
            }
            cur = next;
        }
    };
    assert_eq!(count(State::Claimed), 1);
    assert_eq!(count(State::Resolved), 1);
    assert_eq!(count(State::Open), ids.len() - 2);
}

#[test]
fn same_external_id_two_queues_one_artifact_threads_apart() {
    let (_d, st) = store();
    let q1 = queue(&st, ReportersSee::All);
    let q2 = queue(&st, ReportersSee::All);
    let t1 = file(&st, &q1, Some("emp-1"), Some("invoice:4821"));
    let t2 = file(&st, &q2, Some("emp-1"), Some("invoice:4821"));
    let r1 = st.report(&q1, &t1, Visibility::All).unwrap().unwrap();
    let r2 = st.report(&q2, &t2, Visibility::All).unwrap().unwrap();
    assert_eq!(r1.artifact.id, r2.artifact.id, "one artifact");
    assert_eq!(r1.artifact.external_id, "invoice:4821");
    // Each queue reaches its own threads alone.
    assert!(st.report(&q1, &t2, Visibility::All).unwrap().is_none());
    assert!(st.report(&q2, &t1, Visibility::All).unwrap().is_none());
    let (in1, _) = st.reports(&q1, Visibility::All, None, None).unwrap();
    assert_eq!(in1.len(), 1);
    let a = session(&st, "claude", "a");
    assert!(matches!(
        st.claim(DAEMON, &q1, &t2, &a),
        Err(CoreError::NotFound)
    ));
    assert!(
        st.set_thread_metadata(DAEMON, &q1, &t2, json!(1), 1)
            .is_err()
    );
}

#[test]
fn a_report_without_an_external_id_is_keyed_by_its_page() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::All);
    let t = file(&st, &qid, None, None);
    let r = st.report(&qid, &t, Visibility::All).unwrap().unwrap();
    assert_eq!(r.artifact.external_id, format!("{ORIGIN}/invoices/4821"));
    assert_eq!(r.artifact.url, format!("{ORIGIN}/invoices/4821"));
    assert_eq!(r.anchor.route, None);
    assert_eq!(r.metadata, json!({"build": "2026.10.27"}));
    let again = file(&st, &qid, None, None);
    let r2 = st.report(&qid, &again, Visibility::All).unwrap().unwrap();
    assert_eq!(r.artifact.id, r2.artifact.id);
}

#[test]
fn metadata_if_version_conflict() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::All);
    let tid = file(&st, &qid, None, None);
    assert_eq!(
        st.set_thread_metadata(DAEMON, &qid, &tid, json!({"ticket": "A-1"}), 1)
            .unwrap(),
        2
    );
    let err = st
        .set_thread_metadata(DAEMON, &qid, &tid, json!({"ticket": "A-2"}), 1)
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::DocConflict {
                current: Some(2),
                ..
            }
        ),
        "{err:?}"
    );
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!(r.metadata, json!({"ticket": "A-1"}));
    assert_eq!(r.metadata_version, 2);
    let big = json!("x".repeat(clax_core::queue::MAX_THREAD_METADATA_BYTES));
    assert!(matches!(
        st.set_thread_metadata(DAEMON, &qid, &tid, big, 2),
        Err(CoreError::Invalid {
            code: "metadata_too_large",
            ..
        })
    ));
}

#[test]
fn metadata_write_records_audit_with_value_and_hashes() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::All);
    let tid = file(&st, &qid, None, None);
    let before = clax_core::queue::metadata_sha256("{\"build\":\"2026.10.27\"}");
    st.set_thread_metadata(DAEMON, &qid, &tid, json!({"ticket": "A-1"}), 1)
        .unwrap();
    // The same value again writes and records nothing.
    st.set_thread_metadata(DAEMON, &qid, &tid, json!({"ticket": "A-1"}), 2)
        .unwrap();
    let rows = audit(&st, "thread.metadata");
    assert_eq!(
        rows.len(),
        2,
        "the filing's metadata and the write: {rows:?}"
    );
    // The filing journals the metadata the report was filed with.
    assert_eq!(rows[0]["thread_id"], json!(tid));
    assert_eq!(rows[0]["metadata"], json!({"build": "2026.10.27"}));
    assert_eq!(rows[0]["metadata_sha256"], json!(before));
    assert_eq!(
        rows[0]["previous_sha256"],
        json!(clax_core::queue::metadata_sha256("null"))
    );
    assert_eq!(rows[1]["thread_id"], json!(tid));
    assert_eq!(rows[1]["metadata"], json!({"ticket": "A-1"}));
    assert_eq!(
        rows[1]["metadata_sha256"],
        json!(clax_core::queue::metadata_sha256("{\"ticket\":\"A-1\"}"))
    );
    assert_eq!(rows[1]["previous_sha256"], rows[0]["metadata_sha256"]);
    // A report filed without metadata journals none.
    let mut bare = report(None, "No metadata");
    bare.metadata = serde_json::Value::Null;
    st.file_report(DAEMON, &qid, None, ExternalScope::Instance, bare)
        .unwrap();
    assert_eq!(audit(&st, "thread.metadata").len(), 2);
    let made = audit(&st, "external.create");
    assert_eq!(made.len(), 1);
    assert_eq!(
        made[0]["external_id"],
        json!(format!("{ORIGIN}/invoices/4821"))
    );
    assert_eq!(
        made[0]["metadata_sha256"],
        json!(clax_core::queue::metadata_sha256("null"))
    );
}

#[test]
fn reporter_token_expires_after_900s() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let (grant, token, key) = st.create_service_grant(DAEMON, &qid, "backend").unwrap();
    assert!(token.expose().starts_with("cxq_"));
    let found = st
        .service_grant_by_token(&token.sha256_hex())
        .unwrap()
        .unwrap();
    assert_eq!(found.id, grant.id);
    assert_eq!(
        found.public_key,
        clax_core::sig::encode_public(&key.public())
    );
    let reporter_key = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    let (rt, expires_at) = st
        .mint_reporter_token(&grant, "emp-1", ORIGIN, &reporter_key, None)
        .unwrap();
    assert!(rt.expose().starts_with("cxr_"));
    assert_eq!(expires_at, "2026-10-27T12:15:00.000Z");
    let t = st.reporter_by_token(&rt.sha256_hex()).unwrap().unwrap();
    assert_eq!(
        (t.reporter_id.as_str(), t.origin.as_str()),
        ("emp-1", ORIGIN)
    );

    clock.advance(REPORTER_TOKEN_SECS - 1);
    assert!(st.reporter_by_token(&rt.sha256_hex()).unwrap().is_some());
    clock.advance(1);
    assert!(matches!(
        st.reporter_by_token(&rt.sha256_hex()),
        Err(CoreError::TokenExpired)
    ));
    assert!(st.reporter_by_token("00").unwrap().is_none());

    // Revoking the grant ends what it minted.
    let (rt2, _) = st
        .mint_reporter_token(&grant, "emp-1", ORIGIN, &reporter_key, None)
        .unwrap();
    st.revoke_service_grant(DAEMON, &qid, &grant.id).unwrap();
    assert!(st.reporter_by_token(&rt2.sha256_hex()).unwrap().is_none());
    assert!(
        st.service_grant_by_token(&token.sha256_hex())
            .unwrap()
            .is_none()
    );
}

#[test]
fn mint_refuses_origin_not_in_queue() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (grant, _, _) = st.create_service_grant(DAEMON, &qid, "backend").unwrap();
    let key = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    assert!(matches!(
        st.mint_reporter_token(&grant, "emp-1", "https://evil.example", &key, None),
        Err(CoreError::WrongOrigin)
    ));
    // The listed origin as a browser may spell it is accepted.
    st.mint_reporter_token(&grant, "emp-1", "https://APP.example.com:443", &key, None)
        .unwrap();
    assert!(
        st.mint_reporter_token(&grant, "emp-1", ORIGIN, "nope", None)
            .is_err()
    );
}

#[test]
fn members_and_roles() {
    let (_d, st) = store();
    let qid = st
        .create_queue(
            DAEMON,
            Some("u_maker"),
            NewQueue {
                name: " Reports ".into(),
                origins: vec![ORIGIN.into(), "https://app.example.com/".into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap();
    assert_eq!(qid.name, "Reports");
    assert_eq!(qid.origins, [ORIGIN]);
    let qid = qid.id;
    assert_eq!(
        st.queue_role(&qid, Some("u_maker")).unwrap(),
        Some(Role::Admin)
    );
    assert_eq!(st.queue_role(&qid, None).unwrap(), None, "not the owner's");
    assert_eq!(st.queue_role(&qid, Some("u_t")).unwrap(), None);
    st.set_member(DAEMON, &qid, "u_t", Some(Role::Triage))
        .unwrap();
    assert_eq!(
        st.queue_role(&qid, Some("u_t")).unwrap(),
        Some(Role::Triage)
    );
    assert_eq!(st.queues_of(Some("u_t")).unwrap()[0].role, Role::Triage);
    assert!(st.queues_of(None).unwrap().is_empty());
    assert_eq!(
        st.queue_members(&qid).unwrap(),
        [("u_t".to_string(), Role::Triage)]
    );
    st.set_member(DAEMON, &qid, "u_t", None).unwrap();
    assert!(st.queues_of(Some("u_t")).unwrap().is_empty());
    let updates = audit(&st, "queue.update");
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[1]["role"], json!(null));

    st.delete_queue(DAEMON, &qid).unwrap();
    assert!(st.queue(&qid).unwrap().is_none());
    assert!(matches!(
        st.reports(&qid, Visibility::All, None, None),
        Err(CoreError::NotAMember)
    ));
}

#[test]
fn an_agent_writes_only_under_its_own_lease_and_a_person_needs_none() {
    use super::ReportWriter;
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, None, None);
    let a = session(&st, "claude", "a");
    let sa = st.get_session(&a).unwrap().unwrap();
    let agent = ReportWriter::Agent(&sa);
    let replies = |st: &Store| {
        st.report(&qid, &tid, Visibility::All)
            .unwrap()
            .unwrap()
            .comments
            .len()
    };

    // Never claimed: refused, and nothing is written.
    assert!(matches!(
        st.report_reply(DAEMON, &qid, &tid, &agent, "on it"),
        Err(CoreError::ClaimRequired)
    ));
    assert!(matches!(
        st.set_thread_metadata_as(DAEMON, &qid, &tid, Some(&a), json!({"x": 1}), 1),
        Err(CoreError::ClaimRequired)
    ));
    assert_eq!(replies(&st), 1);

    st.claim(DAEMON, &qid, &tid, &a).unwrap();
    let (c, r) = st
        .report_reply(DAEMON, &qid, &tid, &agent, "on it")
        .unwrap();
    assert_eq!(c.author_kind, "agent");
    assert_eq!(r.comments.len(), 2);

    // Lapsed: lease_lost; released: claim_required again.
    clock.advance(LEASE_SECS + 1);
    assert!(matches!(
        st.resolve_report(DAEMON, &qid, &tid, &agent, ResolveReason::Fixed),
        Err(CoreError::LeaseLost)
    ));
    st.claim(DAEMON, &qid, &tid, &a).unwrap();
    st.release(DAEMON, &qid, &tid, &a).unwrap();
    assert!(matches!(
        st.report_reply(DAEMON, &qid, &tid, &agent, "late"),
        Err(CoreError::ClaimRequired)
    ));

    // A person replies and resolves without a claim.
    let person = ReportWriter::Person {
        name: "Ana".into(),
        public_id: Some("u_ana".into()),
    };
    st.report_reply(DAEMON, &qid, &tid, &person, "thanks")
        .unwrap();
    let (t, _, r) = st
        .resolve_report(DAEMON, &qid, &tid, &person, ResolveReason::Duplicate)
        .unwrap();
    assert_eq!(t.resolved_by.as_deref(), Some("viewer:u_ana"));
    assert_eq!(r.state, State::Resolved);
    assert_eq!(r.resolve_reason, Some(ResolveReason::Duplicate));
    let resolves = audit(&st, "thread.resolve");
    assert_eq!(resolves.last().unwrap()["reason"], json!("duplicate"));
    assert_eq!(replies(&st), 3);
}

#[test]
fn a_report_is_in_no_thread_listing() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, Some("emp-1"), Some("invoice:4821"));
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    let aid = clax_core::ArtifactId::parse(&r.artifact.id).unwrap();
    assert!(st.list_threads(&aid, true, None, 50).unwrap().0.is_empty());
    let t = st.get_thread(&tid).unwrap().unwrap();
    assert_eq!(t.queue_id.as_deref(), Some(qid.as_str()));
}

#[test]
fn a_removed_members_sessions_lose_their_leases_and_renew_none() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let t1 = file(&st, &qid, Some("emp-1"), Some("invoice:1"));
    let t2 = file(&st, &qid, Some("emp-1"), Some("invoice:2"));
    st.set_member(DAEMON, &qid, "u_t", Some(Role::Triage))
        .unwrap();
    let a = session(&st, "claude", "a");
    st.with_tx(|tx| {
        tx.execute("UPDATE sessions SET person = 'u_t' WHERE id = ?1", [&a])?;
        Ok(())
    })
    .unwrap();
    st.claim(DAEMON, &qid, &t1, &a).unwrap();
    st.claim(DAEMON, &qid, &t2, &a).unwrap();
    // While a member, working renews the leases.
    clock.advance(100);
    assert_eq!(
        st.renew_session_leases(&a, std::slice::from_ref(&t1))
            .unwrap(),
        1
    );

    st.set_member(DAEMON, &qid, "u_t", None).unwrap();
    for tid in [&t1, &t2] {
        let r = st.report(&qid, tid, Visibility::All).unwrap().unwrap();
        assert_eq!(r.state, State::Open, "{tid}");
        assert!(st.thread_owner(tid).unwrap().is_none(), "{tid}");
        assert!(matches!(
            st.check_lease(&qid, tid, &a),
            Err(CoreError::ClaimRequired)
        ));
    }
    let released = audit(&st, "queue.release");
    assert_eq!(released.len(), 2, "{released:?}");
    assert!(released.iter().all(|r| r["reason"] == "member_removed"));
    // A lease the person's session claims while no member is never renewed.
    st.with_tx(|tx| {
        tx.execute(
            "UPDATE queue_claims SET released = 0 WHERE thread_id = ?1",
            [&t1],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        st.renew_session_leases(&a, &[t1.clone(), t2.clone()])
            .unwrap(),
        0
    );
    // The owner's session on the owner's queue still renews.
    let o = session(&st, "codex", "o");
    st.claim(DAEMON, &qid, &t2, &o).unwrap();
    assert_eq!(
        st.renew_session_leases(&o, std::slice::from_ref(&t2))
            .unwrap(),
        1
    );
}

// The feedback loop (spec 2026-10-29-feedback-loop-design §5, §6, §7.3).

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nclip";

/// The `kind` records' bodies, without the fields every record carries.
fn bodies(st: &Store, kind: &str) -> Vec<serde_json::Value> {
    audit(st, kind)
        .into_iter()
        .map(|mut v| {
            let o = v.as_object_mut().unwrap();
            for k in ["clax_commit", "clax_version", "v", "via"] {
                o.remove(k);
            }
            v
        })
        .collect()
}

fn context(release: &str) -> ReportContext {
    clax_core::report::parse_context(&format!(
        r#"{{"v":1,"captured_at":"2026-10-27T11:59:58.000Z","release":{{"version":"{release}","commit":"f7e862ab0"}},"flags":{{"new_editor":"b"}}}}"#
    ))
    .unwrap()
}

fn diag(text: &str) -> DiagLine {
    DiagLine {
        at: "2026-10-27T11:59:57.000Z".into(),
        level: "error".into(),
        kind: "console".into(),
        text: text.into(),
    }
}

fn profile() -> ReporterProfile {
    ReporterProfile {
        name: Some("Ana Lima".into()),
        email: Some("ana@example.com".into()),
        github: Some("analima".into()),
        avatar_url: None,
    }
}

fn fix(summary: &str) -> NewFix {
    NewFix {
        summary: summary.into(),
        url: Some("https://github.com/acme/app/pull/123".into()),
        commit: Some("f7e862ab0".into()),
        release: Some("0.6.1".into()),
        verify_url: None,
        verify_after: Some("release:0.6.1".into()),
    }
}

fn ana() -> ReportWriter<'static> {
    ReportWriter::Person {
        name: "Ana".into(),
        public_id: Some("u_ana".into()),
    }
}

fn by_ana() -> Verdict<'static> {
    Verdict::Person {
        name: "Ana".into(),
        public_id: Some("u_ana".into()),
    }
}

/// A report of `emp-1` in `qid`, claimed by a new agent session and fixed
/// by it: the report, and the session.
fn fixed(st: &Store, qid: &str) -> (String, String) {
    let tid = file(st, qid, Some("emp-1"), None);
    let a = session(st, "claude", "fixer");
    st.claim(DAEMON, qid, &tid, &a).unwrap();
    let sa = st.get_session(&a).unwrap().unwrap();
    st.propose_fix(
        DAEMON,
        qid,
        &tid,
        &ReportWriter::Agent(&sa),
        fix("Fixed the rounding"),
    )
    .unwrap();
    (tid, a)
}

fn state(st: &Store, qid: &str, tid: &str) -> State {
    st.report(qid, tid, Visibility::All).unwrap().unwrap().state
}

#[test]
fn file_stores_context_server_stamp_diagnostics_and_profile() {
    let (_d, st, _clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    // A minted token keeps its profile, and the token read back carries it.
    let (grant, _, _) = st.create_service_grant(DAEMON, &qid, "backend").unwrap();
    let key = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    let (rt, _) = st
        .mint_reporter_token(&grant, "emp-1", ORIGIN, &key, Some(&profile()))
        .unwrap();
    let tok = st.reporter_by_token(&rt.sha256_hex()).unwrap().unwrap();
    assert_eq!(tok.profile, Some(profile()));
    let bad = ReporterProfile {
        avatar_url: Some("javascript:alert(1)".into()),
        ..ReporterProfile::default()
    };
    assert!(matches!(
        st.mint_reporter_token(&grant, "emp-1", ORIGIN, &key, Some(&bad)),
        Err(CoreError::InvalidProfile(_))
    ));

    let server = ServerStamp {
        version: "0.6.0".into(),
        commit: Some("f7e862ab0".into()),
        build: None,
        received_at: "2026-10-27T12:00:00.000Z".into(),
    };
    let mut r = report(Some("invoice:4821"), "The total is wrong");
    r.context = Some(context("0.6.0"));
    r.server = Some(server.clone());
    r.diagnostics = Some(vec![diag("TypeError: x is undefined"), diag("GET /x 500")]);
    let filed = st
        .file_report(DAEMON, &qid, Some(&tok), ExternalScope::Instance, r)
        .unwrap();
    let got = st
        .report(&qid, &filed.id, Visibility::All)
        .unwrap()
        .unwrap();
    assert_eq!(got.context, Some(context("0.6.0")));
    assert_eq!(
        got.context
            .as_ref()
            .unwrap()
            .release
            .as_ref()
            .unwrap()
            .commit
            .as_deref(),
        Some("f7e862ab0")
    );
    assert_eq!(got.server, Some(server));
    assert_eq!(got.reporter_profile, Some(profile()));
    assert_eq!(got.diagnostics_lines, 2);
    assert_eq!(got.reporter_id.as_deref(), Some("emp-1"));
    assert_eq!((got.reopen_count, got.fixes.len()), (0, 0));
    assert_eq!(
        st.report_diagnostics(&qid, &filed.id, Visibility::All, None)
            .unwrap()
            .unwrap(),
        [diag("TypeError: x is undefined"), diag("GET /x 500")]
    );
    // A report filed with none has none.
    let bare = file(&st, &qid, Some("emp-1"), Some("invoice:1"));
    let b = st.report(&qid, &bare, Visibility::All).unwrap().unwrap();
    assert!(b.context.is_none() && b.server.is_none() && b.reporter_profile.is_none());
    assert_eq!(b.diagnostics_lines, 0);
    assert_eq!(
        st.report_diagnostics(&qid, &bare, Visibility::All, None)
            .unwrap(),
        None
    );
    // More lines than a report holds are refused.
    let mut over = report(None, "Too much");
    over.diagnostics = Some(vec![diag("x"); clax_core::report::MAX_DIAG_LINES + 1]);
    assert!(matches!(
        st.file_report(DAEMON, &qid, None, ExternalScope::Instance, over),
        Err(CoreError::InvalidDiagnostics(_))
    ));
}

#[test]
fn reporter_view_never_carries_profile() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let tok = token(&qid, "emp-1", Some(profile()));
    let filed = st
        .file_report(
            DAEMON,
            &qid,
            Some(&tok),
            ExternalScope::Instance,
            report(None, "Broken"),
        )
        .unwrap();
    assert_eq!(
        filed.reporter_profile, None,
        "the filer reads its report as a reporter"
    );
    let tid = filed.id;
    let mine = Visibility::Reporter("emp-1");
    assert!(
        st.report(&qid, &tid, mine)
            .unwrap()
            .unwrap()
            .reporter_profile
            .is_none()
    );
    let (listed, _) = st.reports(&qid, mine, None, None).unwrap();
    assert!(listed.iter().all(|r| r.reporter_profile.is_none()));
    let commented = st
        .reporter_comment(DAEMON, &qid, &tid, "emp-1", "Still there")
        .unwrap();
    assert!(commented.reporter_profile.is_none());
    // Triage reads it.
    assert_eq!(
        st.report(&qid, &tid, Visibility::All)
            .unwrap()
            .unwrap()
            .reporter_profile,
        Some(profile())
    );
    let (all, _) = st.reports(&qid, Visibility::All, None, None).unwrap();
    assert_eq!(all[0].reporter_profile, Some(profile()));
}

#[test]
fn fix_moves_to_fix_proposed_and_ends_the_lease() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, Some("emp-1"), None);
    let a = session(&st, "claude", "a");
    let lease = st.claim(DAEMON, &qid, &tid, &a).unwrap();
    let sa = st.get_session(&a).unwrap().unwrap();
    let r = st
        .propose_fix(
            DAEMON,
            &qid,
            &tid,
            &ReportWriter::Agent(&sa),
            fix("Fixed the rounding"),
        )
        .unwrap();
    assert_eq!(r.state, State::FixProposed);
    assert_eq!(r.status, "open", "the thread stays open, so replies flow");
    assert!(r.claim.is_none());
    assert_eq!(r.fixes.len(), 1);
    let f = &r.fixes[0];
    assert_eq!((f.n, f.outcome), (1, FixOutcome::Pending));
    assert_eq!(f.fix, fix("Fixed the rounding"));
    assert_eq!(f.proposer_name, "claude");
    assert!(f.verdict_at.is_none());
    let last = r.comments.last().unwrap();
    assert_eq!(
        (last.author_kind.as_str(), last.body.as_str()),
        ("agent", "Fixed the rounding")
    );
    // The reporter sees the summary.
    let seen = st
        .report(&qid, &tid, Visibility::Reporter("emp-1"))
        .unwrap()
        .unwrap();
    assert_eq!(seen.comments.last().unwrap().body, "Fixed the rounding");
    assert_eq!(seen.state, State::FixProposed);
    // The lease ended, as a release ends it.
    assert!(st.thread_owner(&tid).unwrap().is_none());
    assert!(matches!(
        st.check_lease(&qid, &tid, &a),
        Err(CoreError::ClaimRequired)
    ));
    let released = audit(&st, "queue.release");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0]["lease_id"], json!(lease.lease_id));
    assert_eq!(
        bodies(&st, "queue.fix"),
        [json!({
            "queue_id": qid, "thread_id": tid, "n": 1,
            "url": "https://github.com/acme/app/pull/123", "commit": "f7e862ab0"
        })]
    );
    // A triage person proposes without a claim; it is attempt 2.
    let r = st
        .propose_fix(DAEMON, &qid, &tid, &ana(), fix("Also the tax line"))
        .unwrap();
    assert_eq!(r.fixes.iter().map(|f| f.n).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(r.fixes[1].proposer_name, "Ana");
    assert_eq!(
        (r.fixes[0].outcome, r.fixes[1].outcome),
        (FixOutcome::Superseded, FixOutcome::Pending),
        "the earlier fix waits no more"
    );
    assert_eq!(r.state, State::FixProposed);
    assert_eq!(r.comments.last().unwrap().author_kind, "viewer");
    // A bad fix is refused.
    assert!(matches!(
        st.propose_fix(DAEMON, &qid, &tid, &ana(), fix("  ")),
        Err(CoreError::Invalid {
            code: "invalid_fix",
            ..
        })
    ));
}

#[test]
fn fix_needs_the_live_lease() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, None, None);
    let a = session(&st, "claude", "a");
    let sa = st.get_session(&a).unwrap().unwrap();
    let agent = ReportWriter::Agent(&sa);
    assert!(matches!(
        st.propose_fix(DAEMON, &qid, &tid, &agent, fix("Fixed")),
        Err(CoreError::ClaimRequired)
    ));
    st.claim(DAEMON, &qid, &tid, &a).unwrap();
    clock.advance(LEASE_SECS + 1);
    assert!(matches!(
        st.propose_fix(DAEMON, &qid, &tid, &agent, fix("Fixed")),
        Err(CoreError::LeaseLost)
    ));
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!(r.state, State::Open);
    assert!(r.fixes.is_empty());
    assert_eq!(r.comments.len(), 1, "nothing was written");
    assert!(bodies(&st, "queue.fix").is_empty());
}

#[test]
fn claim_of_fix_proposed_is_not_claimable() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, a) = fixed(&st, &qid);
    let b = session(&st, "codex", "b");
    for sid in [&a, &b] {
        assert!(matches!(
            st.claim(DAEMON, &qid, &tid, sid),
            Err(CoreError::NotClaimable)
        ));
    }
    assert_eq!(state(&st, &qid, &tid), State::FixProposed);
    // Nor is a closed report fixed.
    st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-1"), None, None)
        .unwrap();
    assert!(matches!(
        st.propose_fix(DAEMON, &qid, &tid, &ana(), fix("Again")),
        Err(CoreError::NotClaimable)
    ));
    let other = file(&st, &qid, None, Some("other"));
    st.resolve_report(DAEMON, &qid, &other, &ana(), ResolveReason::Wontfix)
        .unwrap();
    assert!(matches!(
        st.propose_fix(DAEMON, &qid, &other, &ana(), fix("Again")),
        Err(CoreError::NotClaimable)
    ));
}

#[test]
fn verify_resolves_with_reason_fixed_and_records_context() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, _) = fixed(&st, &qid);
    let r = st
        .verify_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            Some("Works on 0.6.1".into()),
            Some(context("0.6.1")),
        )
        .unwrap();
    assert_eq!(r.state, State::Verified);
    assert_eq!(r.status, "resolved");
    assert_eq!(r.resolve_reason, Some(ResolveReason::Fixed));
    assert!(r.reporter_profile.is_none(), "the reporter's own view");
    let f = &r.fixes[0];
    assert_eq!(f.outcome, FixOutcome::Verified);
    assert_eq!(f.verdict_note.as_deref(), Some("Works on 0.6.1"));
    assert_eq!(f.verdict_context, Some(context("0.6.1")));
    assert!(f.verdict_at.is_some());
    let t = st.get_thread(&tid).unwrap().unwrap();
    assert_eq!(t.resolved_by.as_deref(), Some("reporter:emp-1"));
    assert_eq!(
        bodies(&st, "queue.verify"),
        [json!({"queue_id": qid, "thread_id": tid, "n": 1})]
    );
    let resolves = audit(&st, "thread.resolve");
    assert_eq!(resolves.last().unwrap()["reason"], json!("fixed"));
    // Listed as verified, not as resolved.
    let (v, _) = st
        .reports(
            &qid,
            Visibility::All,
            Some(StateFilter::One(State::Verified)),
            None,
        )
        .unwrap();
    assert_eq!(v.len(), 1);
    let (res, _) = st
        .reports(
            &qid,
            Visibility::All,
            Some(StateFilter::One(State::Resolved)),
            None,
        )
        .unwrap();
    assert!(res.is_empty());

    // A triage person verifies on the reporter's behalf, as themself.
    let (other, _) = fixed(&st, &qid);
    let r = st
        .verify_report(DAEMON, &qid, &other, by_ana(), None, None)
        .unwrap();
    assert_eq!(r.state, State::Verified);
    assert_eq!(
        st.get_thread(&other)
            .unwrap()
            .unwrap()
            .resolved_by
            .as_deref(),
        Some("viewer:u_ana")
    );
    assert_eq!(r.reporter_profile, None, "this reporter has none");
}

#[test]
fn verify_twice_is_not_fix_proposed() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, _) = fixed(&st, &qid);
    st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-1"), None, None)
        .unwrap();
    assert!(matches!(
        st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-1"), None, None),
        Err(CoreError::NotFixProposed)
    ));
    // Nor is an open report verified.
    let open = file(&st, &qid, Some("emp-1"), Some("o"));
    assert!(matches!(
        st.verify_report(DAEMON, &qid, &open, by_ana(), None, None),
        Err(CoreError::NotFixProposed)
    ));
    assert_eq!(bodies(&st, "queue.verify").len(), 1);
}

#[test]
fn reopen_from_fix_proposed_rejects_attempt_and_counts() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, a) = fixed(&st, &qid);
    let (r, woken) = st
        .reopen_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            "Still wrong on 0.6.1".into(),
            Some(PNG.to_vec()),
            Some(vec![diag("still failing")]),
            Some(context("0.6.1")),
        )
        .unwrap();
    assert_eq!(
        woken.targets.iter().collect::<Vec<_>>(),
        [&a],
        "the fix's claimant is woken"
    );
    assert_eq!(r.state, State::Open);
    assert_eq!(r.status, "open");
    assert_eq!(r.reopen_count, 1);
    let f = &r.fixes[0];
    assert_eq!(f.outcome, FixOutcome::Rejected);
    assert_eq!(f.verdict_note.as_deref(), Some("Still wrong on 0.6.1"));
    assert_eq!(f.verdict_context, Some(context("0.6.1")));
    let last = r.comments.last().unwrap();
    assert_eq!(
        (last.author_kind.as_str(), last.body.as_str()),
        ("reporter", "Still wrong on 0.6.1")
    );
    assert_eq!(
        (last.capture, last.has_clip, last.diagnostics_lines),
        (Some(1), true, 1)
    );
    assert!(!r.has_clip, "the filing had no clip, and still has none");
    assert_eq!(
        st.block_on(st.report_comment_clip_async(&qid, &tid, &last.id, Visibility::All))
            .unwrap()
            .as_deref(),
        Some(PNG)
    );
    assert_eq!(r.diagnostics_lines, 1);
    assert_eq!(
        st.report_diagnostics(&qid, &tid, Visibility::Reporter("emp-1"), None)
            .unwrap()
            .unwrap(),
        [diag("still failing")]
    );
    assert_eq!(
        bodies(&st, "queue.reopen"),
        [json!({"queue_id": qid, "thread_id": tid, "n": 1, "reopen_count": 1})]
    );
    // Open again: claimable, and fixable again as attempt 2.
    let b = session(&st, "codex", "b");
    st.claim(DAEMON, &qid, &tid, &b).unwrap();
    let sb = st.get_session(&b).unwrap().unwrap();
    let r = st
        .propose_fix(
            DAEMON,
            &qid,
            &tid,
            &ReportWriter::Agent(&sb),
            fix("Take two"),
        )
        .unwrap();
    assert_eq!(r.fixes[1].n, 2);
    let (_, woken) = st
        .reopen_report(
            DAEMON,
            &qid,
            &tid,
            by_ana(),
            "Not yet".into(),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(woken.targets.iter().collect::<Vec<_>>(), [&b]);
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!(r.reopen_count, 2);
    assert_eq!(r.diagnostics_lines, 1, "kept when none are given");
    // An ended proposer's session is not woken.
    st.claim(DAEMON, &qid, &tid, &b).unwrap();
    st.propose_fix(DAEMON, &qid, &tid, &ReportWriter::Agent(&sb), fix("Three"))
        .unwrap();
    st.end_session(DAEMON, &b).unwrap();
    let (_, woken) = st
        .reopen_report(DAEMON, &qid, &tid, by_ana(), "No".into(), None, None, None)
        .unwrap();
    assert!(woken.targets.is_empty(), "{woken:?}");
    // A bad clip writes nothing.
    let (other, _) = fixed(&st, &qid);
    assert!(matches!(
        st.reopen_report(
            DAEMON,
            &qid,
            &other,
            by_ana(),
            "x".into(),
            Some(b"nope".to_vec()),
            None,
            None
        ),
        Err(CoreError::Invalid {
            code: "invalid_clip",
            ..
        })
    ));
    assert_eq!(state(&st, &qid, &other), State::FixProposed);
}

#[test]
fn reopen_from_verified_reopens_thread() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, _) = fixed(&st, &qid);
    st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-1"), None, None)
        .unwrap();
    let (r, _) = st
        .reopen_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            "It came back".into(),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(r.state, State::Open);
    assert_eq!(r.status, "open");
    assert_eq!(r.resolve_reason, None);
    assert_eq!(r.reopen_count, 1);
    assert_eq!(
        r.fixes[0].outcome,
        FixOutcome::Verified,
        "a verified attempt keeps its verdict"
    );
    let t = st.get_thread(&tid).unwrap().unwrap();
    assert_eq!((t.status.as_str(), t.resolved_by), ("open", None));
    // From resolved too.
    let other = file(&st, &qid, Some("emp-1"), Some("other"));
    st.resolve_report(DAEMON, &qid, &other, &ana(), ResolveReason::CannotReproduce)
        .unwrap();
    let (r, woken) = st
        .reopen_report(
            DAEMON,
            &qid,
            &other,
            by_ana(),
            "Here it is".into(),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!((r.state, r.reopen_count), (State::Open, 1));
    assert!(woken.targets.is_empty(), "never claimed: no one to wake");
    assert!(r.fixes.is_empty());
    assert_eq!(
        bodies(&st, "queue.reopen").last().unwrap(),
        &json!({"queue_id": qid, "thread_id": other, "reopen_count": 1})
    );
}

#[test]
fn reopen_of_open_is_not_fix_proposed() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let tid = file(&st, &qid, Some("emp-1"), None);
    let reopen = |st: &Store| {
        st.reopen_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            "?".into(),
            None,
            None,
            None,
        )
    };
    assert!(matches!(reopen(&st), Err(CoreError::NotFixProposed)));
    let a = session(&st, "claude", "a");
    st.claim(DAEMON, &qid, &tid, &a).unwrap();
    assert!(matches!(reopen(&st), Err(CoreError::NotFixProposed)));
    let r = st.report(&qid, &tid, Visibility::All).unwrap().unwrap();
    assert_eq!((r.reopen_count, r.comments.len()), (0, 1));
}

#[test]
fn under_own_a_reporter_cannot_verify_anothers_report() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, _) = fixed(&st, &qid);
    assert!(matches!(
        st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-2"), None, None),
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        st.reopen_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-2"),
            "no".into(),
            None,
            None,
            None
        ),
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        st.report_diagnostics(&qid, &tid, Visibility::Reporter("emp-2"), None),
        Err(CoreError::NotFound)
    ));
    assert_eq!(state(&st, &qid, &tid), State::FixProposed);
    // Under `all` another reporter reads it, but the verdict is the filer's.
    st.update_queue(
        DAEMON,
        &qid,
        super::QueuePatch {
            reporters_see: Some(ReportersSee::All),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        st.report(&qid, &tid, Visibility::Reporter("emp-2"))
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-2"), None, None),
        Err(CoreError::NotFound)
    ));
}

#[test]
fn active_filter_lists_open_and_claimed_only() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let mut next = 0;
    let mut one = |st: &Store| {
        clock.advance(1);
        next += 1;
        file(st, &qid, Some("emp-1"), Some(&format!("r{next}")))
    };
    let open = one(&st);
    let claimed = one(&st);
    let a = session(&st, "claude", "a");
    st.claim(DAEMON, &qid, &claimed, &a).unwrap();
    let proposed = one(&st);
    st.propose_fix(DAEMON, &qid, &proposed, &ana(), fix("Fixed"))
        .unwrap();
    let verified = one(&st);
    st.propose_fix(DAEMON, &qid, &verified, &ana(), fix("Fixed"))
        .unwrap();
    st.verify_report(DAEMON, &qid, &verified, by_ana(), None, None)
        .unwrap();
    let resolved = one(&st);
    st.resolve_report(DAEMON, &qid, &resolved, &ana(), ResolveReason::Duplicate)
        .unwrap();
    let ids = |f: Option<StateFilter>| {
        st.reports(&qid, Visibility::All, f, None)
            .unwrap()
            .0
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(Some(StateFilter::Active)),
        [open.clone(), claimed.clone()]
    );
    for (s, want) in [
        (State::Open, &open),
        (State::Claimed, &claimed),
        (State::FixProposed, &proposed),
        (State::Verified, &verified),
        (State::Resolved, &resolved),
    ] {
        assert_eq!(ids(Some(StateFilter::One(s))), [want.as_str()], "{s:?}");
        assert_eq!(state(&st, &qid, want), s);
    }
    assert_eq!(ids(None).len(), 5);
}

#[test]
fn badge_counts_fix_proposed_and_unseen_replies() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::All);
    let quiet = file(&st, &qid, Some("emp-1"), Some("quiet"));
    let replied = file(&st, &qid, Some("emp-1"), Some("replied"));
    let theirs = file(&st, &qid, Some("emp-2"), Some("theirs"));
    assert_eq!(
        st.reporter_badge(&qid, "emp-1").unwrap(),
        Badge {
            waiting: 0,
            report_ids: vec![]
        }
    );
    clock.advance(1);
    st.report_reply(DAEMON, &qid, &replied, &ana(), "Which invoice?")
        .unwrap();
    st.report_reply(DAEMON, &qid, &theirs, &ana(), "Which invoice?")
        .unwrap();
    // The reporter's own comment is no reply to it.
    st.reporter_comment(DAEMON, &qid, &quiet, "emp-1", "More detail")
        .unwrap();
    let (proposed, _) = fixed(&st, &qid);
    let badge = st.reporter_badge(&qid, "emp-1").unwrap();
    assert_eq!(badge.waiting, 2);
    assert_eq!(badge.report_ids, [replied.clone(), proposed.clone()]);
    // Under `all`, another reporter's comment is one.
    st.reporter_comment(DAEMON, &qid, &quiet, "emp-2", "Me too")
        .unwrap();
    assert_eq!(
        st.reporter_badge(&qid, "emp-1").unwrap().report_ids,
        [quiet.clone(), replied.clone(), proposed.clone()]
    );
    assert_eq!(
        st.reporter_badge(&qid, "emp-2").unwrap().report_ids,
        [theirs]
    );
    assert_eq!(st.reporter_badge(&qid, "emp-3").unwrap().waiting, 0);
    assert!(matches!(
        st.reporter_badge("01J0000000000000000000000Z", "emp-1"),
        Err(CoreError::NotAMember)
    ));
}

#[test]
fn mark_seen_clears_reply_badge_but_not_fix_proposed() {
    let (_d, st, clock) = clocked();
    let qid = queue(&st, ReportersSee::Own);
    let replied = file(&st, &qid, Some("emp-1"), Some("replied"));
    clock.advance(1);
    st.report_reply(DAEMON, &qid, &replied, &ana(), "Which invoice?")
        .unwrap();
    let (proposed, _) = fixed(&st, &qid);
    assert_eq!(st.reporter_badge(&qid, "emp-1").unwrap().waiting, 2);

    clock.advance(1);
    st.mark_reporter_seen(&qid, &replied, "emp-1").unwrap();
    st.mark_reporter_seen(&qid, &proposed, "emp-1").unwrap();
    assert_eq!(
        st.reporter_badge(&qid, "emp-1").unwrap().report_ids,
        [proposed.as_str()],
        "a fix waiting for a verdict waits until it has one"
    );
    // A later reply counts again.
    clock.advance(1);
    st.report_reply(DAEMON, &qid, &replied, &ana(), "Got it")
        .unwrap();
    assert_eq!(
        st.reporter_badge(&qid, "emp-1").unwrap().report_ids,
        [replied.clone(), proposed.clone()]
    );
    // The verdict clears the fix.
    st.verify_report(
        DAEMON,
        &qid,
        &proposed,
        Verdict::Reporter("emp-1"),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        st.reporter_badge(&qid, "emp-1").unwrap().report_ids,
        [replied.as_str()]
    );
    // Another reporter's mark is refused where it does not see the report.
    assert!(matches!(
        st.mark_reporter_seen(&qid, &replied, "emp-2"),
        Err(CoreError::NotFound)
    ));
    assert_eq!(st.reporter_badge(&qid, "emp-1").unwrap().waiting, 1);
}

#[test]
fn diagnostics_are_per_queue() {
    let (_d, st) = store();
    let q1 = queue(&st, ReportersSee::Own);
    let q2 = queue(&st, ReportersSee::Own);
    let mut r = report(Some("invoice:4821"), "Broken");
    r.diagnostics = Some(vec![diag("boom")]);
    let t1 = st
        .file_report(DAEMON, &q1, None, ExternalScope::Instance, r)
        .unwrap()
        .id;
    assert_eq!(
        st.report_diagnostics(&q1, &t1, Visibility::All, None)
            .unwrap()
            .unwrap(),
        [diag("boom")]
    );
    // Through another queue the report is not there.
    assert!(matches!(
        st.report_diagnostics(&q2, &t1, Visibility::All, None),
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        st.report_diagnostics("01J0000000000000000000000Z", &t1, Visibility::All, None),
        Err(CoreError::NotAMember)
    ));
    st.delete_queue(DAEMON, &q1).unwrap();
    assert!(matches!(
        st.report_diagnostics(&q1, &t1, Visibility::All, None),
        Err(CoreError::NotAMember)
    ));
}

#[test]
fn watchers_list_and_unwatch() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let other = queue(&st, ReportersSee::Own);
    let a = session(&st, "claude", "a");
    let b = session(&st, "codex", "b");
    assert!(st.queue_watchers(&qid).unwrap().is_empty());
    st.watch_queue(&qid, &a).unwrap();
    st.watch_queue(&qid, &a).unwrap();
    st.watch_queue(&qid, &b).unwrap();
    st.watch_queue(&other, &b).unwrap();
    let mut w = st.queue_watchers(&qid).unwrap();
    w.sort();
    let mut both = vec![a.clone(), b.clone()];
    both.sort();
    assert_eq!(w, both);
    st.unwatch_queue(&qid, &a).unwrap();
    st.unwatch_queue(&qid, &a).unwrap();
    assert_eq!(st.queue_watchers(&qid).unwrap(), [b.as_str()]);
    // An ended session watches nothing.
    st.end_session(DAEMON, &b).unwrap();
    assert!(st.queue_watchers(&qid).unwrap().is_empty());
    assert!(st.queue_watchers(&other).unwrap().is_empty());
    assert!(matches!(
        st.watch_queue(&qid, &b),
        Err(CoreError::Invalid {
            code: "unknown_session",
            ..
        })
    ));
    assert!(matches!(
        st.watch_queue("01J0000000000000000000000Z", &a),
        Err(CoreError::NotAMember)
    ));
}

#[test]
fn comments_leave_a_verified_or_resolved_report_as_it_is() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (verified, _) = fixed(&st, &qid);
    st.verify_report(
        DAEMON,
        &qid,
        &verified,
        Verdict::Reporter("emp-1"),
        None,
        None,
    )
    .unwrap();
    let resolved = file(&st, &qid, Some("emp-1"), Some("resolved"));
    st.resolve_report(DAEMON, &qid, &resolved, &ana(), ResolveReason::Wontfix)
        .unwrap();
    for (tid, want) in [(&verified, State::Verified), (&resolved, State::Resolved)] {
        let r = st
            .reporter_comment(DAEMON, &qid, tid, "emp-1", "Back again")
            .unwrap();
        assert_eq!((r.state, r.status.as_str()), (want, "resolved"), "{want:?}");
        let (_, r) = st.report_reply(DAEMON, &qid, tid, &ana(), "Noted").unwrap();
        assert_eq!((r.state, r.reopen_count), (want, 0), "{want:?}");
        assert!(r.resolve_reason.is_some());
    }
    assert!(audit(&st, "thread.reopen").is_empty());
    assert!(bodies(&st, "queue.reopen").is_empty());
}

#[test]
fn a_resolve_closes_the_fix_waiting_for_a_verdict() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let (tid, _) = fixed(&st, &qid);
    let (_, _, r) = st
        .resolve_report(DAEMON, &qid, &tid, &ana(), ResolveReason::Duplicate)
        .unwrap();
    assert_eq!(r.state, State::Resolved);
    assert_eq!(r.fixes[0].outcome, FixOutcome::Closed);
    // A reopen from resolved leaves the closed attempt as it is.
    let (r, _) = st
        .reopen_report(
            DAEMON,
            &qid,
            &tid,
            by_ana(),
            "Not a duplicate".into(),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        (r.state, r.fixes[0].outcome),
        (State::Open, FixOutcome::Closed)
    );
    assert_eq!(audit(&st, "thread.reopen").len(), 1);
    // A resolve however it comes ends the wait.
    let (other, _) = fixed(&st, &qid);
    st.resolve_thread(DAEMON, &other, "agent:claude").unwrap();
    let r = st.report(&qid, &other, Visibility::All).unwrap().unwrap();
    assert_eq!(
        (r.state, r.fixes[0].outcome),
        (State::Resolved, FixOutcome::Closed)
    );
}

#[test]
fn every_capture_keeps_its_clip_and_diagnostics() {
    let (_d, st) = store();
    let qid = queue(&st, ReportersSee::Own);
    let first: &[u8] = b"\x89PNG\r\n\x1a\nfiled";
    let second: &[u8] = b"\x89PNG\r\n\x1a\nreopen-1";
    let third: &[u8] = b"\x89PNG\r\n\x1a\nreopen-2";
    let mut r = report(Some("invoice:9"), "Broken");
    r.clip = Some(first.to_vec());
    r.diagnostics = Some(vec![diag("filed")]);
    let tok = token(&qid, "emp-1", None);
    let tid = st
        .file_report(DAEMON, &qid, Some(&tok), ExternalScope::Instance, r)
        .unwrap()
        .id;
    let reopen = |body: &str, clip: &[u8], diags: Option<Vec<DiagLine>>| {
        st.propose_fix(DAEMON, &qid, &tid, &ana(), fix("Try this"))
            .unwrap();
        st.reopen_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            body.into(),
            Some(clip.to_vec()),
            diags,
            None,
        )
        .unwrap()
        .0
    };
    reopen("Still", second, Some(vec![diag("one"), diag("two")]));
    let r = reopen("Again", third, None);
    let clip = |cid: Option<&str>| match cid {
        None => st.block_on(st.report_clip_async(&qid, &tid, Visibility::All)),
        Some(c) => st.block_on(st.report_comment_clip_async(&qid, &tid, c, Visibility::All)),
    };
    assert_eq!(
        clip(None).unwrap().as_deref(),
        Some(first),
        "the filing's clip stays"
    );
    let captures: Vec<_> = r
        .comments
        .iter()
        .filter(|c| c.capture.is_some())
        .map(|c| (c.capture, c.has_clip, c.diagnostics_lines, c.id.clone()))
        .collect();
    assert_eq!(captures.len(), 2);
    assert_eq!(
        (captures[0].0, captures[0].1, captures[0].2),
        (Some(1), true, 2)
    );
    assert_eq!(
        (captures[1].0, captures[1].1, captures[1].2),
        (Some(2), true, 0)
    );
    assert_eq!(clip(Some(&captures[0].3)).unwrap().as_deref(), Some(second));
    assert_eq!(clip(Some(&captures[1].3)).unwrap().as_deref(), Some(third));
    // The filing comment carries no capture and no clip of its own.
    assert_eq!(clip(Some(&r.comments[0].id)).unwrap(), None);
    assert!(matches!(clip(Some("nope")), Err(CoreError::NotFound)));
    assert_eq!(r.diagnostics_lines, 2, "the latest capture holding any");
    let diags = |capture| {
        st.report_diagnostics(&qid, &tid, Visibility::All, capture)
            .unwrap()
    };
    assert_eq!(diags(None).unwrap(), [diag("one"), diag("two")]);
    assert_eq!(diags(Some(0)).unwrap(), [diag("filed")]);
    assert_eq!(diags(Some(1)).unwrap().len(), 2);
    assert_eq!(diags(Some(2)), None);
    // Deleting the report removes every clip it held.
    let keys: Vec<String> = st
        .with_read(|c| {
            Ok(c.prepare(
                "SELECT clip_key FROM comments WHERE thread_id = ?1 AND clip_key IS NOT NULL",
            )?
            .query_map([&tid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(keys.len(), 2);
    let aid = clax_core::ArtifactId::parse(&r.artifact.id).unwrap();
    for key in &keys {
        assert!(
            st.block_on(st.content.read_clip(&aid, key))
                .unwrap()
                .is_some()
        );
    }
    st.delete_thread(DAEMON, &tid).unwrap();
    for key in &keys {
        assert_eq!(
            st.block_on(st.content.read_clip(&aid, key)).unwrap(),
            None,
            "{key}"
        );
    }
}

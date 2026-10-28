use super::{NewQueue, NewReport, Visibility};
use crate::Store;
use crate::test_util::{DAEMON, anchor, session, store, store_with_clock};
use clax_core::CoreError;
use clax_core::queue::{LEASE_SECS, REPORTER_TOKEN_SECS, ReportersSee, Role, State};
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
    }
}

fn file(st: &Store, qid: &str, reporter: Option<&str>, external_id: Option<&str>) -> String {
    st.file_report(
        DAEMON,
        qid,
        reporter,
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
                .reports(&qid, Visibility::All, Some(s), cur.as_deref())
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
    assert_eq!(rows.len(), 2, "the filing's metadata and the write: {rows:?}");
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
    st.file_report(DAEMON, &qid, None, bare).unwrap();
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
        .mint_reporter_token(&grant, "emp-1", ORIGIN, &reporter_key)
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
        .mint_reporter_token(&grant, "emp-1", ORIGIN, &reporter_key)
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
        st.mint_reporter_token(&grant, "emp-1", "https://evil.example", &key),
        Err(CoreError::WrongOrigin)
    ));
    // The listed origin as a browser may spell it is accepted.
    st.mint_reporter_token(&grant, "emp-1", "https://APP.example.com:443", &key)
        .unwrap();
    assert!(
        st.mint_reporter_token(&grant, "emp-1", ORIGIN, "nope")
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
        st.resolve_report(DAEMON, &qid, &tid, &agent),
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
    let (t, _, r) = st.resolve_report(DAEMON, &qid, &tid, &person).unwrap();
    assert_eq!(t.resolved_by.as_deref(), Some("viewer:u_ana"));
    assert_eq!(r.state, State::Resolved);
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
    assert_eq!(st.renew_session_leases(&a, &[t1.clone()]).unwrap(), 1);

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
    assert_eq!(st.renew_session_leases(&a, &[t1.clone(), t2.clone()]).unwrap(), 0);
    // The owner's session on the owner's queue still renews.
    let o = session(&st, "codex", "o");
    st.claim(DAEMON, &qid, &t2, &o).unwrap();
    assert_eq!(st.renew_session_leases(&o, &[t2.clone()]).unwrap(), 1);
}

use super::{Ask, REQUEST_LIFE_S, Requester, Status};
use crate::Store;
use crate::inbox::{InboxQuery, Kind};
use crate::test_util::{DAEMON, store_with_clock};
use clax_core::CoreError;
use clax_core::queue::{ReportersSee, Role};
use clax_core::working::ManualClock;
use std::sync::Arc;

const T0: &str = "2026-11-24T09:00:00Z";

fn store() -> (tempfile::TempDir, Arc<ManualClock>, Store) {
    let clock = Arc::new(ManualClock::at(T0));
    let (dir, st) = store_with_clock(clock.clone());
    (dir, clock, st)
}

/// The owner's agents, naming no session.
fn owners_agent() -> Requester {
    Requester::default()
}

fn ask(name: &str) -> Ask {
    Ask {
        name: name.into(),
        origins: vec![
            "https://app.example.com".into(),
            "https://app-pr-*.example.com".into(),
        ],
        reporters_see: ReportersSee::Own,
        note: Some("  for the invoices app  ".into()),
    }
}

fn code(e: CoreError) -> &'static str {
    match e {
        CoreError::Invalid { code, .. } => code,
        CoreError::NotFound => "not_found",
        other => panic!("unexpected {other:?}"),
    }
}

/// The owner's unread `queue_request` items.
fn unread_requests(st: &Store) -> usize {
    st.inbox_list(&InboxQuery {
        limit: 200,
        ..Default::default()
    })
    .unwrap()
    .0
    .into_iter()
    .filter(|i| i.kind == Kind::QueueRequest && i.read_at.is_none())
    .count()
}

#[test]
fn a_request_waits_for_its_person_who_approves_it_into_a_queue_of_theirs() {
    let (_d, _c, st) = store();
    let r = st
        .request_queue(DAEMON, &owners_agent(), ask("Invoices"))
        .unwrap();
    assert_eq!(r.status, Status::Pending);
    assert_eq!(r.note.as_deref(), Some("for the invoices app"));
    assert_eq!(r.agent, "An agent");
    assert_eq!(r.person, None);
    assert_eq!(r.expires_at, "2026-11-24T10:00:00.000Z");
    assert_eq!(unread_requests(&st), 1, "its person is told");
    // Nothing is made before the answer, and a pending take takes nothing.
    assert!(st.queues_of(None).unwrap().is_empty());
    let t = st
        .take_queue_request(DAEMON, &r.id, &owners_agent())
        .unwrap();
    assert!(t.grant.is_none());
    assert_eq!(t.request.status, Status::Pending);

    let (r, q) = st.approve_queue_request(DAEMON, &r.id, None).unwrap();
    assert_eq!(r.status, Status::Approved);
    assert_eq!(r.queue_id.as_deref(), Some(q.id.as_str()));
    assert_eq!(q.role, Role::Admin);
    assert_eq!(q.name, "Invoices");
    assert_eq!(unread_requests(&st), 0, "its item is read once answered");
    assert_eq!(st.queues_of(None).unwrap().len(), 1);
    assert!(
        st.service_grants(&q.id).unwrap().is_empty(),
        "no grant until the agent takes it"
    );

    // The agent's first take makes the one service grant; the next makes none.
    let t = st
        .take_queue_request(DAEMON, &r.id, &owners_agent())
        .unwrap();
    let (grant, token, _key) = t.grant.expect("the grant, once");
    assert!(
        token
            .expose()
            .starts_with(clax_core::queue::SERVICE_TOKEN_PREFIX)
    );
    assert_eq!(grant.name, "Requested by An agent");
    assert!(t.request.taken_at.is_some());
    let again = st
        .take_queue_request(DAEMON, &r.id, &owners_agent())
        .unwrap();
    assert!(again.grant.is_none());
    assert_eq!(st.service_grants(&q.id).unwrap().len(), 1);

    // Answered once, it stays answered.
    assert_eq!(
        code(st.deny_queue_request(DAEMON, &r.id, None).unwrap_err()),
        "request_closed"
    );
    assert_eq!(
        code(st.approve_queue_request(DAEMON, &r.id, None).unwrap_err()),
        "request_closed"
    );
}

#[test]
fn a_denied_request_makes_nothing() {
    let (_d, _c, st) = store();
    let r = st
        .request_queue(DAEMON, &owners_agent(), ask("Invoices"))
        .unwrap();
    let r = st.deny_queue_request(DAEMON, &r.id, None).unwrap();
    assert_eq!(r.status, Status::Denied);
    assert!(r.decided_at.is_some());
    let t = st
        .take_queue_request(DAEMON, &r.id, &owners_agent())
        .unwrap();
    assert!(t.grant.is_none());
    assert!(st.queues_of(None).unwrap().is_empty());
    assert_eq!(unread_requests(&st), 0);
}

#[test]
fn a_request_is_only_its_persons_to_answer_and_its_agents_to_read() {
    let (_d, _c, st) = store();
    let r = st
        .request_queue(DAEMON, &owners_agent(), ask("Invoices"))
        .unwrap();
    let other = Requester {
        person: Some("p_sam".into()),
        grant_id: Some("g_sam".into()),
        session_id: None,
    };
    assert_eq!(
        code(st.take_queue_request(DAEMON, &r.id, &other).unwrap_err()),
        "not_found"
    );
    assert_eq!(
        code(
            st.approve_queue_request(DAEMON, &r.id, Some("p_sam"))
                .unwrap_err()
        ),
        "not_found"
    );
    assert_eq!(
        code(
            st.deny_queue_request(DAEMON, &r.id, Some("p_sam"))
                .unwrap_err()
        ),
        "not_found"
    );
    assert_eq!(st.queue_requests_for(Some("p_sam"), None).unwrap(), vec![]);
    assert_eq!(
        st.queue_requests_for(None, Some(Status::Pending))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        st.queue_requests_for(None, Some(Status::Denied))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn an_unanswered_request_expires_after_its_life_on_the_stores_clock() {
    let (_d, clock, st) = store();
    let r = st
        .request_queue(DAEMON, &owners_agent(), ask("Invoices"))
        .unwrap();
    clock.advance(REQUEST_LIFE_S - 1);
    assert_eq!(
        st.queue_request(&r.id).unwrap().unwrap().status,
        Status::Pending
    );
    clock.advance(1);
    assert_eq!(
        st.queue_request(&r.id).unwrap().unwrap().status,
        Status::Expired
    );
    assert_eq!(unread_requests(&st), 0);
    assert_eq!(
        code(st.approve_queue_request(DAEMON, &r.id, None).unwrap_err()),
        "request_closed"
    );
}

#[test]
fn bad_fields_and_too_many_waiting_are_refused() {
    let (_d, _c, st) = store();
    let mut a = ask(" ");
    assert_eq!(
        code(
            st.request_queue(DAEMON, &owners_agent(), a.clone())
                .unwrap_err()
        ),
        "invalid_name"
    );
    a = ask("Invoices");
    a.origins = vec!["ftp://nope".into()];
    assert_eq!(
        code(st.request_queue(DAEMON, &owners_agent(), a).unwrap_err()),
        "invalid_origin"
    );
    a = ask("Invoices");
    a.note = Some("x".repeat(super::MAX_NOTE_CHARS + 1));
    assert_eq!(
        code(st.request_queue(DAEMON, &owners_agent(), a).unwrap_err()),
        "invalid_args"
    );
    for i in 0..super::MAX_PENDING {
        st.request_queue(DAEMON, &owners_agent(), ask(&format!("Q{i}")))
            .unwrap();
    }
    assert_eq!(
        code(
            st.request_queue(DAEMON, &owners_agent(), ask("One more"))
                .unwrap_err()
        ),
        "limit_reached"
    );
    let unknown = Requester {
        grant_id: Some("g_gone".into()),
        ..Requester::default()
    };
    assert_eq!(
        code(st.request_queue(DAEMON, &unknown, ask("Q")).unwrap_err()),
        "grant_revoked"
    );
}

#[test]
fn a_queue_deleted_before_the_read_hands_over_no_grant() {
    let (_d, _c, st) = store();
    let r = st
        .request_queue(DAEMON, &owners_agent(), ask("Invoices"))
        .unwrap();
    let (_, q) = st.approve_queue_request(DAEMON, &r.id, None).unwrap();
    st.delete_queue(DAEMON, &q.id).unwrap();
    let t = st
        .take_queue_request(DAEMON, &r.id, &owners_agent())
        .unwrap();
    assert!(t.grant.is_none());
    assert_eq!(t.request.status, Status::Approved);
}

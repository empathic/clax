use super::*;
use crate::bot_runs::{Bindings, DriverId, NewRunOf, StepCommit, StepKind, StoredBinding};
use crate::bot_surfaces::NewRegistration;
use crate::credits::KeySource;
use crate::roles::OWNER;
use crate::test_util::{DAEMON, store_with_clock};
use clax_core::working::{Clock, ManualClock};
use serde_json::json;
use std::sync::Arc;

const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const BOUNDS: MessageBounds = MessageBounds {
    undelivered: 3,
    bytes: 64,
};

/// A store on a stopped clock, two registrations of `shell`, and a run
/// of `shell.triage` the first one started.
struct Env {
    _dir: tempfile::TempDir,
    st: Store,
    clock: Arc<ManualClock>,
    shell: String,
    other: String,
    run: String,
}

impl Env {
    fn new() -> Env {
        let clock = Arc::new(ManualClock::at("2026-11-11T10:00:00Z"));
        let (dir, st) = store_with_clock(clock.clone());
        st.put_surface_def(HASH, "{}", clock.now()).unwrap();
        let register = || {
            st.register_surface(
                DAEMON,
                NewRegistration {
                    name: "shell".into(),
                    def_hash: HASH.into(),
                    prompt: None,
                    settings: json!({}),
                    registered_by: OWNER.into(),
                    replaces: None,
                },
                clock.now(),
            )
            .unwrap()
            .id
        };
        let (shell, other) = (register(), register());
        let bindings = Bindings::bound(vec![StoredBinding::bound(
            "shell",
            Some(HASH.into()),
            Some(shell.clone()),
            None,
            Value::Null,
            vec![],
        )])
        .unwrap();
        let run = st
            .create_run_of(
                DAEMON,
                NewRunOf {
                    kind: "shell.triage".into(),
                    request: json!({"ask": "Tidy up"}),
                    bindings,
                    person: OWNER.into(),
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    started_by: shell.clone(),
                    parent_run: None,
                    agent_grant: None,
                },
            )
            .unwrap()
            .id;
        Env {
            _dir: dir,
            st,
            clock,
            shell,
            other,
            run,
        }
    }

    fn send(&self, body: Value, key: Option<&str>) -> Result<Appended> {
        self.send_as(&self.shell, body, key)
    }

    fn send_as(&self, registration: &str, body: Value, key: Option<&str>) -> Result<Appended> {
        self.st.append_message(
            NewMessage {
                run_id: &self.run,
                registration_id: registration,
                body: &body,
                key,
            },
            BOUNDS,
        )
    }

    fn seqs(&self) -> Vec<i64> {
        self.st
            .undelivered_messages(&self.run)
            .unwrap()
            .iter()
            .map(|m| m.seq)
            .collect()
    }

    /// Claims the run as `d1` and begins step `n` as `kind`.
    fn begin(&self, n: i64, kind: StepKind) {
        let d = DriverId::named("d1");
        self.st.claim_run(&self.run, &d, 60).unwrap();
        self.st.begin_step(&self.run, &d, n, kind).unwrap();
    }

    fn commit(&self, n: i64, delivered: Vec<i64>) -> Result<crate::bot_runs::Committed> {
        self.st.commit_bot_step(
            DAEMON,
            &self.run,
            &DriverId::named("d1"),
            n,
            StepCommit {
                payload: json!({}),
                delivered,
                ..StepCommit::default()
            },
        )
    }
}

fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
    match r {
        Err(CoreError::Invalid { code, .. }) => code,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn messages_are_numbered_in_the_order_they_arrive_and_listed_so() {
    let env = Env::new();
    let first = env.send(json!({"say": "one"}), None).unwrap();
    env.clock.advance(1);
    let second = env.send(json!({"say": "two"}), None).unwrap();
    assert!(first.new && second.new && !first.waiting);
    assert_eq!((first.message.seq, second.message.seq), (1, 2));
    let listed = env.st.undelivered_messages(&env.run).unwrap();
    assert_eq!(listed[0], first.message);
    assert_eq!(listed[1].body, json!({"say": "two"}));
    assert_eq!(listed[1].created_at, "2026-11-11T10:00:01.000Z");
    assert_eq!(listed[1].registration_id, env.shell);
}

#[test]
fn a_key_names_one_message_and_another_body_under_it_is_refused() {
    let env = Env::new();
    let sent = env.send(json!({"say": "one"}), Some("k1")).unwrap();
    let again = env.send(json!({"say": "one"}), Some("k1")).unwrap();
    assert_eq!(
        again,
        Appended {
            message: sent.message.clone(),
            new: false,
            waiting: false
        }
    );
    assert_eq!(
        code(env.send(json!({"say": "other"}), Some("k1"))),
        CODE_KEY_REUSED
    );
    // Without a key the same body is a new message.
    env.send(json!({"say": "one"}), None).unwrap();
    env.send(json!({"say": "one"}), Some("k2")).unwrap();
    assert_eq!(env.seqs(), [1, 2, 3]);
}

#[test]
fn only_the_starter_sends_while_the_run_lives_within_its_bounds() {
    let env = Env::new();
    assert_eq!(
        code(env.send_as(&env.other, json!(1), None)),
        CODE_NOT_THE_STARTER
    );
    assert!(matches!(
        env.st.append_message(
            NewMessage {
                run_id: "no-such-run",
                registration_id: &env.shell,
                body: &json!(1),
                key: None,
            },
            BOUNDS,
        ),
        Err(CoreError::NotFound)
    ));
    let long = json!("x".repeat(BOUNDS.bytes));
    assert_eq!(code(env.send(long, None)), CODE_MESSAGE_TOO_LARGE);
    for n in 0..3 {
        env.send(json!(n), Some(&format!("k{n}"))).unwrap();
    }
    assert_eq!(code(env.send(json!(3), None)), CODE_TOO_MANY_MESSAGES);
    // A key it holds is answered whatever the bounds say now.
    assert!(matches!(
        env.send(json!(0), Some("k0")),
        Ok(Appended { new: false, .. })
    ));
    assert_eq!(env.seqs(), [1, 2, 3]);

    // Delivered, they make room.
    env.begin(1, StepKind::Script);
    env.commit(1, vec![1, 2]).unwrap();
    env.send(json!(4), None).unwrap();
    assert_eq!(env.seqs(), [3, 4]);

    // An ended run takes none, but answers a key it holds.
    env.st.cancel_run(DAEMON, &env.run).unwrap();
    env.begin(2, StepKind::Stopped);
    env.commit(2, vec![]).unwrap();
    assert_eq!(code(env.send(json!(5), None)), CODE_RUN_ENDED);
    assert!(matches!(
        env.send(json!(1), Some("k1")),
        Ok(Appended { new: false, .. })
    ));
}

#[test]
fn a_message_is_delivered_by_the_commit_of_the_step_that_took_it_and_by_no_other() {
    let env = Env::new();
    env.send(json!("a"), None).unwrap();
    env.send(json!("b"), None).unwrap();
    env.begin(1, StepKind::Script);
    // Begun and not committed (a crash): both still wait.
    assert_eq!(env.seqs(), [1, 2]);
    env.clock.advance(5);
    env.commit(1, vec![1]).unwrap();
    assert_eq!(env.seqs(), [2]);
    let all = env.st.messages_of(&env.run).unwrap();
    assert_eq!(
        (all[0].delivered_step, all[0].delivered_at.as_deref()),
        (Some(1), Some("2026-11-11T10:00:05.000Z"))
    );
    // A commit that names a message delivered already commits nothing.
    env.begin(2, StepKind::Script);
    assert_eq!(code(env.commit(2, vec![2, 1])), "run_invalid");
    assert_eq!(env.st.open_step(&env.run).unwrap().map(|s| s.n), Some(2));
    assert_eq!(env.seqs(), [2]);
    env.commit(2, vec![2]).unwrap();
    assert!(env.seqs().is_empty());
}

/// A delivered message goes with the run's journal; one it was never
/// given is kept for as long as the run's end is kept for its
/// registration, so the end can name it.
#[test]
fn an_ended_runs_messages_go_with_its_journal_and_those_never_given_with_its_end() {
    let env = Env::new();
    env.send(json!("a"), None).unwrap();
    env.send(json!("b"), None).unwrap();
    env.begin(1, StepKind::Script);
    env.commit(1, vec![1]).unwrap();
    env.st.cancel_run(DAEMON, &env.run).unwrap();
    env.begin(2, StepKind::Stopped);
    env.commit(2, vec![]).unwrap();
    env.clock.advance(23 * 3600);
    assert_eq!(env.st.prune_ended_runs().unwrap().messages, 0);
    env.clock.advance(3601);
    assert_eq!(env.st.prune_ended_runs().unwrap().messages, 1);
    assert_eq!(env.seqs(), [2]);
    env.clock.advance(6 * 24 * 3600);
    assert_eq!(env.st.prune_ended_runs().unwrap().messages, 1);
    assert!(env.st.messages_of(&env.run).unwrap().is_empty());
}

/// One pass of `prune_ended_runs` takes all of an ended run's journal
/// keeps at once: its content and the blob no other item names, and the
/// messages it was given, each counted in what it answers, while the
/// message it never gave stays until the run's end goes.
#[test]
fn one_prune_takes_an_ended_runs_content_and_given_messages_and_keeps_the_rest() {
    use crate::bot_content::{ContentLimits, NewContent};
    let env = Env::new();
    let item = env
        .st
        .put_bot_content(
            &NewContent {
                id: "c1",
                run_id: &env.run,
                surface: "shell",
                registration_id: Some(&env.shell),
                name: None,
                media_type: "text/plain",
                width: None,
                height: None,
                bytes: b"notes",
            },
            ContentLimits::default(),
        )
        .unwrap();
    env.send(json!("a"), None).unwrap();
    env.send(json!("b"), None).unwrap();
    env.begin(1, StepKind::Script);
    env.commit(1, vec![1]).unwrap();
    env.st.cancel_run(DAEMON, &env.run).unwrap();
    env.begin(2, StepKind::Stopped);
    env.commit(2, vec![]).unwrap();
    env.clock.advance(24 * 3600 + 1);
    let pruned = env.st.prune_ended_runs().unwrap();
    assert_eq!(
        (pruned.contents, pruned.blobs, pruned.messages),
        (1, 1, 1),
        "{pruned:?}"
    );
    assert_eq!(env.st.bot_content("c1").unwrap(), None);
    assert_eq!(env.st.bot_content_bytes(&item).unwrap(), None);
    assert_eq!(env.seqs(), [2], "the message never given is kept");
    env.clock.advance(7 * 24 * 3600);
    let pruned = env.st.prune_ended_runs().unwrap();
    assert_eq!(
        (pruned.contents, pruned.blobs, pruned.messages),
        (0, 0, 1),
        "{pruned:?}"
    );
    assert!(env.st.messages_of(&env.run).unwrap().is_empty());
}

/// A message to a parked run leaves it parked, on the call it waits on
/// and with its parks as they were: it says the run waits.
#[test]
fn a_message_to_a_parked_run_leaves_it_parked() {
    use crate::bot_pending::NewCall;
    use crate::bot_runs::{JournalPlace, Park, RunStatus};
    let env = Env::new();
    env.begin(1, StepKind::Script);
    let d = DriverId::named("d1");
    env.st
        .open_call(
            &env.run,
            &d,
            NewCall {
                step_n: 1,
                seq: 0,
                attempt: 1,
                registration_id: env.shell.clone(),
                def_hash: HASH.into(),
                op: "shell.ask".into(),
                args: json!(["Which?"]),
                args_hash: "h".into(),
                key: format!("{}:1:0", env.run),
                access: crate::bot_pending::CallAccess::Read,
                waits: true,
                expires_at: Some(env.clock.now() + chrono::Duration::hours(24)),
            },
            None,
            env.clock.now(),
        )
        .unwrap();
    let parked = env
        .st
        .park_run(
            &env.run,
            &d,
            JournalPlace { step_n: 1, seq: 0 },
            &json!({"surface": "shell"}),
            None,
            env.clock.now(),
        )
        .unwrap();
    assert!(matches!(parked, Park::Parked(_)));
    assert_eq!(env.st.run_parking(&env.run).unwrap().unwrap().parks, 1);
    let before = env.st.run(&env.run).unwrap().unwrap();
    let sent = env.send(json!("wake up"), Some("w")).unwrap();
    assert!(sent.new && sent.waiting);
    assert_eq!(env.st.run(&env.run).unwrap().unwrap(), before);
    assert_eq!(before.status, RunStatus::Parked);
    let parking = env.st.run_parking(&env.run).unwrap().unwrap();
    assert_eq!(parking.parks, 1);
    assert_eq!(parking.waiting_on, Some(json!({"surface": "shell"})));
    // The call it waits on is still open.
    let call = env.st.pending_call_at(&env.run, 1, 0).unwrap().unwrap();
    assert!(call.answered_at.is_none());
    // Sent again, it changes nothing, and still waits.
    let again = env.send(json!("wake up"), Some("w")).unwrap();
    assert!(!again.new && again.waiting);
    assert_eq!(env.seqs(), [1]);
    // Once the run is no longer parked it is answered as it was first:
    // `waiting`, as the host was told.
    env.st.unpark_run(&env.run, env.clock.now()).unwrap();
    assert_ne!(
        env.st.run(&env.run).unwrap().unwrap().status,
        RunStatus::Parked
    );
    let later = env.send(json!("wake up"), Some("w")).unwrap();
    assert!(!later.new && later.waiting);
    assert_eq!(later.message, sent.message);
}

/// A message whose stored body does not read as JSON is refused, never
/// read as `null`: its body is an error, it is listed undelivered, a step's
/// commit that would deliver it is refused, and the key it was sent
/// under is refused rather than called reused.
#[test]
fn a_message_whose_stored_body_does_not_read_is_refused_and_never_delivered() {
    let env = Env::new();
    env.send(json!("a"), Some("k1")).unwrap();
    env.send(json!("b"), None).unwrap();
    env.st
        .with_write(|c| {
            c.execute(
                "UPDATE bot_messages SET body_json = '{\"half' WHERE run_id = ?1 AND seq = 1",
                params![env.run],
            )?;
            Ok(())
        })
        .unwrap();
    let held = env.st.undelivered_messages(&env.run).unwrap();
    assert_eq!(held.iter().map(|m| m.seq).collect::<Vec<_>>(), [1, 2]);
    assert!(
        matches!(held[0].body(), Err(CoreError::Db(_))),
        "{:?}",
        held[0].body()
    );
    assert_eq!(held[1].body().unwrap(), &json!("b"));
    assert!(matches!(
        env.send(json!("a"), Some("k1")),
        Err(CoreError::Db(_))
    ));
    env.begin(1, StepKind::Script);
    assert_eq!(code(env.commit(1, vec![1, 2])), "run_invalid");
    env.commit(1, vec![2]).unwrap();
    assert_eq!(env.seqs(), [1], "the unreadable message stays undelivered");
}

/// A run's end kept for its registration whose stored end or result does
/// not read as JSON is refused, never told as a `null` end or no result.
#[test]
fn a_kept_end_that_does_not_read_is_refused() {
    let env = Env::new();
    env.st.cancel_run(DAEMON, &env.run).unwrap();
    env.begin(1, StepKind::Stopped);
    env.st
        .commit_bot_step_on(
            DAEMON,
            &env.run,
            &DriverId::named("d1"),
            1,
            StepCommit {
                payload: json!({}),
                ..StepCommit::default()
            },
            None,
            &[crate::bot_pending::EndFor {
                registration_id: env.shell.clone(),
                end: json!({"kind": "stopped"}),
                result: Some(json!({"ok": true})),
            }],
        )
        .unwrap();
    assert!(env.st.kept_end(&env.run, &env.shell).unwrap().is_some());
    for (column, text) in [("end_json", "nope"), ("result_json", "{")] {
        env.st
            .with_write(|c| {
                c.execute(
                    &format!("UPDATE bot_run_ends SET {column} = ?2 WHERE run_id = ?1"),
                    params![env.run, text],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            matches!(env.st.kept_end(&env.run, &env.shell), Err(CoreError::Db(_))),
            "{column}"
        );
    }
}

//! A held browser confirmation offered to the owner's passkey through chitin
//! (`clax_server::confirm_chitin`): queued as an item for the owner's title
//! with its binding, answered by a passkey approval or a decline chitin
//! records in the title's flow log, once, whichever of that and the browser
//! comes first. chitin is a fake here: a script that queues and lists, and
//! a flow log the test appends to as chitin-approve does.

use clax_core::working::ManualClock;
use clax_testkit::{TestServer, error_code as code};
use reqwest::Method;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A chitin that queues item `000000000001`, `…2`, … (`queued <id>`, as
/// chitin prints it), keeping each run's arguments as `say.<n>`, and lists
/// `pending.json` beside it. Run with no arguments, it prints the usage of a
/// chitin that takes the queue's brief (approve-into-fold 710bddd).
const FAKE_CHITIN: &str = r#"#!/bin/sh
d=$(dirname "$0")
case "$1" in
  "") echo 'usage: chitin ... | say --as <title> --queue --ask <line> --why <text> --effect <text> --undo yes|no|partly [--undo-how <text>] <to> <text> | ...' >&2; exit 2 ;;
  say)
    n=$(( $(cat "$d/count" 2>/dev/null || echo 0) + 1 ))
    echo "$n" > "$d/count"
    printf '%s\n' "$@" > "$d/say.$n"
    printf '%s|%s\n' "$CHITIN_TITLE" "$CHITIN_STATE_DIR" > "$d/env"
    printf 'queued %012x\n' "$n" ;;
  pending) cat "$d/pending.json" 2>/dev/null || echo '[]' ;;
  *) exit 2 ;;
esac
"#;

struct Rig {
    ts: TestServer,
    dir: tempfile::TempDir,
}

impl Rig {
    fn bin_dir(&self) -> PathBuf {
        self.dir.path().join("bin")
    }
    fn log(&self) -> PathBuf {
        self.dir.path().join("state/log/bobby.jsonl")
    }
    /// The arguments of the `n`th queue, once it has run.
    async fn queued(&self, n: u32) -> Vec<String> {
        let p = self.bin_dir().join(format!("say.{n}"));
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&p)
                && s.ends_with('\n')
                && s.lines().count() >= 16
            {
                return s.lines().map(str::to_string).collect();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("chitin was not asked to queue #{n}");
    }
    /// Appends chitin's flow-log line for item `n` (`outcome`, `extra`).
    fn flow(&self, n: u32, outcome: &str, extra: Value) {
        let mut l = json!({"ts": "2026-11-02T20:00:00.000Z", "side": "sent", "op": "queue",
            "from": "bobby", "to": "clax", "outcome": outcome, "ref": format!("{n:012x}")});
        l.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(self.log())
            .unwrap();
        writeln!(f, "{l}").unwrap();
    }
    /// Confirmation `id`, once its status is no longer `open` (or after
    /// `wait` seconds).
    async fn settled(&self, id: &str, wait: u32) -> Value {
        let (s, v) = self
            .ts
            .token_call(
                Method::GET,
                &format!("/api/confirmations/{id}?wait={wait}"),
                json!(null),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["confirmation"].clone()
    }
}

/// A daemon with `[sign]` naming the fake chitin and an empty flow log for
/// `bobby`, an agent live, and `clock` (or the real one).
async fn rig(clock: Option<Arc<ManualClock>>, pending: Option<&str>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(dir.path().join("state/log")).unwrap();
    std::fs::write(dir.path().join("state/log/bobby.jsonl"), "").unwrap();
    if let Some(p) = pending {
        std::fs::write(bin.join("pending.json"), p).unwrap();
    }
    let exe = clax_fake_exe::install(&bin.join("chitin"), FAKE_CHITIN);
    let sign = Arc::new(clax_core::config::SignConfig {
        chitin: exe.to_string_lossy().into_owned(),
        as_title: "bobby".into(),
        state_dir: Some(dir.path().join("state").to_string_lossy().into_owned()),
        confirm_to: Some("clax".into()),
    });
    let f = move |s: &mut clax_http::state::AppState| s.sign = Some(sign);
    let ts = match clock {
        Some(c) => TestServer::spawn_with_clock_and(c, f).await,
        None => TestServer::spawn_with(f).await,
    };
    ts.register_session("claude", "c-1").await;
    Rig { ts, dir }
}

async fn page(ts: &TestServer, title: &str) -> String {
    ts.publish(title, &[("index.html", "<h1>hi</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn open_world(ts: &TestServer, aid: &str, confirm: Option<&str>) -> (u16, Value) {
    ts.token_call(
        Method::PATCH,
        &format!("/api/artifacts/{aid}"),
        json!({"site_access": "world"}),
        confirm,
    )
    .await
}

/// The confirmation a 202 asked for.
fn held((status, v): &(u16, Value)) -> String {
    assert_eq!(*status, 202, "{v}");
    v["confirmation_id"].as_str().unwrap().to_string()
}

/// The decision confirmation `id` records, read with the token.
async fn decision(ts: &TestServer, id: &str) -> Value {
    let (_, v) = ts
        .token_call(
            Method::GET,
            &format!("/api/confirmations/{id}"),
            json!(null),
            None,
        )
        .await;
    v["confirmation"]["decision"].clone()
}

#[tokio::test]
async fn a_held_decision_is_queued_for_the_passkey_and_its_approval_lets_it_through_once() {
    let r = rig(None, None).await;
    let aid = page(&r.ts, "Held").await;
    let id = held(&open_world(&r.ts, &aid, None).await);
    let args = r.queued(1).await;
    let h = clax_http::confirm::binding(&id, &decision(&r.ts, &id).await);
    // Queued as the owner's title, to clax, with chitin's four-field brief,
    // as CHITIN_TITLE=clax under the configured state dir.
    assert_eq!(&args[..5], ["say", "--as", "bobby", "--queue", "--ask"]);
    assert_eq!(args[5], "Confirm in Clax: open “Held”'s site to the world");
    assert_eq!(
        &args[12..15],
        ["--undo-how", clax_server::confirm_chitin::UNDO_HOW, "clax"]
    );
    let text = args[15..].join("\n");
    assert_eq!(
        text,
        format!(
            "Confirm in Clax: open “Held”'s site to the world\nAsked with Clax's daemon token while this agent was live: claude.\nclax confirmation {id} widen #{h}"
        )
    );
    let env = std::fs::read_to_string(r.bin_dir().join("env")).unwrap();
    assert_eq!(
        env,
        format!("clax|{}\n", r.dir.path().join("state").display())
    );
    // Asked again, the same confirmation: not queued again.
    assert_eq!(held(&open_world(&r.ts, &aid, None).await), id);
    // Lines that are not a passkey's approval of this item answer nothing:
    // Touch ID at the Mac, a refused assertion, another item, another title.
    r.flow(1, "approved", json!({}));
    r.flow(1, "approve-refused", json!({"signer": "webauthn"}));
    r.flow(9, "approved", json!({"signer": "webauthn"}));
    r.flow(1, "approved", json!({"signer": "webauthn", "from": "alex"}));
    assert_eq!(r.settled(&id, 3).await["status"], "open");
    assert!(!std::path::Path::new(&r.bin_dir().join("say.2")).exists());
    // The passkey's approval answers it, as the browser would have.
    r.flow(1, "approved", json!({"signer": "webauthn"}));
    assert_eq!(r.settled(&id, 10).await["status"], "approved");
    let (s, v) = open_world(&r.ts, &aid, Some(&id)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(r.ts.site_get_anon(&aid, "/").await.status(), 200);
    // Once: the same approval again, and the browser after it, change nothing.
    r.flow(1, "approved", json!({"signer": "webauthn"}));
    let res = r.ts.confirm_raw(&id, false).await;
    assert_eq!(res.status(), 409);
    let c = r.settled(&id, 0).await;
    assert_eq!(c["status"], "approved");
    assert!(c["used_at"].is_string(), "{c}");
}

#[tokio::test]
async fn a_decline_from_chitin_refuses_the_decision() {
    let r = rig(None, None).await;
    let aid = page(&r.ts, "Kept").await;
    let id = held(&open_world(&r.ts, &aid, None).await);
    r.queued(1).await;
    // `chitin decline` at the Mac logs no signer; a passkey's logs webauthn.
    r.flow(1, "declined", json!({"to": "-"}));
    assert_eq!(r.settled(&id, 10).await["status"], "declined");
    let (s, v) = open_world(&r.ts, &aid, Some(&id)).await;
    assert_eq!((s, code(&v)), (403, "declined"));
}

#[tokio::test]
async fn the_browser_answering_first_leaves_the_passkey_nothing_to_answer() {
    let r = rig(None, None).await;
    let aid = page(&r.ts, "Either").await;
    let id = held(&open_world(&r.ts, &aid, None).await);
    r.queued(1).await;
    r.ts.confirm(&id, false).await;
    r.flow(1, "approved", json!({"signer": "webauthn"}));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(r.settled(&id, 0).await["status"], "declined");
    let (s, v) = open_world(&r.ts, &aid, Some(&id)).await;
    assert_eq!((s, code(&v)), (403, "declined"));
}

#[tokio::test]
async fn an_approval_after_the_confirmation_expired_answers_nothing() {
    let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
    let r = rig(Some(clock.clone()), None).await;
    let aid = page(&r.ts, "Late").await;
    let id = held(&open_world(&r.ts, &aid, None).await);
    r.queued(1).await;
    clock.advance(clax_http::confirm::CONFIRMATION_TTL_SECS);
    r.flow(1, "approved", json!({"signer": "webauthn"}));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(r.settled(&id, 0).await["status"], "expired");
    let (s, v) = open_world(&r.ts, &aid, Some(&id)).await;
    assert_eq!((s, code(&v)), (410, "confirmation_expired"));
}

#[tokio::test]
async fn each_confirmation_is_its_own_item_and_one_approval_answers_its_own() {
    let r = rig(None, None).await;
    let a = page(&r.ts, "A").await;
    let b = page(&r.ts, "B").await;
    let ca = held(&open_world(&r.ts, &a, None).await);
    let qa = r.queued(1).await;
    let cb = held(&open_world(&r.ts, &b, None).await);
    let qb = r.queued(2).await;
    assert!(qa.last().unwrap().contains(&ca) && qb.last().unwrap().contains(&cb));
    // B's item approved: B's confirmation, not A's.
    r.flow(2, "approved", json!({"signer": "webauthn"}));
    assert_eq!(r.settled(&cb, 10).await["status"], "approved");
    assert_eq!(r.settled(&ca, 0).await["status"], "open");
    // B's approval presented for A's decision is refused as another decision's.
    let (s, v) = open_world(&r.ts, &a, Some(&cb)).await;
    assert_eq!((s, code(&v)), (403, "confirmation_mismatch"));
}

#[tokio::test]
async fn without_confirm_to_nothing_is_queued() {
    let dir = tempfile::tempdir().unwrap();
    let exe = clax_fake_exe::install(&dir.path().join("chitin"), FAKE_CHITIN);
    let sign = Arc::new(clax_core::config::SignConfig {
        chitin: exe.to_string_lossy().into_owned(),
        as_title: "bobby".into(),
        state_dir: Some(dir.path().to_string_lossy().into_owned()),
        confirm_to: None,
    });
    let ts = TestServer::spawn_with(move |s| s.sign = Some(sign)).await;
    ts.register_session("claude", "c-1").await;
    let aid = page(&ts, "Off").await;
    held(&open_world(&ts, &aid, None).await);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!Path::new(&dir.path().join("say.1")).exists());
}

#[tokio::test]
async fn with_five_of_claxs_items_waiting_nothing_more_is_queued() {
    let item = |n: u32| {
        json!({"id": format!("{n:012x}"), "as": "bobby", "to": "clax", "queued_by": "clax",
            "text": "x", "declined": false, "expired": false})
    };
    let full: Vec<Value> = (100..105).map(item).collect();
    let r = rig(None, Some(&serde_json::to_string(&full).unwrap())).await;
    let aid = page(&r.ts, "Capped").await;
    let id = held(&open_world(&r.ts, &aid, None).await);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!r.bin_dir().join("say.1").exists());
    // The browser card still asks.
    r.ts.confirm(&id, true).await;
    let (s, v) = open_world(&r.ts, &aid, Some(&id)).await;
    assert_eq!(s, 200, "{v}");
}

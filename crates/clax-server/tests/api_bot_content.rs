//! Content through a host's operations (`docs/contract.md`, "ClaxBot
//! surfaces", "Content"; `clax_server::routes::bot_content`,
//! `clax_store::bot_content`, `clax_bot::images`), end to end with the
//! generic test host: a host's operation answers an image, the script is
//! given the whole handle, the model is shown the image once, and a
//! handle the script passes back reaches the host, which reads its bytes;
//! another run's handle, an upload too large or not of its type, and a
//! read of content never passed are refused; and prune removes a run's
//! content and the bytes no item names.

use clax_bot::bot_surfaces::{CallFailure, HubLimits, Millis, Retry};
use clax_bot::broker::{CallBroker, Sending};
use clax_bot::clax_surface::wire::Notation;
use clax_bot::clax_surface::wire::Outcome;
use clax_bot::surface::{CallPlace, Claim};
use clax_core::audit::AuditCtx;
use clax_core::working::ManualClock;
use clax_http::bot_surfaces as hub;
use clax_store::bot_runs::{DriverId, StepCommit, StepKind};
use clax_testkit::fake_host::{Behaviour, FakeHost, HttpTransport};
use clax_testkit::fake_messages::{FakeMessages, Turn};
use clax_testkit::remotes::json_of;
use clax_testkit::{ManualSleeper, TestServer};
use serde_json::{Value, json};
use std::sync::Arc;

const KEY: &str = "sk-ant-api03-TOPSECRETKEYVALUE-abcd";
const START: &str = "2026-11-11T00:00:00Z";

/// A surface of a camera and a shelf: one operation answers an image,
/// one takes one.
const LENS: &str = r#"/** A camera and a shelf of pictures. */
declare const lens: {
  /** A picture of what the camera sees. @read */
  look(): Promise<Image>;
  /** Keeps a picture on the shelf, with a note. @write */
  keep(picture: Image, note: string): Promise<null>;
  /** Puts anything at all in a drawer. @write */
  stash(thing: unknown): Promise<null>;
};
interface Kinds {
  /** The request is what the person wants of the camera. @allow lens.* */
  shoot(request: { ask: Instruction }): { summary: string };
}
"#;

fn crc(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// The head of a PNG of `w` by `h` pixels: what Clax reads an image's
/// type and size from.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    let crc = crc(&b[12..29]);
    b.extend_from_slice(&crc.to_be_bytes());
    b
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn configure(ts: &TestServer, base_url: Option<&str>) {
    if let Some(base_url) = base_url {
        ts.store
            .set_bot_settings(
                &AuditCtx::DAEMON,
                &[(
                    clax_store::bot_settings::Setting::BaseUrl,
                    Some(json!(base_url)),
                )],
            )
            .unwrap();
    }
    let r = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!(s, 200, "{v}");
}

/// The tool result for `id` in the last user message of a request.
fn tool_result<'a>(request: &'a Value, id: &str) -> &'a Value {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().into_iter().flatten())
        .find(|b| b["type"] == "tool_result" && b["tool_use_id"] == id)
        .unwrap_or_else(|| panic!("no tool result {id} in {request}"))
}

/// The script of the run below: it looks, keeps the picture, tries to
/// keep a handle of no content of the run's and one changed from what
/// it was given, and returns the picture and what each try was thrown.
const LOOK: &str = r#"const p = await clax.lens.look();
await clax.lens.keep(p, "kept");
let forged = "sent";
try { await clax.lens.keep({...p, id: "0".repeat(32)}, "forged"); } catch (e) { forged = e.code; }
let changed = "sent";
try { await clax.lens.keep({...p, bytes: p.bytes + 1}, "changed"); } catch (e) { changed = e.code; }
return {picture: p, forged, changed};"#;

/// A run over HTTP, driven by the daemon's driver: the host's `look`
/// uploads a PNG and answers its reference; the script is given the whole
/// handle and passes it to `keep`, whose host is sent it whole and reads
/// the bytes; the turn after the script is shown the image, and the one
/// after that the text alone.
#[tokio::test]
async fn an_image_a_host_answers_reaches_the_script_the_model_once_and_the_host_again() {
    let fake = FakeMessages::start(vec![
        Turn::script("tu_1", "Look and keep", LOOK),
        Turn::script("tu_2", "Nothing more", "return 1;"),
        Turn::finish("Kept a picture."),
    ])
    .await;
    let ts = TestServer::spawn_with(|s| {
        let cfg = clax_core::config::HomeConfig::from_text(
            s.home.root().join(clax_core::config::FILE),
            "[bot]\nmodel_takes_images = true\n",
            Vec::new(),
        )
        .unwrap();
        s.settings = Arc::new(clax_http::settings::Settings {
            running: Some(cfg),
            start_error: None,
            audit: s.settings.audit.clone(),
        });
        let d = clax_server::bot::driver(s);
        clax_server::bot::start_driver(&d);
    })
    .await;
    configure(&ts, Some(&fake.base)).await;
    let picture = png(2, 1);
    let mut host = FakeHost::new(HttpTransport::of(&ts), "lens", Notation::Dts, LENS)
        .on("look", Behaviour::upload("image/png", picture.clone()))
        .on("keep", Behaviour::result(Value::Null))
        .wait_ms(10_000);
    let registered = host.register().await.expect("lens registers");
    assert!(registered.withheld.is_empty(), "{registered:?}");
    let run = host
        .start_run("shoot", json!({"ask": "Keep a picture."}), None)
        .await
        .expect("the run starts");
    host.serve_until(|h| !h.ended().is_empty(), 16)
        .await
        .expect("the host is told the run ended");

    // The upload was the run's.
    let (status, handle) = host.uploads()[0].clone();
    assert_eq!(status, 200, "{handle}");
    let sha = clax_core::audit::sha256_hex(&picture);
    assert_eq!(
        handle,
        json!({"id": handle["id"], "media_type": "image/png", "bytes": picture.len(), "sha256": sha,
               "width": 2, "height": 1})
    );
    let id = handle["id"].as_str().unwrap().to_string();

    // `keep` was sent the whole handle once; the forged and the changed
    // handles reached no host.
    let kept = host.calls_of("keep");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].args, vec![handle.clone(), json!("kept")]);
    // So the host reads its bytes.
    let (status, bytes) = host.download(&run, &id).await;
    assert_eq!((status, bytes), (200, picture.clone()));

    // The turn after the script was shown the image after the result's
    // text, which says which handle it is.
    let requests = fake.requests();
    assert_eq!(requests.len(), 3);
    let shown = tool_result(&requests[1], "tu_1");
    let parts = shown["content"]
        .as_array()
        .unwrap_or_else(|| panic!("text and an image: {shown}"));
    assert_eq!(parts.len(), 2, "{shown}");
    assert_eq!(
        parts[1],
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/png",
               "data": base64(&picture)}})
    );
    let text: Value = serde_json::from_str(parts[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["value"]["picture"], handle);
    assert_eq!(text["value"]["forged"], "invalid_args");
    assert_eq!(text["value"]["changed"], "invalid_args");
    assert_eq!(text["images"], json!([{"id": id, "shown": true}]));
    // The turn after that is sent the text and a note, and no image.
    let later = tool_result(&requests[2], "tu_1");
    let later = later["content"].as_str().expect("the text alone");
    assert!(
        later.ends_with(clax_bot::images::IMAGES_SHOWN_ONCE),
        "{later}"
    );
    assert!(!requests[2].to_string().contains("\"type\":\"image\""));
    // The step keeps the handle, not the bytes, and the item is shown.
    let (_, steps) = ts.store.run_with_steps(&run).unwrap().unwrap();
    let script = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    let payload = script.payload.as_ref().unwrap();
    assert_eq!(
        payload["images"],
        json!([{"id": id, "sha256": sha, "media_type": "image/png"}])
    );
    assert!(payload["result"]["content"].is_string());
    let row = ts.store.bot_content(&id).unwrap().unwrap();
    assert!(row.shown);
    assert_eq!(row.op.as_deref(), Some("lens.look"));
    assert_eq!(fake.left(), 0);
}

// ---------------------------------------------------------------------------
// The conformance rule, limits and prune, on a daemon the test drives
// ---------------------------------------------------------------------------

/// A daemon on a clock the test moves, with small content limits and no
/// driver: the test opens the runs' calls itself.
struct Daemon {
    ts: TestServer,
    clock: Arc<ManualClock>,
}

/// A run the test holds, its first step open.
struct Running {
    id: String,
    driver: DriverId,
    next: i64,
}

const ITEM: u64 = 64;
const RUN: u64 = 100;

async fn daemon() -> Daemon {
    daemon_with(HubLimits {
        content: true,
        content_item_bytes: ITEM,
        content_run_bytes: RUN,
        ..HubLimits::default()
    })
    .await
}

/// [`daemon`] under `limits`.
async fn daemon_with(limits: HubLimits) -> Daemon {
    let clock = Arc::new(ManualClock::at(START));
    let sleeper = Arc::new(ManualSleeper::default());
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        state.bot_surfaces = Arc::new(hub::Surfaces::new(state.store.clone(), sleeper, limits));
    })
    .await;
    configure(&ts, None).await;
    Daemon { ts, clock }
}

impl Daemon {
    async fn host(&self) -> FakeHost<HttpTransport> {
        let mut host = FakeHost::new(HttpTransport::of(&self.ts), "lens", Notation::Dts, LENS);
        host.register().await.expect("lens registers");
        host
    }

    async fn run(&self, host: &mut FakeHost<HttpTransport>) -> Running {
        let id = host
            .start_run("shoot", json!({"ask": "Keep a picture."}), None)
            .await
            .expect("the run starts");
        let driver = DriverId::mint();
        let st = &self.ts.store;
        st.claim_run(&id, &driver, 60).unwrap().expect("claimed");
        st.begin_step(&id, &driver, 1, StepKind::Script).unwrap();
        Running {
            id,
            driver,
            next: 0,
        }
    }

    /// A call of `op` with `args` for `run`, to `host`'s registration,
    /// sent through the daemon's broker under the run's claim.
    fn call(
        &self,
        host: &FakeHost<HttpTransport>,
        run: &mut Running,
        op: &str,
        args: Value,
    ) -> Result<String, CallFailure> {
        let registration = host.registration().unwrap();
        let st = &self.ts.store;
        let definition = st.registration(registration).unwrap().unwrap().def_hash;
        let seq = run.next;
        run.next += 1;
        let place = CallPlace {
            step: 1,
            attempt: 1,
            seq,
            key: format!("{}:1:{seq}", run.id),
        };
        let now = hub::now(st);
        let sending = Sending {
            registration,
            definition: &definition,
            run: &run.id,
            op,
            args: args.as_array().map_or(&[], Vec::as_slice),
            raw: &args,
            place: &place,
            waits: false,
            deadline: Some(Millis(now.0 + 30_000)),
            retry: Retry::NONE,
            joined: None,
        };
        self.ts
            .state
            .bot_surfaces
            .broker()
            .send(&Claim::new(st, &run.driver, &run.id), &sending)
    }

    /// What the run's journal holds for the call: whether it is a value,
    /// and the value or the failure.
    fn outcome(&self, call: &str) -> Option<(bool, Value)> {
        let st = &self.ts.store;
        let row = st.pending_call(call).unwrap().expect("kept");
        row.answered_at.as_ref()?;
        st.bot_calls(&row.run_id, row.step_n)
            .unwrap()
            .into_iter()
            .find(|e| e.seq() == row.seq)
            .map(|e| (e.ok(), e.result().clone()))
    }

    fn end(&self, run: &Running) {
        let st = &self.ts.store;
        let ctx = AuditCtx::DAEMON;
        st.commit_bot_step(&ctx, &run.id, &run.driver, 1, StepCommit::default())
            .unwrap();
        st.begin_step(&run.id, &run.driver, 2, StepKind::Finish)
            .unwrap();
        st.commit_bot_step(&ctx, &run.id, &run.driver, 2, StepCommit::default())
            .unwrap();
    }
}

fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

/// Every route asked with another run's or another registration's IDs
/// answers as for content that is not there, and an answer naming
/// another run's upload settles its call `invalid_result`.
#[tokio::test]
async fn another_runs_or_registrations_content_is_absent_everywhere() {
    let d = daemon().await;
    let mut host = d.host().await;
    let mut other = d.host().await;
    let mut a = d.run(&mut host).await;
    let mut b = d.run(&mut host).await;
    let picture = png(3, 2);

    // No call of the run is waiting: nothing is uploaded for it.
    let (status, body) = host.upload(&a.id, "image/png", picture.clone(), &[]).await;
    assert_eq!((status, code(&body)), (404, "not_found"), "{body}");
    let look_a = d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let look_b = d.call(&host, &mut b, "lens.look", json!([])).unwrap();
    // Another registration of the surface uploads for none of its runs.
    let (status, body) = other.upload(&a.id, "image/png", picture.clone(), &[]).await;
    assert_eq!((status, code(&body)), (404, "not_found"), "{body}");
    let (status, handle) = host.upload(&a.id, "image/png", picture.clone(), &[]).await;
    assert_eq!(status, 200, "{handle}");
    let id = handle["id"].as_str().unwrap().to_string();

    // Run b's call answered with run a's upload: refused, as absent.
    host.answer(&look_b, Outcome::Result(json!({"id": id})));
    host.answer(
        &look_a,
        Outcome::Result(json!({"id": id, "meta": {"at": 1}})),
    );
    host.serve_once().await;
    let (ok, failure) = d.outcome(&look_b).expect("settled");
    assert!(!ok);
    assert_eq!(failure["code"], "invalid_result", "{failure}");
    let (ok, value) = d.outcome(&look_a).expect("settled");
    assert!(ok, "{value}");
    let mut whole = handle.clone();
    whole["meta"] = json!({"at": 1});
    assert_eq!(value, whole);

    // Not yet passed to the host: absent to it.
    assert_eq!(host.download(&a.id, &id).await.0, 404);
    let keep = d
        .call(&host, &mut a, "lens.keep", json!([handle, "kept"]))
        .unwrap();
    assert!(!keep.is_empty());
    let (status, bytes) = host.download(&a.id, &id).await;
    assert_eq!((status, bytes), (200, picture.clone()));
    // Another run's ID, another registration, an ID of nothing: absent.
    assert_eq!(host.download(&b.id, &id).await.0, 404);
    assert_eq!(other.download(&a.id, &id).await.0, 404);
    assert_eq!(host.download(&a.id, &"0".repeat(32)).await.0, 404);
}

/// An upload over an item's limit, one that would take its run past the
/// run's, one whose bytes are not its type, and one whose host states
/// what is not so are refused, and none is kept.
#[tokio::test]
async fn an_upload_too_large_or_not_of_its_type_is_refused() {
    let d = daemon().await;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let picture = png(3, 2);
    let up = |media: &'static str, bytes: Vec<u8>| (media, bytes);
    for ((media, bytes), status, want) in [
        (up("text/plain", vec![b'a'; 65]), 413, "content_too_large"),
        (up("text/plain", vec![b'a'; 4096]), 413, "content_too_large"),
        (up("image/jpeg", picture.clone()), 400, "content_mismatch"),
        (up("image/png", b"words".to_vec()), 400, "content_mismatch"),
        (up("text/plain", picture.clone()), 400, "content_mismatch"),
        (up("text/plain", vec![0xff, 0xfe]), 400, "content_mismatch"),
        (up("not a type", b"words".to_vec()), 400, "invalid_request"),
    ] {
        let (s, body) = host.upload(&a.id, media, bytes, &[]).await;
        assert_eq!((s, code(&body)), (status, want), "{media}: {body}");
    }
    for stated in [
        ("sha256", "0".repeat(64)),
        ("bytes", "1".to_string()),
        ("width", "4".to_string()),
        ("colour", "red".to_string()),
    ] {
        let (s, body) = host
            .upload(
                &a.id,
                "image/png",
                picture.clone(),
                std::slice::from_ref(&stated),
            )
            .await;
        assert_eq!(s, 400, "{stated:?}: {body}");
    }
    // What is so may be stated.
    let sha = clax_core::audit::sha256_hex(&picture);
    let true_ = [
        ("sha256", sha),
        ("bytes", picture.len().to_string()),
        ("width", "3".to_string()),
        ("height", "2".to_string()),
    ];
    let (s, body) = host
        .upload(&a.id, "image/png", picture.clone(), &true_)
        .await;
    assert_eq!(s, 200, "{body}");
    // The run's items are 100 bytes together at the most: one of 33 bytes
    // more fits, and then one of 64 does not.
    assert_eq!(
        host.upload(&a.id, "text/plain", vec![b'a'; 33], &[])
            .await
            .0,
        200
    );
    let (s, body) = host.upload(&a.id, "text/plain", vec![b'a'; 64], &[]).await;
    assert_eq!((s, code(&body)), (413, "run_content_full"), "{body}");
}

/// Prune removes an ended run's items a day after it ended, and the bytes
/// no item names any more; another run's item of the same bytes keeps
/// them.
#[tokio::test]
async fn prune_removes_a_runs_content_and_the_bytes_no_item_names() {
    let d = daemon().await;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    let mut b = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    d.call(&host, &mut b, "lens.look", json!([])).unwrap();
    let (shared, only) = (png(3, 2), png(5, 4));
    let mut ids = Vec::new();
    for (run, bytes) in [(&a, &shared), (&a, &only), (&b, &shared)] {
        let (s, h) = host.upload(&run.id, "image/png", bytes.clone(), &[]).await;
        assert_eq!(s, 200, "{h}");
        ids.push(h["id"].as_str().unwrap().to_string());
    }
    let st = &d.ts.store;
    let rows: Vec<_> = ids
        .iter()
        .map(|id| st.bot_content(id).unwrap().unwrap())
        .collect();
    d.end(&a);
    d.clock.advance_millis(24 * 3600 * 1000 + 1000);
    let pruned = st.prune_ended_runs().unwrap();
    assert_eq!((pruned.contents, pruned.blobs), (2, 1));
    assert_eq!(st.bot_content(&ids[0]).unwrap(), None);
    assert_eq!(st.bot_content(&ids[1]).unwrap(), None);
    assert_eq!(st.bot_content_bytes(&rows[1]).unwrap(), None);
    assert_eq!(st.bot_content_bytes(&rows[2]).unwrap().unwrap(), shared);
}

/// A `GET` of the content `id` as the registration `registration`, for
/// `run`, with the daemon's token.
async fn get_content(
    ts: &TestServer,
    registration: &str,
    run: &str,
    id: &str,
) -> reqwest::Response {
    ts.client
        .get(format!("{}/api/bot/surfaces/lens/content/{id}", ts.base))
        .query(&[("registration", registration), ("run", run)])
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap()
}

/// A download is the bytes as an attachment, under a policy that runs
/// nothing in them: a host's bytes of any type are never a page of the
/// daemon's origin.
#[tokio::test]
async fn a_download_is_an_attachment_that_runs_nothing() {
    let d = daemon().await;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let (s, handle) = host.upload(&a.id, "image/png", png(3, 2), &[]).await;
    assert_eq!(s, 200, "{handle}");
    let id = handle["id"].as_str().unwrap();
    d.call(&host, &mut a, "lens.keep", json!([handle, "kept"]))
        .unwrap();
    let res = get_content(&d.ts, host.registration().unwrap(), &a.id, id).await;
    assert_eq!(res.status(), 200);
    let h = res.headers();
    assert_eq!(h["content-type"], "image/png");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["content-disposition"], "attachment");
    assert_eq!(h["content-security-policy"], "sandbox; default-src 'none'");
}

/// A registration that was never issued is answered as content that is
/// not there, on both routes: 404 `not_found`.
#[tokio::test]
async fn an_unknown_registration_is_answered_not_found() {
    let d = daemon().await;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let never = "0".repeat(32);
    let res =
        d.ts.client
            .put(format!("{}/api/bot/surfaces/lens/content", d.ts.base))
            .query(&[("registration", never.as_str()), ("run", a.id.as_str())])
            .bearer_auth(&d.ts.token)
            .header("content-type", "text/plain")
            .body("words")
            .send()
            .await
            .unwrap();
    let (s, body) = json_of(res).await;
    assert_eq!((s, code(&body)), (404, "not_found"), "{body}");
    let res = get_content(&d.ts, &never, &a.id, &"1".repeat(32)).await;
    let (s, body) = json_of(res).await;
    assert_eq!((s, code(&body)), (404, "not_found"), "{body}");
}

/// A handle is passed to a registration only at a place its operation's
/// type says holds `Content` or `Image`: a copy of one in an argument of
/// type `unknown`, or in the `meta` of a handle passed, grants no read.
#[tokio::test]
async fn a_look_alike_of_a_handle_grants_no_read() {
    let d = daemon().await;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let (s, x) = host.upload(&a.id, "image/png", png(3, 2), &[]).await;
    assert_eq!(s, 200, "{x}");
    let (s, y) = host.upload(&a.id, "image/png", png(5, 4), &[]).await;
    assert_eq!(s, 200, "{y}");
    let (x_id, y_id) = (x["id"].as_str().unwrap(), y["id"].as_str().unwrap());
    // In an argument of type `unknown`: not passed.
    d.call(&host, &mut a, "lens.stash", json!([y])).unwrap();
    assert_eq!(host.download(&a.id, y_id).await.0, 404);
    // In the `meta` of a handle passed as an `Image`: the handle is
    // passed, and what its `meta` holds is not.
    let mut x_with_y = x.clone();
    x_with_y["meta"] = json!({"also": y});
    d.call(&host, &mut a, "lens.keep", json!([x_with_y, "kept"]))
        .unwrap();
    assert_eq!(host.download(&a.id, x_id).await.0, 200);
    assert_eq!(host.download(&a.id, y_id).await.0, 404);
    // Passed where the type says `Image`, it is read.
    d.call(&host, &mut a, "lens.keep", json!([y, "kept"]))
        .unwrap();
    assert_eq!(host.download(&a.id, y_id).await.0, 200);
}

/// The daemon's content of all runs together is held to its own limit,
/// refused `content_store_full` past it, whichever run would pass it.
#[tokio::test]
async fn all_runs_content_together_is_held_to_the_daemons_limit() {
    let d = daemon_with(HubLimits {
        content: true,
        content_item_bytes: ITEM,
        content_run_bytes: RUN,
        content_total_bytes: 80,
        ..HubLimits::default()
    })
    .await;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    let mut b = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    d.call(&host, &mut b, "lens.look", json!([])).unwrap();
    for (run, bytes) in [(&a, png(3, 2)), (&a, png(5, 4))] {
        let (s, h) = host.upload(&run.id, "image/png", bytes, &[]).await;
        assert_eq!(s, 200, "{h}");
    }
    // 66 bytes are kept; 33 more, of another run, would be 99.
    let (s, body) = host.upload(&b.id, "image/png", png(7, 6), &[]).await;
    assert_eq!((s, code(&body)), (507, "content_store_full"), "{body}");
    let (s, body) = host.upload(&b.id, "text/plain", vec![b'a'; 14], &[]).await;
    assert_eq!(s, 200, "{body}");
}

/// What the daemon's listener records of a connection from this machine,
/// for a request handed to its router directly.
fn local(ts: &TestServer) -> axum::extract::ConnectInfo<clax_http::auth::Conn> {
    axum::extract::ConnectInfo(clax_http::auth::Conn {
        peer: std::net::SocketAddr::from(([127, 0, 0, 1], 50_000)),
        local: ts.addr,
    })
}

/// A body of `chunks` pieces of `piece` bytes, which counts the pieces
/// read from it.
fn counted(chunks: usize, piece: usize) -> (axum::body::Body, Arc<std::sync::atomic::AtomicUsize>) {
    use futures::StreamExt as _;
    use std::sync::atomic::Ordering;
    let read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = read.clone();
    let stream = futures::stream::iter(0..chunks).map(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'a'; piece]))
    });
    (axum::body::Body::from_stream(stream), read)
}

/// Under the daemon's own limits, through its router: an upload whose
/// `Content-Length` is over an item's 20 MiB is refused with nothing of it
/// read, as is one for a run with no call waiting; one that says no
/// length is read no further than a byte past the limit; and one of 20
/// MiB is taken.
#[tokio::test]
async fn an_upload_is_checked_before_its_body_is_read_under_the_daemons_limits() {
    use std::sync::atomic::Ordering;
    use tower::ServiceExt as _;
    let d = daemon_with(HubLimits {
        content: true,
        ..HubLimits::default()
    })
    .await;
    let limits = d.ts.state.bot_surfaces.limits();
    assert_eq!(limits.content_item_bytes, 20 << 20);
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    let waiting = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let registration = host.registration().unwrap().to_string();
    let router = clax_server::router::build_router(d.ts.state.clone());
    const MIB: usize = 1 << 20;
    let put = |run: &str, length: Option<usize>, body: axum::body::Body| {
        let mut req = axum::http::Request::put(format!(
            "/api/bot/surfaces/lens/content?registration={registration}&run={run}"
        ))
        .header("authorization", format!("Bearer {}", d.ts.token))
        .header("host", d.ts.addr.to_string())
        .extension(local(&d.ts))
        .header("content-type", "text/plain");
        if let Some(n) = length {
            req = req.header("content-length", n.to_string());
        }
        req.body(body).unwrap()
    };
    let answer = |res: axum::response::Response| async move {
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
        )
    };
    // A length a byte over the limit: refused, and not a piece read.
    let (body, read) = counted(21, MIB);
    let res = router
        .clone()
        .oneshot(put(&a.id, Some((20 << 20) + 1), body))
        .await
        .unwrap();
    let (s, v) = answer(res).await;
    assert_eq!((s, code(&v)), (413, "content_too_large"), "{v}");
    assert_eq!(read.load(Ordering::SeqCst), 0);
    // No call of the run waiting: absent, and not a piece read.
    let (body, read) = counted(1, MIB);
    let res = router
        .clone()
        .oneshot(put(&waiting.id, Some(MIB), body))
        .await
        .unwrap();
    let (s, v) = answer(res).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    assert_eq!(read.load(Ordering::SeqCst), 0);
    // No length: read to a piece past the limit, and no further.
    let (body, read) = counted(32, MIB);
    let res = router
        .clone()
        .oneshot(put(&a.id, None, body))
        .await
        .unwrap();
    let (s, v) = answer(res).await;
    assert_eq!((s, code(&v)), (413, "content_too_large"), "{v}");
    assert!(read.load(Ordering::SeqCst) <= 21, "{read:?}");
    // 20 MiB is an item.
    let (body, _) = counted(20, MIB);
    let res = router
        .clone()
        .oneshot(put(&a.id, Some(20 << 20), body))
        .await
        .unwrap();
    let (s, v) = answer(res).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["bytes"], 20 << 20);
}

/// Uploads of one registration in flight at once are bounded: one more is
/// refused `too_many_uploads` before its body is read, and once one ends
/// another is taken.
#[tokio::test]
async fn one_registrations_uploads_at_once_are_bounded() {
    use std::sync::atomic::Ordering;
    use tower::ServiceExt as _;
    let d = daemon_with(HubLimits {
        content: true,
        ..HubLimits::default()
    })
    .await;
    let most = d.ts.state.bot_surfaces.limits().uploads_per_registration;
    let mut host = d.host().await;
    let mut a = d.run(&mut host).await;
    d.call(&host, &mut a, "lens.look", json!([])).unwrap();
    let registration = host.registration().unwrap().to_string();
    let router = clax_server::router::build_router(d.ts.state.clone());
    let put = |body: axum::body::Body| {
        axum::http::Request::put(format!(
            "/api/bot/surfaces/lens/content?registration={registration}&run={}",
            a.id
        ))
        .header("authorization", format!("Bearer {}", d.ts.token))
        .header("host", d.ts.addr.to_string())
        .extension(local(&d.ts))
        .header("content-type", "text/plain")
        .body(body)
        .unwrap()
    };
    // `most` uploads whose bodies are read up to their first piece and
    // then wait.
    let mut senders = Vec::new();
    let mut pending = Vec::new();
    for _ in 0..most {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let mut started_tx = Some(started_tx);
        let stream = futures::stream::unfold(rx, move |mut rx| {
            if let Some(s) = started_tx.take() {
                let _ = s.send(());
            }
            async move { rx.recv().await.map(|piece| (piece, rx)) }
        });
        let req = put(axum::body::Body::from_stream(stream));
        pending.push(tokio::spawn(router.clone().oneshot(req)));
        started_rx.await.expect("the upload's body is being read");
        senders.push(tx);
    }
    let (body, read) = counted(1, 8);
    let res = router.clone().oneshot(put(body)).await.unwrap();
    assert_eq!(res.status(), 429);
    let v: Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(code(&v), "too_many_uploads", "{v}");
    assert_eq!(read.load(Ordering::SeqCst), 0);
    // The ones in flight end; another is taken.
    for tx in senders {
        tx.send(Ok(axum::body::Bytes::from_static(b"words")))
            .await
            .unwrap();
    }
    for p in pending {
        assert_eq!(p.await.unwrap().unwrap().status(), 200);
    }
    let (body, _) = counted(1, 8);
    let res = router.oneshot(put(body)).await.unwrap();
    assert_eq!(res.status(), 200);
}

/// A press: an operation that answers an image, and an effect that takes
/// one.
const PRESS: &str = r#"/** A press that prints pictures. */
declare const press: {
  /** The picture on the plate. @read */
  look(): Promise<Image>;
  /** Prints a picture. @effect */
  print(picture: Image): Promise<null>;
};
interface Kinds {
  /** The request is what the person wants printed. @allow press.* */
  printing(request: { ask: Instruction }): { summary: string };
}
"#;

/// A call that passes content is an effect like any other: under a
/// registration's kept `max_effects_per_run` of 1, the second `print` of
/// a handle is refused `effect_limit` and its host is sent one.
#[tokio::test]
async fn a_call_that_passes_content_counts_against_the_effect_limit() {
    use clax_bot::clax_surface::wire::{Bound, Settings};
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Print twice",
            r#"const p = await clax.press.look();
await clax.press.print(p);
let again = "sent";
try { await clax.press.print(p); } catch (e) { again = e.code; }
return {again};"#,
        ),
        Turn::finish("Printed once."),
    ])
    .await;
    let ts = TestServer::spawn_with(|s| {
        let d = clax_server::bot::driver(s);
        clax_server::bot::start_driver(&d);
    })
    .await;
    configure(&ts, Some(&fake.base)).await;
    let settings = Settings {
        max_effects_per_run: Some(Bound::At(1)),
        ..Settings::default()
    };
    let mut host = FakeHost::new(HttpTransport::of(&ts), "press", Notation::Dts, PRESS)
        .with_registration(None, settings)
        .on("look", Behaviour::upload("image/png", png(2, 1)))
        .on("print", Behaviour::result(Value::Null))
        .wait_ms(10_000);
    host.register().await.expect("press registers");
    host.start_run("printing", json!({"ask": "Print it."}), None)
        .await
        .expect("the run starts");
    host.serve_until(|h| !h.ended().is_empty(), 16)
        .await
        .expect("the host is told the run ended");
    assert_eq!(host.calls_of("print").len(), 1);
    let requests = fake.requests();
    let result = tool_result(&requests[1], "tu_1");
    let text = result["content"].as_str().map_or_else(
        || result["content"][0]["text"].as_str().unwrap().to_string(),
        str::to_string,
    );
    let told: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(told["value"]["again"], "effect_limit", "{told}");
    assert_eq!(fake.left(), 0);
}

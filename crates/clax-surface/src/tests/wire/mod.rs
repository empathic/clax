//! The host protocol's messages ([`crate::wire`]) against their
//! contract fixture, `docs/claxbot-surfaces/contract.json`: every
//! message there reads and writes as its type, what a daemon may add is
//! read, what a reader refuses is refused, the fixture's definitions
//! are definitions, and the examples in the design's text are messages.
//! Generated values no reader of a message can be made to panic on.

use super::own;
use crate::wire::{
    self, Answer, CallKind, Delivery, ErrorResponse, MessageRequest, MessageResponse, Notation,
    Outcome, RegisterRequest, Registered, RunState, ServeRequest, ServeResponse, StartRunRequest,
    StartRunResponse,
};
use crate::{
    Access, Author, Definition, Limits, Reserved, canonical_manifest, parse, parse_manifest,
    print_manifest,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("../../../../../docs/claxbot-surfaces/contract.json");
const DESIGN: &str =
    include_str!("../../../../../docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the fixture is JSON")
}

/// `v` with every `$file:<name>` replaced by that file of the fixture.
fn with_files(v: &Value, files: &Value) -> Value {
    match v {
        Value::String(s) => match s.strip_prefix("$file:") {
            Some(name) => files[name].clone(),
            None => v.clone(),
        },
        Value::Array(items) => items.iter().map(|i| with_files(i, files)).collect(),
        Value::Object(o) => o
            .iter()
            .map(|(k, i)| (k.clone(), with_files(i, files)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        _ => v.clone(),
    }
}

/// Reads `json` as a `T`, writes it and reads that: the two readings
/// are equal, and what was written is returned.
fn round<T: DeserializeOwned + Serialize + PartialEq + std::fmt::Debug>(
    json: &Value,
) -> Result<Value, String> {
    let first: T = serde_json::from_value(json.clone()).map_err(|e| e.to_string())?;
    let written = serde_json::to_value(&first).map_err(|e| e.to_string())?;
    let second: T = serde_json::from_value(written.clone()).map_err(|e| e.to_string())?;
    assert_eq!(first, second, "{json}");
    Ok(written)
}

/// [`round`] for the type the fixture names.
fn round_as(ty: &str, json: &Value) -> Result<Value, String> {
    match ty {
        "RegisterRequest" => round::<RegisterRequest>(json),
        "Registered" => round::<Registered>(json),
        "ErrorResponse" => round::<ErrorResponse>(json),
        "ServeRequest" => round::<ServeRequest>(json),
        "ServeResponse" => round::<ServeResponse>(json),
        "StartRunRequest" => round::<StartRunRequest>(json),
        "StartRunResponse" => round::<StartRunResponse>(json),
        "MessageRequest" => round::<MessageRequest>(json),
        "MessageResponse" => round::<MessageResponse>(json),
        "RunState" => round::<RunState>(json),
        other => panic!("the fixture names a type the test does not know: {other}"),
    }
}

fn entries<'a>(fixture: &'a Value, section: &str) -> impl Iterator<Item = (&'a str, &'a Value)> {
    fixture[section]
        .as_array()
        .unwrap_or_else(|| panic!("the fixture has `{section}`"))
        .iter()
        .map(|e| {
            (
                e["type"].as_str().expect("an entry names its type"),
                &e["json"],
            )
        })
}

#[test]
fn every_message_of_the_fixture_is_written_as_it_is_read() {
    let fixture = fixture();
    let mut types = std::collections::BTreeSet::new();
    for (ty, json) in entries(&fixture, "messages") {
        let json = with_files(json, &fixture["files"]);
        let written = round_as(ty, &json).unwrap_or_else(|e| panic!("{ty}: {e}\n{json}"));
        assert_eq!(written, json, "{ty} is not written as it was read");
        types.insert(ty);
    }
    assert_eq!(
        types.into_iter().collect::<Vec<_>>(),
        [
            "ErrorResponse",
            "MessageRequest",
            "MessageResponse",
            "RegisterRequest",
            "Registered",
            "RunState",
            "ServeRequest",
            "ServeResponse",
            "StartRunRequest",
            "StartRunResponse"
        ],
        "the fixture has an example of every message"
    );
}

#[test]
fn what_a_daemon_adds_is_read_and_what_a_reader_refuses_is_refused() {
    let fixture = fixture();
    for (ty, json) in entries(&fixture, "tolerated") {
        let written = round_as(ty, json).unwrap_or_else(|e| panic!("{ty}: {e}\n{json}"));
        assert_ne!(&written, json, "{ty}: the example adds nothing\n{json}");
    }
    for (ty, json) in entries(&fixture, "refused") {
        let json = with_files(json, &fixture["files"]);
        assert!(round_as(ty, &json).is_err(), "{ty} was read:\n{json}");
    }
    assert!(entries(&fixture, "tolerated").count() >= 4);
    assert!(entries(&fixture, "refused").count() >= 12);
}

#[test]
fn a_delivery_of_an_unknown_kind_is_kept_whole_and_skipped() {
    let fixture = fixture();
    let (_, json) = entries(&fixture, "tolerated")
        .find(|(ty, _)| *ty == "ServeResponse")
        .expect("a tolerated ServeResponse");
    let response: ServeResponse = serde_json::from_value(json.clone()).unwrap();
    let Delivery::Other(event) = &response.calls[0] else {
        panic!("{:?}", response.calls[0]);
    };
    assert_eq!(Value::Object(event.clone()), json["calls"][0]);
    assert_eq!(response.calls[0].id(), None);
    assert_eq!(response.calls[1].id(), Some("c1"));
    assert_eq!(
        serde_json::to_value(&response.calls[0]).unwrap(),
        json["calls"][0]
    );
}

#[test]
fn an_event_is_a_delivery_with_no_id_whose_event_is_read_whole() {
    let fixture = fixture();
    // The events of the fixture's run, in the order its serves deliver
    // them: each read whole, with no ID.
    let mut steps = Vec::new();
    for (_, json) in entries(&fixture, "messages").filter(|(ty, _)| *ty == "ServeResponse") {
        let response: ServeResponse = serde_json::from_value(json.clone()).unwrap();
        for (delivery, written) in response.calls.iter().zip(json["calls"].as_array().unwrap()) {
            let Delivery::Event(event) = delivery else {
                continue;
            };
            assert_eq!(
                (event.run.as_str(), &event.event),
                ("run-1", &written["event"])
            );
            assert_eq!(delivery.id(), None);
            steps.push(format!(
                "{} {}",
                event.event["type"].as_str().unwrap(),
                event.event["step"]
            ));
        }
    }
    assert_eq!(
        steps,
        [
            "step 1",
            "parked 2",
            "resumed 2",
            "step 2",
            "step 3",
            "step 4",
            "step 5"
        ]
    );
}

#[test]
fn every_request_and_answer_of_the_fixtures_script_is_a_message() {
    let fixture = fixture();
    let steps = fixture["script"]
        .as_array()
        .expect("the fixture has a script");
    let mut hosts = 0;
    for step in steps {
        let Some(host) = step["host"].as_str() else {
            continue;
        };
        hosts += 1;
        let request = with_files(&step["request"], &fixture["files"]);
        let status = step["status"].as_u64().expect("a step has a status");
        let (asks, answers) = match host {
            "register" => ("RegisterRequest", "Registered"),
            "serve" => ("ServeRequest", "ServeResponse"),
            other => panic!("the script has a request the test does not know: {other}"),
        };
        round_as(asks, &request).unwrap_or_else(|e| panic!("{asks}: {e}\n{step}"));
        if status == 200 {
            round_as(answers, &step["response"])
                .unwrap_or_else(|e| panic!("{answers}: {e}\n{step}"));
        } else {
            let refusal: ErrorResponse = serde_json::from_value(step["response"].clone())
                .unwrap_or_else(|e| panic!("{e}\n{step}"));
            assert_eq!(
                u64::from(wire::status(&refusal.error.code)),
                status,
                "the status of `{}`\n{step}",
                refusal.error.code
            );
        }
    }
    assert!(hosts > 20, "{hosts}");
}

fn host(src: &str, notation: Notation) -> Definition {
    let reserved = Reserved::of(&[&own()]);
    let author = Author::Host {
        reserved: &reserved,
    };
    match notation {
        Notation::Dts => parse(src, author, &Limits::default()).map_err(|e| e.to_string()),
        Notation::Manifest => {
            parse_manifest(src, author, &Limits::default()).map_err(|e| e.to_string())
        }
    }
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_fixtures_two_files_are_one_definition() {
    let fixture = fixture();
    let dts = fixture["files"]["desk.d.ts"].as_str().unwrap();
    let manifest = fixture["files"]["desk.json"].as_str().unwrap();
    let from_dts = host(dts, Notation::Dts);
    assert_eq!(
        manifest,
        print_manifest(&from_dts),
        "desk.json is desk.d.ts's manifest as `print_manifest` lays it out"
    );
    let from_manifest = host(manifest, Notation::Manifest);
    assert_eq!(from_dts, from_manifest);
    assert_eq!(
        canonical_manifest(&from_dts),
        canonical_manifest(&from_manifest)
    );
    // The script's calls are of the file's operations, with their access.
    for step in fixture["script"].as_array().unwrap() {
        if step["host"] != "serve" || step["status"] != 200 {
            continue;
        }
        let response: ServeResponse = serde_json::from_value(step["response"].clone()).unwrap();
        for delivery in response.calls {
            let Delivery::Call(call) = delivery else {
                continue;
            };
            let op = from_dts
                .operation(&format!("desk.{}", call.op))
                .unwrap_or_else(|| panic!("desk.{} is not an operation", call.op));
            assert_eq!(CallKind::of(op.op.access), Some(call.call), "{}", call.op);
            op.check_args(&call.args).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

#[test]
fn a_call_is_of_the_operations_access_and_no_host_call_spends() {
    assert_eq!(CallKind::of(Access::Read), Some(CallKind::Read));
    assert_eq!(CallKind::of(Access::Write), Some(CallKind::Write));
    assert_eq!(CallKind::of(Access::Effect), Some(CallKind::Effect));
    assert_eq!(CallKind::of(Access::Spend), None);
}

#[test]
fn a_result_of_null_is_a_result_and_a_missing_one_is_none() {
    let answer: Answer = serde_json::from_value(json!({"id": "c", "result": null})).unwrap();
    assert_eq!(answer.outcome, Outcome::Result(Value::Null));
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({"id": "c", "result": null})
    );
    let ended =
        json!({"id": "e", "call": "ended", "run": "r", "end": {"finished": {}}, "result": null});
    let Delivery::Ended(read) = serde_json::from_value(ended.clone()).unwrap() else {
        panic!("not an end");
    };
    assert_eq!(read.result, Some(Value::Null));
    assert_eq!(serde_json::to_value(&read).unwrap(), ended);
}

/// A setting is left out or given: `null` is no setting's value, a
/// limit's (no limit is `"none"`) or any other's, and the registration
/// is no message, as for a value of the wrong type.
#[test]
fn a_setting_of_null_is_refused_and_not_read_as_left_out() {
    for settings in [
        json!({"max_effects_per_run": null}),
        json!({"call_timeout_s": null}),
        json!({"wait_limit_s": null}),
        json!({"runs": null}),
        json!({"runs": {"max_steps": null}}),
        json!({"runs": {"max_run_minutes": null}}),
        json!({"allow": null}),
        json!({"reader_view": null}),
    ] {
        let request = json!({"clax_surface": 1, "notation": "dts", "definition": "",
                             "settings": settings});
        assert!(
            serde_json::from_value::<RegisterRequest>(request).is_err(),
            "{settings} was read"
        );
    }
    let limit = serde_json::from_value::<wire::Settings>(json!({"max_effects_per_run": null}))
        .unwrap_err()
        .to_string();
    assert!(
        limit.contains("a limit is a whole number, zero or more, or \"none\""),
        "{limit}"
    );
    let none: wire::Settings =
        serde_json::from_value(json!({"max_effects_per_run": "none", "runs": {}})).unwrap();
    assert_eq!(none.max_effects_per_run, Some(wire::Bound::None));
    assert_eq!(none.runs, Some(wire::RunSettings::default()));
}

/// A registration's withheld operation says why in the word the run's
/// reason gives, one source for both: `content` for an operation that
/// holds content, and none for the reasons that are a run's alone.
#[test]
fn a_withheld_operations_reason_is_the_runs_reasons_word() {
    use crate::Withheld;
    assert_eq!(Withheld::Content.reason(), Some(wire::WITHHELD_CONTENT));
    for run_only in [Withheld::Kind, Withheld::Operator, Withheld::Starter] {
        assert_eq!(run_only.reason(), None, "{run_only:?}");
    }
    let op = wire::WithheldOp {
        op: "scans.file".into(),
        reason: Withheld::Content.reason().unwrap().into(),
    };
    let by_its_other_name: wire::Withheld = op.clone();
    assert_eq!(
        serde_json::to_value(&by_its_other_name).unwrap(),
        json!({"op": "scans.file", "reason": "content"})
    );
}

/// `null` is not "left out" for any optional member of a host's
/// message whose type has no `null`: a start's `allow: null` narrows to
/// nothing and is not read as "all the kind allows", `bind: null` is no
/// start without a bind, and `registration: null` is no new
/// registration. Each message is read with the member left out.
#[test]
fn an_optional_member_of_a_hosts_message_refuses_null() {
    let register = json!({"clax_surface": 1, "notation": "dts", "definition": ""});
    let start = json!({"registration": "r", "kind": "k", "request": {}});
    let message = json!({"registration": "r", "message": {}});
    for (ty, base, member) in [
        ("RegisterRequest", &register, "registration"),
        ("RegisterRequest", &register, "prompt"),
        ("StartRunRequest", &start, "allow"),
        ("StartRunRequest", &start, "bind"),
        ("MessageRequest", &message, "key"),
    ] {
        round_as(ty, base).unwrap_or_else(|e| panic!("{ty} without `{member}`: {e}"));
        let mut with_null = base.clone();
        with_null[member] = Value::Null;
        assert!(
            round_as(ty, &with_null).is_err(),
            "{ty} read `{member}: null` as left out"
        );
    }
}

/// The lines of the design's block of the protocol's examples, with
/// what stands for "some value" there written as a value: `{…}` as
/// `{}`, `[…]` as `[]`, a bare `…` as `null`, and the `|` between two
/// forms of an answer as a comma.
fn design_examples() -> String {
    let start = DESIGN
        .find("POST /api/bot/surfaces/shell/serve\n")
        .expect("the design has the serve example");
    let block = &DESIGN[start..];
    let end = block.find("```").expect("the example's block ends");
    block[..end]
        .replace("{…}", "{}")
        .replace("[…]", "[]")
        .replace(": …}", ": null}")
        .replace("} | {", "}, {")
}

/// The JSON document that starts at `text`'s first `{`.
fn first_json(text: &str) -> Value {
    let from = text.find('{').expect("a document");
    serde_json::Deserializer::from_str(&text[from..])
        .into_iter::<Value>()
        .next()
        .expect("a document")
        .unwrap_or_else(|e| panic!("{e}\n{text}"))
}

#[test]
fn the_designs_examples_are_messages() {
    let examples = design_examples();
    let (serve, rest) = examples
        .split_once("POST /api/bot/surfaces/shell/runs")
        .expect("the runs example");
    let (asked, answered) = serve.split_once('→').expect("the serve's answer");
    // The design's messages name no registration, and the protocol's
    // do: each is read with one added.
    let named = |mut message: Value| {
        message["registration"] = json!("0f3a9c1e5b7d4a2e8c6b1d0f9e7a3c55");
        message
    };
    serde_json::from_value::<ServeRequest>(first_json(asked)).unwrap_err();
    let request: ServeRequest = serde_json::from_value(named(first_json(asked))).unwrap();
    assert_eq!(request.wait_ms, 12000);
    assert!(matches!(request.answers[0].outcome, Outcome::Result(_)));
    assert!(matches!(request.answers[1].outcome, Outcome::Error(_)));
    let response: ServeResponse = serde_json::from_value(first_json(answered)).unwrap();
    assert!(
        matches!(&response.calls[0], Delivery::Call(c) if c.call == CallKind::Write && c.op == "settings.set" && c.expires_in_ms == Some(29000))
    );
    assert!(matches!(&response.calls[1], Delivery::Call(c) if c.op == "ask"));
    assert!(
        matches!(&response.calls[2], Delivery::Ended(e) if e.end == wire::End::Finished {} && e.result.is_some())
    );
    let (asked, answered) = rest.split_once('→').expect("the start's answer");
    let start: StartRunRequest = serde_json::from_value(named(first_json(asked))).unwrap();
    assert_eq!(start.kind, "triage");
    assert!(start.bind.is_some_and(|b| b.contains_key("clax")));
    let started: StartRunResponse = serde_json::from_value(first_json(answered)).unwrap();
    assert_eq!(started.run, "…");
    // "Deadlines, retries and what a call may promise" writes this one.
    assert!(DESIGN.contains(r#"`{"id": "…", "accepted":"#));
    let accepted: Answer = serde_json::from_value(json!({"id": "…", "accepted": true})).unwrap();
    assert_eq!(accepted.outcome, Outcome::Accepted);
}

#[test]
fn an_end_these_types_do_not_know_is_kept_whole_and_a_known_one_is_read_strictly() {
    let fixture = fixture();
    let (_, json) = entries(&fixture, "tolerated")
        .find(|(_, json)| json["calls"][0]["end"]["timed_out"].is_object())
        .expect("a tolerated end");
    let response: ServeResponse = serde_json::from_value(json.clone()).unwrap();
    let Delivery::Ended(ended) = &response.calls[0] else {
        panic!("{:?}", response.calls[0]);
    };
    let wire::End::Other(end) = &ended.end else {
        panic!("{:?}", ended.end);
    };
    assert_eq!(Value::Object(end.clone()), json["calls"][0]["end"]);
    assert_eq!(
        serde_json::to_value(&ended.end).unwrap(),
        json["calls"][0]["end"]
    );
    // An object of two members, or of none, is no end these types
    // know either.
    for other in [
        json!({}),
        json!({"finished": {}, "stopped": {"reason": "x"}}),
    ] {
        let read: wire::End = serde_json::from_value(other.clone()).unwrap();
        assert_eq!(read, wire::End::Other(other.as_object().unwrap().clone()));
    }
    // A known end that lacks what it has is a mistake, not another end.
    serde_json::from_value::<wire::End>(json!({"gave_up": {"code": "model"}})).unwrap_err();
    serde_json::from_value::<wire::End>(json!("finished")).unwrap_err();
    let read: wire::End = serde_json::from_value(json!({"finished": {"at": 1}})).unwrap();
    assert_eq!(read, wire::End::Finished {});
}

#[test]
fn the_refusal_of_a_version_says_which_are_spoken() {
    let body = wire::ErrorBody::unsupported_version(2);
    assert_eq!(body.code, wire::CODE_UNSUPPORTED_VERSION);
    assert_eq!(body.supported, [wire::VERSION]);
    assert_eq!(
        serde_json::to_value(&body).unwrap()["supported"],
        json!([1])
    );
}

#[test]
fn every_code_has_its_status() {
    for (code, status) in [
        (wire::CODE_INVALID_REQUEST, 400),
        (wire::CODE_INVALID_DEFINITION, 400),
        (wire::CODE_UNSUPPORTED_VERSION, 400),
        (wire::CODE_NAME_MISMATCH, 400),
        (wire::CODE_INVALID_SETTINGS, 400),
        (wire::CODE_PROMPT_TOO_LARGE, 400),
        (wire::CODE_TOO_MANY_ANSWERS, 400),
        (wire::CODE_UNKNOWN_REGISTRATION, 404),
        (wire::CODE_TOO_LARGE, 413),
        (wire::CODE_INTERNAL_ERROR, 500),
        (wire::CODE_STORAGE_FAILED, 500),
        (wire::CODE_TOO_MANY_REGISTRATIONS, 429),
        (wire::CODE_NOT_YET, 501),
        ("anything_else", 500),
    ] {
        assert_eq!(wire::status(code), status, "{code}");
    }
}

/// A generator of numbers that gives the same ones each run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A value made of the protocol's own names and of everything JSON
/// has, `depth` levels deep at the most.
fn any_value(rng: &mut Rng, depth: u32) -> Value {
    const KEYS: &[&str] = &[
        "id",
        "call",
        "run",
        "op",
        "args",
        "key",
        "expires_in_ms",
        "end",
        "result",
        "error",
        "code",
        "message",
        "accepted",
        "retry_after_ms",
        "wait_ms",
        "answers",
        "calls",
        "registration",
        "clax_surface",
        "notation",
        "definition",
        "prompt",
        "settings",
        "allow",
        "kind",
        "request",
        "bind",
        "finished",
        "supported",
        "gave_up",
        "stopped",
        "reason",
        "hash",
        "withheld",
        "errors",
        "line",
        "pointer",
        "unread",
        "runs",
        "max_steps",
        "x",
    ];
    const WORDS: &[&str] = &[
        "read", "write", "effect", "ended", "event", "dts", "manifest", "none", "names", "", "…",
    ];
    match rng.below(if depth == 0 { 6 } else { 9 }) {
        0 => Value::Null,
        1 => Value::Bool(rng.below(2) == 0),
        2 => json!(rng.below(4)),
        3 => json!(-(rng.below(1000) as i64)),
        4 => json!(rng.next() as f64 / 7.0),
        5 => json!(WORDS[rng.below(WORDS.len() as u64) as usize]),
        6 => (0..rng.below(4))
            .map(|_| any_value(rng, depth - 1))
            .collect(),
        _ => (0..rng.below(6))
            .map(|_| {
                let key = KEYS[rng.below(KEYS.len() as u64) as usize];
                (key.to_string(), any_value(rng, depth - 1))
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
    }
}

/// The JSON Pointer of every value in `v`, itself among them.
fn pointers(v: &Value, at: String, out: &mut Vec<String>) {
    match v {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                pointers(item, format!("{at}/{i}"), out);
            }
        }
        Value::Object(o) => {
            for (key, item) in o {
                pointers(item, format!("{at}/{key}"), out);
            }
        }
        _ => {}
    }
    out.push(at);
}

#[test]
fn no_value_makes_a_reader_panic_and_what_is_read_is_written_as_read() {
    let fixture = fixture();
    let seeds: Vec<(&str, Value)> = entries(&fixture, "messages")
        .chain(entries(&fixture, "tolerated"))
        .map(|(ty, json)| (ty, with_files(json, &fixture["files"])))
        .collect();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut read = 0;
    for round_n in 0..20_000 {
        // A message of the fixture with one of its values replaced, or
        // a value that was never a message.
        let (ty, mut json) = seeds[rng.below(seeds.len() as u64) as usize].clone();
        if round_n % 4 == 0 {
            json = any_value(&mut rng, 4);
        } else {
            let mut places = Vec::new();
            pointers(&json, String::new(), &mut places);
            let place = &places[rng.below(places.len() as u64) as usize];
            *json.pointer_mut(place).expect("a place of the value") = any_value(&mut rng, 3);
        }
        // `round` holds what is read to what is written and read again.
        if round_as(ty, &json).is_ok() {
            read += 1;
        }
    }
    assert!(read > 1000, "{read} of the values were messages");
    assert!(read < 19_000, "{read} of the values were messages");
}

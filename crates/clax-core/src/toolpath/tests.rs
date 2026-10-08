//! Renderer, redaction and conformance tests (spec §15). The golden
//! history `tests/toolpath/history.json` holds one or more events of every
//! kind, live and backfilled; its renderings are compared with
//! `tests/toolpath/expected/`, and validated against Toolpath's schema,
//! vendored in `tests/toolpath/schema/`. `CLAX_UPDATE_GOLDEN=1` rewrites
//! the expected files.

use super::seal::seal;
use super::*;
use crate::audit::{AuditIds, SystemReason};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/toolpath")
        .join(name)
}

fn read_json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(data(name)).unwrap()).unwrap()
}

/// The vendored Toolpath schema.
fn schema() -> Value {
    read_json("schema/toolpath.schema.json")
}

/// Whether `s` matches the schema's actor pattern,
/// `^(human|agent|tool|ci):[a-zA-Z0-9_.-]+(/[a-zA-Z0-9_.-]+)?$`
/// (`actor_strings_match_schema_pattern` pins the vendored pattern to it).
fn is_actor_ref(s: &str) -> bool {
    let seg = |x: &str| {
        !x.is_empty()
            && x.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    let Some((kind, rest)) = s.split_once(':') else {
        return false;
    };
    let kind_ok = matches!(kind, "human" | "agent" | "tool" | "ci");
    kind_ok
        && match rest.split_once('/') {
            Some((a, b)) => seg(a) && seg(b),
            None => seg(rest),
        }
}

/// Whether `s` is an RFC 3339 date-time, the schema's `date-time` format.
fn is_timestamp(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

/// The ways `doc`, a Toolpath graph, breaks the parts of the schema Clax's
/// output exercises: identities, actor strings and timestamps, parents,
/// the change map's structural perspectives, refs, and actor definitions.
/// Full validation against the vendored schema runs on the golden
/// documents in the web unit gate (`web/scripts/toolpath-schema.test.ts`),
/// with Ajv; this keeps the nondeterministic renderings checked in Rust.
fn conformance_errors(doc: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    let mut err = |at: String, what: &str| errs.push(format!("{at}: {what}"));
    if !doc.pointer("/graph/id").is_some_and(Value::is_string) {
        err("graph".into(), "no graph.id");
    }
    let Some(paths) = doc["paths"].as_array() else {
        err("paths".into(), "not an array");
        return errs;
    };
    let ident_keys = ["system", "id"];
    let def_keys = ["name", "provider", "model", "identities", "keys"];
    for (i, p) in paths.iter().enumerate() {
        let at = format!("paths/{i}");
        for f in ["id", "head"] {
            if !p["path"][f].is_string() {
                err(at.clone(), &format!("path.{f} is not a string"));
            }
        }
        if let Some(actors) = p.pointer("/meta/actors").and_then(Value::as_object) {
            for (k, d) in actors {
                if !is_actor_ref(k) {
                    err(at.clone(), &format!("actor key {k}"));
                }
                let d = d.as_object();
                if !d.is_some_and(|d| d.keys().all(|k| def_keys.contains(&k.as_str()))) {
                    err(at.clone(), &format!("definition of {k}"));
                }
                for id in d
                    .and_then(|d| d.get("identities"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let ok = id.as_object().is_some_and(|o| {
                        o.len() == 2
                            && ident_keys
                                .iter()
                                .all(|k| o.get(*k).is_some_and(Value::is_string))
                    });
                    if !ok {
                        err(at.clone(), &format!("identity of {k}"));
                    }
                }
            }
        }
        for (j, st) in p["steps"].as_array().into_iter().flatten().enumerate() {
            let at = format!("{at}/steps/{j}");
            let s = &st["step"];
            if !s["id"].is_string() {
                err(at.clone(), "step.id");
            }
            if !s["actor"].as_str().is_some_and(is_actor_ref) {
                err(at.clone(), "step.actor");
            }
            if !s["timestamp"].as_str().is_some_and(is_timestamp) {
                err(at.clone(), "step.timestamp");
            }
            if let Some(ps) = s.get("parents")
                && !ps
                    .as_array()
                    .is_some_and(|ps| ps.iter().all(Value::is_string))
            {
                err(at.clone(), "step.parents");
            }
            match st["change"].as_object() {
                Some(c) => {
                    for (k, v) in c {
                        let ok = v.as_object().is_some_and(|o| {
                            o.keys().all(|k| k == "raw" || k == "structural")
                                && o.get("raw").is_none_or(Value::is_string)
                                && o.get("structural").is_some_and(|x| x["type"].is_string())
                        });
                        if !ok {
                            err(at.clone(), &format!("change {k}"));
                        }
                    }
                }
                None => err(at.clone(), "no change"),
            }
            for r in st
                .pointer("/meta/refs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let ok = r
                    .as_object()
                    .is_some_and(|o| o.len() == 2 && o["rel"].is_string() && o["href"].is_string());
                if !ok {
                    err(at.clone(), "ref");
                }
            }
            if st.pointer("/meta/source").is_some() {
                err(at.clone(), "meta.source");
            }
        }
    }
    errs
}

/// Fails with every way `doc` breaks [`conformance_errors`]' checks.
pub(super) fn assert_valid(doc: &Value, what: &str) {
    let errors = conformance_errors(doc);
    assert!(
        errors.is_empty(),
        "{what} does not conform:\n{}",
        errors.join("\n")
    );
}

/// Compares `actual` with the expected file `name`, or rewrites it under
/// `CLAX_UPDATE_GOLDEN=1`.
pub(super) fn golden(name: &str, actual: &str) {
    let path = data(&format!("expected/{name}"));
    if std::env::var_os("CLAX_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (CLAX_UPDATE_GOLDEN=1 writes it)", path.display()));
    if want != actual {
        let line = want
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(want.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from the golden output from line {}:\n  want: {}\n  got:  {}",
            line + 1,
            want.lines().nth(line).unwrap_or("<end>"),
            actual.lines().nth(line).unwrap_or("<end>"),
        );
    }
}

/// The golden history: the environment it renders under, and its rows.
pub(super) struct History {
    pub(super) env: RenderEnv,
    pub(super) rows: Vec<AuditRow>,
}

fn row_from(v: &Value) -> AuditRow {
    let id = |k: &str| v["ids"][k].as_str().map(str::to_string);
    AuditRow {
        seq: v["seq"].as_i64().unwrap(),
        at: v["at"].as_str().unwrap().into(),
        kind: v["kind"].as_str().unwrap().into(),
        actor: v["actor"].to_string(),
        ids: AuditIds {
            artifact: id("artifact"),
            artifact2: id("artifact2"),
            thread: id("thread"),
            session: id("session"),
            question: id("question"),
            call: id("call"),
            origin: id("origin"),
        },
        body: v["body"].to_string(),
        backfilled: v["backfilled"].as_bool().unwrap(),
    }
}

pub(super) fn history() -> History {
    let h = read_json("history.json");
    History {
        env: RenderEnv {
            install: h["install"].as_str().unwrap().into(),
            view_base: h["view_base"].as_str().map(str::to_string),
        },
        rows: h["rows"].as_array().unwrap().iter().map(row_from).collect(),
    }
}

/// Every row rendered as one linear chain, in `seq` order.
pub(super) fn render_chain(rows: &[AuditRow], env: &RenderEnv, opts: &Redaction) -> Vec<Value> {
    let mut prev = None;
    rows.iter()
        .map(|r| {
            let s = render_step(r, prev, env, opts).unwrap();
            prev = Some(r.seq);
            s
        })
        .collect()
}

const OPTION_SETS: [Redaction; 5] = [
    Redaction::NONE,
    Redaction {
        no_text: true,
        no_names: false,
        no_paths: false,
    },
    Redaction {
        no_text: false,
        no_names: true,
        no_paths: false,
    },
    Redaction {
        no_text: false,
        no_names: false,
        no_paths: true,
    },
    Redaction::ALL,
];

/// The golden history as an export source.
fn history_source(h: &History) -> project::MemSource {
    project::MemSource {
        rows: h.rows.clone(),
        info: BTreeMap::new(),
    }
}

/// The environment the golden exports are written under.
fn export_env(h: &History) -> project::ExportEnv {
    project::ExportEnv {
        render: h.env.clone(),
        clax_version: "0.3.1".into(),
        clax_commit: "abc1234def5678abc1234def5678abc1234def56".into(),
    }
}

/// The whole golden history exported under `opts` in the artifacts shape
/// (spec §8.2), as the projection writes it, indented.
fn export_text(h: &History, opts: &Redaction) -> String {
    let req = project::Export {
        redaction: *opts,
        pretty: true,
        ..Default::default()
    };
    let mut out = Vec::new();
    project::export(&history_source(h), &req, &export_env(h), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// [`export_text`], parsed.
fn export_graph(h: &History, opts: &Redaction) -> Value {
    serde_json::from_str(&export_text(h, opts)).unwrap()
}

/// The history as the first segment of a journal (spec §7.2), rendered as
/// the journal renders (no browser URLs): `PathOpen` (with no `continues`
/// ref, there being no segment before it), each actor's `ActorDef` before
/// its first step and again, merged, whenever its definition grows, the
/// steps, and, when `closed`, `Head` and `PathClose`.
pub(super) fn segment(h: &History, closed: bool) -> String {
    let env = &RenderEnv::journal(h.env.install.clone());
    let opts = Redaction::NONE;
    let install = clax_uri(&env.install, Obj::Install);
    let mut out = String::new();
    let mut line = |v: Value| {
        out.push_str(&serde_json::to_string(&v).unwrap());
        out.push('\n');
    };
    line(json!({"PathOpen": {
        "version": "1",
        "id": format!("clax-journal-{}-20261006-001", &env.install[..8]),
        "base": {"uri": install},
        "graph_ref": format!("toolpath://clax/{}", env.install),
        "meta": {
            "title": "Clax audit trail 2026-10-06 #1",
            "kind": KIND_URI,
            "source": install,
            "clax": {"projection": "journal", "install": env.install, "segment": "20261006-001",
                     "first_seq": h.rows[0].seq, "clax_version": "0.3.1",
                     "clax_commit": "abc1234def5678abc1234def5678abc1234def56",
                     "redaction": [], "segment_max_bytes": 64u64 << 20},
        },
    }}));
    let mut defined: BTreeMap<String, Value> = BTreeMap::new();
    let mut prev = None;
    for r in &h.rows {
        let rendered = render(r, prev, env, &opts).unwrap();
        for (actor, definition) in rendered.actors {
            let old = defined.get(&actor).unwrap_or(&Value::Null);
            let merged = merge_actor_def(old, &definition);
            if defined.get(&actor) != Some(&merged) {
                line(json!({"ActorDef": {"actor": actor, "definition": merged}}));
                defined.insert(actor, merged);
            }
        }
        line(json!({"Step": rendered.step}));
        prev = Some(r.seq);
    }
    if closed {
        line(json!({"Head": {"step_id": step_id(prev.unwrap())}}));
        line(json!({"PathClose": {}}));
    }
    out
}

#[test]
fn render_each_kind_matches_golden() {
    let h = history();
    let kinds: BTreeSet<&str> = h.rows.iter().map(|r| r.kind.as_str()).collect();
    for k in AuditKind::ALL {
        assert!(kinds.contains(k.as_str()), "the golden history has no {k}");
    }
    let steps = render_chain(&h.rows, &h.env, &Redaction::NONE);
    for (r, s) in h.rows.iter().zip(&steps) {
        assert_eq!(s["step"]["id"], step_id(r.seq));
        assert_eq!(s["step"]["timestamp"], r.at.as_str());
        let change = s["change"].as_object().unwrap();
        assert_eq!(change.len(), 1, "{}: one change key", r.kind);
        let (key, c) = change.iter().next().unwrap();
        assert!(
            key.starts_with(&format!("clax://{}", h.env.install)),
            "{key}"
        );
        assert_eq!(c["structural"]["type"], format!("clax.{}", r.kind));
        assert!(c.get("raw").is_none());
        for f in ENVELOPE {
            assert!(
                c["structural"].get(f).is_none(),
                "{}: {f} in structural",
                r.kind
            );
        }
        let clax = &s["meta"]["clax"];
        assert_eq!(clax["seq"], r.seq);
        assert_eq!(clax["kind"], r.kind.as_str());
        assert_eq!(clax["install"], h.env.install.as_str());
        assert_eq!(clax["backfilled"], r.backfilled);
        assert_eq!(clax["v"], 1);
        let d = s["meta"]["description"].as_str().unwrap();
        assert!(!d.is_empty() && !d.contains('\n'), "{d}");
        if r.backfilled {
            // Backfilled steps are told apart: made by Clax, flagged, and
            // naming who the history says acted.
            assert_eq!(s["step"]["actor"], "tool:clax/0.3.0");
            assert_eq!(clax["system_reason"], "backfill");
            assert!(clax["via"] == "daemon" && clax.get("git").is_none());
            if r.kind != "backfill.skip" {
                assert!(d.ends_with(", recorded from earlier history"), "{d}");
                assert!(clax["for_actor"].is_string(), "{}", r.kind);
                assert!(c["structural"]["inferred"].is_array(), "{}", r.kind);
            }
        } else {
            assert!(!d.contains("earlier history"), "{d}");
        }
    }
    let mut text = serde_json::to_string_pretty(&steps).unwrap();
    text.push('\n');
    golden("steps.json", &text);
}

#[test]
fn render_is_byte_stable() {
    let h = history();
    for opts in OPTION_SETS {
        let a = render_chain(&h.rows, &h.env, &opts);
        let b = render_chain(&h.rows, &h.env, &opts);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.to_string(), y.to_string());
        }
    }
    // The stored text's key order does not matter: a body written with its
    // keys reversed renders to the same bytes.
    for r in &h.rows {
        let body: Map<String, Value> = serde_json::from_str(&r.body).unwrap();
        let reversed: Vec<String> = body
            .iter()
            .rev()
            .map(|(k, v)| format!("{}:{}", Value::from(k.as_str()), v))
            .collect();
        let shuffled = AuditRow {
            body: format!("{{{}}}", reversed.join(",")),
            ..r.clone()
        };
        assert_eq!(
            render_step(r, Some(1), &h.env, &Redaction::NONE)
                .unwrap()
                .to_string(),
            render_step(&shuffled, Some(1), &h.env, &Redaction::NONE)
                .unwrap()
                .to_string()
        );
    }
    // Keys come out sorted, at every depth.
    fn sorted(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                m.keys().zip(m.keys().skip(1)).all(|(a, b)| a < b) && m.values().all(sorted)
            }
            Value::Array(a) => a.iter().all(sorted),
            _ => true,
        }
    }
    for s in render_chain(&h.rows, &h.env, &Redaction::NONE) {
        assert!(sorted(&s));
    }
}

#[test]
fn actor_strings_match_schema_pattern() {
    let s = schema();
    let actor_ref = &s["$defs"]["actorRef"];
    assert_eq!(actor_ref["type"], "string");
    let pattern = actor_ref["pattern"].as_str().unwrap();
    assert_eq!(
        pattern, r"^(human|agent|tool|ci):[a-zA-Z0-9_.-]+(/[a-zA-Z0-9_.-]+)?$",
        "the vendored actor pattern changed; recheck spec §10.1"
    );
    // The spec's timestamp format is the schema's.
    assert_eq!(s["$defs"]["timestamp"]["format"], "date-time");

    let h = history();
    let env = h.env.clone();
    let mut strings: Vec<String> = Vec::new();
    for r in &h.rows {
        for (a, _) in actor_defs(r, &env, &Redaction::NONE) {
            strings.push(a);
        }
        assert!(is_timestamp(&r.at), "{}", r.at);
    }
    // Values outside the pattern become `-`.
    let odd = [
        Actor::Agent(AgentActor {
            session_id: Some("01JB9S00000000000000000009".into()),
            harness: Some("my harness/2".into()),
            harness_session_id: Some("a b/c:d\u{e9}\u{1F600}".into()),
            ..AgentActor::default()
        }),
        Actor::Agent(AgentActor {
            session_id: Some("01JB9S00000000000000000009".into()),
            harness: Some(String::new()),
            harness_session_id: Some(String::new()),
            ..AgentActor::default()
        }),
        Actor::Agent(AgentActor {
            session_id: Some("01JB9S00000000000000000009".into()),
            ..AgentActor::default()
        }),
        Actor::Agent(AgentActor::default()),
        Actor::Viewer {
            public_id: "u_x/y z".into(),
            display_name: None,
        },
        Actor::Owner {
            public_id: String::new(),
        },
        Actor::Anonymous,
        Actor::System {
            reason: SystemReason::Rule,
        },
    ];

    for a in &odd {
        strings.push(actor_string(a, "0.3.1+dirty build"));
    }
    for s in &strings {
        assert!(is_actor_ref(s), "{s}");
    }
    assert_eq!(
        actor_string(&odd[0], "0.3.1"),
        "agent:my-harness-2/a-b-c-d--"
    );
    assert_eq!(
        actor_string(&odd[1], "0.3.1"),
        "agent:unknown/clax-01JB9S00000000000000000009"
    );
    assert_eq!(
        actor_string(&odd[2], "0.3.1"),
        "agent:unknown/clax-01JB9S00000000000000000009"
    );
    assert_eq!(actor_string(&odd[3], "0.3.1"), "agent:clax-mcp");
    assert_eq!(
        actor_string(&odd[7], "0.3.1+dirty build"),
        "tool:clax/0.3.1-dirty-build"
    );
    // The originals stay in the ActorDef.
    assert_eq!(
        actor_def(&odd[0], "i", "c", &Redaction::NONE)["identities"][1],
        json!({"system": "my harness/2-session", "id": "a b/c:d\u{e9}\u{1F600}"})
    );
}

/// A row of `kind` by `actor` with `ids` and the kind's `body` fields (the
/// envelope added).
fn row(seq: i64, kind: &str, actor: Value, ids: AuditIds, body: Value) -> AuditRow {
    let mut b = body;
    b["v"] = 1.into();
    if b.get("via").is_none() {
        b["via"] = "mcp".into();
    }
    AuditRow {
        seq,
        at: format!("2026-10-06T15:00:{:02}.000Z", seq % 60),
        kind: kind.into(),
        actor: actor.to_string(),
        ids,
        body: b.to_string(),
        backfilled: false,
    }
}

fn on(artifact: &str, thread: Option<&str>) -> AuditIds {
    AuditIds {
        artifact: Some(artifact.into()),
        thread: thread.map(str::to_string),
        ..AuditIds::default()
    }
}

const SID: &str = "01JB9S00000000000000000001";

fn agent_json(transcript: &str) -> Value {
    json!({"type": "agent", "session_id": SID, "harness": "claude",
           "harness_session_id": "3f2c", "agent_handle": "a_9f", "transcript_path": transcript})
}

/// Rows holding sentinel text in every field `--no-text` replaces.
fn text_rows() -> Vec<AuditRow> {
    let owner = json!({"type": "owner", "public_id": "u_own"});
    let q = AuditIds {
        question: Some("01JB9Q00000000000000000001".into()),
        ..AuditIds::default()
    };
    vec![
        row(
            1,
            "comment.add",
            owner.clone(),
            on("a1", Some("t1")),
            json!({"comment_id": "c1", "body": "SENTINEL-TEXT-body", "author_kind": "owner",
                   "author_name": "Owner", "via_harness": null, "via_page": false}),
        ),
        row(
            2,
            "version.publish",
            owner.clone(),
            on("a1", None),
            json!({"n": 3, "label": "SENTINEL-TEXT-label", "note": "SENTINEL-TEXT-note",
                   "title": "SENTINEL-TEXT-title", "files": {}, "content_sha256": null,
                   "carried": [], "addresses": [], "by_page": false}),
        ),
        row(
            3,
            "live.snapshot",
            owner.clone(),
            on("a1", None),
            json!({"n": 4, "label": "SENTINEL-TEXT-label2", "note": "SENTINEL-TEXT-note2",
                   "title": "SENTINEL-TEXT-title2", "files": {}, "content_sha256": null,
                   "carried": [], "addresses": [], "by_page": true,
                   "origin": "http://localhost:5173", "path": "/x"}),
        ),
        row(
            4,
            "artifact.create",
            owner.clone(),
            on("a1", None),
            json!({"title": "SENTINEL-TEXT-create", "kind": "html", "icon": null,
                   "capabilities": {"custom": {"prompt": "SENTINEL-TEXT-capability"}},
                   "contract_version": "1"}),
        ),
        row(
            5,
            "artifact.update",
            owner.clone(),
            on("a1", None),
            json!({"fields": {"title": "SENTINEL-TEXT-newtitle",
                              "description": "SENTINEL-TEXT-description", "pinned": true}}),
        ),
        row(
            6,
            "artifact.delete",
            owner.clone(),
            on("a1", None),
            json!({"title": "SENTINEL-TEXT-deleted", "current_version": 4}),
        ),
        row(
            7,
            "question.ask",
            agent_json("/t.jsonl"),
            q.clone(),
            json!({"source": "ask", "tool_use_id": "toolu_1",
                   "questions": [{"question": "SENTINEL-TEXT-question", "options": [{"label": "SENTINEL-TEXT-option"}]}]}),
        ),
        row(
            8,
            "question.answer",
            owner.clone(),
            q.clone(),
            json!({"answers": [{"answer": "SENTINEL-TEXT-answer"}], "answered_via": "shell"}),
        ),
        row(
            9,
            "question.decline",
            owner.clone(),
            q.clone(),
            json!({"reason": "SENTINEL-TEXT-decline"}),
        ),
        row(
            10,
            "question.release",
            owner.clone(),
            q.clone(),
            json!({"reason": "SENTINEL-TEXT-release"}),
        ),
        row(
            11,
            "question.withdraw",
            owner.clone(),
            q,
            json!({"reason": "SENTINEL-TEXT-withdraw"}),
        ),
        row(
            12,
            "working.start",
            agent_json("/t.jsonl"),
            on("a1", None),
            json!({"key": "k", "message": "SENTINEL-TEXT-message", "thread_ids": []}),
        ),
        row(
            13,
            "thread.open",
            owner.clone(),
            on("a1", Some("t1")),
            json!({"version_n": 1, "anchor": {"kind": "text", "selector": "h1",
                   "quote": "SENTINEL-TEXT-quote", "prefix": "SENTINEL-TEXT-prefix",
                   "suffix": "SENTINEL-TEXT-suffix", "html_hash": null, "file": null, "route": null},
                   "live_path": null, "has_clip": false, "first_comment_id": "c1"}),
        ),
        row(
            14,
            "backfill.skip",
            json!({"type": "system", "reason": "backfill"}),
            AuditIds::default(),
            json!({"table": "comments", "row_id": "c9", "reason": "SENTINEL-TEXT-reason", "via": "daemon"}),
        ),
    ]
}

/// Every string in `v` that contains `needle`, with where it is.
fn find(v: &Value, needle: &str, at: &str, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.contains(needle) => out.push(format!("{at}: {s}")),
        Value::Object(m) => {
            for (k, x) in m {
                if k.contains(needle) {
                    out.push(format!("{at}: key {k}"));
                }
                find(x, needle, &format!("{at}/{k}"), out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                find(x, needle, &format!("{at}/{i}"), out);
            }
        }
        _ => {}
    }
}

/// The steps and actor definitions `rows` render to under `opts`.
fn rendered(rows: &[AuditRow], opts: &Redaction) -> Value {
    let env = RenderEnv::export("6a1f0c3e9b2d4785a0c1e2f3d4b5a697", "http://localhost:7480");
    let steps = render_chain(rows, &env, opts);
    let defs: Vec<Value> = rows
        .iter()
        .flat_map(|r| actor_defs(r, &env, opts))
        .map(|(k, d)| json!({"actor": k, "definition": d}))
        .collect();
    json!({"steps": steps, "defs": defs})
}

fn hits(v: &Value, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    find(v, needle, "", &mut out);
    out
}

fn text_hash(s: &str) -> String {
    format!("sha256:{}", crate::audit::sha256_hex(s.as_bytes()))
}

#[test]
fn no_text_hashes_bodies() {
    let rows = text_rows();
    let plain = rendered(&rows, &Redaction::NONE);
    assert!(
        hits(&plain, "SENTINEL-TEXT").len() >= 20,
        "the sentinels are rendered without --no-text"
    );
    for opts in [
        Redaction {
            no_text: true,
            ..Redaction::NONE
        },
        Redaction::ALL,
    ] {
        let out = rendered(&rows, &opts);
        let leaks = hits(&out, "SENTINEL-TEXT");
        assert!(leaks.is_empty(), "--no-text leaks:\n{}", leaks.join("\n"));
        let st = |i: usize| -> &Value {
            out["steps"][i]["change"]
                .as_object()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .get("structural")
                .unwrap()
        };
        let redacted = |s: &str| json!({"redacted": "text", "sha256": text_hash(s)});
        assert_eq!(st(0)["body"], redacted("SENTINEL-TEXT-body"));
        assert_eq!(st(1)["note"], redacted("SENTINEL-TEXT-note"));
        assert_eq!(st(1)["label"], redacted("SENTINEL-TEXT-label"));
        assert_eq!(st(1)["title"], redacted("SENTINEL-TEXT-title"));
        assert_eq!(st(4)["fields"]["title"], redacted("SENTINEL-TEXT-newtitle"));
        assert_eq!(st(4)["fields"]["pinned"], true);
        assert_eq!(st(8)["reason"], redacted("SENTINEL-TEXT-decline"));
        assert_eq!(st(12)["anchor"]["quote"], redacted("SENTINEL-TEXT-quote"));
        assert_eq!(st(12)["anchor"]["selector"], "h1");
        // Structured text is hashed as its compact JSON.
        let questions = json!([{"question": "SENTINEL-TEXT-question",
                                "options": [{"label": "SENTINEL-TEXT-option"}]}]);
        assert_eq!(st(6)["questions"], redacted(&questions.to_string()));
        // What is not text stays.
        assert_eq!(st(0)["comment_id"], "c1");
        assert_eq!(st(1)["n"], 3);
        // The description no longer names the title.
        assert_eq!(
            out["steps"][1]["meta"]["description"],
            "Published version 3 of artifact a1"
        );
    }
    // Null text stays null.
    let r = row(
        1,
        "version.publish",
        json!({"type": "anonymous"}),
        on("a1", None),
        json!({"n": 1, "label": null, "note": null, "title": "T"}),
    );
    let s = render_step(&r, None, &RenderEnv::journal("i"), &Redaction::ALL).unwrap();
    assert_eq!(
        s["change"]["clax://i/a/a1/v/1"]["structural"]["note"],
        Value::Null
    );
}

#[test]
fn no_names_keeps_public_ids() {
    let viewer =
        json!({"type": "viewer", "public_id": "u_77c0", "display_name": "SENTINEL-NAME-viewer"});
    let rows = vec![
        row(
            1,
            "comment.add",
            viewer.clone(),
            on("a1", Some("t1")),
            json!({"comment_id": "c1", "body": "hello", "author_kind": "viewer",
                   "author_name": "SENTINEL-NAME-author", "via_harness": null, "via_page": false}),
        ),
        row(
            2,
            "thread.open",
            json!({"type": "system", "reason": "backfill"}),
            on("a1", Some("t1")),
            json!({"version_n": 1, "anchor": null, "live_path": null, "has_clip": false,
                   "first_comment_id": "c1", "via": "daemon",
                   "for_actor": {"type": "viewer", "public_id": "u_88d1",
                                 "display_name": "SENTINEL-NAME-for"}}),
        ),
    ];
    let plain = rendered(&rows, &Redaction::NONE);
    assert_eq!(hits(&plain, "SENTINEL-NAME").len(), 4);
    for opts in [
        Redaction {
            no_names: true,
            ..Redaction::NONE
        },
        Redaction::ALL,
    ] {
        let out = rendered(&rows, &opts);
        let leaks = hits(&out, "SENTINEL-NAME");
        assert!(leaks.is_empty(), "--no-names leaks:\n{}", leaks.join("\n"));
        let s0 = &out["steps"][0];
        assert_eq!(s0["step"]["actor"], "human:clax-viewer/u_77c0");
        let st =
            &s0["change"]["clax://6a1f0c3e9b2d4785a0c1e2f3d4b5a697/a/a1/t/t1/c/c1"]["structural"];
        assert_eq!(st["author_name"], json!({"redacted": "name"}));
        assert_eq!(
            out["steps"][1]["change"]["clax://6a1f0c3e9b2d4785a0c1e2f3d4b5a697/a/a1/t/t1"]["structural"]
                ["for_actor"],
            json!({"type": "viewer", "public_id": "u_88d1", "display_name": {"redacted": "name"}})
        );
        assert_eq!(
            out["steps"][1]["meta"]["clax"]["for_actor"],
            "human:clax-viewer/u_88d1"
        );
        // Public IDs stay, in the actor strings and the identities.
        let defs = out["defs"].as_array().unwrap();
        assert!(defs.iter().any(|d| d["actor"] == "human:clax-viewer/u_77c0"
            && d["definition"]["identities"][0]["id"]
                == "6a1f0c3e9b2d4785a0c1e2f3d4b5a697/u_77c0"
            && d["definition"].get("name").is_none()));
        assert!(
            defs.iter()
                .any(|d| d["actor"] == "human:clax-viewer/u_88d1")
        );
    }
}

#[test]
fn no_paths_redacts_and_drops_file_refs() {
    let agent = agent_json("/SENTINEL-PATH/t/3f2c.jsonl");
    let git = json!({"repo_root": "/SENTINEL-PATH/repo", "remote": "origin",
                     "remote_url": "git@github.com:empathic/app.git",
                     "head": "9c1e5d2b7a4f3e8d1c6b0a9f2e7d4c3b8a1f6e5d", "dirty": false,
                     "untracked": 0, "captured_at": "2026-10-06T14:03:11.512Z"});
    let local = json!({"repo_root": "/SENTINEL-PATH/lib", "remote": "origin",
                       "remote_url": "/SENTINEL-PATH/remote.git",
                       "head": "1111111111111111111111111111111111111111", "dirty": false,
                       "untracked": 0, "captured_at": "2026-10-06T14:03:11.512Z"});
    let no_remote = json!({"repo_root": "/SENTINEL-PATH/solo",
                           "head": "2222222222222222222222222222222222222222", "dirty": false,
                           "untracked": 0, "captured_at": "2026-10-06T14:03:11.512Z"});
    let session = AuditIds {
        session: Some(SID.into()),
        ..AuditIds::default()
    };
    let rows = vec![
        row(
            1,
            "session.start",
            agent.clone(),
            session.clone(),
            json!({"harness": "claude", "harness_session_id": "3f2c", "cwd": "/SENTINEL-PATH/cwd",
                   "transcript_path": "/SENTINEL-PATH/t/3f2c.jsonl", "pid": 1,
                   "git": git, "git_capture": "ok"}),
        ),
        row(
            2,
            "artifact.create",
            agent.clone(),
            on("a1", None),
            json!({"title": "T", "kind": "html", "icon": null, "capabilities": null,
                   "contract_version": "1", "git": local, "git_capture": "ok"}),
        ),
        row(
            3,
            "artifact.update",
            agent.clone(),
            on("a1", None),
            json!({"fields": {"pinned": true}, "git": no_remote, "git_capture": "ok"}),
        ),
        row(
            4,
            "session.end",
            json!({"type": "system", "reason": "ttl"}),
            session,
            json!({"reason": "ttl", "via": "daemon", "for_actor": agent}),
        ),
        // URLs whose query can hold a token are paths.
        row(
            5,
            "thread.move",
            json!({"type": "owner", "public_id": "u_own"}),
            AuditIds {
                artifact2: Some("a2".into()),
                ..on("a1", Some("t1"))
            },
            json!({"from_artifact_id": "a1", "to_artifact_id": "a2", "move_kind": "move",
                   "rule_id": null, "move_id": "m1",
                   "from_url": "http://localhost:5173/x?code=SENTINEL-PATH-from",
                   "to_url": "http://localhost:5173/y#SENTINEL-PATH-to"}),
        ),
        row(
            6,
            "thread.open",
            json!({"type": "owner", "public_id": "u_own"}),
            on("a1", Some("t2")),
            json!({"version_n": 1, "live_path": "/x", "has_clip": false, "first_comment_id": "c1",
                   "anchor": {"kind": "text", "selector": "h1", "quote": null, "prefix": null,
                              "suffix": null, "html_hash": null, "file": null,
                              "route": "?q=SENTINEL-PATH-route"}}),
        ),
    ];
    let plain = rendered(&rows, &Redaction::NONE);
    assert!(!hits(&plain, "file://").is_empty());
    assert!(hits(&plain, "SENTINEL-PATH").len() >= 10);
    for opts in [
        Redaction {
            no_paths: true,
            ..Redaction::NONE
        },
        Redaction::ALL,
    ] {
        let out = rendered(&rows, &opts);
        let leaks = hits(&out, "SENTINEL-PATH");
        assert!(leaks.is_empty(), "--no-paths leaks:\n{}", leaks.join("\n"));
        let files = hits(&out, "file://");
        assert!(
            files.is_empty(),
            "--no-paths keeps file refs:\n{}",
            files.join("\n")
        );
        let s0 = &out["steps"][0];
        let st = &s0["change"][format!("clax://6a1f0c3e9b2d4785a0c1e2f3d4b5a697/s/{SID}").as_str()]
            ["structural"];
        assert_eq!(
            st["cwd"],
            json!({"redacted": "path", "sha256": text_hash("/SENTINEL-PATH/cwd")})
        );
        assert_eq!(
            s0["meta"]["clax"]["git"]["repo_root"],
            json!({"redacted": "path", "sha256": text_hash("/SENTINEL-PATH/repo")})
        );
        // A hosted remote stays, and so does its at-revision ref.
        assert_eq!(
            s0["meta"]["clax"]["git"]["remote_url"],
            "git@github.com:empathic/app.git"
        );
        let refs = s0["meta"]["refs"].as_array().unwrap();
        assert!(refs.contains(&json!({"rel": "at-revision",
            "href": "git:github:empathic/app@9c1e5d2b7a4f3e8d1c6b0a9f2e7d4c3b8a1f6e5d"})));
        assert!(!refs.iter().any(|r| r["rel"] == "transcript"));
        assert!(
            refs.contains(&json!({"rel": "agent-session", "href": "agent://claude-code/3f2c"}))
        );
        // A local remote, or none, names a path: no at-revision.
        for i in [1, 2] {
            let refs = out["steps"][i]["meta"]["refs"].as_array().unwrap();
            assert!(!refs.iter().any(|r| r["rel"] == "at-revision"), "{refs:?}");
        }
        // The transcript identity keeps its hash only.
        let def = &out["defs"][0]["definition"];
        assert!(def["identities"].as_array().unwrap().contains(&json!({
            "system": "claude-code-transcript-sha256",
            "id": text_hash("/SENTINEL-PATH/t/3f2c.jsonl"),
        })));
    }
    // Without --no-paths the refs carry the file URLs.
    let refs = plain["steps"][2]["meta"]["refs"].as_array().unwrap();
    assert!(refs.contains(&json!({"rel": "at-revision",
        "href": "git:file:///SENTINEL-PATH/solo@2222222222222222222222222222222222222222"})));
    assert!(
        refs.contains(&json!({"rel": "transcript", "href": "file:///SENTINEL-PATH/t/3f2c.jsonl"}))
    );
}

#[test]
fn args_hash_never_redacted() {
    let h = history();
    let call_rows: Vec<&AuditRow> = h
        .rows
        .iter()
        .filter(|r| r.body.contains("args_sha256"))
        .collect();
    assert!(call_rows.len() >= 10);
    for opts in OPTION_SETS {
        for r in &call_rows {
            let body: Value = serde_json::from_str(&r.body).unwrap();
            let s = render_step(r, None, &h.env, &opts).unwrap();
            if let Some(want) = body.pointer("/call/args_sha256") {
                assert_eq!(&s["meta"]["clax"]["call"]["args_sha256"], want, "{opts:?}");
            }
            if let Some(want) = body.get("args_sha256") {
                let st = s["change"].as_object().unwrap().values().next().unwrap();
                assert_eq!(&st["structural"]["args_sha256"], want, "{opts:?}");
            }
        }
    }
}

#[test]
fn no_step_has_meta_source() {
    let h = history();
    for opts in OPTION_SETS {
        for s in render_chain(&h.rows, &h.env, &opts) {
            assert!(s["meta"].get("source").is_none(), "{s}");
            assert!(s["meta"].get("signatures").is_none(), "{s}");
        }
    }
}

/// The golden file name of the export under `opts`.
fn export_golden_name(opts: &Redaction) -> String {
    let names = opts.names();
    let slug = match names.len() {
        0 => "none".to_string(),
        3 => "all".to_string(),
        _ => names.join("+"),
    };
    format!("export.{slug}.path.json")
}

pub(super) fn pretty(v: &Value) -> String {
    let mut text = serde_json::to_string_pretty(v).unwrap();
    text.push('\n');
    text
}

/// Every golden export (one per option set), the sealed journal segment
/// and an unrenderable step are written as `expected/*.path.json`; the web
/// unit gate validates each against the vendored schema with Ajv
/// (`web/scripts/toolpath-schema.test.ts`).
#[test]
fn golden_exports_match_and_conform() {
    let h = history();
    let export = export_graph(&h, &Redaction::NONE);
    assert_valid(&export, "the golden export");
    // The checks have teeth: each of these breaks the schema.
    let broken: [fn(&mut Value); 3] = [
        |d| d["paths"][0]["steps"][0]["step"]["actor"] = "agent:a b".into(),
        |d| d["paths"][0]["steps"][0]["step"]["timestamp"] = "yesterday".into(),
        |d| {
            let change = d["paths"][0]["steps"][0]["change"].as_object_mut().unwrap();
            let c = change.values_mut().next().unwrap();
            c["structural"].as_object_mut().unwrap().remove("type");
        },
    ];
    for (i, b) in broken.iter().enumerate() {
        let mut doc = export.clone();
        b(&mut doc);
        assert!(
            !conformance_errors(&doc).is_empty(),
            "breakage {i} passed the checks"
        );
    }
    // Every kind is in it, and every redaction validates too.
    let kinds: BTreeSet<&str> = export["paths"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["steps"].as_array().unwrap())
        .map(|s| s["meta"]["clax"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.len(), AuditKind::ALL.len());
    for opts in OPTION_SETS {
        let text = export_text(&h, &opts);
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_valid(&doc, &format!("the export under {opts:?}"));
        golden(&export_golden_name(&opts), &text);
    }
    // A thread moved between artifacts is in both paths, under one ID.
    let moved: Vec<&Value> = export["paths"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["steps"].as_array().unwrap())
        .filter(|s| s["meta"]["clax"]["kind"] == "thread.move")
        .collect();
    assert_eq!(moved.len(), 2);
    assert_eq!(moved[0]["step"]["id"], moved[1]["step"]["id"]);
}

/// The journal's segments, as the segment writer writes them: the golden
/// history as a first segment, closed, is the golden
/// `segment.path.jsonl`, the same as the shape §7.2 describes
/// ([`segment`]); sealed by the JSONL RFC's reading rules, it and the
/// still-open segment conform, and so does every segment of a journal that
/// rotates by day and by size.
#[test]
fn segments_seal_and_validate_against_schema() {
    use super::segment::{MemFs, SegmentConfig, SegmentWriter};
    use std::path::Path;
    let h = history();
    let cfg = SegmentConfig::new(
        h.env.install.clone(),
        "0.3.1",
        "abc1234def5678abc1234def5678abc1234def56",
    );
    let clock = std::sync::Arc::new(crate::working::ManualClock::at("2026-10-06T12:00:00Z"));
    let fs = MemFs::new();
    let mut w =
        SegmentWriter::open_or_recover("/j", cfg.clone(), clock.clone(), Box::new(fs.clone()))
            .unwrap();
    w.append_batch(&h.rows).unwrap();
    let file = Path::new("/j/2026/10/clax-6a1f0c3e-20261006-001.path.jsonl");
    let open = fs.text(file).unwrap();
    assert_eq!(open, segment(&h, false));
    w.close().unwrap();
    let closed = fs.text(file).unwrap();
    assert_eq!(closed, segment(&h, true));
    golden("segment.path.jsonl", &closed);
    let rendered = render_chain(
        &h.rows,
        &RenderEnv::journal(h.env.install.clone()),
        &Redaction::NONE,
    );
    // The journal carries no browser URLs: the port can change between writes.
    assert!(!closed.contains("http://localhost:7480") && !closed.contains("\"view\""));
    golden(
        "segment.sealed.path.json",
        &pretty(&seal(&closed).unwrap().graph),
    );
    for (what, text) in [("closed", &closed), ("open", &open)] {
        let sealed = seal(text).unwrap_or_else(|e| panic!("the {what} segment: {e}"));
        assert!(sealed.warnings.is_empty(), "{:?}", sealed.warnings);
        assert_valid(&sealed.graph, &format!("the sealed {what} segment"));
        let path = &sealed.graph["paths"][0];
        assert_eq!(path["path"]["head"], step_id(h.rows.last().unwrap().seq));
        assert_eq!(path["steps"].as_array().unwrap(), &rendered);
        assert_eq!(path["meta"]["kind"], KIND_URI);
        let actors = path["meta"]["actors"].as_object().unwrap();
        for s in &rendered {
            assert!(actors.contains_key(s["step"]["actor"].as_str().unwrap()));
        }
    }

    // A journal that rotates by day and by size: every segment seals and
    // conforms, each names the one before it, and together they hold every
    // step once.
    let rows: Vec<AuditRow> = h
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut r = r.clone();
            if i >= 40 {
                r.at = r.at.replace("2026-10-06", "2026-10-07");
            }
            r
        })
        .collect();
    let fs = MemFs::new();
    let mut w = SegmentWriter::open_or_recover(
        "/j",
        SegmentConfig {
            max_bytes: 16 << 10,
            ..cfg
        },
        clock,
        Box::new(fs.clone()),
    )
    .unwrap();
    w.append_batch(&rows).unwrap();
    w.close().unwrap();
    let paths = fs.paths();
    assert!(paths.len() > 2, "{paths:?}");
    let mut steps = Vec::new();
    let mut prev: Option<String> = None;
    for p in &paths {
        let sealed = seal(&fs.text(p).unwrap()).unwrap();
        assert!(sealed.warnings.is_empty(), "{:?}", sealed.warnings);
        assert_valid(&sealed.graph, &p.display().to_string());
        let path = &sealed.graph["paths"][0];
        let refs = &path["meta"]["refs"];
        match &prev {
            None => assert!(refs.is_null()),
            Some(name) => assert_eq!(refs, &json!([{"rel": "continues", "href": name}])),
        }
        prev = Some(p.file_name().unwrap().to_string_lossy().into_owned());
        let ps = path["steps"].as_array().unwrap();
        assert!(
            ps[0]["step"].get("parents").is_none(),
            "no parents across paths"
        );
        steps.extend(ps.iter().map(|s| s["step"]["id"].clone()));
    }
    let want: Vec<Value> = rows.iter().map(|r| step_id(r.seq).into()).collect();
    assert_eq!(steps, want);
}

#[test]
fn seal_follows_the_reading_rules() {
    let open = r#"{"PathOpen":{"version":"1","id":"p"}}"#;
    let step = |id: &str, parent: Option<&str>| {
        let mut s = json!({"step": {"id": id, "actor": "human:a", "timestamp": "2026-10-06T00:00:00Z"},
                           "change": {"x": {"structural": {"type": "t"}}}});
        if let Some(p) = parent {
            s["step"]["parents"] = json!([p]);
        }
        json!({"Step": s}).to_string()
    };
    let lines = |ls: &[String]| ls.iter().map(|l| format!("{l}\n")).collect::<String>();
    // A single tip is the head; an unknown variant is skipped with a warning.
    let ok = seal(&lines(&[
        open.into(),
        step("a", None),
        r#"{"Future":{}}"#.into(),
        step("b", Some("a")),
    ]))
    .unwrap();
    assert_eq!(ok.graph["paths"][0]["path"]["head"], "b");
    assert_eq!(ok.warnings.len(), 1);
    // Two tips and no Head line: fatal. A Head line settles it.
    let forked = [
        open.into(),
        step("a", None),
        step("b", Some("a")),
        step("c", Some("a")),
    ];
    assert!(seal(&lines(&forked)).is_err());
    let mut headed = forked.to_vec();
    headed.push(r#"{"Head":{"step_id":"c"}}"#.into());
    assert_eq!(
        seal(&lines(&headed)).unwrap().graph["paths"][0]["path"]["head"],
        "c"
    );
    // Fatal: no PathOpen first, malformed JSON, a partial last line, a step
    // signature before its step.
    assert!(seal(&lines(&[step("a", None)])).is_err());
    assert!(seal(&lines(&[open.into(), "{".into()])).is_err());
    assert!(seal(&format!("{open}\n{}", step("a", None))).is_err());
    assert!(
        seal(&lines(&[
            open.into(),
            r#"{"Signature":{"target":"step:a","signature":{}}}"#.into(),
            step("a", None),
        ]))
        .is_err()
    );
}

#[test]
fn normalize_remote_follows_toolpath_git() {
    // The documented cases of toolpath_git::normalize_git_url at Toolpath
    // commit 77dc16a5 (see tests/toolpath/schema/SOURCE).
    for (url, want) in [
        ("git@github.com:org/repo.git", "github:org/repo"),
        ("https://github.com/org/repo.git", "github:org/repo"),
        ("https://github.com/org/repo", "github:org/repo"),
        ("git@gitlab.com:org/repo.git", "gitlab:org/repo"),
        ("https://gitlab.com/org/repo.git", "gitlab:org/repo"),
        ("https://gitlab.com/org/repo", "gitlab:org/repo"),
        (
            "https://bitbucket.org/org/repo",
            "https://bitbucket.org/org/repo",
        ),
        (
            "https://bitbucket.org/org/repo.git",
            "https://bitbucket.org/org/repo.git",
        ),
    ] {
        assert_eq!(normalize_remote(url), want, "{url}");
    }
}

#[test]
fn unknown_kinds_render_and_bad_rows_do_not() {
    let mut unrenderable = Vec::new();
    let env = RenderEnv::journal("i");
    let future = row(
        7,
        "future.thing",
        json!({"type": "owner", "public_id": "u_1"}),
        on("a1", None),
        json!({"x": 1}),
    );
    let s = render_step(&future, Some(6), &env, &Redaction::NONE).unwrap();
    assert_eq!(
        s["change"]["clax://i/a/a1"]["structural"],
        json!({"type": "clax.future.thing", "x": 1})
    );
    assert_eq!(s["meta"]["description"], "Recorded future.thing");
    for (bad, err) in [
        (
            AuditRow {
                actor: "{\"type\":\"martian\"}".into(),
                ..future.clone()
            },
            RenderError::Actor,
        ),
        (
            AuditRow {
                body: "[1]".into(),
                ..future.clone()
            },
            RenderError::Body,
        ),
        (
            AuditRow {
                body: "SECRET not json".into(),
                ..future.clone()
            },
            RenderError::Body,
        ),
        (
            AuditRow {
                kind: "Bad Kind".into(),
                ..future.clone()
            },
            RenderError::Kind,
        ),
    ] {
        assert_eq!(render_step(&bad, None, &env, &Redaction::NONE), Err(err));
        let u = unrenderable_step(&bad, Some(6), &env);
        assert!(!u.to_string().contains("SECRET") && !u.to_string().contains("martian"));
        assert_eq!(u["step"]["parents"], json!(["e000000000006"]));
        unrenderable.push(u);
    }
    let graph = json!({"graph": {"id": "g"}, "paths": [
        {"path": {"id": "p", "head": "e000000000007"}, "steps": unrenderable}]});
    assert_valid(&graph, "unrenderable steps");
    golden("unrenderable.path.json", &pretty(&graph));
}

#[test]
fn redaction_is_deny_by_default() {
    let owner = json!({"type": "owner", "public_id": "u_own"});
    let rows = vec![
        // A kind this build does not know.
        row(
            1,
            "comment.edit",
            owner.clone(),
            on("a1", Some("t1")),
            json!({"text": "SENTINEL-NEW-text", "cwd": "/SENTINEL-NEW/cwd",
                   "nested": {"author_name": "SENTINEL-NEW-name"}, "n": 3}),
        ),
        // New fields on a known kind, at every depth.
        row(
            2,
            "comment.add",
            owner.clone(),
            on("a1", Some("t1")),
            json!({"comment_id": "c1", "body": "b", "author_kind": "owner", "author_name": "O",
                   "via_harness": null, "via_page": false,
                   "edited_from": "SENTINEL-NEW-edited",
                   "for_actor": {"type": "viewer", "public_id": "u_1", "nickname": "SENTINEL-NEW-nick"},
                   "git": {"repo_root": "/r", "head": "1111111111111111111111111111111111111111",
                           "dirty": false, "untracked": 0, "captured_at": "2026-10-06T14:03:11.512Z",
                           "stash_note": "SENTINEL-NEW-git"},
                   "call": {"call_id": "01JBC0000000000000000000C1", "tool": "publish",
                            "args_sha256": "sha256:00", "started_at": "2026-10-06T14:03:11.402Z",
                            "args": {"body": "SENTINEL-NEW-args"}}}),
        ),
        row(
            3,
            "thread.open",
            owner.clone(),
            on("a1", Some("t1")),
            json!({"version_n": 1, "live_path": null, "has_clip": false, "first_comment_id": "c1",
                   "anchor": {"kind": "text", "selector": "h1", "quote": "q", "prefix": null,
                              "suffix": null, "html_hash": null, "file": null, "route": null,
                              "context": "SENTINEL-NEW-context"}}),
        ),
        row(
            4,
            "artifact.update",
            owner,
            on("a1", None),
            json!({"fields": {"pinned": true, "summary": "SENTINEL-NEW-summary"}}),
        ),
    ];
    let unclassified: Vec<Vec<String>> = rows
        .iter()
        .map(|r| redact::unclassified(&r.kind, &serde_json::from_str(&r.body).unwrap()))
        .collect();
    assert_eq!(unclassified[0], ["cwd", "n", "nested", "text"]);
    assert_eq!(
        unclassified[1],
        [
            "call/args",
            "edited_from",
            "for_actor/nickname",
            "git/stash_note"
        ]
    );
    assert_eq!(unclassified[2], ["anchor/context"]);
    assert_eq!(unclassified[3], ["fields/summary"]);

    let plain = rendered(&rows, &Redaction::NONE);
    assert_eq!(hits(&plain, "SENTINEL-NEW").len(), 9);
    for opts in &OPTION_SETS[1..] {
        let out = rendered(&rows, opts);
        let leaks = hits(&out, "SENTINEL-NEW");
        assert!(leaks.is_empty(), "{opts:?} leaks:\n{}", leaks.join("\n"));
        let st = out["steps"][0]["change"]["clax://6a1f0c3e9b2d4785a0c1e2f3d4b5a697/a/a1/t/t1"]
            ["structural"]
            .clone();
        assert_eq!(
            st["text"],
            json!({"redacted": "unclassified", "sha256": text_hash("SENTINEL-NEW-text")})
        );
        assert_eq!(st["type"], "clax.comment.edit");
        let clax = &out["steps"][1]["meta"]["clax"];
        assert_eq!(
            clax["call"]["args_sha256"], "sha256:00",
            "argument hashes stay"
        );
        assert_eq!(clax["call"]["args"]["redacted"], "unclassified");
    }
}

#[test]
fn every_recorded_kind_is_classified_and_renders_conformant() {
    use crate::store::backfill::tests::live_history;
    use crate::store::test_util::{session, store};
    use crate::working::{StopReason, Transition};
    let (_d, st) = store();
    live_history(&st);
    let sid = session(&st, "claude", "h9");
    st.record_working(
        &crate::audit::AuditCtx::DAEMON,
        &[
            Transition::Started {
                session_id: sid.clone(),
                artifact_id: "a1".into(),
                key: "k".into(),
                message: Some("working on it".into()),
                thread_ids: vec![],
            },
            Transition::Stopped {
                session_id: sid.clone(),
                artifact_id: "a1".into(),
                key: "k".into(),
                harness: "claude".into(),
                agent: "a_x".into(),
                reason: StopReason::Ttl,
                duration_ms: 1,
            },
        ],
    )
    .unwrap();
    // A tool call, with every optional field, from the live builder.
    let call = crate::audit::CallHeader {
        call_id: "01JBC0000000000000000000C7".into(),
        tool: "list".into(),
        harness_tool: Some("mcp__plugin_clax_clax__list".into()),
        args_sha256: crate::toolpath::args::args_sha256(&json!({})),
        started_at: "2026-10-06T14:03:11.402Z".into(),
        harness_call_id: Some("toolu_01C7".into()),
    };
    let mut report = crate::audit::ToolCallReport::new(
        call,
        "2026-10-06T14:03:11.913Z".into(),
        crate::audit::ToolOutcome::Ok,
    );
    report.artifact_id = Some("a1".into());
    st.record_tool_call(&crate::audit::AuditCtx::DAEMON, &report)
        .unwrap()
        .expect("recorded");
    let live = st.events_after(0, 100_000).unwrap();
    let home = st.home().clone();
    st.with_write(|c| {
        c.execute_batch("DELETE FROM audit_events; DELETE FROM install WHERE k = 'backfill';")?;
        Ok(())
    })
    .unwrap();
    drop(st);
    let st = crate::Store::open(&home).unwrap();
    let backfilled = st.events_after(0, 100_000).unwrap();
    assert!(backfilled.iter().all(|r| r.backfilled) && !backfilled.is_empty());
    let install = st.install_id().unwrap();

    const FIXTURE_TEXTS: [&str; 6] = ["one", "two", "on it", "Ana", "Claude", "working on it"];
    let mut plain: BTreeMap<&str, usize> = BTreeMap::new();
    let mut kinds = BTreeSet::new();
    for rows in [&live, &backfilled] {
        for r in rows.iter() {
            kinds.insert(r.kind.clone());
            let body: Map<String, Value> = serde_json::from_str(&r.body).unwrap();
            let missing = redact::unclassified(&r.kind, &body);
            assert!(missing.is_empty(), "{}: unclassified {missing:?}", r.kind);
            assert_eq!(
                body["clax_version"],
                env!("CARGO_PKG_VERSION"),
                "stamped at record time"
            );
            assert_eq!(body["clax_commit"], crate::build_commit());
        }
        for opts in OPTION_SETS {
            let env = RenderEnv::export(install.clone(), "http://localhost:7480");
            let steps = render_chain(rows, &env, &opts);
            let doc = json!({"graph": {"id": "g"}, "paths": [{
                "path": {"id": "p", "head": steps.last().unwrap()["step"]["id"]},
                "steps": steps}]});
            assert_valid(&doc, &format!("recorded events under {opts:?}"));
            // The fixture's comment bodies, names and working message:
            // rendered as written without options (the positive control),
            // and never under every option.
            for text in FIXTURE_TEXTS {
                let found: Vec<String> = hits(&doc, text)
                    .into_iter()
                    .filter(|h| h.ends_with(&format!(": {text}")))
                    .collect();
                if opts == Redaction::NONE {
                    *plain.entry(text).or_default() += found.len();
                }
                if opts == Redaction::ALL {
                    assert!(found.is_empty(), "{text} survives redaction: {found:?}");
                }
            }
        }
    }
    for text in FIXTURE_TEXTS {
        assert!(
            plain[text] > 0,
            "{text} is not in the recorded events: {plain:?}"
        );
    }
    assert!(kinds.len() >= 19, "{kinds:?}");
}

#[test]
fn rerendering_is_byte_identical_across_port_and_build() {
    let h = history();
    let install = h.env.install.clone();
    // The journal renders no browser URL: one port or another, the same
    // bytes. An export's renderings differ only in their view refs and URL.
    let journal = render_chain(
        &h.rows,
        &RenderEnv::journal(install.clone()),
        &Redaction::NONE,
    );
    for port in ["http://localhost:7480", "http://127.0.0.1:7481/"] {
        let mut export = render_chain(
            &h.rows,
            &RenderEnv::export(install.clone(), port),
            &Redaction::NONE,
        );
        for s in &mut export {
            s["meta"]["clax"].as_object_mut().unwrap().remove("url");
            let meta = s["meta"].as_object_mut().unwrap();
            if let Some(Value::Array(refs)) = meta.get_mut("refs") {
                refs.retain(|r| r["rel"] != "view");
                if refs.is_empty() {
                    meta.remove("refs");
                }
            }
        }
        assert_eq!(
            serde_json::to_string(&export).unwrap(),
            serde_json::to_string(&journal).unwrap()
        );
    }
    // A row names the build that recorded it, whichever build renders it:
    // the renderer reads no build of its own.
    let old = row(
        5,
        "artifact.delete",
        json!({"type": "system", "reason": "ttl"}),
        on("a1", None),
        json!({"title": "T", "current_version": 1, "clax_version": "0.2.9",
               "clax_commit": "0123456789abcdef0123456789abcdef01234567"}),
    );
    let r = render(&old, None, &RenderEnv::journal("i"), &Redaction::NONE).unwrap();
    assert_eq!(r.step["step"]["actor"], "tool:clax/0.2.9");
    assert_eq!(r.step["meta"]["clax"]["clax_version"], "0.2.9");
    assert_eq!(r.step["meta"]["clax"]["system_reason"], "ttl");
    assert_eq!(
        r.actors[0].1["identities"][0]["id"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    let text = r.step.to_string();
    assert!(!text.contains(crate::build_commit()) || crate::build_commit() == "unknown");
    // A row that does not say renders `unknown`, never the rendering build.
    let unstamped = row(
        6,
        "artifact.delete",
        json!({"type": "system", "reason": "daemon"}),
        on("a1", None),
        json!({"title": "T", "current_version": 1}),
    );
    let r = render_step(&unstamped, None, &RenderEnv::journal("i"), &Redaction::NONE).unwrap();
    assert_eq!(r["step"]["actor"], "tool:clax/unknown");
    assert_eq!(r["meta"]["clax"]["clax_commit"], "unknown");
}

#[test]
fn merged_actor_definitions_keep_every_identity() {
    let a = json!({"name": "Claude Code", "identities": [
        {"system": "clax-session", "id": "S1"},
        {"system": "claude-code-transcript", "id": "/t/1.jsonl"}]});
    let b = json!({"name": "Claude Code", "provider": "anthropic", "identities": [
        {"system": "clax-session", "id": "S1"},
        {"system": "claude-code-transcript", "id": "/t/2.jsonl"}]});
    assert_eq!(
        merge_actor_def(&a, &b),
        json!({"name": "Claude Code", "provider": "anthropic", "identities": [
            {"system": "claude-code-transcript", "id": "/t/1.jsonl"},
            {"system": "claude-code-transcript", "id": "/t/2.jsonl"},
            {"system": "clax-session", "id": "S1"}]})
    );
    // Merging is idempotent: re-merging what a writer already wrote adds
    // nothing, so it re-emits only when a definition grows.
    let ab = merge_actor_def(&a, &b);
    assert_eq!(merge_actor_def(&ab, &b), ab);
}

#[test]
fn local_remotes_are_paths() {
    for (url, local) in [
        ("https://github.com/o/r.git", false),
        ("ssh://git@host/x", false),
        ("git@github.com:o/r.git", false),
        ("host:repo", false),
        ("file:///srv/r.git", true),
        ("FILE:///srv/r.git", true),
        ("file:/srv/r.git", true),
        ("/srv/r.git", true),
        ("../r", true),
        ("./r", true),
        ("repo-dir", true),
        ("~/r.git", true),
        ("C:\\repos\\r", true),
        ("C:/repos/r", true),
        ("dir/sub:x", true),
    ] {
        assert_eq!(redact::is_local_remote(url), local, "{url}");
    }
}

// --- export projections (spec §8), over the store's one read transaction ---

/// A store whose history is the golden history, row for row.
fn history_store(h: &History) -> (tempfile::TempDir, crate::Store) {
    let (dir, st) = crate::store::test_util::store();
    st.with_write(|c| {
        c.execute("DELETE FROM audit_events", [])?;
        for r in &h.rows {
            c.execute(
                "INSERT INTO audit_events (seq, at, kind, actor, artifact_id, artifact2_id,
                     thread_id, session_id, question_id, call_id, origin, body, backfilled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    r.seq,
                    r.at,
                    r.kind,
                    r.actor,
                    r.ids.artifact,
                    r.ids.artifact2,
                    r.ids.thread,
                    r.ids.session,
                    r.ids.question,
                    r.ids.call,
                    r.ids.origin,
                    r.body,
                    r.backfilled
                ],
            )?;
        }
        Ok(())
    })
    .unwrap();
    (dir, st)
}

/// `req` exported from `st` under the golden environment: its text, or the
/// error's code.
fn store_export(
    st: &crate::Store,
    h: &History,
    req: &project::Export,
) -> std::result::Result<String, String> {
    let mut out = Vec::new();
    match st.export(req, &export_env(h), &mut out) {
        Ok(()) => Ok(String::from_utf8(out).unwrap()),
        Err(crate::CoreError::Invalid { code, .. }) => {
            assert!(out.is_empty(), "a refused export wrote {} bytes", out.len());
            Err(code.to_string())
        }
        Err(e) => panic!("{e}"),
    }
}

fn select(sel: project::Selection) -> project::Export {
    project::Export {
        selection: sel,
        ..Default::default()
    }
}

/// The path IDs of a graph, in order.
fn path_ids(doc: &Value) -> Vec<String> {
    doc["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"]["id"].as_str().unwrap().to_string())
        .collect()
}

/// The `seq`s of a path's steps.
fn seqs_of(path: &Value) -> Vec<i64> {
    path["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["meta"]["clax"]["seq"].as_i64().unwrap())
        .collect()
}

fn path<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["paths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"]["id"] == id)
        .unwrap_or_else(|| panic!("no path {id}"))
}

fn step(path: &Value, seq: i64) -> &Value {
    path["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["meta"]["clax"]["seq"] == seq)
        .unwrap_or_else(|| panic!("no step {seq}"))
}

fn refs(step: &Value) -> Vec<(String, String)> {
    step["meta"]["refs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|r| {
            (
                r["rel"].as_str().unwrap().to_string(),
                r["href"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn parse(text: &str) -> Value {
    let doc: Value = serde_json::from_str(text).unwrap();
    assert_valid(&doc, "an export");
    doc
}

const S1: &str = "01JB9S00000000000000000001";
const S1_HARNESS: &str = "3f2c9a1e-5b7d-4c11-9e0a-2d6f8b1c0e44";
const S3: &str = "01JB9S00000000000000000003";

#[test]
fn export_twice_is_identical() {
    let h = history();
    let (_dir, st) = history_store(&h);
    for req in [
        project::Export::default(),
        project::Export {
            pretty: true,
            redaction: Redaction::ALL,
            ..Default::default()
        },
        project::Export {
            shape: project::Shape::Journal,
            format: project::Format::Jsonl,
            ..Default::default()
        },
        project::Export {
            shape: project::Shape::Journal,
            ..select(project::Selection {
                by_sessions: vec![S1.into()],
                since: Some("2026-10-06T14:02:00Z".into()),
                ..Default::default()
            })
        },
    ] {
        let first = store_export(&st, &h, &req).unwrap();
        assert_eq!(first, store_export(&st, &h, &req).unwrap(), "{req:?}");
        // The store's rows read through SQL project as the same rows held
        // in memory do.
        let mut mem = Vec::new();
        project::export(&history_source(&h), &req, &export_env(&h), &mut mem).unwrap();
        assert_eq!(first, String::from_utf8(mem).unwrap(), "{req:?}");
    }
    // Indenting changes the layout only.
    let compact = store_export(&st, &h, &project::Export::default()).unwrap();
    let pretty = store_export(
        &st,
        &h,
        &project::Export {
            pretty: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!compact.contains('\n') && pretty.ends_with("}\n"));
    assert_eq!(parse(&compact), parse(&pretty));
    // The golden export is this projection's.
    assert_eq!(pretty, export_text(&h, &Redaction::NONE));
}

#[test]
fn export_has_no_session_paths() {
    let h = history();
    let (_dir, st) = history_store(&h);
    let doc = parse(&store_export(&st, &h, &project::Export::default()).unwrap());
    let ids = path_ids(&doc);
    assert_eq!(
        ids,
        [
            "clax-artifact-b4ckf1llart0",
            "clax-artifact-k3m9q2w8x1ab",
            "clax-artifact-p7v2n4c8d1ef",
            "clax-artifact-q2w4e6r8t0yu",
            "clax-artifact-z9x8c7v6b5nm",
            "clax-install-6a1f0c3e",
        ]
    );
    // Every row is a step of the path of each artifact it names, or of the
    // install path; agent sessions are actors and refs, never paths.
    for r in &h.rows {
        let mut want: Vec<String> = [&r.ids.artifact, &r.ids.artifact2]
            .into_iter()
            .flatten()
            .map(|a| format!("clax-artifact-{a}"))
            .collect();
        if want.is_empty() {
            want.push("clax-install-6a1f0c3e".into());
        }
        let got: Vec<&String> = ids
            .iter()
            .filter(|id| seqs_of(path(&doc, id)).contains(&r.seq))
            .collect();
        want.sort();
        assert_eq!(got, want.iter().collect::<Vec<_>>(), "seq {}", r.seq);
    }
    for p in doc["paths"].as_array().unwrap() {
        assert_eq!(p["meta"]["kind"], KIND_URI);
        let seqs = seqs_of(p);
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(p["path"]["head"], step_id(*seqs.last().unwrap()));
        // Linear: each step's only parent is the path's previous step.
        let steps = p["steps"].as_array().unwrap();
        assert!(steps[0]["step"].get("parents").is_none());
        for w in steps.windows(2) {
            assert_eq!(w[1]["step"]["parents"], json!([w[0]["step"]["id"]]));
        }
    }
    assert_eq!(doc["meta"]["clax"]["first_seq"], 1);
    assert_eq!(doc["meta"]["clax"]["last_seq"], 62);
}

#[test]
fn by_session_filters_steps_not_shape() {
    let h = history();
    let (_dir, st) = history_store(&h);
    let by = |s: &str| {
        store_export(
            &st,
            &h,
            &select(project::Selection {
                by_sessions: vec![s.into()],
                ..Default::default()
            }),
        )
    };
    let text = by(S1).unwrap();
    let doc = parse(&text);
    // Still artifact paths and the install path: the session is no path.
    let ids = path_ids(&doc);
    assert!(
        ids.iter()
            .all(|id| id.starts_with("clax-artifact-") || id.starts_with("clax-install-"))
    );
    assert!(ids.contains(&"clax-install-6a1f0c3e".to_string()));
    assert!(ids.contains(&"clax-artifact-k3m9q2w8x1ab".to_string()));
    // The session's own steps, and the owner's and viewers' on its
    // artifacts; never another agent's.
    let k3 = seqs_of(path(&doc, "clax-artifact-k3m9q2w8x1ab"));
    for seq in [11, 12, 14, 15, 16, 22, 33, 34] {
        assert!(k3.contains(&seq), "seq {seq} in {k3:?}");
    }
    assert!(
        !k3.contains(&35),
        "session 3's doc write is not session 1's"
    );
    let install = seqs_of(path(&doc, "clax-install-6a1f0c3e"));
    assert!(install.contains(&7) && install.contains(&62));
    assert!(!install.contains(&8) && !install.contains(&61) && !install.contains(&36));
    // A harness session ID names the same session.
    assert_eq!(parse(&by(S1_HARNESS).unwrap())["paths"], doc["paths"]);
    // History the backfill recorded for a session counts as its own.
    let old = parse(&by("0aa1b2c3-0000-4000-8000-000000000001").unwrap());
    assert_eq!(
        seqs_of(path(&old, "clax-artifact-b4ckf1llart0"))[..2],
        [2, 3]
    );
    assert_eq!(
        by("01JB9SNOSUCHSESSION0000000").unwrap_err(),
        "unknown_session"
    );
}

#[test]
fn move_appears_in_both_artifacts_with_same_change() {
    let h = history();
    let (_dir, st) = history_store(&h);
    let doc = parse(&store_export(&st, &h, &project::Export::default()).unwrap());
    let (from, to) = ("clax-artifact-p7v2n4c8d1ef", "clax-artifact-q2w4e6r8t0yu");
    let a = step(path(&doc, from), 43);
    let b = step(path(&doc, to), 43);
    assert_eq!(a["step"]["id"], b["step"]["id"]);
    assert_eq!(a["change"], b["change"]);
    let same = |s: &Value| -> Vec<String> {
        refs(s)
            .into_iter()
            .filter(|(rel, _)| rel == "same-change")
            .map(|(_, href)| href)
            .collect()
    };
    assert_eq!(same(a), [format!("toolpath:{to}/e000000000043")]);
    assert_eq!(same(b), [format!("toolpath:{from}/e000000000043")]);
    // Refs to another path of the graph gain their toolpath: form.
    assert!(refs(a).contains(&("moved-to".into(), format!("toolpath:{to}"))));
    assert!(refs(b).contains(&("moved-from".into(), format!("toolpath:{from}"))));
    assert!(!refs(b).contains(&("moved-to".into(), format!("toolpath:{to}"))));
    // A tool call's produced steps are named in the graph.
    let call = step(path(&doc, "clax-artifact-k3m9q2w8x1ab"), 29);
    let produced: Vec<String> = refs(call)
        .into_iter()
        .filter(|(rel, _)| rel == "produced")
        .map(|(_, h)| h)
        .collect();
    assert!(!produced.is_empty());
    assert!(
        produced
            .iter()
            .all(|h| h.starts_with("toolpath:clax-artifact-k3m9q2w8x1ab/e"))
    );
    // With only one side selected, the step stays, with no same-change and
    // its refs in their clax:// form.
    let one = parse(
        &store_export(
            &st,
            &h,
            &select(project::Selection {
                artifacts: vec!["p7v2n4c8d1ef".into()],
                ..Default::default()
            }),
        )
        .unwrap(),
    );
    assert_eq!(path_ids(&one), [from]);
    let alone = step(path(&one, from), 43);
    assert!(same(alone).is_empty());
    assert!(refs(alone).iter().all(|(_, h)| !h.starts_with("toolpath:")));
}

#[test]
fn selectors_union_within_and_intersect_across() {
    let h = history();
    let (_dir, st) = history_store(&h);
    let run = |sel: project::Selection| parse(&store_export(&st, &h, &select(sel)).unwrap());
    // Artifacts union; an artifact selector leaves out the install path.
    let two = run(project::Selection {
        artifacts: vec!["p7v2n4c8d1ef".into(), "b4ckf1llart0".into()],
        ..Default::default()
    });
    assert_eq!(
        path_ids(&two),
        ["clax-artifact-b4ckf1llart0", "clax-artifact-p7v2n4c8d1ef"]
    );
    // An artifact and a live page are one kind of selector: they union.
    let mixed = run(project::Selection {
        artifacts: vec!["b4ckf1llart0".into()],
        live_pages: vec!["http://localhost:5173/settings".into()],
        ..Default::default()
    });
    assert_eq!(path_ids(&mixed), path_ids(&two));
    assert_eq!(
        mixed["meta"]["clax"]["selection"]["live"],
        json!(["http://localhost:5173/settings"])
    );
    // Time intersects with artifacts: since inclusive, until exclusive.
    let timed = run(project::Selection {
        artifacts: vec!["k3m9q2w8x1ab".into()],
        since: Some("2026-10-06T14:02:06.018Z".into()),
        until: Some("2026-10-06T14:02:41.023Z".into()),
        ..Default::default()
    });
    assert_eq!(
        seqs_of(path(&timed, "clax-artifact-k3m9q2w8x1ab")),
        [18, 19, 20, 21, 22]
    );
    // Sessions intersect with artifacts and time.
    let s3 = run(project::Selection {
        artifacts: vec!["k3m9q2w8x1ab".into(), "p7v2n4c8d1ef".into()],
        by_sessions: vec![S3.into()],
        until: Some("2026-10-06T14:04:06Z".into()),
        ..Default::default()
    });
    assert_eq!(path_ids(&s3), ["clax-artifact-k3m9q2w8x1ab"]);
    let k3 = seqs_of(path(&s3, "clax-artifact-k3m9q2w8x1ab"));
    assert!(k3.contains(&35) && k3.contains(&14) && !k3.contains(&12) && !k3.contains(&37));
    // A bare date is 00:00 UTC; an empty range selects nothing.
    let none = run(project::Selection {
        since: Some("2026-10-07".into()),
        ..Default::default()
    });
    assert_eq!(path_ids(&none), Vec::<String>::new());
    assert_eq!(none["meta"]["clax"]["first_seq"], Value::Null);
    let all = run(project::Selection {
        since: Some("2026-10-06".into()),
        until: Some("2026-10-07".into()),
        ..Default::default()
    });
    assert_eq!(all["meta"]["clax"]["last_seq"], 62);
    // The graph ID follows the selection.
    assert_ne!(two["graph"]["id"], all["graph"]["id"]);
    assert_eq!(
        two["graph"]["id"],
        run(project::Selection {
            artifacts: vec![
                "b4ckf1llart0".into(),
                "p7v2n4c8d1ef".into(),
                "b4ckf1llart0".into()
            ],
            ..Default::default()
        })["graph"]["id"]
    );
    // Refusals name what is wrong, before any byte.
    for (sel, code) in [
        (
            project::Selection {
                artifacts: vec!["zzzzzzzzzzzz".into()],
                ..Default::default()
            },
            "unknown_artifact",
        ),
        (
            project::Selection {
                artifacts: vec!["not an id".into()],
                ..Default::default()
            },
            "invalid_id",
        ),
        (
            project::Selection {
                since: Some("last week".into()),
                ..Default::default()
            },
            "invalid_time",
        ),
        (
            project::Selection {
                since: Some("2026-10-07".into()),
                until: Some("2026-10-06".into()),
                ..Default::default()
            },
            "invalid_range",
        ),
    ] {
        assert_eq!(store_export(&st, &h, &select(sel)).unwrap_err(), code);
    }
}

#[test]
fn jsonl_requires_single_path() {
    let h = history();
    let (_dir, st) = history_store(&h);
    let jsonl = |sel: project::Selection, shape: project::Shape| {
        store_export(
            &st,
            &h,
            &project::Export {
                selection: sel,
                shape,
                format: project::Format::Jsonl,
                ..Default::default()
            },
        )
    };
    use project::Shape::{Artifacts, Journal};
    assert_eq!(
        jsonl(Default::default(), Artifacts).unwrap_err(),
        "jsonl_needs_one_path"
    );
    let two = project::Selection {
        artifacts: vec!["p7v2n4c8d1ef".into(), "b4ckf1llart0".into()],
        ..Default::default()
    };
    assert_eq!(
        jsonl(two.clone(), Artifacts).unwrap_err(),
        "jsonl_needs_one_path"
    );
    // One artifact, or the journal shape, is one path, which seals into a
    // valid graph.
    let one = project::Selection {
        artifacts: vec!["k3m9q2w8x1ab".into()],
        ..Default::default()
    };
    for (sel, shape, id) in [
        (one, Artifacts, "clax-artifact-k3m9q2w8x1ab"),
        (two, Journal, ""),
        (Default::default(), Journal, ""),
    ] {
        let text = jsonl(sel, shape).unwrap();
        let sealed = seal(&text).unwrap();
        assert!(sealed.warnings.is_empty(), "{:?}", sealed.warnings);
        assert_valid(&sealed.graph, "a sealed JSONL export");
        let p = &sealed.graph["paths"][0];
        if !id.is_empty() {
            assert_eq!(p["path"]["id"], id);
        }
        // Each actor is defined before its first step.
        let mut defined = BTreeSet::new();
        for line in text.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            if let Some(d) = v.get("ActorDef") {
                defined.insert(d["actor"].as_str().unwrap().to_string());
            }
            if let Some(s) = v.get("Step") {
                assert!(defined.contains(s["step"]["actor"].as_str().unwrap()));
            }
        }
        assert!(text.ends_with("{\"PathClose\":{}}\n"));
    }
    let journal = seal(&jsonl(Default::default(), Journal).unwrap()).unwrap();
    let p = &journal.graph["paths"][0];
    assert!(
        p["path"]["id"]
            .as_str()
            .unwrap()
            .starts_with("clax-export-")
    );
    assert_eq!(seqs_of(p), (1..=62).collect::<Vec<_>>());
    assert_eq!(p["meta"]["clax"]["redaction"], json!([]));
    // A JSONL path needs a step; indenting applies to JSON only.
    let later = project::Selection {
        since: Some("2027-01-01".into()),
        ..Default::default()
    };
    assert_eq!(jsonl(later, Journal).unwrap_err(), "empty_selection");
    let pretty = project::Export {
        shape: Journal,
        format: project::Format::Jsonl,
        pretty: true,
        ..Default::default()
    };
    assert_eq!(
        store_export(&st, &h, &pretty).unwrap_err(),
        "invalid_option"
    );
}

#[test]
fn live_selector_matches_origin_and_path() {
    let h = history();
    let (_dir, st) = history_store(&h);
    let live = |url: &str| {
        store_export(
            &st,
            &h,
            &select(project::Selection {
                live_pages: vec![url.into()],
                ..Default::default()
            }),
        )
        .map(|t| path_ids(&parse(&t)))
    };
    // From the history's live.page event; a route or query is not the page.
    for url in [
        "http://localhost:5173/settings",
        "http://localhost:5173/settings?tab=2#top",
    ] {
        assert_eq!(live(url).unwrap(), ["clax-artifact-p7v2n4c8d1ef"], "{url}");
    }
    // The same path on another origin, or another path, is another page.
    for url in [
        "http://localhost:5174/settings",
        "https://localhost:5173/settings",
        "http://localhost:5173/settings/a",
    ] {
        assert_eq!(live(url).unwrap_err(), "unknown_live_page", "{url}");
    }
    // A page there now is found by its table row too, under its joined
    // site's key origin.
    st.with_write(|c| {
        c.execute_batch(
            "INSERT INTO artifacts (id, title, created_at, updated_at, contract_version, kind)
                 VALUES ('q2w4e6r8t0yu', 'Settings A', '2026-10-06T14:04:47Z', '2026-10-06T14:04:47Z', '1', 'live');
             INSERT INTO live_pages (artifact_id, origin, path, created_at)
                 VALUES ('q2w4e6r8t0yu', 'http://localhost:5173', '/settings/a', '2026-10-06T14:04:47Z');
             INSERT INTO live_sites (origin, site, joined_at, last_used_at)
                 VALUES ('http://127.0.0.1:5173', 'http://localhost:5173', '2026-10-06T14:05:43Z', '2026-10-06T14:05:43Z');",
        )?;
        Ok(())
    })
    .unwrap();
    for url in [
        "http://localhost:5173/settings/a",
        "http://127.0.0.1:5173/settings/a",
    ] {
        assert_eq!(live(url).unwrap(), ["clax-artifact-q2w4e6r8t0yu"], "{url}");
    }
    let doc = parse(
        &store_export(
            &st,
            &h,
            &select(project::Selection {
                live_pages: vec!["http://localhost:5173/settings/a".into()],
                ..Default::default()
            }),
        )
        .unwrap(),
    );
    let meta = &doc["paths"][0]["meta"];
    assert_eq!(meta["title"], "Settings A");
    assert_eq!(
        meta["clax"],
        json!({"projection": "artifact", "artifact_id": "q2w4e6r8t0yu", "artifact_kind": "live",
               "origin": "http://localhost:5173", "path": "/settings/a"})
    );
    assert_eq!(
        meta["refs"],
        json!([{"rel": "view", "href": "http://localhost:7480/a/q2w4e6r8t0yu"}])
    );
    // A page merged away keeps its origin and path, and a page a live.page
    // event records under a joined origin is found under the site's key.
    st.with_write(|c| {
        c.execute_batch(
            "INSERT INTO artifacts (id, title, created_at, updated_at, contract_version, kind)
                 VALUES ('z9x8c7v6b5nm', 'Old settings', '2026-10-06T14:05:57Z', '2026-10-06T14:05:57Z', '1', 'live');
             INSERT INTO live_merged_pages (artifact_id, origin, path, merged_into, merged_at)
                 VALUES ('z9x8c7v6b5nm', 'http://127.0.0.1:5173', '/old', 'p7v2n4c8d1ef', '2026-10-06T14:05:57Z');",
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        live("http://127.0.0.1:5173/old").unwrap(),
        ["clax-artifact-z9x8c7v6b5nm"]
    );
    let merged = parse(
        &store_export(
            &st,
            &h,
            &select(project::Selection {
                artifacts: vec!["z9x8c7v6b5nm".into()],
                ..Default::default()
            }),
        )
        .unwrap(),
    );
    assert_eq!(
        merged["paths"][0]["meta"]["clax"]["origin"],
        "http://127.0.0.1:5173"
    );
    assert_eq!(merged["paths"][0]["meta"]["clax"]["path"], "/old");
    st.with_write(|c| {
        c.execute(
            "INSERT INTO live_sites (origin, site, joined_at, last_used_at)
                 VALUES ('http://localhost:5199', 'http://localhost:5173', '2026-10-06T14:05:43Z', '2026-10-06T14:05:43Z')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        live("http://localhost:5199/settings").unwrap(),
        ["clax-artifact-p7v2n4c8d1ef"]
    );
    // A title is text: --no-text leaves it out.
    let redacted = parse(
        &store_export(
            &st,
            &h,
            &project::Export {
                redaction: Redaction {
                    no_text: true,
                    ..Redaction::NONE
                },
                ..select(project::Selection {
                    artifacts: vec!["q2w4e6r8t0yu".into()],
                    ..Default::default()
                })
            },
        )
        .unwrap(),
    );
    assert_eq!(
        redacted["paths"][0]["meta"]["title"],
        "Artifact q2w4e6r8t0yu"
    );
}

/// The journal shape of the whole history, as a golden the web unit gate
/// validates.
#[test]
fn golden_journal_export_matches_and_conforms() {
    let h = history();
    let req = project::Export {
        shape: project::Shape::Journal,
        pretty: true,
        ..Default::default()
    };
    let mut out = Vec::new();
    project::export(&history_source(&h), &req, &export_env(&h), &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let doc = parse(&text);
    let p = &doc["paths"][0];
    assert_eq!(path_ids(&doc).len(), 1);
    assert_eq!(seqs_of(p), (1..=62).collect::<Vec<_>>());
    assert_eq!(p["meta"]["clax"]["projection"], "journal");
    golden("export.journal.path.json", &text);
}

/// The golden history's `tool.call` bodies are what the live builder makes
/// from the same report, so the goldens follow the real recorder.
#[test]
fn golden_tool_calls_match_the_live_builder() {
    let h = read_json("history.json");
    let mut seen = 0;
    for row in h["rows"].as_array().unwrap() {
        if row["kind"] != "tool.call" {
            continue;
        }
        seen += 1;
        let body = row["body"].as_object().unwrap();
        let report: crate::audit::ToolCallReport =
            serde_json::from_value(Value::Object(body.clone())).unwrap();
        let produced: Vec<i64> = serde_json::from_value(body["produced"].clone()).unwrap();
        let artifact = row["ids"]["artifact"].as_str().map(str::to_string);
        let rec = crate::store::audit::tool_call_record(
            row["at"].as_str().unwrap(),
            &report,
            artifact.clone(),
            &produced,
        );
        let mut want: Map<String, Value> = body.clone();
        for k in redact::ENVELOPE_FIELDS {
            want.remove(k);
        }
        let got: Map<String, Value> = rec.body.into_iter().collect();
        assert_eq!(got, want, "seq {}", row["seq"]);
        assert_eq!(rec.ids.call.as_deref(), body["call_id"].as_str());
        assert_eq!(rec.ids.artifact, artifact);
    }
    assert!(seen >= 2);
}

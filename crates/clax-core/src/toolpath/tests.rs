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
fn assert_valid(doc: &Value, what: &str) {
    let errors = conformance_errors(doc);
    assert!(
        errors.is_empty(),
        "{what} does not conform:\n{}",
        errors.join("\n")
    );
}

/// Compares `actual` with the expected file `name`, or rewrites it under
/// `CLAX_UPDATE_GOLDEN=1`.
fn golden(name: &str, actual: &str) {
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
struct History {
    env: RenderEnv,
    rows: Vec<AuditRow>,
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

fn history() -> History {
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
fn render_chain(rows: &[AuditRow], env: &RenderEnv, opts: &Redaction) -> Vec<Value> {
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

/// A Graph of one path per artifact (by `artifact_id` or `artifact2_id`,
/// in artifact ID order) plus the install path, shaped as spec §8.2 says,
/// so the renderer's steps can be checked in the document they go into.
fn export_graph(h: &History, opts: &Redaction) -> Value {
    let env = &h.env;
    let mut by_artifact: BTreeMap<&str, Vec<&AuditRow>> = BTreeMap::new();
    let mut install: Vec<&AuditRow> = Vec::new();
    for r in &h.rows {
        let arts: BTreeSet<&str> = [&r.ids.artifact, &r.ids.artifact2]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if arts.is_empty() {
            install.push(r);
        }
        for a in arts {
            by_artifact.entry(a).or_default().push(r);
        }
    }
    let path = |id: String, base: String, rows: &[&AuditRow], meta: Value| {
        let mut actors: BTreeMap<String, Value> = BTreeMap::new();
        let mut steps = Vec::new();
        let mut prev = None;
        for r in rows {
            let rendered = render(r, prev, env, opts).unwrap();
            steps.push(rendered.step);
            for (k, d) in rendered.actors {
                let merged = match actors.get(&k) {
                    Some(old) => merge_actor_def(old, &d),
                    None => d,
                };
                actors.insert(k, merged);
            }
            prev = Some(r.seq);
        }
        let mut meta = meta;
        meta["kind"] = KIND_URI.into();
        meta["source"] = base.clone().into();
        meta["actors"] = json!(actors);
        json!({
            "path": {"id": id, "base": {"uri": base}, "head": step_id(prev.unwrap())},
            "steps": steps,
            "meta": meta,
        })
    };
    let mut paths: Vec<Value> = by_artifact
        .iter()
        .map(|(a, rows)| {
            path(
                format!("clax-artifact-{a}"),
                clax_uri(&env.install, Obj::Artifact(a)),
                rows,
                json!({"title": format!("Artifact {a}"),
                       "clax": {"projection": "artifact", "artifact_id": a}}),
            )
        })
        .collect();
    // A step on two artifacts appears in both paths, joined by same-change.
    let ids: Vec<String> = paths
        .iter()
        .map(|p| p["path"]["id"].as_str().unwrap().to_string())
        .collect();
    let mut seen: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, p) in paths.iter().enumerate() {
        for s in p["steps"].as_array().unwrap() {
            seen.entry(s["step"]["id"].as_str().unwrap().into())
                .or_default()
                .push(i);
        }
    }
    for (step, at) in seen.iter().filter(|(_, at)| at.len() > 1) {
        for &i in at {
            for &j in at.iter().filter(|&&j| j != i) {
                let s = paths[i]["steps"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|s| s["step"]["id"] == step.as_str())
                    .unwrap();
                s["meta"]["refs"].as_array_mut().unwrap().push(json!({
                    "rel": "same-change",
                    "href": format!("toolpath:{}/{step}", ids[j]),
                }));
            }
        }
    }
    paths.push(path(
        format!("clax-install-{}", &env.install[..8]),
        clax_uri(&env.install, Obj::Install),
        &install,
        json!({"title": "Clax install audit trail", "clax": {"projection": "install"}}),
    ));
    json!({
        "graph": {"id": format!("clax-{}-golden", &env.install[..8])},
        "paths": paths,
        "meta": {
            "title": "Clax export",
            "refs": [{"rel": "source", "href": clax_uri(&env.install, Obj::Install)}],
            "clax": {"install": env.install, "clax_version": "0.3.1",
                     "clax_commit": "abc1234def5678abc1234def5678abc1234def56", "redaction": opts.names(),
                     "first_seq": h.rows.first().unwrap().seq, "last_seq": h.rows.last().unwrap().seq},
        },
    })
}

/// The history as one journal segment (spec §7.2), rendered as the journal
/// renders (no browser URLs): `PathOpen`, each actor's `ActorDef` before
/// its first step and again, merged, whenever its definition grows, the
/// steps, and, when `closed`, `Head` and `PathClose`.
fn segment(h: &History, closed: bool) -> String {
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
            "refs": [{"rel": "continues", "href": format!("clax-{}-20261005-001.path.jsonl", &env.install[..8])}],
            "clax": {"projection": "journal", "install": env.install, "segment": "20261006-001",
                     "first_seq": h.rows[0].seq, "clax_version": "0.3.1",
                     "clax_commit": "abc1234def5678abc1234def5678abc1234def56"},
        },
    }}));
    let mut defined: BTreeMap<String, Value> = BTreeMap::new();
    let mut prev = None;
    for r in &h.rows {
        let rendered = render(r, prev, env, &opts).unwrap();
        for (actor, definition) in rendered.actors {
            let merged = match defined.get(&actor) {
                Some(old) => merge_actor_def(old, &definition),
                None => definition,
            };
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

fn pretty(v: &Value) -> String {
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
        let doc = export_graph(&h, &opts);
        assert_valid(&doc, &format!("the export under {opts:?}"));
        golden(&export_golden_name(&opts), &pretty(&doc));
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

#[test]
fn golden_segments_seal_and_validate() {
    let h = history();
    let closed = segment(&h, true);
    golden("segment.path.jsonl", &closed);
    let open = segment(&h, false);
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

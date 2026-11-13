//! What a run of a kind is given: its operations, beside an operator's
//! and a starter's patterns and with content withheld, and the places
//! in its request of the person's words and of content. Tables over the
//! design's two files of a host's and the crate's own fixture named
//! `clax`; ClaxBot's own file and its fake's are checked where they
//! live (crates/clax-cli/tests/bot_surface.rs).

use super::{host, own, review, shell};
use crate::{ContentUse, Definition, Pattern, RunOperations, RunScope, ValueErrorKind, Withheld};
use serde_json::{Value, json};

fn patterns(list: &[&str]) -> Vec<Pattern> {
    list.iter().map(|p| Pattern::parse(p).unwrap()).collect()
}

/// The run's operations, what is withheld (by name and reason), what
/// is wanted elsewhere, and where its request holds content.
type Expected<'a> = (
    &'a [&'a str],
    &'a [(&'a str, Withheld)],
    &'a [&'a str],
    ContentUse,
);

#[track_caller]
fn given(def: &Definition, kind: &str, scope: &RunScope<'_>, expected: Expected<'_>) {
    let RunOperations {
        granted,
        withheld,
        elsewhere,
        request_content,
    } = def.run_operations(kind, scope).unwrap();
    let elsewhere: Vec<String> = elsewhere.iter().map(ToString::to_string).collect();
    assert_eq!(
        (
            granted.as_slice(),
            withheld.as_slice(),
            elsewhere,
            request_content
        ),
        (
            expected.0,
            expected.1,
            expected
                .2
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            expected.3
        ),
        "{kind}"
    );
}

use Withheld::{Content, Kind, Operator, Starter};

/// A run the shell joins is given every operation the operator's and
/// the join's patterns name, in declaration order, with no kind's
/// `@allow` to narrow them, and the screenshot withheld for content.
#[test]
fn a_run_the_shell_joins_is_given_what_the_operator_and_the_join_name() {
    let shell = shell();
    let all = shell.joined_operations(&RunScope::default());
    assert_eq!(
        all.granted,
        [
            "shell.parts",
            "shell.windows",
            "shell.workspaces",
            "shell.bridge",
            "shell.settings.get",
            "shell.settings.set",
            "shell.window.open",
            "shell.notify",
            "shell.ask",
        ]
    );
    assert_eq!(all.withheld, [("shell.screenshot", Content)]);
    assert!(all.elsewhere.is_empty());
    assert_eq!(all.request_content, ContentUse::None);
    let operator = patterns(&["shell.settings.*", "shell.windows", "shell.screenshot"]);
    let join = patterns(&["shell.settings.get", "shell.windows", "shell.screenshot"]);
    let narrowed = shell.joined_operations(&RunScope {
        operator: Some(&operator),
        starter: Some(&join),
        content: false,
    });
    assert_eq!(narrowed.granted, ["shell.windows", "shell.settings.get"]);
    assert_eq!(
        narrowed.withheld,
        [
            ("shell.parts", Operator),
            ("shell.workspaces", Operator),
            ("shell.bridge", Operator),
            ("shell.screenshot", Content),
            ("shell.settings.set", Starter),
            ("shell.window.open", Operator),
            ("shell.notify", Operator),
            ("shell.ask", Operator),
        ]
    );
    // A join that names nothing the operator allows binds nothing.
    let none = patterns(&["shell.notify"]);
    let empty = shell.joined_operations(&RunScope {
        operator: Some(&operator),
        starter: Some(&none),
        content: false,
    });
    assert!(empty.granted.is_empty());
}

#[test]
fn a_run_of_shells_triage_is_given_what_every_list_names() {
    let shell = shell();
    let defaults = RunScope::default();
    // Its `@allow`, and of those the screenshot withheld: it is an
    // `Image`. `threads.open` and `runs.wait` are Clax's.
    given(
        &shell,
        "triage",
        &defaults,
        (
            &[
                "shell.parts",
                "shell.windows",
                "shell.settings.get",
                "shell.ask",
            ],
            &[
                ("shell.workspaces", Kind),
                ("shell.bridge", Kind),
                ("shell.screenshot", Content),
                ("shell.settings.set", Kind),
                ("shell.window.open", Kind),
                ("shell.notify", Kind),
            ],
            &["threads.open", "runs.wait"],
            ContentUse::Optional,
        ),
    );
    // Once Clax stores content, the screenshot is given.
    given(
        &shell,
        "triage",
        &RunScope {
            content: true,
            ..defaults
        },
        (
            &[
                "shell.parts",
                "shell.windows",
                "shell.screenshot",
                "shell.settings.get",
                "shell.ask",
            ],
            &[
                ("shell.workspaces", Kind),
                ("shell.bridge", Kind),
                ("shell.settings.set", Kind),
                ("shell.window.open", Kind),
                ("shell.notify", Kind),
            ],
            &["threads.open", "runs.wait"],
            ContentUse::Optional,
        ),
    );
    // The operator's list, then the starter's, narrow it further; the
    // first reason that holds is the one given.
    let operator = patterns(&["shell.settings.*", "shell.ask", "shell.screenshot"]);
    let starter = patterns(&["shell.*"]);
    given(
        &shell,
        "triage",
        &RunScope {
            operator: Some(&operator),
            starter: Some(&starter),
            content: false,
        },
        (
            &["shell.settings.get", "shell.ask"],
            &[
                ("shell.parts", Operator),
                ("shell.windows", Operator),
                ("shell.workspaces", Kind),
                ("shell.bridge", Kind),
                ("shell.screenshot", Content),
                ("shell.settings.set", Kind),
                ("shell.window.open", Kind),
                ("shell.notify", Kind),
            ],
            &["threads.open", "runs.wait"],
            ContentUse::Optional,
        ),
    );
    let none: Vec<Pattern> = Vec::new();
    let only_ask = patterns(&["shell.ask"]);
    for (starter, granted) in [(&none, &[][..]), (&only_ask, &["shell.ask"][..])] {
        let ops = shell
            .run_operations(
                "triage",
                &RunScope {
                    starter: Some(starter),
                    ..defaults
                },
            )
            .unwrap();
        assert_eq!(ops.granted, granted);
        let by_starter: Vec<&str> = ops
            .withheld
            .iter()
            .filter(|(_, why)| *why == Starter)
            .map(|(op, _)| *op)
            .collect();
        // The starter's list is asked before content is.
        let expected: Vec<&str> = [
            "shell.parts",
            "shell.windows",
            "shell.screenshot",
            "shell.settings.get",
            "shell.ask",
        ]
        .into_iter()
        .filter(|op| !granted.contains(op))
        .collect();
        assert_eq!(by_starter, expected);
    }
    // An operator who allows nothing gives nothing.
    let ops = shell
        .run_operations(
            "triage",
            &RunScope {
                operator: Some(&none),
                ..defaults
            },
        )
        .unwrap();
    assert!(ops.granted.is_empty());
    assert_eq!(ops.withheld.len(), shell.operations().len());
    // No such kind.
    assert!(shell.run_operations("nope", &defaults).is_none());
    // The reasons, as said.
    assert_eq!(Content.to_string(), "content is not available yet");
    assert_eq!(Kind.to_string(), "the kind's allow list does not name it");
}

#[test]
fn a_run_of_each_of_reviews_kinds_is_given_its_allow_less_the_diff() {
    let review = review();
    let defaults = RunScope::default();
    // `review.diff` answers with a `Diff`, whose `diff` is a `Content`.
    for (kind, granted, withheld) in [
        (
            "range",
            &[
                "review.view",
                "review.comments",
                "review.files",
                "review.commits",
                "review.checks",
                "review.progress",
            ][..],
            &[
                ("review.diff", Content),
                ("review.quest.get", Kind),
                ("review.quest.note", Kind),
            ][..],
        ),
        (
            "about",
            &[
                "review.view",
                "review.comments",
                "review.files",
                "review.commits",
                "review.checks",
                "review.progress",
            ],
            &[
                ("review.diff", Content),
                ("review.quest.get", Kind),
                ("review.quest.note", Kind),
            ],
        ),
        (
            "annotate",
            &[
                "review.view",
                "review.comments",
                "review.files",
                "review.commits",
                "review.checks",
                "review.progress",
                "review.quest.get",
                "review.quest.note",
            ],
            &[("review.diff", Content)],
        ),
        (
            "plan",
            &[
                "review.view",
                "review.files",
                "review.commits",
                "review.progress",
            ],
            &[
                ("review.comments", Kind),
                ("review.diff", Content),
                ("review.checks", Kind),
                ("review.quest.get", Kind),
                ("review.quest.note", Kind),
            ],
        ),
    ] {
        given(
            &review,
            kind,
            &defaults,
            (granted, withheld, &[], ContentUse::None),
        );
    }
}

#[test]
fn a_kind_of_clax_s_own_that_says_nothing_narrows_nothing() {
    let own = own();
    let defaults = RunScope::default();
    for kind in ["thread", "brief"] {
        let ops = own.run_operations(kind, &defaults).unwrap();
        // Every operation but those that hold content.
        let content: Vec<&str> = ops
            .withheld
            .iter()
            .map(|(op, why)| {
                assert_eq!(*why, Content, "{op}");
                *op
            })
            .collect();
        assert_eq!(ops.granted.len() + content.len(), own.operations().len());
        assert!(ops.elsewhere.is_empty());
        assert_eq!(ops.request_content, ContentUse::None);
        let all = own
            .run_operations(
                kind,
                &RunScope {
                    content: true,
                    ..defaults
                },
            )
            .unwrap();
        assert_eq!(all.granted.len(), own.operations().len());
    }
}

/// A host's file whose one kind, `go`, takes this request.
fn requesting(request: &str, rest: &str) -> Definition {
    host(&format!(
        "declare const demo: {{}};\n{rest}interface Kinds {{\n/** K. @allow */\ngo(request: {request}): {{}};\n}}"
    ))
    .unwrap_or_else(|e| panic!("{request}: {e}"))
}

#[test]
fn where_a_request_holds_content_is_said_of_its_kind() {
    let defaults = RunScope::default();
    for (request, rest, content) in [
        ("{ ask: Instruction }", "", ContentUse::None),
        ("{ shot: Image }", "", ContentUse::Required),
        ("{ shot?: Image }", "", ContentUse::Optional),
        ("{ shots: Image[] }", "", ContentUse::Optional),
        ("Image | null", "", ContentUse::Optional),
        ("Record<string, Content>", "", ContentUse::Optional),
        (
            "{ a: Image } | { b: { c: Content } }",
            "",
            ContentUse::Required,
        ),
        ("{ a: Image } | { b: string }", "", ContentUse::Optional),
        ("P", "interface P { pic: Image }\n", ContentUse::Required),
        (
            "Q",
            "interface Q extends P {}\ninterface P { pic: Content }\n",
            ContentUse::Required,
        ),
        // An interface that can only hold itself has no value, and
        // requires nothing.
        (
            "L",
            "interface L { next: L; pic?: Image }\n",
            ContentUse::Optional,
        ),
        ("Content", "", ContentUse::Required),
        ("unknown", "", ContentUse::None),
    ] {
        let def = requesting(request, rest);
        let ops = def.run_operations("go", &defaults).unwrap();
        assert_eq!(ops.request_content, content, "{request}");
    }
}

/// A request with the person's words in every place a type can hold
/// them, beside strings of the same text that are data.
const ASKS: &str = "{
  ask: Instruction;
  note: string;
  turns?: { said: Instruction; by: string }[];
  deep?: { inner: { words: Instruction } };
  by_key?: Record<string, Instruction>;
  either?: { kind: \"said\"; words: Instruction } | { kind: \"data\"; words: string };
  first?: { a: Instruction } | { a: string };
  second?: { a: string } | { a: Instruction };
  text?: Instruction | string;
  maybe?: Instruction | null;
  turn?: Turn;
  any?: unknown;
  clip?: Image;
  clips?: Content[];
}";

const TURN: &str = "interface Turn { words: Instruction; at: string }\n";

#[test]
fn the_persons_words_in_a_request_are_named_by_pointer() {
    let def = requesting(ASKS, TURN);
    let go = def.kind("go").unwrap();
    let places = |v: Value| go.instruction_places(&def, &v).unwrap();
    let content = |v: Value| go.content_places(&def, &v).unwrap();
    // The same text, as the person's words and as data: only the first.
    assert_eq!(places(json!({"ask": "same", "note": "same"})), ["/ask"]);
    // Nested, in arrays, under keys (escaped), in an interface.
    assert_eq!(
        places(json!({
            "ask": "a",
            "note": "n",
            "turns": [{"said": "t0", "by": "x"}, {"said": "t1", "by": "y"}],
            "deep": {"inner": {"words": "w"}},
            "by_key": {"a/b~c": "k", "plain": "p"},
            "turn": {"words": "w", "at": "now"},
            "any": {"ask": "not a place"},
        })),
        [
            "/ask",
            "/by_key/a~1b~0c",
            "/by_key/plain",
            "/deep/inner/words",
            "/turn/words",
            "/turns/0/said",
            "/turns/1/said",
        ]
    );
    // An optional field left out, or `null`, holds nothing.
    assert_eq!(
        places(json!({"ask": "a", "note": "n", "turns": null, "deep": null})),
        ["/ask"]
    );
    assert_eq!(
        places(json!({"ask": "a", "note": "n", "turns": []})),
        ["/ask"]
    );
    // A union: only the alternative the value is read as.
    assert_eq!(
        places(json!({"ask": "a", "note": "n", "either": {"kind": "said", "words": "x"}})),
        ["/ask", "/either/words"]
    );
    assert_eq!(
        places(json!({"ask": "a", "note": "n", "either": {"kind": "data", "words": "x"}})),
        ["/ask"]
    );
    // Untold apart, the first alternative the value fits, as the check
    // reads it.
    assert_eq!(
        places(json!({"ask": "a", "note": "n", "first": {"a": "x"}, "second": {"a": "y"}})),
        ["/ask", "/first/a"]
    );
    // A string that a union reads as `string` too is data; one only
    // `Instruction` takes is the person's.
    assert_eq!(
        places(json!({"ask": "a", "note": "n", "text": "t", "maybe": "m"})),
        ["/ask", "/maybe"]
    );
    // Content, by the same reading.
    assert_eq!(
        content(json!({
            "ask": "a",
            "note": "n",
            "clip": {"id": "u1"},
            "clips": [{"id": "u2"}, {"id": "u3", "meta": {}}],
        })),
        ["/clip", "/clips/0", "/clips/1"]
    );
    assert!(content(json!({"ask": "a", "note": "n"})).is_empty());
    // The request itself, when it is the person's words.
    let bare = requesting("Instruction", "");
    let go = bare.kind("go").unwrap();
    assert_eq!(go.instruction_places(&bare, &json!("hi")).unwrap(), [""]);
    // A request that the kind refuses is refused, and so is a kind of
    // another definition.
    let e = def
        .kind("go")
        .unwrap()
        .instruction_places(&def, &json!({"ask": 1, "note": "n"}))
        .unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (
            ValueErrorKind::Wrong,
            "kind go: the request.ask must be a string"
        )
    );
    let e = go.instruction_places(&def, &json!("hi")).unwrap_err();
    assert_eq!(e.kind, ValueErrorKind::ForeignType);
}

#[test]
fn the_persons_words_in_a_message_are_named_by_the_kinds_message_type() {
    let def = host(&format!(
        "declare const demo: {{}};\n{TURN}interface Kinds {{\n/** K. @allow */\n\
         go(request: {{ ask: Instruction }}, message?: {{ say: Instruction; note: string; turn?: Turn }}): {{}};\n\
         /** One. @allow */\nonce(request: {{ ask: Instruction }}): {{}};\n}}"
    ))
    .unwrap();
    let go = def.kind("go").unwrap();
    assert_eq!(
        go.message_instruction_places(
            &def,
            &json!({"say": "s", "note": "s", "turn": {"words": "w", "at": "t"}})
        )
        .unwrap(),
        ["/say", "/turn/words"]
    );
    // Read by the message's type, not the request's.
    let e = go
        .message_instruction_places(&def, &json!({"ask": "a"}))
        .unwrap_err();
    assert_eq!(e.kind, ValueErrorKind::Wrong);
    assert!(
        e.message.starts_with("kind go: the message"),
        "{}",
        e.message
    );
    // A kind with one parameter takes none.
    let e = def
        .kind("once")
        .unwrap()
        .message_instruction_places(&def, &json!({"say": "s"}))
        .unwrap_err();
    assert_eq!(e.message, "kind once takes no messages after it starts");
}

#[test]
fn the_example_files_requests_name_the_persons_words() {
    let shell = shell();
    let triage = shell.kind("triage").unwrap();
    let request = json!({
        "ask": "Tidy these",
        "touches": [{"artifact": "a1", "title": "Tidy these"}],
        "clip": {"id": "u1"},
    });
    assert_eq!(
        triage.instruction_places(&shell, &request).unwrap(),
        ["/ask"]
    );
    assert_eq!(triage.content_places(&shell, &request).unwrap(), ["/clip"]);
    let review = review();
    let range = review.kind("range").unwrap();
    let request = json!({
        "head": "h", "path": "p", "side": "new", "blob": "b",
        "start_line": 1, "end_line": 2, "commit": "c",
        "thread": {"id": "t", "comments": [{"author": null, "body": "why?"}]},
        "ask": "why?",
        "conversation": [{"from": "person", "text": "why?"}],
    });
    assert_eq!(
        range.instruction_places(&review, &request).unwrap(),
        ["/ask"]
    );
    assert!(range.content_places(&review, &request).unwrap().is_empty());
}

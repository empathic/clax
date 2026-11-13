//! Values checked against types, a call's arguments and answers, and
//! allow patterns.

use super::{file, held as check_held, host, own, review, sent as check_sent, shell};
use crate::{Definition, Pattern, Ty, ValueErrorKind};
use serde_json::{Value, json};

/// A definition with these interfaces, to check values against.
fn types(interfaces: &str) -> Definition {
    host(&file("", interfaces)).unwrap_or_else(|e| panic!("{e}"))
}

/// The type of field `f` of interface `T`.
fn field_ty(d: &Definition, f: &str) -> Ty {
    d.interface("T")
        .unwrap()
        .fields
        .iter()
        .find(|x| x.name == f)
        .unwrap()
        .ty
        .clone()
}

#[track_caller]
fn accepts(d: &Definition, ty: &Ty, v: Value) {
    assert_eq!(check_held(d, ty, &v), Ok(()), "{v}");
}

#[track_caller]
fn refuses(d: &Definition, ty: &Ty, v: Value, why: &str) {
    assert_eq!(check_held(d, ty, &v), Err(why.to_string()), "{v}");
}

#[test]
fn values_are_checked_against_each_form_of_type() {
    let d = types(
        "interface T {\n  s: string; n: number; i: Int; b: boolean; z: null; u: unknown;\n  lit: \"auto\" | \"raw\"; one: \"only\"; yes: true; flag: true | \"auto\"; list: Int[]; map: Record<string, Int>;\n  obj: { a: string; b?: Int }; either: string | Int[] | null; said: Instruction;\n}",
    );
    let t = |f: &str| field_ty(&d, f);

    accepts(&d, &t("s"), json!("x"));
    refuses(&d, &t("s"), json!(1), "value must be a string");
    accepts(&d, &t("said"), json!("do this"));
    refuses(&d, &t("said"), json!(null), "value must be a string");
    accepts(&d, &t("n"), json!(1.5));
    refuses(&d, &t("n"), json!("1"), "value must be a number");
    accepts(&d, &t("i"), json!(3));
    accepts(&d, &t("i"), json!(-3));
    accepts(&d, &t("i"), json!(u64::MAX));
    refuses(&d, &t("i"), json!(1.5), "value must be an integer");
    refuses(&d, &t("i"), json!(1.0), "value must be an integer");
    refuses(&d, &t("i"), json!("3"), "value must be an integer");
    accepts(&d, &t("b"), json!(false));
    refuses(&d, &t("b"), json!(0), "value must be a boolean");
    accepts(&d, &t("z"), json!(null));
    refuses(&d, &t("z"), json!(false), "value must be null");
    accepts(&d, &t("u"), json!({"any": [1, null]}));
    accepts(&d, &t("u"), json!(null));
    accepts(&d, &t("lit"), json!("raw"));
    // Today's wording for a string that is none of an enumeration's.
    refuses(
        &d,
        &t("lit"),
        json!("cooked"),
        "value must be one of auto, raw",
    );
    refuses(&d, &t("lit"), json!(1), "value must be one of auto, raw");
    refuses(&d, &t("one"), json!("other"), "value must be \"only\"");
    accepts(&d, &t("yes"), json!(true));
    refuses(&d, &t("yes"), json!(false), "value must be true");
    refuses(
        &d,
        &t("flag"),
        json!(false),
        "value must be true | \"auto\"",
    );
    accepts(&d, &t("list"), json!([]));
    accepts(&d, &t("list"), json!([1, 2]));
    refuses(
        &d,
        &t("list"),
        json!([1, "2"]),
        "value[1] must be an integer",
    );
    refuses(&d, &t("list"), json!({}), "value must be an array");
    accepts(&d, &t("map"), json!({"a": 1, "b c": 2}));
    refuses(
        &d,
        &t("map"),
        json!({"a": 1, "b": 2.5}),
        "value.b must be an integer",
    );
    refuses(
        &d,
        &t("map"),
        json!({"a": 1, "b c": 2.5}),
        "value[\"b c\"] must be an integer",
    );
    refuses(&d, &t("map"), json!([]), "value must be an object");
    accepts(&d, &t("obj"), json!({"a": "x"}));
    accepts(&d, &t("obj"), json!({"a": "x", "b": null}));
    accepts(&d, &t("obj"), json!({"a": "x", "b": 2}));
    refuses(&d, &t("obj"), json!({"b": 2}), "value.a is missing");
    refuses(
        &d,
        &t("obj"),
        json!({"a": null}),
        "value.a must be a string",
    );
    refuses(
        &d,
        &t("obj"),
        json!({"a": "x", "c": 1}),
        "value has no field \"c\" (fields: a, b)",
    );
    refuses(&d, &t("obj"), json!("x"), "value must be an object");
    accepts(&d, &t("either"), json!("x"));
    accepts(&d, &t("either"), json!([1]));
    accepts(&d, &t("either"), json!(null));
    // The one alternative an array can be says what is wrong with it.
    refuses(
        &d,
        &t("either"),
        json!([1, 1.5]),
        "value[1] must be an integer",
    );
    refuses(
        &d,
        &t("either"),
        json!(true),
        "value must be string | number[] | null",
    );

    // The whole interface, and a path through it.
    let whole = Ty::Named("T".into());
    let mut v = json!({
        "s": "x", "n": 1, "i": 1, "b": true, "z": null, "u": 0, "lit": "auto", "one": "only", "yes": true,
        "flag": "auto", "list": [1], "map": {}, "obj": {"a": "x"}, "either": null, "said": "go",
    });
    accepts(&d, &whole, v.clone());
    v["obj"]["b"] = json!("two");
    refuses(&d, &whole, v, "value.obj.b must be an integer");
}

#[test]
fn an_interface_has_the_fields_it_inherits() {
    let d = review();
    let page = Ty::Named("CommentPage".into());
    let ok = json!({"subject": "o/r#1", "head_sha": null, "next_cursor": null, "truncated": false, "items": [], "total": 3});
    accepts(&d, &page, ok.clone());
    let mut v = ok.clone();
    v.as_object_mut().unwrap().remove("subject");
    refuses(&d, &page, v, "value.subject is missing");
    let mut v = ok.clone();
    v["truncated"] = json!("no");
    refuses(&d, &page, v, "value.truncated must be a boolean");
    let mut v = ok;
    v["more"] = json!(1);
    refuses(
        &d,
        &page,
        v,
        "value has no field \"more\" (fields: subject, head_sha, next_cursor, truncated, items, total)",
    );
    // A result that is an interface which extends another.
    let comments = d.operation("review.comments").unwrap();
    assert_eq!(
        comments
            .check_result(&json!({"head_sha": null, "next_cursor": null, "truncated": false, "items": [], "total": 3}))
            .unwrap_err().message,
        "clax.review.comments: the result.subject is missing"
    );
}

#[test]
fn a_union_of_interfaces_is_told_apart_by_a_literal_field() {
    let d = types(
        "interface Named { subject: string }\ninterface Pull extends Named { kind: \"pull\"; title: string }\ninterface Ref extends Named { kind: \"ref\" | \"tag\"; name: string }\ninterface T { view: Pull | Ref; open: { ok: true; id: string } | { ok: false; why: string } }",
    );
    let view = field_ty(&d, "view");
    accepts(
        &d,
        &view,
        json!({"subject": "s", "kind": "pull", "title": "t"}),
    );
    accepts(
        &d,
        &view,
        json!({"subject": "s", "kind": "ref", "name": "main"}),
    );
    accepts(
        &d,
        &view,
        json!({"subject": "s", "kind": "tag", "name": "v1"}),
    );
    // The value says which it means, so the refusal is about that one.
    refuses(
        &d,
        &view,
        json!({"subject": "s", "kind": "ref", "title": "t"}),
        "value has no field \"title\" (fields: subject, kind, name)",
    );
    refuses(
        &d,
        &view,
        json!({"kind": "pull", "title": "t"}),
        "value.subject is missing",
    );
    refuses(
        &d,
        &view,
        json!({"subject": "s", "kind": "pull", "title": 5}),
        "value.title must be a string",
    );
    refuses(
        &d,
        &view,
        json!({"subject": "s", "kind": "branch"}),
        "value must be Pull | Ref",
    );
    refuses(
        &d,
        &view,
        json!({"subject": "s"}),
        "value must be Pull | Ref",
    );
    refuses(&d, &view, json!("pull"), "value must be Pull | Ref");
    let open = field_ty(&d, "open");
    accepts(&d, &open, json!({"ok": true, "id": "w1"}));
    accepts(&d, &open, json!({"ok": false, "why": "closed"}));
    refuses(
        &d,
        &open,
        json!({"ok": false, "id": "w1"}),
        "value has no field \"id\" (fields: ok, why)",
    );
    // A result that is a union of interfaces, in the example file.
    let review = review();
    let view = review.operation("review.view").unwrap();
    let e = view
        .check_result(&json!({"kind": "ref", "subject": "o/r@main", "head_sha": null, "url": "u"}))
        .unwrap_err()
        .message;
    assert_eq!(e, "clax.review.view: the result.ref is missing");
    assert_eq!(
        view.check_result(&json!({"kind": "fork"}))
            .unwrap_err()
            .message,
        "clax.review.view: the result must be PullView | CompareView | RefView"
    );
}

#[test]
fn content_is_a_whole_handle_once_clax_has_filled_it_in() {
    let d = types("interface T { c: Content; i: Image; shots: Image[] }");
    let (c, i) = (field_ty(&d, "c"), field_ty(&d, "i"));
    let handle = json!({"id": "c1", "media_type": "text/x-diff", "bytes": 48213, "sha256": "ab12"});
    accepts(&d, &c, handle.clone());
    accepts(
        &d,
        &c,
        json!({"id": "c1", "media_type": "image/png", "bytes": 0, "sha256": "ab", "name": "a.png", "width": 2, "height": 3, "meta": {"path": "a", "n": [1]}}),
    );
    accepts(
        &d,
        &c,
        json!({"id": "c1", "media_type": "text/plain", "bytes": 1, "sha256": "ab", "name": null, "meta": null}),
    );
    refuses(
        &d,
        &c,
        json!({"id": "c1", "media_type": "text/plain", "sha256": "ab"}),
        "value.bytes is missing",
    );
    // Sizes are whole and not negative.
    for (field, bad) in [
        ("bytes", json!(1.5)),
        ("bytes", json!(-1)),
        ("width", json!(-4)),
        ("height", json!("3")),
    ] {
        let mut v = json!({"id": "c1", "media_type": "image/png", "bytes": 1, "sha256": "ab", "width": 1, "height": 1});
        v[field] = bad;
        refuses(
            &d,
            &c,
            v,
            &format!("value.{field} must be a whole number, zero or more"),
        );
    }
    refuses(
        &d,
        &c,
        json!({"id": "c1", "media_type": "text/plain", "bytes": 1, "sha256": "ab", "meta": [1]}),
        "value.meta must be an object",
    );
    refuses(
        &d,
        &c,
        json!({"id": 7, "media_type": "text/plain", "bytes": 1, "sha256": "ab"}),
        "value.id must be a string",
    );
    // Bytes never travel in a value, and what the model was shown is not
    // part of a handle.
    for extra in ["data", "shown"] {
        let mut v = handle.clone();
        v[extra] = json!(false);
        refuses(
            &d,
            &c,
            v,
            &format!(
                "value has no field \"{extra}\" (fields: id, media_type, bytes, sha256, name, width, height, meta)"
            ),
        );
    }
    refuses(&d, &c, json!("c1"), "value must be an object");
    // An image is content of one of four types.
    for media_type in crate::IMAGE_MEDIA_TYPES {
        accepts(
            &d,
            &i,
            json!({"id": "c1", "media_type": media_type, "bytes": 1, "sha256": "ab", "width": 4, "height": 4}),
        );
    }
    refuses(
        &d,
        &i,
        handle.clone(),
        "value.media_type must be one of image/png, image/jpeg, image/webp, image/gif",
    );
    refuses(&d, &i, json!({"id": "c1"}), "value.media_type is missing");
    refuses(
        &d,
        &field_ty(&d, "shots"),
        json!([handle]),
        "value[0].media_type must be one of image/png, image/jpeg, image/webp, image/gif",
    );
}

#[test]
fn content_is_a_reference_as_a_provider_sends_it() {
    let d = types("interface T { c: Content; i: Image; page: { diff: Content; note?: string } }");
    let (c, i) = (field_ty(&d, "c"), field_ty(&d, "i"));
    for ty in [&c, &i] {
        assert_eq!(check_sent(&d, ty, &json!({"id": "c1"})), Ok(()));
        assert_eq!(
            check_sent(&d, ty, &json!({"id": "c1", "meta": {"path": "a.png"}})),
            Ok(())
        );
        assert_eq!(
            check_sent(&d, ty, &json!({"id": "c1", "meta": null})),
            Ok(())
        );
        assert_eq!(
            check_sent(&d, ty, &json!({})),
            Err("value.id is missing".to_string())
        );
        assert_eq!(
            check_sent(&d, ty, &json!({"id": 1})),
            Err("value.id must be a string".to_string())
        );
        assert_eq!(
            check_sent(&d, ty, &json!({"id": "c1", "meta": "x"})),
            Err("value.meta must be an object".to_string())
        );
        // What the bytes are is Clax's finding: a sender may not claim it.
        for claim in ["media_type", "bytes", "sha256", "width", "name", "shown"] {
            let mut v = json!({"id": "c1"});
            v[claim] = json!(1);
            assert_eq!(
                check_sent(&d, ty, &v),
                Err(format!("value has no field \"{claim}\" (fields: id, meta)"))
            );
        }
        // And a reference is not what a script may hold.
        assert_eq!(
            check_held(&d, ty, &json!({"id": "c1"})),
            Err("value.media_type is missing".to_string())
        );
    }
    // Everything else is the same on both sides, at any depth.
    let page = field_ty(&d, "page");
    assert_eq!(
        check_sent(&d, &page, &json!({"diff": {"id": "c9"}, "note": "n"})),
        Ok(())
    );
    assert_eq!(
        check_sent(&d, &page, &json!({"diff": {"id": "c9"}, "note": 1})),
        Err("value.note must be a string".to_string())
    );
    assert_eq!(
        check_held(&d, &page, &json!({"diff": {"id": "c9"}})),
        Err("value.diff.media_type is missing".to_string())
    );
}

#[test]
fn an_answer_is_checked_before_clax_fills_it_in_and_a_result_after() {
    let shell = shell();
    let shot = shell.operation("shell.screenshot").unwrap();
    let whole = json!({"id": "c1", "media_type": "image/png", "bytes": 9, "sha256": "ab", "width": 3, "height": 3});
    assert_eq!(
        shot.check_answer(&json!({"id": "c1", "meta": {"part": "bar"}})),
        Ok(())
    );
    assert_eq!(
        shot.check_answer(&whole).unwrap_err().message,
        "clax.shell.screenshot: the answer has no field \"bytes\" (fields: id, meta)"
    );
    assert_eq!(shot.check_result(&whole), Ok(()));
    assert_eq!(
        shot.check_result(&json!({"id": "c1"})).unwrap_err().message,
        "clax.shell.screenshot: the result.media_type is missing"
    );
    let mut text = whole.clone();
    text["media_type"] = json!("text/plain");
    assert_eq!(
        shot.check_result(&text).unwrap_err().message,
        "clax.shell.screenshot: the result.media_type must be one of image/png, image/jpeg, image/webp, image/gif"
    );
    // With no content in it, an answer and a result are checked alike.
    let clax = own();
    let list = clax.operation("files.list").unwrap();
    let files = json!([{"path": "a", "bytes": 1.5, "content_type": "text/html"}]);
    assert_eq!(
        list.check_answer(&files).unwrap_err().message,
        "clax.files.list: the answer[0].bytes must be an integer"
    );
    assert_eq!(
        list.check_result(&files).unwrap_err().message,
        "clax.files.list: the result[0].bytes must be an integer"
    );
    assert_eq!(list.check_answer(&json!([])), Ok(()));
    let get = clax.operation("db.get").unwrap();
    assert_eq!(get.check_answer(&json!(null)), Ok(()));
    assert_eq!(
        get.check_answer(&json!({"id": "d", "version": 2, "data": [1]})),
        Ok(())
    );
    assert_eq!(
        get.check_answer(&json!({"id": "d", "version": "2", "data": null}))
            .unwrap_err()
            .message,
        "clax.db.get: the answer.version must be an integer"
    );
}

#[test]
fn a_kinds_request_message_and_result_are_checked() {
    let message = |r: Result<(), crate::ValueError>| r.map_err(|e| e.message);
    let shell = shell();
    let triage = shell.kind("triage").unwrap();
    // A request comes from a starter: its content is a reference.
    let request = json!({
        "ask": "Make these two match.",
        "touches": [{"artifact": "a1", "title": "Notes"}, {"artifact": "a2", "title": "Bar", "part": "bar"}],
        "clip": {"id": "c1"},
    });
    assert_eq!(triage.check_request(&shell, &request), Ok(()));
    assert_eq!(
        message(triage.check_request(
            &shell,
            &json!({"ask": "x", "touches": [{"artifact": "a1"}]})
        )),
        Err("kind triage: the request.touches[0].title is missing".to_string())
    );
    assert_eq!(
        message(triage.check_request(&shell, &json!({"ask": ["x"], "touches": []}))),
        Err("kind triage: the request.ask must be a string".to_string())
    );
    // A whole handle is what a script holds, not what a starter sends.
    let mut whole = request.clone();
    whole["clip"] = json!({"id": "c1", "media_type": "image/png", "bytes": 9, "sha256": "ab"});
    assert_eq!(
        message(triage.check_request(&shell, &whole)),
        Err("kind triage: the request.clip has no field \"bytes\" (fields: id, meta)".to_string())
    );
    // What the run finishes with is the script's own value.
    assert_eq!(
        triage.check_result(
            &shell,
            &json!({"asked": [{"artifact": "a1", "run": "r1", "outcome": "finished"}], "summary": "Done."})
        ),
        Ok(())
    );
    assert_eq!(
        message(triage.check_result(&shell, &json!({"asked": [], "summary": "Done.", "note": 1}))),
        Err("kind triage: the result has no field \"note\" (fields: asked, summary)".to_string())
    );
    assert_eq!(
        message(triage.check_message(&shell, &json!("more"))),
        Err("kind triage takes no messages after it starts".to_string())
    );
    let clax = own();
    let brief = clax.kind("brief").unwrap();
    assert_eq!(
        brief.check_result(&clax, &json!({"summary": "Built it."})),
        Ok(())
    );
    assert_eq!(
        message(brief.check_result(&clax, &json!({}))),
        Err("kind brief: the result.summary is missing".to_string())
    );
    let thread = clax.kind("thread").unwrap();
    assert_eq!(
        thread.check_request(
            &clax,
            &json!({
                "artifact": {"id": "A1", "title": "T", "description": null, "version": 3, "url": null},
                "for": "Ada",
                "threads": [{"id": "t1", "anchor": {"x": 1}, "resolved": false, "comments": [
                    {"id": "c1", "author": "Ada", "from_sender": true, "at": "2026-11-10T10:00:00Z", "text": "Fix it"},
                ]}],
            })
        ),
        Ok(())
    );
    // A kind that takes messages, and whose result holds content: a
    // message's is a reference, the result's a whole handle.
    let d = host(&file(
        "",
        "interface Kinds {\n  /** Chat. @allow */\n  chat(request: { ask: Instruction }, message?: { say: Instruction; clip?: Image } | \"stop\"): { shot?: Image };\n}",
    ))
    .unwrap();
    let chat = d.kind("chat").unwrap();
    let whole = json!({"id": "c2", "media_type": "image/png", "bytes": 9, "sha256": "ab"});
    assert_eq!(chat.check_message(&d, &json!("stop")), Ok(()));
    assert_eq!(
        chat.check_message(&d, &json!({"say": "And the footer.", "clip": {"id": "c2"}})),
        Ok(())
    );
    assert_eq!(
        message(chat.check_message(&d, &json!({"say": 1}))),
        Err("kind chat: the message.say must be a string".to_string())
    );
    assert_eq!(
        message(chat.check_message(&d, &json!("go"))),
        Err(
            "kind chat: the message must be { say: Instruction; clip?: Image } | \"stop\""
                .to_string()
        )
    );
    assert_eq!(
        message(chat.check_message(&d, &json!({"say": "x", "clip": whole}))),
        Err("kind chat: the message.clip has no field \"bytes\" (fields: id, meta)".to_string())
    );
    assert_eq!(chat.check_result(&d, &json!({"shot": whole})), Ok(()));
    assert_eq!(chat.check_result(&d, &json!({})), Ok(()));
    assert_eq!(
        message(chat.check_result(&d, &json!({"shot": {"id": "c2"}}))),
        Err("kind chat: the result.shot.media_type is missing".to_string())
    );
    // A kind checked against a definition it is not one of is not
    // checked: a definition has its own kinds' types alone.
    let other = types("interface T { v: null }");
    for (kind, name) in [(chat, "chat"), (triage, "triage")] {
        let e = kind
            .check_request(&other, &json!({"ask": "x"}))
            .unwrap_err();
        assert_eq!(e.kind, ValueErrorKind::ForeignType);
        assert_eq!(
            e.message,
            format!(
                "kind {name} is not one of the kinds of surface `{}`",
                other.name()
            )
        );
    }
    // A copy of a kind is the kind.
    let copy = chat.clone();
    assert_eq!(copy.check_result(&d, &json!({"shot": whole})), Ok(()));
    // The free checks stay, for a type that is neither's.
    assert_eq!(
        check_sent(&d, chat.message.as_ref().unwrap(), &json!("go")),
        Err("value must be { say: Instruction; clip?: Image } | \"stop\"".to_string())
    );
    assert_eq!(
        check_held(&shell, &triage.result, &json!({"asked": []})),
        Err("value.summary is missing".to_string())
    );
}

/// What a refusal calls a type is what the model reads it as.
#[test]
fn a_refusal_names_a_type_as_the_model_reads_it() {
    for d in [own(), shell(), review()] {
        let mut all: Vec<&Ty> = Vec::new();
        for o in d.operations() {
            all.extend(o.op.params.iter().map(|p| &p.ty));
            all.push(&o.op.result);
        }
        all.extend(
            d.interfaces()
                .iter()
                .flat_map(|i| i.fields.iter().map(|f| &f.ty)),
        );
        for k in d.kinds() {
            all.extend([&k.request, &k.result]);
            all.extend(&k.message);
        }
        assert!(all.len() > 20, "{}", all.len());
        for ty in all {
            assert_eq!(
                crate::value::shown_whole(&d, ty),
                crate::print::model_text(ty)
            );
        }
    }
    let d = types(
        "interface T { v: (\"a\\\"b\" | \"c\\\\d\" | true | Int | { a?: Record<string, T[]>; b: {} } | null)[][] }",
    );
    let ty = field_ty(&d, "v");
    let text =
        "(\"a\\\"b\" | \"c\\\\d\" | true | number | { a?: Record<string, T[]>; b: {} } | null)[][]";
    assert_eq!(crate::print::model_text(&ty), text);
    assert_eq!(crate::value::shown_whole(&d, &ty), text);
    assert_eq!(
        check_held(&d, &ty, &json!([["x"]])),
        Err(format!("value[0][0] must be {}", &text[1..text.len() - 5]))
    );
}

#[test]
fn a_calls_arguments_are_checked_for_count_and_type() {
    let clax = own();
    let args = |op: &str, args: Value| {
        clax.operation(op)
            .unwrap()
            .check_args(args.as_array().unwrap())
            .map_err(|e| e.message)
    };
    assert_eq!(
        args("files.read", json!(["a.html"])),
        Ok(vec![json!("a.html")])
    );
    // An optional parameter left out, passed as null, or given.
    assert_eq!(args("draft.publish", json!([])), Ok(vec![Value::Null]));
    assert_eq!(args("draft.publish", json!([null])), Ok(vec![Value::Null]));
    assert_eq!(
        args("draft.publish", json!([{"note": "n"}])),
        Ok(vec![json!({"note": "n"})])
    );
    assert_eq!(
        args("db.list", json!(["c", null, null])),
        Ok(vec![json!("c"), Value::Null])
    );
    assert_eq!(
        args("web.search", json!(["q", 5])),
        Ok(vec![json!("q"), json!(5)])
    );
    assert_eq!(
        args("db.set", json!(["c", "d", {"x": [1]}])),
        Ok(vec![json!("c"), json!("d"), json!({"x": [1]}), Value::Null])
    );

    let e = |op: &str, a: Value| args(op, a).unwrap_err();
    assert_eq!(
        e("files.write", json!(["a.html"])),
        "clax.files.write: argument 2 (text) is missing"
    );
    assert_eq!(
        e("files.write", json!(["a.html", null])),
        "clax.files.write: argument 2 (text) is missing"
    );
    // `null` is "left out" for a required parameter of any type: today's rule.
    assert_eq!(
        e("db.set", json!(["c", "d", null])),
        "clax.db.set: argument 3 (data) is missing"
    );
    assert_eq!(
        e("files.read", json!([1])),
        "clax.files.read: argument 1 (path) must be a string"
    );
    assert_eq!(
        e("files.read", json!(["a", "b"])),
        "clax.files.read: takes at most 1 argument, got 2"
    );
    assert_eq!(
        e("files.write", json!(["a", "b", "c"])),
        "clax.files.write: takes at most 2 arguments, got 3"
    );
    assert_eq!(
        e("web.search", json!(["q", "5"])),
        "clax.web.search: argument 2 (limit) must be an integer"
    );
    assert_eq!(
        e("db.list", json!(["c", {"limt": 1}])),
        "clax.db.list: argument 2 (opts) has no field \"limt\" (fields: limit, cursor)"
    );
    assert_eq!(
        e("draft.publish", json!([{"addresses": ["t1", 2]}])),
        "clax.draft.publish: argument 1 (opts).addresses[1] must be a string"
    );
    assert_eq!(
        e("db.delete", json!(["c", "d", {}])),
        "clax.db.delete: argument 3 (opts).if_version is missing"
    );
    assert_eq!(
        e("db.get", json!(["c"])),
        "clax.db.get: argument 2 (id) is missing"
    );
    // The text `crates/clax-bot/src/api.rs` gives today.
    assert_eq!(
        e("web.fetch", json!(["u", {"format": "html"}])),
        "clax.web.fetch: argument 2 (opts).format must be one of auto, raw"
    );

    // An unknown field's name is the caller's own, and is quoted whole,
    // with what would not show escaped; in a provider's answer it is cut.
    let long = "k".repeat(100);
    assert_eq!(
        e("db.list", json!(["c", {long.as_str(): 1}])),
        format!("clax.db.list: argument 2 (opts) has no field \"{long}\" (fields: limit, cursor)")
    );
    assert_eq!(
        e("db.list", json!(["c", {"it's\0\u{202e}\"": 1}])),
        "clax.db.list: argument 2 (opts) has no field \"it's\\0\\u{202e}\\\"\" (fields: limit, cursor)"
    );
    assert_eq!(
        clax.operation("db.list")
            .unwrap()
            .check_answer(&json!({"docs": [], long.as_str(): 1}))
            .unwrap_err()
            .message,
        format!(
            "clax.db.list: the answer has no field \"{}…\" (fields: docs, next_cursor)",
            "k".repeat(64)
        )
    );
    // An apostrophe is itself in both, as `{:?}` writes a string.
    assert_eq!(
        clax.operation("db.list")
            .unwrap()
            .check_answer(&json!({"docs": [], "it's \"it\"": 1}))
            .unwrap_err()
            .message,
        "clax.db.list: the answer has no field \"it's \\\"it\\\"\" (fields: docs, next_cursor)"
    );
    assert_eq!(crate::check::shown("it's"), "it's");

    // A host's operation is named below `clax` too, and its arguments
    // are what a script holds: whole handles.
    let shell = shell();
    let shot = shell.operation("shell.screenshot").unwrap();
    assert_eq!(
        shot.check_args(&[json!({"part": "bar"})]),
        Ok(vec![json!({"part": "bar"})])
    );
    assert_eq!(
        shot.check_args(&[json!({"part": "bar", "window": "w"})])
            .unwrap_err()
            .message,
        "clax.shell.screenshot: argument 1 (of) must be { part: string } | { window: string }"
    );
    let d = host(&file(
        "  /** Puts it. @write */\n  put(path: string, c: Content): Promise<null>;",
        "",
    ))
    .unwrap();
    let put = d.operation("demo.put").unwrap();
    assert!(
        put.check_args(&[
            json!("a.png"),
            json!({"id": "c1", "media_type": "image/png", "bytes": 9, "sha256": "ab"})
        ])
        .is_ok()
    );
    assert_eq!(
        put.check_args(&[json!("a.png"), json!({"id": "c1"})])
            .unwrap_err()
            .message,
        "clax.demo.put: argument 2 (c).media_type is missing"
    );
}

// ---------------------------------------------------------- allow patterns

#[test]
fn allow_patterns_name_operations() {
    let p = |s: &str| Pattern::parse(s).unwrap();
    // Exact.
    assert!(p("shell.windows").matches("shell.windows"));
    assert!(!p("shell.windows").matches("shell.window"));
    assert!(!p("shell.windows").matches("shell.windows.open"));
    assert!(!p("shell.window").matches("shell.windows"));
    // A namespace.
    assert!(p("web.*").matches("web.fetch"));
    assert!(p("shell.settings.*").matches("shell.settings.get"));
    assert!(!p("shell.settings.*").matches("shell.windows"));
    assert!(!p("web.*").matches("web"));
    assert!(!p("web.*").matches("webhooks.list"));
    // A surface: everything below it, at any depth.
    assert!(p("review.*").matches("review.view"));
    assert!(p("review.*").matches("review.quest.note"));
    assert!(!p("review.*").matches("threads.open"));
    // Everything.
    assert!(p("*").matches("files.read"));

    for bad in [
        "",
        ".",
        "shell.",
        ".windows",
        "shell.*.get",
        "*.get",
        "shell.**",
        "shell windows",
        "shell.constructor",
        "shell.then",
        "a-b",
    ] {
        assert!(Pattern::parse(bad).is_err(), "{bad:?}");
    }
    assert!(Pattern::parse(&"x".repeat(100_000)).is_ok());
    assert!(Pattern::parse(&"x ".repeat(100_000)).unwrap_err().len() < 300);
    for s in ["*", "shell.*", "shell.settings.*", "files.read"] {
        assert_eq!(p(s).to_string(), s);
    }
    // Clax's own operations are named without `clax.`, and no other
    // surface takes the name: a pattern under it could match nothing,
    // and says what to write.
    for (bad, write) in [
        ("clax.files.read", "files.read"),
        ("clax.*", "*"),
        ("clax.files.*", "files.*"),
    ] {
        assert_eq!(
            Pattern::parse(bad),
            Err(format!(
                "`{bad}` is not an allow pattern: Clax's own operations are named without `clax.` (`{write}`)"
            ))
        );
    }
    assert_eq!(
        Pattern::parse("clax"),
        Err(
            "`clax` is not an allow pattern: Clax's own operations are named without `clax.` \
             (`files.read`, `files.*`)"
                .to_string()
        )
    );
    // A list allows what one of its patterns names, and an empty one nothing.
    let list = [p("shell.parts"), p("threads.*")];
    let allows = |list: &[Pattern], op: &str| list.iter().any(|p| p.matches(op));
    assert!(allows(&list, "shell.parts") && allows(&list, "threads.open"));
    assert!(!allows(&list, "shell.windows") && !allows(&[], "shell.parts"));
}

#[test]
fn a_kinds_allow_narrows_its_runs_operations() {
    let (clax, shell, review) = (own(), shell(), review());
    let triage = shell.kind("triage").unwrap();
    let had: Vec<&str> = shell
        .operations()
        .map(|o| o.name)
        .filter(|n| triage.allows(n))
        .collect();
    assert_eq!(
        had,
        [
            "shell.parts",
            "shell.windows",
            "shell.screenshot",
            "shell.settings.get",
            "shell.ask"
        ]
    );
    assert!(triage.allows("threads.open") && triage.allows("runs.wait"));
    assert!(!triage.allows("files.write"));

    let annotate = review.kind("annotate").unwrap();
    assert!(review.operations().all(|o| annotate.allows(o.name)));
    assert!(!annotate.allows("files.read"));
    assert!(!review.kind("plan").unwrap().allows("review.quest.note"));

    // A kind with no `@allow` narrows nothing.
    let thread = clax.kind("thread").unwrap();
    assert_eq!(thread.allow, None);
    assert!(clax.operations().all(|o| thread.allows(o.name)));
    assert!(thread.allows("shell.ask"));

    // Whether a pattern names anything is for whoever holds every
    // surface of a run to ask: `threads.open` and `runs.wait` are not
    // operations of any file here.
    let named = |surfaces: &[&Definition]| -> Vec<String> {
        triage
            .allow
            .as_ref()
            .unwrap()
            .iter()
            .filter(|p| !p.names_any(surfaces))
            .map(Pattern::to_string)
            .collect()
    };
    assert_eq!(named(&[&shell, &clax]), ["threads.open", "runs.wait"]);
    assert_eq!(named(&[&clax]).len(), 7);
    assert!(
        Pattern::parse("review.quest.*")
            .unwrap()
            .names_any(&[&clax, &review])
    );
    assert!(
        !Pattern::parse("review.quests.*")
            .unwrap()
            .names_any(&[&clax, &review])
    );
    assert!(!Pattern::parse("*").unwrap().names_any(&[]));
}

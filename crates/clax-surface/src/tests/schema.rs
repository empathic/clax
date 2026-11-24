//! A kind's result as the JSON Schema of a tool's input
//! (`Kind::result_schema`).

use super::manifest::manifest;
use super::{file, host, review, shell};
use crate::Definition;
use serde_json::{Value, json};

/// A host's file whose one kind, `k`, finishes with `result`, beside
/// `interfaces`.
fn kind_of(result: &str, interfaces: &str) -> Definition {
    let src = kind_file(result, interfaces);
    host(&src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn kind_file(result: &str, interfaces: &str) -> String {
    file(
        "  /** Reads. @read */\n  get(): Promise<null>;",
        &format!(
            "{interfaces}\ninterface Kinds {{\n  /** The request. @allow */\n  k(request: {{ ask: Instruction }}): {result};\n}}\n"
        ),
    )
}

fn schema(d: &Definition) -> Value {
    d.kind("k").unwrap().result_schema(d).expect("an object")
}

#[test]
fn a_summary_is_the_object_with_its_one_required_string() {
    let want = json!({"type": "object", "properties": {"summary": {"type": "string"}},
                      "required": ["summary"], "additionalProperties": false});
    // Written where it is used, and as an interface whose doc comment
    // the schema leaves to the tool's description.
    assert_eq!(schema(&kind_of("{ summary: string }", "")), want);
    assert_eq!(
        schema(&kind_of(
            "Done",
            "/** The summary tells the person what you did. */\ninterface Done {\n  summary: string;\n}"
        )),
        want
    );
}

#[test]
fn fields_keep_their_order_their_docs_and_which_are_required() {
    let d = kind_of(
        "Verdict",
        "interface Base {\n  /** What was decided. */\n  title: string;\n}\ninterface Verdict extends Base {\n  confidence?: Int;\n  status: \"upheld\" | \"dismissed\";\n}",
    );
    assert_eq!(
        schema(&d),
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "What was decided."},
                "confidence": {"type": "integer"},
                "status": {"enum": ["upheld", "dismissed"]},
            },
            // The inherited field first, then the interface's own, in
            // the order written.
            "required": ["title", "status"],
            "additionalProperties": false,
        })
    );
}

/// The text of the schema keeps the definition's order, which a
/// `serde_json::Value` (sorted maps, as this workspace builds it) does
/// not: the inherited field first, then the interface's own as written,
/// and `$defs` in the order the interfaces are declared. The text reads
/// as the schema.
#[test]
fn the_schemas_text_keeps_the_definitions_order_of_fields_and_defs() {
    let d = kind_of(
        "Verdict",
        "interface Zone {\n  z: string;\n}\ninterface Area {\n  a: string;\n}\ninterface Base {\n  title: string;\n}\ninterface Verdict extends Base {\n  confidence?: Int;\n  status: Area;\n  where: Zone;\n}",
    );
    let k = d.kind("k").unwrap();
    let text = k.result_schema_text(&d).expect("an object");
    assert_eq!(
        text,
        concat!(
            r##"{"type": "object", "properties": {"title": {"type": "string"}, "##,
            r##""confidence": {"type": "integer"}, "status": {"$ref": "#/$defs/Area"}, "##,
            r##""where": {"$ref": "#/$defs/Zone"}}, "required": ["title", "status", "where"], "##,
            r##""additionalProperties": false, "$defs": {"Zone": {"type": "object", "##,
            r##""properties": {"z": {"type": "string"}}, "required": ["z"], "##,
            r##""additionalProperties": false}, "Area": {"type": "object", "##,
            r##""properties": {"a": {"type": "string"}}, "required": ["a"], "##,
            r##""additionalProperties": false}}}"##,
        )
    );
    let read: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(Some(read), k.result_schema(&d));
}

#[test]
fn an_interface_the_result_reaches_is_under_defs_and_only_those() {
    let d = kind_of(
        "Out",
        "interface Unused {\n  x: string;\n}\n/** A part. */\ninterface Part {\n  name: string;\n  next: Part | null;\n}\ninterface Out {\n  parts: Part[];\n  shot?: Image;\n  file: Content;\n}",
    );
    let handle = |media_type: Value| {
        json!({"type": "object", "properties": {
                   "id": {"type": "string"}, "media_type": media_type,
                   "bytes": {"type": "integer"}, "sha256": {"type": "string"},
                   "name": {"type": "string"}, "width": {"type": "integer"},
                   "height": {"type": "integer"}, "meta": {"type": "object"}},
               "required": ["id", "media_type", "bytes", "sha256"],
               "additionalProperties": false})
    };
    assert_eq!(
        schema(&d),
        json!({
            "type": "object",
            "properties": {
                "parts": {"type": "array", "items": {"$ref": "#/$defs/Part"}},
                "shot": handle(json!({"enum": ["image/png", "image/jpeg", "image/webp", "image/gif"]})),
                "file": handle(json!({"type": "string"})),
            },
            "required": ["parts", "file"],
            "additionalProperties": false,
            "$defs": {
                "Part": {
                    "description": "A part.",
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "next": {"anyOf": [{"$ref": "#/$defs/Part"}, {"type": "null"}]},
                    },
                    "required": ["name", "next"],
                    "additionalProperties": false,
                },
            },
        })
    );
}

/// A kind's result is an interface or an object type: a run's value
/// is the input of a tool, which is an object. Anything else is refused
/// in each notation's words, saying to wrap the value in an object.
#[test]
fn a_result_that_is_not_an_object_is_refused_in_both_notations() {
    let interfaces = "interface T {\n  a: string;\n}";
    for (result, form) in [
        ("string[]", "an array"),
        ("string", "a string"),
        ("Int", "a number"),
        ("unknown", "`unknown`"),
        ("null", "`null`"),
        ("\"done\"", "a string"),
        ("T | null", "a union"),
        ("T | { b: Int }", "a union"),
        ("Record<string, Int>", "a `Record`"),
        ("Content", "a type of Clax's"),
    ] {
        let src = kind_file(result, interfaces);
        let e = host(&src).expect_err(result);
        assert_eq!(
            e.message,
            format!(
                "the result of kind `k` is {form}, and a run finishes by calling a tool whose input is an object: make the result an interface or an object type, wrapping the value in a field (`{{ value: … }}`)"
            ),
            "{result}"
        );
    }
    // The manifest says it in its own words, at the kind's `result`.
    let doc = |result: &str| {
        format!(
            r#"{{"clax_surface":1,"name":"demo","operations":[{{"name":"get","description":"Reads.","access":"read","params":[],"result":{{"type":"null"}}}}],"kinds":[{{"name":"k","description":"K.","allow":[],"request":{{"type":"null"}},"result":{result}}}]}}"#
        )
    };
    let e = manifest(&doc(r#"{"type": "array", "items": {"type": "string"}}"#)).unwrap_err();
    assert!(
        e.message.starts_with(
            "the `result` of kind `k` is an array, and a run finishes by calling a tool whose input is an object: make it an object type or a `$ref` to an interface, wrapping the value in a property"
        ),
        "{e}"
    );
    // An object type there is taken, and is its schema.
    let d = manifest(&doc(
        r#"{"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"], "additionalProperties": false}"#,
    ))
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        schema(&d),
        json!({"type": "object", "properties": {"n": {"type": "integer"}},
               "required": ["n"], "additionalProperties": false})
    );
}

/// A kind of a definition always has a schema: one built by hand with a
/// result that is not an object has none.
#[test]
fn a_kind_whose_result_is_not_an_object_has_no_schema() {
    let d = kind_of("{ summary: string }", "");
    let mut k = d.kind("k").unwrap().clone();
    k.result = crate::Ty::Str;
    assert_eq!(k.result_schema(&d), None);
    k.result = crate::Ty::Named("Missing".into());
    assert_eq!(k.result_schema(&d), None);
}

/// An interface reached through another is under `$defs` too: no
/// `$ref` is left dangling.
#[test]
fn every_interface_reached_through_another_is_under_defs() {
    let d = kind_of(
        "Out",
        "interface B {\n  n: Int;\n}\ninterface A {\n  b: B;\n}\ninterface Out {\n  a: A;\n}",
    );
    let s = schema(&d);
    assert_eq!(s["properties"]["a"], json!({"$ref": "#/$defs/A"}));
    assert_eq!(
        s["$defs"],
        json!({
            "B": {"type": "object", "properties": {"n": {"type": "integer"}},
                  "required": ["n"], "additionalProperties": false},
            "A": {"type": "object", "properties": {"b": {"$ref": "#/$defs/B"}},
                  "required": ["b"], "additionalProperties": false},
        })
    );
}

/// The design's two example files have a schema for every kind.
#[test]
fn every_kind_of_the_example_files_has_a_schema() {
    for d in [shell(), review()] {
        for k in d.kinds() {
            assert!(k.result_schema(&d).is_some(), "{}", k.name);
        }
    }
}

#[test]
fn the_shell_files_triage_finishes_with_its_result() {
    let d = shell();
    assert_eq!(
        d.kind("triage").unwrap().result_schema(&d).unwrap(),
        json!({
            "type": "object",
            "properties": {
                "asked": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "artifact": {"type": "string"},
                        "run": {"type": "string"},
                        "outcome": {"type": "string"},
                    },
                    "required": ["artifact", "run", "outcome"],
                    "additionalProperties": false,
                }},
                "summary": {"type": "string"},
            },
            "required": ["asked", "summary"],
            "additionalProperties": false,
        })
    );
}

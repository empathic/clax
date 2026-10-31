//! A thread as W3C Web Annotations (<https://www.w3.org/TR/annotation-model/>):
//! read-only, made from the thread's view as the reader reads it, so what
//! visibility withholds there is withheld here too.
//!
//! The thread is an `AnnotationPage`: its first comment is the annotation
//! (`commenting`), on the anchored version's page; each later comment is a
//! `replying` annotation on that one. The target's `source` is the
//! version's URL (`/a/{aid}/v/{n}`), which never changes, so it is the
//! evidence of what the commenter saw; its selector is the anchor's:
//! a `CssSelector` for the element, refined by a `TextQuoteSelector` for the
//! quote or a media fragment (`xywh=percent:`) for a drawn area, and a
//! `FragmentSelector` naming a custom anchor. A whole-artifact anchor, or
//! one whose selector is withheld, targets the version itself.

use clax_core::anchor::INDEX_FILE;
use serde_json::{Map, Value, json};

/// The Web Annotation JSON-LD context.
pub const CONTEXT: &str = "http://www.w3.org/ns/anno.jsonld";
/// The media type an annotation is served as (spec §2 of the protocol).
pub const MEDIA_TYPE: &str = "application/ld+json; profile=\"http://www.w3.org/ns/anno.jsonld\"";

/// Thread view `t` (as [`super::thread::Vis::filter_thread`] renders it)
/// as an `AnnotationPage` of its comments, oldest first.
#[must_use]
pub fn thread_annotations(t: &Value) -> Value {
    let aid = t["artifact_id"].as_str().unwrap_or_default();
    let tid = t["id"].as_str().unwrap_or_default();
    let thread_iri = format!("/a/{aid}?thread={tid}");
    let target = target(
        aid,
        t["version_n"].as_u64().unwrap_or_default(),
        &t["anchor"],
    );
    let comments = t["comments"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let root = comments
        .first()
        .and_then(|c| c["id"].as_str())
        .map(|cid| format!("{thread_iri}#{cid}"));
    let items: Vec<Value> = comments
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let cid = c["id"].as_str().unwrap_or_default();
            let mut a = json!({
                "id": format!("{thread_iri}#{cid}"),
                "type": "Annotation",
                "motivation": if i == 0 { "commenting" } else { "replying" },
                "creator": creator(c),
                "created": c["created_at"],
                "target": match (&root, i) {
                    (Some(r), 1..) => json!(r),
                    _ => target.clone(),
                },
            });
            if let Some(body) = c["body"].as_str() {
                a["body"] = json!({"type": "TextualBody", "value": body, "format": "text/plain"});
            }
            a
        })
        .collect();
    json!({
        "@context": CONTEXT,
        "id": thread_iri,
        "type": "AnnotationPage",
        "items": items,
    })
}

/// A comment's author: a `Person` for a viewer, `Software` for an agent.
fn creator(c: &Value) -> Value {
    let kind = if c["author_kind"] == clax_core::model::AUTHOR_AGENT {
        "Software"
    } else {
        "Person"
    };
    json!({"type": kind, "name": c["author_name"]})
}

/// The anchor's page of version `n` of `aid`, narrowed by its selector.
fn target(aid: &str, n: u64, anchor: &Value) -> Value {
    let source = format!("/a/{aid}/v/{n}");
    let str_of = |k: &str| anchor[k].as_str().filter(|s| !s.is_empty());
    let mut selector = match (anchor["kind"].as_str(), str_of("selector")) {
        (Some("custom"), _) => {
            str_of("custom_name").map(|name| json!({"type": "FragmentSelector", "value": name}))
        }
        (Some("element" | "range" | "area"), Some(css)) => {
            Some(json!({"type": "CssSelector", "value": css}))
        }
        _ => None,
    };
    if let Some(sel) = selector.as_mut() {
        if let Some(exact) = str_of("quote") {
            let mut q = Map::new();
            q.insert("type".into(), json!("TextQuoteSelector"));
            q.insert("exact".into(), json!(exact));
            for k in ["prefix", "suffix"] {
                if let Some(v) = str_of(k) {
                    q.insert(k.into(), json!(v));
                }
            }
            sel["refinedBy"] = Value::Object(q);
        } else if let Some(area) = anchor["area"].as_object() {
            let pct = |k: &str| area.get(k).and_then(Value::as_f64).unwrap_or_default() * 100.0;
            sel["refinedBy"] = json!({
                "type": "FragmentSelector",
                "conformsTo": "http://www.w3.org/TR/media-frags/",
                "value": format!(
                    "xywh=percent:{},{},{},{}",
                    round2(pct("x")),
                    round2(pct("y")),
                    round2(pct("w")),
                    round2(pct("h"))
                ),
            });
        }
    }
    let file = str_of("file").filter(|f| *f != INDEX_FILE);
    if selector.is_none() && file.is_none() {
        return json!(source);
    }
    let mut t = json!({"type": "SpecificResource", "source": source});
    if let Some(sel) = selector {
        t["selector"] = sel;
    }
    if let Some(f) = file {
        // The page within the version: no Web Annotation term names it.
        t["clax:file"] = json!(f);
    }
    t
}

/// `v` to two decimal places, without trailing zeros.
fn round2(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::model::Thread;

    /// The view of a stored thread anchored by `anchor`, with a viewer's
    /// comment and an agent's reply.
    fn view(anchor: Value) -> Value {
        let t: Thread = serde_json::from_value(json!({
            "id": "t1",
            "artifact_id": "7q3k9mzx2b4t",
            "version_n": 3,
            "anchor": anchor,
            "status": "open",
            "sent_to_agent": false,
            "has_clip": false,
            "created_at": "2026-10-31T00:00:00Z",
            "resolved_at": null,
            "resolved_by": null,
            "comments": [
                {"id": "c1", "thread_id": "t1", "author_kind": "viewer",
                 "author_name": "Bobby", "via_harness": null, "body": "Why 7?",
                 "created_at": "2026-10-31T00:00:00Z"},
                {"id": "c2", "thread_id": "t1", "author_kind": "agent",
                 "author_name": "claude", "via_harness": "claude", "body": "Fixed.",
                 "created_at": "2026-10-31T00:01:00Z"},
            ],
        }))
        .unwrap();
        json!(t)
    }

    fn first_target(anchor: Value) -> Value {
        thread_annotations(&view(anchor))["items"][0]["target"].clone()
    }

    #[test]
    fn a_thread_is_a_page_of_its_comment_and_replies() {
        let page = thread_annotations(&view(json!({"kind": "artifact", "file": ""})));
        assert_eq!(page["@context"], CONTEXT);
        assert_eq!(page["type"], "AnnotationPage");
        assert_eq!(page["id"], "/a/7q3k9mzx2b4t?thread=t1");
        let [first, reply] = page["items"].as_array().unwrap().as_slice() else {
            panic!("two items: {page}");
        };
        assert_eq!(first["motivation"], "commenting");
        assert_eq!(first["creator"], json!({"type": "Person", "name": "Bobby"}));
        assert_eq!(first["body"]["value"], "Why 7?");
        assert_eq!(first["created"], "2026-10-31T00:00:00Z");
        assert_eq!(reply["motivation"], "replying");
        assert_eq!(reply["creator"]["type"], "Software");
        assert_eq!(reply["target"], first["id"]);
    }

    #[test]
    fn an_artifact_anchor_targets_the_version() {
        assert_eq!(
            first_target(json!({"kind": "artifact", "file": ""})),
            "/a/7q3k9mzx2b4t/v/3"
        );
    }

    #[test]
    fn an_element_anchor_is_a_css_selector() {
        assert_eq!(
            first_target(json!({"kind": "element", "selector": "#chart > svg"})),
            json!({
                "type": "SpecificResource",
                "source": "/a/7q3k9mzx2b4t/v/3",
                "selector": {"type": "CssSelector", "value": "#chart > svg"},
            })
        );
    }

    #[test]
    fn a_range_anchor_refines_its_element_by_the_quote() {
        let t = first_target(json!({
            "kind": "range", "selector": "p:nth-of-type(2)",
            "quote": "p95", "prefix": "the ", "suffix": " window",
        }));
        assert_eq!(
            t["selector"],
            json!({
                "type": "CssSelector",
                "value": "p:nth-of-type(2)",
                "refinedBy": {"type": "TextQuoteSelector", "exact": "p95",
                              "prefix": "the ", "suffix": " window"},
            })
        );
    }

    #[test]
    fn an_area_anchor_refines_its_element_by_a_media_fragment() {
        let t = first_target(json!({
            "kind": "area", "selector": "main img",
            "area": {"x": 0.1, "y": 0.25, "w": 0.5, "h": 0.125},
        }));
        assert_eq!(
            t["selector"]["refinedBy"],
            json!({
                "type": "FragmentSelector",
                "conformsTo": "http://www.w3.org/TR/media-frags/",
                "value": "xywh=percent:10,25,50,12.5",
            })
        );
    }

    #[test]
    fn a_custom_anchor_is_a_fragment_naming_it() {
        let t = first_target(json!({"kind": "custom", "custom_name": "row-42"}));
        assert_eq!(
            t["selector"],
            json!({"type": "FragmentSelector", "value": "row-42"})
        );
    }

    #[test]
    fn a_page_other_than_the_index_is_named() {
        let t = first_target(json!({"kind": "element", "selector": "h1", "file": "docs/b.html"}));
        assert_eq!(t["clax:file"], "docs/b.html");
        let t = first_target(json!({"kind": "artifact", "file": "docs/b.html"}));
        assert_eq!(
            t,
            json!({"type": "SpecificResource", "source": "/a/7q3k9mzx2b4t/v/3", "clax:file": "docs/b.html"})
        );
    }

    #[test]
    fn what_the_view_withholds_is_left_out() {
        let mut v = view(json!({"kind": "range", "selector": "p", "quote": "secret"}));
        clax_core::visibility::placeholder::anchor(&mut v);
        clax_core::visibility::placeholder::comment(&mut v["comments"][0]);
        let page = thread_annotations(&v);
        let first = &page["items"][0];
        assert_eq!(first["target"], "/a/7q3k9mzx2b4t/v/3");
        assert!(first.get("body").is_none(), "{first}");
        assert!(!page.to_string().contains("secret"));
    }
}

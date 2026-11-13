//! The images a script step shows the model
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
//! "Content", "Showing the model").
//!
//! - **Which.** After a script step, each `Image` handle in the script's
//!   returned value that the run holds ([`found`]: an object whose `id`
//!   names an item of the run's content with the same `sha256` and an
//!   image's media type), in the order the value is written out, each
//!   once. [`RunLimits::images_per_step`] of them at most, each of
//!   [`RunLimits::image_bytes`] at most and [`RunLimits::image_side`]
//!   pixels a side at most (as its header says), and none while the run
//!   sends the model none ([`crate::RunSettings::model_takes_images`]):
//!   each one left out is listed with why ([`choose`]).
//! - **What the tool result says.** Its JSON gains `images`, one entry
//!   for each such handle, `{"id", "shown": true}` or `{"id", "shown":
//!   false, "why"}`. A value that holds no image handle gains nothing, so
//!   its tool result is the one it was before content.
//! - **What is kept.** The step's payload keeps the handles of the images
//!   shown (`images: [{id, sha256, media_type}]`) and its `result` the
//!   text alone: no image's bytes are stored with a step, sent with its
//!   events, or carried in the history.
//! - **When it is shown.** Each turn until one is answered after the step
//!   is sent its tool result with the images after the text, read from
//!   the store when the request is built ([`show`]): the turn that answers
//!   the step, or, for a run stopped after the step and continued, the
//!   continuation's first. A turn refused for its length showed nothing.
//!   Every turn after one that was answered is sent the text and
//!   [`IMAGES_SHOWN_ONCE`]: each image is seen by the model once. An item
//!   is marked shown (`bot_content.shown`) when the turn that was sent it
//!   commits with the model's answer, and not before.

use crate::policy::RunLimits;
use clax_core::Result;
use clax_sample::messages::{Block, Message, Role, ToolImage};
use clax_store::Store;
use clax_store::bot_content::ContentRow;
use clax_store::bot_runs::{Step, StepKind};
use clax_surface::IMAGE_MEDIA_TYPES;
use serde_json::{Value, json};

/// What ends the text of an earlier script step's tool result whose
/// images were shown with it, on every turn after the one that showed
/// them.
pub const IMAGES_SHOWN_ONCE: &str =
    "The images of this result were shown once, with it, and are not shown again.";

/// Why an image is not shown when the run sends the model none
/// ([`crate::RunSettings::model_takes_images`] is false): what was done,
/// not a claim about the model, which nothing here knows.
const NOT_SENT: &str = "images are not sent to the model here ([bot] model_takes_images is off)";

/// The most values [`found`] looks at in one script's value: one past the
/// sandbox's result limit could hold no more.
const MOST_VALUES: usize = 1 << 20;

/// The image handles in `value`, a script's returned value, that are of
/// `run`'s content: in the order the value is written out (an array's
/// elements in order, an object's members in the order its map keeps
/// them), each once, and nothing inside a handle.
///
/// # Errors
/// The store's.
pub fn found(store: &Store, run: &str, value: &Value) -> Result<Vec<ContentRow>> {
    let mut out: Vec<ContentRow> = Vec::new();
    let mut todo = vec![value];
    let mut seen = 0usize;
    while let Some(v) = todo.pop() {
        seen += 1;
        if seen > MOST_VALUES {
            break;
        }
        match v {
            Value::Array(items) => todo.extend(items.iter().rev()),
            Value::Object(o) => {
                let id = o.get("id").and_then(Value::as_str);
                let sha = o.get("sha256").and_then(Value::as_str);
                let media = o.get("media_type").and_then(Value::as_str);
                // A handle of any media type is one: nothing inside it
                // (its `meta`) is looked at.
                if let (Some(id), Some(sha), Some(media)) = (id, sha, media) {
                    if IMAGE_MEDIA_TYPES.contains(&media)
                        && out.iter().all(|r| r.id != id)
                        && let Some(row) = store.bot_content_of(run, id)?
                        && row.sha256 == sha
                        && IMAGE_MEDIA_TYPES.contains(&row.media_type.as_str())
                    {
                        out.push(row);
                    }
                    continue;
                }
                let members: Vec<&Value> = o.values().collect();
                todo.extend(members.into_iter().rev());
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Which of `found` are shown, as the tool result tells it (`told`: one
/// entry for each, in order) and as the step keeps it (`shown`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chosen {
    /// The tool result's `images`.
    pub told: Vec<Value>,
    /// The items shown, in order.
    pub shown: Vec<ContentRow>,
}

impl Chosen {
    /// The step's payload's `images`: the shown items' `id`, `sha256` and
    /// `media_type`.
    #[must_use]
    pub fn kept(&self) -> Value {
        Value::Array(
            self.shown
                .iter()
                .map(|r| json!({"id": r.id, "sha256": r.sha256, "media_type": r.media_type}))
                .collect(),
        )
    }
}

/// Which of `found` the model is shown under `limits`, for a model that
/// takes images or not.
#[must_use]
pub fn choose(found: Vec<ContentRow>, takes_images: bool, limits: &RunLimits) -> Chosen {
    let mut chosen = Chosen::default();
    for row in found {
        let why = if !takes_images {
            Some(NOT_SENT.to_string())
        } else if chosen.shown.len() >= limits.images_per_step {
            Some(format!(
                "{} images at the most are shown after a step",
                limits.images_per_step
            ))
        } else if row.bytes > limits.image_bytes {
            Some(format!(
                "an image shown is {} bytes at the most, and this is {}",
                limits.image_bytes, row.bytes
            ))
        } else if row.width.unwrap_or(0) > limits.image_side
            || row.height.unwrap_or(0) > limits.image_side
        {
            Some(format!(
                "an image shown is {} pixels a side at the most, and this is {}x{}",
                limits.image_side,
                row.width.unwrap_or(0),
                row.height.unwrap_or(0)
            ))
        } else {
            None
        };
        match why {
            Some(why) => chosen
                .told
                .push(json!({"id": row.id, "shown": false, "why": why})),
            None => {
                chosen.told.push(json!({"id": row.id, "shown": true}));
                chosen.shown.push(row);
            }
        }
    }
    chosen
}

/// Whether `step` is a turn the model answered: one that was sent its
/// request and committed an answer, not one refused for its length
/// (`prompt_too_long`, which commits a turn with no answer).
fn answered(step: &Step) -> bool {
    step.kind == StepKind::Turn
        && step
            .payload
            .as_ref()
            .is_some_and(|p| p["stop_reason"] != "prompt_too_long")
}

/// `messages`, the history a turn is sent, with the images script steps
/// showed, and the IDs of the items it is sent: the tool result of a
/// script step that kept images and that no answered turn came after
/// (one the model was not yet shown it in) has them after its text, read
/// from the store (for a model that takes images; `takes_images`); each
/// script step that kept images and that an answered turn came after has
/// [`IMAGES_SHOWN_ONCE`] after its text. A history whose steps kept no
/// image is returned as it is, with no ID.
///
/// # Errors
/// The store's, reading an image's bytes.
pub fn show(
    store: &Store,
    run: &str,
    steps: &[Step],
    takes_images: bool,
    mut messages: Vec<Message>,
) -> Result<(Vec<Message>, Vec<String>)> {
    let committed: Vec<&Step> = steps.iter().filter(|s| s.committed_at.is_some()).collect();
    let mut sent = Vec::new();
    for (at, step) in committed.iter().enumerate() {
        let Some(p) = step.payload.as_ref() else {
            continue;
        };
        let kept = p["images"].as_array().filter(|i| !i.is_empty());
        let (StepKind::Script, Some(kept)) = (step.kind, kept) else {
            continue;
        };
        let Some(id) = p["tool_use_id"].as_str() else {
            continue;
        };
        let latest = !committed[at + 1..].iter().any(|s| answered(s));
        // The images, read once and only when a tool result is found to
        // put them in: messages a policy gave may have left the step's
        // out, and then no image is sent, so none is said sent.
        let mut images: Option<Vec<ToolImage>> = None;
        for m in messages.iter_mut().filter(|m| m.role == Role::User) {
            for b in &mut m.content {
                // A stored tool result is read back as JSON (`Block::Raw`).
                let v = b.to_json();
                if v["type"] != "tool_result" || v["tool_use_id"] != id {
                    continue;
                }
                let Some(text) = v["content"].as_str() else {
                    continue;
                };
                let is_error = v["is_error"] == true;
                let replaced = if latest {
                    let images = match &mut images {
                        Some(images) => images,
                        None => {
                            let read = if takes_images {
                                read_kept(store, run, kept, &mut sent)?
                            } else {
                                Vec::new()
                            };
                            images.insert(read)
                        }
                    };
                    Block::tool_result_with_images(id, text, is_error, images, true)
                } else {
                    Block::tool_result(id, format!("{text}\n\n{IMAGES_SHOWN_ONCE}"), is_error)
                };
                *b = match v.get("cache_control") {
                    Some(cache) => {
                        let mut j = replaced.to_json();
                        j["cache_control"] = cache.clone();
                        Block::Raw(j)
                    }
                    None => replaced,
                };
            }
        }
    }
    Ok((messages, sent))
}

/// The images a request body holds: how many `image` blocks with a
/// base64 source it has, wherever they are (a message's content or a
/// tool result's), and the length of their base64 data, which the
/// body's JSON text carries as it is. A turn's estimate of its cost
/// prices these apart from the text ([`crate::run::IMAGE_INPUT_TOKENS`]).
#[must_use]
pub fn in_body(body: &Value) -> (u64, usize) {
    let (mut count, mut bytes) = (0u64, 0usize);
    let mut todo = vec![body];
    while let Some(v) = todo.pop() {
        match v {
            Value::Array(items) => todo.extend(items.iter()),
            Value::Object(o) => {
                if o.get("type").and_then(Value::as_str) == Some("image")
                    && let Some(source) = o.get("source")
                    && source["type"] == "base64"
                    && let Some(data) = source["data"].as_str()
                {
                    count += 1;
                    bytes += data.len();
                    continue;
                }
                todo.extend(o.values());
            }
            _ => {}
        }
    }
    (count, bytes)
}

/// The bytes of the images a step kept (`kept`, its payload's `images`)
/// that the run still holds with the same hash, in order, with each one's
/// ID pushed onto `sent`.
fn read_kept(
    store: &Store,
    run: &str,
    kept: &[Value],
    sent: &mut Vec<String>,
) -> Result<Vec<ToolImage>> {
    let mut images = Vec::new();
    for k in kept {
        let row = store.bot_content_of(run, k["id"].as_str().unwrap_or_default())?;
        let Some(row) = row.filter(|r| k["sha256"] == r.sha256) else {
            continue;
        };
        if let Some(bytes) = store.bot_content_bytes(&row)? {
            images.push(ToolImage::new(row.media_type.clone(), bytes));
            sent.push(row.id);
        }
    }
    Ok(images)
}

/// What a script step's tool result and payload gain for the images its
/// value holds: the tool result's `images` and the payload's, `None`
/// when the value holds no image handle.
///
/// # Errors
/// The store's.
pub fn of_step(
    store: &Store,
    run: &str,
    value: Option<&Value>,
    takes_images: bool,
    limits: &RunLimits,
) -> Result<Option<Chosen>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let found = found(store, run, value)?;
    if found.is_empty() {
        return Ok(None);
    }
    Ok(Some(choose(found, takes_images, limits)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunSettings;

    fn row(id: &str, bytes: u64) -> ContentRow {
        ContentRow {
            id: id.into(),
            run_id: "r".into(),
            surface: "lens".into(),
            registration_id: None,
            op: None,
            key: None,
            step_n: None,
            name: None,
            sha256: "0".repeat(64),
            bytes,
            media_type: "image/png".into(),
            width: Some(1),
            height: Some(1),
            meta: None,
            shown: false,
            created_at: String::new(),
        }
    }

    #[test]
    fn four_are_shown_in_order_and_each_one_left_out_says_why() {
        let limits = RunLimits::of(&RunSettings::default());
        let rows: Vec<ContentRow> = ["a", "b", "big", "c", "d", "e"]
            .iter()
            .map(|id| {
                row(
                    id,
                    if *id == "big" {
                        limits.image_bytes + 1
                    } else {
                        10
                    },
                )
            })
            .collect();
        let chosen = choose(rows.clone(), true, &limits);
        let shown: Vec<&str> = chosen.shown.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(shown, ["a", "b", "c", "d"]);
        assert_eq!(chosen.told.len(), 6);
        assert_eq!(chosen.told[2]["shown"], false);
        assert!(
            chosen.told[2]["why"]
                .as_str()
                .unwrap()
                .contains("bytes at the most")
        );
        assert_eq!(
            chosen.told[5]["why"],
            "4 images at the most are shown after a step"
        );
        // A model that takes none is shown none, and told so of each.
        let none = choose(rows, false, &limits);
        assert!(none.shown.is_empty());
        assert!(
            none.told
                .iter()
                .all(|t| t["shown"] == false && t["why"] == NOT_SENT)
        );
        assert_eq!(none.kept(), json!([]));
    }

    #[test]
    fn an_image_over_8000_pixels_a_side_is_left_out_and_says_why() {
        let limits = RunLimits::of(&RunSettings::default());
        let sized = |id: &str, w: u32, h: u32| ContentRow {
            width: Some(w),
            height: Some(h),
            ..row(id, 10)
        };
        let rows = vec![
            sized("wide", 8001, 1),
            sized("tall", 1, 8001),
            sized("square", 8000, 8000),
        ];
        let chosen = choose(rows, true, &limits);
        let shown: Vec<&str> = chosen.shown.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(shown, ["square"]);
        assert_eq!(
            chosen.told[0]["why"],
            "an image shown is 8000 pixels a side at the most, and this is 8001x1"
        );
        assert_eq!(
            chosen.told[1]["why"],
            "an image shown is 8000 pixels a side at the most, and this is 1x8001"
        );
    }

    /// An image handle is found in a script's value as it is written
    /// out, and not inside another handle, whatever that handle's media
    /// type: a handle's `meta` is the host's, and nothing in it is shown.
    #[test]
    fn an_image_inside_another_handle_is_not_found() {
        let (_dir, store, payload) = kept_image();
        let handle = payload["images"][0].clone();
        let found_ids = |value: Value| -> Vec<String> {
            found(&store, "r", &value)
                .unwrap()
                .into_iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(found_ids(json!({"pictures": [handle.clone()]})), ["c1"]);
        for outer in ["application/pdf", "image/png"] {
            let wrapped = json!({"id": "c9", "sha256": "0".repeat(64), "media_type": outer,
                                 "meta": {"preview": handle.clone()}});
            assert!(found_ids(wrapped).is_empty(), "{outer}");
        }
    }

    /// A store on a fresh home that keeps one image, `c1`, of the run `r`,
    /// and the payload a script step that showed it keeps.
    fn kept_image() -> (tempfile::TempDir, Store, Value) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        let new = clax_store::bot_content::NewContent {
            id: "c1",
            run_id: "r",
            surface: "lens",
            registration_id: None,
            name: None,
            media_type: "image/png",
            width: Some(2),
            height: Some(1),
            bytes: b"pixels",
        };
        let row = store
            .put_bot_content(&new, clax_store::bot_content::ContentLimits::default())
            .unwrap();
        let chosen = Chosen {
            told: Vec::new(),
            shown: vec![row],
        };
        let payload = json!({"tool_use_id": "tu_1", "images": chosen.kept()});
        (dir, store, payload)
    }

    fn step(n: i64, kind: StepKind, payload: Value) -> Step {
        Step {
            run_id: "r".into(),
            n,
            kind,
            started_at: String::new(),
            attempt: 1,
            committed_at: Some(String::new()),
            payload: Some(payload),
            observed: Vec::new(),
            failures: 0,
        }
    }

    fn turn(n: i64, stop: &str) -> Step {
        step(n, StepKind::Turn, json!({"stop_reason": stop}))
    }

    #[test]
    fn a_steps_images_are_shown_to_the_first_turn_after_it_and_said_shown_after_that() {
        let (_dir, store, payload) = kept_image();
        let script = step(2, StepKind::Script, payload.clone());
        // The tool result for `tu_1` in the history `steps` builds.
        let sent = |steps: &[Step]| {
            let messages = vec![Message::user(vec![Block::tool_result(
                "tu_1",
                "the result",
                false,
            )])];
            let (messages, ids) = show(&store, "r", steps, true, messages).unwrap();
            (messages[0].content[0].to_json(), ids)
        };
        let with_image = |(block, ids): (Value, Vec<String>)| {
            let parts = block["content"].as_array().cloned().unwrap_or_default();
            parts.len() == 2 && parts[1]["type"] == "image" && ids == ["c1"]
        };
        // The turn that answers the step is sent the text and the image.
        assert!(with_image(sent(&[turn(1, "tool_use"), script.clone()])));
        // A run stopped after the step and continued: no turn was sent
        // the image, so the continuation's first turn is.
        let stopped = step(3, StepKind::Stopped, json!({"reason": "cancelled"}));
        assert!(with_image(sent(&[
            turn(1, "tool_use"),
            script.clone(),
            stopped.clone(),
        ])));
        // A turn refused for its length was shown nothing either.
        assert!(with_image(sent(&[
            turn(1, "tool_use"),
            script.clone(),
            stopped,
            turn(4, "prompt_too_long"),
        ])));
        // Once a turn was answered with it, every later turn is sent the
        // text and the note, and no image.
        let (later, ids) = sent(&[turn(1, "tool_use"), script, turn(3, "tool_use")]);
        assert!(ids.is_empty());
        assert_eq!(
            later["content"],
            format!("the result\n\n{IMAGES_SHOWN_ONCE}")
        );
        // A model that takes no images is sent none, and nothing is shown.
        let messages = vec![Message::user(vec![Block::tool_result("tu_1", "t", false)])];
        let steps = [turn(1, "tool_use"), step(2, StepKind::Script, payload)];
        let (_, ids) = show(&store, "r", &steps, false, messages).unwrap();
        assert!(ids.is_empty());
    }

    /// The images of a request body are counted wherever they are, with
    /// the length of their base64 data; a body with none has none.
    #[test]
    fn a_bodys_images_are_counted_with_their_base64_length() {
        let images = [
            ToolImage::new("image/png", vec![0; 3]),
            ToolImage::new("image/jpeg", vec![0; 4]),
        ];
        let body = json!({
            "messages": [
                Message::user(vec![Block::text("hi")]).to_json(),
                Message::user(vec![Block::tool_result_with_images("tu_1", "the result", false, &images, true)]).to_json(),
            ],
        });
        // 3 bytes are 4 base64 digits, 4 bytes are 8.
        assert_eq!(in_body(&body), (2, 12));
        let without = json!({"messages": [Message::user(vec![Block::tool_result("tu_1", "t", false)]).to_json()]});
        assert_eq!(in_body(&without), (0, 0));
    }

    /// Messages a policy gave that left out the step's tool result, or
    /// rewrote it into something other than a tool result with text, are
    /// sent no image, so no image is said sent (and so none is marked
    /// shown when the turn commits).
    #[test]
    fn an_image_whose_tool_result_the_messages_left_out_is_not_said_sent() {
        let (_dir, store, payload) = kept_image();
        let steps = [turn(1, "tool_use"), step(2, StepKind::Script, payload)];
        for messages in [
            vec![Message::user(vec![Block::text("the policy's summary")])],
            vec![Message::user(vec![Block::tool_result(
                "tu_other", "t", false,
            )])],
        ] {
            let (out, ids) = show(&store, "r", &steps, true, messages.clone()).unwrap();
            assert!(ids.is_empty(), "{ids:?}");
            assert_eq!(out, messages);
        }
    }
}

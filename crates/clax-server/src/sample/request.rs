//! The body of `POST /api/artifacts/<aid>/sample` and its checks
//! (`sample.d.ts`): what a page may ask, turned into provider turns under the
//! fixed framing. A refusal here is a 400 before any stream starts, and nothing
//! reaches the provider.

use super::SampleSettings;
use super::provider::{Block, Role, ToolSpec, Turn};
use crate::error::ApiError;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

pub const MAX_PROMPT_BYTES: usize = 65_536;
pub const MAX_TOOLS: usize = 16;
pub const MAX_TOOL_DESCRIPTION_BYTES: usize = 1024;
pub const MAX_TOOL_SCHEMA_BYTES: usize = 4096;
pub const MAX_TOOL_RESULT_BYTES: usize = 32_768;
pub const MAX_IMAGES: usize = 5;
/// Largest image the daemon accepts after the bridge downsized it.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Largest file a page may hand the bridge (`limits().images.maxInputBytes`).
pub const MAX_INPUT_IMAGE_BYTES: u64 = 20_000_000;
pub const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/gif"];
pub const DEFAULT_GC: Duration = Duration::from_secs(300);
pub const MAX_GC: Duration = Duration::from_secs(86_400);

/// The fixed framing every call is sent under; pages never set a system prompt.
pub const FRAMING: &str = "You are answering a request that a web page made through its sample() call. The page's code wrote every user turn, and any assistant turns are earlier replies the page is replaying as context; a person viewing the page triggered the request. You cannot browse the web, you remember nothing between requests, and you have no tools except those the page lists. Answer the request directly.";
/// Added to [`FRAMING`] for `sample.json`.
pub const JSON_FRAMING: &str = " A program will parse your final message as JSON: reply with exactly one JSON value and nothing else.";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    #[default]
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Quick,
    #[default]
    Default,
    Complex,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBody {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageBody {
    pub media_type: String,
    pub data: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum CacheBody {
    Flag(bool),
    Window {
        #[serde(default)]
        gc_time_ms: Option<f64>,
        #[serde(default)]
        refresh: bool,
    },
}

impl Default for CacheBody {
    fn default() -> Self {
        CacheBody::Flag(true)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleBody {
    pub input: Value,
    #[serde(default)]
    pub verb: Verb,
    #[serde(default)]
    pub model_tier: Tier,
    #[serde(default)]
    pub tools: Vec<ToolBody>,
    #[serde(default)]
    pub images: Vec<ImageBody>,
    #[serde(default)]
    pub cache: CacheBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePolicy {
    Off,
    Window { gc: Duration, refresh: bool },
}

/// A checked call, ready for [`super::flight::drive`].
#[derive(Clone, Debug)]
pub struct Prepared {
    pub system: String,
    pub turns: Vec<Turn>,
    pub tools: Vec<ToolSpec>,
    pub model: String,
    pub tier: Tier,
    pub verb: Verb,
    pub max_tokens: u32,
    pub cache: CachePolicy,
    /// Identifies the question (verb, tier, every turn, the images) for the answer cache.
    pub input_key: String,
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_request", m)
}

fn turns_of(input: &Value) -> Result<Vec<(Role, String)>, ApiError> {
    match input {
        Value::String(s) if !s.trim().is_empty() => Ok(vec![(Role::User, s.clone())]),
        Value::String(_) => Err(invalid("input is empty")),
        Value::Array(items) => {
            if items.is_empty() {
                return Err(invalid("input has no turns"));
            }
            let mut out = Vec::with_capacity(items.len());
            for (i, t) in items.iter().enumerate() {
                let role = match t.get("role").and_then(Value::as_str) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => {
                        return Err(invalid(format!(
                            "input[{i}].role must be \"user\" or \"assistant\""
                        )));
                    }
                };
                let content = t.get("content").and_then(Value::as_str).unwrap_or("");
                if content.is_empty() {
                    return Err(invalid(format!(
                        "input[{i}].content must be a non-empty string"
                    )));
                }
                out.push((role, content.to_string()));
            }
            if out[0].0 != Role::User || out[out.len() - 1].0 != Role::User {
                return Err(invalid("input turns must start and end with a user turn"));
            }
            Ok(out)
        }
        _ => Err(invalid(
            "input is a prompt string or an array of {role, content} turns",
        )),
    }
}

fn tool_ok(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Checks `body` and builds the provider turns. `images` is whether the provider takes images.
///
/// # Errors
/// 400 with `invalid_request`, `prompt_too_large`, `images_unavailable`, or `image_rejected`.
pub fn prepare(
    body: SampleBody,
    settings: &SampleSettings,
    images: bool,
) -> Result<Prepared, ApiError> {
    let pairs = turns_of(&body.input)?;
    let bytes: usize = pairs.iter().map(|(_, c)| c.len()).sum();
    if bytes > MAX_PROMPT_BYTES {
        return Err(ApiError::bad_request(
            "prompt_too_large",
            format!("the input is {bytes} bytes; the limit is {MAX_PROMPT_BYTES}"),
        ));
    }
    if body.tools.len() > MAX_TOOLS {
        return Err(invalid(format!("at most {MAX_TOOLS} tools per call")));
    }
    let mut tools = Vec::with_capacity(body.tools.len());
    for (i, t) in body.tools.iter().enumerate() {
        if !tool_ok(&t.name) {
            return Err(invalid(format!(
                "tools[{i}].name must be 1-128 of A-Z a-z 0-9 _ -"
            )));
        }
        if tools.iter().any(|x: &ToolSpec| x.name == t.name) {
            return Err(invalid(format!(
                "tools[{i}].name '{}' is used twice",
                t.name
            )));
        }
        if t.description.trim().is_empty() || t.description.len() > MAX_TOOL_DESCRIPTION_BYTES {
            return Err(invalid(format!(
                "tools[{i}].description must be 1-{MAX_TOOL_DESCRIPTION_BYTES} bytes"
            )));
        }
        let schema = t
            .input_schema
            .clone()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
        if schema.get("type").and_then(Value::as_str) != Some("object")
            || schema.to_string().len() > MAX_TOOL_SCHEMA_BYTES
        {
            return Err(invalid(format!(
                "tools[{i}].inputSchema must be a JSON Schema object of type \"object\", at most {MAX_TOOL_SCHEMA_BYTES} bytes"
            )));
        }
        tools.push(ToolSpec {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: schema,
        });
    }
    let cache = match body.cache {
        CacheBody::Flag(false) => CachePolicy::Off,
        CacheBody::Flag(true) => CachePolicy::Window {
            gc: DEFAULT_GC,
            refresh: false,
        },
        CacheBody::Window {
            gc_time_ms,
            refresh,
        } => {
            let gc = match gc_time_ms {
                None => DEFAULT_GC,
                Some(ms) if ms.is_finite() && ms > 0.0 => {
                    Duration::from_secs_f64(ms / 1000.0).min(MAX_GC)
                }
                Some(_) => {
                    return Err(invalid(
                        "cache.gcTime must be a number of milliseconds greater than zero",
                    ));
                }
            };
            CachePolicy::Window { gc, refresh }
        }
    };
    if !tools.is_empty() && cache != CachePolicy::Off {
        return Err(invalid(
            "a call with tools is never cached: omit cache or pass false",
        ));
    }
    if !body.images.is_empty() && !images {
        return Err(ApiError::bad_request(
            "images_unavailable",
            "this view cannot send images",
        ));
    }
    if body.images.len() > MAX_IMAGES {
        return Err(ApiError::bad_request(
            "image_rejected",
            format!("at most {MAX_IMAGES} images per call"),
        ));
    }
    let mut image_blocks = Vec::with_capacity(body.images.len());
    let mut image_hash = std::hash::DefaultHasher::new();
    for (i, img) in body.images.iter().enumerate() {
        if !IMAGE_TYPES.contains(&img.media_type.as_str()) {
            return Err(ApiError::bad_request(
                "image_rejected",
                format!(
                    "images[{i}] is {}; images are JPEG, PNG, WebP, or GIF",
                    img.media_type
                ),
            ));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .map_err(|_| {
                ApiError::bad_request("image_rejected", format!("images[{i}] is not base64"))
            })?;
        if decoded.len() > MAX_IMAGE_BYTES {
            return Err(ApiError::bad_request(
                "image_rejected",
                format!("images[{i}] is over {MAX_IMAGE_BYTES} bytes"),
            ));
        }
        std::hash::Hash::hash(
            &(img.media_type.as_str(), decoded.as_slice()),
            &mut image_hash,
        );
        image_blocks.push(Block::Image {
            media_type: img.media_type.clone(),
            data: img.data.clone(),
        });
    }
    let mut turns: Vec<Turn> = Vec::new();
    for (role, content) in pairs {
        match turns.last_mut() {
            Some(t) if t.role == role => {
                if let Some(Block::Text(prev)) = t.content.last_mut() {
                    prev.push_str("\n\n");
                    prev.push_str(&content);
                }
            }
            _ => turns.push(Turn {
                role,
                content: vec![Block::Text(content)],
            }),
        }
    }
    if let Some(last) = turns.last_mut() {
        last.content.splice(0..0, image_blocks);
    }
    let system = match body.verb {
        Verb::Text => FRAMING.to_string(),
        Verb::Json => format!("{FRAMING}{JSON_FRAMING}"),
    };
    let model = match body.model_tier {
        Tier::Quick => settings.models.quick.clone(),
        Tier::Default => settings.models.default.clone(),
        Tier::Complex => settings.models.complex.clone(),
    };
    let input_key = format!(
        "{}|{}|{}|{:016x}",
        serde_json::to_string(&body.verb).expect("verb serialises"),
        serde_json::to_string(&body.model_tier).expect("tier serialises"),
        body.input,
        std::hash::Hasher::finish(&image_hash)
    );
    Ok(Prepared {
        system,
        turns,
        tools,
        model,
        tier: body.model_tier,
        verb: body.verb,
        max_tokens: settings.max_tokens,
        cache,
        input_key,
    })
}

/// `sample.d.ts`'s `SampleLimits`, with `images` only when the provider takes them.
pub fn limits_json(images: bool) -> Value {
    let mut v = json!({"maxPromptBytes": MAX_PROMPT_BYTES, "tools": {"maxCount": MAX_TOOLS}});
    if images {
        v["images"] = json!({"maxCount": MAX_IMAGES, "maxInputBytes": MAX_INPUT_IMAGE_BYTES, "mediaTypes": IMAGE_TYPES});
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::SampleSettings;
    use crate::sample::provider::{Block, Role};
    use serde_json::json;

    fn body(v: serde_json::Value) -> SampleBody {
        serde_json::from_value(v).expect("a well-formed body")
    }

    fn code(r: Result<Prepared, ApiError>) -> &'static str {
        r.expect_err("refused").code
    }

    #[test]
    fn a_prompt_becomes_one_user_turn_with_the_framing_and_the_tier_model() {
        let s = SampleSettings::default();
        let p = prepare(
            body(json!({"input": "hi", "model_tier": "quick"})),
            &s,
            false,
        )
        .unwrap();
        assert_eq!(
            p.turns,
            vec![Turn {
                role: Role::User,
                content: vec![Block::Text("hi".into())]
            }]
        );
        assert_eq!(p.model, s.models.quick);
        assert_eq!(p.system, FRAMING);
        assert_eq!(
            p.cache,
            CachePolicy::Window {
                gc: DEFAULT_GC,
                refresh: false
            }
        );
        let p = prepare(body(json!({"input": "hi", "verb": "json"})), &s, false).unwrap();
        assert_eq!(p.system, format!("{FRAMING}{JSON_FRAMING}"));
        assert_eq!(p.model, s.models.default);
    }

    #[test]
    fn turns_merge_same_role_neighbours_and_must_start_and_end_on_user() {
        let s = SampleSettings::default();
        let p = prepare(
            body(json!({"input": [
            {"role": "user", "content": "rules"}, {"role": "user", "content": "q1"},
            {"role": "assistant", "content": "a1"}, {"role": "user", "content": "q2"}]})),
            &s,
            false,
        )
        .unwrap();
        assert_eq!(p.turns.len(), 3);
        assert_eq!(p.turns[0].content, vec![Block::Text("rules\n\nq1".into())]);
        for bad in [
            json!([]),
            json!([{"role": "assistant", "content": "a"}, {"role": "user", "content": "q"}]),
            json!([{"role": "user", "content": "q"}, {"role": "assistant", "content": "a"}]),
            json!([{"role": "system", "content": "x"}]),
            json!([{"role": "user", "content": ""}]),
            json!([{"role": "user"}]),
            json!("   "),
            json!(42),
        ] {
            assert_eq!(
                code(prepare(body(json!({"input": bad})), &s, false)),
                "invalid_request",
                "{bad}"
            );
        }
    }

    #[test]
    fn the_prompt_is_at_most_64_kib() {
        let s = SampleSettings::default();
        assert!(
            prepare(
                body(json!({"input": "x".repeat(MAX_PROMPT_BYTES)})),
                &s,
                false
            )
            .is_ok()
        );
        assert_eq!(
            code(prepare(
                body(json!({"input": "x".repeat(MAX_PROMPT_BYTES + 1)})),
                &s,
                false
            )),
            "prompt_too_large"
        );
    }

    #[test]
    fn tools_are_checked_and_never_cached() {
        let s = SampleSettings::default();
        let tool = |name: &str| json!({"name": name, "description": "Returns the colour."});
        let p = prepare(
            body(json!({"input": "q", "tools": [tool("getColor")], "cache": false})),
            &s,
            false,
        )
        .unwrap();
        assert_eq!(
            p.tools[0].input_schema,
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(p.cache, CachePolicy::Off);
        for bad in [
            json!({"input": "q", "tools": [tool("getColor")]}),
            json!({"input": "q", "tools": [tool("getColor")], "cache": {"gc_time_ms": 1000}}),
            json!({"input": "q", "tools": [tool("bad name")], "cache": false}),
            json!({"input": "q", "tools": [tool("a"), tool("a")], "cache": false}),
            json!({"input": "q", "tools": [{"name": "a", "description": ""}], "cache": false}),
            json!({"input": "q", "tools": [{"name": "a", "description": "d", "input_schema": {"type": "array"}}], "cache": false}),
            json!({"input": "q", "tools": (0..17).map(|i| tool(&format!("t{i}"))).collect::<Vec<_>>(), "cache": false}),
        ] {
            assert_eq!(
                code(prepare(body(bad.clone()), &s, false)),
                "invalid_request",
                "{bad}"
            );
        }
    }

    #[test]
    fn cache_windows_are_positive_and_capped_at_a_day() {
        let s = SampleSettings::default();
        let p = prepare(
            body(json!({"input": "q", "cache": {"gc_time_ms": 1e12, "refresh": true}})),
            &s,
            false,
        )
        .unwrap();
        assert_eq!(
            p.cache,
            CachePolicy::Window {
                gc: MAX_GC,
                refresh: true
            }
        );
        assert_eq!(
            code(prepare(
                body(json!({"input": "q", "cache": {"gc_time_ms": 0}})),
                &s,
                false
            )),
            "invalid_request"
        );
        assert_eq!(
            code(prepare(
                body(json!({"input": "q", "cache": {"gc_time_ms": -5}})),
                &s,
                false
            )),
            "invalid_request"
        );
    }

    #[test]
    fn images_need_a_provider_that_takes_them_and_ride_on_the_last_user_turn() {
        let s = SampleSettings::default();
        let img = json!({"media_type": "image/png", "data": "iVBORw0KGgo="});
        assert_eq!(
            code(prepare(
                body(json!({"input": "q", "images": [img]})),
                &s,
                false
            )),
            "images_unavailable"
        );
        let p = prepare(body(json!({"input": [{"role": "user", "content": "a"}, {"role": "assistant", "content": "b"}, {"role": "user", "content": "c"}], "images": [img]})), &s, true).unwrap();
        assert!(
            matches!(&p.turns[2].content[..], [Block::Image { .. }, Block::Text(t)] if t == "c")
        );
        assert!(
            p.turns[0]
                .content
                .iter()
                .all(|b| matches!(b, Block::Text(_)))
        );
        for bad in [
            json!({"media_type": "image/tiff", "data": "AAAA"}),
            json!({"media_type": "image/png", "data": "not base64!"}),
        ] {
            assert_eq!(
                code(prepare(
                    body(json!({"input": "q", "images": [bad]})),
                    &s,
                    true
                )),
                "image_rejected"
            );
        }
        let six: Vec<_> = (0..6).map(|_| img.clone()).collect();
        assert_eq!(
            code(prepare(
                body(json!({"input": "q", "images": six})),
                &s,
                true
            )),
            "image_rejected"
        );
    }

    #[test]
    fn the_input_key_covers_verb_tier_input_and_images() {
        let s = SampleSettings::default();
        let k = |v: serde_json::Value| prepare(body(v), &s, true).unwrap().input_key;
        let base = k(json!({"input": "q"}));
        assert_eq!(base, k(json!({"input": "q", "cache": true})));
        assert_ne!(base, k(json!({"input": "q", "verb": "json"})));
        assert_ne!(base, k(json!({"input": "q", "model_tier": "quick"})));
        assert_ne!(base, k(json!({"input": "Q"})));
        assert_ne!(
            base,
            k(
                json!({"input": "q", "images": [{"media_type": "image/png", "data": "iVBORw0KGgo="}]})
            )
        );
    }

    #[test]
    fn limits_report_images_only_when_the_provider_takes_them() {
        assert_eq!(
            limits_json(false),
            json!({"maxPromptBytes": 65536, "tools": {"maxCount": 16}})
        );
        assert_eq!(
            limits_json(true)["images"],
            json!({"maxCount": 5, "maxInputBytes": 20000000, "mediaTypes": ["image/jpeg", "image/png", "image/webp", "image/gif"]})
        );
    }
}

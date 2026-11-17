//! Google's Gemini API: a [`MessagesRequest`] as a `POST
//! {base_url}/models/{model}:streamGenerateContent?alt=sse` body ([`body`]),
//! and its stream of `GenerateContentResponse`s as the Messages API's events
//! ([`Translator`]).
//!
//! # Request
//!
//! The system blocks become `systemInstruction`; turns become `contents`
//! (`user` and `model`, consecutive turns of one role merged); a
//! `tool_use` becomes a `functionCall` part and its `tool_result` a
//! `functionResponse` part naming the same function. The images of a
//! turn's tool results (the base64 `image` blocks of their content) follow
//! the turn's other parts, in the same `user` content: for each result
//! with images a `text` part `Images of the result of <function>, call
//! <ID>:` (`Images of result <n> of this turn (<function>):` for a call
//! whose ID was made here, by its place among the turn's results) and
//! then one `inlineData` part (`mimeType`, `data`) per image, in order.
//! They are not put inside the `functionResponse` itself. A turn whose results have no image is
//! sent as it was before results could have any. Cache breakpoints, betas
//! and [`MessagesRequest::extra`] are not sent. Thinking asks for
//! thought summaries (`includeThoughts`); `effort` is sent as
//! `thinkingLevel` (`low`, `medium`, else `high`).
//!
//! # Thought signatures
//!
//! Gemini signs its reasoning on whichever part it likes
//! (`thoughtSignature`) and wants every signature back on the part it came
//! on. A thought part becomes a thinking block whose `signature` is the
//! part's; a signature on a text or `functionCall` part is kept in a
//! `signature` field of its block (so the block is [`Block::Raw`]); each is
//! sent back on its part.
//!
//! # Stream
//!
//! Each chunk carries the parts produced since the last (text grows the open
//! text or thinking block; a `functionCall` arrives whole) and the running
//! `usageMetadata` (`promptTokenCount` counting cached tokens;
//! `thoughtsTokenCount` billed as output). `finishReason` closes the blocks;
//! the stream's end, after it, ends the message.

use super::{Answer, classify, images_of, protocol};
use super::{Block, MessagesError, MessagesRequest, ModelInfo, Role, ToolChoice};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// The prefix of a call ID made here for a `functionCall` that came without
/// one: never sent back, since Gemini did not make it.
const MADE_ID: &str = "gemini_call_";

/// The path of a call to `model` under the base URL.
pub(super) fn path(model: &str) -> String {
    let model = model.strip_prefix("models/").unwrap_or(model);
    format!("/models/{model}:streamGenerateContent?alt=sse")
}

fn thinking_level(effort: &str) -> &'static str {
    match effort {
        "low" => "low",
        "medium" => "medium",
        _ => "high",
    }
}

/// The text of a `tool_result`'s content: a string, or its text blocks.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// `part` with the `signature` `v` carries, when any.
fn signed(mut part: Value, v: &Value) -> Value {
    if let Some(s) = v["signature"].as_str().filter(|s| !s.is_empty()) {
        part["thoughtSignature"] = json!(s);
    }
    part
}

/// The parts of `content`, a turn of `role`; `names` maps each call ID seen
/// so far to its function's name, which a `functionResponse` names again.
fn parts(role: Role, content: &[Block], names: &mut HashMap<String, String>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut images = Vec::new();
    // The tool results of the turn so far.
    let mut results = 0usize;
    for b in content {
        let v = b.to_json();
        match v["type"].as_str() {
            Some("text") => {
                let text = v["text"].as_str().unwrap_or("");
                if role == Role::Assistant {
                    out.push(signed(json!({"text": text}), &v));
                } else if !text.is_empty() {
                    out.push(json!({"text": text}));
                }
            }
            Some("thinking") if role == Role::Assistant => {
                out.push(signed(
                    json!({"text": v["thinking"].as_str().unwrap_or(""), "thought": true}),
                    &v,
                ));
            }
            Some("tool_use") if role == Role::Assistant => {
                let id = v["id"].as_str().unwrap_or("");
                let name = v["name"].as_str().unwrap_or("");
                names.insert(id.to_string(), name.to_string());
                let mut call = json!({"name": name, "args": v["input"]});
                if !id.starts_with(MADE_ID) {
                    call["id"] = json!(id);
                }
                out.push(signed(json!({"functionCall": call}), &v));
            }
            Some("tool_result") if role != Role::Assistant => {
                let id = v["tool_use_id"].as_str().unwrap_or("");
                let text = text_of(&v["content"]);
                let response = if v["is_error"] == true {
                    json!({"error": text})
                } else {
                    json!({"result": text})
                };
                let name = names.get(id).map_or("", String::as_str);
                let mut r = json!({"name": name, "response": response});
                if !id.starts_with(MADE_ID) {
                    r["id"] = json!(id);
                }
                out.push(json!({"functionResponse": r}));
                results += 1;
                let shown = images_of(&v["content"]);
                if !shown.is_empty() {
                    // Named by the call's ID, which tells two calls of one
                    // function apart; a call whose ID was made here (and is
                    // not sent) by its place among the turn's results.
                    let label = if id.starts_with(MADE_ID) {
                        format!("Images of result {results} of this turn ({name}):")
                    } else {
                        format!("Images of the result of {name}, call {id}:")
                    };
                    images.push(json!({"text": label}));
                }
                for (media_type, data) in shown {
                    images.push(json!({"inlineData": {"mimeType": media_type, "data": data}}));
                }
            }
            _ => {}
        }
    }
    out.extend(images);
    out
}

/// The streaming request body of `req`.
/// The `generateContent` body of `req`, each tool's schema its
/// [`super::Tool::input_schema`].
#[cfg(test)]
pub(super) fn body(req: &MessagesRequest) -> Value {
    body_marked(req, false)
}

/// The `generateContent` body of `req`; with `marked`, a tool's schema
/// that has a text is its mark, for [`super::spliced`]. Gemini takes the
/// schema as JSON Schema (`parametersJsonSchema`), as it is: nothing of
/// it is rewritten, so its members keep their order.
pub(super) fn body_marked(req: &MessagesRequest, marked: bool) -> Value {
    let mut contents: Vec<Value> = Vec::new();
    let mut names = HashMap::new();
    for m in &req.messages {
        let role = if m.role == Role::Assistant {
            "model"
        } else {
            "user"
        };
        let p = parts(m.role, &m.content, &mut names);
        if p.is_empty() {
            continue;
        }
        match contents.last_mut() {
            Some(last) if last["role"] == role => {
                if let Some(a) = last["parts"].as_array_mut() {
                    a.extend(p);
                }
            }
            _ => contents.push(json!({"role": role, "parts": p})),
        }
    }
    let mut b = Map::new();
    if !req.system.is_empty() {
        let system: Vec<Value> = req.system.iter().map(|s| json!({"text": s.text})).collect();
        b.insert("systemInstruction".into(), json!({"parts": system}));
    }
    b.insert("contents".into(), Value::Array(contents));
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .enumerate()
            .map(|(n, t)| {
                let schema = super::schema_of(&req.tools, n, marked);
                json!({"name": t.name, "description": t.description,
                       "parametersJsonSchema": schema})
            })
            .collect();
        b.insert("tools".into(), json!([{"functionDeclarations": decls}]));
    }
    match &req.tool_choice {
        Some(ToolChoice::Auto { .. }) => {
            b.insert(
                "toolConfig".into(),
                json!({"functionCallingConfig": {"mode": "AUTO"}}),
            );
        }
        Some(ToolChoice::None) => {
            b.insert(
                "toolConfig".into(),
                json!({"functionCallingConfig": {"mode": "NONE"}}),
            );
        }
        None => {}
    }
    let mut config = json!({"maxOutputTokens": req.max_tokens});
    if req.thinking.is_some() || req.effort.is_some() {
        let mut t = Map::new();
        if req.thinking.is_some() {
            t.insert("includeThoughts".into(), json!(true));
        }
        if let Some(e) = &req.effort {
            t.insert("thinkingLevel".into(), json!(thinking_level(e)));
        }
        config["thinkingConfig"] = Value::Object(t);
    }
    b.insert("generationConfig".into(), config);
    Value::Object(b)
}

/// The models one page of `GET {base_url}/models` lists that can generate
/// content, without their `models/` prefix.
pub(super) fn models(v: &Value) -> Result<Vec<ModelInfo>, MessagesError> {
    let Some(list) = v["models"].as_array() else {
        // An empty page has no `models` at all.
        return if v.is_object() {
            Ok(Vec::new())
        } else {
            Err(protocol("a model list that is not an object"))
        };
    };
    Ok(list
        .iter()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_none_or(|a| a.iter().any(|x| x == "generateContent"))
        })
        .filter_map(|m| {
            let name = m["name"].as_str()?;
            let id = name.strip_prefix("models/").unwrap_or(name);
            (!id.is_empty()).then(|| ModelInfo {
                id: id.to_string(),
                display_name: m["displayName"].as_str().unwrap_or(id).to_string(),
                created_at: String::new(),
            })
        })
        .collect())
}

/// What kind of block is open for text to grow.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Text(usize),
    Thinking(usize),
}

/// Turns a `streamGenerateContent` stream into the Messages API's events.
#[derive(Default)]
pub(super) struct Translator {
    started: bool,
    next_index: usize,
    open: Option<Open>,
    calls: usize,
    finished: bool,
    usage_seen: bool,
    done: bool,
}

impl Translator {
    fn start_block(&mut self, out: &mut Vec<Value>, block: Value) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        out.push(json!({"type": "content_block_start", "index": index, "content_block": block}));
        index
    }

    fn close(&mut self, out: &mut Vec<Value>) {
        if let Some(Open::Text(i) | Open::Thinking(i)) = self.open.take() {
            out.push(json!({"type": "content_block_stop", "index": i}));
        }
    }

    fn sign(out: &mut Vec<Value>, index: usize, sig: &str) {
        out.push(json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "signature_delta", "signature": sig}}));
    }

    /// `text` (and its part's signature) into the open block of its kind.
    fn text(&mut self, out: &mut Vec<Value>, thought: bool, text: &str, sig: Option<&str>) {
        let index = match (self.open, thought) {
            (Some(Open::Thinking(i)), true) | (Some(Open::Text(i)), false) => i,
            _ => {
                self.close(out);
                if thought {
                    let i = self.start_block(
                        out,
                        json!({"type": "thinking", "thinking": "", "signature": ""}),
                    );
                    self.open = Some(Open::Thinking(i));
                    i
                } else {
                    let mut block = json!({"type": "text", "text": ""});
                    if sig.is_some() {
                        // Its signature arrives as a delta below.
                        block["signature"] = json!("");
                    }
                    let i = self.start_block(out, block);
                    self.open = Some(Open::Text(i));
                    i
                }
            }
        };
        if !text.is_empty() {
            let delta = if thought {
                json!({"type": "thinking_delta", "thinking": text})
            } else {
                json!({"type": "text_delta", "text": text})
            };
            out.push(json!({"type": "content_block_delta", "index": index, "delta": delta}));
        }
        if let Some(s) = sig {
            Self::sign(out, index, s);
        }
    }

    fn call(&mut self, out: &mut Vec<Value>, call: &Value, sig: Option<&str>) {
        self.close(out);
        self.calls += 1;
        let id = match call["id"].as_str() {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("{MADE_ID}{}", self.next_index),
        };
        let mut block = json!({"type": "tool_use", "id": id,
            "name": call["name"].as_str().unwrap_or(""), "input": {}});
        if let Some(s) = sig {
            block["signature"] = json!(s);
        }
        let index = self.start_block(out, block);
        let args = if call["args"].is_object() {
            call["args"].to_string()
        } else {
            "{}".into()
        };
        out.push(json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "input_json_delta", "partial_json": args}}));
        out.push(json!({"type": "content_block_stop", "index": index}));
    }

    fn stop_reason(&self, r: &str) -> String {
        match r {
            "STOP" if self.calls > 0 => "tool_use".into(),
            "STOP" => "end_turn".into(),
            "MAX_TOKENS" => "max_tokens".into(),
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
            | "IMAGE_SAFETY" | "LANGUAGE" => "refusal".into(),
            other => other.to_ascii_lowercase(),
        }
    }

    /// The Messages API's events for one `data:` of the stream.
    pub(super) fn apply(&mut self, data: &str) -> Result<Vec<Value>, MessagesError> {
        let mut out = Vec::new();
        if self.done {
            return Ok(out);
        }
        let v: Value = serde_json::from_str(data)
            .map_err(|e| protocol(format!("a chunk that is not JSON: {e}")))?;
        if v["error"].is_object() {
            let e = &v["error"];
            let status = e["code"]
                .as_u64()
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(0);
            let said = e["message"].as_str().map(str::to_string);
            let message = said
                .clone()
                .unwrap_or_else(|| "the provider reported an error".into());
            return Err(classify(Answer::new(status, "", said), message, None));
        }
        if !self.started {
            self.started = true;
            out.push(json!({"type": "message_start", "message": {
                "id": v["responseId"].as_str().unwrap_or(""),
                "model": v["modelVersion"].as_str().unwrap_or(""),
                "usage": {},
            }}));
        }
        let candidate = v["candidates"].as_array().and_then(|c| c.first());
        if let Some(c) = candidate {
            for p in c["content"]["parts"].as_array().into_iter().flatten() {
                let sig = p["thoughtSignature"].as_str().filter(|s| !s.is_empty());
                if p["functionCall"].is_object() {
                    self.call(&mut out, &p["functionCall"], sig);
                } else if let Some(t) = p["text"].as_str() {
                    self.text(&mut out, p["thought"] == true, t, sig);
                } else if let Some(s) = sig {
                    // A signature alone signs the open block, or a text
                    // block of its own.
                    match self.open {
                        Some(Open::Text(i) | Open::Thinking(i)) => Self::sign(&mut out, i, s),
                        None => self.text(&mut out, false, "", Some(s)),
                    }
                }
            }
        }
        if let Some(m) = v["usageMetadata"].as_object() {
            let n = |k: &str| m.get(k).and_then(Value::as_u64).unwrap_or(0);
            let prompt = n("promptTokenCount");
            let cached = n("cachedContentTokenCount").min(prompt);
            self.usage_seen = true;
            out.push(json!({"type": "message_delta", "delta": {}, "usage": {
                "input_tokens": prompt - cached,
                "cache_read_input_tokens": cached,
                "cache_creation_input_tokens": 0,
                "output_tokens": n("candidatesTokenCount") + n("thoughtsTokenCount"),
            }}));
        }
        let reason = candidate
            .and_then(|c| c["finishReason"].as_str())
            .map(str::to_string)
            .or_else(|| {
                // A refused prompt has no candidates, only the reason.
                v["promptFeedback"]["blockReason"]
                    .as_str()
                    .map(|_| "SAFETY".to_string())
            });
        if let Some(r) = reason
            && !self.finished
        {
            self.close(&mut out);
            self.finished = true;
            out.push(json!({"type": "message_delta",
                "delta": {"stop_reason": self.stop_reason(&r)}, "usage": {}}));
        }
        Ok(out)
    }

    /// The events the stream's end adds: the message's end, once a chunk
    /// carried its finish reason and usage.
    pub(super) fn end(&mut self) -> Result<Vec<Value>, MessagesError> {
        if self.done || !self.finished {
            // Not finished: the assembler reports the cut.
            return Ok(Vec::new());
        }
        if !self.usage_seen {
            return Err(protocol("the stream carried no token usage"));
        }
        self.done = true;
        Ok(vec![json!({"type": "message_stop"})])
    }
}

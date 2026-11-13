//! OpenAI's Chat Completions API, as Mistral, Subconscious and other hosts
//! serve it: a [`MessagesRequest`] as a streaming `POST
//! {base_url}/chat/completions` body ([`body`]), and its `chat.completion.chunk`
//! stream as the Messages API's events ([`Translator`]).
//!
//! # Request
//!
//! The system blocks become one `system` message; a user turn's
//! `tool_result`s become `tool` messages ahead of its text; an assistant
//! turn's `tool_use`s become its `tool_calls`. A `tool` message holds text
//! alone, so the images of a turn's tool results (the base64 `image`
//! blocks of their content) go in one `user` message after the turn's last
//! `tool` message, so that the answers to one turn's calls stay together:
//! its content is an array of parts, for each result with images a `text`
//! part `Images of the result of tool call <ID>:` and then one `image_url`
//! part per image, in order, whose URL is the `data:` URL of the image;
//! a user turn's own text is the last `text` part of that message, and a
//! mid-conversation system turn's text stays a `system` message after it.
//! A turn whose results have no image is sent as it was before results
//! could have any. Thinking is not sent back (the API has no field for
//! it), nor are cache breakpoints, betas or [`MessagesRequest::extra`].
//! `effort` is sent as `reasoning_effort` (`max` as `xhigh`); adaptive
//! thinking itself has no counterpart.
//!
//! # Stream
//!
//! Content streams into text blocks; `reasoning_content` (or `reasoning`),
//! a `thinking` content part (Mistral) and text inside a leading
//! `<think>…</think>` (models that reason inline) into thinking blocks with
//! an empty signature; each tool call into a `tool_use` block whose
//! arguments stream as `input_json_delta`s. `finish_reason` closes the
//! blocks; `usage` (`prompt_tokens` counting the cached and, on OpenRouter,
//! the cache-written ones, `prompt_tokens_details.cache_write_tokens`) sets
//! the usage, and OpenRouter's `cost` the billed cost ([`billed_micro_usd`]);
//! `data: [DONE]` ends the message, which must have carried its usage.

use super::{Block, MessagesError, MessagesRequest, ModelInfo, Role, ToolChoice};
use super::{classify, images_of, protocol};
use serde_json::{Map, Value, json};

/// The path of a call under the base URL.
pub(super) const PATH: &str = "/chat/completions";

/// Hosts that refuse `stream_options` (an unknown field is a 422 there) and
/// send the usage in the last chunk unasked.
const USAGE_UNASKED: &[&str] = &["api.mistral.ai"];

/// `effort` as `reasoning_effort`.
fn reasoning_effort(effort: &str) -> &str {
    match effort {
        "max" => "xhigh",
        e => e,
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

/// What a call billed, in micro-USD, from OpenRouter's `usage`: its `cost`
/// (US dollars), plus `cost_details.upstream_inference_cost` when
/// `is_byok` is true (`cost` is then only OpenRouter's fee). `None` unless
/// each figure needed is a finite non-negative number.
fn billed_micro_usd(u: &Value) -> Option<u64> {
    let dollars = |v: &Value| v.as_f64().filter(|d| d.is_finite() && *d >= 0.0);
    let mut total = dollars(&u["cost"])?;
    if u["is_byok"] == true {
        total += dollars(&u["cost_details"]["upstream_inference_cost"])?;
    }
    Some(micro_usd_up(total))
}

/// US dollars as micro-USD, rounded up so that only zero is 0; a nonzero
/// product within 10⁻⁶ of a whole number (binary floating-point error) is
/// that number. `as` saturates past `u64::MAX`, beyond any real bill.
fn micro_usd_up(dollars: f64) -> u64 {
    let micro = dollars * 1_000_000.0;
    let near = micro.round();
    if near > 0.0 && (micro - near).abs() < 1e-6 {
        near as u64
    } else {
        micro.ceil() as u64
    }
}

/// The content parts that show the images of the result of tool call
/// `id`: none when `content` has no image.
fn image_parts(out: &mut Vec<Value>, id: &str, content: &Value) {
    let images = images_of(content);
    if images.is_empty() {
        return;
    }
    out.push(json!({"type": "text", "text": format!("Images of the result of tool call {id}:")}));
    for (media_type, data) in images {
        out.push(json!({
            "type": "image_url",
            "image_url": {"url": format!("data:{media_type};base64,{data}")},
        }));
    }
}

/// The message(s) `m` is in Chat Completions.
fn push_message(out: &mut Vec<Value>, role: Role, content: &[Block]) {
    let mut text: Vec<String> = Vec::new();
    match role {
        Role::User | Role::System => {
            let mut images: Vec<Value> = Vec::new();
            for b in content {
                match (b, b.to_json()) {
                    (
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            ..
                        },
                        _,
                    ) => {
                        out.push(tool_message(tool_use_id, content, *is_error));
                        image_parts(&mut images, tool_use_id, content);
                    }
                    (_, v) if v["type"] == "text" => {
                        text.push(v["text"].as_str().unwrap_or("").to_string());
                    }
                    (_, v) if v["type"] == "tool_result" => {
                        let id = v["tool_use_id"].as_str().unwrap_or("");
                        out.push(tool_message(id, &v["content"], v["is_error"] == true));
                        image_parts(&mut images, id, &v["content"]);
                    }
                    _ => {}
                }
            }
            if !images.is_empty() {
                if role == Role::User && !text.is_empty() {
                    images.push(json!({"type": "text", "text": text.join("\n\n")}));
                    text.clear();
                }
                out.push(json!({"role": "user", "content": images}));
            }
            if !text.is_empty() {
                let role = if role == Role::System {
                    "system"
                } else {
                    "user"
                };
                out.push(json!({"role": role, "content": text.join("\n\n")}));
            }
        }
        Role::Assistant => {
            let mut calls = Vec::new();
            for b in content {
                let v = b.to_json();
                match v["type"].as_str() {
                    Some("text") => text.push(v["text"].as_str().unwrap_or("").to_string()),
                    Some("tool_use") => calls.push(json!({
                        "id": v["id"],
                        "type": "function",
                        "function": {"name": v["name"], "arguments": v["input"].to_string()},
                    })),
                    _ => {}
                }
            }
            let mut m = json!({"role": "assistant", "content": text.join("")});
            if !calls.is_empty() {
                if text.is_empty() {
                    m["content"] = Value::Null;
                }
                m["tool_calls"] = Value::Array(calls);
            }
            out.push(m);
        }
    }
}

fn tool_message(id: &str, content: &Value, is_error: bool) -> Value {
    let text = text_of(content);
    let text = if is_error {
        format!("Error: {text}")
    } else {
        text
    };
    json!({"role": "tool", "tool_call_id": id, "content": text})
}

/// The streaming request body of `req` for the host at `base_url`.
pub(super) fn body(req: &MessagesRequest, base_url: &str) -> Value {
    let mut messages = Vec::new();
    if !req.system.is_empty() {
        let system: Vec<&str> = req.system.iter().map(|s| s.text.as_str()).collect();
        messages.push(json!({"role": "system", "content": system.join("\n\n")}));
    }
    for m in &req.messages {
        push_message(&mut messages, m.role, &m.content);
    }
    let mut b = Map::new();
    b.insert("model".into(), json!(req.model));
    b.insert("messages".into(), Value::Array(messages));
    b.insert("max_tokens".into(), json!(req.max_tokens));
    b.insert("stream".into(), json!(true));
    let host = clax_core::config::url_host(base_url);
    if !USAGE_UNASKED.contains(&host.as_str()) {
        b.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.input_schema,
                }})
            })
            .collect();
        b.insert("tools".into(), Value::Array(tools));
    }
    match &req.tool_choice {
        Some(ToolChoice::Auto {
            disable_parallel_tool_use,
        }) => {
            b.insert("tool_choice".into(), json!("auto"));
            if *disable_parallel_tool_use && !req.tools.is_empty() {
                b.insert("parallel_tool_calls".into(), json!(false));
            }
        }
        Some(ToolChoice::None) => {
            b.insert("tool_choice".into(), json!("none"));
        }
        None => {}
    }
    if let Some(e) = &req.effort {
        b.insert("reasoning_effort".into(), json!(reasoning_effort(e)));
    }
    Value::Object(b)
}

/// The models a `GET {base_url}/models` answer lists.
pub(super) fn models(v: &Value) -> Result<Vec<ModelInfo>, MessagesError> {
    let data = v["data"]
        .as_array()
        .ok_or_else(|| protocol("a model list without data"))?;
    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str().filter(|s| !s.is_empty())?;
            Some(ModelInfo {
                id: id.to_string(),
                display_name: m["name"].as_str().unwrap_or(id).to_string(),
                created_at: String::new(),
            })
        })
        .collect())
}

/// Splits streamed content at `<think>` and `</think>`, holding back a tail
/// that may be the start of a tag. Only a `<think>` before any other text
/// opens thinking, so a model that writes the tag in its answer keeps it.
#[derive(Default)]
struct ThinkTags {
    inside: bool,
    /// Text other than whitespace was seen outside thinking.
    answered: bool,
    held: String,
}

impl ThinkTags {
    /// `piece` as (thinking?, text) runs.
    fn split(&mut self, piece: &str) -> Vec<(bool, String)> {
        let mut buf = std::mem::take(&mut self.held) + piece;
        let mut out = Vec::new();
        loop {
            let tag = if self.inside { "</think>" } else { "<think>" };
            if !self.inside && self.answered {
                out.push((false, buf));
                return out;
            }
            if let Some(i) = buf.find(tag) {
                let before = buf[..i].to_string();
                if !self.inside && !before.trim().is_empty() {
                    // Text came first: the tag is the answer's.
                    self.answered = true;
                    continue;
                }
                if !before.is_empty() && self.inside {
                    out.push((true, before));
                }
                buf = buf[i + tag.len()..].to_string();
                self.inside = !self.inside;
                continue;
            }
            let keep = (1..tag.len())
                .rev()
                .find(|k| {
                    buf.len() >= *k
                        && buf.is_char_boundary(buf.len() - k)
                        && tag.starts_with(&buf[buf.len() - k..])
                })
                .unwrap_or(0);
            let (now, later) = buf.split_at(buf.len() - keep);
            if !self.inside && !now.trim().is_empty() {
                self.answered = true;
            }
            if !now.is_empty() {
                out.push((self.inside, now.to_string()));
            }
            self.held = later.to_string();
            return out;
        }
    }

    /// What is held back, at the end of the content.
    fn flush(&mut self) -> Option<(bool, String)> {
        let held = std::mem::take(&mut self.held);
        (!held.is_empty()).then_some((self.inside, held))
    }
}

/// Turns a Chat Completions stream into the Messages API's events.
#[derive(Default)]
pub(super) struct Translator {
    started: bool,
    next_index: usize,
    /// The open text or thinking block: its index and whether it thinks.
    open_text: Option<(usize, bool)>,
    /// Each tool call's stream index and its block's index.
    tools: Vec<(u64, usize)>,
    /// Blocks started and not stopped, in order.
    open: Vec<usize>,
    think: ThinkTags,
    finished: bool,
    usage_seen: bool,
    done: bool,
}

impl Translator {
    fn start_block(&mut self, out: &mut Vec<Value>, block: Value) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        self.open.push(index);
        out.push(json!({"type": "content_block_start", "index": index, "content_block": block}));
        index
    }

    fn stop_block(&mut self, out: &mut Vec<Value>, index: usize) {
        if let Some(p) = self.open.iter().position(|i| *i == index) {
            self.open.remove(p);
            out.push(json!({"type": "content_block_stop", "index": index}));
        }
    }

    /// `s` into the open text (or thinking) block, starting one as needed.
    fn text(&mut self, out: &mut Vec<Value>, thinking: bool, s: &str) {
        if s.is_empty() {
            return;
        }
        let index = match self.open_text {
            Some((i, t)) if t == thinking => i,
            other => {
                if let Some((i, _)) = other {
                    self.stop_block(out, i);
                }
                let block = if thinking {
                    json!({"type": "thinking", "thinking": "", "signature": ""})
                } else {
                    json!({"type": "text", "text": ""})
                };
                let i = self.start_block(out, block);
                self.open_text = Some((i, thinking));
                i
            }
        };
        let delta = if thinking {
            json!({"type": "thinking_delta", "thinking": s})
        } else {
            json!({"type": "text_delta", "text": s})
        };
        out.push(json!({"type": "content_block_delta", "index": index, "delta": delta}));
    }

    fn content(&mut self, out: &mut Vec<Value>, s: &str) {
        for (thinking, run) in self.think.split(s) {
            self.text(out, thinking, &run);
        }
    }

    /// Stops every open block.
    fn close(&mut self, out: &mut Vec<Value>) {
        if let Some((thinking, held)) = self.think.flush() {
            self.text(out, thinking, &held);
        }
        for i in std::mem::take(&mut self.open) {
            out.push(json!({"type": "content_block_stop", "index": i}));
        }
        self.open_text = None;
    }

    /// The Messages API's events for one `data:` of the stream.
    pub(super) fn apply(&mut self, data: &str) -> Result<Vec<Value>, MessagesError> {
        let mut out = Vec::new();
        if self.done {
            return Ok(out);
        }
        if data.trim() == "[DONE]" {
            return self.end_message();
        }
        let v: Value = serde_json::from_str(data)
            .map_err(|e| protocol(format!("a chunk that is not JSON: {e}")))?;
        if v["error"].is_object() || v["object"] == "error" {
            let e = if v["error"].is_object() {
                &v["error"]
            } else {
                &v
            };
            let message = e["message"]
                .as_str()
                .unwrap_or("the provider reported an error")
                .to_string();
            let status = e["code"].as_u64().and_then(|c| u16::try_from(c).ok());
            return Err(classify(
                status.unwrap_or(0),
                e["type"].as_str().unwrap_or(""),
                message,
                None,
            ));
        }
        if !self.started {
            self.started = true;
            out.push(json!({"type": "message_start", "message": {
                "id": v["id"].as_str().unwrap_or(""),
                "model": v["model"].as_str().unwrap_or(""),
                "usage": {},
            }}));
        }
        if let Some(choice) = v["choices"].as_array().and_then(|c| c.first()) {
            let d = &choice["delta"];
            for k in ["reasoning_content", "reasoning"] {
                if let Some(s) = d[k].as_str() {
                    self.text(&mut out, true, s);
                }
            }
            match &d["content"] {
                Value::String(s) => self.content(&mut out, s),
                Value::Array(parts) => {
                    for p in parts {
                        match p["type"].as_str() {
                            Some("text") => {
                                self.content(&mut out, p["text"].as_str().unwrap_or(""));
                            }
                            Some("thinking") => {
                                let t = match &p["thinking"] {
                                    Value::String(s) => s.clone(),
                                    v => text_of(v),
                                };
                                self.text(&mut out, true, &t);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            for (n, tc) in d["tool_calls"].as_array().into_iter().flatten().enumerate() {
                self.tool_call(&mut out, n, tc);
            }
            if let Some(r) = choice["finish_reason"].as_str() {
                self.close(&mut out);
                self.finished = true;
                out.push(json!({"type": "message_delta",
                    "delta": {"stop_reason": self.stop_reason(r)}, "usage": {}}));
            }
        }
        if v["usage"].is_object() {
            let u = &v["usage"];
            let n = |x: &Value| x.as_u64().unwrap_or(0);
            let prompt = n(&u["prompt_tokens"]);
            let details = &u["prompt_tokens_details"];
            let cached = n(&details["cached_tokens"]).min(prompt);
            let written = n(&details["cache_write_tokens"]).min(prompt - cached);
            self.usage_seen = true;
            let mut usage = json!({
                "input_tokens": prompt - cached - written,
                "cache_read_input_tokens": cached,
                "cache_creation_input_tokens": written,
                "output_tokens": n(&u["completion_tokens"]),
            });
            if let Some(billed) = billed_micro_usd(u) {
                usage["billed_micro_usd"] = json!(billed);
            }
            out.push(json!({"type": "message_delta", "delta": {}, "usage": usage}));
        }
        Ok(out)
    }

    fn tool_call(&mut self, out: &mut Vec<Value>, n: usize, tc: &Value) {
        let at = tc["index"].as_u64().unwrap_or(n as u64);
        let index = match self.tools.iter().find(|(s, _)| *s == at) {
            Some((_, i)) => *i,
            None => {
                if let Some((i, _)) = self.open_text.take() {
                    self.stop_block(out, i);
                }
                let id = match tc["id"].as_str() {
                    Some(id) if !id.is_empty() => id.to_string(),
                    _ => format!("call_{}", self.next_index),
                };
                let i = self.start_block(
                    out,
                    json!({"type": "tool_use", "id": id,
                           "name": tc["function"]["name"].as_str().unwrap_or(""), "input": {}}),
                );
                self.tools.push((at, i));
                i
            }
        };
        let args = match &tc["function"]["arguments"] {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        };
        if !args.is_empty() {
            out.push(json!({"type": "content_block_delta", "index": index,
                "delta": {"type": "input_json_delta", "partial_json": args}}));
        }
    }

    fn stop_reason(&self, r: &str) -> String {
        match r {
            "tool_calls" | "function_call" => "tool_use".into(),
            // Some hosts finish a turn of tool calls with `stop`.
            "stop" if !self.tools.is_empty() => "tool_use".into(),
            "stop" => "end_turn".into(),
            "length" => "max_tokens".into(),
            "content_filter" => "refusal".into(),
            // Mistral: the prompt and answer outgrew the context window.
            "model_length" => "model_context_window_exceeded".into(),
            other => other.to_string(),
        }
    }

    fn end_message(&mut self) -> Result<Vec<Value>, MessagesError> {
        if !self.started {
            return Err(protocol("the stream ended before its first chunk"));
        }
        if !self.usage_seen {
            return Err(protocol("the stream carried no token usage"));
        }
        let mut out = Vec::new();
        self.close(&mut out);
        if !self.finished {
            return Err(MessagesError::StreamCut {
                message: "the stream ended without a finish_reason".into(),
            });
        }
        self.done = true;
        out.push(json!({"type": "message_stop"}));
        Ok(out)
    }

    /// The events the stream's end adds: none, since `[DONE]` ends it.
    pub(super) fn end(&mut self) -> Result<Vec<Value>, MessagesError> {
        Ok(Vec::new())
    }
}

//! The Messages API provider (spec D10): one streaming `POST {base_url}/v1/messages`
//! per round, with the key in `x-api-key` and nowhere else. Assistant content
//! blocks (thinking blocks with their signatures included) are collected as
//! they stream and handed back whole in `ProviderEvent::End`, so the next tool
//! round can pass them back unchanged. Provider error messages have the key
//! replaced by `[redacted]`.

use super::provider::*;
use super::sse::SseParser;
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl AnthropicProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> AnthropicProvider {
        AnthropicProvider {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("a reqwest client with default TLS"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }
}

fn block_json(b: &Block) -> Value {
    match b {
        Block::Text(t) => json!({"type": "text", "text": t}),
        Block::Image { media_type, data } => {
            json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
        }
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error})
        }
        Block::Raw(v) => v.clone(),
    }
}

/// The JSON body of one round's request.
pub fn request_body(req: &ProviderRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|t| json!({"role": t.role.as_str(), "content": t.content.iter().map(block_json).collect::<Vec<_>>()}))
        .collect();
    let mut body = json!({
        "model": req.model, "max_tokens": req.max_tokens, "stream": true,
        "system": req.system, "messages": messages,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
        if req.final_round {
            body["tool_choice"] = json!({"type": "none"});
        }
    }
    body
}

fn redact(message: &str, key: &str) -> String {
    if key.is_empty() {
        message.to_string()
    } else {
        message.replace(key, "[redacted]")
    }
}

fn error_code(kind: &str, message: &str) -> ProviderErrorCode {
    match kind {
        "rate_limit_error" => ProviderErrorCode::RateLimited,
        "authentication_error" | "permission_error" | "billing_error" | "not_found_error" => {
            ProviderErrorCode::Unavailable
        }
        "request_too_large" => ProviderErrorCode::PromptTooLarge,
        "invalid_request_error" if message.contains("prompt is too long") => {
            ProviderErrorCode::PromptTooLarge
        }
        "invalid_request_error" => ProviderErrorCode::InvalidRequest,
        _ => ProviderErrorCode::Upstream,
    }
}

/// The event for a non-2xx answer: by the body's error type when it has one, else by status.
pub fn http_error(status: u16, body: &str, key: &str) -> ProviderEvent {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let err = parsed
        .as_ref()
        .map(|v| &v["error"])
        .filter(|e| e.is_object());
    let (code, message) = match err {
        Some(e) => {
            let message = e["message"].as_str().unwrap_or("").to_string();
            (
                error_code(e["type"].as_str().unwrap_or(""), &message),
                message,
            )
        }
        None => (
            match status {
                429 => ProviderErrorCode::RateLimited,
                401..=404 => ProviderErrorCode::Unavailable,
                413 => ProviderErrorCode::PromptTooLarge,
                400 => ProviderErrorCode::InvalidRequest,
                _ => ProviderErrorCode::Upstream,
            },
            format!("the provider answered HTTP {status}"),
        ),
    };
    ProviderEvent::Error {
        code,
        message: redact(&message, key),
    }
}

fn stop_from(reason: &str) -> Stop {
    match reason {
        "max_tokens" => Stop::MaxTokens,
        "tool_use" => Stop::ToolUse,
        "refusal" => Stop::Refusal,
        _ => Stop::EndTurn,
    }
}

/// Folds one round's stream events into provider events and the round's content blocks.
#[derive(Default)]
pub struct Accumulator {
    blocks: Vec<Value>,
    partial: HashMap<usize, String>,
    stop: Option<Stop>,
}

impl Accumulator {
    fn block(&mut self, i: usize) -> &mut Value {
        if self.blocks.len() <= i {
            self.blocks.resize(i + 1, Value::Null);
        }
        &mut self.blocks[i]
    }

    fn append(&mut self, i: usize, field: &str, piece: &str) {
        let b = self.block(i);
        let now = format!("{}{piece}", b[field].as_str().unwrap_or(""));
        b[field] = Value::String(now);
    }

    /// Applies one SSE event; returns what it produced for the orchestrator.
    pub fn apply(&mut self, event: &str, data: &str) -> Vec<ProviderEvent> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let index = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str().unwrap_or(event) {
            "content_block_start" => {
                let mut b = v["content_block"].clone();
                if b["type"] == "tool_use" {
                    b["input"] = json!({});
                }
                *self.block(index) = b;
                Vec::new()
            }
            "content_block_delta" => {
                let d = &v["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let t = d["text"].as_str().unwrap_or("").to_string();
                        self.append(index, "text", &t);
                        vec![ProviderEvent::Text(t)]
                    }
                    Some("thinking_delta") => {
                        let t = d["thinking"].as_str().unwrap_or("").to_string();
                        self.append(index, "thinking", &t);
                        Vec::new()
                    }
                    Some("signature_delta") => {
                        let sig = d["signature"].clone();
                        self.block(index)["signature"] = sig;
                        Vec::new()
                    }
                    Some("input_json_delta") => {
                        self.partial
                            .entry(index)
                            .or_default()
                            .push_str(d["partial_json"].as_str().unwrap_or(""));
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                if self.block(index)["type"] != "tool_use" {
                    return Vec::new();
                }
                let raw = self.partial.remove(&index).unwrap_or_default();
                let input = if raw.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
                };
                let b = self.block(index);
                b["input"] = input.clone();
                vec![ProviderEvent::ToolUse {
                    id: b["id"].as_str().unwrap_or("").to_string(),
                    name: b["name"].as_str().unwrap_or("").to_string(),
                    input,
                }]
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(stop_from(r));
                }
                Vec::new()
            }
            "message_stop" => vec![ProviderEvent::End {
                stop: self.stop.unwrap_or(Stop::EndTurn),
                assistant: std::mem::take(&mut self.blocks)
                    .into_iter()
                    .filter(|b| !b.is_null())
                    .collect(),
            }],
            "error" => {
                let e = &v["error"];
                let message = e["message"]
                    .as_str()
                    .unwrap_or("the provider reported an error")
                    .to_string();
                vec![ProviderEvent::Error {
                    code: error_code(e["type"].as_str().unwrap_or(""), &message),
                    message,
                }]
            }
            _ => Vec::new(),
        }
    }
}

async fn run(
    client: reqwest::Client,
    url: String,
    key: String,
    req: ProviderRequest,
    tx: mpsc::Sender<ProviderEvent>,
) {
    let upstream = |m: String| ProviderEvent::Error {
        code: ProviderErrorCode::Upstream,
        message: m,
    };
    let res = match client
        .post(&url)
        .header("x-api-key", &key)
        .header("anthropic-version", API_VERSION)
        .json(&request_body(&req))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = tx
                .send(upstream(redact(
                    &format!("could not reach the provider: {e}"),
                    &key,
                )))
                .await;
            return;
        }
    };
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        let _ = tx.send(http_error(status, &body, &key)).await;
        return;
    }
    let mut parser = SseParser::default();
    let mut acc = Accumulator::default();
    let mut bytes = res.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(upstream(redact(
                        &format!("the provider's stream broke: {e}"),
                        &key,
                    )))
                    .await;
                return;
            }
        };
        for ev in parser.push(&chunk) {
            for out in acc.apply(&ev.event, &ev.data) {
                let last = matches!(out, ProviderEvent::End { .. } | ProviderEvent::Error { .. });
                let out = match out {
                    ProviderEvent::Error { code, message } => ProviderEvent::Error {
                        code,
                        message: redact(&message, &key),
                    },
                    other => other,
                };
                if tx.send(out).await.is_err() || last {
                    return;
                }
            }
        }
    }
    let _ = tx
        .send(upstream(
            "the provider's stream ended before message_stop".into(),
        ))
        .await;
}

impl super::provider::SampleProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn supports_images(&self) -> bool {
        true
    }

    fn stream(&self, req: ProviderRequest) -> EventStream {
        let (tx, rx) = mpsc::channel(64);
        let (client, url, key) = (
            self.client.clone(),
            format!("{}/v1/messages", self.base_url),
            self.api_key.clone(),
        );
        tokio::spawn(async move {
            let closed = tx.clone();
            tokio::select! {
                () = closed.closed() => {}
                () = run(client, url, key, req, tx) => {}
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(acc: &mut Accumulator, events: &[(&str, serde_json::Value)]) -> Vec<ProviderEvent> {
        events
            .iter()
            .flat_map(|(e, d)| acc.apply(e, &d.to_string()))
            .collect()
    }

    #[test]
    fn text_deltas_stream_and_the_turn_is_kept_verbatim() {
        let mut acc = Accumulator::default();
        let out = run(
            &mut acc,
            &[
                (
                    "message_start",
                    json!({"type": "message_start", "message": {"id": "msg_1"}}),
                ),
                (
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                (
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
                ),
                ("ping", json!({"type": "ping"})),
                (
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Hel"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "lo"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 1}),
                ),
                (
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
                ),
                ("message_stop", json!({"type": "message_stop"})),
            ],
        );
        assert_eq!(
            out,
            vec![
                ProviderEvent::Text("Hel".into()),
                ProviderEvent::Text("lo".into()),
                ProviderEvent::End {
                    stop: Stop::EndTurn,
                    assistant: vec![
                        json!({"type": "thinking", "thinking": "hmm", "signature": "sig"}),
                        json!({"type": "text", "text": "Hello"}),
                    ],
                },
            ]
        );
    }

    #[test]
    fn tool_input_arrives_in_pieces_and_is_parsed_at_block_stop() {
        let mut acc = Accumulator::default();
        let out = run(
            &mut acc,
            &[
                (
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "getColor", "input": {}}}),
                ),
                (
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"loc"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "ation\": \"Paris\"}"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                (
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_2", "name": "now", "input": {}}}),
                ),
                (
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 1}),
                ),
                (
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
                ),
                ("message_stop", json!({"type": "message_stop"})),
            ],
        );
        assert_eq!(
            out[0],
            ProviderEvent::ToolUse {
                id: "toolu_1".into(),
                name: "getColor".into(),
                input: json!({"location": "Paris"})
            }
        );
        assert_eq!(
            out[1],
            ProviderEvent::ToolUse {
                id: "toolu_2".into(),
                name: "now".into(),
                input: json!({})
            }
        );
        let ProviderEvent::End { stop, assistant } = &out[2] else {
            panic!("{out:?}")
        };
        assert_eq!(*stop, Stop::ToolUse);
        assert_eq!(assistant[0]["input"], json!({"location": "Paris"}));
    }

    #[test]
    fn stop_reasons_and_stream_errors_map_to_the_contract() {
        for (reason, stop) in [
            ("max_tokens", Stop::MaxTokens),
            ("refusal", Stop::Refusal),
            ("stop_sequence", Stop::EndTurn),
            ("pause_turn", Stop::EndTurn),
        ] {
            let mut acc = Accumulator::default();
            let out = run(
                &mut acc,
                &[
                    (
                        "message_delta",
                        json!({"type": "message_delta", "delta": {"stop_reason": reason}}),
                    ),
                    ("message_stop", json!({"type": "message_stop"})),
                ],
            );
            assert_eq!(
                out,
                vec![ProviderEvent::End {
                    stop,
                    assistant: vec![]
                }],
                "{reason}"
            );
        }
        let mut acc = Accumulator::default();
        let out = run(
            &mut acc,
            &[(
                "error",
                json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
            )],
        );
        assert_eq!(
            out,
            vec![ProviderEvent::Error {
                code: ProviderErrorCode::Upstream,
                message: "Overloaded".into()
            }]
        );
    }

    #[test]
    fn http_errors_map_by_type_then_status() {
        let e = |status: u16, body: &str| match http_error(status, body, "sk-secret") {
            ProviderEvent::Error { code, .. } => code,
            other => panic!("{other:?}"),
        };
        let typed = |t: &str, m: &str| {
            json!({"type": "error", "error": {"type": t, "message": m}}).to_string()
        };
        assert_eq!(
            e(429, &typed("rate_limit_error", "slow down")),
            ProviderErrorCode::RateLimited
        );
        assert_eq!(
            e(401, &typed("authentication_error", "bad key")),
            ProviderErrorCode::Unavailable
        );
        assert_eq!(
            e(402, &typed("billing_error", "no credit")),
            ProviderErrorCode::Unavailable
        );
        assert_eq!(
            e(404, &typed("not_found_error", "model: x")),
            ProviderErrorCode::Unavailable
        );
        assert_eq!(
            e(
                400,
                &typed(
                    "invalid_request_error",
                    "prompt is too long: 300000 tokens > 200000 maximum"
                )
            ),
            ProviderErrorCode::PromptTooLarge
        );
        assert_eq!(
            e(
                400,
                &typed("invalid_request_error", "tools.0.input_schema: invalid")
            ),
            ProviderErrorCode::InvalidRequest
        );
        assert_eq!(
            e(413, &typed("request_too_large", "too big")),
            ProviderErrorCode::PromptTooLarge
        );
        assert_eq!(
            e(529, &typed("overloaded_error", "busy")),
            ProviderErrorCode::Upstream
        );
        assert_eq!(e(503, "<html>"), ProviderErrorCode::Upstream);
        assert_eq!(e(429, ""), ProviderErrorCode::RateLimited);
    }

    #[test]
    fn provider_errors_never_carry_the_key() {
        let body = json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key: sk-secret"}}).to_string();
        let ProviderEvent::Error { message, .. } = http_error(401, &body, "sk-secret") else {
            panic!()
        };
        assert!(!message.contains("sk-secret"), "{message}");
        assert!(message.contains("[redacted]"), "{message}");
    }

    #[test]
    fn the_provider_debug_string_hides_the_key() {
        let p = AnthropicProvider::new("http://127.0.0.1:9", "sk-secret");
        assert!(!format!("{p:?}").contains("sk-secret"));
    }

    #[test]
    fn the_request_body_echoes_raw_turns_and_ends_tool_rounds_with_tool_choice_none() {
        let req = ProviderRequest {
            model: "claude-sonnet-5-5".into(),
            max_tokens: 100,
            system: "frame".into(),
            messages: vec![
                Turn {
                    role: Role::User,
                    content: vec![
                        Block::Image {
                            media_type: "image/png".into(),
                            data: "AAAA".into(),
                        },
                        Block::Text("hi".into()),
                    ],
                },
                Turn {
                    role: Role::Assistant,
                    content: vec![
                        Block::Raw(json!({"type": "thinking", "thinking": "", "signature": "s"})),
                        Block::Raw(
                            json!({"type": "tool_use", "id": "t1", "name": "n", "input": {}}),
                        ),
                    ],
                },
                Turn {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        tool_use_id: "t1".into(),
                        content: "teal".into(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "n".into(),
                description: "d".into(),
                input_schema: json!({"type": "object", "properties": {}}),
            }],
            final_round: true,
        };
        assert_eq!(
            request_body(&req),
            json!({
                "model": "claude-sonnet-5-5", "max_tokens": 100, "stream": true, "system": "frame",
                "messages": [
                    {"role": "user", "content": [
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
                        {"type": "text", "text": "hi"}]},
                    {"role": "assistant", "content": [
                        {"type": "thinking", "thinking": "", "signature": "s"},
                        {"type": "tool_use", "id": "t1", "name": "n", "input": {}}]},
                    {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "teal", "is_error": false}]}],
                "tools": [{"name": "n", "description": "d", "input_schema": {"type": "object", "properties": {}}}],
                "tool_choice": {"type": "none"}
            })
        );
        let no_tools = ProviderRequest {
            tools: vec![],
            final_round: false,
            ..req
        };
        let body = request_body(&no_tools);
        assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());
    }
}

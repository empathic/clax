//! A Messages API tool-use client: one streaming `POST {base_url}/v1/messages`
//! per call, folded into live [`Delta`]s and the assembled
//! [`AssistantMessage`] (content blocks kept verbatim, usage, stop reason),
//! and `GET {base_url}/v1/models` for the models a key may use.
//!
//! The key is passed per call as an [`ApiKey`], whose `Debug` hides it and
//! which has no `Display`. It is sent only to the `base_url` given with it,
//! in `x-api-key`, after that URL passes
//! [`clax_core::config::base_url_ok`] (the `[sample]` rule), and every
//! error message has it replaced by `[redacted]`. A redirect is never
//! followed (it would carry the key to the host it names): a 3xx answer is
//! [`MessagesError::Redirected`].
//!
//! Every wait has a deadline ([`Timeouts`]): for the answer's headers, for
//! each next piece of the body (the API sends `ping`s while it works), and
//! optionally for the whole call; one that passes is
//! [`MessagesError::StreamCut`], which is retryable.
//!
//! It builds for wasm32 too, where requests go through the Worker's
//! `fetch`. [`with_retry`] retries the errors [`MessagesError::retryable`]
//! names under a bounded [`RetryPolicy`], waiting through an injected
//! sleeper (`clax_rt::time::sleep` in production).

use super::sse::SseParser;
use super::transport::{Body, Transport};
use futures::StreamExt;
use serde_json::{Map, Value, json};
use std::future::Future;
use std::time::Duration;

pub use super::anthropic::API_VERSION;

/// Most pages [`MessagesClient::list_models`] follows before it stops.
pub const MAX_MODEL_PAGES: usize = 50;
/// Models asked for per page.
pub const MODEL_PAGE_SIZE: u32 = 100;
/// Highest content block index a stream may use.
const MAX_BLOCK_INDEX: usize = 4096;

/// An API key. `Debug` prints `ApiKey([redacted])`; there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> ApiKey {
        ApiKey(key.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// `cache_control: {type: "ephemeral", ttl?}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheControl {
    /// `"5m"` or `"1h"`; `None` leaves the API's default (5 minutes).
    pub ttl: Option<String>,
}

impl CacheControl {
    fn to_json(&self) -> Value {
        let mut v = json!({"type": "ephemeral"});
        if let Some(ttl) = &self.ttl {
            v["ttl"] = json!(ttl);
        }
        v
    }
}

/// One text block of the system prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemBlock {
    pub text: String,
    pub cache_control: Option<CacheControl>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A mid-conversation operator instruction (after a user turn), which
    /// leaves the cached prefix before it intact.
    System,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        }
    }
}

/// A content block. Blocks read from a response are typed only when the
/// typed form serialises back to exactly what arrived; anything else (an
/// unknown type, an extra field such as `citations`) is kept as
/// [`Block::Raw`], so a turn is always passed back byte-for-byte as JSON.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text {
        text: String,
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        /// A string, or an array of content blocks.
        content: Value,
        is_error: bool,
        cache_control: Option<CacheControl>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    Raw(Value),
}

impl Block {
    pub fn text(text: impl Into<String>) -> Block {
        Block::Text {
            text: text.into(),
            cache_control: None,
        }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Block {
        Block::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: Value::String(content.into()),
            is_error,
            cache_control: None,
        }
    }

    pub fn to_json(&self) -> Value {
        let cached = |mut v: Value, cc: &Option<CacheControl>| {
            if let Some(cc) = cc {
                v["cache_control"] = cc.to_json();
            }
            v
        };
        match self {
            Block::Text {
                text,
                cache_control,
            } => cached(json!({"type": "text", "text": text}), cache_control),
            Block::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
                cache_control,
            } => {
                let mut v =
                    json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content});
                if *is_error {
                    v["is_error"] = json!(true);
                }
                cached(v, cache_control)
            }
            Block::Thinking {
                thinking,
                signature,
            } => json!({"type": "thinking", "thinking": thinking, "signature": signature}),
            Block::RedactedThinking { data } => json!({"type": "redacted_thinking", "data": data}),
            Block::Raw(v) => v.clone(),
        }
    }

    /// The block for `v`: typed when that round-trips exactly, else raw.
    pub fn from_json(v: Value) -> Block {
        let s = |k: &str| v[k].as_str().map(str::to_string);
        let typed = match v["type"].as_str() {
            Some("text") => s("text").map(|text| Block::Text {
                text,
                cache_control: None,
            }),
            Some("tool_use") => match (s("id"), s("name")) {
                (Some(id), Some(name)) => Some(Block::ToolUse {
                    id,
                    name,
                    input: v["input"].clone(),
                }),
                _ => None,
            },
            Some("thinking") => match (s("thinking"), s("signature")) {
                (Some(thinking), Some(signature)) => Some(Block::Thinking {
                    thinking,
                    signature,
                }),
                _ => None,
            },
            Some("redacted_thinking") => s("data").map(|data| Block::RedactedThinking { data }),
            _ => None,
        };
        match typed {
            Some(b) if b.to_json() == v => b,
            _ => Block::Raw(v),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn user(content: Vec<Block>) -> Message {
        Message {
            role: Role::User,
            content,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"role": self.role.as_str(), "content": self.content.iter().map(Block::to_json).collect::<Vec<_>>()})
    }
}

/// A client tool the model may call.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// A JSON Schema object.
    pub input_schema: Value,
    pub cache_control: Option<CacheControl>,
    /// `eager_input_streaming: true`: the input streams as the model writes
    /// it, unvalidated, so it may not parse; such a call comes back as
    /// [`ToolCall::Invalid`], to answer with an `is_error` result.
    pub eager_input_streaming: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolChoice {
    Auto { disable_parallel_tool_use: bool },
    None,
}

/// `thinking: {type: "adaptive", display?}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Thinking {
    /// `"summarized"`, `"omitted"`; `None` leaves the model's default.
    pub display: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: Vec<SystemBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    pub tool_choice: Option<ToolChoice>,
    pub thinking: Option<Thinking>,
    /// `output_config.effort`: `low` … `max`; `None` leaves the model's default.
    pub effort: Option<String>,
    /// `anthropic-beta` values, sent joined by commas when any.
    pub betas: Vec<String>,
    /// Top-level body fields set last, over any this builds (for example
    /// `fallbacks`).
    pub extra: Map<String, Value>,
}

impl MessagesRequest {
    /// The streaming request body.
    pub fn body(&self) -> Value {
        let mut b = Map::new();
        b.insert("model".into(), json!(self.model));
        b.insert("max_tokens".into(), json!(self.max_tokens));
        b.insert("stream".into(), json!(true));
        if !self.system.is_empty() {
            let system: Vec<Value> = self
                .system
                .iter()
                .map(|s| {
                    Block::Text {
                        text: s.text.clone(),
                        cache_control: s.cache_control.clone(),
                    }
                    .to_json()
                })
                .collect();
            b.insert("system".into(), Value::Array(system));
        }
        b.insert(
            "messages".into(),
            self.messages.iter().map(Message::to_json).collect(),
        );
        if !self.tools.is_empty() {
            let tools: Vec<Value> = self
                .tools
                .iter()
                .map(|t| {
                    let mut v = json!({"name": t.name, "description": t.description, "input_schema": t.input_schema});
                    if let Some(cc) = &t.cache_control {
                        v["cache_control"] = cc.to_json();
                    }
                    if t.eager_input_streaming {
                        v["eager_input_streaming"] = json!(true);
                    }
                    v
                })
                .collect();
            b.insert("tools".into(), Value::Array(tools));
        }
        match &self.tool_choice {
            Some(ToolChoice::Auto {
                disable_parallel_tool_use,
            }) => {
                let mut c = json!({"type": "auto"});
                if *disable_parallel_tool_use {
                    c["disable_parallel_tool_use"] = json!(true);
                }
                b.insert("tool_choice".into(), c);
            }
            Some(ToolChoice::None) => {
                b.insert("tool_choice".into(), json!({"type": "none"}));
            }
            None => {}
        }
        if let Some(t) = &self.thinking {
            let mut v = json!({"type": "adaptive"});
            if let Some(d) = &t.display {
                v["display"] = json!(d);
            }
            b.insert("thinking".into(), v);
        }
        if let Some(e) = &self.effort {
            b.insert("output_config".into(), json!({"effort": e}));
        }
        for (k, v) in &self.extra {
            b.insert(k.clone(), v.clone());
        }
        Value::Object(b)
    }

    /// The `anthropic-beta` header's value, when any beta is named.
    pub fn beta_header(&self) -> Option<String> {
        (!self.betas.is_empty()).then(|| self.betas.join(","))
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// Token counts of one call. `input_tokens` excludes the cached parts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    /// Overwrites each count `v` carries (`message_start`'s, then each
    /// `message_delta`'s cumulative counts).
    fn merge(&mut self, v: &Value) {
        let set = |slot: &mut u64, k: &str| {
            if let Some(n) = v[k].as_u64() {
                *slot = n;
            }
        };
        set(&mut self.input_tokens, "input_tokens");
        set(
            &mut self.cache_creation_input_tokens,
            "cache_creation_input_tokens",
        );
        set(&mut self.cache_read_input_tokens, "cache_read_input_tokens");
        set(&mut self.output_tokens, "output_tokens");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    PauseTurn,
    Refusal,
    ModelContextWindowExceeded,
    Other(String),
}

impl StopReason {
    pub fn parse(s: &str) -> StopReason {
        match s {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            "pause_turn" => StopReason::PauseTurn,
            "refusal" => StopReason::Refusal,
            "model_context_window_exceeded" => StopReason::ModelContextWindowExceeded,
            other => StopReason::Other(other.to_string()),
        }
    }
}

/// The assembled assistant turn.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub id: String,
    pub model: String,
    /// The blocks in index order, to pass back as the assistant turn.
    pub content: Vec<Block>,
    /// `None` only when the stream never sent one.
    pub stop_reason: Option<StopReason>,
    /// `stop_details` as sent (set on a refusal), else `None`.
    pub stop_details: Option<Value>,
    pub usage: Usage,
    /// Tool calls whose streamed input did not parse as a JSON object
    /// (possible with [`Tool::eager_input_streaming`]); their block's
    /// `input` is `{}`. Read through [`AssistantMessage::tool_calls`].
    invalid_inputs: Vec<InvalidInput>,
}

#[derive(Clone, Debug, PartialEq)]
struct InvalidInput {
    id: String,
    raw: String,
    error: String,
}

/// One client tool call of a turn, from [`AssistantMessage::tool_calls`].
#[derive(Clone, Debug, PartialEq)]
pub enum ToolCall {
    Valid {
        id: String,
        name: String,
        input: Value,
    },
    /// Its input did not parse: do not run it; answer it with
    /// [`ToolCall::invalid_result`].
    Invalid {
        id: String,
        name: String,
        /// The input text as it streamed.
        raw: String,
        error: String,
    },
}

impl ToolCall {
    pub fn id(&self) -> &str {
        match self {
            ToolCall::Valid { id, .. } | ToolCall::Invalid { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ToolCall::Valid { name, .. } | ToolCall::Invalid { name, .. } => name,
        }
    }

    /// For an invalid call, the `is_error` `tool_result` to send back:
    /// content `{"INVALID_JSON": <raw>}`. `None` for a valid one.
    pub fn invalid_result(&self) -> Option<Block> {
        match self {
            ToolCall::Invalid { id, raw, .. } => Some(Block::tool_result(
                id.clone(),
                json!({"INVALID_JSON": raw}).to_string(),
                true,
            )),
            ToolCall::Valid { .. } => None,
        }
    }
}

impl AssistantMessage {
    /// The turn to append to the history.
    pub fn to_message(&self) -> Message {
        Message {
            role: Role::Assistant,
            content: self.content.clone(),
        }
    }

    /// The client tool calls (`tool_use` blocks, typed or kept raw for an
    /// extra field such as `caller`), in order; server tool calls are not
    /// among them. Check [`AssistantMessage::stop_reason`] first: a turn
    /// stopped by `max_tokens` or `refusal` may hold cut-off calls that
    /// must not run.
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|b| {
                let (id, name, input) = match b {
                    Block::ToolUse { id, name, input } => (id.as_str(), name.as_str(), input),
                    Block::Raw(v) if v["type"] == "tool_use" => {
                        (v["id"].as_str()?, v["name"].as_str()?, &v["input"])
                    }
                    _ => return None,
                };
                Some(match self.invalid_inputs.iter().find(|i| i.id == id) {
                    Some(bad) => ToolCall::Invalid {
                        id: id.to_string(),
                        name: name.to_string(),
                        raw: bad.raw.clone(),
                        error: bad.error.clone(),
                    },
                    None => ToolCall::Valid {
                        id: id.to_string(),
                        name: name.to_string(),
                        input: input.clone(),
                    },
                })
            })
            .collect()
    }
}

/// What a stream produced, for a live view. Blocks are addressed by the
/// stream's `index`.
#[derive(Clone, Debug, PartialEq)]
pub enum Delta {
    MessageStart {
        id: String,
        model: String,
        usage: Usage,
    },
    /// The block as `content_block_start` sent it.
    BlockStart {
        index: usize,
        block: Value,
    },
    Text {
        index: usize,
        text: String,
    },
    Thinking {
        index: usize,
        thinking: String,
    },
    /// A piece of a tool call's input JSON.
    ToolInput {
        index: usize,
        partial_json: String,
    },
    /// The finished block.
    BlockStop {
        index: usize,
        block: Block,
    },
    /// The running usage and, when known, the stop reason.
    MessageDelta {
        stop_reason: Option<StopReason>,
        usage: Usage,
    },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a call failed. Messages never carry the key.
#[derive(Clone, Debug, PartialEq)]
pub enum MessagesError {
    /// The base URL fails [`clax_core::config::base_url_ok`]; nothing was sent.
    BadBaseUrl,
    /// 401 or 403, or an `authentication_error`/`permission_error`.
    Auth { status: u16, message: String },
    /// 429; `retry_after` from the header when it held seconds.
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    /// 529, or an `overloaded_error` (also mid-stream).
    Overloaded { message: String },
    /// 400 `invalid_request_error`.
    InvalidRequest { message: String },
    /// Any other provider error; `status` is 0 for one sent mid-stream.
    Api {
        status: u16,
        kind: String,
        message: String,
    },
    /// The base URL answered with a redirect (3xx, or 0 for a browser's
    /// opaque redirect), which is never followed.
    Redirected { status: u16 },
    /// No answer: connect, TLS or send failed.
    Network { message: String },
    /// The stream broke, ended before `message_stop`, or a deadline passed.
    StreamCut { message: String },
    /// The provider sent something this cannot read.
    Protocol { message: String },
    /// A content delta of a type this does not know (`kind` is
    /// `content_block_delta/<type>`): dropping it would leave its block
    /// incomplete, so the turn fails. Not retryable: the same request
    /// streams the same type again; this client needs to learn it.
    UnknownEvent { kind: String },
}

impl MessagesError {
    /// What went wrong in a few words of this client's own, safe to show
    /// to whoever configured the provider: the kind and the HTTP status,
    /// never text from the answer (the base URL may be anyone's host).
    pub fn summary(&self) -> String {
        let status = |s: &u16| {
            if *s == 0 {
                String::new()
            } else {
                format!(" (HTTP {s})")
            }
        };
        match self {
            MessagesError::BadBaseUrl => "the base URL is not one a key may be sent to".into(),
            MessagesError::Auth { status: s, .. } => format!("the key was refused{}", status(s)),
            MessagesError::RateLimited { .. } => "rate limited (HTTP 429)".into(),
            MessagesError::Overloaded { .. } => "the provider is overloaded".into(),
            MessagesError::InvalidRequest { .. } => {
                "the provider refused the request (HTTP 400)".into()
            }
            MessagesError::Api {
                status: s, kind, ..
            } => {
                let kind = if !kind.is_empty()
                    && kind.len() <= 40
                    && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                {
                    kind.as_str()
                } else {
                    "provider error"
                };
                format!("{kind}{}", status(s))
            }
            MessagesError::Redirected { status: s } => {
                format!("the base URL answered with a redirect{}", status(s))
            }
            MessagesError::Network { .. } => "the provider could not be reached".into(),
            MessagesError::StreamCut { .. } => "the answer was cut off".into(),
            MessagesError::Protocol { .. } => "the answer was not the Messages API's".into(),
            MessagesError::UnknownEvent { .. } => {
                "the answer held an event this client does not know".into()
            }
        }
    }

    /// Whether the same request may succeed when sent again. A retried
    /// stream starts over: its deltas arrive again from the first.
    pub fn retryable(&self) -> bool {
        match self {
            MessagesError::RateLimited { .. }
            | MessagesError::Overloaded { .. }
            | MessagesError::Network { .. }
            | MessagesError::StreamCut { .. } => true,
            MessagesError::Api { status, kind, .. } => *status >= 500 || kind == "api_error",
            _ => false,
        }
    }

    fn redacted(self, key: &ApiKey) -> MessagesError {
        let k = key.expose();
        if k.is_empty() {
            return self;
        }
        let r = |m: String| m.replace(k, "[redacted]");
        match self {
            MessagesError::Auth { status, message } => MessagesError::Auth {
                status,
                message: r(message),
            },
            MessagesError::RateLimited {
                retry_after,
                message,
            } => MessagesError::RateLimited {
                retry_after,
                message: r(message),
            },
            MessagesError::Overloaded { message } => MessagesError::Overloaded {
                message: r(message),
            },
            MessagesError::InvalidRequest { message } => MessagesError::InvalidRequest {
                message: r(message),
            },
            MessagesError::Api {
                status,
                kind,
                message,
            } => MessagesError::Api {
                status,
                kind: r(kind),
                message: r(message),
            },
            MessagesError::Network { message } => MessagesError::Network {
                message: r(message),
            },
            MessagesError::StreamCut { message } => MessagesError::StreamCut {
                message: r(message),
            },
            MessagesError::Protocol { message } => MessagesError::Protocol {
                message: r(message),
            },
            MessagesError::UnknownEvent { kind } => MessagesError::UnknownEvent { kind: r(kind) },
            e @ (MessagesError::BadBaseUrl | MessagesError::Redirected { .. }) => e,
        }
    }
}

impl std::fmt::Display for MessagesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessagesError::BadBaseUrl => {
                f.write_str("the base URL must be https://, or http:// on a loopback host")
            }
            MessagesError::Auth { status, message } => {
                write!(f, "the provider refused the key (HTTP {status}): {message}")
            }
            MessagesError::RateLimited {
                retry_after,
                message,
            } => match retry_after {
                Some(d) => write!(f, "rate limited, retry after {}s: {message}", d.as_secs()),
                None => write!(f, "rate limited: {message}"),
            },
            MessagesError::Overloaded { message } => {
                write!(f, "the provider is overloaded: {message}")
            }
            MessagesError::InvalidRequest { message } => write!(f, "invalid request: {message}"),
            MessagesError::Api {
                status,
                kind,
                message,
            } => write!(f, "provider error {kind} (HTTP {status}): {message}"),
            MessagesError::Redirected { status: 0 } => f.write_str(
                "the base URL answered with an opaque redirect; set the URL it leads to instead",
            ),
            MessagesError::Redirected { status } => write!(
                f,
                "the base URL answered with a redirect (HTTP {status}); set the URL it names instead"
            ),
            MessagesError::Network { message } => {
                write!(f, "could not reach the provider: {message}")
            }
            MessagesError::StreamCut { message } => write!(f, "the stream was cut: {message}"),
            MessagesError::Protocol { message } => {
                write!(f, "unreadable provider answer: {message}")
            }
            MessagesError::UnknownEvent { kind } => {
                write!(
                    f,
                    "the provider streamed an event this client does not know: {kind}"
                )
            }
        }
    }
}

impl std::error::Error for MessagesError {}

/// The error for a provider error of `kind` (`status` 0 mid-stream).
fn classify(
    status: u16,
    kind: &str,
    message: String,
    retry_after: Option<Duration>,
) -> MessagesError {
    match (status, kind) {
        (401 | 403, _) | (_, "authentication_error" | "permission_error") => {
            MessagesError::Auth { status, message }
        }
        (429, _) | (_, "rate_limit_error") => MessagesError::RateLimited {
            retry_after,
            message,
        },
        (529, _) | (_, "overloaded_error") => MessagesError::Overloaded { message },
        (_, "invalid_request_error") | (400, "") => MessagesError::InvalidRequest { message },
        _ => MessagesError::Api {
            status,
            kind: kind.to_string(),
            message,
        },
    }
}

/// The error for a non-2xx answer with `body`.
pub fn http_error(status: u16, retry_after: Option<&str>, body: &str) -> MessagesError {
    if status == 0 || (300..400).contains(&status) {
        return MessagesError::Redirected { status };
    }
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = &v["error"];
    let kind = e["type"].as_str().unwrap_or("");
    let message = e["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("the provider answered HTTP {status}"));
    let retry_after = retry_after
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64);
    classify(status, kind, message, retry_after)
}

// ---------------------------------------------------------------------------
// Stream folding
// ---------------------------------------------------------------------------

fn protocol(m: impl Into<String>) -> MessagesError {
    MessagesError::Protocol { message: m.into() }
}

/// A block whose `input` may stream as `input_json_delta`s; `start` is the
/// input its `content_block_start` carried, kept when no delta arrives.
struct Partial {
    index: usize,
    start: Value,
    json: String,
}

/// Folds one call's SSE events into [`Delta`]s and its [`AssistantMessage`].
#[derive(Default)]
pub struct Assembler {
    id: String,
    model: String,
    blocks: Vec<Value>,
    partial: Vec<Partial>,
    invalid: Vec<InvalidInput>,
    stop_reason: Option<StopReason>,
    stop_details: Option<Value>,
    usage: Usage,
    done: bool,
}

impl Assembler {
    /// Whether `message_stop` arrived.
    pub fn done(&self) -> bool {
        self.done
    }

    fn block(&mut self, i: usize) -> Result<&mut Value, MessagesError> {
        if i > MAX_BLOCK_INDEX {
            return Err(protocol(format!("content block index {i} is out of range")));
        }
        if self.blocks.len() <= i {
            self.blocks.resize(i + 1, Value::Null);
        }
        Ok(&mut self.blocks[i])
    }

    /// Block `i`, which must have started.
    fn started(&mut self, i: usize) -> Result<&mut Value, MessagesError> {
        let b = self.block(i)?;
        if !b.is_object() {
            return Err(protocol(format!(
                "a delta for block {i}, which never started"
            )));
        }
        Ok(b)
    }

    fn append(&mut self, i: usize, field: &str, piece: &str) -> Result<(), MessagesError> {
        let b = self.started(i)?;
        let now = format!("{}{piece}", b[field].as_str().unwrap_or(""));
        b[field] = Value::String(now);
        Ok(())
    }

    /// Applies one SSE event. A `content_block_*` event without an `index`
    /// is a [`MessagesError::Protocol`]; a delta of a type this does not
    /// know is a [`MessagesError::UnknownEvent`]. Unknown event types (not
    /// deltas) are skipped, as `ping` is.
    pub fn apply(&mut self, event: &str, data: &str) -> Result<Vec<Delta>, MessagesError> {
        let v: Value = serde_json::from_str(data)
            .map_err(|e| protocol(format!("an event that is not JSON ({event}): {e}")))?;
        let kind = v["type"].as_str().unwrap_or(event).to_string();
        let index = || {
            v["index"]
                .as_u64()
                .map(|i| i as usize)
                .ok_or_else(|| protocol(format!("{kind} without an index")))
        };
        match kind.as_str() {
            "message_start" => {
                let m = &v["message"];
                self.id = m["id"].as_str().unwrap_or("").to_string();
                self.model = m["model"].as_str().unwrap_or("").to_string();
                self.usage.merge(&m["usage"]);
                Ok(vec![Delta::MessageStart {
                    id: self.id.clone(),
                    model: self.model.clone(),
                    usage: self.usage,
                }])
            }
            "content_block_start" => {
                let index = index()?;
                let sent = v["content_block"].clone();
                if !sent.is_object() {
                    return Err(protocol("content_block_start without a block"));
                }
                let mut b = sent.clone();
                // Any block whose input streams (tool_use, server_tool_use,
                // mcp_tool_use, …) starts with an object `input`.
                if b["input"].is_object() {
                    let start = std::mem::replace(&mut b["input"], json!({}));
                    self.partial.retain(|p| p.index != index);
                    self.partial.push(Partial {
                        index,
                        start,
                        json: String::new(),
                    });
                }
                *self.block(index)? = b;
                Ok(vec![Delta::BlockStart { index, block: sent }])
            }
            "content_block_delta" => {
                let index = index()?;
                let d = &v["delta"];
                let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let text = s("text");
                        self.append(index, "text", &text)?;
                        Ok(vec![Delta::Text { index, text }])
                    }
                    Some("thinking_delta") => {
                        let thinking = s("thinking");
                        self.append(index, "thinking", &thinking)?;
                        Ok(vec![Delta::Thinking { index, thinking }])
                    }
                    Some("signature_delta") => {
                        self.append(index, "signature", &s("signature"))?;
                        Ok(Vec::new())
                    }
                    Some("input_json_delta") => {
                        let partial_json = s("partial_json");
                        let Some(p) = self.partial.iter_mut().find(|p| p.index == index) else {
                            return Err(protocol(format!(
                                "input for block {index}, which has no input"
                            )));
                        };
                        p.json.push_str(&partial_json);
                        Ok(vec![Delta::ToolInput {
                            index,
                            partial_json,
                        }])
                    }
                    Some("citations_delta") => {
                        let b = self.started(index)?;
                        if !b["citations"].is_array() {
                            b["citations"] = json!([]);
                        }
                        if let Some(a) = b["citations"].as_array_mut() {
                            a.push(d["citation"].clone());
                        }
                        Ok(Vec::new())
                    }
                    other => Err(MessagesError::UnknownEvent {
                        kind: format!("content_block_delta/{}", other.unwrap_or("(none)")),
                    }),
                }
            }
            "content_block_stop" => {
                let index = index()?;
                self.started(index)?;
                if let Some(pos) = self.partial.iter().position(|p| p.index == index) {
                    let Partial {
                        start, json: raw, ..
                    } = self.partial.remove(pos);
                    let input = if raw.trim().is_empty() {
                        start
                    } else {
                        match serde_json::from_str::<Value>(&raw) {
                            Ok(v) if v.is_object() => v,
                            parsed => {
                                let b = &self.blocks[index];
                                self.invalid.push(InvalidInput {
                                    id: b["id"].as_str().unwrap_or("").to_string(),
                                    raw,
                                    error: match parsed {
                                        Err(e) => e.to_string(),
                                        Ok(_) => "the input is not a JSON object".into(),
                                    },
                                });
                                json!({})
                            }
                        }
                    };
                    self.block(index)?["input"] = input;
                }
                let block = Block::from_json(self.block(index)?.clone());
                Ok(vec![Delta::BlockStop { index, block }])
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(StopReason::parse(r));
                }
                let details = &v["delta"]["stop_details"];
                if details.is_object() {
                    self.stop_details = Some(details.clone());
                }
                self.usage.merge(&v["usage"]);
                Ok(vec![Delta::MessageDelta {
                    stop_reason: self.stop_reason.clone(),
                    usage: self.usage,
                }])
            }
            "message_stop" => {
                self.done = true;
                Ok(Vec::new())
            }
            "error" => {
                let e = &v["error"];
                let message = e["message"]
                    .as_str()
                    .unwrap_or("the provider reported an error")
                    .to_string();
                Err(classify(0, e["type"].as_str().unwrap_or(""), message, None))
            }
            // `ping` and event types added later.
            _ => Ok(Vec::new()),
        }
    }

    /// The assembled message, once [`Assembler::done`].
    pub fn finish(self) -> Result<AssistantMessage, MessagesError> {
        if !self.done {
            return Err(MessagesError::StreamCut {
                message: "the stream ended before message_stop".into(),
            });
        }
        Ok(AssistantMessage {
            id: self.id,
            model: self.model,
            content: self
                .blocks
                .into_iter()
                .filter(|b| !b.is_null())
                .map(Block::from_json)
                .collect(),
            stop_reason: self.stop_reason,
            stop_details: self.stop_details,
            usage: self.usage,
            invalid_inputs: self.invalid,
        })
    }
}

// ---------------------------------------------------------------------------
// Deadlines
// ---------------------------------------------------------------------------

/// How long a call may wait. Each that passes is a [`MessagesError::StreamCut`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// From sending the request to the answer's status and headers.
    pub headers: Duration,
    /// Between two pieces of the body (the API sends `ping`s while it works).
    pub idle: Duration,
    /// The whole call, when set. A run loop sets it to the smaller of its
    /// tick budget and the run's remaining time, so one call cannot outlast
    /// either (run caps are checked only between steps).
    pub total: Option<Duration>,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            headers: Duration::from_secs(120),
            idle: Duration::from_secs(90),
            total: None,
        }
    }
}

/// `fut`, or a [`MessagesError::StreamCut`] naming `what` once `d` passes.
async fn deadline<T>(
    d: Duration,
    what: &str,
    fut: impl Future<Output = T>,
) -> Result<T, MessagesError> {
    clax_rt::time::timeout(d, fut)
        .await
        .map_err(|_| MessagesError::StreamCut {
            message: format!("no {what} within {} ms", d.as_millis()),
        })
}

/// `fut` within `total`, when set.
async fn bounded<T>(
    total: Option<Duration>,
    fut: impl Future<Output = Result<T, MessagesError>>,
) -> Result<T, MessagesError> {
    match total {
        Some(d) => deadline(d, "end of the call", fut).await?,
        None => fut.await,
    }
}

/// The next piece of `body` within `idle`; `None` at its end.
async fn next_chunk(body: &mut Body, idle: Duration) -> Result<Option<Vec<u8>>, MessagesError> {
    match deadline(idle, "data from the provider", body.next()).await? {
        Some(Ok(c)) => Ok(Some(c)),
        Some(Err(e)) => Err(MessagesError::StreamCut { message: e }),
        None => Ok(None),
    }
}

/// The whole of `body`, each piece within `idle`.
async fn read_all(mut body: Body, idle: Duration) -> Result<Vec<u8>, MessagesError> {
    let mut out = Vec::new();
    while let Some(c) = next_chunk(&mut body, idle).await? {
        out.extend_from_slice(&c);
    }
    Ok(out)
}

/// Folds an event-stream `body` into its message, each piece within `idle`.
async fn fold<F: FnMut(Delta)>(
    mut body: Body,
    idle: Duration,
    mut on_delta: F,
) -> Result<AssistantMessage, MessagesError> {
    let mut parser = SseParser::default();
    let mut asm = Assembler::default();
    while let Some(chunk) = next_chunk(&mut body, idle).await? {
        for ev in parser.push(&chunk) {
            for d in asm.apply(&ev.event, &ev.data)? {
                on_delta(d);
            }
            if asm.done() {
                return asm.finish();
            }
        }
    }
    asm.finish()
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A model `GET /v1/models` lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    /// RFC 3339, as sent.
    pub created_at: String,
}

/// The HTTP side. One per process is enough; keys and URLs go per call.
#[derive(Clone)]
pub struct MessagesClient {
    http: Transport,
    timeouts: Timeouts,
}

impl Default for MessagesClient {
    fn default() -> Self {
        MessagesClient::new()
    }
}

impl std::fmt::Debug for MessagesClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessagesClient")
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

fn endpoint(base_url: &str, path: &str) -> Result<String, MessagesError> {
    if !clax_core::config::base_url_ok(base_url) {
        return Err(MessagesError::BadBaseUrl);
    }
    Ok(format!("{}{path}", base_url.trim_end_matches('/')))
}

/// `s` with everything but unreserved characters percent-encoded.
fn query_value(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl MessagesClient {
    /// Default [`Timeouts`]; natively a 10 s connect timeout too.
    pub fn new() -> MessagesClient {
        MessagesClient {
            http: Transport::new(),
            timeouts: Timeouts::default(),
        }
    }

    pub fn with_timeouts(mut self, timeouts: Timeouts) -> MessagesClient {
        self.timeouts = timeouts;
        self
    }

    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// Sends one request and returns the body of a 2xx answer.
    async fn send(
        &self,
        method: &str,
        url: &str,
        key: &ApiKey,
        betas: Option<String>,
        json: Option<&Value>,
    ) -> Result<Body, MessagesError> {
        let mut headers = vec![
            ("x-api-key", key.expose().to_string()),
            ("anthropic-version", API_VERSION.to_string()),
        ];
        if let Some(b) = betas {
            headers.push(("anthropic-beta", b));
        }
        let res = deadline(
            self.timeouts.headers,
            "answer",
            self.http.send(method, url, &headers, json),
        )
        .await?
        .map_err(|message| MessagesError::Network { message })?;
        if res.success() {
            return Ok(res.body);
        }
        if res.redirect() {
            return Err(MessagesError::Redirected { status: res.status });
        }
        let body = read_all(res.body, self.timeouts.idle)
            .await
            .unwrap_or_default();
        Err(http_error(
            res.status,
            res.retry_after.as_deref(),
            &String::from_utf8_lossy(&body),
        ))
    }

    /// Streams one call, handing each [`Delta`] to `on_delta` as it arrives,
    /// and returns the assembled message.
    pub async fn stream<F: FnMut(Delta)>(
        &self,
        base_url: &str,
        key: &ApiKey,
        req: &MessagesRequest,
        on_delta: F,
    ) -> Result<AssistantMessage, MessagesError> {
        bounded(self.timeouts.total, async {
            let url = endpoint(base_url, "/v1/messages")?;
            let body = self
                .send("POST", &url, key, req.beta_header(), Some(&req.body()))
                .await?;
            fold(body, self.timeouts.idle, on_delta).await
        })
        .await
        .map_err(|e| e.redacted(key))
    }

    /// Every model the key may use, following `has_more`/`last_id` for at
    /// most [`MAX_MODEL_PAGES`] pages.
    pub async fn list_models(
        &self,
        base_url: &str,
        key: &ApiKey,
    ) -> Result<Vec<ModelInfo>, MessagesError> {
        bounded(
            self.timeouts.total,
            self.models(base_url, key, MODEL_PAGE_SIZE, MAX_MODEL_PAGES),
        )
        .await
        .map_err(|e| e.redacted(key))
    }

    /// Whether the key and URL work: one page of one model.
    pub async fn test_connection(&self, base_url: &str, key: &ApiKey) -> Result<(), MessagesError> {
        bounded(self.timeouts.total, self.models(base_url, key, 1, 1))
            .await
            .map(|_| ())
            .map_err(|e| e.redacted(key))
    }

    async fn models(
        &self,
        base_url: &str,
        key: &ApiKey,
        limit: u32,
        pages: usize,
    ) -> Result<Vec<ModelInfo>, MessagesError> {
        let url = endpoint(base_url, "/v1/models")?;
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..pages {
            let mut page = format!("{url}?limit={limit}");
            if let Some(a) = &after {
                page.push_str(&format!("&after_id={}", query_value(a)));
            }
            let body = self.send("GET", &page, key, None, None).await?;
            let bytes = read_all(body, self.timeouts.idle).await?;
            let v: Value = serde_json::from_slice(&bytes)
                .map_err(|e| protocol(format!("a model list that is not JSON: {e}")))?;
            let data = v["data"]
                .as_array()
                .ok_or_else(|| protocol("a model list without data"))?;
            for m in data {
                let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                if !s("id").is_empty() {
                    out.push(ModelInfo {
                        id: s("id"),
                        display_name: s("display_name"),
                        created_at: s("created_at"),
                    });
                }
            }
            match (v["has_more"].as_bool(), v["last_id"].as_str()) {
                (Some(true), Some(last)) if after.as_deref() != Some(last) => {
                    after = Some(last.to_string());
                }
                _ => break,
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Retries
// ---------------------------------------------------------------------------

/// Bounded exponential backoff: attempt `n` (from 0) that failed waits
/// `base * 2^n`, at most `cap`; a `retry-after` replaces that wait, and one
/// longer than `max_retry_after` ends the retries.
#[derive(Clone, Debug, PartialEq)]
pub struct RetryPolicy {
    /// Most attempts in all, the first included.
    pub max_attempts: u32,
    pub base: Duration,
    pub cap: Duration,
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(8),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// The wait before attempt `failed + 1`, or `None` to stop with `err`.
    pub fn delay(&self, failed: u32, err: &MessagesError) -> Option<Duration> {
        if !err.retryable() || failed + 1 >= self.max_attempts {
            return None;
        }
        if let MessagesError::RateLimited {
            retry_after: Some(d),
            ..
        } = err
        {
            return (*d <= self.max_retry_after).then_some(*d);
        }
        let factor = 1u32.checked_shl(failed).unwrap_or(u32::MAX);
        Some(self.base.saturating_mul(factor).min(self.cap))
    }
}

/// Runs `op` until it succeeds, fails with an error `policy` does not
/// retry, or runs out of attempts, awaiting `sleep(d)` between attempts.
pub async fn with_retry<T, Op, Fut, S, SFut>(
    policy: &RetryPolicy,
    mut sleep: S,
    mut op: Op,
) -> Result<T, MessagesError>
where
    Op: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, MessagesError>>,
    S: FnMut(Duration) -> SFut,
    SFut: Future<Output = ()>,
{
    let mut attempt = 0;
    loop {
        match op(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) => match policy.delay(attempt, &e) {
                Some(d) => {
                    sleep(d).await;
                    attempt += 1;
                }
                None => return Err(e),
            },
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

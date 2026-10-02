//! The provider behind `sample()` (spec D10: "provider is a trait"). A
//! provider turns one [`ProviderRequest`] (one round) into a stream of
//! [`ProviderEvent`]s ending in exactly one `End` or `Error`. Dropping the
//! stream cancels the request. The orchestration of rounds, page tools,
//! caching, and counting is not the provider's (see `sample::flight`).

use futures::Stream;
use serde_json::Value;
use std::pin::Pin;

pub type EventStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

pub trait SampleProvider: Send + Sync + 'static {
    /// `anthropic` or `stub`.
    fn name(&self) -> &'static str;
    /// Whether requests may carry [`Block::Image`].
    fn supports_images(&self) -> bool;
    /// Starts one round.
    fn stream(&self, req: ProviderRequest) -> EventStream;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderRequest {
    pub model: String,
    pub max_tokens: u32,
    /// The fixed framing (`sample::request::FRAMING`); pages never set it.
    pub system: String,
    pub messages: Vec<Turn>,
    pub tools: Vec<ToolSpec>,
    /// The last round of a call with tools: the model must answer, not call tools.
    pub final_round: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub content: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(String),
    /// Base64 image bytes.
    Image {
        media_type: String,
        data: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// One block of an earlier assistant turn exactly as the same provider
    /// produced it (`ProviderEvent::End.assistant`), passed back unchanged.
    Raw(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// The round ended; `assistant` is the round's content blocks, to be
    /// passed back as [`Block::Raw`] if another round follows.
    End {
        stop: Stop,
        assistant: Vec<Value>,
    },
    Error {
        code: ProviderErrorCode,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    MaxTokens,
    ToolUse,
    Refusal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderErrorCode {
    RateLimited,
    /// The key, the account, or the model cannot be used.
    Unavailable,
    InvalidRequest,
    PromptTooLarge,
    Upstream,
}

impl ProviderErrorCode {
    /// The `sample.d.ts` error code a page sees.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderErrorCode::RateLimited => "rate_limited",
            ProviderErrorCode::Unavailable => "sampling_disabled",
            ProviderErrorCode::InvalidRequest => "invalid_request",
            ProviderErrorCode::PromptTooLarge => "prompt_too_large",
            ProviderErrorCode::Upstream => "upstream_error",
        }
    }

    /// Parses the codes the stub's `[[error:<code>]]` accepts; anything else is `Upstream`.
    pub fn parse(code: &str) -> ProviderErrorCode {
        match code {
            "rate_limited" => ProviderErrorCode::RateLimited,
            "sampling_disabled" | "unavailable" => ProviderErrorCode::Unavailable,
            "invalid_request" => ProviderErrorCode::InvalidRequest,
            "prompt_too_large" => ProviderErrorCode::PromptTooLarge,
            _ => ProviderErrorCode::Upstream,
        }
    }
}

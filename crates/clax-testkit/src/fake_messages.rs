//! A scripted stand-in for the Anthropic Messages API, for ClaxBot's
//! daemon-backed tests: `POST /v1/messages` answers each request with the
//! next scripted turn (as the streaming format's server-sent events, or a
//! 400 once the script is spent), `GET /v1/models` lists two models.
//! Every request is kept ([`FakeMessages::requests`]). No model is called.

use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// One scripted assistant turn: its content blocks and stop reason.
#[derive(Clone, Debug)]
pub struct Turn {
    blocks: Vec<Value>,
    stop: &'static str,
}

impl Turn {
    /// A turn of narration alone (`end_turn`).
    pub fn text(text: &str) -> Turn {
        Turn {
            blocks: vec![json!({"type": "text", "text": text})],
            stop: "end_turn",
        }
    }

    /// A turn that narrates, then calls `run_script {title, code}`.
    pub fn script(id: &str, title: &str, code: &str) -> Turn {
        Turn {
            blocks: vec![
                json!({"type": "text", "text": format!("Running {title}.")}),
                json!({"type": "tool_use", "id": id, "name": "run_script",
                       "input": {"title": title, "code": code}}),
            ],
            stop: "tool_use",
        }
    }

    /// A turn that calls `finish {summary}`.
    pub fn finish(summary: &str) -> Turn {
        Turn {
            blocks: vec![
                json!({"type": "tool_use", "id": "tu_finish", "name": "finish",
                                "input": {"summary": summary}}),
            ],
            stop: "tool_use",
        }
    }

    /// The turn as the streaming format's events.
    fn sse(&self) -> String {
        let mut s = String::new();
        let mut ev = |name: &str, data: Value| {
            s.push_str(&format!("event: {name}\ndata: {data}\n\n"));
        };
        ev(
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_fake", "model": "claude-sonnet-5-5",
                "usage": {"input_tokens": 1000, "cache_read_input_tokens": 0,
                          "cache_creation_input_tokens": 0, "output_tokens": 1}}}),
        );
        for (i, b) in self.blocks.iter().enumerate() {
            if b["type"] == "text" {
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": i,
                           "content_block": {"type": "text", "text": ""}}),
                );
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": i,
                           "delta": {"type": "text_delta", "text": b["text"]}}),
                );
            } else {
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": i,
                           "content_block": {"type": "tool_use", "id": b["id"], "name": b["name"],
                                             "input": {}}}),
                );
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": i,
                           "delta": {"type": "input_json_delta",
                                     "partial_json": b["input"].to_string()}}),
                );
            }
            ev(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": i}),
            );
        }
        ev(
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": self.stop},
                   "usage": {"output_tokens": 200}}),
        );
        ev("message_stop", json!({"type": "message_stop"}));
        s
    }
}

#[derive(Default)]
struct Inner {
    turns: Mutex<VecDeque<Turn>>,
    seen: Mutex<Vec<Value>>,
}

/// The fake API, listening on a loopback port.
#[derive(Clone)]
pub struct FakeMessages {
    /// `http://127.0.0.1:<port>`: the base URL to configure.
    pub base: String,
    inner: Arc<Inner>,
}

async fn messages(State(i): State<Arc<Inner>>, body: axum::Json<Value>) -> Response {
    i.seen
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(body.0);
    let next = i
        .turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front();
    match next {
        Some(t) => ([("content-type", "text/event-stream")], t.sse()).into_response(),
        None => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(
                json!({"type": "error", "error": {"type": "invalid_request_error",
                                                          "message": "no scripted turn left"}}),
            ),
        )
            .into_response(),
    }
}

async fn models() -> axum::Json<Value> {
    axum::Json(json!({"data": [
        {"type": "model", "id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5",
         "created_at": "2026-09-01T00:00:00Z"},
        {"type": "model", "id": "claude-haiku-5-5", "display_name": "Claude Haiku 5.5",
         "created_at": "2026-09-01T00:00:00Z"},
    ], "has_more": false, "first_id": "claude-sonnet-5-5", "last_id": "claude-haiku-5-5"}))
}

impl FakeMessages {
    /// Starts the fake API with `turns` to answer, in order.
    pub async fn start(turns: Vec<Turn>) -> FakeMessages {
        let inner = Arc::new(Inner {
            turns: Mutex::new(turns.into()),
            seen: Mutex::default(),
        });
        let app = Router::new()
            .route("/v1/messages", post(messages))
            .route("/v1/models", get(models))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeMessages { base, inner }
    }

    /// Every `POST /v1/messages` body, in order.
    pub fn requests(&self) -> Vec<Value> {
        self.inner
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The scripted turns not yet answered.
    pub fn left(&self) -> usize {
        self.inner
            .turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

//! The Anthropic provider against a mocked Messages API on 127.0.0.1. No test
//! here reaches a real provider.

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use clax_server::sample::anthropic::AnthropicProvider;
use clax_server::sample::provider::*;
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

/// A mock answering every `POST /v1/messages` with `status` and `body`.
async fn mock(status: u16, body: String) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let app = Router::new().route(
        "/v1/messages",
        post(
            move |headers: HeaderMap, axum::Json(v): axum::Json<Value>| {
                let s = s.clone();
                let body = body.clone();
                async move {
                    s.lock().unwrap().push((headers, v));
                    (
                        StatusCode::from_u16(status).unwrap(),
                        [("content-type", "text/event-stream")],
                        body,
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn req(text: &str) -> ProviderRequest {
    ProviderRequest {
        model: "claude-sonnet-5-5".into(),
        max_tokens: 64,
        system: "frame".into(),
        messages: vec![Turn {
            role: Role::User,
            content: vec![Block::Text(text.into())],
        }],
        tools: vec![],
        final_round: true,
    }
}

async fn run(base: &str, key: &str, r: ProviderRequest) -> Vec<ProviderEvent> {
    AnthropicProvider::new(base, key).stream(r).collect().await
}

fn text_turn(parts: &[&str], stop: &str) -> String {
    let mut ev = vec![
        json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
    ];
    for p in parts {
        ev.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": p}}));
    }
    ev.push(json!({"type": "content_block_stop", "index": 0}));
    ev.push(json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 3}}));
    ev.push(json!({"type": "message_stop"}));
    sse(&ev)
}

#[tokio::test]
async fn streams_text_with_the_key_and_version_headers() {
    let (base, seen) = mock(200, text_turn(&["Hel", "lo"], "end_turn")).await;
    let out = run(&base, "sk-test", req("hi")).await;
    assert_eq!(
        out[..2],
        [
            ProviderEvent::Text("Hel".into()),
            ProviderEvent::Text("lo".into())
        ]
    );
    assert_eq!(
        out[2],
        ProviderEvent::End {
            stop: Stop::EndTurn,
            assistant: vec![json!({"type": "text", "text": "Hello"})]
        }
    );
    let (headers, body) = seen.lock().unwrap()[0].clone();
    assert_eq!(headers["x-api-key"], "sk-test");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "claude-sonnet-5-5");
    assert_eq!(body["system"], "frame");
    assert_eq!(
        body["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}])
    );
}

#[tokio::test]
async fn tool_calls_arrive_parsed_with_the_turn_to_echo() {
    let body = sse(&[
        json!({"type": "message_start", "message": {"id": "msg_1"}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQB"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "getColor", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"shade\":"}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": " \"dark\"}"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        json!({"type": "message_stop"}),
    ]);
    let (base, _) = mock(200, body).await;
    let out = run(&base, "k", req("x")).await;
    assert_eq!(
        out[0],
        ProviderEvent::ToolUse {
            id: "toolu_1".into(),
            name: "getColor".into(),
            input: json!({"shade": "dark"})
        }
    );
    let ProviderEvent::End { stop, assistant } = &out[1] else {
        panic!("{out:?}")
    };
    assert_eq!(*stop, Stop::ToolUse);
    assert_eq!(
        assistant[0],
        json!({"type": "thinking", "thinking": "", "signature": "EqQB"})
    );
}

#[tokio::test]
async fn http_errors_become_one_error_event() {
    for (status, kind, code) in [
        (429, "rate_limit_error", ProviderErrorCode::RateLimited),
        (401, "authentication_error", ProviderErrorCode::Unavailable),
        (529, "overloaded_error", ProviderErrorCode::Upstream),
    ] {
        let (base, _) = mock(
            status,
            json!({"type": "error", "error": {"type": kind, "message": "m sk-live"}}).to_string(),
        )
        .await;
        let out = run(&base, "sk-live", req("x")).await;
        assert_eq!(out.len(), 1, "{status}");
        let ProviderEvent::Error { code: got, message } = &out[0] else {
            panic!("{out:?}")
        };
        assert_eq!(*got, code);
        assert!(!message.contains("sk-live"));
    }
}

#[tokio::test]
async fn an_error_mid_stream_follows_the_text_already_sent() {
    let body = format!(
        "{}{}",
        sse(&[
            json!({"type": "message_start", "message": {"id": "m"}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "par"}}),
        ]),
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"
    );
    let (base, _) = mock(200, body).await;
    let out = run(&base, "k", req("x")).await;
    assert_eq!(
        out,
        vec![
            ProviderEvent::Text("par".into()),
            ProviderEvent::Error {
                code: ProviderErrorCode::Upstream,
                message: "Overloaded".into()
            },
        ]
    );
}

#[tokio::test]
async fn refusal_and_length_stops_are_reported() {
    let (base, _) = mock(200, text_turn(&["I can"], "refusal")).await;
    assert!(matches!(
        run(&base, "k", req("x")).await.last(),
        Some(ProviderEvent::End {
            stop: Stop::Refusal,
            ..
        })
    ));
    let (base, _) = mock(200, text_turn(&["cut"], "max_tokens")).await;
    assert!(matches!(
        run(&base, "k", req("x")).await.last(),
        Some(ProviderEvent::End {
            stop: Stop::MaxTokens,
            ..
        })
    ));
}

#[tokio::test]
async fn a_stream_that_stops_early_or_an_unreachable_host_is_upstream_error() {
    let (base, _) = mock(
        200,
        sse(&[json!({"type": "message_start", "message": {"id": "m"}})]),
    )
    .await;
    assert!(matches!(
        run(&base, "k", req("x")).await.as_slice(),
        [ProviderEvent::Error {
            code: ProviderErrorCode::Upstream,
            ..
        }]
    ));
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    assert!(matches!(
        run(&closed, "k", req("x")).await.as_slice(),
        [ProviderEvent::Error {
            code: ProviderErrorCode::Upstream,
            ..
        }]
    ));
}

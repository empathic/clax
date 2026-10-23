//! The client against a scripted HTTP/1.1 server on 127.0.0.1 (raw TCP, so
//! a test controls how the body is cut). No test reaches a real provider.

use super::*;
use crate::sse::SseParser;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const KEY: &str = "sk-ant-test-0123456789";

enum Reply {
    /// The whole body, written `piece` bytes at a time, then the connection closes.
    Full {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        piece: usize,
    },
    /// A chunked 200 whose connection closes after `body`, with no last chunk.
    Cut { body: Vec<u8> },
    /// Reads the request and never answers, holding the connection open.
    Stall,
}

fn ok(body: impl Into<Vec<u8>>, piece: usize) -> Reply {
    Reply::Full {
        status: 200,
        headers: vec![("content-type", "text/event-stream".into())],
        body: body.into(),
        piece,
    }
}

fn json_reply(status: u16, body: Value) -> Reply {
    Reply::Full {
        status,
        headers: vec![("content-type", "application/json".into())],
        body: body.to_string().into_bytes(),
        piece: usize::MAX,
    }
}

#[derive(Clone, Debug)]
struct Seen {
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// The request on `sock`; `None` when the client gave up before sending it all.
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = sock.read(&mut tmp).await.ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let target = lines.next().unwrap().split(' ').nth(1).unwrap().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut tmp).await.ok().filter(|n| *n > 0)?;
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Seen {
        target,
        headers,
        body,
    })
}

/// Answers one connection per reply, in order, and logs each request.
async fn serve(replies: Vec<Reply>) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let l = log.clone();
    tokio::spawn(async move {
        for reply in replies {
            let (mut sock, _) = listener.accept().await.unwrap();
            let Some(seen) = read_request(&mut sock).await else {
                continue;
            };
            l.lock().unwrap().push(seen);
            match reply {
                Reply::Full {
                    status,
                    headers,
                    body,
                    piece,
                } => {
                    let mut head = format!("HTTP/1.1 {status} X\r\nconnection: close\r\n");
                    for (k, v) in headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    sock.write_all(head.as_bytes()).await.unwrap();
                    for p in body.chunks(piece.max(1)) {
                        sock.write_all(p).await.unwrap();
                        sock.flush().await.unwrap();
                    }
                }
                Reply::Cut { body } => {
                    let head = "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
                    sock.write_all(head.as_bytes()).await.unwrap();
                    let chunk = format!("{:x}\r\n", body.len());
                    sock.write_all(chunk.as_bytes()).await.unwrap();
                    sock.write_all(&body).await.unwrap();
                    sock.write_all(b"\r\n").await.unwrap();
                    sock.flush().await.unwrap();
                }
                Reply::Stall => {
                    tokio::spawn(async move {
                        let _held = sock;
                        std::future::pending::<()>().await;
                    });
                    continue;
                }
            }
            let _ = sock.shutdown().await;
        }
    });
    (base, log)
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

/// A tool-use turn: thinking, text with a multibyte character, a tool call
/// whose input arrives in three pieces, with cache usage.
fn tool_turn() -> Vec<Value> {
    vec![
        json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-sonnet-5-5", "content": [],
            "usage": {"input_tokens": 12, "cache_creation_input_tokens": 3400, "cache_read_input_tokens": 51000, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Need the "}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "draft."}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQBCkYIARgCKkD=="}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "ping"}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Reading the café "}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "page ☕"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "run_script", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": ""}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"title\": \"Read ☕\", "}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"code\": \"return 1\"}"}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 87}}),
        json!({"type": "message_stop"}),
    ]
}

fn request() -> MessagesRequest {
    MessagesRequest {
        model: "claude-sonnet-5-5".into(),
        max_tokens: 8000,
        system: vec![SystemBlock {
            text: "You are ClaxBot.".into(),
            cache_control: Some(CacheControl::default()),
        }],
        messages: vec![Message::user(vec![Block::text("Fix the page")])],
        tools: vec![Tool {
            name: "run_script".into(),
            description: "Run code".into(),
            input_schema: json!({"type": "object", "properties": {"title": {"type": "string"}, "code": {"type": "string"}}, "required": ["title", "code"]}),
            cache_control: None,
            eager_input_streaming: false,
        }],
        tool_choice: Some(ToolChoice::Auto {
            disable_parallel_tool_use: false,
        }),
        thinking: Some(Thinking {
            display: Some("summarized".into()),
        }),
        effort: Some("medium".into()),
        betas: vec![],
        extra: Map::new(),
    }
}

fn key() -> ApiKey {
    ApiKey::new(KEY)
}

async fn call(
    base: &str,
    req: &MessagesRequest,
) -> (Result<AssistantMessage, MessagesError>, Vec<Delta>) {
    let mut deltas = Vec::new();
    let out = MessagesClient::new()
        .stream(base, &key(), req, |d| deltas.push(d))
        .await;
    (out, deltas)
}

fn expected_content() -> Vec<Block> {
    vec![
        Block::Thinking {
            thinking: "Need the draft.".into(),
            signature: "EqQBCkYIARgCKkD==".into(),
        },
        Block::text("Reading the café page ☕"),
        Block::ToolUse {
            id: "toolu_1".into(),
            name: "run_script".into(),
            input: json!({"title": "Read ☕", "code": "return 1"}),
        },
    ]
}

#[tokio::test]
async fn a_tool_use_turn_streams_in_small_pieces_into_deltas_and_one_message() {
    let (base, log) = serve(vec![ok(sse(&tool_turn()), 3)]).await;
    let req = request();
    let (out, deltas) = call(&base, &req).await;
    let msg = out.unwrap();
    assert_eq!(msg.id, "msg_1");
    assert_eq!(msg.model, "claude-sonnet-5-5");
    assert_eq!(msg.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(msg.stop_details, None);
    assert_eq!(
        msg.usage,
        Usage {
            input_tokens: 12,
            cache_creation_input_tokens: 3400,
            cache_read_input_tokens: 51000,
            output_tokens: 87,
        }
    );
    assert_eq!(msg.content, expected_content());
    assert_eq!(
        msg.tool_calls(),
        vec![ToolCall::Valid {
            id: "toolu_1".into(),
            name: "run_script".into(),
            input: json!({"title": "Read ☕", "code": "return 1"})
        }]
    );

    let texts: String = deltas
        .iter()
        .filter_map(|d| match d {
            Delta::Text { index: 1, text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, "Reading the café page ☕");
    let input: String = deltas
        .iter()
        .filter_map(|d| match d {
            Delta::ToolInput {
                index: 2,
                partial_json,
            } => Some(partial_json.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(input, "{\"title\": \"Read ☕\", \"code\": \"return 1\"}");
    assert!(
        matches!(&deltas[0], Delta::MessageStart { id, usage, .. } if id == "msg_1" && usage.cache_read_input_tokens == 51000)
    );
    assert!(deltas.contains(&Delta::BlockStop {
        index: 2,
        block: expected_content()[2].clone()
    }));
    assert!(matches!(
        deltas.last(),
        Some(Delta::MessageDelta { stop_reason: Some(StopReason::ToolUse), usage }) if usage.output_tokens == 87
    ));

    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.target, "/v1/messages");
    assert_eq!(seen.header("x-api-key"), Some(KEY));
    assert_eq!(seen.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(seen.json(), req.body());
    assert_eq!(
        seen.json(),
        json!({
            "model": "claude-sonnet-5-5", "max_tokens": 8000, "stream": true,
            "system": [{"type": "text", "text": "You are ClaxBot.", "cache_control": {"type": "ephemeral"}}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "Fix the page"}]}],
            "tools": [{"name": "run_script", "description": "Run code", "input_schema": {"type": "object", "properties": {"title": {"type": "string"}, "code": {"type": "string"}}, "required": ["title", "code"]}}],
            "tool_choice": {"type": "auto"},
            "thinking": {"type": "adaptive", "display": "summarized"},
            "output_config": {"effort": "medium"}
        })
    );
}

#[test]
fn the_stream_assembles_the_same_at_every_byte_boundary() {
    let body = sse(&tool_turn()).into_bytes();
    for size in 1..=13 {
        let mut parser = SseParser::default();
        let mut asm = Assembler::default();
        for chunk in body.chunks(size) {
            for ev in parser.push(chunk) {
                asm.apply(&ev.event, &ev.data).unwrap();
            }
        }
        let msg = asm.finish().unwrap();
        assert_eq!(msg.content, expected_content(), "chunk size {size}");
        assert_eq!(msg.usage.output_tokens, 87, "chunk size {size}");
    }
}

#[test]
fn assistant_blocks_round_trip_verbatim_into_the_next_request() {
    let cited = json!({"type": "text", "text": "Per the doc", "citations": [{"type": "char_location", "cited_text": "x", "document_index": 0, "start_char_index": 0, "end_char_index": 1}]});
    let events = vec![
        json!({"type": "message_start", "message": {"id": "msg_2", "model": "m", "usage": {"input_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig-omitted"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": "", "citations": []}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Per the doc"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "citations_delta", "citation": cited["citations"][0]}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "content_block_start", "index": 3, "content_block": {"type": "future_block", "payload": {"a": [1, 2]}}}),
        json!({"type": "content_block_stop", "index": 3}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}),
        json!({"type": "message_stop"}),
    ];
    let mut asm = Assembler::default();
    for e in &events {
        asm.apply(e["type"].as_str().unwrap(), &e.to_string())
            .unwrap();
    }
    let msg = asm.finish().unwrap();
    assert_eq!(
        msg.content[..2],
        [
            Block::Thinking {
                thinking: String::new(),
                signature: "sig-omitted".into()
            },
            Block::RedactedThinking {
                data: "EmwKAhgBEgy3va3pzix".into()
            },
        ]
    );
    assert!(
        matches!(msg.content[2], Block::Raw(_)),
        "{:?}",
        msg.content[2]
    );
    let mut next = request();
    next.messages.push(msg.to_message());
    next.messages.push(Message::user(vec![Block::tool_result(
        "toolu_9", "teal", true,
    )]));
    assert_eq!(
        next.body()["messages"][1],
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "", "signature": "sig-omitted"},
            {"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix"},
            cited,
            {"type": "future_block", "payload": {"a": [1, 2]}},
        ]})
    );
    assert_eq!(
        next.body()["messages"][2],
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_9", "content": "teal", "is_error": true}]})
    );
}

#[test]
fn a_refusal_keeps_its_stop_details_and_unknown_reasons_survive() {
    let mut asm = Assembler::default();
    for e in [
        json!({"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}}}),
        json!({"type": "message_stop"}),
    ] {
        asm.apply("x", &e.to_string()).unwrap();
    }
    let msg = asm.finish().unwrap();
    assert_eq!(msg.stop_reason, Some(StopReason::Refusal));
    assert_eq!(msg.stop_details.unwrap()["category"], "cyber");
    assert_eq!(
        StopReason::parse("new_reason"),
        StopReason::Other("new_reason".into())
    );
}

fn error_body(kind: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_1"})
}

#[tokio::test]
async fn every_http_error_maps_to_its_class() {
    let mut retry = json_reply(429, error_body("rate_limit_error", "slow down"));
    if let Reply::Full { headers, .. } = &mut retry {
        headers.push(("retry-after", "7".into()));
    }
    let (base, _) = serve(vec![
        json_reply(401, error_body("authentication_error", "invalid x-api-key")),
        json_reply(403, error_body("permission_error", "not allowed")),
        retry,
        json_reply(529, error_body("overloaded_error", "Overloaded")),
        json_reply(
            400,
            error_body("invalid_request_error", "messages: roles must alternate"),
        ),
        json_reply(500, error_body("api_error", "Internal")),
        json_reply(404, error_body("not_found_error", "model: nope")),
        Reply::Full {
            status: 502,
            headers: vec![],
            body: b"<html>bad gateway</html>".to_vec(),
            piece: usize::MAX,
        },
    ])
    .await;
    let req = request();
    let mut got = Vec::new();
    for _ in 0..8 {
        got.push(call(&base, &req).await.0.unwrap_err());
    }
    assert!(
        matches!(&got[0], MessagesError::Auth { status: 401, .. }),
        "{:?}",
        got[0]
    );
    assert!(
        matches!(&got[1], MessagesError::Auth { status: 403, .. }),
        "{:?}",
        got[1]
    );
    assert_eq!(
        got[2],
        MessagesError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
            message: "slow down".into()
        }
    );
    assert_eq!(
        got[3],
        MessagesError::Overloaded {
            message: "Overloaded".into()
        }
    );
    assert_eq!(
        got[4],
        MessagesError::InvalidRequest {
            message: "messages: roles must alternate".into()
        }
    );
    assert!(matches!(&got[5], MessagesError::Api { status: 500, kind, .. } if kind == "api_error"));
    assert!(
        matches!(&got[6], MessagesError::Api { status: 404, kind, .. } if kind == "not_found_error")
    );
    assert!(
        matches!(&got[7], MessagesError::Api { status: 502, message, .. } if message.contains("HTTP 502"))
    );
    let retryable: Vec<bool> = got.iter().map(MessagesError::retryable).collect();
    assert_eq!(
        retryable,
        [false, false, true, true, false, true, false, true]
    );
}

#[tokio::test]
async fn the_key_never_appears_in_an_error() {
    let echo = format!("invalid x-api-key: {KEY}");
    let (base, _) = serve(vec![
        json_reply(401, error_body("authentication_error", &echo)),
        ok(
            sse(&[json!({"type": "error", "error": {"type": "api_error", "message": echo}})]),
            5,
        ),
        json_reply(401, error_body("authentication_error", &echo)),
    ])
    .await;
    let req = request();
    let mut errors = vec![
        call(&base, &req).await.0.unwrap_err(),
        call(&base, &req).await.0.unwrap_err(),
        MessagesClient::new()
            .list_models(&base, &key())
            .await
            .unwrap_err(),
    ];
    // A real transport error, whose message names the URL (here holding the key).
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = format!("http://{}/{KEY}", closed.local_addr().unwrap());
    drop(closed);
    let network = call(&dead, &req).await.0.unwrap_err();
    assert!(
        matches!(network, MessagesError::Network { .. }),
        "{network:?}"
    );
    errors.push(network);
    errors.push(
        MessagesError::Api {
            status: 0,
            kind: KEY.into(),
            message: KEY.into(),
        }
        .redacted(&key()),
    );
    for e in &errors {
        for shown in [format!("{e}"), format!("{e:?}")] {
            assert!(!shown.contains(KEY), "{shown}");
            assert!(shown.contains("[redacted]"), "{shown}");
        }
    }
    assert_eq!(format!("{:?}", key()), "ApiKey([redacted])");
    assert!(!format!("{:?}", MessagesClient::new()).contains(KEY));
}

#[tokio::test]
async fn an_error_event_mid_stream_is_classified() {
    let mut events = tool_turn()[..4].to_vec();
    events.push(
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    );
    let (base, _) = serve(vec![ok(sse(&events), 7)]).await;
    let (out, deltas) = call(&base, &request()).await;
    assert_eq!(
        out.unwrap_err(),
        MessagesError::Overloaded {
            message: "Overloaded".into()
        }
    );
    assert!(deltas.iter().any(|d| matches!(d, Delta::Thinking { .. })));
}

#[tokio::test]
async fn a_stream_cut_mid_way_is_a_stream_cut() {
    let half = sse(&tool_turn()[..9]);
    // Cut inside an event, and with the transfer encoding unfinished.
    let (base, _) = serve(vec![
        ok(
            format!("{half}event: content_block_delta\ndata: {{\"type\": \"content"),
            4,
        ),
        Reply::Cut {
            body: half.clone().into_bytes(),
        },
    ])
    .await;
    for _ in 0..2 {
        let (out, deltas) = call(&base, &request()).await;
        let err = out.unwrap_err();
        assert!(matches!(err, MessagesError::StreamCut { .. }), "{err:?}");
        assert!(err.retryable());
        assert!(deltas.iter().any(|d| matches!(d, Delta::Text { .. })));
    }
}

#[tokio::test]
async fn an_unreachable_provider_is_a_network_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let err = call(&base, &request()).await.0.unwrap_err();
    assert!(matches!(err, MessagesError::Network { .. }), "{err:?}");
    assert!(err.retryable());
}

#[tokio::test]
async fn a_base_url_the_sample_rule_refuses_is_never_called() {
    for bad in [
        "http://example.com",
        "ftp://127.0.0.1",
        "https://user:pw@api.example.com",
        "",
    ] {
        let err = call(bad, &request()).await.0.unwrap_err();
        assert_eq!(err, MessagesError::BadBaseUrl, "{bad}");
        let err = MessagesClient::new()
            .list_models(bad, &key())
            .await
            .unwrap_err();
        assert_eq!(err, MessagesError::BadBaseUrl, "{bad}");
    }
}

#[tokio::test]
async fn list_models_follows_pages_and_test_connection_asks_for_one() {
    let page1 = json!({"data": [
        {"type": "model", "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "created_at": "2026-09-01T00:00:00Z"},
        {"type": "model", "id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5", "created_at": "2026-08-01T00:00:00Z"}],
        "has_more": true, "first_id": "claude-opus-5-5", "last_id": "claude-sonnet-5-5"});
    let page2 = json!({"data": [
        {"type": "model", "id": "claude-haiku-5-5", "display_name": "Claude Haiku 5.5", "created_at": "2026-07-01T00:00:00Z"}],
        "has_more": false, "first_id": "claude-haiku-5-5", "last_id": "claude-haiku-5-5"});
    let (base, log) = serve(vec![
        json_reply(200, page1.clone()),
        json_reply(200, page2),
        json_reply(200, page1),
        json_reply(401, error_body("authentication_error", "invalid x-api-key")),
    ])
    .await;
    let client = MessagesClient::new();
    let models = client
        .list_models(&format!("{base}/"), &key())
        .await
        .unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]
    );
    assert_eq!(
        models[0],
        ModelInfo {
            id: "claude-opus-5-5".into(),
            display_name: "Claude Opus 5.5".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
        }
    );
    client.test_connection(&base, &key()).await.unwrap();
    let err = client.test_connection(&base, &key()).await.unwrap_err();
    assert!(
        matches!(err, MessagesError::Auth { status: 401, .. }),
        "{err:?}"
    );
    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen.iter().map(|s| s.target.as_str()).collect::<Vec<_>>(),
        [
            "/v1/models?limit=100",
            "/v1/models?limit=100&after_id=claude-sonnet-5-5",
            "/v1/models?limit=1",
            "/v1/models?limit=1",
        ]
    );
    assert!(seen.iter().all(|s| s.header("x-api-key") == Some(KEY)));
}

#[test]
fn the_retry_policy_is_bounded_and_honours_retry_after() {
    let p = RetryPolicy {
        max_attempts: 4,
        base: Duration::from_millis(100),
        cap: Duration::from_millis(250),
        max_retry_after: Duration::from_secs(30),
    };
    let over = MessagesError::Overloaded {
        message: String::new(),
    };
    assert_eq!(p.delay(0, &over), Some(Duration::from_millis(100)));
    assert_eq!(p.delay(1, &over), Some(Duration::from_millis(200)));
    assert_eq!(p.delay(2, &over), Some(Duration::from_millis(250)));
    assert_eq!(p.delay(3, &over), None);
    let limited = |s| MessagesError::RateLimited {
        retry_after: Some(Duration::from_secs(s)),
        message: String::new(),
    };
    assert_eq!(p.delay(0, &limited(9)), Some(Duration::from_secs(9)));
    assert_eq!(p.delay(0, &limited(31)), None);
    assert_eq!(
        p.delay(
            0,
            &MessagesError::InvalidRequest {
                message: String::new()
            }
        ),
        None
    );
    assert_eq!(p.delay(40, &over), None);
    let wide = RetryPolicy {
        max_attempts: u32::MAX,
        ..p
    };
    assert_eq!(wide.delay(40, &over), Some(Duration::from_millis(250)));
}

#[tokio::test]
async fn with_retry_waits_through_the_injected_sleeper() {
    let policy = RetryPolicy::default();
    let slept = Arc::new(Mutex::new(Vec::new()));
    let sleeper = |s: Arc<Mutex<Vec<Duration>>>| {
        move |d: Duration| {
            s.lock().unwrap().push(d);
            std::future::ready(())
        }
    };
    let script = Arc::new(Mutex::new(vec![
        Err(MessagesError::RateLimited {
            retry_after: Some(Duration::from_secs(2)),
            message: String::new(),
        }),
        Err(MessagesError::Overloaded {
            message: String::new(),
        }),
        Ok(42),
    ]));
    let s = script.clone();
    let out = with_retry(&policy, sleeper(slept.clone()), move |_| {
        let next = s.lock().unwrap().remove(0);
        std::future::ready(next)
    })
    .await;
    assert_eq!(out, Ok(42));
    assert_eq!(
        *slept.lock().unwrap(),
        [Duration::from_secs(2), Duration::from_secs(1)]
    );

    let slept = Arc::new(Mutex::new(Vec::new()));
    let mut attempts = Vec::new();
    let out: Result<(), _> = with_retry(&policy, sleeper(slept.clone()), |n| {
        attempts.push(n);
        std::future::ready(Err(MessagesError::Network {
            message: "down".into(),
        }))
    })
    .await;
    assert!(matches!(out, Err(MessagesError::Network { .. })));
    assert_eq!(attempts, [0, 1, 2]);
    assert_eq!(slept.lock().unwrap().len(), 2);

    let out: Result<(), _> = with_retry(
        &policy,
        |_| async { panic!("no wait") },
        |_| {
            std::future::ready(Err(MessagesError::Auth {
                status: 401,
                message: String::new(),
            }))
        },
    )
    .await;
    assert!(matches!(out, Err(MessagesError::Auth { .. })));
}

/// A server whose every answer redirects to a second server, and the log
/// of that second one, which would answer if a client followed.
async fn redirect_to_trap(status: u16, n: usize) -> (String, Log, Log) {
    let (trap, trap_log) = serve((0..n).map(|_| ok(sse(&tool_turn()), usize::MAX)).collect()).await;
    let (base, log) = serve(
        (0..n)
            .map(|_| Reply::Full {
                status,
                headers: vec![("location", format!("{trap}/v1/messages"))],
                body: Vec::new(),
                piece: usize::MAX,
            })
            .collect(),
    )
    .await;
    (base, log, trap_log)
}

#[tokio::test]
async fn a_redirect_is_never_followed_so_the_key_stays_with_the_base_url() {
    let (base, log, trap) = redirect_to_trap(307, 2).await;
    let err = call(&base, &request()).await.0.unwrap_err();
    assert_eq!(err, MessagesError::Redirected { status: 307 });
    assert!(!err.retryable());
    let err = MessagesClient::new()
        .list_models(&base, &key())
        .await
        .unwrap_err();
    assert_eq!(err, MessagesError::Redirected { status: 307 });
    assert_eq!(log.lock().unwrap().len(), 2);
    assert!(
        trap.lock().unwrap().is_empty(),
        "the redirect target was called"
    );

    let (base, _, trap) = redirect_to_trap(302, 1).await;
    let err = MessagesClient::new()
        .test_connection(&base, &key())
        .await
        .unwrap_err();
    assert_eq!(err, MessagesError::Redirected { status: 302 });
    assert!(trap.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_sample_provider_never_follows_a_redirect_either() {
    use crate::provider::{ProviderErrorCode, ProviderEvent, ProviderRequest, SampleProvider};
    let (base, log, trap) = redirect_to_trap(308, 1).await;
    let req = ProviderRequest {
        model: "claude-sonnet-5-5".into(),
        max_tokens: 64,
        system: "frame".into(),
        messages: vec![],
        tools: vec![],
        final_round: true,
    };
    let out: Vec<ProviderEvent> = crate::anthropic::AnthropicProvider::new(&base, KEY)
        .stream(req)
        .collect()
        .await;
    assert!(
        matches!(&out[..], [ProviderEvent::Error { code: ProviderErrorCode::Upstream, message }] if message.contains("308")),
        "{out:?}"
    );
    assert_eq!(log.lock().unwrap()[0].header("x-api-key"), Some(KEY));
    assert!(
        trap.lock().unwrap().is_empty(),
        "the redirect target was called"
    );
}

fn quick(headers: u64, idle: u64, total: Option<u64>) -> MessagesClient {
    MessagesClient::new().with_timeouts(Timeouts {
        headers: Duration::from_secs(headers),
        idle: Duration::from_secs(idle),
        total: total.map(Duration::from_secs),
    })
}

#[tokio::test(start_paused = true)]
async fn an_answer_that_never_comes_is_cut_at_the_header_deadline() {
    // Shorter than the 10 s connect timeout, so it is the first to pass.
    let (base, _) = serve(vec![Reply::Stall, Reply::Stall]).await;
    let client = quick(5, 60, None);
    let mut deltas = Vec::new();
    let err = client
        .stream(&base, &key(), &request(), |d| deltas.push(d))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MessagesError::StreamCut { message } if message.contains("answer")),
        "{err:?}"
    );
    assert!(err.retryable());
    assert!(deltas.is_empty());
    let err = client.list_models(&base, &key()).await.unwrap_err();
    assert!(matches!(err, MessagesError::StreamCut { .. }), "{err:?}");
}

fn chunks_then_silence(events: &[Value]) -> Body {
    let pieces: Vec<Result<Vec<u8>, String>> = vec![Ok(sse(events).into_bytes())];
    Box::pin(futures::stream::iter(pieces).chain(futures::stream::pending()))
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_goes_quiet_is_cut_at_the_idle_deadline() {
    let start = tokio::time::Instant::now();
    let mut deltas = Vec::new();
    let err = fold(
        chunks_then_silence(&tool_turn()[..9]),
        Duration::from_secs(30),
        |d| deltas.push(d),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, MessagesError::StreamCut { message } if message.contains("data")),
        "{err:?}"
    );
    assert_eq!(start.elapsed(), Duration::from_secs(30));
    assert!(deltas.iter().any(|d| matches!(d, Delta::Text { .. })));
}

#[tokio::test(start_paused = true)]
async fn pings_keep_a_stream_alive_until_the_total_cap() {
    let start = tokio::time::Instant::now();
    // A ping every 10 s of virtual time, forever: never quiet for the 30 s
    // idle limit, so only the 95 s cap ends it.
    let ping = sse(&[json!({"type": "ping"})]).into_bytes();
    let pings: Body = Box::pin(futures::stream::unfold(ping, |p| async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Some((Ok(p.clone()), p))
    }));
    let err = bounded(
        Some(Duration::from_secs(95)),
        fold(pings, Duration::from_secs(30), |_| {}),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, MessagesError::StreamCut { message } if message.contains("end of the call")),
        "{err:?}"
    );
    assert_eq!(start.elapsed(), Duration::from_secs(95));
}

fn apply_all(events: &[Value]) -> Result<AssistantMessage, MessagesError> {
    let mut asm = Assembler::default();
    for e in events {
        asm.apply(e["type"].as_str().unwrap(), &e.to_string())?;
    }
    asm.finish()
}

#[test]
fn input_streams_into_any_block_that_has_one_and_unknown_deltas_fail() {
    let msg = apply_all(&[
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"query\": \"cl"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "ax\"}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "mcp_tool_use", "id": "mcptoolu_1", "name": "read", "server_name": "s", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"path\": \"/a\"}"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_stop"}),
    ])
    .unwrap();
    assert_eq!(
        msg.content,
        [
            Block::Raw(
                json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "clax"}})
            ),
            Block::Raw(
                json!({"type": "mcp_tool_use", "id": "mcptoolu_1", "name": "read", "server_name": "s", "input": {"path": "/a"}})
            ),
        ]
    );
    assert_eq!(
        msg.tool_calls().len(),
        0,
        "server tool calls are not the client's"
    );

    let start = json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}});
    for bad in [
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
        json!({"type": "content_block_delta", "index": 3, "delta": {"type": "text_delta", "text": "x"}}),
        json!({"type": "content_block_delta", "delta": {"type": "text_delta", "text": "x"}}),
        json!({"type": "content_block_stop"}),
    ] {
        let err = apply_all(&[start.clone(), bad.clone()]).unwrap_err();
        assert!(
            matches!(err, MessagesError::Protocol { .. }),
            "{bad}: {err:?}"
        );
    }
    let err = apply_all(&[
        start.clone(),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "new_delta", "x": 1}}),
    ])
    .unwrap_err();
    assert_eq!(
        err,
        MessagesError::UnknownEvent {
            kind: "content_block_delta/new_delta".into()
        }
    );
    assert!(!err.retryable());
    assert!(err.to_string().contains("new_delta"));
    let err = apply_all(&[
        json!({"type": "content_block_start", "content_block": {"type": "text", "text": ""}}),
    ])
    .unwrap_err();
    assert!(
        matches!(&err, MessagesError::Protocol { message } if message.contains("index")),
        "{err:?}"
    );
}

#[test]
fn a_tool_call_with_an_extra_field_stays_raw_and_is_still_a_tool_call() {
    let msg = apply_all(&[
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_7", "name": "run_script", "input": {}, "caller": {"type": "direct"}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"code\": \"1\"}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        json!({"type": "message_stop"}),
    ])
    .unwrap();
    assert!(matches!(&msg.content[0], Block::Raw(v) if v["caller"]["type"] == "direct"));
    assert_eq!(
        msg.tool_calls(),
        [ToolCall::Valid {
            id: "toolu_7".into(),
            name: "run_script".into(),
            input: json!({"code": "1"})
        }]
    );
}

#[test]
fn input_that_does_not_parse_is_listed_for_an_error_result() {
    let msg = apply_all(&[
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_8", "name": "run_script", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"code\": \"ret"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_9", "name": "finish", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "[1]"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}}),
        json!({"type": "message_stop"}),
    ])
    .unwrap();
    let calls = msg.tool_calls();
    let got: Vec<(&str, &str, &str)> = calls
        .iter()
        .map(|c| match c {
            ToolCall::Invalid { id, name, raw, .. } => (id.as_str(), name.as_str(), raw.as_str()),
            ToolCall::Valid { .. } => panic!("{c:?}"),
        })
        .collect();
    assert_eq!(
        got,
        [
            ("toolu_8", "run_script", "{\"code\": \"ret"),
            ("toolu_9", "finish", "[1]")
        ]
    );
    assert!(
        matches!(&calls[1], ToolCall::Invalid { error, .. } if error.contains("not a JSON object"))
    );
    // The answer: an is_error result whose content is {"INVALID_JSON": raw}, as JSON.
    let Some(Block::ToolResult {
        tool_use_id,
        content,
        is_error,
        ..
    }) = calls[0].invalid_result()
    else {
        panic!()
    };
    assert_eq!((tool_use_id.as_str(), is_error), ("toolu_8", true));
    let parsed: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed, json!({"INVALID_JSON": "{\"code\": \"ret"}));
    // The turn passed back still has an object input for the API.
    assert_eq!(msg.to_message().to_json()["content"][0]["input"], json!({}));
    assert_eq!((calls[0].id(), calls[0].name()), ("toolu_8", "run_script"));
}

#[test]
fn a_block_that_starts_with_its_input_keeps_it_when_no_delta_follows() {
    let msg = apply_all(&[
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srvtoolu_2", "name": "web_search", "input": {"query": "clax"}}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_3", "name": "finish", "input": {"summary": "done"}}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_stop"}),
    ])
    .unwrap();
    assert_eq!(
        msg.content[0],
        Block::Raw(
            json!({"type": "server_tool_use", "id": "srvtoolu_2", "name": "web_search", "input": {"query": "clax"}})
        )
    );
    assert_eq!(
        msg.tool_calls(),
        [ToolCall::Valid {
            id: "toolu_3".into(),
            name: "finish".into(),
            input: json!({"summary": "done"})
        }]
    );
    assert!(msg.tool_calls()[0].invalid_result().is_none());
}

#[test]
fn an_opaque_redirect_is_named_as_one() {
    assert!(
        MessagesError::Redirected { status: 0 }
            .to_string()
            .contains("opaque redirect")
    );
    assert!(
        MessagesError::Redirected { status: 307 }
            .to_string()
            .contains("HTTP 307")
    );
}

#[tokio::test]
async fn betas_extra_fields_system_turns_and_eager_tools_reach_the_wire() {
    let (base, log) = serve(vec![ok(sse(&tool_turn()), 64), ok(sse(&tool_turn()), 64)]).await;
    let mut req = request();
    req.betas = vec![
        "server-side-fallback-2026-07-01".into(),
        "other-2026-01-01".into(),
    ];
    req.extra.insert("fallbacks".into(), json!("default"));
    req.extra.insert("max_tokens".into(), json!(123));
    req.tools[0].eager_input_streaming = true;
    req.messages.push(Message {
        role: Role::System,
        content: vec![Block::text("Keep it short.")],
    });
    call(&base, &req).await.0.unwrap();
    call(&base, &request()).await.0.unwrap();
    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen[0].header("anthropic-beta"),
        Some("server-side-fallback-2026-07-01,other-2026-01-01")
    );
    let body = seen[0].json();
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["max_tokens"], 123);
    assert_eq!(body["tools"][0]["eager_input_streaming"], true);
    assert_eq!(
        body["messages"][1],
        json!({"role": "system", "content": [{"type": "text", "text": "Keep it short."}]})
    );
    assert_eq!(seen[1].header("anthropic-beta"), None);
    assert!(
        seen[1].json()["tools"][0]
            .get("eager_input_streaming")
            .is_none()
    );
}

#[test]
fn an_errors_summary_carries_no_text_from_the_answer() {
    let secret = "<html>internal admin panel: db=10.0.0.7 password=hunter2</html>";
    let errors = [
        MessagesError::Auth {
            status: 401,
            message: secret.into(),
        },
        MessagesError::RateLimited {
            retry_after: None,
            message: secret.into(),
        },
        MessagesError::Overloaded {
            message: secret.into(),
        },
        MessagesError::InvalidRequest {
            message: secret.into(),
        },
        MessagesError::Api {
            status: 500,
            kind: "api_error".into(),
            message: secret.into(),
        },
        MessagesError::Api {
            status: 418,
            kind: secret.into(),
            message: secret.into(),
        },
        MessagesError::Redirected { status: 302 },
        MessagesError::Network {
            message: secret.into(),
        },
        MessagesError::StreamCut {
            message: secret.into(),
        },
        MessagesError::Protocol {
            message: secret.into(),
        },
        MessagesError::UnknownEvent {
            kind: secret.into(),
        },
        MessagesError::BadBaseUrl,
    ];
    for e in &errors {
        let s = e.summary();
        assert!(!s.is_empty() && s.len() < 80, "{s}");
        for leak in ["hunter2", "10.0.0.7", "<html>", "panel"] {
            assert!(!s.contains(leak), "{e:?} -> {s}");
        }
    }
    assert_eq!(errors[0].summary(), "the key was refused (HTTP 401)");
    assert_eq!(errors[4].summary(), "api_error (HTTP 500)");
    assert_eq!(errors[5].summary(), "provider error (HTTP 418)");
}

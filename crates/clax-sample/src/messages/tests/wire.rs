//! OpenAI's Chat Completions and Gemini's `streamGenerateContent`, through
//! the same client, against the scripted server. Each answer is the shape
//! the provider's documentation gives.

use super::*;

/// `data:` lines with no event names, as both APIs stream.
fn data(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

/// A history: the request, a turn that thought, spoke and called a tool,
/// and the call's result.
fn history(signature: Option<&str>) -> MessagesRequest {
    let mut req = request();
    let mut call = json!({"type": "tool_use", "id": "call_7", "name": "run_script",
        "input": {"title": "Read", "code": "return 1"}});
    if let Some(s) = signature {
        call["signature"] = json!(s);
    }
    req.messages.push(Message {
        role: Role::Assistant,
        content: vec![
            Block::Thinking {
                thinking: "Look first.".into(),
                signature: "sig-think".into(),
            },
            Block::text("Reading it."),
            Block::from_json(call),
        ],
    });
    req.messages.push(Message::user(vec![
        Block::tool_result("call_7", "1", false),
        Block::text("Go on"),
    ]));
    req
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions
// ---------------------------------------------------------------------------

#[test]
fn a_history_becomes_chat_completions_messages() {
    let req = history(None);
    let b = openai::body(&req, "https://api.subconscious.dev/v1");
    assert_eq!(
        b["messages"],
        json!([
            {"role": "system", "content": "You are ClaxBot."},
            {"role": "user", "content": "Fix the page"},
            {"role": "assistant", "content": "Reading it.", "tool_calls": [{
                "id": "call_7", "type": "function",
                "function": {"name": "run_script", "arguments": "{\"code\":\"return 1\",\"title\":\"Read\"}"},
            }]},
            {"role": "tool", "tool_call_id": "call_7", "content": "1"},
            {"role": "user", "content": "Go on"},
        ])
    );
    assert_eq!(b["model"], "claude-sonnet-5-5");
    assert_eq!(b["max_tokens"], 8000);
    assert_eq!(b["stream"], true);
    assert_eq!(b["stream_options"], json!({"include_usage": true}));
    assert_eq!(b["tool_choice"], "auto");
    assert_eq!(b["reasoning_effort"], "medium");
    assert_eq!(b["tools"][0]["type"], "function");
    assert_eq!(b["tools"][0]["function"]["name"], "run_script");
    assert_eq!(
        b["tools"][0]["function"]["parameters"],
        req.tools[0].input_schema
    );
    // No Messages API field leaks through.
    for k in ["system", "thinking", "output_config", "parallel_tool_calls"] {
        assert!(b.get(k).is_none(), "{k}: {b}");
    }
}

/// [`history`] with a second call beside the first, each result showing
/// images; `text` is the user turn's own.
fn history_with_images(text: bool) -> MessagesRequest {
    let mut req = history(None);
    req.messages[1].content.push(Block::ToolUse {
        id: "call_8".into(),
        name: "look".into(),
        input: json!({}),
    });
    let png = ToolImage::new("image/png", vec![0x89, b'P', b'N', b'G']);
    let jpeg = ToolImage::new("image/jpeg", vec![0xff, 0xd8, 0xff]);
    let mut turn = vec![
        Block::tool_result_with_images("call_7", "1", false, &[png.clone(), jpeg], true),
        // As a history read back from JSON holds it.
        Block::from_json(
            Block::tool_result_with_images("call_8", "", true, &[png], true).to_json(),
        ),
        Block::tool_result("call_9", "none", false),
    ];
    if text {
        turn.push(Block::text("Go on"));
    }
    req.messages[2].content = turn;
    req
}

#[test]
fn a_tool_results_images_follow_the_tool_messages_in_one_user_message() {
    let image_parts = json!([
        {"type": "text", "text": "Images of the result of tool call call_7:"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw=="}},
        {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,/9j/"}},
        {"type": "text", "text": "Images of the result of tool call call_8:"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw=="}},
    ]);
    let tools = json!([
        {"role": "tool", "tool_call_id": "call_7", "content": "1"},
        {"role": "tool", "tool_call_id": "call_8", "content": "Error: "},
        {"role": "tool", "tool_call_id": "call_9", "content": "none"},
    ]);
    let base = "https://api.openai.com/v1";

    let b = openai::body(&history_with_images(true), base);
    let mut with_text = image_parts.clone();
    with_text
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "text", "text": "Go on"}));
    assert_eq!(
        b["messages"].as_array().unwrap()[3..6],
        tools.as_array().unwrap()[..]
    );
    assert_eq!(
        b["messages"][6],
        json!({"role": "user", "content": with_text})
    );
    assert_eq!(b["messages"].as_array().unwrap().len(), 7);

    let b = openai::body(&history_with_images(false), base);
    assert_eq!(
        b["messages"][6],
        json!({"role": "user", "content": image_parts})
    );
    assert_eq!(b["messages"].as_array().unwrap().len(), 7);

    // A system turn's text stays a system message, after the images.
    let mut req = history_with_images(true);
    req.messages[2].role = Role::System;
    let b = openai::body(&req, base);
    assert_eq!(
        b["messages"][6],
        json!({"role": "user", "content": image_parts})
    );
    assert_eq!(
        b["messages"][7],
        json!({"role": "system", "content": "Go on"})
    );
}

#[test]
fn a_tool_results_images_are_inline_data_after_the_turns_parts() {
    let b = gemini::body(&history_with_images(true));
    assert_eq!(
        b["contents"][2],
        json!({"role": "user", "parts": [
            {"functionResponse": {"name": "run_script", "id": "call_7", "response": {"result": "1"}}},
            {"functionResponse": {"name": "look", "id": "call_8", "response": {"error": ""}}},
            {"functionResponse": {"name": "", "id": "call_9", "response": {"result": "none"}}},
            {"text": "Go on"},
            {"text": "Images of the result of run_script, call call_7:"},
            {"inlineData": {"mimeType": "image/png", "data": "iVBORw=="}},
            {"inlineData": {"mimeType": "image/jpeg", "data": "/9j/"}},
            {"text": "Images of the result of look, call call_8:"},
            {"inlineData": {"mimeType": "image/png", "data": "iVBORw=="}},
        ]})
    );
}

/// Two results of one function in one turn: each one's images are
/// labelled by its call, by its ID or, for an ID made here, by its place.
#[test]
fn two_results_of_one_function_label_their_images_apart() {
    let png = ToolImage::new("image/png", b"\x89PNG".to_vec());
    let call = |id: &str| Block::ToolUse {
        id: id.into(),
        name: "run_script".into(),
        input: json!({}),
    };
    let mut req = request();
    for (a, b) in [("c1", "c2"), ("gemini_call_1", "gemini_call_2")] {
        req.messages = vec![
            Message {
                role: Role::User,
                content: vec![Block::text("Look")],
            },
            Message {
                role: Role::Assistant,
                content: vec![call(a), call(b)],
            },
            Message {
                role: Role::User,
                content: vec![
                    Block::tool_result_with_images(
                        a,
                        "first",
                        false,
                        std::slice::from_ref(&png),
                        true,
                    ),
                    Block::tool_result_with_images(
                        b,
                        "second",
                        false,
                        std::slice::from_ref(&png),
                        true,
                    ),
                ],
            },
        ];
        let parts = gemini::body(&req)["contents"][2]["parts"].clone();
        let labels: Vec<&str> = parts
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect();
        let want = if a == "c1" {
            vec![
                "Images of the result of run_script, call c1:",
                "Images of the result of run_script, call c2:",
            ]
        } else {
            vec![
                "Images of result 1 of this turn (run_script):",
                "Images of result 2 of this turn (run_script):",
            ]
        };
        assert_eq!(labels, want);
    }
}

#[test]
fn mistral_is_not_sent_stream_options_and_max_effort_is_xhigh() {
    let mut req = request();
    req.effort = Some("max".into());
    req.tool_choice = Some(ToolChoice::Auto {
        disable_parallel_tool_use: true,
    });
    let b = openai::body(&req, "https://API.Mistral.ai/v1");
    assert!(b.get("stream_options").is_none(), "{b}");
    assert_eq!(b["reasoning_effort"], "xhigh");
    assert_eq!(b["parallel_tool_calls"], false);
    req.effort = None;
    req.tools.clear();
    let b = openai::body(&req, "https://api.mistral.ai/v1");
    assert!(b.get("reasoning_effort").is_none(), "{b}");
    // Without tools there is nothing to call in parallel.
    assert!(b.get("parallel_tool_calls").is_none(), "{b}");
}

fn chunk(delta: Value, finish: Option<&str>) -> Value {
    json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "zai-glm-5-3-flash",
           "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
}

fn openai_turn() -> String {
    let mut s = data(&[
        chunk(
            json!({"role": "assistant", "reasoning_content": "Need the "}),
            None,
        ),
        chunk(json!({"reasoning_content": "draft."}), None),
        chunk(json!({"content": "Reading the café "}), None),
        chunk(json!({"content": "page ☕"}), None),
        chunk(
            json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function",
                                   "function": {"name": "run_script", "arguments": "{\"title\": \"Read ☕\", "}}]}),
            None,
        ),
        chunk(
            json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"code\": \"return 1\"}"}}]}),
            None,
        ),
        chunk(json!({}), Some("tool_calls")),
        json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [],
               "usage": {"prompt_tokens": 1200, "completion_tokens": 87,
                         "prompt_tokens_details": {"cached_tokens": 1000}}}),
    ]);
    s.push_str("data: [DONE]\n\n");
    s
}

#[tokio::test]
async fn a_chat_completions_stream_assembles_into_one_messages_turn() {
    let (base, log) = serve(vec![ok(openai_turn(), 7)]).await;
    let base = format!("{base}/v1");
    let mut deltas = Vec::new();
    let msg = MessagesClient::new()
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |d| {
            deltas.push(d)
        })
        .await
        .unwrap();
    assert_eq!(
        msg.content,
        vec![
            Block::Thinking {
                thinking: "Need the draft.".into(),
                signature: String::new(),
            },
            Block::text("Reading the café page ☕"),
            Block::ToolUse {
                id: "call_a".into(),
                name: "run_script".into(),
                input: json!({"title": "Read ☕", "code": "return 1"}),
            },
        ]
    );
    assert_eq!(msg.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(msg.model, "zai-glm-5-3-flash");
    assert_eq!(
        msg.usage,
        Usage {
            input_tokens: 200,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1000,
            output_tokens: 87,
            billed_micro_usd: None,
        }
    );
    assert!(matches!(&msg.tool_calls()[..], [ToolCall::Valid { id, .. }] if id == "call_a"));
    assert!(deltas.iter().any(|d| matches!(d, Delta::Thinking { .. })));
    assert!(deltas.iter().any(|d| matches!(d, Delta::ToolInput { .. })));
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.target, "/v1/chat/completions");
    assert_eq!(
        seen.header("authorization").map(str::to_string),
        Some(format!("Bearer {KEY}"))
    );
    assert_eq!(seen.header("x-api-key"), None);
    assert_eq!(seen.json()["messages"][1]["content"], "Fix the page");
}

#[tokio::test]
async fn reasoning_inside_think_tags_streams_as_thinking() {
    let mut s = data(&[
        chunk(json!({"content": "<thi"}), None),
        chunk(json!({"content": "nk>plan it</th"}), None),
        chunk(json!({"content": "ink>\n\nDone: <think> stays"}), None),
        chunk(json!({}), Some("stop")),
        json!({"choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 3}}),
    ]);
    s.push_str("data: [DONE]\n\n");
    let (base, _) = serve(vec![ok(s, 4)]).await;
    let msg = MessagesClient::new()
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |_| {})
        .await
        .unwrap();
    assert_eq!(
        msg.content,
        vec![
            Block::Thinking {
                thinking: "plan it".into(),
                signature: String::new(),
            },
            Block::text("\n\nDone: <think> stays"),
        ]
    );
    assert_eq!(msg.stop_reason, Some(StopReason::EndTurn));
}

/// The usage of one OpenAI-API turn streamed as `chunks`, then `[DONE]`.
async fn usage_of(chunks: &[Value]) -> Usage {
    let (base, _) = serve(vec![ok(data(chunks) + "data: [DONE]\n\n", 9)]).await;
    MessagesClient::new()
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |_| {})
        .await
        .unwrap()
        .usage
}

/// OpenRouter's Chat Completions usage, with `extra` fields merged in.
fn usage(extra: Value) -> Value {
    let mut u = json!({"prompt_tokens": 1200, "completion_tokens": 87,
                       "prompt_tokens_details": {"cached_tokens": 1000}});
    for (k, v) in extra.as_object().unwrap() {
        u[k] = v.clone();
    }
    u
}

fn billed(billed_micro_usd: Option<u64>) -> Usage {
    Usage {
        input_tokens: 200,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 1000,
        output_tokens: 87,
        billed_micro_usd,
    }
}

#[tokio::test]
async fn a_usage_cost_is_the_billed_cost_in_micro_usd_rounded_up() {
    let upstream = |c: Value| json!({"upstream_inference_cost": c});
    let cases = [
        (json!({"cost": 0.0012345}), Some(1235)),
        (json!({"cost": 0.0000004}), Some(1)),
        (json!({"cost": 1e-13}), Some(1)),
        (json!({"cost": 0.000003}), Some(3)),
        (json!({"cost": 0.0019}), Some(1900)),
        (json!({"cost": 0}), Some(0)),
        (json!({}), None),
        (json!({"cost": -0.5}), None),
        (json!({"cost": "0.25"}), None),
        (json!({"cost": null}), None),
        // BYOK: cost is OpenRouter's fee; the upstream charge is added.
        (
            json!({"cost": 0, "is_byok": true, "cost_details": upstream(json!(0.0019))}),
            Some(1900),
        ),
        (
            json!({"cost": 0.0001, "is_byok": true, "cost_details": upstream(json!(0.002))}),
            Some(2100),
        ),
        (json!({"cost": 0, "is_byok": true}), None),
        (
            json!({"cost": 0, "is_byok": true, "cost_details": upstream(Value::Null)}),
            None,
        ),
        (
            json!({"cost": 0, "is_byok": true, "cost_details": upstream(json!(-1.0))}),
            None,
        ),
        (
            json!({"cost": 0.0001, "is_byok": false, "cost_details": upstream(json!(0.002))}),
            Some(100),
        ),
    ];
    for (extra, want) in cases {
        let got = usage_of(&[
            chunk(json!({"content": "hi"}), Some("stop")),
            json!({"choices": [], "usage": usage(extra.clone())}),
        ])
        .await;
        assert_eq!(got, billed(want), "{extra}");
    }
}

#[tokio::test]
async fn the_billed_cost_is_read_wherever_the_usage_comes_and_kept_by_a_later_one() {
    let costed = usage(json!({"cost": 0.0019}));
    let mut with_finish = chunk(json!({"content": "hi"}), Some("stop"));
    with_finish["usage"] = costed.clone();
    assert_eq!(usage_of(&[with_finish]).await, billed(Some(1900)));
    let mut before = chunk(json!({"content": "hi"}), None);
    before["usage"] = costed.clone();
    let finish = chunk(json!({}), Some("stop"));
    assert_eq!(
        usage_of(&[before, finish.clone()]).await,
        billed(Some(1900))
    );
    let later = json!({"choices": [], "usage": usage(json!({}))});
    assert_eq!(
        usage_of(&[
            chunk(json!({"content": "hi"}), None),
            json!({"choices": [], "usage": costed}),
            finish,
            later,
        ])
        .await,
        billed(Some(1900))
    );
}

#[tokio::test]
async fn cache_written_tokens_are_cache_creation_not_input() {
    let u = json!({"prompt_tokens": 1500, "completion_tokens": 87,
                   "prompt_tokens_details": {"cached_tokens": 1000, "cache_write_tokens": 300}});
    let got = usage_of(&[
        chunk(json!({"content": "hi"}), Some("stop")),
        json!({"choices": [], "usage": u}),
    ])
    .await;
    assert_eq!(
        got,
        Usage {
            input_tokens: 200,
            cache_creation_input_tokens: 300,
            cache_read_input_tokens: 1000,
            output_tokens: 87,
            billed_micro_usd: None,
        }
    );
    // A count past the prompt is held to it.
    let u = json!({"prompt_tokens": 1200, "completion_tokens": 1,
                   "prompt_tokens_details": {"cached_tokens": 1000, "cache_write_tokens": 900}});
    let got = usage_of(&[
        chunk(json!({"content": "hi"}), Some("stop")),
        json!({"choices": [], "usage": u}),
    ])
    .await;
    assert_eq!(
        (got.input_tokens, got.cache_creation_input_tokens),
        (0, 200)
    );
}

#[tokio::test]
async fn a_chat_completions_stream_without_usage_or_done_fails() {
    let no_usage = data(&[chunk(json!({"content": "hi"}), Some("stop"))]) + "data: [DONE]\n\n";
    let cut = data(&[chunk(json!({"content": "hi"}), None)]);
    let (base, _) = serve(vec![ok(no_usage, 9), ok(cut, 9)]).await;
    let client = MessagesClient::new();
    let err = client
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |_| {})
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MessagesError::Protocol { message } if message.contains("usage")),
        "{err:?}"
    );
    let err = client
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, MessagesError::StreamCut { .. }), "{err:?}");
}

#[tokio::test]
async fn openai_errors_are_classified_with_the_key_redacted() {
    let (base, _) = serve(vec![
        json_reply(
            400,
            json!({"object": "error", "message": format!("bad key {KEY}"), "type": "invalid_request_error"}),
        ),
        ok(
            data(&[json!({"error": {"message": "slow down", "type": "rate_limit_error", "code": 429}})]),
            9,
        ),
    ])
    .await;
    let client = MessagesClient::new();
    let err = client
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(
        err,
        MessagesError::InvalidRequest {
            message: "bad key [redacted]".into()
        }
    );
    let err = client
        .stream(ModelApi::OpenAi, &base, &key(), &request(), |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, MessagesError::RateLimited { .. }), "{err:?}");
}

#[tokio::test]
async fn openai_models_are_listed_with_the_bearer_key() {
    let (base, log) = serve(vec![json_reply(
        200,
        json!({"object": "list", "data": [
            {"id": "subconscious/glm-5.3-marathon", "object": "model", "name": "GLM 5.3 Marathon"},
            {"id": "zai-glm-5-3-flash", "object": "model"},
        ]}),
    )])
    .await;
    let base = format!("{base}/v1");
    let models = MessagesClient::new()
        .list_models(ModelApi::OpenAi, &base, &key())
        .await
        .unwrap();
    let ids: Vec<(&str, &str)> = models
        .iter()
        .map(|m| (m.id.as_str(), m.display_name.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("subconscious/glm-5.3-marathon", "GLM 5.3 Marathon"),
            ("zai-glm-5-3-flash", "zai-glm-5-3-flash"),
        ]
    );
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.target, "/v1/models");
    assert_eq!(
        seen.header("authorization").map(str::to_string),
        Some(format!("Bearer {KEY}"))
    );
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

#[test]
fn a_history_becomes_gemini_contents_with_its_signatures() {
    let mut req = history(Some("sig-call"));
    // A call Gemini sent without an ID: its ID here is never sent back.
    req.messages[1].content[2] = Block::from_json(json!({"type": "tool_use",
        "id": "gemini_call_2", "name": "run_script", "input": {}}));
    req.messages[2].content[0] = Block::tool_result("gemini_call_2", "oops", true);
    let b = gemini::body(&req);
    assert_eq!(
        b["systemInstruction"],
        json!({"parts": [{"text": "You are ClaxBot."}]})
    );
    assert_eq!(
        b["contents"],
        json!([
            {"role": "user", "parts": [{"text": "Fix the page"}]},
            {"role": "model", "parts": [
                {"text": "Look first.", "thought": true, "thoughtSignature": "sig-think"},
                {"text": "Reading it."},
                {"functionCall": {"name": "run_script", "args": {}}},
            ]},
            {"role": "user", "parts": [
                {"functionResponse": {"name": "run_script", "response": {"error": "oops"}}},
                {"text": "Go on"},
            ]},
        ])
    );
    assert_eq!(
        b["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"],
        req.tools[0].input_schema
    );
    assert_eq!(
        b["toolConfig"],
        json!({"functionCallingConfig": {"mode": "AUTO"}})
    );
    assert_eq!(
        b["generationConfig"],
        json!({"maxOutputTokens": 8000,
               "thinkingConfig": {"includeThoughts": true, "thinkingLevel": "medium"}})
    );
    let b = gemini::body(&history(Some("sig-call")));
    assert_eq!(
        b["contents"][1]["parts"][2],
        json!({"functionCall": {"name": "run_script", "id": "call_7",
                                "args": {"title": "Read", "code": "return 1"}},
               "thoughtSignature": "sig-call"})
    );
    assert_eq!(
        b["contents"][2]["parts"][0],
        json!({"functionResponse": {"name": "run_script", "id": "call_7",
                                    "response": {"result": "1"}}})
    );
}

fn gemini_chunk(parts: Value, finish: Option<&str>, usage: Option<Value>) -> Value {
    let mut c = json!({"content": {"role": "model", "parts": parts}});
    if let Some(f) = finish {
        c["finishReason"] = json!(f);
    }
    let mut v = json!({"candidates": [c], "modelVersion": "gemini-3.5-flash", "responseId": "r-1"});
    if let Some(u) = usage {
        v["usageMetadata"] = u;
    }
    v
}

#[tokio::test]
async fn a_gemini_stream_assembles_into_one_messages_turn() {
    let body = data(&[
        gemini_chunk(json!([{"text": "Need the ", "thought": true}]), None, None),
        gemini_chunk(
            json!([{"text": "draft.", "thought": true, "thoughtSignature": "sig-1"}]),
            None,
            None,
        ),
        gemini_chunk(json!([{"text": "Reading the café "}]), None, None),
        gemini_chunk(
            json!([{"text": "page ☕"},
                   {"functionCall": {"name": "run_script", "args": {"title": "Read ☕", "code": "return 1"}},
                    "thoughtSignature": "sig-2"}]),
            Some("STOP"),
            Some(
                json!({"promptTokenCount": 1200, "cachedContentTokenCount": 1000,
                        "candidatesTokenCount": 60, "thoughtsTokenCount": 27}),
            ),
        ),
    ]);
    let (base, log) = serve(vec![ok(body, 11)]).await;
    let base = format!("{base}/v1beta");
    let mut req = request();
    req.model = "gemini-3.5-flash".into();
    let msg = MessagesClient::new()
        .stream(ModelApi::Gemini, &base, &key(), &req, |_| {})
        .await
        .unwrap();
    assert_eq!(msg.id, "r-1");
    assert_eq!(msg.model, "gemini-3.5-flash");
    assert_eq!(msg.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(
        msg.usage,
        Usage {
            input_tokens: 200,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1000,
            output_tokens: 87,
            billed_micro_usd: None,
        }
    );
    assert_eq!(msg.content.len(), 3, "{:?}", msg.content);
    assert_eq!(
        msg.content[0],
        Block::Thinking {
            thinking: "Need the draft.".into(),
            signature: "sig-1".into(),
        }
    );
    assert_eq!(msg.content[1], Block::text("Reading the café page ☕"));
    let calls = msg.tool_calls();
    let [ToolCall::Valid { id, name, input }] = &calls[..] else {
        panic!("{calls:?}");
    };
    assert!(id.starts_with("gemini_call_"), "{id}");
    assert_eq!(name, "run_script");
    assert_eq!(input, &json!({"title": "Read ☕", "code": "return 1"}));
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(
        seen.target,
        "/v1beta/models/gemini-3.5-flash:streamGenerateContent?alt=sse"
    );
    assert_eq!(seen.header("x-goog-api-key"), Some(KEY));
    assert_eq!(seen.header("authorization"), None);

    // The turn goes back with each signature on its part and no made ID.
    req.messages.push(msg.to_message());
    let b = gemini::body(&req);
    assert_eq!(
        b["contents"][1]["parts"],
        json!([
            {"text": "Need the draft.", "thought": true, "thoughtSignature": "sig-1"},
            {"text": "Reading the café page ☕"},
            {"functionCall": {"name": "run_script", "args": {"title": "Read ☕", "code": "return 1"}},
             "thoughtSignature": "sig-2"},
        ])
    );
}

#[tokio::test]
async fn a_signature_alone_signs_the_text_before_it() {
    let body = data(&[
        gemini_chunk(json!([{"text": "All done."}]), None, None),
        gemini_chunk(
            json!([{"text": "", "thoughtSignature": "sig-end"}]),
            Some("STOP"),
            Some(json!({"promptTokenCount": 10, "candidatesTokenCount": 3})),
        ),
    ]);
    let (base, _) = serve(vec![ok(body, 64)]).await;
    let msg = MessagesClient::new()
        .stream(ModelApi::Gemini, &base, &key(), &request(), |_| {})
        .await
        .unwrap();
    assert_eq!(msg.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(
        msg.content,
        vec![Block::Raw(
            json!({"type": "text", "text": "All done.", "signature": "sig-end"})
        )]
    );
    let mut req = request();
    req.messages.push(msg.to_message());
    assert_eq!(
        gemini::body(&req)["contents"][1]["parts"],
        json!([{"text": "All done.", "thoughtSignature": "sig-end"}])
    );
}

#[tokio::test]
async fn a_gemini_stream_cut_before_its_finish_reason_is_retryable() {
    let cut = data(&[gemini_chunk(json!([{"text": "Half"}]), None, None)]);
    let blocked = data(&[json!({"promptFeedback": {"blockReason": "SAFETY"},
        "usageMetadata": {"promptTokenCount": 4}})]);
    let error = data(&[
        json!({"error": {"code": 429, "message": "quota", "status": "RESOURCE_EXHAUSTED"}}),
    ]);
    let (base, _) = serve(vec![ok(cut, 9), ok(blocked, 9), ok(error, 9)]).await;
    let client = MessagesClient::new();
    let err = client
        .stream(ModelApi::Gemini, &base, &key(), &request(), |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, MessagesError::StreamCut { .. }), "{err:?}");
    assert!(err.retryable());
    let msg = client
        .stream(ModelApi::Gemini, &base, &key(), &request(), |_| {})
        .await
        .unwrap();
    assert_eq!(msg.stop_reason, Some(StopReason::Refusal));
    assert!(msg.content.is_empty());
    let err = client
        .stream(ModelApi::Gemini, &base, &key(), &request(), |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, MessagesError::RateLimited { .. }), "{err:?}");
}

#[tokio::test]
async fn gemini_models_follow_their_pages_and_keep_the_generating_ones() {
    let (base, log) = serve(vec![
        json_reply(
            200,
            json!({"models": [
                {"name": "models/gemini-3.5-flash", "displayName": "Gemini 3.5 Flash",
                 "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/text-embedding-005", "supportedGenerationMethods": ["embedContent"]},
            ], "nextPageToken": "p/2"}),
        ),
        json_reply(
            200,
            json!({"models": [{"name": "models/gemini-3.1-pro-preview",
                               "supportedGenerationMethods": ["generateContent"]}]}),
        ),
    ])
    .await;
    let base = format!("{base}/v1beta");
    let models = MessagesClient::new()
        .list_models(ModelApi::Gemini, &base, &key())
        .await
        .unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["gemini-3.5-flash", "gemini-3.1-pro-preview"]);
    assert_eq!(models[0].display_name, "Gemini 3.5 Flash");
    let log = log.lock().unwrap();
    assert_eq!(log[0].target, "/v1beta/models?pageSize=100");
    assert_eq!(log[1].target, "/v1beta/models?pageSize=100&pageToken=p%2F2");
    assert_eq!(log[1].header("x-goog-api-key"), Some(KEY));
}

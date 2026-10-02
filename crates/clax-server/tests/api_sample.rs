mod common;
use clax_server::sample::provider::{Block, Role};
use clax_server::sample::sse::SseParser;
use clax_server::sample::stub::StubProvider;
use clax_server::sample::{SampleSettings, Sampler};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// A test daemon whose sample provider is the stub (5 ms per piece); returns the stub and the sampler.
async fn stub_server(
    images: bool,
    settings: SampleSettings,
) -> (TestServer, StubProvider, Arc<Sampler>) {
    stub_server_with(images, settings, Duration::from_millis(5)).await
}

/// [`stub_server`] with the stub pausing `delay` before each streamed piece.
async fn stub_server_with(
    images: bool,
    settings: SampleSettings,
    delay: Duration,
) -> (TestServer, StubProvider, Arc<Sampler>) {
    let stub = StubProvider::new(images, delay);
    let sampler = Arc::new(Sampler::new(Arc::new(stub.clone()), settings));
    let s = sampler.clone();
    let ts = TestServer::spawn_with(move |st| st.sample = s).await;
    (ts, stub, sampler)
}

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json("/api/artifacts", json!({"title": "Ask", "capabilities": caps, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}))
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// One sample call's open stream.
struct Call {
    body: std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
    parser: SseParser,
    queued: std::collections::VecDeque<(String, Value)>,
}

impl Call {
    /// The next frame within 10 s, keep-alives skipped.
    async fn next(&mut self) -> (String, Value) {
        use futures::StreamExt;
        loop {
            if let Some(f) = self.queued.pop_front() {
                return f;
            }
            let chunk = tokio::time::timeout(Duration::from_secs(10), self.body.next())
                .await
                .expect("a frame within 10 s")
                .expect("open")
                .unwrap();
            for e in self.parser.push(&chunk) {
                self.queued
                    .push_back((e.event, serde_json::from_str(&e.data).unwrap()));
            }
        }
    }

    /// Frames up to and including `done` or `error`.
    async fn rest(&mut self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        loop {
            let f = self.next().await;
            let end = f.0 == "done" || f.0 == "error";
            out.push(f);
            if end {
                return out;
            }
        }
    }
}

fn text_of(frames: &[(String, Value)]) -> String {
    frames
        .iter()
        .filter(|(n, _)| n == "text")
        .map(|(_, d)| d["delta"].as_str().unwrap())
        .collect()
}

/// The viewer cookie of the owner's browser in these tests when a test names
/// none (a cookie is read only when its value is a ULID).
const OWNER_BROWSER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// Starts a call as the owner's browser: the bearer token plus a viewer
/// cookie (`cookie`, else [`OWNER_BROWSER`]).
async fn start(
    ts: &TestServer,
    aid: &str,
    body: Value,
    cookie: Option<&str>,
) -> Result<Call, (u16, Value)> {
    let c = cookie.unwrap_or(OWNER_BROWSER);
    let r = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/sample", ts.base)),
        )
        .header("cookie", format!("clax_viewer={c}"))
        .json(&body);
    let res = r.send().await.unwrap();
    if !res.status().is_success() {
        let s = res.status().as_u16();
        return Err((s, res.json().await.unwrap_or(Value::Null)));
    }
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    Ok(Call {
        body: Box::pin(res.bytes_stream()),
        parser: SseParser::default(),
        queued: Default::default(),
    })
}

#[tokio::test]
async fn streams_text_then_done_under_the_framing_and_the_tier_model() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let mut c = start(
        &ts,
        &aid,
        json!({"input": "hello", "model_tier": "complex"}),
        None,
    )
    .await
    .unwrap();
    let (name, s) = c.next().await;
    assert_eq!(name, "start");
    assert_eq!(s["cached"], false);
    assert!(s["call_id"].as_str().is_some_and(|id| id.len() == 26));
    let frames = c.rest().await;
    assert_eq!(text_of(&frames), "echo: hello");
    assert_eq!(
        frames.last().unwrap(),
        &(
            "done".to_string(),
            json!({"text": "echo: hello", "truncated": false, "model_tier_applied": "complex"})
        )
    );
    let req = &stub.requests()[0];
    assert_eq!(req.model, SampleSettings::default().models.complex);
    assert!(
        req.system
            .starts_with("You are answering a request that a web page made")
    );
}

#[tokio::test]
async fn a_tool_round_trip_joins_the_rounds_text_with_a_blank_line() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let viewer = ts.viewer(None).await;
    let tools = json!([{"name": "getColor", "description": "Returns the page's accent colour."}]);
    let mut c = start(
        &ts,
        &aid,
        json!({"input": "[[tool:getColor]]", "tools": tools, "cache": false}),
        Some(&viewer.cookie),
    )
    .await
    .unwrap();
    let call_id = c.next().await.1["call_id"].as_str().unwrap().to_string();
    assert_eq!(
        c.next().await,
        ("text".into(), json!({"delta": "Checking."}))
    );
    assert_eq!(
        c.next().await,
        (
            "tool_call".into(),
            json!({"id": "toolu_stub_1", "name": "getColor", "input": {}})
        )
    );
    let url = format!(
        "{}/api/artifacts/{aid}/sample/{call_id}/tool_result",
        ts.base
    );
    let other = ts.viewer(None).await;
    let res = ts
        .authed(ts.client.post(&url))
        .header("cookie", format!("clax_viewer={}", other.cookie))
        .json(&json!({"id": "toolu_stub_1", "content": "red"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let res = ts
        .authed(ts.client.post(&url))
        .header("cookie", format!("clax_viewer={}", viewer.cookie))
        .json(&json!({"id": "toolu_nope", "content": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let res = ts
        .authed(ts.client.post(&url))
        .header("cookie", format!("clax_viewer={}", viewer.cookie))
        .json(&json!({"id": "toolu_stub_1", "content": "teal"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let frames = c.rest().await;
    assert_eq!(text_of(&frames), "\n\ntool said: teal");
    assert_eq!(
        frames.last().unwrap().1["text"],
        "Checking.\n\ntool said: teal"
    );
    let second = &stub.requests()[1];
    assert_eq!(second.messages[1].role, Role::Assistant);
    assert!(matches!(
        &second.messages[1].content[..],
        [Block::Raw(_), Block::Raw(_)]
    ));
    assert!(
        matches!(&second.messages[2].content[..], [Block::ToolResult { content, is_error: false, .. }] if content == "teal")
    );
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/sample/01J00000000000000000000000/tool_result",
            ts.base
        )))
        .header("cookie", format!("clax_viewer={}", viewer.cookie))
        .json(&json!({"id": "x", "content": "y"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn a_tool_that_never_answers_times_out_into_an_error_result() {
    let settings = SampleSettings {
        tool_timeout: Duration::from_millis(200),
        ..SampleSettings::default()
    };
    let (ts, stub, sampler) = stub_server(false, settings).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let tools = json!([{"name": "stuck", "description": "Never answers."}]);
    let mut c = start(
        &ts,
        &aid,
        json!({"input": "[[tool:stuck]]", "tools": tools, "cache": false}),
        None,
    )
    .await
    .unwrap();
    let frames = c.rest().await;
    assert_eq!(frames.last().unwrap().0, "done");
    assert!(text_of(&frames).contains("tool said: Error: the tool did not answer within 0 s"));
    assert!(matches!(
        &stub.requests()[1].messages[2].content[..],
        [Block::ToolResult { is_error: true, .. }]
    ));
    assert_eq!(sampler.open_calls(), 0);
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_call() {
    let (ts, stub, sampler) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let tools = json!([{"name": "stuck", "description": "Never answers."}]);
    let mut waiting = start(
        &ts,
        &aid,
        json!({"input": "[[tool:stuck]]", "tools": tools, "cache": false}),
        None,
    )
    .await
    .unwrap();
    while waiting.next().await.0 != "tool_call" {}
    let mut slow = start(
        &ts,
        &aid,
        json!({"input": "[[slow]]", "cache": false}),
        None,
    )
    .await
    .unwrap();
    while slow.next().await.0 != "text" {}
    assert_eq!(sampler.open_calls(), 2);
    drop(waiting);
    drop(slow);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while sampler.open_calls() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(sampler.open_calls(), 0);
    assert_eq!(stub.requests().len(), 2, "no further round was asked for");
}

#[tokio::test]
async fn provider_outcomes_map_to_the_contract() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    for (prompt, code) in [
        ("[[error:rate_limited]]", "rate_limited"),
        ("[[error:unavailable]]", "sampling_disabled"),
        ("[[refuse]]", "refused"),
        ("[[empty]]", "empty_completion"),
    ] {
        let frames = start(&ts, &aid, json!({"input": prompt, "cache": false}), None)
            .await
            .unwrap()
            .rest()
            .await;
        assert_eq!(frames.last().unwrap().1["code"], code, "{prompt}");
    }
    let frames = start(
        &ts,
        &aid,
        json!({"input": "[[error-after:upstream_error]]", "cache": false}),
        None,
    )
    .await
    .unwrap()
    .rest()
    .await;
    assert_eq!(text_of(&frames), "partial ");
    assert_eq!(frames.last().unwrap().1["code"], "upstream_error");
    let frames = start(
        &ts,
        &aid,
        json!({"input": "[[truncate]]", "cache": false}),
        None,
    )
    .await
    .unwrap()
    .rest()
    .await;
    assert_eq!(
        frames.last().unwrap().1,
        json!({"text": "cut", "truncated": true, "model_tier_applied": "default"})
    );
}

#[tokio::test]
async fn json_calls_resolve_a_value_or_invalid_json() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let frames = start(
        &ts,
        &aid,
        json!({"input": "[[say:Here: {\"a\": 1}]]", "verb": "json"}),
        None,
    )
    .await
    .unwrap()
    .rest()
    .await;
    assert_eq!(frames.last().unwrap().1["value"], json!({"a": 1}));
    let frames = start(
        &ts,
        &aid,
        json!({"input": "[[say:no json at all]]", "verb": "json"}),
        None,
    )
    .await
    .unwrap()
    .rest()
    .await;
    assert_eq!(frames.last().unwrap().1["code"], "invalid_json");
    let frames = start(
        &ts,
        &aid,
        json!({"input": "[[truncate]]", "verb": "json"}),
        None,
    )
    .await
    .unwrap()
    .rest()
    .await;
    assert_eq!(frames.last().unwrap().1["code"], "invalid_json");
}

#[tokio::test]
async fn bad_calls_are_refused_before_any_stream() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    for (body, code) in [
        (json!({"input": ""}), "invalid_request"),
        (
            json!({"input": [{"role": "user", "content": "q"}, {"role": "assistant", "content": "a"}]}),
            "invalid_request",
        ),
        (json!({"input": "x".repeat(65_537)}), "prompt_too_large"),
        (
            json!({"input": "q", "images": [{"media_type": "image/png", "data": "iVBORw0KGgo="}]}),
            "images_unavailable",
        ),
        (
            json!({"input": "q", "tools": [{"name": "a", "description": "d"}]}),
            "invalid_request",
        ),
        (
            json!({"input": "q", "model_tier": "huge"}),
            "invalid_request",
        ),
        (json!({"prompt": "q"}), "invalid_request"),
    ] {
        let (status, err) = start(&ts, &aid, body.clone(), None)
            .await
            .err()
            .expect("refused");
        assert_eq!(
            (status, err["error"]["code"].as_str().unwrap()),
            (400, code),
            "{body}"
        );
    }
    assert!(stub.requests().is_empty());
}

#[tokio::test]
async fn undeclared_unconfigured_missing_and_foreign_origin_calls_are_refused() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let undeclared = artifact(&ts, json!({"db": {}})).await;
    let (s, e) = start(&ts, &undeclared, json!({"input": "q"}), None)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (s, e["error"]["code"].as_str().unwrap()),
        (403, "not_declared")
    );
    let (s, _) = start(&ts, "zzzzzzzzzzzz", json!({"input": "q"}), None)
        .await
        .err()
        .unwrap();
    assert_eq!(s, 404);
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/sample", ts.base)),
        )
        .header("cookie", format!("clax_viewer={OWNER_BROWSER}"))
        .header(
            "origin",
            format!("http://{aid}.localhost:{}", ts.addr.port()),
        )
        .json(&json!({"input": "q"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let off = TestServer::spawn().await;
    let aid = artifact(&off, json!({"sample": {}})).await;
    let (s, e) = start(&off, &aid, json!({"input": "q"}), None)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (s, e["error"]["code"].as_str().unwrap()),
        (403, "sampling_disabled")
    );
}

#[tokio::test]
async fn status_reports_availability_and_limits_and_never_the_key() {
    let (ts, _, _) = stub_server(true, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let v: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/sample"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["available"], true);
    assert_eq!(v["provider"], "stub");
    assert_eq!(v["limits"]["maxPromptBytes"], 65536);
    assert_eq!(v["limits"]["images"]["maxCount"], 5);
    let off = TestServer::spawn().await;
    let aid = artifact(&off, json!({"sample": {}})).await;
    let v: Value = off
        .get_authed(&format!("/api/artifacts/{aid}/sample"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (v["available"].clone(), v["provider"].clone()),
        (json!(false), Value::Null)
    );
    assert!(v["limits"].get("images").is_none());
    let on = TestServer::spawn_with(|st| {
        st.sample = Arc::new(Sampler::from_config(&Default::default(), |_| {
            Some("sk-never-shown".into())
        }));
    })
    .await;
    let aid = artifact(&on, json!({"sample": {}})).await;
    let text = on
        .get_authed(&format!("/api/artifacts/{aid}/sample"))
        .await
        .text()
        .await
        .unwrap();
    assert!(!text.contains("sk-never-shown"), "{text}");
}

#[tokio::test]
async fn only_the_owners_browser_may_spend_the_key() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let lan = ts.viewer(Some("Ben")).await;
    let url = format!("{}/api/artifacts/{aid}/sample", ts.base);
    // A LAN viewer: a cookie, no token.
    let res = ts
        .client
        .post(&url)
        .header("cookie", format!("clax_viewer={}", lan.cookie))
        .json(&json!({"input": "q"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // An agent or a script: the token, no browser.
    let res = ts
        .authed(ts.client.post(&url))
        .json(&json!({"input": "q"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "forbidden"
    );
    let res = ts
        .client
        .post(format!("{url}/01J00000000000000000000000/tool_result"))
        .header("cookie", format!("clax_viewer={}", lan.cookie))
        .json(&json!({"id": "x", "content": "y"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert!(stub.requests().is_empty());
    // The status a caller without the token reads says nothing is available.
    let v: Value = ts
        .get(&format!("/api/artifacts/{aid}/sample"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            v["available"].clone(),
            v["provider"].clone(),
            v["calls_today"].clone()
        ),
        (json!(false), Value::Null, json!(0))
    );
}

#[tokio::test]
async fn the_daemon_reports_its_sampler_to_the_token_only_and_never_the_key() {
    let ts = TestServer::spawn_with(|st| {
        st.sample = Arc::new(Sampler::from_config(&Default::default(), |_| {
            Some("sk-never-shown".into())
        }));
    })
    .await;
    assert_eq!(ts.get("/api/sample").await.status(), 401);
    let res = ts.get_authed("/api/sample").await;
    let text = res.text().await.unwrap();
    assert!(!text.contains("sk-never-shown"), "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v,
        json!({"available": true, "provider": "anthropic", "reason": null, "detail": null, "key_env": "ANTHROPIC_API_KEY", "daily_call_cap": null})
    );
    let off = TestServer::spawn_with(|st| {
        st.sample = Arc::new(Sampler::from_config(&Default::default(), |_| None))
    })
    .await;
    let v: Value = off.get_authed("/api/sample").await.json().await.unwrap();
    assert_eq!(
        (v["available"].clone(), v["reason"].clone()),
        (json!(false), json!("no_key"))
    );
}

async fn done_of(
    ts: &TestServer,
    aid: &str,
    body: Value,
    cookie: Option<&str>,
) -> (Value, Vec<(String, Value)>) {
    let mut c = start(ts, aid, body, cookie).await.expect("a stream");
    let started = c.next().await.1;
    (started, c.rest().await)
}

#[tokio::test]
async fn a_repeat_is_replayed_without_asking_the_provider() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let v = ts.viewer(None).await;
    let (s1, f1) = done_of(&ts, &aid, json!({"input": "hello"}), Some(&v.cookie)).await;
    let (s2, f2) = done_of(&ts, &aid, json!({"input": "hello"}), Some(&v.cookie)).await;
    assert_eq!(
        (s1["cached"].clone(), s2["cached"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(f2, vec![("done".to_string(), f1.last().unwrap().1.clone())]);
    assert_eq!(stub.requests().len(), 1);
    done_of(
        &ts,
        &aid,
        json!({"input": "hello", "model_tier": "quick"}),
        Some(&v.cookie),
    )
    .await;
    done_of(
        &ts,
        &aid,
        json!({"input": "hello", "verb": "json"}),
        Some(&v.cookie),
    )
    .await;
    let (s, _) = done_of(
        &ts,
        &aid,
        json!({"input": "hello", "cache": false}),
        Some(&v.cookie),
    )
    .await;
    assert_eq!(s["cached"], false);
    assert_eq!(stub.requests().len(), 4);
    let (s, _) = done_of(&ts, &aid, json!({"input": "hello"}), Some(&v.cookie)).await;
    assert_eq!(s["cached"], true);
}

#[tokio::test]
async fn answers_are_cached_per_viewer() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let (owner, lan) = (ts.viewer(Some("Owner")).await, ts.viewer(None).await); // two of the owner's browsers
    done_of(
        &ts,
        &aid,
        json!({"input": "same question"}),
        Some(&owner.cookie),
    )
    .await;
    let (s, _) = done_of(
        &ts,
        &aid,
        json!({"input": "same question"}),
        Some(&lan.cookie),
    )
    .await;
    assert_eq!(s["cached"], false);
    let (s, _) = done_of(&ts, &aid, json!({"input": "same question"}), None).await;
    assert_eq!(s["cached"], false);
    assert_eq!(stub.requests().len(), 3);
    let other = artifact(&ts, json!({"sample": {}})).await;
    let (s, _) = done_of(
        &ts,
        &other,
        json!({"input": "same question"}),
        Some(&owner.cookie),
    )
    .await;
    assert_eq!(s["cached"], false);
}

#[tokio::test]
async fn a_short_window_expires_and_refresh_asks_again_and_overwrites() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    done_of(
        &ts,
        &aid,
        json!({"input": "q", "cache": {"gc_time_ms": 50}}),
        None,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (s, _) = done_of(&ts, &aid, json!({"input": "q"}), None).await;
    assert_eq!(s["cached"], false);
    let (s, _) = done_of(
        &ts,
        &aid,
        json!({"input": "q", "cache": {"gc_time_ms": 60000, "refresh": true}}),
        None,
    )
    .await;
    assert_eq!(s["cached"], false);
    let (s, _) = done_of(
        &ts,
        &aid,
        json!({"input": "q", "cache": {"gc_time_ms": 60000}}),
        None,
    )
    .await;
    assert_eq!(s["cached"], true);
    assert_eq!(stub.requests().len(), 3);
}

#[tokio::test]
async fn failures_and_invalid_json_are_never_stored() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    for body in [
        json!({"input": "[[error:upstream_error]]"}),
        json!({"input": "[[say:nope]]", "verb": "json"}),
    ] {
        for _ in 0..2 {
            let (s, _) = done_of(&ts, &aid, body.clone(), None).await;
            assert_eq!(s["cached"], false, "{body}");
        }
    }
    assert_eq!(stub.requests().len(), 4);
}

#[tokio::test]
async fn identical_calls_in_flight_share_one_answer_even_if_the_first_leaves() {
    let (ts, stub, _) =
        stub_server_with(false, SampleSettings::default(), Duration::from_millis(200)).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let mut first = start(&ts, &aid, json!({"input": "shared"}), None)
        .await
        .unwrap();
    first.next().await;
    first.next().await;
    let mut second = start(&ts, &aid, json!({"input": "shared"}), None)
        .await
        .unwrap();
    assert_eq!(second.next().await.1["cached"], true);
    drop(first);
    let frames = second.rest().await;
    assert_eq!(text_of(&frames), "echo: shared");
    assert_eq!(frames.last().unwrap().1["text"], "echo: shared");
    assert_eq!(stub.requests().len(), 1);
}

#[tokio::test]
async fn the_daily_cap_counts_calls_that_reach_the_provider() {
    let settings = SampleSettings {
        daily_call_cap: Some(2),
        ..SampleSettings::default()
    };
    let (ts, _, _) = stub_server(false, settings).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let (s, _) = done_of(&ts, &aid, json!({"input": "a"}), None).await;
    assert_eq!(
        (s["calls_today"].clone(), s["daily_call_cap"].clone()),
        (json!(1), json!(2))
    );
    let (s, _) = done_of(&ts, &aid, json!({"input": "b", "cache": false}), None).await;
    assert_eq!(s["calls_today"], 2);
    let (status, e) = start(&ts, &aid, json!({"input": "c"}), None)
        .await
        .err()
        .expect("capped");
    assert_eq!(
        (status, e["error"]["code"].as_str().unwrap()),
        (429, "rate_limited")
    );
    let (s, _) = done_of(&ts, &aid, json!({"input": "a"}), None).await;
    assert_eq!(
        (s["cached"].clone(), s["calls_today"].clone()),
        (json!(true), json!(2))
    );
    let v: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/sample"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (v["calls_today"].clone(), v["daily_call_cap"].clone()),
        (json!(2), json!(2))
    );
    let other = artifact(&ts, json!({"sample": {}})).await;
    let (s, _) = done_of(&ts, &other, json!({"input": "c"}), None).await;
    assert_eq!(s["calls_today"], 1);
}

#[tokio::test]
async fn a_flood_from_one_viewer_is_rate_limited() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let (flooder, other) = (ts.viewer(None).await, ts.viewer(None).await);
    let url = format!("{}/api/artifacts/{aid}/sample", ts.base);
    let mut held = Vec::new();
    for _ in 0..6 {
        let (client, url, cookie, token) = (
            ts.client.clone(),
            url.clone(),
            flooder.cookie.clone(),
            ts.token.clone(),
        );
        held.push(tokio::spawn(async move {
            let res = client
                .post(url)
                .bearer_auth(token)
                .header("cookie", format!("clax_viewer={cookie}"))
                .json(&json!({"input": "[[slow]]", "cache": false}))
                .send()
                .await;
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(res);
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, e) = start(
        &ts,
        &aid,
        json!({"input": "[[slow]]", "cache": false}),
        Some(&flooder.cookie),
    )
    .await
    .err()
    .expect("refused");
    assert_eq!(
        (status, e["error"]["code"].as_str().unwrap()),
        (429, "rate_limited")
    );
    let mut fine = start(
        &ts,
        &aid,
        json!({"input": "hi", "cache": false}),
        Some(&other.cookie),
    )
    .await
    .expect("another viewer still runs");
    assert_eq!(fine.next().await.0, "start");
    for h in &held {
        h.abort();
    }
}

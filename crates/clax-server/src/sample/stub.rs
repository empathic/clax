//! A deterministic provider for tests and demos (`[sample] provider = "stub"`).
//! It answers from directives in the latest user text and never makes a
//! network call:
//!
//! - default: `echo: <text>` in three pieces;
//! - `[[say:TEXT]]`: exactly TEXT (everything up to the last `]]`);
//! - `[[tool:NAME]]` or `[[tool:NAME {json}]]`: `Checking.`, then a call of
//!   page tool NAME with that input (`{}` when absent); on the last round,
//!   `no rounds left`;
//! - a turn of tool results: `tool said: <results joined by "; ">`;
//! - `[[slow]]`: `tick ` fifty times, 100 ms apart;
//! - `[[refuse]]`: `I won` then a refusal; `[[empty]]`: no text;
//!   `[[truncate]]`: `cut` then the length limit;
//! - `[[error:CODE]]`: an error with that code (`ProviderErrorCode::parse`);
//!   `[[error-after:CODE]]`: `partial ` then that error;
//! - `[[images]]`: `saw N images` for the image blocks of the latest user turn.
//!
//! Each text piece waits `delay` first (`stub_delay_ms`). Every request is
//! recorded for tests.

use super::provider::*;
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct StubProvider {
    images: bool,
    delay: Duration,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

impl StubProvider {
    pub fn new(images: bool, delay: Duration) -> StubProvider {
        StubProvider {
            images,
            delay,
            requests: Arc::default(),
        }
    }

    /// Every request this provider has been sent, oldest first.
    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.requests.lock().expect("stub requests lock").clone()
    }
}

fn latest_user_text(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .rev()
        .filter(|t| t.role == Role::User)
        .find_map(|t| {
            t.content.iter().rev().find_map(|b| {
                if let Block::Text(s) = b {
                    Some(s.clone())
                } else {
                    None
                }
            })
        })
        .unwrap_or_default()
}

fn end(stop: Stop, text: &str, extra: Vec<Value>) -> ProviderEvent {
    let mut assistant = Vec::new();
    if !text.is_empty() {
        assistant.push(json!({"type": "text", "text": text}));
    }
    assistant.extend(extra);
    ProviderEvent::End { stop, assistant }
}

fn directive<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("[[{name}"))? + 2 + name.len();
    let end = text.rfind("]]")?;
    (end >= start).then(|| &text[start..end])
}

/// The events of one round, each with the pause before it.
fn script(req: &ProviderRequest, delay: Duration) -> Vec<(Duration, ProviderEvent)> {
    use ProviderEvent::{Error, Text, ToolUse};
    let z = Duration::ZERO;
    let results: Vec<&str> = req
        .messages
        .last()
        .map(|t| {
            t.content
                .iter()
                .filter_map(|b| {
                    if let Block::ToolResult { content, .. } = b {
                        Some(content.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if !results.is_empty() {
        let t = format!("tool said: {}", results.join("; "));
        return vec![
            (delay, Text(t.clone())),
            (z, end(Stop::EndTurn, &t, vec![])),
        ];
    }
    let prompt = latest_user_text(req);
    if let Some(say) = directive(&prompt, "say:") {
        return vec![
            (delay, Text(say.to_string())),
            (z, end(Stop::EndTurn, say, vec![])),
        ];
    }
    if let Some(spec) = directive(&prompt, "tool:") {
        if req.final_round {
            return vec![
                (delay, Text("no rounds left".into())),
                (z, end(Stop::EndTurn, "no rounds left", vec![])),
            ];
        }
        let (name, input) = match spec.split_once(' ') {
            Some((n, j)) => (
                n.to_string(),
                serde_json::from_str(j.trim()).unwrap_or_else(|_| json!({})),
            ),
            None => (spec.trim().to_string(), json!({})),
        };
        let call = json!({"type": "tool_use", "id": "toolu_stub_1", "name": name, "input": input});
        return vec![
            (delay, Text("Checking.".into())),
            (
                z,
                ToolUse {
                    id: "toolu_stub_1".into(),
                    name,
                    input,
                },
            ),
            (z, end(Stop::ToolUse, "Checking.", vec![call])),
        ];
    }
    if prompt.contains("[[slow]]") {
        let mut out: Vec<(Duration, ProviderEvent)> = (0..50)
            .map(|_| (Duration::from_millis(100), Text("tick ".into())))
            .collect();
        out.push((z, end(Stop::EndTurn, &"tick ".repeat(50), vec![])));
        return out;
    }
    if prompt.contains("[[refuse]]") {
        return vec![
            (delay, Text("I won".into())),
            (z, end(Stop::Refusal, "I won", vec![])),
        ];
    }
    if prompt.contains("[[empty]]") {
        return vec![(z, end(Stop::EndTurn, "", vec![]))];
    }
    if prompt.contains("[[truncate]]") {
        return vec![
            (delay, Text("cut".into())),
            (z, end(Stop::MaxTokens, "cut", vec![])),
        ];
    }
    if let Some(code) = directive(&prompt, "error-after:") {
        return vec![
            (delay, Text("partial ".into())),
            (
                z,
                Error {
                    code: ProviderErrorCode::parse(code),
                    message: "stub error".into(),
                },
            ),
        ];
    }
    if let Some(code) = directive(&prompt, "error:") {
        return vec![(
            z,
            Error {
                code: ProviderErrorCode::parse(code),
                message: "stub error".into(),
            },
        )];
    }
    if prompt.contains("[[images]]") {
        let n = req
            .messages
            .iter()
            .rev()
            .find(|t| t.role == Role::User)
            .map_or(0, |t| {
                t.content
                    .iter()
                    .filter(|b| matches!(b, Block::Image { .. }))
                    .count()
            });
        let t = format!("saw {n} images");
        return vec![
            (delay, Text(t.clone())),
            (z, end(Stop::EndTurn, &t, vec![])),
        ];
    }
    let whole = format!("echo: {prompt}");
    let chars: Vec<char> = whole.chars().collect();
    let size = chars.len().div_ceil(3).max(1);
    let mut out: Vec<(Duration, ProviderEvent)> = chars
        .chunks(size)
        .map(|c| (delay, Text(c.iter().collect())))
        .collect();
    out.push((z, end(Stop::EndTurn, &whole, vec![])));
    out
}

impl SampleProvider for StubProvider {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn supports_images(&self) -> bool {
        self.images
    }

    fn stream(&self, req: ProviderRequest) -> EventStream {
        let steps = script(&req, self.delay);
        self.requests.lock().expect("stub requests lock").push(req);
        Box::pin(futures::stream::iter(steps).then(|(pause, ev)| async move {
            if !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
            ev
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    fn req(text: &str) -> ProviderRequest {
        ProviderRequest {
            model: "m".into(),
            max_tokens: 10,
            system: "s".into(),
            messages: vec![Turn {
                role: Role::User,
                content: vec![Block::Text(text.into())],
            }],
            tools: vec![],
            final_round: true,
        }
    }

    async fn all(p: &StubProvider, r: ProviderRequest) -> Vec<ProviderEvent> {
        p.stream(r).collect().await
    }

    fn text(evs: &[ProviderEvent]) -> String {
        evs.iter()
            .filter_map(|e| {
                if let ProviderEvent::Text(t) = e {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn echoes_in_three_pieces_and_records_the_request() {
        let p = StubProvider::new(false, Duration::ZERO);
        let evs = all(&p, req("hello there")).await;
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, ProviderEvent::Text(_)))
                .count(),
            3
        );
        assert_eq!(text(&evs), "echo: hello there");
        assert!(matches!(
            evs.last(),
            Some(ProviderEvent::End {
                stop: Stop::EndTurn,
                ..
            })
        ));
        assert_eq!(p.requests().len(), 1);
    }

    #[tokio::test]
    async fn scripts() {
        let p = StubProvider::new(false, Duration::ZERO);
        let evs = all(&p, req("x [[say:{\"a\": 1}]]")).await;
        assert_eq!(text(&evs), "{\"a\": 1}");
        let evs = all(&p, req("[[refuse]]")).await;
        assert!(matches!(
            evs.last(),
            Some(ProviderEvent::End {
                stop: Stop::Refusal,
                ..
            })
        ));
        let evs = all(&p, req("[[empty]]")).await;
        assert_eq!(text(&evs), "");
        let evs = all(&p, req("[[truncate]]")).await;
        assert!(matches!(
            evs.last(),
            Some(ProviderEvent::End {
                stop: Stop::MaxTokens,
                ..
            })
        ));
        let evs = all(&p, req("[[error:rate_limited]]")).await;
        assert_eq!(
            evs,
            vec![ProviderEvent::Error {
                code: ProviderErrorCode::RateLimited,
                message: "stub error".into()
            }]
        );
        let evs = all(&p, req("[[error-after:upstream_error]]")).await;
        assert_eq!(text(&evs), "partial ");
        assert!(matches!(
            evs.last(),
            Some(ProviderEvent::Error {
                code: ProviderErrorCode::Upstream,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_tool_directive_calls_the_tool_then_answers_from_its_result() {
        let p = StubProvider::new(false, Duration::ZERO);
        let first = ProviderRequest {
            final_round: false,
            ..req("[[tool:getColor {\"shade\": \"dark\"}]]")
        };
        let evs = all(&p, first.clone()).await;
        assert_eq!(text(&evs), "Checking.");
        assert!(evs.contains(&ProviderEvent::ToolUse {
            id: "toolu_stub_1".into(),
            name: "getColor".into(),
            input: json!({"shade": "dark"})
        }));
        let ProviderEvent::End {
            stop: Stop::ToolUse,
            assistant,
        } = evs.last().unwrap()
        else {
            panic!("{evs:?}")
        };
        let mut second = first.clone();
        second.messages.push(Turn {
            role: Role::Assistant,
            content: assistant.iter().cloned().map(Block::Raw).collect(),
        });
        second.messages.push(Turn {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "toolu_stub_1".into(),
                content: "teal".into(),
                is_error: false,
            }],
        });
        assert_eq!(text(&all(&p, second).await), "tool said: teal");
        let last = ProviderRequest {
            final_round: true,
            ..req("[[tool:getColor]]")
        };
        assert_eq!(text(&all(&p, last).await), "no rounds left");
    }

    #[tokio::test]
    async fn counts_the_images_it_was_shown() {
        let p = StubProvider::new(true, Duration::ZERO);
        let mut r = req("[[images]]");
        r.messages[0].content.insert(
            0,
            Block::Image {
                media_type: "image/png".into(),
                data: "AAAA".into(),
            },
        );
        assert_eq!(text(&all(&p, r).await), "saw 1 images");
        assert!(p.supports_images());
    }
}

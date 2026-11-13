//! What the run loop answers a turn with when it is not a script's
//! result: one golden a case, of the messages a model is sent next.

use super::names::{self as g, Golden};
use super::pins::driven;
use crate::common::*;
use serde_json::{Value, json};

/// A thread run whose turns are `replies`, then a turn that calls
/// `finish`: pins, as `golden`, every message of the last request after
/// the request message, which is the turns before it as kept and what
/// the run loop answered each with.
async fn pin_answer(golden: Golden, replies: Vec<Reply>) {
    let env = Env::new().await;
    let turns = replies.len();
    for reply in replies {
        env.api.push(reply);
    }
    env.api.push(finish_turn("done"));
    let run = env.run();
    let p = driven(env, run).await;
    let reqs = p.env.api.requests();
    assert_eq!(reqs.len(), turns + 1, "each turn, and the finish");
    let messages = reqs[turns].body["messages"].as_array().unwrap();
    p.pin(
        &format!(
            "what a model is sent after {turns} turn(s) ({})",
            golden.file()
        ),
        golden,
        &json!(messages[1..]),
    );
}

fn tool(id: &'static str, name: &'static str, input: Value) -> B {
    B::Tool(id, name, input.to_string())
}

#[tokio::test]
async fn a_reply_cut_at_max_tokens_without_a_call_is_asked_to_continue() {
    pin_answer(
        g::ANSWER_MAX_TOKENS_NO_TOOL_CALL,
        vec![stream(&[B::Text("First I will")], "max_tokens", USAGE)],
    )
    .await;
}

#[tokio::test]
async fn a_call_cut_at_max_tokens_is_answered_with_an_error() {
    pin_answer(
        g::ANSWER_MAX_TOKENS_CUT_TOOL_CALL,
        vec![stream(
            &[tool(
                "tu_1",
                "run_script",
                json!({"title": "Edit", "code": "return 1;"}),
            )],
            "max_tokens",
            USAGE,
        )],
    )
    .await;
}

/// A paused turn is continued with no new message.
#[tokio::test]
async fn a_paused_turn_is_continued_as_it_is() {
    pin_answer(
        g::ANSWER_PAUSE_TURN,
        vec![stream(&[B::Text("Working on it.")], "pause_turn", USAGE)],
    )
    .await;
}

/// The turn that continues a paused one is kept in the paused turn's
/// message, after its blocks: two turns of one role are one message. Here
/// the second has no tool call, so the nudge follows.
#[tokio::test]
async fn the_turn_after_a_paused_one_is_merged_into_its_message() {
    pin_answer(
        g::ANSWER_PAUSE_TURN_THEN_NO_TOOL_CALL,
        vec![
            stream(&[B::Text("Working on it.")], "pause_turn", USAGE),
            text_turn("The heading is fine."),
        ],
    )
    .await;
}

#[tokio::test]
async fn tool_input_that_is_not_json_is_answered_with_an_error() {
    pin_answer(
        g::ANSWER_TOOL_INPUT_NOT_JSON,
        vec![stream(
            &[B::Tool(
                "tu_1",
                "run_script",
                r#"{"title": "Edit", "code": "retur"#.to_string(),
            )],
            "tool_use",
            USAGE,
        )],
    )
    .await;
}

#[tokio::test]
async fn tool_input_that_does_not_match_its_schema_is_answered_with_an_error() {
    pin_answer(
        g::ANSWER_TOOL_INPUT_NOT_ITS_SCHEMA,
        vec![stream(
            &[tool("tu_1", "run_script", json!({"title": 1}))],
            "tool_use",
            USAGE,
        )],
    )
    .await;
}

/// `finish` with a summary that is not a string: answered, and the run
/// goes on.
#[tokio::test]
async fn finish_input_that_does_not_match_its_schema_is_answered_with_an_error() {
    pin_answer(
        g::ANSWER_TOOL_INPUT_FINISH_NOT_ITS_SCHEMA,
        vec![stream(
            &[tool("tu_1", "finish", json!({"summary": 1}))],
            "tool_use",
            USAGE,
        )],
    )
    .await;
}

/// `give_up` without a reason: answered, and the run goes on.
#[tokio::test]
async fn give_up_input_that_does_not_match_its_schema_is_answered_with_an_error() {
    pin_answer(
        g::ANSWER_TOOL_INPUT_GIVE_UP_NOT_ITS_SCHEMA,
        vec![stream(
            &[tool("tu_1", "give_up", json!({}))],
            "tool_use",
            USAGE,
        )],
    )
    .await;
}

#[tokio::test]
async fn an_unknown_tool_is_answered_with_an_error() {
    pin_answer(
        g::ANSWER_TOOL_UNKNOWN,
        vec![stream(
            &[tool("tu_1", "search", json!({"query": "blue"}))],
            "tool_use",
            USAGE,
        )],
    )
    .await;
}

#[tokio::test]
async fn several_tool_calls_in_a_reply_are_each_answered_with_an_error() {
    pin_answer(
        g::ANSWER_TOOL_SEVERAL_CALLS,
        vec![stream(
            &[
                tool(
                    "tu_1",
                    "run_script",
                    json!({"title": "Edit", "code": "return 1;"}),
                ),
                tool("tu_2", "finish", json!({"summary": "done"})),
            ],
            "tool_use",
            USAGE,
        )],
    )
    .await;
}

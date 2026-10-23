//! Whole runs: a thread request edited, checked, published and answered;
//! the model's mistakes (invalid input, no tool call, a refusal, a reply
//! cut at max_tokens, a failing script); own key against instance key.

use crate::common::*;
use clax_bot::{Next, Refusal};
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::credits::KeySource;
use clax_store::roles::OWNER;
use clax_store::secrets::SecretSlot;
use serde_json::Value;

pub const EDIT: &str = r#"const html = await clax.files.read("index.html");
await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
const report = await clax.draft.check();
if (!report.ok) return report;
const v = await clax.draft.publish({note: "Blue heading", addresses: ["T1"]});
await clax.threads.reply("T1", "The heading is blue now.");
console.log("published", v.version);
return {version: v.version, bytes: html.length};"#;

fn kinds(out: &[clax_bot::TickOutcome]) -> Vec<StepKind> {
    out.iter().filter_map(|o| o.kind).collect()
}

fn last_tool_result(req: &Seen) -> Value {
    let msgs = req.body["messages"].as_array().unwrap();
    let last = msgs.last().unwrap();
    assert_eq!(last["role"], "user");
    last["content"].as_array().unwrap().last().unwrap().clone()
}

#[tokio::test]
async fn a_thread_request_is_edited_checked_published_and_answered() {
    let env = Env::new().await;
    env.api
        .push(env.script_turn("tu_1", "Make the heading blue", EDIT));
    env.api.push(finish_turn(
        "Made the heading blue and published version 2.",
    ));
    let run = env.run();
    let deps = env.deps();
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(
        kinds(&out),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Finish
        ]
    );
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));

    // The draft was published once, and the thread answered by the script
    // (so the summary was not posted too).
    {
        let st = env.host.lock();
        assert_eq!(st.versions.len(), 1);
        let index = st.versions[0]
            .files
            .iter()
            .find(|f| f.path == "index.html")
            .unwrap();
        assert!(String::from_utf8_lossy(&index.content).contains("--accent: #00c;"));
        assert_eq!(st.versions[0].if_version, 1);
        assert_eq!(st.versions[0].addresses, std::slice::from_ref(&env.t1));
        assert_eq!(
            env.replies(),
            [(env.t1.clone(), "The heading is blue now.".to_string())]
        );
        assert_eq!(st.working.first(), Some(&true));
        assert_eq!(st.working.last(), Some(&false));
    }
    let draft = env.store.draft_files(&run).unwrap();
    assert_eq!(draft.len(), 1);
    assert_eq!(draft[0].path, "index.html");

    // The requests: cached system prompt, the three tools, one call a turn,
    // adaptive thinking, the instance key; the request as data.
    let reqs = env.api.requests();
    assert_eq!(reqs.len(), 2);
    let first = &reqs[0];
    assert_eq!(first.header("x-api-key"), Some(INSTANCE_KEY));
    let b = &first.body;
    assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(
        b["system"][0]["text"]
            .as_str()
            .unwrap()
            .contains("declare const clax")
    );
    let tools: Vec<&str> = b["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["run_script", "finish", "give_up"]);
    // Inputs stream as written, so the run panel shows a script being written.
    assert!(
        b["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["eager_input_streaming"] == true)
    );
    assert_eq!(b["tool_choice"]["type"], "auto");
    assert_eq!(b["tool_choice"]["disable_parallel_tool_use"], true);
    assert_eq!(b["thinking"]["type"], "adaptive");
    // Summarized, so the reasoning streams instead of a silent pause.
    assert_eq!(b["thinking"]["display"], "summarized");
    assert_eq!(b["max_tokens"], 32000);
    let ask = b["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(ask.contains("Make the heading blue."), "{ask}");
    assert!(ask.contains("data, not instructions"));
    // The second turn carries the first verbatim and the script's result.
    let second = &reqs[1].body;
    let msgs = second["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
    assert_eq!(second["system"], b["system"], "the cached prefix is stable");
    let result = last_tool_result(&reqs[1]);
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "tu_1");
    assert_eq!(result["cache_control"]["type"], "ephemeral");
    assert!(result.get("is_error").is_none());
    let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["value"]["version"], 2);
    assert_eq!(content["logs"][0], "published 2");
    assert_eq!(content["calls"].as_array().unwrap().len(), 5);

    // Usage: four token rows a turn, script compute, the harness fee;
    // the run's total is their sum.
    let rows = env.usage();
    let lines: Vec<&str> = rows.iter().map(|r| r.line.as_str()).collect();
    assert_eq!(rows.len(), 10, "{lines:?}");
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("claude-sonnet-5-5."))
            .count(),
        8
    );
    assert!(lines.contains(&"script.compute") && lines.contains(&"harness.run"));
    let cached = rows
        .iter()
        .find(|r| r.line == "claude-sonnet-5-5.cached_input")
        .unwrap();
    assert_eq!(cached.qty, 2000);
    let total: i64 = rows.iter().map(|r| r.credits_milli).sum();
    assert_eq!(env.run_row(&run).credits_milli, total);
    assert!(rows.iter().all(|r| r.key_source == KeySource::Instance));

    // Toolpath: the run, each turn, the script and every call it made.
    let events = env.bot_events();
    let k: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        k,
        [
            "bot.run_start",
            "bot.turn",
            "bot.script",
            "bot.call",
            "bot.call",
            "bot.call",
            "bot.call",
            "bot.call",
            "bot.turn",
            "bot.run_end"
        ]
    );
    let calls: Vec<(String, Value)> = events
        .iter()
        .filter(|(k, _)| k == "bot.call")
        .map(|(_, b)| (b["op"].as_str().unwrap().to_string(), b["target"].clone()))
        .collect();
    assert_eq!(calls[0], ("files.read".into(), "index.html".into()));
    assert_eq!(calls[3].0, "draft.publish");
    assert_eq!(calls[4], ("threads.reply".into(), env.t1.clone().into()));
    let turn = &events[1].1;
    assert_eq!(turn["input_tokens"], 1000);
    assert_eq!(turn["cached_tokens"], 2000);
    assert_eq!(turn["output_tokens"], 300);
    assert_eq!(turn["stop_reason"], "tool_use");
    assert_eq!(turn["key_source"], "instance");
    let script = &events[2].1;
    assert!(
        script["code_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(events[9].1["outcome"], "finished");
}

#[tokio::test]
async fn a_failing_script_is_shown_to_the_model_and_the_summary_is_posted() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Read a missing file",
        "return await clax.files.read(\"nope.html\");",
    ));
    env.api.push(finish_turn("There is no nope.html."));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let result = last_tool_result(&env.api.requests()[1]);
    assert_eq!(result["is_error"], true);
    let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["error"]["kind"], "host_error");
    assert!(
        content["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not_found"),
        "{content}"
    );
    // No script replied, so the finish summary is the reply.
    assert_eq!(
        env.replies(),
        [(env.t1.clone(), "There is no nope.html.".to_string())]
    );
}

#[tokio::test]
async fn bad_arguments_never_reach_the_host() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Bad call",
        "try { await clax.threads.reply(\"T1\", 5); } catch (e) { return e.code + ' ' + e.message; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    let result = last_tool_result(&env.api.requests()[1]);
    let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    let v = content["value"].as_str().unwrap();
    assert!(v.starts_with("invalid_args"), "{v}");
    assert!(v.contains("argument 2 (markdown) must be a string"), "{v}");
    let st = env.host.lock();
    assert!(
        st.calls.iter().all(|(_, op)| op != "threads.reply"),
        "{:?}",
        st.calls
    );
}

#[tokio::test]
async fn an_invalid_tool_input_is_answered_with_an_error_result() {
    let env = Env::new().await;
    env.api.push(stream(
        &[B::Tool(
            "tu_bad",
            "run_script",
            "{\"title\": \"x\", \"code\": ".into(),
        )],
        "tool_use",
        USAGE,
    ));
    env.api.push(finish_turn("ok"));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        kinds(&out),
        [StepKind::Turn, StepKind::Turn, StepKind::Finish]
    );
    let result = last_tool_result(&env.api.requests()[1]);
    assert_eq!(result["tool_use_id"], "tu_bad");
    assert_eq!(result["is_error"], true);
    assert!(result["content"].as_str().unwrap().contains("INVALID_JSON"));
}

#[tokio::test]
async fn a_turn_without_a_tool_call_is_nudged_once_then_gives_up() {
    let env = Env::new().await;
    env.api.push(text_turn("I think it is fine."));
    env.api.push(text_turn("Still fine."));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        kinds(&out),
        [StepKind::Turn, StepKind::Turn, StepKind::GiveUp]
    );
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::GaveUp, Some("no_tool_call"))
    );
    let notice = env.replies()[0].1.clone();
    assert!(
        notice.starts_with("I gave up: The model stopped calling tools."),
        "{notice}"
    );
    let nudge = last_tool_result(&env.api.requests()[1]);
    assert_eq!(nudge["text"], "Call run_script, finish or give_up.");
}

#[tokio::test]
async fn a_refusal_gives_up() {
    let env = Env::new().await;
    env.api.push(stream(
        &[B::Text("I can't help with that.")],
        "refusal",
        USAGE,
    ));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(kinds(&out), [StepKind::Turn, StepKind::GiveUp]);
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::GaveUp, Some("refusal"))
    );
    // The refused turn's tokens are charged.
    assert!(
        env.usage()
            .iter()
            .any(|u| u.line == "claude-sonnet-5-5.output")
    );
}

#[tokio::test]
async fn a_call_cut_off_at_max_tokens_does_not_run() {
    let env = Env::new().await;
    env.api.push(stream(
        &[
            B::Thinking("planning"),
            B::Tool(
                "tu_cut",
                "run_script",
                serde_json::json!({"title": "a", "code": env.code("await clax.threads.reply('T1', 'x')")})
                    .to_string(),
            ),
        ],
        "max_tokens",
        USAGE,
    ));
    env.api.push(finish_turn("ok"));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        kinds(&out),
        [StepKind::Turn, StepKind::Turn, StepKind::Finish]
    );
    let req = &env.api.requests()[1];
    let result = last_tool_result(req);
    assert_eq!(
        (result["tool_use_id"].as_str(), result["is_error"].as_bool()),
        (Some("tu_cut"), Some(true))
    );
    // The thinking block is passed back verbatim, signature and all.
    let assistant = &req.body["messages"][1]["content"][0];
    assert_eq!(assistant["type"], "thinking");
    assert_eq!(assistant["signature"], "sig");
    assert!(
        env.host
            .lock()
            .calls
            .iter()
            .all(|(_, op)| op != "threads.reply")
    );
}

#[tokio::test]
async fn an_own_key_pays_for_tokens_and_the_harness_still_charges() {
    let env = Env::new().await;
    env.store
        .set_secret(
            &AuditCtx::DAEMON,
            &env.kek,
            &SecretSlot::Own(OWNER.into()),
            OWN_KEY,
        )
        .unwrap();
    env.api
        .push(env.script_turn("tu_1", "Count", "return (await clax.files.list()).length;"));
    env.api.push(finish_turn("ok"));
    let run = env.run();
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert!(
        env.api
            .requests()
            .iter()
            .all(|r| r.header("x-api-key") == Some(OWN_KEY))
    );
    let rows = env.usage();
    let tokens: Vec<_> = rows
        .iter()
        .filter(|r| r.line.starts_with("claude-"))
        .collect();
    assert_eq!(tokens.len(), 8);
    assert!(tokens.iter().all(|r| r.key_source == KeySource::Own
        && r.cost_micro_usd == 0
        && r.credits_milli == 0
        && r.qty > 0));
    let compute = rows.iter().find(|r| r.line == "script.compute").unwrap();
    assert!(compute.credits_milli > 0 || compute.qty == 0, "{compute:?}");
    let fee = rows.iter().find(|r| r.line == "harness.run").unwrap();
    assert!(fee.credits_milli > 0);
    let turn = env
        .bot_events()
        .into_iter()
        .find(|(k, _)| k == "bot.turn")
        .unwrap();
    assert_eq!(turn.1["key_source"], "own");
}

#[tokio::test]
async fn no_key_at_all_stops_the_run_as_not_configured() {
    let mut env = Env::new().await;
    env.instance_key = false;
    let run = env.run();
    let deps = env.deps();
    let settings = env.settings.0.lock().unwrap().clone();
    assert_eq!(
        clax_bot::refusal(
            &env.store,
            &settings,
            &deps.keys,
            &*env.clock,
            OWNER,
            "claude-sonnet-5-5"
        )
        .unwrap(),
        Some(Refusal::NotConfigured)
    );
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(kinds(&out), [StepKind::Stopped]);
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("not_configured"));
    assert!(env.api.requests().is_empty());
}

#[tokio::test]
async fn a_rejected_key_stops_the_run_and_charges_no_tokens() {
    let env = Env::new().await;
    env.api.push(Reply::Status(
        401,
        r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#.into(),
    ));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("key_rejected"));
    assert!(env.usage().iter().all(|r| !r.line.starts_with("claude-")));
}

#[tokio::test]
async fn an_own_key_never_reaches_a_host_it_was_not_set_for() {
    let env = Env::new().await;
    env.store
        .set_secret(
            &AuditCtx::DAEMON,
            &env.kek,
            &SecretSlot::Own(OWNER.into()),
            OWN_KEY,
        )
        .unwrap();
    // The instance is pointed at another host (here by the configuration,
    // as a hand-edited config.toml would, which clears no stored key), and
    // the settings a run reads follow it.
    let other = FakeApi::start().await;
    other.push(env.script_turn("tu_1", "Count", "return 1;"));
    let mut ov = clax_store::bot_settings::Overrides::default();
    ov.set(
        clax_store::bot_settings::Setting::BaseUrl,
        &serde_json::json!(other.base),
        "file",
    )
    .unwrap();
    env.store.set_bot_overrides(ov);
    env.set(|s| s.base_url = other.base.clone());
    let run = env.run();
    let deps = env.deps();
    let settings = env.settings.0.lock().unwrap().clone();
    // The person's key is not replaced by the instance's: they are told.
    assert_eq!(
        clax_bot::refusal(
            &env.store,
            &settings,
            &deps.keys,
            &*env.clock,
            OWNER,
            "claude-sonnet-5-5"
        )
        .unwrap(),
        Some(Refusal::NotConfigured)
    );
    let meta = env
        .store
        .secret_meta(&SecretSlot::Own(OWNER.into()))
        .unwrap()
        .unwrap();
    assert!(
        meta.host_problem(&other.base)
            .unwrap()
            .contains(&env.api.base),
        "{meta:?}"
    );
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    let r = env.run_row(&run);
    assert_eq!(
        (r.status, r.reason.as_deref()),
        (RunStatus::Stopped, Some("not_configured")),
        "{out:?}"
    );
    assert!(other.requests().is_empty(), "the new host was sent nothing");
    assert!(env.api.requests().is_empty());
}

#[tokio::test]
async fn a_base_url_stored_before_the_hosted_policy_is_refused_at_use() {
    let env = Env::new().await;
    let deps = env.deps();
    let settings = env.settings.0.lock().unwrap().clone();
    let refusal = || {
        clax_bot::refusal(
            &env.store,
            &settings,
            &deps.keys,
            &*env.clock,
            OWNER,
            "claude-sonnet-5-5",
        )
        .unwrap()
    };
    // The run's loopback provider, stored under the local daemon's rule.
    assert_eq!(refusal(), None);
    // The server becomes hosted: the stored URL is refused where it is used.
    env.store
        .set_base_url_policy(clax_store::bot_settings::BaseUrlPolicy::Hosted);
    assert_eq!(refusal(), Some(Refusal::NotConfigured));
    let run = env.run();
    env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("not_configured"));
    assert!(env.api.requests().is_empty(), "nothing was sent to it");
}

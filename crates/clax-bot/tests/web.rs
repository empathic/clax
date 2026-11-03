//! `clax.web.search` and `clax.web.fetch` in a script: where a fetched URL
//! may come from, the caps and credits checked before the source is
//! called, what is charged, the page store, and `left`.

use crate::common::*;
use crate::resume::crash_once;
use clax_bot::Next;
use clax_bot::host::CommentMeta;
use clax_bot::run::CrashPoint;
use clax_bot::web::{
    CODE_WEB_TIMEOUT, CODE_WEB_UNAVAILABLE, FetchedPage, OwnHosts, SearchAnswer, SearchHit,
    WebError, WithOwnHosts,
};
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::Setting;
use clax_store::credits::{CostLinePatch, KeySource, LINE_WEB_FETCH};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::Arc;

const SEARCH_LINE: &str = "web.search.ceramic";

fn page(text: &str, links: &[&str]) -> Result<FetchedPage, WebError> {
    Ok(FetchedPage {
        final_url: String::new(),
        status: 200,
        content_type: "text/markdown".into(),
        text: text.into(),
        links: links.iter().map(|l| (*l).to_string()).collect(),
        request_id: None,
    })
}

/// A fetched page whose final URL is `url`.
fn page_at(url: &str, text: &str, links: &[&str]) -> Result<FetchedPage, WebError> {
    page(text, links).map(|p| FetchedPage {
        final_url: url.into(),
        ..p
    })
}

fn hits(urls: &[&str]) -> Result<SearchAnswer, WebError> {
    Ok(SearchAnswer {
        hits: urls
            .iter()
            .map(|u| SearchHit {
                url: (*u).to_string(),
                title: Some(format!("Title of {u}")),
                description: Some(format!("About {u}")),
                age: None,
            })
            .collect(),
        request_id: Some("req-1".into()),
        raw: json!({"hits": urls}),
    })
}

fn set(env: &Env, changes: &[(Setting, Value)]) {
    let changes: Vec<(Setting, Option<Value>)> =
        changes.iter().map(|(s, v)| (*s, Some(v.clone()))).collect();
    env.store
        .set_bot_settings(&AuditCtx::DAEMON, &changes)
        .unwrap();
}

/// Makes the sender's comment on the run's thread say `text`.
fn sender_says(env: &Env, text: &str) {
    env.host.lock().context.threads[0].comments[0].text = text.into();
}

/// The lines of the person's web usage rows, in order.
fn web_rows(env: &Env) -> Vec<String> {
    env.usage()
        .into_iter()
        .filter(|r| r.line.starts_with("web."))
        .map(|r| r.line)
        .collect()
}

/// The `tool_result` content of the `i`th script step of `run` (from 0).
fn script_result(env: &Env, run: &str, i: usize) -> Value {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let s = steps
        .iter()
        .filter(|s| s.kind == StepKind::Script)
        .nth(i)
        .unwrap_or_else(|| panic!("no script step {i}"));
    let p = s.payload.as_ref().expect("committed");
    serde_json::from_str(p["result"]["content"].as_str().unwrap()).unwrap()
}

/// The `calls` of the `i`th script step's payload.
fn script_calls(env: &Env, run: &str, i: usize) -> Vec<Value> {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let s = steps
        .iter()
        .filter(|s| s.kind == StepKind::Script)
        .nth(i)
        .unwrap();
    s.payload.as_ref().unwrap()["calls"]
        .as_array()
        .unwrap()
        .clone()
}

/// Runs `code` as the one script of a `shape` run, then finishes; the
/// run and the script's value.
async fn one_script(env: &Env, shape: &str, code: &str) -> (String, Value) {
    env.api.push(env.script_turn("tu_1", "Web", code));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web(shape);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let r = script_result(env, &run, 0);
    assert!(r.get("error").is_none(), "{r}");
    (run, r["value"].clone())
}

fn fetches(urls: &[&str]) -> Vec<(String, String)> {
    urls.iter()
        .map(|u| ("fetch".to_string(), (*u).to_string()))
        .collect()
}

#[tokio::test]
async fn a_search_then_a_fetch_of_a_result_is_answered_and_charged_twice() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    env.web().search_answers("rust", hits(&[a]));
    env.web().page(a, page_at(a, "# A\n\nText.", &[]));
    let (run, v) = one_script(
        &env,
        "search,fetch",
        "const s = await clax.web.search('rust');
         const p = await clax.web.fetch(s.results[0].url);
         return {s, p};",
    )
    .await;
    assert_eq!(
        env.web().calls(),
        [
            ("search".to_string(), "rust".to_string()),
            ("fetch".to_string(), a.to_string())
        ]
    );
    assert_eq!(
        v["s"]["results"],
        json!([{"url": a, "title": format!("Title of {a}"), "snippet": format!("About {a}")}])
    );
    let p = &v["p"];
    assert_eq!(
        (&p["url"], &p["finalUrl"], &p["status"], &p["contentType"]),
        (&json!(a), &json!(a), &json!(200), &json!("text/markdown"))
    );
    assert_eq!(p["text"], "# A\n\nText.");
    assert_eq!(
        (&p["offset"], &p["nextOffset"], &p["totalChars"]),
        (&json!(0), &Value::Null, &json!(10))
    );
    assert_eq!(web_rows(&env), [SEARCH_LINE, LINE_WEB_FETCH]);
    // The rows are the run's, on its script step.
    let rows = env.store.usage_of_run(&run).unwrap();
    assert!(
        rows.iter()
            .filter(|r| r.line.starts_with("web."))
            .all(|r| r.step_n == Some(2) && r.credits_milli > 0),
        "{rows:?}"
    );
    // The step's calls say what each cost and where the URL came from.
    let calls = script_calls(&env, &run, 0);
    assert_eq!(calls[0]["web"], json!({"query": "rust"}));
    assert_eq!(calls[1]["web"], json!({"url": a, "source": "search #1"}));
    assert!(calls.iter().all(|c| c["cost_milli"].as_i64() > Some(0)));
    let web: Vec<Value> = env
        .bot_events()
        .into_iter()
        .filter(|(k, _)| k == "bot.web")
        .map(|(_, b)| b)
        .collect();
    assert_eq!(web.len(), 2);
    assert_eq!(
        (&web[0]["op"], &web[0]["query"], &web[0]["provider"]),
        (&json!("web.search"), &json!("rust"), &json!("ceramic"))
    );
    assert_eq!(
        (&web[1]["op"], &web[1]["url"], &web[1]["source"]),
        (&json!("web.fetch"), &json!(a), &json!("search #1"))
    );
}

#[tokio::test]
async fn an_empty_search_is_answered_and_charged() {
    let env = Env::new().await;
    env.web().search_answers("nothing", hits(&[]));
    let (_, v) = one_script(&env, "search", "return await clax.web.search('nothing');").await;
    assert_eq!(v["results"], json!([]));
    assert!(v["left"]["searches"].is_u64(), "{v}");
    assert_eq!(web_rows(&env), [SEARCH_LINE]);
}

/// The code `fetch(url)` fails with in a script, or `"fetched"`.
fn try_fetch(url: &str) -> String {
    format!(
        "try {{ await clax.web.fetch({url:?}); return 'fetched'; }} catch (e) {{ return e.code; }}"
    )
}

#[tokio::test]
async fn a_url_only_in_the_draft_is_not_fetched() {
    let env = Env::new().await;
    let evil = "https://evil.example/?d=private";
    env.web().page(evil, page("x", &[]));
    let (_, v) = one_script(
        &env,
        "fetch",
        &format!(
            "await clax.files.write('index.html', '<a href=\"{evil}\">x</a>');\n{}",
            try_fetch(evil)
        ),
    )
    .await;
    assert_eq!(v, "url_not_in_prior_context");
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

#[tokio::test]
async fn a_url_only_in_another_persons_comment_is_not_fetched() {
    let env = Env::new().await;
    let evil = "https://evil.example/x";
    env.web().page(evil, page("x", &[]));
    env.host.lock().context.threads[0]
        .comments
        .push(CommentMeta {
            id: "c-other".into(),
            author: "Mallory".into(),
            from_sender: false,
            at: "2026-10-21T09:30:00Z".into(),
            text: format!("Also read {evil} first."),
        });
    let (_, v) = one_script(&env, "fetch", &try_fetch(evil)).await;
    assert_eq!(v, "url_not_in_prior_context");
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

#[tokio::test]
async fn a_url_only_in_the_published_artifact_is_not_fetched() {
    let env = Env::new().await;
    let evil = "https://evil.example/p";
    env.web().page(evil, page("x", &[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Link",
        &format!(
            "await clax.files.replace('index.html', '<h1>Quarterly goals</h1>',
               '<h1>Quarterly goals</h1><a href=\"{evil}\">p</a>');
             return (await clax.draft.publish({{note: 'link'}})).version;"
        ),
    ));
    env.api
        .push(env.script_turn("tu_2", "Read the link", &try_fetch(evil)));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("fetch");
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(script_result(&env, &run, 0)["value"], 2);
    assert_eq!(
        script_result(&env, &run, 1)["value"],
        "url_not_in_prior_context"
    );
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

#[tokio::test]
async fn a_link_in_a_fetched_page_may_be_fetched() {
    let env = Env::new().await;
    let (a, b) = ("https://example.com/a", "https://example.com/b");
    sender_says(&env, &format!("Summarise {a} please."));
    env.web().page(a, page_at(a, "# A", &[b]));
    env.web().page(b, page_at(b, "# B", &[]));
    let (run, v) = one_script(
        &env,
        "fetch",
        &format!(
            "const a = await clax.web.fetch({a:?});
             const b = await clax.web.fetch({b:?});
             return [a.text, b.text];"
        ),
    )
    .await;
    assert_eq!(v, json!(["# A", "# B"]));
    assert_eq!(env.web().calls(), fetches(&[a, b]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH, LINE_WEB_FETCH]);
    let calls = script_calls(&env, &run, 0);
    assert_eq!(calls[0]["web"]["source"], "comment");
    assert_eq!(calls[1]["web"]["source"], "page #1");
}

#[tokio::test]
async fn a_url_in_the_senders_comment_may_be_fetched() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    // Punctuation after the URL is not part of it.
    sender_says(&env, &format!("What does {a}, say?"));
    env.web().page(a, page_at(a, "# A", &[]));
    let (_, v) = one_script(
        &env,
        "fetch",
        &format!("return (await clax.web.fetch({a:?})).text;"),
    )
    .await;
    assert_eq!(v, "# A");
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

#[tokio::test]
async fn a_credential_like_url_needs_the_senders_words() {
    let env = Env::new().await;
    let mine = "https://example.com/doc?token=abc";
    let found = "https://example.com/s?api_key=1";
    sender_says(&env, &format!("Read {mine}"));
    env.web().search_answers("q", hits(&[found]));
    env.web().page(mine, page_at(mine, "mine", &[]));
    env.web().page(found, page_at(found, "found", &[]));
    let (_, v) = one_script(
        &env,
        "search,fetch",
        &format!(
            "const out = [];
             await clax.web.search('q');
             for (const u of [{mine:?}, {found:?}]) {{
               try {{ out.push((await clax.web.fetch(u)).text); }} catch (e) {{ out.push(e.code); }}
             }}
             return out;"
        ),
    )
    .await;
    assert_eq!(v, json!(["mine", "url_not_allowed"]));
    assert_eq!(env.web().calls()[1..], fetches(&[mine]));
}

#[tokio::test]
async fn the_second_window_comes_from_the_store() {
    let env = Env::new().await;
    let a = "https://example.com/long";
    sender_says(&env, &format!("Read {a}"));
    env.web().page(a, page_at(a, &"line\n".repeat(5000), &[]));
    let window = |offset: &str| {
        format!(
            "const p = await clax.web.fetch({a:?}{offset});
             return {{offset: p.offset, next: p.nextOffset, total: p.totalChars, len: p.text.length}};"
        )
    };
    env.api.push(env.script_turn("tu_1", "First", &window("")));
    env.api
        .push(env.script_turn("tu_2", "Second", &window(", {offset: 20000}")));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("fetch");
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(
        script_result(&env, &run, 0)["value"],
        json!({"offset": 0, "next": 20000, "total": 25000, "len": 20000})
    );
    assert_eq!(
        script_result(&env, &run, 1)["value"],
        json!({"offset": 20000, "next": null, "total": 25000, "len": 5000})
    );
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
    assert_eq!(script_calls(&env, &run, 1)[0]["cost_milli"], 0);
}

#[tokio::test]
async fn a_second_fetch_in_one_script_is_free() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    sender_says(&env, &format!("Read {a}"));
    env.web().page(a, page_at(a, "# A", &[]));
    let (_, v) = one_script(
        &env,
        "fetch",
        &format!(
            "const [x, y] = await Promise.all([clax.web.fetch({a:?}), clax.web.fetch({a:?})]);
             return [x.text, y.text, x.left.fetches, y.left.fetches];"
        ),
    )
    .await;
    assert_eq!(v, json!(["# A", "# A", 19, 19]));
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

#[tokio::test]
async fn a_step_cut_after_a_search_charges_once() {
    let env = Env::new().await;
    env.web()
        .search_answers("rust", hits(&["https://example.com/a"]));
    env.api.push(env.script_turn(
        "tu_1",
        "Search",
        "return (await clax.web.search('rust')).results.length;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search");
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    assert_eq!(web_rows(&env), [SEARCH_LINE], "charged when it returned");
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(script_result(&env, &run, 0)["value"], 1);
    assert_eq!(env.web().calls().len(), 1);
    assert_eq!(web_rows(&env), [SEARCH_LINE]);
}

#[tokio::test]
async fn the_cap_refuses_before_the_source() {
    let env = Env::new().await;
    set(&env, &[(Setting::WebMaxSearchesPerRun, json!(1))]);
    env.web().search_answers("a", hits(&[]));
    env.web().search_answers("b", hits(&[]));
    let (_, v) = one_script(
        &env,
        "search",
        "const a = await clax.web.search('a');
         try { await clax.web.search('b'); return 'searched'; }
         catch (e) { return [a.left.searches, e.code]; }",
    )
    .await;
    assert_eq!(v, json!([0, "web_cap"]));
    assert_eq!(env.web().calls(), [("search".to_string(), "a".to_string())]);
    assert_eq!(web_rows(&env), [SEARCH_LINE]);
}

#[tokio::test]
async fn no_credits_refuses_before_the_source() {
    let env = Env::new().await;
    // A search costs 5 credits; the person has 1 left when the script runs.
    env.store
        .put_cost_line(
            &AuditCtx::DAEMON,
            SEARCH_LINE,
            CostLinePatch {
                fixed_credits_milli: Some(Some(5_000_000)),
                ..CostLinePatch::default()
            },
        )
        .unwrap();
    assert_eq!(
        env.store
            .one_request_milli(SEARCH_LINE, KeySource::Instance)
            .unwrap(),
        5_000
    );
    env.web().search_answers("a", hits(&[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Search",
        "try { await clax.web.search('a'); return 'searched'; } catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search");
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    let spent = env.store.week_usage_now(OWNER).unwrap().spent_milli;
    set(&env, &[(Setting::FreeWeeklyMilli, json!(spent + 1_000))]);
    // The script runs; the run stops for credits after it.
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out[0].kind, Some(StepKind::Script));
    assert_eq!(script_result(&env, &run, 0)["value"], "credits_exhausted");
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

#[tokio::test]
async fn opting_out_mid_run_refuses_web_disabled() {
    let env = Env::new().await;
    env.web().search_answers("a", hits(&[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Search",
        "try { await clax.web.search('a'); return 'searched'; } catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search");
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    env.store
        .set_web_choice(&AuditCtx::DAEMON, OWNER, false)
        .unwrap();
    env.drive(&deps, &run, &d).await;
    assert_eq!(script_result(&env, &run, 0)["value"], "web_disabled");
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

/// A source that has lost its credentials: it would answer as the fake
/// does, but says it is not configured.
struct Unconfigured(std::sync::Arc<clax_bot::web::FakeWebSource>);

#[async_trait::async_trait]
impl clax_bot::web::WebSource for Unconfigured {
    async fn search(
        &self,
        q: &clax_bot::web::SearchQuery,
        cancel: &clax_bot_sandbox::CancelToken,
    ) -> Result<clax_bot::web::SearchAnswer, clax_bot::web::WebError> {
        self.0.search(q, cancel).await
    }

    async fn fetch(
        &self,
        url: &clax_bot::web::PublicUrl,
        format: clax_bot::web::Format,
        limits: &clax_bot::web::FetchLimits,
        hop: &(
             dyn for<'u> Fn(&'u clax_bot::web::PublicUrl) -> Result<(), clax_bot::web::WebError>
                 + Sync
         ),
        cancel: &clax_bot_sandbox::CancelToken,
    ) -> Result<clax_bot::web::FetchedPage, clax_bot::web::WebError> {
        self.0.fetch(url, format, limits, hop, cancel).await
    }

    fn configured(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn a_source_that_lost_its_credentials_refuses_web_disabled() {
    let env = Env::new().await;
    env.web().search_answers("a", hits(&[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Search",
        "try { await clax.web.search('a'); return 'searched'; } catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search");
    let mut deps = env.deps();
    deps.web = Some(std::sync::Arc::new(Unconfigured(env.web())));
    env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(script_result(&env, &run, 0)["value"], "web_disabled");
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

#[tokio::test]
async fn a_host_without_a_source_refuses_web_disabled() {
    let env = Env::new().await;
    env.web().search_answers("a", hits(&[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Search",
        "try { await clax.web.search('a'); return 'searched'; } catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search");
    let mut deps = env.deps();
    deps.web = None;
    env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(script_result(&env, &run, 0)["value"], "web_disabled");
    assert!(env.web().calls().is_empty());
}

#[tokio::test]
async fn a_failed_search_is_free_and_a_failed_fetch_is_not() {
    let env = Env::new().await;
    let a = "https://example.com/slow";
    sender_says(&env, &format!("Read {a}"));
    env.web().search_answers(
        "down",
        Err(WebError::new(
            CODE_WEB_UNAVAILABLE,
            "the provider is down",
            true,
        )),
    );
    env.web().page(
        a,
        Err(WebError::new(CODE_WEB_TIMEOUT, "too slow", true).went_out(true)),
    );
    let (run, v) = one_script(
        &env,
        "search,fetch",
        &format!(
            "const codes = [];
             try {{ await clax.web.search('down'); }} catch (e) {{ codes.push([e.code, e.message]); }}
             try {{ await clax.web.fetch({a:?}); }} catch (e) {{ codes.push([e.code, e.message]); }}
             return codes;"
        ),
    )
    .await;
    // Both are retryable, and their messages say so.
    assert_eq!(
        v,
        json!([
            [
                "web_unavailable",
                "the provider is down (retryable: try again)"
            ],
            ["web_timeout", "too slow (retryable: try again)"]
        ])
    );
    assert_eq!(env.web().calls().len(), 2);
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
    let calls = script_calls(&env, &run, 0);
    assert_eq!(calls[0]["cost_milli"], 0);
    assert!(calls[1]["cost_milli"].as_i64() > Some(0), "{calls:?}");
}

#[tokio::test]
async fn a_web_error_says_whether_it_is_retryable_and_replays_so() {
    let env = Env::new().await;
    let off = "https://off.example/x";
    env.web().search_answers(
        "down",
        Err(WebError::new(
            CODE_WEB_UNAVAILABLE,
            "the provider is down",
            true,
        )),
    );
    env.api.push(env.script_turn(
        "tu_1",
        "Web",
        &format!(
            "const out = [];
             try {{ await clax.web.search('down'); }} catch (e) {{ out.push([e.code, e.retryable]); }}
             try {{ await clax.web.fetch({off:?}); }} catch (e) {{ out.push([e.code, e.retryable]); }}
             return out;"
        ),
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search,fetch");
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&a, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    // The script run again answers from the journal, flag and all.
    assert_eq!(
        script_result(&env, &run, 0)["value"],
        json!([
            ["web_unavailable", true],
            ["url_not_in_prior_context", false]
        ])
    );
    assert_eq!(env.web().calls().len(), 1);
}

#[tokio::test]
async fn failed_calls_count_against_the_cap_uncharged_across_a_restart() {
    let env = Env::new().await;
    set(
        &env,
        &[
            (Setting::WebMaxSearchesPerRun, json!(2)),
            (Setting::WebMaxFetchesPerRun, json!(1)),
        ],
    );
    let a = "https://example.com/gone";
    sender_says(&env, &format!("Read {a}"));
    env.web().search_answers(
        "down",
        Err(WebError::new(
            CODE_WEB_UNAVAILABLE,
            "the provider is down",
            true,
        )),
    );
    // `a` has no scripted answer: the fake fails its fetch before it goes
    // out, so it is not charged, but it reached the source.
    let attempts = "const out = [];
         for (let i = 0; i < 2; i++) {
           try { await clax.web.search('down'); } catch (e) { out.push(e.code); }
         }
         return out;";
    env.api.push(env.script_turn("tu_1", "Search", attempts));
    env.api.push(env.script_turn(
        "tu_2",
        "Again",
        &format!(
            "const out = [];
             try {{ await clax.web.search('down'); }} catch (e) {{ out.push(e.code); }}
             for (let i = 0; i < 2; i++) {{
               try {{ await clax.web.fetch({a:?}); }} catch (e) {{ out.push(e.code); }}
             }}
             return out;"
        ),
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search,fetch");
    let mut crashing = env.deps();
    crash_once(
        &mut crashing,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&crashing, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(
        script_result(&env, &run, 0)["value"],
        json!(["web_unavailable", "web_unavailable"])
    );
    // The two searches replayed after the restart count once each, so the
    // third is refused; so is the second fetch, after one that failed.
    assert_eq!(
        script_result(&env, &run, 1)["value"],
        json!(["web_cap", "web_unavailable", "web_cap"])
    );
    assert_eq!(
        env.web().calls(),
        [
            ("search".to_string(), "down".to_string()),
            ("search".to_string(), "down".to_string()),
            ("fetch".to_string(), a.to_string()),
        ]
    );
    assert!(web_rows(&env).is_empty(), "{:?}", web_rows(&env));
}

#[tokio::test]
async fn a_redirect_to_a_private_host_is_refused() {
    let env = Env::new().await;
    let a = "http://example.com/a";
    sender_says(&env, &format!("Read {a}"));
    env.web().redirect(a, "http://10.0.0.1/");
    env.web().page("http://10.0.0.1/", page("secret", &[]));
    let (_, v) = one_script(&env, "fetch", &try_fetch(a)).await;
    assert_eq!(v, "url_not_allowed");
    assert_eq!(env.web().calls(), fetches(&[a]));
    // The request to the URL went out: the fetch is charged.
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

#[tokio::test]
async fn the_instances_own_hosts_are_never_fetched() {
    let env = Env::new().await;
    let (own, site, away) = (
        "https://clax.example/a",
        "https://abcdefghijkl.sites.example/",
        "https://example.com/r",
    );
    sender_says(&env, &format!("Read {own}, {site} and {away}"));
    env.web().page(own, page_at(own, "own", &[]));
    env.web().page(site, page_at(site, "site", &[]));
    env.web().redirect(away, "https://clax.example/admin");
    env.api.push(env.script_turn(
        "tu_1",
        "Web",
        &format!(
            "const out = [];
             for (const u of [{own:?}, {site:?}, {away:?}]) {{
               try {{ out.push((await clax.web.fetch(u)).text); }} catch (e) {{ out.push(e.code); }}
             }}
             return out;"
        ),
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("fetch");
    let web = Arc::new(WithOwnHosts::new(
        env.web(),
        OwnHosts::new(Some("https://clax.example"), Some("sites.example")),
    ));
    let deps = env.deps().with_web(web);
    let out = env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let r = script_result(&env, &run, 0);
    assert_eq!(
        r["value"],
        json!(["url_not_allowed", "url_not_allowed", "url_not_allowed"]),
        "{r}"
    );
    // The own hosts were refused before the source; the redirect to one
    // after its request went out, so only it is charged.
    assert_eq!(env.web().calls(), fetches(&[away]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

#[tokio::test]
async fn the_allowlist_holds_on_redirects() {
    let env = Env::new().await;
    set(
        &env,
        &[
            (Setting::WebFetchUrlRule, json!("allowlist")),
            (Setting::WebFetchAllowlist, json!(["*.example.com"])),
        ],
    );
    let (a, apex, off) = (
        "https://docs.example.com/a",
        "https://example.com/b",
        "https://other.example/c",
    );
    sender_says(&env, &format!("Read {a}, {apex} and {off}"));
    env.web().redirect(a, off);
    env.web().page(apex, page_at(apex, "apex", &[]));
    env.web().page(off, page_at(off, "off", &[]));
    let (_, v) = one_script(
        &env,
        "fetch",
        &format!(
            "const out = [];
             for (const u of [{a:?}, {apex:?}, {off:?}]) {{
               try {{ out.push((await clax.web.fetch(u)).text); }} catch (e) {{ out.push(e.code); }}
             }}
             return out;"
        ),
    )
    .await;
    assert_eq!(
        v,
        json!(["url_not_allowed", "url_not_allowed", "url_not_allowed"])
    );
    // Only the first went out, and was refused on its redirect.
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

#[tokio::test]
async fn left_counts_down() {
    let env = Env::new().await;
    set(
        &env,
        &[
            (Setting::WebMaxSearchesPerRun, json!(3)),
            (Setting::WebMaxFetchesPerRun, json!(2)),
        ],
    );
    let a = "https://example.com/a";
    env.web().search_answers("a", hits(&[a]));
    env.web().search_answers("b", hits(&[]));
    env.web().page(a, page_at(a, "# A", &[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Web",
        "const s1 = await clax.web.search('a');
         const p = await clax.web.fetch(s1.results[0].url);
         const s2 = await clax.web.search('b');
         return [s1.left, p.left, s2.left];",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search,fetch");
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    let before = env.store.week_usage_now(OWNER).unwrap().remaining_milli();
    let ps = env
        .store
        .one_request_milli(SEARCH_LINE, KeySource::Instance)
        .unwrap();
    let pf = env
        .store
        .one_request_milli(LINE_WEB_FETCH, KeySource::Instance)
        .unwrap();
    assert!(ps > 0 && pf > 0, "{ps} {pf}");
    env.drive(&deps, &run, &d).await;
    let v = &script_result(&env, &run, 0)["value"];
    let left = |i: usize| {
        let l = &v[i];
        let credits = (l["credits"].as_f64().unwrap() * 1000.0).round() as i64;
        (l["searches"].clone(), l["fetches"].clone(), credits)
    };
    assert_eq!(left(0), (json!(2), json!(2), before - ps));
    assert_eq!(left(1), (json!(2), json!(1), before - ps - pf));
    assert_eq!(left(2), (json!(1), json!(1), before - 2 * ps - pf));
}

#[tokio::test]
async fn a_missing_web_cost_line_refuses_before_the_source() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    sender_says(&env, &format!("Read {a}"));
    env.web().search_answers("q", hits(&[]));
    env.web().page(a, page_at(a, "# A", &[]));
    for line in [LINE_WEB_FETCH, SEARCH_LINE] {
        env.store.delete_cost_line(&AuditCtx::DAEMON, line).unwrap();
    }
    let (_, v) = one_script(
        &env,
        "search,fetch",
        &format!(
            "const out = [];
             try {{ await clax.web.search('q'); }} catch (e) {{ out.push([e.code, e.message]); }}
             try {{ await clax.web.fetch({a:?}); }} catch (e) {{ out.push([e.code, e.message]); }}
             return out;"
        ),
    )
    .await;
    for (i, line) in [SEARCH_LINE, LINE_WEB_FETCH].iter().enumerate() {
        assert_eq!(v[i][0], "web_unavailable", "{v}");
        let m = v[i][1].as_str().unwrap();
        assert!(
            m.contains("pricing is not configured") && m.contains(line),
            "{m}"
        );
        assert!(!m.contains("retryable"), "{m}");
    }
    assert!(env.web().calls().is_empty());
    assert!(web_rows(&env).is_empty());
}

#[tokio::test]
async fn a_stored_page_must_be_read_in_its_format() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    sender_says(&env, &format!("Read {a}"));
    env.web().page(a, page_at(a, "# A", &[]));
    let (_, v) = one_script(
        &env,
        "fetch",
        &format!(
            "const p = await clax.web.fetch({a:?});
             try {{ await clax.web.fetch({a:?}, {{format: 'raw'}}); return 'read'; }}
             catch (e) {{ return [p.text, e.code, e.message]; }}"
        ),
    )
    .await;
    assert_eq!(v[0], "# A");
    assert_eq!(v[1], "invalid_args");
    assert!(
        v[2].as_str()
            .unwrap()
            .contains("already read https://example.com/a as auto"),
        "{v}"
    );
    assert_eq!(env.web().calls(), fetches(&[a]));
}

#[tokio::test]
async fn a_page_over_the_store_limit_is_cut_on_a_character() {
    let env = Env::new().await;
    let a = "https://example.com/big";
    sender_says(&env, &format!("Read {a}"));
    // 600,000 two-byte characters: the store keeps the first 1 MiB.
    env.web().page(a, page_at(a, &"é".repeat(600_000), &[]));
    let (run, v) = one_script(
        &env,
        "fetch",
        &format!(
            "const p = await clax.web.fetch({a:?}, {{offset: 524000}});
             const q = await clax.web.fetch({a:?}, {{offset: 530000}});
             return [p.text.length, p.nextOffset, p.totalChars, q.offset, q.text, q.nextOffset];"
        ),
    )
    .await;
    assert_eq!(v, json!([288, null, 600_000, 524_288, "", null]));
    let kept = env.store.web_page(&run, a).unwrap().unwrap();
    assert_eq!(kept.text.len(), 1 << 20);
    assert_eq!(kept.text.chars().count(), 524_288);
    assert_eq!(kept.total_chars, 600_000);
    assert_eq!(env.web().calls(), fetches(&[a]));
}

#[tokio::test]
async fn a_stored_page_is_served_at_the_fetch_cap() {
    let env = Env::new().await;
    set(&env, &[(Setting::WebMaxFetchesPerRun, json!(1))]);
    let (a, b) = ("https://example.com/a", "https://example.com/b");
    sender_says(&env, &format!("Read {a} and {b}"));
    env.web().page(a, page_at(a, "# A", &[]));
    env.web().page(b, page_at(b, "# B", &[]));
    let (_, v) = one_script(
        &env,
        "fetch",
        &format!(
            "const out = [];
             for (const u of [{a:?}, {a:?}, {b:?}]) {{
               try {{ const p = await clax.web.fetch(u); out.push([p.text, p.left.fetches]); }}
               catch (e) {{ out.push(e.code); }}
             }}
             return out;"
        ),
    )
    .await;
    assert_eq!(v, json!([["# A", 0], ["# A", 0], "web_cap"]));
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

/// A fetch run whose script step is cut once after it fetched, and
/// resumed by another driver after `between`; the run.
async fn fetch_cut_then_resumed(env: &Env, a: &str, between: impl FnOnce(&Env)) -> String {
    sender_says(env, &format!("Read {a}"));
    env.web().page(a, page_at(a, "# A", &[]));
    env.api.push(env.script_turn(
        "tu_1",
        "Read",
        &format!("return (await clax.web.fetch({a:?})).text;"),
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("fetch");
    let mut first = env.deps();
    crash_once(
        &mut first,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = env.drive(&first, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    between(env);
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    run
}

#[tokio::test]
async fn a_resumed_fetch_answers_from_the_journal_with_its_source_and_cost() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    let run = fetch_cut_then_resumed(&env, a, |_| {}).await;
    assert_eq!(script_result(&env, &run, 0)["value"], "# A");
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
    // The call the resumed step reports was answered from the journal,
    // with what the first attempt recorded.
    let call = &script_calls(&env, &run, 0)[0];
    assert_eq!(call["web"], json!({"url": a, "source": "comment"}));
    let charged = env.store.usage_of_run(&run).unwrap();
    let charged = charged.iter().find(|r| r.line == LINE_WEB_FETCH).unwrap();
    assert_eq!(call["cost_milli"], charged.credits_milli);
    // The journal entry records the source that allowed the fetch.
    let entry = &env.store.bot_calls(&run, 2).unwrap()[0];
    assert_eq!(entry.result()["source"]["kind"], "comment");
    assert_eq!(
        entry.result()["source"]["comment_id"],
        format!("c-{}", env.t1)
    );
    assert_eq!(entry.result()["format"], "auto");
}

#[tokio::test]
async fn a_resumed_fetch_answers_from_the_journal_after_fetch_is_turned_off() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    let run = fetch_cut_then_resumed(&env, a, |env| {
        set(env, &[(Setting::WebFetchEnabled, json!(false))]);
    })
    .await;
    assert_eq!(script_result(&env, &run, 0)["value"], "# A");
    assert_eq!(env.web().calls(), fetches(&[a]));
    assert_eq!(web_rows(&env), [LINE_WEB_FETCH]);
}

#[tokio::test]
async fn refused_and_stored_fetches_are_in_the_audit_trail() {
    let env = Env::new().await;
    let (a, evil) = ("https://example.com/a", "https://evil.example/?d=x");
    sender_says(&env, &format!("Read {a}"));
    env.web().page(a, page_at(a, "# A", &[]));
    one_script(
        &env,
        "fetch",
        &format!(
            "try {{ await clax.web.fetch({evil:?}); }} catch (e) {{}}
             await clax.web.fetch({a:?});
             await clax.web.fetch({a:?});
             return 1;"
        ),
    )
    .await;
    let web: Vec<Value> = env
        .bot_events()
        .into_iter()
        .filter(|(k, _)| k == "bot.web")
        .map(|(_, b)| b)
        .collect();
    assert_eq!(web.len(), 3, "{web:?}");
    assert_eq!(
        (
            &web[0]["url"],
            &web[0]["ok"],
            &web[0]["code"],
            &web[0]["credits_milli"]
        ),
        (
            &json!(evil),
            &json!(false),
            &json!("url_not_in_prior_context"),
            &json!(0)
        )
    );
    assert_eq!(
        (&web[1]["ok"], &web[1]["source"]),
        (&json!(true), &json!("comment"))
    );
    assert!(web[1]["credits_milli"].as_i64() > Some(0));
    assert_eq!(
        (&web[2]["ok"], &web[2]["stored"], &web[2]["credits_milli"]),
        (&json!(true), &json!(true), &json!(0))
    );
}

// ---------------------------------------------------------------------------
// Sources in the reply
// ---------------------------------------------------------------------------

use clax_bot::web::page::sources_markdown;
use clax_store::bot_web::WebPageRow;

fn stored(url: &str, final_url: &str, text: &str) -> WebPageRow {
    WebPageRow {
        url: url.into(),
        step_n: 2,
        seq: 0,
        final_url: final_url.into(),
        status: 200,
        content_type: "text/markdown".into(),
        text: text.into(),
        total_chars: text.chars().count() as i64,
        links: Vec::new(),
    }
}

#[test]
fn sources_markdown_lists_titles_and_final_urls_in_order() {
    assert_eq!(sources_markdown(&[]), None);
    let pages = [
        stored(
            "https://a.example/",
            "https://a.example/x",
            "intro\n# The A page\nbody",
        ),
        stored("https://b.example/", "", "no heading"),
        stored(
            "https://c.example/",
            "ftp://c.example/",
            "## not a title\n#also not",
        ),
    ];
    assert_eq!(
        sources_markdown(&pages).unwrap(),
        "\n\nSources:\n- [The A page](https://a.example/x)\n\
         - [https\\:\\/\\/b\\.example\\/](https://b.example/)\n\
         - [https\\:\\/\\/c\\.example\\/](https://c.example/)"
    );
}

#[test]
fn a_hostile_title_cannot_change_the_link() {
    let evil = "# x](https://evil.example) [y \\ <b>*_`!";
    let md = sources_markdown(&[stored("https://a.example/", "https://a.example/", evil)]).unwrap();
    let link = md.lines().last().unwrap();
    assert!(
        link.starts_with("- [") && link.ends_with("](https://a.example/)"),
        "{link}"
    );
    let title = &link[3..link.len() - "](https://a.example/)".len()];
    // Every bracket, backslash and markup character of the title is escaped.
    let mut chars = title.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
        } else {
            assert!(!"[]()<>*_`!&|".contains(c), "bare {c:?} in {title}");
        }
    }
    // A title with a line break stays on one line.
    let md = sources_markdown(&[stored(
        "https://a.example/",
        "https://a.example/",
        "# a\r\n",
    )])
    .unwrap();
    assert_eq!(md.lines().count(), 4);
}

/// `url` fetched by a script that ends with `tail`, then finish.
async fn fetch_then(env: &Env, url: &str, tail: &str, summary: &str) -> String {
    env.web().page(url, page_at(url, "# Doc title\nbody", &[]));
    sender_says(env, &format!("Read {url}"));
    env.api.push(env.script_turn(
        "tu_1",
        "Web",
        &format!("await clax.web.fetch({url:?}); {tail}"),
    ));
    env.api.push(finish_turn(summary));
    let run = env.run_with_web("fetch");
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    run
}

#[tokio::test]
async fn finish_lists_the_pages_fetched() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    let run = fetch_then(&env, a, "", "All done.").await;
    let replies = env.replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].1,
        "All done.\n\nSources:\n- [Doc title](https://example.com/a)"
    );
    let fin = env
        .store
        .run_with_steps(&run)
        .unwrap()
        .unwrap()
        .1
        .into_iter()
        .find(|s| s.kind == StepKind::Finish)
        .unwrap();
    assert_eq!(
        fin.payload.unwrap()["sources"],
        json!([{"title": "Doc title", "url": a}])
    );
}

#[tokio::test]
async fn a_run_that_replied_gets_sources_as_a_notice() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    fetch_then(&env, a, "await clax.threads.reply('T1', 'Mine.');", "done").await;
    let bodies: Vec<String> = env.replies().into_iter().map(|r| r.1).collect();
    assert_eq!(
        bodies,
        ["Mine.", "Sources:\n- [Doc title](https://example.com/a)"]
    );
}

#[tokio::test]
async fn a_run_that_only_searched_lists_none() {
    let env = Env::new().await;
    env.web()
        .search_answers("rust", hits(&["https://example.com/a"]));
    env.api.push(env.script_turn(
        "tu_1",
        "Search",
        "return (await clax.web.search('rust')).results.length;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search");
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    let replies = env.replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].1, "done");
}

#[tokio::test]
async fn a_resumed_finish_posts_the_sources_once() {
    let env = Env::new().await;
    let a = "https://example.com/a";
    env.web().page(a, page_at(a, "# Doc title", &[]));
    sender_says(&env, &format!("Read {a}"));
    env.api.push(env.script_turn(
        "tu_1",
        "Web",
        &format!("await clax.web.fetch({a:?}); await clax.threads.reply('T1', 'Mine.');"),
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("fetch");
    let mut d = env.deps();
    crash_once(
        &mut d,
        CrashPoint::BeforeCommit {
            step: 4,
            kind: StepKind::Finish,
        },
    );
    let out = env.drive(&d, &run, &DriverId::mint()).await;
    assert_eq!(
        out.last().unwrap().next,
        Next::Waiting(clax_bot::Wait::Crashed)
    );
    assert_eq!(env.replies().len(), 2, "posted before the crash");
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let bodies: Vec<String> = env.replies().into_iter().map(|r| r.1).collect();
    assert_eq!(
        bodies,
        ["Mine.", "Sources:\n- [Doc title](https://example.com/a)"]
    );
}

#[test]
fn a_url_with_markup_is_plain_text_and_an_encoded_target() {
    let md = sources_markdown(&[stored(
        "https://a.example/",
        "https://a.example/a`b*c_d)e f",
        "",
    )])
    .unwrap();
    assert_eq!(
        md.lines().last().unwrap(),
        "- [https\\:\\/\\/a\\.example\\/a\\`b\\*c\\_d\\)e f](https://a.example/a`b*c_d%29e%20f)"
    );
    assert_eq!(
        clax_bot::web::page::link_target("https://x/a)b c<"),
        "https://x/a%29b%20c%3C"
    );
}

#[test]
fn a_non_http_final_url_falls_back_or_drops_the_page() {
    let pages = [
        stored("https://a.example/", "javascript:alert(1)", "# A"),
        stored("ftp://b.example/", "data:text/html,x", "# B"),
    ];
    assert_eq!(
        sources_markdown(&pages).unwrap(),
        "\n\nSources:\n- [A](https://a.example/)"
    );
}

#[test]
fn a_page_reached_twice_is_listed_once() {
    let pages = [
        stored("https://a.example/1", "https://z.example/p", "# First"),
        stored("https://a.example/2", "https://z.example/p/", "# Second"),
        stored("https://a.example/3", "https://z.example/q", "# Third"),
    ];
    assert_eq!(
        sources_markdown(&pages).unwrap(),
        "\n\nSources:\n- [First](https://z.example/p)\n- [Third](https://z.example/q)"
    );
}

#[test]
fn a_heading_in_a_code_fence_is_not_a_title() {
    let text = "```sh\n# a comment\n```\n    # indented code\n   # Real title ##  \n";
    let md = sources_markdown(&[stored("https://a.example/", "", text)]).unwrap();
    assert!(md.ends_with("- [Real title](https://a.example/)"), "{md}");
}

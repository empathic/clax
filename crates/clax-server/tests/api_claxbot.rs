//! ClaxBot's routes (spec docs/superpowers/specs/2026-10-21-claxbot-design.md
//! §10; `clax_server::routes::{bot, admin, audit_trail}`): who reaches
//! each, the shapes they answer, keys staying write-only, the views and
//! changes each records, refusals recorded as `bot.refused`, and runs
//! started by every send path.

use clax_core::audit::AuditCtx;
use clax_store::bot_settings::{Overrides, Setting};
use clax_testkit::TestServer;
use clax_testkit::remotes::json_of;
use reqwest::Method;
use serde_json::{Value, json};

const KEY: &str = "sk-ant-api03-TOPSECRETKEYVALUE-abcd";

/// Who a request is.
#[derive(Clone, Debug)]
enum Who {
    /// The daemon token, by hand.
    Owner,
    /// The owner's browser.
    OwnerBrowser,
    /// A connected person, by their cookie.
    Person(String),
    /// A LAN viewer who is no connected person.
    Viewer(String),
    /// The owner's agent (the token with the agent marker).
    Agent,
    /// No credential.
    Anonymous,
}

async fn call(ts: &TestServer, who: &Who, method: Method, path: &str, body: Value) -> (u16, Value) {
    let mut r = ts.client.request(method, format!("{}{path}", ts.base));
    r = match who {
        Who::Owner => ts.authed(r),
        Who::OwnerBrowser => r.header("cookie", ts.owner_cookie()),
        Who::Person(c) | Who::Viewer(c) => r.header("cookie", format!("clax_viewer={c}")),
        Who::Agent => ts.authed(r).header("x-clax-channel", "agent"),
        Who::Anonymous => r,
    };
    if !body.is_null() {
        r = r.json(&body);
    }
    json_of(r.send().await.unwrap()).await
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// The events of `kind` recorded so far, as (actor, body).
fn events(ts: &TestServer, kind: &str) -> Vec<(Value, Value)> {
    ts.store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| {
            (
                serde_json::from_str(&e.actor).unwrap(),
                serde_json::from_str(&e.body).unwrap(),
            )
        })
        .collect()
}

/// A daemon with a connected person made admin (`ana`), a member (`sam`)
/// and a LAN viewer (`lan`), as (cookie, public ID).
struct People {
    ts: TestServer,
    ana: (String, String),
    sam: (String, String),
    lan: String,
}

async fn people() -> People {
    let ts = TestServer::spawn().await;
    let ana = ts.person("Ana").await;
    let sam = ts.person("Sam").await;
    let lan = ts.viewer(Some("Lou")).await;
    ts.store
        .set_admin(&AuditCtx::DAEMON, &ana.public_id, true)
        .unwrap();
    People {
        ana: (ana.cookie, ana.public_id),
        sam: (sam.cookie, sam.public_id),
        lan: lan.cookie,
        ts,
    }
}

/// An artifact with one open thread: (artifact ID, thread ID).
async fn artifact_with_thread(ts: &TestServer) -> (String, String) {
    let aid = ts.publish("Notes", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread(&aid, 1, "Make it blue").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    (aid, tid)
}

#[tokio::test]
async fn admin_routes_admit_the_owner_and_admins_alone() {
    let p = people().await;
    let ts = &p.ts;
    let routes: &[(Method, &str, Value)] = &[
        (Method::GET, "/api/admin/people", Value::Null),
        (Method::GET, "/api/admin/provider", Value::Null),
        (Method::PUT, "/api/admin/provider", json!({})),
        (Method::GET, "/api/admin/pricing", Value::Null),
        (Method::GET, "/api/admin/usage", Value::Null),
        (Method::DELETE, "/api/admin/provider/key", Value::Null),
        (
            Method::PUT,
            "/api/admin/people/%2Aowner/flags",
            json!({"claxbot": true}),
        ),
        (
            Method::PUT,
            "/api/admin/people/%2Aowner/credits",
            json!({"lift_milli": 0, "reason": "none"}),
        ),
    ];
    for (m, path, body) in routes {
        for who in [Who::Owner, Who::OwnerBrowser, Who::Person(p.ana.0.clone())] {
            let (s, v) = call(ts, &who, m.clone(), path, body.clone()).await;
            assert_eq!(s, 200, "{who:?} {m} {path}: {v}");
        }
        for (who, want) in [
            (Who::Person(p.sam.0.clone()), (403, "forbidden")),
            (Who::Viewer(p.lan.clone()), (403, "forbidden")),
            (Who::Agent, (403, "not_for_agents")),
            (Who::Anonymous, (401, "unauthorized")),
        ] {
            let (s, v) = call(ts, &who, m.clone(), path, body.clone()).await;
            assert_eq!((s, code(&v)), want, "{who:?} {m} {path}: {v}");
        }
    }
    // Roles are the owner's alone; the owner is never demoted.
    let path = format!("/api/admin/people/{}/role", p.sam.1);
    let ana = Who::Person(p.ana.0.clone());
    let (s, v) = call(ts, &ana, Method::PUT, &path, json!({"role": "admin"})).await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        &path,
        json!({"role": "admin"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["role"], "admin");
    let owner_role = "/api/admin/people/%2Aowner/role";
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        owner_role,
        json!({"role": "member"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "owner_role"), "{v}");
    assert_eq!(
        events(ts, "admin.role").last().unwrap().1["person"],
        p.sam.1
    );
    // Sam, now an admin, reaches the admin pages and the settings.
    let sam = Who::Person(p.sam.0.clone());
    let (s, _) = call(ts, &sam, Method::GET, "/api/admin/people", Value::Null).await;
    assert_eq!(s, 200);
    let (s, _) = call(ts, &sam, Method::GET, "/api/settings", Value::Null).await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn people_list_flags_lifts_and_refusals() {
    let p = people().await;
    let ts = &p.ts;
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/people",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let list = v.as_array().unwrap();
    assert_eq!(list[0]["person"], "*owner");
    assert_eq!(list[0]["role"], "owner");
    assert_eq!(
        list[0]["flags"],
        json!({"claxbot": true, "claxbot_unenforced": false, "feedback": false})
    );
    let sam = list.iter().find(|x| x["person"] == p.sam.1).unwrap();
    assert_eq!(sam["name"], "Sam");
    assert_eq!(sam["key_source"], "instance");
    assert_eq!(sam["week"]["weekly_milli"], 2_000_000);
    assert_eq!(
        (&sam["runs_week"], &sam["refused_week"]),
        (&json!(0), &json!(0))
    );
    let ana = list.iter().find(|x| x["person"] == p.ana.1).unwrap();
    assert_eq!(ana["role"], "admin");

    // Sam has no flag: a send is refused before any model call, and counted.
    let (aid, tid) = artifact_with_thread(ts).await;
    let sam_who = Who::Person(p.sam.0.clone());
    let body = json!({"thread_ids": [tid], "artifact_id": aid});
    let (s, v) = call(ts, &sam_who, Method::POST, "/api/bot/runs", body.clone()).await;
    assert_eq!((s, code(&v)), (403, "no_flag"), "{v}");
    let refused = events(ts, "bot.refused");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].1["person"], p.sam.1);
    assert_eq!(refused[0].1["reason"], "no_flag");
    assert_eq!(refused[0].1["artifact"], aid);
    assert_eq!(refused[0].0["public_id"], p.sam.1);

    // An admin turns the flag on and lifts the week.
    let flags = format!("/api/admin/people/{}/flags", p.sam.1);
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        &flags,
        json!({"claxbot": true}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["flags"]["claxbot"], true);
    assert_eq!(v["refused_week"], 1);
    let lift = format!("/api/admin/people/{}/credits", p.sam.1);
    let (s, v) = call(
        ts,
        &Who::Person(p.ana.0.clone()),
        Method::PUT,
        &lift,
        json!({"lift_milli": 500_000, "reason": "pilot"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["week"]["weekly_milli"], 2_500_000);
    assert_eq!(v["week"]["lift_milli"], 500_000);
    assert_eq!(events(ts, "admin.flag").len(), 1);
    assert_eq!(events(ts, "admin.credits")[0].1["reason"], "pilot");
    // No key yet: not configured.
    let (s, v) = call(ts, &sam_who, Method::POST, "/api/bot/runs", body.clone()).await;
    assert_eq!((s, code(&v)), (403, "not_configured"), "{v}");
    let (_, me) = call(ts, &sam_who, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(me["available"], false);
    assert_eq!(me["unavailable_reason"], "not_configured");
    // An unknown flag is refused.
    let (s, _) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        &flags,
        json!({"nope": true}),
    )
    .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn keys_are_write_only_and_their_changes_recorded() {
    let p = people().await;
    let ts = &p.ts;
    let sam = Who::Person(p.sam.0.clone());
    // The person's own key.
    let (s, v) = call(
        ts,
        &sam,
        Method::PUT,
        "/api/me/bot/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["key"]["own"]["set"], true);
    assert_eq!(v["key"]["own"]["last4"], "abcd");
    assert_eq!(
        v["key"]["own"]["set_by"],
        json!({"id": p.sam.1, "name": "Sam"})
    );
    assert_eq!(v["key"]["use_own"], true);
    let (s, v) = call(
        ts,
        &sam,
        Method::PUT,
        "/api/me/bot",
        json!({"use_own": false}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["key"]["use_own"], false);
    // The instance key.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["key"]["set"], true);
    assert_eq!(v["sources"]["key"], "db");
    let (s, _) = call(
        ts,
        &Who::Person(p.sam.0.clone()),
        Method::PUT,
        "/api/admin/provider/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 403);
    // Agents never reach a key route.
    for path in ["/api/me/bot/key", "/api/admin/provider/key"] {
        let (s, v) = call(ts, &Who::Agent, Method::PUT, path, json!({"key": KEY})).await;
        assert_eq!((s, code(&v)), (403, "not_for_agents"), "{path}: {v}");
    }
    // No answer, and no event, holds the key.
    let mut seen = Vec::new();
    for (who, path) in [
        (&sam, "/api/me/bot"),
        (&Who::OwnerBrowser, "/api/admin/provider"),
        (&Who::OwnerBrowser, "/api/admin/people"),
        (&Who::OwnerBrowser, "/api/audit?limit=200"),
    ] {
        let (s, v) = call(ts, who, Method::GET, path, Value::Null).await;
        assert_eq!(s, 200, "{path}: {v}");
        seen.push(v.to_string());
    }
    let (_, page) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/audit?limit=200",
        Value::Null,
    )
    .await;
    for e in page["events"].as_array().unwrap() {
        let (_, d) = call(
            ts,
            &Who::OwnerBrowser,
            Method::GET,
            &format!("/api/audit/{}", e["seq"]),
            Value::Null,
        )
        .await;
        seen.push(d.to_string());
    }
    for e in ts.store.events_after(0, 10_000).unwrap() {
        seen.push(e.body);
    }
    for text in seen {
        assert!(!text.contains("TOPSECRET"), "{text}");
    }
    let keys = events(ts, "admin.key");
    let ops: Vec<(&str, &str)> = keys
        .iter()
        .map(|(_, b)| (b["scope"].as_str().unwrap(), b["op"].as_str().unwrap()))
        .collect();
    assert_eq!(
        ops,
        [("own", "set"), ("own", "use_own"), ("instance", "set")]
    );
    let (s, v) = call(ts, &sam, Method::DELETE, "/api/me/bot/key", Value::Null).await;
    assert_eq!(s, 200);
    assert_eq!(v["key"]["own"], Value::Null);
}

#[tokio::test]
async fn me_bot_says_role_export_estimate_and_week() {
    let p = people().await;
    let ts = &p.ts;
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/me/bot",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["role"], "owner");
    assert_eq!(v["enabled"], true);
    assert_eq!(v["can_export"], true);
    assert_eq!(v["estimate_milli"], json!({"low": 5000, "high": 25000}));
    assert_eq!(v["week"]["weekly_milli"], 2_000_000);
    assert_eq!(v["key"], json!({"own": null, "use_own": false}));
    let (_, v) = call(
        ts,
        &Who::Person(p.ana.0.clone()),
        Method::GET,
        "/api/me/bot",
        Value::Null,
    )
    .await;
    assert_eq!(
        (&v["role"], &v["can_export"], &v["enabled"]),
        (&json!("admin"), &json!(true), &json!(false))
    );
    assert_eq!(v["unavailable_reason"], "no_flag");
    let (_, v) = call(
        ts,
        &Who::Person(p.sam.0.clone()),
        Method::GET,
        "/api/me/bot",
        Value::Null,
    )
    .await;
    assert_eq!(
        (&v["role"], &v["can_export"]),
        (&json!("member"), &json!(false))
    );
    for (who, want) in [
        (Who::Viewer(p.lan.clone()), (403, "forbidden")),
        (Who::Agent, (403, "not_for_agents")),
        (Who::Anonymous, (401, "unauthorized")),
    ] {
        let (s, v) = call(ts, &who, Method::GET, "/api/me/bot", Value::Null).await;
        assert_eq!((s, code(&v)), want, "{who:?}: {v}");
    }
    // An admin exports the history; a member does not.
    let res = ts
        .client
        .get(format!("{}/api/toolpath/export", ts.base))
        .header("cookie", format!("clax_viewer={}", p.ana.0))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = ts
        .client
        .get(format!("{}/api/toolpath/export", ts.base))
        .header("cookie", format!("clax_viewer={}", p.sam.0))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn every_view_is_recorded_once_a_window() {
    let p = people().await;
    let ts = &p.ts;
    for path in [
        "/api/admin/people",
        "/api/admin/provider",
        "/api/admin/pricing",
        "/api/admin/usage",
        "/api/me/usage",
        "/api/audit?group=views",
        "/api/settings",
    ] {
        for _ in 0..3 {
            let (s, v) = call(ts, &Who::OwnerBrowser, Method::GET, path, Value::Null).await;
            assert_eq!(s, 200, "{path}: {v}");
        }
    }
    let pages: Vec<String> = events(ts, "view.admin")
        .iter()
        .map(|(_, b)| b["page"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        pages,
        ["people", "provider", "pricing", "usage", "settings"]
    );
    assert_eq!(events(ts, "view.usage").len(), 1);
    assert_eq!(events(ts, "view.usage")[0].1["subject_person"], "*owner");
    let audit = events(ts, "view.audit");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].1["filters"], "group=views");
    assert_eq!(audit[0].0["type"], "owner");
    // Another viewer's views are their own.
    let (s, _) = call(
        ts,
        &Who::Person(p.ana.0.clone()),
        Method::GET,
        "/api/admin/people",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(events(ts, "view.admin").len(), 6);
    // A settings write is admin.settings, with its keys alone.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/settings",
        json!({"set": {"bot.max_steps": 12}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let w = events(ts, "admin.settings");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].1["fields"], json!(["bot.max_steps"]));
}

#[tokio::test]
async fn the_audit_view_resolves_actors_and_scopes_readers() {
    let p = people().await;
    let ts = &p.ts;
    let sam = Who::Person(p.sam.0.clone());
    // Sam is refused (no flag): recorded as Sam.
    let (aid, tid) = artifact_with_thread(ts).await;
    let (s, _) = call(
        ts,
        &sam,
        Method::POST,
        "/api/bot/runs",
        json!({"thread_ids": [tid], "artifact_id": aid}),
    )
    .await;
    assert_eq!(s, 403);
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/audit?group=bot",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let e = &v["events"][0];
    assert_eq!(e["kind"], "bot.refused");
    assert_eq!(e["actor"], json!({"id": p.sam.1, "name": "Sam"}));
    assert_eq!(e["on_behalf_of"], Value::Null);
    assert_eq!(e["artifact_id"], aid);
    let (s, d) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        &format!("/api/audit/{}", e["seq"]),
        Value::Null,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(d["body"]["reason"], "no_flag");
    // Sam sees it (Sam acted); Ana (an admin) sees it; a set_admin by Clax
    // itself reads as "Clax".
    let (_, v) = call(ts, &sam, Method::GET, "/api/audit", Value::Null).await;
    let kinds: Vec<&str> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"bot.refused"), "{kinds:?}");
    assert!(!kinds.contains(&"admin.role"), "{kinds:?}");
    let (_, v) = call(
        ts,
        &Who::Person(p.ana.0.clone()),
        Method::GET,
        "/api/audit?group=admin",
        Value::Null,
    )
    .await;
    let role = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "admin.role")
        .unwrap();
    assert_eq!(role["actor"], json!({"id": "*system", "name": "Clax"}));
    // Filters are checked.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/audit?group=nope",
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_parameter"));
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/audit?actor=claxbot",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["events"], json!([]));
    for (who, want) in [
        (Who::Viewer(p.lan.clone()), 403),
        (Who::Agent, 403),
        (Who::Anonymous, 401),
    ] {
        let (s, _) = call(ts, &who, Method::GET, "/api/audit", Value::Null).await;
        assert_eq!(s, want, "{who:?}");
    }
}

#[tokio::test]
async fn provider_layers_configuration_over_stored_settings() {
    let p = people().await;
    let ts = &p.ts;
    let mut ov = Overrides::default();
    ov.set(Setting::MaxSteps, &json!(9), "file").unwrap();
    ov.set(
        Setting::ModelQuick,
        &json!("claude-haiku-x"),
        "env:CLAX_BOT__MODELS__QUICK",
    )
    .unwrap();
    ts.store.set_bot_overrides(ov);
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"models": {"default": "claude-opus-5-5"}, "caps": {"max_run_minutes": 10}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["provider"], "anthropic");
    assert_eq!(v["models"]["default"], "claude-opus-5-5");
    assert_eq!(v["models"]["quick"], "claude-haiku-x");
    assert_eq!(v["caps"]["max_steps"], 9);
    assert_eq!(v["caps"]["max_run_minutes"], 10);
    assert_eq!(v["sources"]["models.default"], "db");
    assert_eq!(v["sources"]["models.quick"], "env:CLAX_BOT__MODELS__QUICK");
    assert_eq!(v["sources"]["caps.max_steps"], "file");
    assert_eq!(v["sources"]["base_url"], "default");
    assert_eq!(v["sources"]["key"], "default");
    assert_eq!(v["key"]["set"], false);
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"caps": {"max_steps": 3}}),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "setting_overridden"), "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("file"),
        "{v}"
    );
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"caps": {"nope": 3}}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "unknown_setting"), "{v}");
    assert_eq!(events(ts, "admin.provider").len(), 1);
    // Without a key, testing says so and listing models is a conflict.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/provider/test",
        json!({}),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false);
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/provider/models",
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (409, "no_key"), "{v}");
}

#[tokio::test]
async fn pricing_round_trips_and_usage_reports_the_week() {
    let ts = TestServer::spawn().await;
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/pricing",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["credit_price_micro_usd"], 10_000);
    assert_eq!(v["free_weekly_milli"], 2_000_000);
    let lines = v["lines"].as_array().unwrap().clone();
    assert!(
        lines.iter().any(|l| l["id"] == "claude-sonnet-5-5.input"),
        "{v}"
    );
    // A refused line (the last, its margin out of range) writes nothing:
    // not the price, not the first line's new label, not a deletion.
    let mut bad = json!({"credit_price_micro_usd": 30_000, "free_weekly_milli": 1_000_000,
                         "lines": lines.clone()});
    let ls = bad["lines"].as_array_mut().unwrap();
    ls[0]["label"] = json!("Renamed first");
    ls.retain(|l| l["id"] != "toolpath.storage");
    let last = ls.len() - 1;
    ls[last]["margin_bp"] = json!(-5);
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        bad,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_cost_line"), "{v}");
    let (_, after) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/pricing",
        Value::Null,
    )
    .await;
    assert_eq!(after["credit_price_micro_usd"], 10_000);
    assert_eq!(after["lines"], json!(lines));
    assert!(events(&ts, "admin.cost_line").is_empty());
    let mut put =
        json!({"credit_price_micro_usd": 20_000, "free_weekly_milli": 1_000_000, "lines": lines});
    put["lines"]
        .as_array_mut()
        .unwrap()
        .retain(|l| l["id"] != "toolpath.storage");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        put,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["credit_price_micro_usd"], 20_000);
    assert!(
        !v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["id"] == "toolpath.storage")
    );
    let ops: Vec<String> = events(&ts, "admin.cost_line")
        .iter()
        .map(|(_, b)| {
            format!(
                "{} {}",
                b["line"].as_str().unwrap(),
                b["op"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(ops, ["*instance update", "toolpath.storage delete"]);
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/usage?week=2026-10-21",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["week"],
        json!({"start": "2026-10-19T00:00:00Z", "end": "2026-10-26T00:00:00Z"})
    );
    assert_eq!(
        (&v["runs"], &v["refused"], &v["paid_milli"]),
        (&json!(0), &json!(0), &json!(0))
    );
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/usage?week=yesterday",
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_week"));
}

/// Sets the instance key, so the owner's runs are not refused.
async fn configured(ts: &TestServer) {
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn every_send_path_starts_a_run_acting_for_the_sender() {
    let p = people().await;
    let ts = &p.ts;
    configured(ts).await;
    // ClaxBot is listed first, live for the owner and not for Sam.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/agents",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let bot = &v["agents"][0];
    assert_eq!(
        (&bot["handle"], &bot["name"], &bot["builtin"], &bot["live"]),
        (
            &json!("claxbot"),
            &json!("ClaxBot"),
            &json!(true),
            &json!(true)
        )
    );
    let (_, v) = call(
        ts,
        &Who::Person(p.sam.0.clone()),
        Method::GET,
        "/api/agents",
        Value::Null,
    )
    .await;
    assert_eq!(
        (&v["agents"][0]["live"], &v["agents"][0]["unavailable"]),
        (&json!(false), &json!("no_flag"))
    );

    // The browser's send.
    let (aid, tid) = artifact_with_thread(ts).await;
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/bot/runs",
        json!({"thread_ids": [tid], "artifact_id": aid}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let run = &v["run"];
    assert_eq!(
        (&run["status"], &run["request"], &run["key_source"]),
        (&json!("queued"), &json!("thread"), &json!("instance"))
    );
    assert_eq!(run["acting_for"]["id"], "*owner");
    assert_eq!(run["thread_ids"], json!([tid]));
    assert_eq!(run["model"], "claude-sonnet-5-5");
    let rid = run["id"].as_str().unwrap().to_string();

    // A thread send to claxbot (the agents' and the MCP tools' path).
    let (_, tid2) = {
        let t = ts.thread(&aid, 1, "And bigger").await;
        ((), t["id"].as_str().unwrap().to_string())
    };
    let (s, v) = call(
        ts,
        &Who::Agent,
        Method::POST,
        &format!("/api/artifacts/{aid}/threads/{tid2}/send"),
        json!({"to": "claxbot"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["run"]["acting_for"]["id"], "*owner");
    let rid2 = v["run"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["thread"]["id"], tid2);
    // A batch: of threads a run works on, it starts none and names that
    // run; of others, one run of them all.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::POST,
        &format!("/api/artifacts/{aid}/threads:send"),
        json!({"thread_ids": [tid, tid2], "to": "claxbot"}),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "run_active"), "{v}");
    let active = v["error"]["run"]["id"].as_str().unwrap();
    assert!(active == rid || active == rid2, "{v}");
    let tid3 = ts.thread(&aid, 1, "Rounder").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid4 = ts.thread(&aid, 1, "Greener").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::POST,
        &format!("/api/artifacts/{aid}/threads:send"),
        json!({"thread_ids": [tid3, tid4], "to": "claxbot"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["run"]["thread_ids"], json!([tid3, tid4]));
    // A new artifact ClaxBot builds.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/artifacts:new",
        json!({"title": "Plan", "brief": "A plan page", "build": "claxbot"}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let brief_run = v["run"]["id"].as_str().unwrap();
    let (s, r) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        &format!("/api/bot/runs/{brief_run}"),
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["request"], "brief");
    assert_eq!(r["steps"], json!([]));
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/artifacts:new",
        json!({"title": "Plan", "brief": "A plan page", "build": "robot"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_build"));
    // Sam is refused before anything is made.
    let (s, v) = call(
        ts,
        &Who::Person(p.sam.0.clone()),
        Method::POST,
        "/api/artifacts:new",
        json!({"title": "Plan", "brief": "x", "build": "claxbot"}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "no_flag"), "{v}");

    // Who sees and stops the run.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        &format!("/api/bot/runs?artifact={aid}"),
        Value::Null,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["runs"].as_array().unwrap().len(), 3);
    assert!(v["runs"][0].get("steps").is_none());
    let path = format!("/api/bot/runs/{rid}");
    for who in [
        Who::Person(p.sam.0.clone()),
        Who::Viewer(p.lan.clone()),
        Who::Person(p.ana.0.clone()),
    ] {
        let (s, v) = call(ts, &who, Method::GET, &path, Value::Null).await;
        assert_eq!(s, 200, "{who:?}: {v}");
    }
    let (s, _) = call(ts, &Who::Agent, Method::GET, &path, Value::Null).await;
    assert_eq!(s, 403);
    let (s, _) = call(ts, &Who::Anonymous, Method::GET, &path, Value::Null).await;
    assert_eq!(s, 401);
    let cancel = format!("{path}/cancel");
    let (s, v) = call(
        ts,
        &Who::Person(p.sam.0.clone()),
        Method::POST,
        &cancel,
        json!({}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_permitted"), "{v}");
    let (s, v) = call(
        ts,
        &Who::Person(p.ana.0.clone()),
        Method::POST,
        &cancel,
        json!({}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["run"]["status"], "stopping");
    assert_eq!(events(ts, "bot.cancel").len(), 1);
    assert_eq!(events(ts, "bot.run_start").len(), 4);
    assert!(!events(ts, "view.run").is_empty());
    // A draft is the acting person's and admins' alone.
    let (s, _) = call(
        ts,
        &Who::Person(p.sam.0.clone()),
        Method::GET,
        &format!("{path}/draft/"),
        Value::Null,
    )
    .await;
    assert_eq!(s, 404);
    let res = ts
        .client
        .get(format!("{}{path}/draft/", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("sandbox")
    );
    assert_eq!(res.text().await.unwrap(), "<p>hi</p>");
}

/// A provider on a loopback port that answers every request 500 with
/// `body`; its base URL.
async fn failing_provider(body: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let res = format!(
                    "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(res.as_bytes()).await;
            });
        }
    });
    base
}

#[tokio::test]
async fn provider_errors_name_no_text_of_the_answer_and_a_new_url_clears_the_key() {
    let ts = TestServer::spawn().await;
    let secret = "<html>intranet console: db=10.0.0.7 password=hunter2</html>";
    let base = failing_provider(secret).await;
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"base_url": base}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/provider/test",
        json!({}),
    )
    .await;
    assert_eq!((s, &v["ok"]), (200, &json!(false)), "{v}");
    let (s2, m) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/provider/models",
        Value::Null,
    )
    .await;
    assert_eq!((s2, code(&m)), (502, "provider_error"), "{m}");
    for shown in [v.to_string(), m.to_string()] {
        for leak in ["hunter2", "10.0.0.7", "intranet", "<html>"] {
            assert!(!shown.contains(leak), "{shown}");
        }
    }
    assert!(v["error"].as_str().unwrap().contains("500"), "{v}");

    // A new base URL drops the key set for the old one.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"base_url": "https://proxy.example.com"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["key"]["set"], false, "{v}");
    let provider = events(&ts, "admin.provider");
    assert_eq!(
        provider.last().unwrap().1["key_cleared"],
        true,
        "{provider:?}"
    );
    assert!(
        events(&ts, "admin.key")
            .iter()
            .any(|(_, b)| b["op"] == "clear")
    );
}

#[tokio::test]
async fn a_hosted_server_sends_the_key_only_to_a_public_https_host() {
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-21T10:00:00Z"));
    let ts = TestServer::spawn_hosted(clock, Some("setup-secret-for-tests")).await;
    assert_eq!(
        ts.store.base_url_policy(),
        clax_store::bot_settings::BaseUrlPolicy::Hosted
    );
    for bad in [
        "http://127.0.0.1:9",
        "https://10.1.2.3",
        "https://metadata.google.internal",
    ] {
        let e = ts
            .store
            .set_bot_settings(&AuditCtx::DAEMON, &[(Setting::BaseUrl, Some(json!(bad)))])
            .unwrap_err();
        assert!(e.to_string().contains("hosted"), "{bad}: {e}");
    }
    assert!(
        ts.store
            .set_bot_settings(
                &AuditCtx::DAEMON,
                &[(Setting::BaseUrl, Some(json!("https://api.anthropic.com")))]
            )
            .is_ok()
    );
}

#[tokio::test]
async fn a_members_own_key_is_never_sent_to_a_host_an_admin_points_at() {
    let p = people().await;
    let ts = &p.ts;
    let sam = Who::Person(p.sam.0.clone());
    let flags = format!("/api/admin/people/{}/flags", p.sam.1);
    let (s, _) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        &flags,
        json!({"claxbot": true}),
    )
    .await;
    assert_eq!(s, 200);
    // Sam's own key is set for the provider in effect.
    let (s, v) = call(
        ts,
        &sam,
        Method::PUT,
        "/api/me/bot/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (_, me) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(
        me["key"]["own"]["host"], "https://api.anthropic.com",
        "{me}"
    );
    assert_eq!(me["provider_host"], "https://api.anthropic.com", "{me}");
    assert_eq!(me["key"]["own"]["problem"], Value::Null, "{me}");
    // Ana, an admin, points the instance at a host of hers.
    let ana = Who::Person(p.ana.0.clone());
    let (s, v) = call(
        ts,
        &ana,
        Method::PUT,
        "/api/admin/provider",
        json!({"base_url": "https://collector.example.com"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // Sam's key is kept, never sent there, and Sam is told why.
    let (_, me) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(me["key"]["own"]["set"], true, "{me}");
    assert_eq!(me["provider_host"], "https://collector.example.com", "{me}");
    let problem = me["key"]["own"]["problem"].as_str().unwrap_or_default();
    assert_eq!(
        problem, "your key: it was set for https://api.anthropic.com; set it again",
        "{me}"
    );
    assert_eq!(
        (&me["available"], &me["unavailable_reason"]),
        (&json!(false), &json!("not_configured")),
        "{me}"
    );
    let (aid, tid) = artifact_with_thread(ts).await;
    let (s, v) = call(
        ts,
        &sam,
        Method::POST,
        "/api/bot/runs",
        json!({"thread_ids": [tid], "artifact_id": aid}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_configured"), "{v}");
    // Set again, it is bound to the new host.
    let (s, _) = call(
        ts,
        &sam,
        Method::PUT,
        "/api/me/bot/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200);
    let (_, me) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(
        me["key"]["own"]["host"], "https://collector.example.com",
        "{me}"
    );
    assert_eq!(me["key"]["own"]["problem"], Value::Null, "{me}");
}

#[tokio::test]
async fn a_settings_write_of_a_new_base_url_clears_the_instance_key() {
    let ts = TestServer::spawn().await;
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!((s, &v["key"]["set"]), (200, &json!(true)), "{v}");
    // Another field keeps it.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/settings",
        json!({"set": {"bot.api_key_env": "MY_KEY"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        ts.store
            .secret_meta(&clax_store::secrets::SecretSlot::Instance)
            .unwrap()
            .is_some()
    );
    // A new base URL in config.toml clears it, recorded as admin.key clear.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/settings",
        json!({"set": {"bot.base_url": "https://proxy.example"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        ts.store
            .secret_meta(&clax_store::secrets::SecretSlot::Instance)
            .unwrap()
            .is_none()
    );
    let ops: Vec<String> = events(&ts, "admin.key")
        .iter()
        .map(|(_, b)| b["op"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ops, ["set", "clear"]);
}

#[tokio::test]
async fn an_admin_changes_only_the_bot_and_questions_settings() {
    let p = people().await;
    let ts = &p.ts;
    let ana = Who::Person(p.ana.0.clone());
    // Every variable name, [serve], [sample], [hosted], [toolpath] and bin
    // are the owner's: an admin could otherwise make the daemon read any
    // variable and send it to a URL of theirs.
    for key in [
        "bot.api_key_env",
        "bot.base_url",
        "sample.api_key_env",
        "sample.provider",
        "serve.port",
        "hosted.public_origin",
        "toolpath.journal",
        "bin",
    ] {
        let (s, v) = call(
            ts,
            &ana,
            Method::PUT,
            "/api/settings",
            json!({"set": {key: "X"}}),
        )
        .await;
        assert_eq!((s, code(&v)), (403, "owner_only"), "{key}: {v}");
    }
    assert!(!ts.home.root().join("config.toml").exists());
    let (s, v) = call(ts, &ana, Method::GET, "/api/settings", Value::Null).await;
    assert_eq!(s, 200, "{v}");
    let field = |k: &str| {
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == k)
            .unwrap_or_else(|| panic!("no {k}"))
            .clone()
    };
    assert_eq!(field("bot.api_key_env")["editable"], false);
    assert_eq!(field("bot.base_url")["editable"], false);
    assert_eq!(field("serve.port")["editable"], false);
    assert_eq!(field("bot.max_steps")["editable"], true);
    // The rest of [bot], and [questions], an admin may change.
    let (s, v) = call(
        ts,
        &ana,
        Method::PUT,
        "/api/settings",
        json!({"set": {"bot.max_steps": 12, "questions.terminal_after_s": 30}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // The owner changes the owner's fields.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/settings",
        json!({"set": {"bot.api_key_env": "MY_KEY"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

/// A provider on a loopback port that answers every request 500 and keeps
/// each request's `x-api-key`; its base URL and those keys.
async fn key_catcher() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let s = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = l.accept().await {
            let s = s.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some(k) = head.lines().find_map(|l| {
                    l.strip_prefix("x-api-key: ")
                        .or_else(|| l.strip_prefix("X-Api-Key: "))
                }) {
                    s.lock().unwrap().push(k.trim().to_string());
                }
                let res = "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                let _ = sock.write_all(res.as_bytes()).await;
            });
        }
    });
    (base, seen)
}

#[tokio::test]
async fn the_environments_key_goes_only_to_the_owners_base_url() {
    const ENV_KEY: &str = "sk-ant-env-OWNERSOWNERSOWNERS-9999";
    let ts = TestServer::spawn_with(|s| {
        let kek = s.bot.kek.clone();
        s.bot = std::sync::Arc::new(clax_http::bot::Bot {
            kek,
            env_key: Some(clax_http::bot::EnvKey {
                var: "ANTHROPIC_API_KEY".into(),
                key: clax_core::secret::Secret::new(ENV_KEY.to_string()),
            }),
            ..Default::default()
        });
    })
    .await;
    let ana = ts.person("Ana").await;
    ts.store
        .set_admin(&AuditCtx::DAEMON, &ana.public_id, true)
        .unwrap();
    let ana = Who::Person(ana.cookie);
    let test = |who: Who| {
        let ts = &ts;
        async move {
            let (s, v) = call(
                ts,
                &who,
                Method::POST,
                "/api/admin/provider/test",
                json!({}),
            )
            .await;
            assert_eq!(s, 200, "{v}");
            v
        }
    };
    // The base URL from the configuration (the owner's): the
    // environment's key is the instance key, and the test sends it.
    let (owners, seen_owner) = key_catcher().await;
    let mut ov = Overrides::default();
    ov.set(Setting::BaseUrl, &json!(owners), "file").unwrap();
    ts.store.set_bot_overrides(ov);
    let (_, v) = call(&ts, &ana, Method::GET, "/api/admin/provider", Value::Null).await;
    assert_eq!(v["sources"]["key"], "env:ANTHROPIC_API_KEY", "{v}");
    test(Who::OwnerBrowser).await;
    assert_eq!(*seen_owner.lock().unwrap(), [ENV_KEY]);
    // An admin's base URL (the database's): the environment's key is not
    // sent there, by the provider's test or by a run, whoever presses it.
    ts.store.set_bot_overrides(Overrides::default());
    let (theirs, seen_admin) = key_catcher().await;
    let (s, v) = call(
        &ts,
        &ana,
        Method::PUT,
        "/api/admin/provider",
        json!({"base_url": theirs}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["key"]["set"], false, "{v}");
    assert_ne!(v["sources"]["key"], "env:ANTHROPIC_API_KEY", "{v}");
    for who in [ana.clone(), Who::OwnerBrowser] {
        let v = test(who).await;
        assert_eq!(v["ok"], false, "{v}");
    }
    let (_, me) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/me/bot",
        Value::Null,
    )
    .await;
    assert_eq!(
        (&me["available"], &me["unavailable_reason"]),
        (&json!(false), &json!("not_configured")),
        "{me}"
    );
    // A stored instance key is the one that goes there.
    let (s, v) = call(
        &ts,
        &ana,
        Method::PUT,
        "/api/admin/provider/key",
        json!({"key": KEY}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    test(ana).await;
    assert_eq!(*seen_admin.lock().unwrap(), [KEY]);
    // In config.toml the base URL is the owner's to write.
    let ana2 = ts.person("Ann").await;
    ts.store
        .set_admin(&AuditCtx::DAEMON, &ana2.public_id, true)
        .unwrap();
    let (s, v) = call(
        &ts,
        &Who::Person(ana2.cookie),
        Method::PUT,
        "/api/settings",
        json!({"set": {"bot.base_url": "https://proxy.example"}}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
}

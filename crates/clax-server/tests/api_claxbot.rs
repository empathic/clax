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
            (Who::Anonymous, (401, "no_credential")),
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
        json!({"claxbot": true, "claxbot_unenforced": false})
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
    // The key-encryption key that sealed it, by fingerprint, for admins.
    assert_eq!(v["kek"]["set"], true, "{v}");
    let fp = v["kek"]["fingerprint"].as_str().unwrap_or_default();
    assert!(
        fp.len() == 8
            && fp
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{v}"
    );
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
        (Who::Anonymous, (401, "no_credential")),
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
    assert_eq!(v["api"], "anthropic");
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
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"api": "cohere"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_setting"), "{v}");
    assert_eq!(events(ts, "admin.provider").len(), 1);
    // Another API and its base URL, set together.
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"api": "gemini", "base_url": "https://generativelanguage.googleapis.com/v1beta"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["api"], "gemini");
    assert_eq!(v["sources"]["api"], "db");
    assert_eq!(
        v["base_url"],
        "https://generativelanguage.googleapis.com/v1beta"
    );
    assert_eq!(events(ts, "admin.provider").len(), 2);
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
    // `lines` leaves the token lines out: they are under `models`.
    let all = v["lines"].as_array().unwrap().clone();
    assert!(all.iter().any(|l| l["id"] == "toolpath.storage"), "{v}");
    assert!(
        !all.iter().any(|l| l["id"] == "claude-sonnet-5-5.input"),
        "{v}"
    );
    let lines = all.clone();
    let token_lines = || -> Vec<clax_store::credits::CostLine> {
        ts.store
            .cost_lines()
            .unwrap()
            .into_iter()
            .filter(|l| clax_store::credits::parse_token_line(&l.id).is_some())
            .collect()
    };
    let tokens_before = token_lines();
    // What GET answered, PUT takes back as it is, changing nothing.
    let (s, back) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        v.clone(),
    )
    .await;
    assert_eq!(s, 200, "{back}");
    assert_eq!(back, v);
    assert!(events(&ts, "admin.cost_line").is_empty());
    // A token line sent is refused, and nothing is written.
    let mut with_token = json!({"credit_price_micro_usd": 30_000, "free_weekly_milli": 1_000_000,
                                "lines": lines.clone()});
    with_token["lines"].as_array_mut().unwrap().push(json!({
        "id": "claude-sonnet-5-5.input", "label": "Claude Sonnet 5.5 input tokens",
        "unit": "mtok_input", "cost_micro_usd": 1, "margin_bp": 0, "fixed_credits_milli": null,
        "applies": "instance_key", "source": "by hand", "model": "claude-sonnet-5-5"}));
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        with_token,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_cost_line"), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("POST /api/admin/pricing/models"),
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
    assert_eq!(after["lines"], json!(all));
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
    // The token lines it was not sent are kept, unchanged.
    assert_eq!(token_lines(), tokens_before);
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
        "bot.price_catalog_url",
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
    assert_eq!(field("bot.price_catalog_url")["editable"], false);
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

// ---------------------------------------------------------------------------
// Pricing: models, rate cards, default rates and the price catalog
// ---------------------------------------------------------------------------

/// A price catalog on a loopback port: each `GET` is answered with what
/// `body` holds (`None`: 500) and told on `hits` (whether it was answered
/// 200).
struct Catalog {
    url: String,
    body: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    hits: tokio::sync::mpsc::UnboundedReceiver<bool>,
}

impl Catalog {
    async fn start(site: Value) -> Catalog {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/data/site.json", l.local_addr().unwrap());
        let body = std::sync::Arc::new(std::sync::Mutex::new(Some(site.to_string())));
        let (tx, hits) = tokio::sync::mpsc::unbounded_channel();
        let served = body.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                let body = served.lock().unwrap().clone();
                let _ = tx.send(body.is_some());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    let res = match body {
                        Some(b) => format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}",
                            b.len()
                        ),
                        None => "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
                    };
                    let _ = sock.write_all(res.as_bytes()).await;
                });
            }
        });
        Catalog { url, body, hits }
    }

    /// Answers 500 from now on.
    fn fail(&self) {
        *self.body.lock().unwrap() = None;
    }

    /// Waits (at most 30 s) for the next request: whether it was answered
    /// 200.
    async fn hit(&mut self) -> bool {
        tokio::time::timeout(std::time::Duration::from_secs(30), self.hits.recv())
            .await
            .expect("a catalog request within 30 s")
            .expect("the catalog server runs")
    }
}

/// A catalog listing each `(provider, model, input, output, cache_read)`,
/// in US dollars per million tokens.
fn site(models: &[(&str, &str, f64, f64, Option<f64>)]) -> Value {
    json!({"schema_version": 1, "catalog": {"models": models.iter().map(|(p, m, i, o, cr)| json!({
        "provider_id": p, "model_id": m, "model": format!("Model {m}"),
        "pricing": {"standard": {"input": i, "output": o, "cache_read": cr, "cache_write": null}},
        "source": "https://example.test/pricing", "observed_at": "2026-11-01T00:00:00+00:00",
    })).collect::<Vec<_>>()}})
}

/// A daemon whose price catalog is at `url`.
async fn with_catalog(url: &str) -> TestServer {
    let url = url.to_string();
    TestServer::spawn_with(move |s| {
        let kek = s.bot.kek.clone();
        s.bot = std::sync::Arc::new(clax_http::bot::Bot {
            kek,
            catalog_url: Some(url),
            ..Default::default()
        });
    })
    .await
}

/// The owner's `GET /api/admin/pricing`.
async fn pricing(ts: &TestServer) -> Value {
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/pricing",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

/// Model `id` of a pricing view.
fn model<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["model"] == id)
        .unwrap_or_else(|| panic!("no model {id} in {v}"))
}

#[tokio::test]
async fn pricing_shows_each_models_rates_and_where_they_come_from() {
    let cat = Catalog::start(site(&[
        // The seeded Sonnet's lines cost $2 input: $3 drifts.
        ("anthropic", "claude-sonnet-5-5", 3.0, 10.0, Some(0.2)),
        ("anthropic", "Claude-New-1", 1.0, 5.0, None),
        ("openai", "gpt-x", 1.0, 2.0, None),
    ]))
    .await;
    let ts = with_catalog(&cat.url).await;
    let v = pricing(&ts).await;
    assert_eq!(
        v["catalog"],
        json!({"fetched_at": null, "provider": "anthropic"})
    );
    assert_eq!(
        v["default_rates"],
        json!({"input": 560_000, "cached_input": 28_000, "cache_write": 700_000,
               "output": 2_800_000})
    );
    assert_eq!(v["require_explicit_price"], false);
    assert_eq!(v["token_margin_bp"], 4_000);
    assert_eq!(v["sources"]["default_rates.output"], "default");
    assert_eq!(v["sources"]["require_explicit_price"], "default");
    // The tiers' models first, then the other models with lines.
    let ids: Vec<&str> = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-opus-5-5",
            "claude-fable-5-1"
        ]
    );
    let sonnet = model(&v, "claude-sonnet-5-5");
    assert_eq!(sonnet["tiers"], json!(["default"]));
    // Named by its lines' labels.
    assert_eq!(sonnet["name"], "Claude Sonnet 5.5");
    assert_eq!(model(&v, "claude-fable-5-1")["tiers"], json!([]));
    assert_eq!(
        sonnet["units"]["input"],
        json!({"source": "line", "line_source": "Anthropic list price as of 2026-10-06 (check)",
               "cost_micro_usd": 2_000_000, "margin_bp": 4_000,
               "fixed_credits_milli": null, "applies": "instance_key",
               "credits_milli_per_mtok": 280_000, "catalog_micro_usd": null, "drift": false})
    );
    assert_eq!(sonnet["listing"], Value::Null);

    // A model with neither lines nor a listing takes the default rates.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"models": {"quick": "claude-new-1"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let v = pricing(&ts).await;
    let new = model(&v, "claude-new-1");
    assert_eq!(new["tiers"], json!(["quick"]));
    assert_eq!(new["name"], Value::Null);
    assert_eq!(new["units"]["output"]["source"], "default");
    assert_eq!(new["units"]["output"]["fixed_credits_milli"], 2_800_000);
    assert_eq!(new["units"]["output"]["credits_milli_per_mtok"], 2_800_000);

    // Refreshed, it is priced from the catalog (IDs ignoring case), and
    // Sonnet's input line drifts from its listing.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/pricing/catalog/refresh",
        json!({}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let fetched_at = v["catalog"]["fetched_at"].clone();
    assert!(fetched_at.is_string(), "{v}");
    let new = model(&v, "claude-new-1");
    assert_eq!(new["listing"]["model_id"], "Claude-New-1");
    // Without lines, named by its listing.
    assert_eq!(new["name"], "Model Claude-New-1");
    assert_eq!(
        new["units"]["output"],
        json!({"source": "catalog", "line_source": null, "cost_micro_usd": 5_000_000, "margin_bp": 4_000,
               "fixed_credits_milli": null, "applies": "instance_key",
               "credits_milli_per_mtok": 700_000, "catalog_micro_usd": 5_000_000,
               "drift": false})
    );
    // No cache rate listed: charged at the input rate, shown as none.
    assert_eq!(new["units"]["cached_input"]["cost_micro_usd"], 1_000_000);
    assert_eq!(
        new["units"]["cached_input"]["catalog_micro_usd"],
        Value::Null
    );
    let sonnet = model(&v, "claude-sonnet-5-5");
    assert_eq!(sonnet["units"]["input"]["catalog_micro_usd"], 3_000_000);
    assert_eq!(sonnet["units"]["input"]["drift"], true);
    assert_eq!(sonnet["units"]["output"]["drift"], false);
    assert_eq!(sonnet["drift"], true);

    // A refresh that fails keeps the cache.
    cat.fail();
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/pricing/catalog/refresh",
        json!({}),
    )
    .await;
    assert_eq!((s, code(&v)), (502, "tokenulator_error"), "{v}");
    let v = pricing(&ts).await;
    assert_eq!(v["catalog"]["fetched_at"], fetched_at);
    assert_eq!(
        model(&v, "claude-new-1")["units"]["output"]["source"],
        "catalog"
    );
    // Agents are refused.
    let (s, v) = call(
        &ts,
        &Who::Agent,
        Method::POST,
        "/api/admin/pricing/catalog/refresh",
        json!({}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
}

#[tokio::test]
async fn model_rates_are_set_and_deleted_by_hand() {
    let ts = TestServer::spawn().await;
    configured(&ts).await;
    let card = |model: &str| {
        json!({"model": model, "name": "Custom One",
               "input": {"cost_micro_usd": 1_000_000, "margin_bp": 5_000, "fixed_credits_milli": null},
               "cached_input": {"cost_micro_usd": 100_000, "margin_bp": 5_000, "fixed_credits_milli": null},
               "cache_write": {"cost_micro_usd": 1_250_000, "margin_bp": 5_000, "fixed_credits_milli": null},
               "output": {"cost_micro_usd": 0, "margin_bp": 0, "fixed_credits_milli": 900_000}})
    };
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/pricing/models",
        card("custom-1"),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let m = model(&v, "custom-1");
    assert_eq!(m["tiers"], json!([]));
    assert_eq!(m["name"], "Custom One");
    assert_eq!(m["units"]["input"]["source"], "line");
    assert_eq!(m["units"]["input"]["credits_milli_per_mtok"], 150_000);
    assert_eq!(m["units"]["output"]["fixed_credits_milli"], 900_000);
    assert_eq!(m["units"]["output"]["credits_milli_per_mtok"], 900_000);
    // Its lines are under `models`, not `lines`; the store holds them.
    assert!(
        !v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["id"] == "custom-1.output")
    );
    let line =
        serde_json::to_value(ts.store.cost_line("custom-1.output").unwrap().unwrap()).unwrap();
    assert_eq!(line["label"], "Custom One output tokens");
    assert_eq!(line["source"], "set by an admin");
    assert_eq!(line["model"], "custom-1");
    assert_eq!(events(&ts, "admin.cost_line").len(), 4);
    // Set again without a name, its lines keep theirs.
    let mut again = card("custom-1");
    again.as_object_mut().unwrap().remove("name");
    again["input"]["cost_micro_usd"] = json!(2_000_000);
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/pricing/models",
        again,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(model(&v, "custom-1")["name"], "Custom One");
    assert_eq!(
        model(&v, "custom-1")["units"]["input"]["cost_micro_usd"],
        2_000_000
    );
    // Only the input line changed.
    assert_eq!(events(&ts, "admin.cost_line").len(), 5);
    // A new model without a name is named by its ID.
    let mut unnamed = card("custom-3");
    unnamed.as_object_mut().unwrap().remove("name");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/admin/pricing/models",
        unnamed,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(model(&v, "custom-3")["name"], "custom-3");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::DELETE,
        "/api/admin/pricing/models?model=custom-3",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let base = events(&ts, "admin.cost_line").len();

    // Values out of range, a bad model ID or an unknown field write nothing.
    let mut bad = card("custom-2");
    bad["output"]["margin_bp"] = json!(-5);
    let mut no_name = card("");
    no_name["name"] = Value::Null;
    let mut extra = card("custom-2");
    extra["note"] = json!("x");
    // A misspelt unit field would otherwise make a credit-based line
    // cost-based at 0.
    let mut misspelt = card("custom-2");
    misspelt["output"] =
        json!({"cost_micro_usd": 0, "margin_bp": 0, "fixed_credit_milli": 900_000});
    let mut missing = card("custom-2");
    missing["output"] = json!({"cost_micro_usd": 0, "margin_bp": 0});
    for (body, want) in [
        (bad, "invalid_cost_line"),
        (no_name, "invalid_cost_line"),
        (extra, "invalid_json"),
        (misspelt, "invalid_json"),
        (missing, "invalid_json"),
    ] {
        let (s, v) = call(
            &ts,
            &Who::OwnerBrowser,
            Method::POST,
            "/api/admin/pricing/models",
            body,
        )
        .await;
        assert_eq!((s, code(&v)), (400, want), "{v}");
    }
    assert_eq!(events(&ts, "admin.cost_line").len(), base);

    // Deleted, its lines go, and it leaves the page (no tier uses it).
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::DELETE,
        "/api/admin/pricing/models?model=custom-1",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        !v["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["model"] == "custom-1")
    );
    assert!(ts.store.cost_line("custom-1.input").unwrap().is_none());
    assert_eq!(events(&ts, "admin.cost_line").len(), base + 4);
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::DELETE,
        "/api/admin/pricing/models?model=custom-1",
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::DELETE,
        "/api/admin/pricing/models",
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_query"), "{v}");

    // A model a run that has not ended uses keeps its lines.
    let (aid, tid) = artifact_with_thread(&ts).await;
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::POST,
        "/api/bot/runs",
        json!({"thread_ids": [tid], "artifact_id": aid}),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::DELETE,
        "/api/admin/pricing/models?model=claude-sonnet-5-5",
        Value::Null,
    )
    .await;
    assert_eq!((s, code(&v)), (409, "cost_line_in_use"), "{v}");
    assert_eq!(
        model(&pricing(&ts).await, "claude-sonnet-5-5")["units"]["input"]["source"],
        "line"
    );
    // So does a PUT of the pricing that leaves out a line the run may
    // charge (one with no model), and it writes nothing.
    let mut put = pricing(&ts).await;
    put["credit_price_micro_usd"] = json!(20_000);
    put["lines"]
        .as_array_mut()
        .unwrap()
        .retain(|l| l["id"] != "toolpath.storage");
    let before = events(&ts, "admin.cost_line").len();
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        put,
    )
    .await;
    assert_eq!((s, code(&v)), (409, "cost_line_in_use"), "{v}");
    let after = pricing(&ts).await;
    assert_eq!(after["credit_price_micro_usd"], 10_000);
    assert!(
        after["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["id"] == "toolpath.storage")
    );
    assert_eq!(events(&ts, "admin.cost_line").len(), before);
}

#[tokio::test]
async fn default_rates_and_explicit_prices_round_trip_and_overrides_are_refused() {
    let ts = TestServer::spawn().await;
    let mut put = pricing(&ts).await;
    put["default_rates"]["output"] = json!(1_000_000);
    put["require_explicit_price"] = json!(true);
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        put.clone(),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["default_rates"]["output"], 1_000_000);
    assert_eq!(v["default_rates"]["input"], 560_000);
    assert_eq!(v["require_explicit_price"], true);
    assert_eq!(v["sources"]["default_rates.output"], "db");
    assert_eq!(v["sources"]["default_rates.input"], "default");
    assert_eq!(v["sources"]["require_explicit_price"], "db");
    let ops: Vec<Value> = events(&ts, "admin.cost_line")
        .into_iter()
        .map(|(_, b)| b["line"].clone())
        .collect();
    assert_eq!(ops, [json!("*instance")]);
    // With explicit prices required, a model without lines has no price.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"models": {"quick": "claude-unlined"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let v = pricing(&ts).await;
    assert_eq!(
        model(&v, "claude-unlined")["units"]["input"],
        json!({"source": null, "line_source": null, "cost_micro_usd": null, "margin_bp": null,
               "fixed_credits_milli": null, "applies": null,
               "credits_milli_per_mtok": null, "catalog_micro_usd": null, "drift": false})
    );
    // Both may be left out of a PUT: they are kept.
    let mut short = v.clone();
    let o = short.as_object_mut().unwrap();
    o.remove("default_rates");
    o.remove("require_explicit_price");
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        short,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["require_explicit_price"], true);
    // Bad values are refused.
    for (key, bad) in [
        ("default_rates", json!({"input": -1})),
        ("require_explicit_price", json!("yes")),
    ] {
        let mut b = v.clone();
        b[key] = bad;
        let (s, e) = call(
            &ts,
            &Who::OwnerBrowser,
            Method::PUT,
            "/api/admin/pricing",
            b,
        )
        .await;
        assert_eq!((s, code(&e)), (400, "invalid_setting"), "{key}: {e}");
    }
    // A value the configuration sets is refused when it differs, and
    // taken as no change when it is the same.
    let mut ov = Overrides::default();
    ov.set(Setting::DefaultRateInput, &json!(10), "file")
        .unwrap();
    ts.store.set_bot_overrides(ov);
    let v = pricing(&ts).await;
    assert_eq!(v["sources"]["default_rates.input"], "file");
    assert_eq!(v["default_rates"]["input"], 10);
    let (s, _) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        v.clone(),
    )
    .await;
    assert_eq!(s, 200);
    let mut b = v.clone();
    b["default_rates"]["input"] = json!(11);
    let (s, e) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/pricing",
        b,
    )
    .await;
    assert_eq!((s, code(&e)), (409, "setting_overridden"), "{e}");
}

#[tokio::test]
async fn tokenulator_prices_a_model_from_the_refreshed_catalog() {
    let cat = Catalog::start(site(&[
        ("anthropic", "claude-x", 1.5, 7.5, Some(0.15)),
        ("mistral", "mistral-large", 2.0, 6.0, None),
    ]))
    .await;
    let ts = with_catalog(&cat.url).await;
    let price = |body: Value| {
        let ts = &ts;
        async move {
            call(
                ts,
                &Who::OwnerBrowser,
                Method::POST,
                "/api/admin/pricing/tokenulator",
                body,
            )
            .await
        }
    };
    let (s, v) = price(json!({"model": "claude-x"})).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["catalog"]["fetched_at"].is_string(), "{v}");
    // A token line as the store holds it (`lines` leaves them out).
    let line = |_v: &Value, id: &str| {
        serde_json::to_value(
            ts.store
                .cost_line(id)
                .unwrap()
                .unwrap_or_else(|| panic!("no line {id}")),
        )
        .unwrap()
    };
    let input = line(&v, "claude-x.input");
    assert_eq!(input["cost_micro_usd"], 1_500_000);
    assert_eq!(input["margin_bp"], 4_000);
    assert_eq!(input["label"], "Model claude-x input tokens");
    assert!(
        input["source"].as_str().unwrap().starts_with(
            "tokenulator.dev: anthropic claude-x standard rates from https://example.test/pricing"
        ),
        "{input}"
    );
    assert_eq!(line(&v, "claude-x.cached_input")["cost_micro_usd"], 150_000);
    // No cache write listed: the input rate.
    assert_eq!(
        line(&v, "claude-x.cache_write")["cost_micro_usd"],
        1_500_000
    );
    assert_eq!(model(&v, "claude-x")["units"]["output"]["source"], "line");
    // The page reads each unit's line source from the view itself.
    assert_eq!(
        model(&v, "claude-x")["units"]["input"]["line_source"],
        input["source"]
    );
    // Another provider by its ID.
    let (s, v) = price(json!({"model": "mistral-large", "provider": "mistral"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        line(&v, "mistral-large.output")["cost_micro_usd"],
        6_000_000
    );
    // Not listed.
    let (s, v) = price(json!({"model": "claude-nope"})).await;
    assert_eq!((s, code(&v)), (404, "not_on_tokenulator"), "{v}");
    // The catalog cannot be read: a model the cache lists is priced from
    // it, any other refused.
    cat.fail();
    let (s, v) = price(json!({"model": "claude-x"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(line(&v, "claude-x.input")["cost_micro_usd"], 1_500_000);
    let (s, v) = price(json!({"model": "claude-nope"})).await;
    assert_eq!((s, code(&v)), (502, "tokenulator_error"), "{v}");
    // A base URL the catalog knows no provider of needs one named.
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/provider",
        json!({"api": "openai", "base_url": "https://api.subconscious.dev/v1"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(pricing(&ts).await["catalog"]["provider"], Value::Null);
    let (s, v) = price(json!({"model": "claude-x"})).await;
    assert_eq!((s, code(&v)), (400, "no_provider"), "{v}");
}

/// A daemon whose store reads `clock` and whose price catalog is at `url`.
async fn with_catalog_and_clock(
    url: &str,
    clock: std::sync::Arc<clax_core::working::ManualClock>,
) -> TestServer {
    let url = url.to_string();
    TestServer::spawn_with_clock_and(clock, move |s| {
        let kek = s.bot.kek.clone();
        s.bot = std::sync::Arc::new(clax_http::bot::Bot {
            kek,
            catalog_url: Some(url),
            ..Default::default()
        });
    })
    .await
}

#[tokio::test]
async fn the_daemons_refresher_fetches_a_stale_catalog_and_keeps_it_through_failures() {
    let mut cat = Catalog::start(site(&[("anthropic", "claude-x", 1.0, 2.0, None)])).await;
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at("2026-11-01T09:00:00Z"));
    let ts = with_catalog_and_clock(&cat.url, clock.clone()).await;
    // A short period, so it looks again at once; the empty cache is due.
    let task = clax_server::catalog::spawn_refresher(
        ts.state.clone(),
        std::time::Duration::from_millis(1),
    );
    assert!(cat.hit().await);
    task.abort();
    let _ = task.await;

    // A cache a day old is due: each failing refresh keeps it (still due,
    // it is tried again), so once a request after one answered 500 came,
    // that failed refresh has ended.
    let mut cat = Catalog::start(site(&[])).await;
    cat.fail();
    let ts = with_catalog_and_clock(&cat.url, clock.clone()).await;
    let kept = clax_store::price_catalog::CatalogListing {
        provider_id: "anthropic".into(),
        model_id: "claude-x".into(),
        name: "Claude X".into(),
        input_micro_usd: 1_000_000,
        output_micro_usd: 2_000_000,
        cache_read_micro_usd: None,
        cache_write_micro_usd: None,
        source: "https://example.test/pricing".into(),
        observed_at: "2026-10-30".into(),
    };
    let day_ago = ts.store.now_utc() - chrono::Duration::hours(24);
    ts.store
        .replace_price_catalog(std::slice::from_ref(&kept), day_ago)
        .unwrap();
    let task = clax_server::catalog::spawn_refresher(
        ts.state.clone(),
        std::time::Duration::from_millis(1),
    );
    while cat.hit().await {}
    cat.hit().await;
    assert_eq!(
        ts.store.catalog_listing("anthropic", "claude-x").unwrap(),
        Some(kept)
    );
    assert_eq!(ts.store.catalog_fetched_at().unwrap(), Some(day_ago));
    task.abort();
    let _ = task.await;
    // Without a URL (the default) nothing is fetched and the task ends.
    let none = TestServer::spawn().await;
    assert_eq!(none.state.bot.catalog_url, None);
    clax_server::catalog::spawn_refresher(none.state.clone(), std::time::Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(none.store.catalog_fetched_at().unwrap(), None);
}

#[tokio::test]
async fn concurrent_refreshes_share_one_fetch() {
    let mut cat = Catalog::start(site(&[("anthropic", "claude-x", 1.0, 2.0, None)])).await;
    let ts = with_catalog(&cat.url).await;
    // The first holds the flight while it fetches; the second waits on it
    // and takes its outcome.
    let (a, b) = tokio::join!(
        clax_server::catalog::refresh(&ts.state),
        clax_server::catalog::refresh(&ts.state)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b);
    assert!(cat.hit().await);
    assert!(cat.hits.try_recv().is_err(), "one fetch, not two");
    // One that starts after it ended fetches again.
    clax_server::catalog::refresh(&ts.state).await.unwrap();
    assert!(cat.hit().await);
}

#[tokio::test]
async fn a_stale_catalog_is_refreshed_by_the_stores_clock() {
    let mut cat = Catalog::start(site(&[("anthropic", "claude-x", 1.0, 2.0, None)])).await;
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at("2026-11-01T09:00:00Z"));
    let ts = with_catalog_and_clock(&cat.url, clock.clone()).await;
    // Empty: refreshed.
    let r = clax_server::catalog::refresh_if_stale(&ts.state)
        .await
        .expect("an empty cache is stale")
        .unwrap();
    assert_eq!(r.listings, 1);
    assert_eq!(r.fetched_at.to_rfc3339(), "2026-11-01T09:00:00+00:00");
    cat.hit().await;
    // Fresh until a day has passed.
    clock.advance(24 * 3600 - 1);
    assert!(
        clax_server::catalog::refresh_if_stale(&ts.state)
            .await
            .is_none()
    );
    clock.advance(1);
    let r = clax_server::catalog::refresh_if_stale(&ts.state)
        .await
        .expect("a day-old cache is stale")
        .unwrap();
    assert_eq!(r.fetched_at.to_rfc3339(), "2026-11-02T09:00:00+00:00");
    cat.hit().await;
    assert!(cat.hits.try_recv().is_err(), "two fetches, not more");
}

#[tokio::test]
async fn the_providers_models_say_how_each_is_priced() {
    use clax_testkit::fake_messages::FakeMessages;
    let fake = FakeMessages::start(vec![]).await;
    let ts = TestServer::spawn().await;
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::BaseUrl, Some(json!(fake.base)))],
        )
        .unwrap();
    configured(&ts).await;
    // Haiku's output line goes: that unit takes the default rate, the
    // rest their lines.
    ts.store
        .delete_cost_line(&AuditCtx::DAEMON, "claude-haiku-5-5.output")
        .unwrap();
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/provider/models",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let models = v["models"].as_array().unwrap();
    assert_eq!(
        models[0],
        json!({"id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5",
               "pricing": {"source": "line", "estimated": false,
                           "input_credits_milli_per_mtok": 280_000,
                           "output_credits_milli_per_mtok": 1_400_000}})
    );
    assert_eq!(
        models[1]["pricing"],
        json!({"source": "mixed", "estimated": true, "input_credits_milli_per_mtok": 14_000,
               "output_credits_milli_per_mtok": 2_800_000})
    );
    // All four lines gone: every unit at the default rates.
    ts.store
        .delete_model_rates(&AuditCtx::DAEMON, "claude-haiku-5-5")
        .unwrap();
    let (_, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/provider/models",
        Value::Null,
    )
    .await;
    assert_eq!(
        v["models"][1]["pricing"],
        json!({"source": "default", "estimated": true, "input_credits_milli_per_mtok": 560_000,
               "output_credits_milli_per_mtok": 2_800_000})
    );
}

const CF_TOKEN: &str = "cf-TOPSECRETTOKENVALUE-wxyz";

#[tokio::test]
async fn admin_web_round_trips() {
    let p = people().await;
    let ts = &p.ts;
    let owner = Who::OwnerBrowser;
    let (s, v) = call(ts, &owner, Method::GET, "/api/admin/web", Value::Null).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["search"]["enabled"], false);
    assert_eq!(v["search"]["provider"], "ceramic");
    assert_eq!(v["search"]["limit"], 5);
    assert_eq!(v["search"]["max_per_run"], 10);
    assert_eq!(v["fetch"]["url_rule"], "sourced");
    assert_eq!(v["fetch"]["allowlist"], json!([]));
    assert_eq!(v["fetch"]["max_per_run"], 20);
    assert_eq!(v["cloudflare"]["account_id"], "");
    assert_eq!(v["cloudflare"]["token"]["set"], false);
    assert_eq!(v["configured"], false);
    assert_eq!(v["sources"]["search.provider"], "default");
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({
            "search": {"enabled": true, "provider": "exa", "limit": 3, "max_per_run": 4},
            "fetch": {"url_rule": "allowlist", "allowlist": ["example.com"]},
            "cloudflare": {"account_id": "0123456789abcdef0123456789abcdef"},
        }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["search"]["enabled"], true);
    assert_eq!(v["search"]["provider"], "exa");
    assert_eq!(v["search"]["limit"], 3);
    assert_eq!(v["search"]["max_per_run"], 4);
    assert_eq!(v["fetch"]["enabled"], false);
    assert_eq!(v["fetch"]["url_rule"], "allowlist");
    assert_eq!(v["fetch"]["allowlist"], json!(["example.com"]));
    assert_eq!(
        v["cloudflare"]["account_id"],
        "0123456789abcdef0123456789abcdef"
    );
    assert_eq!(v["sources"]["search.provider"], "db");
    assert_eq!(events(ts, "admin.provider").len(), 1);
    // null puts a field back to its default.
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"search": {"provider": null}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["search"]["provider"], "ceramic");
    // A field the daemon's configuration sets is refused.
    let mut ov = Overrides::default();
    ov.set(Setting::WebSearchLimit, &json!(7), "file").unwrap();
    ts.store.set_bot_overrides(ov);
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"search": {"limit": 2}}),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "setting_overridden"), "{v}");
    let (_, v) = call(ts, &owner, Method::GET, "/api/admin/web", Value::Null).await;
    assert_eq!(v["search"]["limit"], 7);
    assert_eq!(v["sources"]["search.limit"], "file");
    // An unknown field is refused.
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"search": {"nope": 1}}),
    )
    .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn a_bot_web_file_value_is_read_only_on_the_admin_page() {
    let p = people().await;
    let ts = &p.ts;
    let cfg = clax_core::config::HomeConfig::from_text(
        std::path::PathBuf::from("/h/config.toml"),
        "[bot.web.fetch]\nurl_rule = \"any_public\"\n[bot.web.search]\nenabled = true\n",
        [],
    )
    .unwrap();
    ts.store
        .set_bot_overrides(clax_server::core::bot_overrides(&cfg).0);
    let owner = Who::OwnerBrowser;
    let (_, v) = call(ts, &owner, Method::GET, "/api/admin/web", Value::Null).await;
    assert_eq!(v["fetch"]["url_rule"], "any_public");
    assert_eq!(v["sources"]["fetch.url_rule"], "file");
    assert_eq!(v["search"]["enabled"], true);
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"fetch": {"url_rule": "sourced"}}),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "setting_overridden"), "{v}");
    // A field the file leaves alone is the page's.
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"fetch": {"enabled": true}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["sources"]["fetch.enabled"], "db");
}

#[tokio::test]
async fn admin_web_refuses_a_bad_rule() {
    let p = people().await;
    let ts = &p.ts;
    for body in [
        json!({"fetch": {"url_rule": "anything"}}),
        json!({"search": {"provider": "bing"}}),
        json!({"search": {"limit": 11}}),
        json!({"fetch": {"allowlist": ["not a host"]}}),
        json!({"cloudflare": {"account_id": "0123"}}),
    ] {
        let (s, v) = call(
            ts,
            &Who::OwnerBrowser,
            Method::PUT,
            "/api/admin/web",
            body.clone(),
        )
        .await;
        assert_eq!(s, 400, "{body}: {v}");
    }
    let (_, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/web",
        Value::Null,
    )
    .await;
    assert_eq!(v["fetch"]["url_rule"], "sourced");
}

#[tokio::test]
async fn the_token_is_write_only() {
    let p = people().await;
    let ts = &p.ts;
    let owner = Who::OwnerBrowser;
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web/token",
        json!({"token": CF_TOKEN}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["cloudflare"]["token"]["set"], true);
    assert_eq!(v["cloudflare"]["token"]["last4"], "wxyz");
    assert_eq!(
        v["cloudflare"]["token"]["host"],
        "https://api.cloudflare.com"
    );
    let (_, v) = call(ts, &owner, Method::GET, "/api/admin/web", Value::Null).await;
    assert_eq!(v["cloudflare"]["token"]["last4"], "wxyz");
    assert!(!v.to_string().contains("TOPSECRET"), "{v}");
    for e in ts.store.events_after(0, 10_000).unwrap() {
        assert!(!e.body.contains("TOPSECRET"), "{}", e.body);
    }
    let keys = events(ts, "admin.key");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].1["scope"], "cloudflare");
    assert_eq!(keys[0].1["op"], "set");
    let (s, v) = call(
        ts,
        &owner,
        Method::DELETE,
        "/api/admin/web/token",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["cloudflare"]["token"]["set"], false);
    let keys = events(ts, "admin.key");
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[1].1["op"], "clear");
    let (s, v) = call(
        ts,
        &Who::Agent,
        Method::PUT,
        "/api/admin/web/token",
        json!({"token": CF_TOKEN}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    for (m, path, body) in [
        (Method::PUT, "/api/admin/web", json!({})),
        (Method::DELETE, "/api/admin/web/token", Value::Null),
    ] {
        let (s, v) = call(ts, &Who::Agent, m.clone(), path, body).await;
        assert_eq!((s, code(&v)), (403, "not_for_agents"), "{m} {path}: {v}");
    }
}

#[tokio::test]
async fn web_is_configured_once_the_account_and_token_are_set() {
    let p = people().await;
    let ts = &p.ts;
    let owner = Who::OwnerBrowser;
    let sam = Who::Person(p.sam.0.clone());
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"search": {"enabled": true},
               "cloudflare": {"account_id": "0123456789abcdef0123456789abcdef"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["configured"], false, "an account ID without a token");
    let (_, me) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(me["web"]["why_off"], "not_configured");
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web/token",
        json!({"token": CF_TOKEN}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // Taken at once, without a restart.
    assert_eq!(v["configured"], true, "{v}");
    let (_, me) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(
        (
            &me["web"]["on"],
            &me["web"]["offered"],
            &me["web"]["why_off"]
        ),
        (&json!(true), &json!(true), &Value::Null)
    );
    let (s, v) = call(
        ts,
        &owner,
        Method::DELETE,
        "/api/admin/web/token",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["configured"], false);
}

#[tokio::test]
async fn a_member_cannot_admin_web() {
    let p = people().await;
    let ts = &p.ts;
    for who in [Who::Person(p.sam.0.clone()), Who::Viewer(p.lan.clone())] {
        for (m, path, body) in [
            (Method::GET, "/api/admin/web", Value::Null),
            (Method::PUT, "/api/admin/web", json!({})),
            (
                Method::PUT,
                "/api/admin/web/token",
                json!({"token": CF_TOKEN}),
            ),
            (Method::DELETE, "/api/admin/web/token", Value::Null),
        ] {
            let (s, v) = call(ts, &who, m.clone(), path, body).await;
            assert_eq!((s, code(&v)), (403, "forbidden"), "{who:?} {m} {path}: {v}");
        }
    }
    let (s, v) = call(ts, &Who::Agent, Method::GET, "/api/admin/web", Value::Null).await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
}

#[tokio::test]
async fn a_member_sees_the_web_prices_of_the_active_provider() {
    let p = people().await;
    let ts = &p.ts;
    let sam = Who::Person(p.sam.0.clone());
    let owner = Who::OwnerBrowser;
    // Fixed charges per thousand requests, so one request is a visible number of milli-credits.
    let (s, mut pr) = call(ts, &owner, Method::GET, "/api/admin/pricing", Value::Null).await;
    assert_eq!(s, 200, "{pr}");
    for l in pr["lines"].as_array_mut().unwrap() {
        let fixed = match l["id"].as_str().unwrap() {
            "web.search.ceramic" => 5_000,
            "web.search.exa" => 70_000,
            "web.fetch" => 9_000,
            _ => continue,
        };
        l["fixed_credits_milli"] = json!(fixed);
    }
    let (s, v) = call(ts, &owner, Method::PUT, "/api/admin/pricing", pr).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["web"]["prices"],
        json!({"search_milli": 5, "fetch_milli": 9})
    );
    let (s, v) = call(
        ts,
        &owner,
        Method::PUT,
        "/api/admin/web",
        json!({"search": {"provider": "exa"}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(
        v["web"]["prices"],
        json!({"search_milli": 70, "fetch_milli": 9})
    );
}

#[tokio::test]
async fn me_turns_web_off() {
    let p = people().await;
    let ts = &p.ts;
    let sam = Who::Person(p.sam.0.clone());
    let (s, v) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["web"]["on"], true);
    assert_eq!(v["web"]["offered"], false);
    assert_eq!(v["web"]["why_off"], "admin_off");
    let (s, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::PUT,
        "/api/admin/web",
        json!({"search": {"enabled": true}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // The host has no source in the test daemon.
    let (_, v) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(v["web"]["why_off"], "not_configured");
    assert_eq!(v["web"]["offered"], false);
    let (s, v) = call(ts, &sam, Method::PUT, "/api/me/bot", json!({"web": false})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["web"]["on"], false);
    let (_, v) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(v["web"]["on"], false);
    assert_eq!(events(ts, "me.web").len(), 1);
    // The choice is the person's own.
    let (_, v) = call(
        ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/me/bot",
        Value::Null,
    )
    .await;
    assert_eq!(v["web"]["on"], true);
    let (s, v) = call(ts, &sam, Method::PUT, "/api/me/bot", json!({"web": true})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["web"]["on"], true);
}

#[tokio::test]
async fn me_put_needs_a_field() {
    let p = people().await;
    let ts = &p.ts;
    let sam = Who::Person(p.sam.0.clone());
    let (s, v) = call(ts, &sam, Method::PUT, "/api/me/bot", json!({})).await;
    assert_eq!((s, code(&v)), (400, "empty_request"), "{v}");
    // Both fields in one request: use_own still needs a key.
    let (s, v) = call(
        ts,
        &sam,
        Method::PUT,
        "/api/me/bot",
        json!({"web": false, "use_own": true}),
    )
    .await;
    assert_eq!(s, 404, "{v}");
    let (_, v) = call(ts, &sam, Method::GET, "/api/me/bot", Value::Null).await;
    assert_eq!(v["web"]["on"], true, "a refused request changes nothing");
}

#[tokio::test]
async fn the_admin_web_view_says_why_web_is_unavailable() {
    // A host whose configured public origin or site domain names no host
    // (`Bot::web_unavailable`) has web off, even with a source, and says why.
    let why = "the site domain \"https://sites.example/x\" is not a bare host name";
    let ts = TestServer::spawn_with(|s| {
        s.bot = std::sync::Arc::new(clax_http::bot::Bot {
            web_configured: true,
            web_unavailable: Some(why.to_string()),
            ..Default::default()
        });
    })
    .await;
    let (s, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/web",
        Value::Null,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["configured"], false, "{v}");
    assert_eq!(v["unavailable"], why, "{v}");
    // Without the reason the same host is configured, and says nothing.
    let ts = TestServer::spawn_with(|s| {
        s.bot = std::sync::Arc::new(clax_http::bot::Bot {
            web_configured: true,
            ..Default::default()
        });
    })
    .await;
    let (_, v) = call(
        &ts,
        &Who::OwnerBrowser,
        Method::GET,
        "/api/admin/web",
        Value::Null,
    )
    .await;
    assert_eq!(v["configured"], true, "{v}");
    assert!(v.get("unavailable").is_none(), "{v}");
}

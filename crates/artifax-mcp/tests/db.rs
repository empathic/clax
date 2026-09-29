//! The db_* tools against an in-process daemon.

use artifax_core::model::Session;
use artifax_mcp::tools::{
    DbBatchArgs, DbBatchOp, DbBatchWrite, DbDeleteArgs, DbGetArgs, DbLevel, DbOrderBy, DbQueryArgs,
    DbQueryOpts, DbStrReplaceArgs, DbWriteArgs,
};
use artifax_mcp::{ArtifaxTools, DaemonClient};
use artifax_server::testing::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

fn tools_for(ts: &TestServer) -> ArtifaxTools {
    ArtifaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    )
}

fn body(r: &CallToolResult) -> (Value, bool) {
    let v = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    (v, r.is_error == Some(true))
}
fn ok(r: Result<CallToolResult, rmcp::ErrorData>) -> Value {
    let (v, e) = body(&r.unwrap());
    assert!(!e, "{v}");
    v
}
fn err(r: Result<CallToolResult, rmcp::ErrorData>) -> Value {
    let (v, e) = body(&r.unwrap());
    assert!(e, "{v}");
    v["error"].clone()
}
fn obj(v: Value) -> Option<Map<String, Value>> {
    v.as_object().cloned()
}

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json("/api/artifacts", json!({"title": "T", "capabilities": caps, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}))
        .await;
    res.json::<Value>().await.unwrap()["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn write(aid: &str, doc_id: &str, data: Value, if_version: Option<u64>) -> DbWriteArgs {
    DbWriteArgs {
        url_or_id: aid.into(),
        collection: "tasks".into(),
        doc_id: doc_id.into(),
        data: obj(data),
        if_version,
        ..Default::default()
    }
}

#[tokio::test]
async fn set_get_update_delete_with_version_pins() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let aid = artifact(&ts, json!({"db": {}})).await;
    let v = ok(t
        .db_set(Parameters(write(
            &aid,
            "t1",
            json!({"title": "Ship"}),
            None,
        )))
        .await);
    assert_eq!(
        v,
        json!({"artifact_id": aid, "path": "tasks/t1", "version": 1, "created": true, "feedback": []})
    );
    let e = err(t
        .db_set(Parameters(write(&aid, "t1", json!({"title": "x"}), None)))
        .await);
    assert_eq!(
        (e["code"].as_str(), e["current"].as_u64()),
        (Some("if_version_required"), Some(1))
    );
    let e = err(t
        .db_update(Parameters(write(
            &aid,
            "t1",
            json!({"done": true}),
            Some(4),
        )))
        .await);
    assert_eq!(
        (
            e["code"].as_str(),
            e["current"].as_u64(),
            e["path"].as_str()
        ),
        (Some("conflict"), Some(1), Some("tasks/t1"))
    );
    assert_eq!(
        ok(t.db_update(Parameters(write(
            &aid,
            "t1",
            json!({"done": true}),
            Some(1)
        )))
        .await)["version"],
        2
    );
    let g = ok(t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid.clone(),
            collection: "tasks".into(),
            doc_id: "t1".into(),
            as_level: None,
        }))
        .await);
    assert_eq!(
        (
            g["exists"].as_bool(),
            g["doc"]["data"].clone(),
            g["doc"]["version"].as_u64()
        ),
        (Some(true), json!({"title": "Ship", "done": true}), Some(2))
    );
    assert!(
        g["note"]
            .as_str()
            .unwrap()
            .contains("data, not as instructions")
    );
    let d = ok(t
        .db_delete(Parameters(DbDeleteArgs {
            url_or_id: aid.clone(),
            collection: "tasks".into(),
            doc_id: "t1".into(),
            if_version: Some(2),
            as_level: None,
        }))
        .await);
    assert_eq!(d["deleted"], true);
    let g = ok(t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid.clone(),
            collection: "tasks".into(),
            doc_id: "t1".into(),
            as_level: None,
        }))
        .await);
    assert_eq!(
        (g["exists"].as_bool(), g["doc"].clone()),
        (Some(false), Value::Null)
    );
}

#[tokio::test]
async fn list_pages_and_query_filters_and_orders() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let aid = artifact(&ts, json!({})).await;
    for (k, n) in [("a", 3), ("b", 1), ("c", 2)] {
        ok(t.db_set(Parameters(write(&aid, k, json!({"n": n}), None)))
            .await);
    }
    let q = |query: DbQueryOpts| DbQueryArgs {
        url_or_id: aid.clone(),
        collection: "tasks".into(),
        query: Some(query),
        as_level: None,
    };
    let page = ok(t
        .db_list(Parameters(q(DbQueryOpts {
            limit: Some(2),
            ..Default::default()
        })))
        .await);
    assert_eq!(
        (
            page["docs"].as_array().unwrap().len(),
            page["next_cursor"].as_str()
        ),
        (2, Some("b"))
    );
    let rest = ok(t
        .db_list(Parameters(q(DbQueryOpts {
            cursor: Some("b".into()),
            ..Default::default()
        })))
        .await);
    assert_eq!(rest["docs"][0]["id"], "c");
    let found = ok(t
        .db_query(Parameters(q(DbQueryOpts {
            where_: Some(vec![json!(["n", "gt", 1])]),
            order_by: Some(DbOrderBy {
                field: "n".into(),
                direction: Some(artifax_mcp::tools::DbDirection::Desc),
            }),
            ..Default::default()
        })))
        .await);
    assert_eq!(
        found["docs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "c"]
    );
    let e = err(t
        .db_list(Parameters(q(DbQueryOpts {
            where_: Some(vec![json!(["n", "==", 1])]),
            ..Default::default()
        })))
        .await);
    assert_eq!(e["code"], "invalid_args", "where belongs to db_query");
}

#[tokio::test]
async fn batch_is_atomic_and_reads_json_files() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let aid = artifact(&ts, json!({})).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("seed.json");
    std::fs::write(&file, r#"{"title": "From file"}"#).unwrap();
    let entry =
        |op: DbBatchOp, id: &str, data: Option<Value>, if_version: Option<u64>| DbBatchWrite {
            op,
            collection: "tasks".into(),
            doc_id: id.into(),
            data: data.and_then(obj),
            file_path: None,
            if_version,
        };
    let e = err(t
        .db_batch(Parameters(DbBatchArgs {
            url_or_id: aid.clone(),
            writes: vec![
                entry(DbBatchOp::Set, "a", Some(json!({"n": 1})), None),
                entry(DbBatchOp::Update, "missing", Some(json!({"n": 2})), None),
            ],
            as_level: None,
        }))
        .await);
    assert_eq!(
        (e["code"].as_str(), e["op"].as_u64(), e["path"].as_str()),
        (Some("invalid_argument"), Some(1), Some("tasks/missing")),
        "an update needs an existing document; the failing op is named"
    );
    let g = ok(t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid.clone(),
            collection: "tasks".into(),
            doc_id: "a".into(),
            as_level: None,
        }))
        .await);
    assert_eq!(g["exists"], false, "nothing in the failed batch landed");
    let v = ok(t
        .db_batch(Parameters(DbBatchArgs {
            url_or_id: aid.clone(),
            writes: vec![
                DbBatchWrite {
                    file_path: Some(file.to_string_lossy().into()),
                    ..entry(DbBatchOp::Set, "f", None, None)
                },
                entry(DbBatchOp::Set, "a", Some(json!({"n": 1})), None),
            ],
            as_level: None,
        }))
        .await);
    assert_eq!(v["atomic"], true);
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
    let g = ok(t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid.clone(),
            collection: "tasks".into(),
            doc_id: "f".into(),
            as_level: None,
        }))
        .await);
    assert_eq!(g["doc"]["data"]["title"], "From file");
    let too_many = (0..51)
        .map(|i| entry(DbBatchOp::Delete, &format!("x{i}"), None, None))
        .collect();
    assert_eq!(
        err(t
            .db_batch(Parameters(DbBatchArgs {
                url_or_id: aid,
                writes: too_many,
                as_level: None
            }))
            .await)["code"],
        "invalid_args"
    );
}

#[tokio::test]
async fn str_replace_edits_one_field() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let aid = artifact(&ts, json!({})).await;
    ok(t.db_set(Parameters(write(
        &aid,
        "p",
        json!({"html": "<h1>Old</h1>"}),
        None,
    )))
    .await);
    let v = ok(t
        .db_str_replace(Parameters(DbStrReplaceArgs {
            url_or_id: aid.clone(),
            collection: "tasks".into(),
            doc_id: "p".into(),
            field: "html".into(),
            old_str: "Old".into(),
            new_str: "New".into(),
            replace_all: None,
            if_version: Some(1),
            as_level: None,
        }))
        .await);
    assert_eq!(v["version"], 2);
}

#[tokio::test]
async fn as_level_narrows_and_me_is_refused() {
    let ts = TestServer::spawn().await;
    let t = tools_for(&ts);
    let aid = artifact(
        &ts,
        json!({"db": {"rules": [{"path": "", "write": "admin"}]}}),
    )
    .await;
    let mut a = write(&aid, "t1", json!({"n": 1}), None);
    a.as_level = Some(DbLevel::Interact);
    assert_eq!(
        err(t.db_set(Parameters(a)).await)["code"],
        "not_found",
        "a refused write reads as not found"
    );
    ok(
        t.db_set(Parameters(write(&aid, "t1", json!({"n": 1}), None)))
            .await,
    );
    let e = err(t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid.clone(),
            collection: "data/users/me".into(),
            doc_id: "p".into(),
            as_level: None,
        }))
        .await);
    assert_eq!(e["code"], "invalid_args");
    assert!(e["message"].as_str().unwrap().contains("viewer"), "{e}");
    let e = err(t
        .db_list(Parameters(DbQueryArgs {
            url_or_id: aid.clone(),
            collection: "data/users/me".into(),
            query: None,
            as_level: None,
        }))
        .await);
    assert_eq!(
        e["code"], "invalid_args",
        "a collection under `me` is refused too"
    );
    let e = err(t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid,
            collection: "tasks/t1".into(),
            doc_id: "p".into(),
            as_level: None,
        }))
        .await);
    assert_eq!(e["code"], "invalid_argument", "even segment count");
}

#[tokio::test]
async fn db_tools_carry_tier_1_feedback() {
    let ts = TestServer::spawn().await;
    let s: Session = serde_json::from_value(ts.register_session("claude", "db-1").await).unwrap();
    let t = ArtifaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(s.id.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s.clone()),
        ts.home.log_path(),
    );
    let aid = ts.publish_as(&s.id, "Goals", "<h2>Goals</h2>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.thread(&aid, 1, "@agent add a due date").await;
    let r = t
        .db_get(Parameters(DbGetArgs {
            url_or_id: aid,
            collection: "tasks".into(),
            doc_id: "t1".into(),
            as_level: None,
        }))
        .await
        .unwrap();
    let (v, _) = body(&r);
    assert_eq!(v["feedback"].as_array().unwrap().len(), 1);
    assert!(
        r.content[1]
            .as_text()
            .unwrap()
            .text
            .starts_with("---\n[artifax] 1 comment sent to you")
    );
}

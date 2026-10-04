//! Query-plan checks for the hot read paths: each query is planned against a
//! seeded database, with and without planner statistics, and must use its
//! expected indexes and never scan `comments` or `threads` in full.

use super::Store;
use super::attention::{
    AGENTS_LIVE, AGENTS_ONE, ATTENTION_LIVE, ATTENTION_ONE, LOOKED_ONE, PEOPLE_LIVE, PEOPLE_ONE,
};
use super::feedback::{FEEDBACK_STATES, TAKE_FEEDBACK};
use super::test_util::store;
use super::threads::{
    ADDRESSED_IN_MANY, COMMENTS_OF_MANY, NAMES_OF_MANY, SENDS_OF_MANY, THREAD_STATUSES,
    list_threads_sql,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params};

const ARTIFACTS: usize = 60;
const THREADS: usize = 8;
const COMMENTS: usize = 3;

/// One viewer writes every comment (the skew that once made the planner
/// prefer `comments_by_author`), a second one is mentioned now and then.
fn seed(c: &Connection) {
    c.execute_batch("BEGIN").unwrap();
    let ts = |i: usize| format!("2026-01-01T00:00:{:02}.{:03}Z", i / 1000 % 60, i % 1000);
    c.execute(
        "INSERT INTO viewers (id, display_name, created_at, public_id) VALUES
         ('v1', 'Ana', ?1, 'u_00000000000000000000001'), ('v2', NULL, ?1, 'u_00000000000000000000002')",
        params![ts(0)],
    )
    .unwrap();
    for s in 0..20 {
        c.execute(
            "INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, agent_handle)
             VALUES (?1, 'claude', '/w', ?2, ?2, ?3)",
            params![format!("s{s}"), ts(s), format!("a_{s}")],
        )
        .unwrap();
    }
    let mut n = 0;
    for a in 0..ARTIFACTS {
        let aid = format!("art{a:04}");
        let sid = format!("s{}", a % 20);
        c.execute(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, owner_session_id, contract_version)
             VALUES (?1, 'T', ?2, ?2, 1, ?3, '1')",
            params![aid, ts(a), sid],
        )
        .unwrap();
        c.execute(
            "INSERT INTO versions (artifact_id, n, created_at, session_id, files_json) VALUES (?1, 1, ?2, ?3, '{}')",
            params![aid, ts(a), sid],
        )
        .unwrap();
        c.execute(
            "INSERT INTO watches (session_id, artifact_id, created_at) VALUES (?1, ?2, ?3)",
            params![sid, aid, ts(a)],
        )
        .unwrap();
        c.execute(
            "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at) VALUES ('v1', ?1, 1, ?2)",
            params![aid, ts(a)],
        )
        .unwrap();
        for t in 0..THREADS {
            let tid = format!("{aid}t{t}");
            n += 1;
            c.execute(
                "INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at) VALUES (?1, ?2, 1, '{}', ?3)",
                params![tid, aid, ts(n)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                 VALUES (?1, 1, ?2, 'explicit', ?3)",
                params![aid, tid, ts(n)],
            )
            .unwrap();
            c.execute(
                "INSERT INTO viewer_threads (viewer_id, thread_id, looked_at) VALUES ('v1', ?1, ?2)",
                params![tid, ts(n)],
            )
            .unwrap();
            for k in 0..COMMENTS {
                let cid = format!("{tid}c{k}");
                n += 1;
                c.execute(
                    "INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, body, created_at)
                     VALUES (?1, ?2, 'viewer', 'Ana', 'u_00000000000000000000001', 'x', ?3)",
                    params![cid, tid, ts(n)],
                )
                .unwrap();
                if k == 0 {
                    c.execute(
                        "INSERT INTO mentions (comment_id, public_id) VALUES (?1, 'u_00000000000000000000002')",
                        params![cid],
                    )
                    .unwrap();
                    c.execute(
                        "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![format!("f{cid}"), tid, cid, sid, ts(n)],
                    )
                    .unwrap();
                }
            }
        }
    }
    c.execute_batch("COMMIT").unwrap();
}

type Hot = (&'static str, String, Vec<Value>, &'static [&'static str]);

/// Each hot query, realistic parameters, and the indexes its plan must name.
/// Named parameters bind by name; the rest by position.
fn hot_queries() -> Vec<Hot> {
    let t = |s: &str| Value::Text(s.into());
    let aid = t("art0001");
    let pid = t("u_00000000000000000000001");
    let tids = t(r#"["art0001t0","art0001t1"]"#);
    let one = vec![
        t(":aid"),
        aid.clone(),
        t(":viewer"),
        t("v1"),
        t(":pid"),
        pid.clone(),
    ];
    let live = vec![t(":viewer"), t("v1"), t(":pid"), pid.clone()];
    let named = |v: Vec<Value>| v;
    vec![
        (
            "attention of one artifact",
            ATTENTION_ONE.into(),
            named(one),
            &[
                "threads_by_artifact",
                "comments_by_thread",
                "version_threads_by_thread",
            ],
        ),
        (
            "attention of every artifact",
            ATTENTION_LIVE.into(),
            named(live),
            &[
                "threads_by_artifact",
                "comments_by_thread",
                "version_threads_by_thread",
            ],
        ),
        (
            "looked-at marks",
            LOOKED_ONE.into(),
            vec![t(":viewer"), t("v1"), t(":aid"), aid.clone()],
            &["threads_by_artifact", "sqlite_autoindex_viewer_threads_1"],
        ),
        (
            "people of one artifact",
            PEOPLE_ONE.into(),
            vec![t(":aid"), aid.clone()],
            &[
                "threads_by_artifact",
                "comments_by_thread",
                "viewers_public_id",
            ],
        ),
        (
            "people of every artifact",
            PEOPLE_LIVE.into(),
            vec![],
            &[
                "threads_by_artifact",
                "comments_by_thread",
                "viewers_public_id",
            ],
        ),
        (
            "agents of one artifact",
            AGENTS_ONE.into(),
            vec![t(":aid"), aid.clone(), t(":limit"), Value::Integer(10)],
            &[
                "watches_by_artifact",
                "threads_by_artifact",
                "comments_by_thread",
            ],
        ),
        (
            "agents of every artifact",
            AGENTS_LIVE.into(),
            vec![t(":limit"), Value::Integer(10)],
            &[
                "watches_by_artifact",
                "threads_by_artifact",
                "comments_by_thread",
            ],
        ),
        (
            "thread list",
            list_threads_sql(),
            vec![
                aid.clone(),
                Value::Integer(1),
                Value::Null,
                Value::Integer(51),
            ],
            &["threads_by_artifact"],
        ),
        (
            "comments of threads",
            COMMENTS_OF_MANY.into(),
            vec![tids.clone()],
            &["comments_by_thread"],
        ),
        (
            "feedback states",
            FEEDBACK_STATES.into(),
            vec![tids.clone()],
            &["feedback_by_thread"],
        ),
        (
            "addressed versions",
            ADDRESSED_IN_MANY.into(),
            vec![tids.clone()],
            &["version_threads_by_thread"],
        ),
        (
            "thread sends",
            SENDS_OF_MANY.into(),
            vec![tids.clone()],
            &["batch_threads_by_thread"],
        ),
        (
            "resolver names",
            NAMES_OF_MANY.into(),
            vec![t(r#"["u_00000000000000000000001"]"#)],
            &["viewers_public_id"],
        ),
        (
            "thread statuses",
            THREAD_STATUSES.into(),
            vec![aid.clone(), tids],
            &["sqlite_autoindex_threads_1"],
        ),
        (
            "feedback polling",
            TAKE_FEEDBACK.into(),
            vec![
                t("s1"),
                Value::Null,
                Value::Integer(0),
                Value::Integer(1),
                Value::Integer(3),
                t("2026-01-01T00:00:00.000Z"),
                Value::Integer(0),
            ],
            &["feedback_by_target"],
        ),
    ]
}

/// Aliases and names the hot queries give `comments` and `threads`.
const BIG_TABLES: &[&str] = &["c", "r", "t", "comments", "threads"];

fn plan(c: &Connection, sql: &str, args: &[Value]) -> Vec<String> {
    let mut st = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let named = matches!(args.first(), Some(Value::Text(s)) if s.starts_with(':'));
    if named {
        for pair in args.chunks(2) {
            let Value::Text(name) = &pair[0] else {
                unreachable!()
            };
            let i = st
                .parameter_index(name)
                .unwrap()
                .expect("a parameter of the query");
            st.raw_bind_parameter(i, &pair[1]).unwrap();
        }
    } else {
        for (i, v) in args.iter().enumerate() {
            st.raw_bind_parameter(i + 1, v).unwrap();
        }
    }
    let mut rows = st.raw_query();
    let mut out = Vec::new();
    while let Some(r) = rows.next().unwrap() {
        out.push(r.get::<_, String>(3).unwrap());
    }
    out
}

fn check_all(c: &Connection, stats: &str) {
    for (name, sql, args, indexes) in hot_queries() {
        let plan = plan(c, &sql, &args);
        let text = plan.join("\n");
        for ix in indexes {
            assert!(
                plan.iter().any(|d| d.split_whitespace().any(|w| w == *ix)),
                "{name} ({stats}) does not use {ix}:\n{text}"
            );
        }
        for d in &plan {
            if let Some(rest) = d.strip_prefix("SCAN ") {
                let target = rest.split_whitespace().next().unwrap_or("");
                assert!(
                    !BIG_TABLES.contains(&target),
                    "{name} ({stats}) scans {target} in full:\n{text}"
                );
            }
        }
        assert!(
            !text.contains("comments_by_author") && !text.contains("mentions_by_viewer"),
            "{name} ({stats}) reaches comments by author or mention:\n{text}"
        );
    }
}

#[test]
fn hot_queries_use_their_indexes_with_and_without_statistics() {
    let (_d, st): (_, Store) = store();
    st.with_write(|c| {
        seed(c);
        let has_stat1: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'sqlite_stat1')",
            [],
            |r| r.get(0),
        )?;
        if has_stat1 {
            c.execute_batch("DELETE FROM sqlite_stat1; ANALYZE sqlite_schema;")?;
        }
        check_all(c, "no statistics");
        c.execute_batch("ANALYZE")?;
        check_all(c, "after ANALYZE");
        Ok(())
    })
    .unwrap();
}

#[test]
fn optimize_runs_on_a_seeded_store() {
    let (_d, st) = store();
    st.with_write(|c| {
        seed(c);
        Ok(())
    })
    .unwrap();
    st.optimize().unwrap();
}

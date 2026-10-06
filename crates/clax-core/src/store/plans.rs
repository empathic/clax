//! Query-plan checks for the hot read paths: each query is planned against a
//! seeded database, with and without planner statistics, and must use its
//! expected indexes and never scan `comments`, `threads` or `audit_events`
//! in full. The question queries also never sort in a temporary B-tree, and
//! no inbox query scans `inbox_items` in full.

use super::Store;
use super::attention::{
    AGENTS_LIVE, AGENTS_ONE, ATTENTION_LIVE, ATTENTION_ONE, LOOKED_ONE, PEOPLE_LIVE, PEOPLE_ONE,
};
use super::audit::{EVENTS_AFTER, EVENTS_FOR_CALL, NEWEST_SEQ};
use super::feedback::{FEEDBACK_STATES, TAKE_FEEDBACK};
use super::inbox as ib;
use super::live::{PAGES_OF_ORIGIN, PENDING_OF, SCOPES_OF_ORIGIN, THREAD_PATHS_OF_PAGE};
use super::questions as q;
use super::site::{
    LINKS_OF_THREAD, PENDING_AT_PATH, PENDING_TO, PICKS_LEFT, PICKS_TO, REFILE_CANDIDATES,
    RULES_OF_ORIGIN, SITE_PAGES, TARGETS_TO, TO_UNMERGE, WATCHES_TO,
};
use super::test_util::store;
use super::threads::{
    ADDRESSED_IN_MANY, COMMENTS_OF_MANY, LIVE_PATHS_OF_MANY, MOVES_OF_MANY, NAMES_OF_MANY,
    PENDING_OF_MANY, SENDS_OF_MANY, THREAD_STATUSES, list_threads_sql, threads_of_many_sql,
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
    // Live pages on a few origins: scope watches from every session, and a
    // pending address on the first thread of every other artifact.
    for s in 0..20 {
        for o in 0..5 {
            c.execute(
                "INSERT INTO live_watches (session_id, origin, path, created_at)
                 VALUES (?1, ?2, '/', ?3)",
                params![format!("s{s}"), format!("https://o{o}.test"), ts(s)],
            )
            .unwrap();
        }
    }
    // Every artifact a live page on one of the origins, each origin with
    // rules, and a move for the first thread of every third artifact.
    for a in 0..ARTIFACTS {
        let aid = format!("art{a:04}");
        c.execute(
            "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![aid, format!("https://o{}.test", a % 5), format!("/p{a}"), ts(a)],
        )
        .unwrap();
        c.execute(
            "INSERT INTO live_picks (artifact_id, pick_id, thread_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![aid, format!("p{a}"), format!("{aid}t0"), ts(a)],
        )
        .unwrap();
        if a % 3 == 0 {
            c.execute(
                "INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url, to_artifact_id,
                    to_url, moved_by, kind, rule_id, created_at)
                 VALUES (?1, ?2, ?3, 'u', ?3, 'u', 'viewer:x', 'merge', ?5, ?4)",
                params![
                    format!("m{a}"),
                    format!("{aid}t0"),
                    aid,
                    ts(a),
                    format!("r{}0", a % 5)
                ],
            )
            .unwrap();
            c.execute(
                "UPDATE threads SET live_path = '/x' WHERE id = ?1",
                params![format!("{aid}t1")],
            )
            .unwrap();
        }
    }
    for o in 0..5 {
        for r in 0..4 {
            c.execute(
                "INSERT INTO live_rules (id, origin, pattern, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    format!("r{o}{r}"),
                    format!("https://o{o}.test"),
                    format!("/r{r}/:id"),
                    ts(r)
                ],
            )
            .unwrap();
        }
    }
    for a in (0..ARTIFACTS).step_by(2) {
        let aid = format!("art{a:04}");
        c.execute(
            "INSERT INTO live_pending (artifact_id, thread_id, source, harness, created_at)
             VALUES (?1, ?2, 'resolve', 'claude', ?3)",
            params![aid, format!("{aid}t0"), ts(a)],
        )
        .unwrap();
    }
    seed_questions(c, &ts);
    seed_inbox(c, &ts);
    // Audit events: a few per thread, every other one under a tool call.
    for e in 0..ARTIFACTS * THREADS * 4 {
        let a = e % ARTIFACTS;
        c.execute(
            "INSERT INTO audit_events (at, kind, actor, artifact_id, thread_id, session_id, call_id, body)
             VALUES (?1, 'comment.add', '{}', ?2, ?3, ?4, ?5, '{}')",
            params![
                ts(e),
                format!("art{a:04}"),
                format!("art{a:04}t{}", e % THREADS),
                format!("s{}", a % 20),
                (e % 2 == 0).then(|| format!("call{}", e / 4))
            ],
        )
        .unwrap();
    }
    c.execute_batch("COMMIT").unwrap();
}

const ITEMS: usize = 6000;

/// Inbox history: replies, versions and published items across 40
/// artifacts and two harnesses, a quarter unread, each with an index entry
/// (one word common to all, one to a tenth of them).
fn seed_inbox(c: &Connection, ts: &dyn Fn(usize) -> String) {
    let kinds = ["reply", "reply", "version", "published"];
    for i in 0..ITEMS {
        let kind = kinds[i % 4];
        let aid = format!("art{:04}", i % 40);
        let tid = (kind == "reply").then(|| format!("{aid}t{}", i % THREADS));
        let sid = format!("s{}", i % 20);
        c.execute(
            "INSERT INTO inbox_items (seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
                session_id, harness, created_at, read_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                i as i64 + 1,
                format!("I{i:05}"),
                kind,
                format!("{kind}:{i}"),
                aid,
                tid,
                (kind == "reply").then(|| format!("c{i}")),
                (kind != "reply").then_some(2 + i as i64 / 100),
                sid,
                if i % 2 == 0 { "claude" } else { "pi" },
                ts(i),
                (i % 4 != 0).then(|| ts(i + 1)),
            ],
        )
        .unwrap();
        let rare = if i % 10 == 0 { " blue header" } else { "" };
        c.execute(
            "INSERT INTO inbox_fts (rowid, text) VALUES (?1, ?2)",
            params![
                i as i64 + 1,
                format!("Quarterly Review claude done item{i}{rare}")
            ],
        )
        .unwrap();
    }
    for (n, q) in (0..QUESTIONS).step_by(4).enumerate() {
        c.execute(
            "INSERT INTO inbox_items (id, kind, key, question_id, session_id, harness, created_at)
             VALUES (?1, 'question', ?2, ?3, 's1', 'claude', ?4)",
            params![
                format!("Q{n:05}"),
                format!("question:Q{q:05}"),
                format!("Q{q:05}"),
                ts(ITEMS + n)
            ],
        )
        .unwrap();
    }
}

/// Each fixed inbox statement, its parameters, and the indexes its plan must name.
fn inbox_queries() -> Vec<Hot> {
    let t = |s: &str| Value::Text(s.into());
    let i = Value::Integer;
    vec![
        (
            "inbox unread count",
            ib::UNREAD_COUNT.into(),
            vec![],
            &["inbox_unread"],
        ),
        (
            "inbox item by ID",
            ib::BY_ID.into(),
            vec![t("I00040")],
            &["sqlite_autoindex_inbox_items_1"],
        ),
        (
            "inbox items by seq",
            ib::BY_SEQS.into(),
            vec![t("[1, 40, 7000]")],
            &[],
        ),
        (
            "inbox mark read",
            ib::MARK_READ.into(),
            vec![t("[\"I00040\"]"), t("x")],
            &["sqlite_autoindex_inbox_items_1"],
        ),
        (
            "inbox mark unread",
            ib::MARK_UNREAD.into(),
            vec![t("[\"I00040\"]")],
            &["sqlite_autoindex_inbox_items_1"],
        ),
        (
            "inbox read by look",
            ib::READ_BY_LOOK.into(),
            vec![t("art0001t1"), t("art0001"), t("2026-01-01T00:00:00.000Z")],
            &["inbox_by_thread"],
        ),
        (
            "inbox read by seen",
            ib::READ_BY_SEEN.into(),
            vec![t("art0001"), i(3), t("2026-01-01T00:00:00.000Z")],
            &["inbox_by_artifact"],
        ),
        (
            "inbox newest stamp",
            ib::NEWEST_STAMP.into(),
            vec![],
            &["inbox_by_created"],
        ),
        (
            "inbox item of a question",
            ib::OF_QUESTION.into(),
            vec![t("Q00040")],
            &["inbox_by_question"],
        ),
        (
            "inbox thread moved",
            ib::THREAD_MOVED.into(),
            vec![t("art0001t1"), t("art0002")],
            &["inbox_by_thread"],
        ),
    ]
}

/// The participation checks an item's creation runs, which must reach
/// comments by thread only.
fn inbox_participation() -> Vec<Hot> {
    let t = |s: &str| Value::Text(s.into());
    vec![
        (
            "inbox owner in a thread",
            ib::OWNER_IN.into(),
            vec![t("u_00000000000000000000001"), t("art0001t1")],
            &["comments_by_thread"],
        ),
        (
            "inbox owner commented on an artifact",
            ib::OWNER_COMMENTED.into(),
            vec![t("art0001"), t("u_00000000000000000000001")],
            &["threads_by_artifact", "comments_by_thread"],
        ),
        (
            "inbox addressed quotes",
            ib::ADDRESSED_QUOTES.into(),
            vec![t("art0001"), Value::Integer(1)],
            &["sqlite_autoindex_version_threads_1"],
        ),
    ]
}

const INBOX_INDEXES: &[&str] = &[
    "inbox_unread",
    "inbox_by_artifact",
    "inbox_by_kind",
    "inbox_by_harness",
    "inbox_by_session",
    "inbox_by_created",
];

/// No inbox statement scans `inbox_items` (alias `i` or not) in full: each
/// walks an index, the rowid, or the FTS index.
fn check_inbox(c: &Connection, stats: &str) {
    let mut all: Vec<(String, String, Vec<Value>, &[&str])> = inbox_queries()
        .into_iter()
        .map(|(n, s, a, ix)| (n.to_string(), s, a, ix))
        .collect();
    let mut drivers = std::collections::HashMap::new();
    for (n, b) in ib::shapes() {
        drivers.insert(n.clone(), b.driver);
        all.push((n, b.sql, b.args, &[][..]));
    }
    for (name, sql, args, indexes) in all {
        let plan = plan(c, &sql, &args);
        let text = plan.join("\n");
        if std::env::var_os("CLAX_PRINT_PLANS").is_some() {
            eprintln!("{name} ({stats}):\n{text}\n");
        }
        for ix in indexes {
            assert!(
                plan.iter().any(|d| d.split_whitespace().any(|w| w == *ix)),
                "{name} ({stats}) does not use {ix}:\n{text}"
            );
        }
        if let Some(d) = drivers.get(&name) {
            // A page, count or mark-all never sorts (items by seq sorts only the seqs given).
            assert!(
                !text.contains("TEMP B-TREE"),
                "{name} ({stats}) sorts:\n{text}"
            );
            // Driven by its one index, whatever the statistics say.
            assert!(
                text.contains(d.plan_word()),
                "{name} ({stats}) is not driven by {d:?}:\n{text}"
            );
            for ix in INBOX_INDEXES {
                assert!(
                    *ix == d.plan_word() || !text.contains(ix),
                    "{name} ({stats}) uses {ix}, not {d:?}:\n{text}"
                );
            }
            assert!(
                *d != ib::Driver::Dates
                    || text.contains("SEARCH i USING INTEGER PRIMARY KEY (rowid"),
                "{name} ({stats}) does not walk the dates' seq range:\n{text}"
            );
        }
        for d in &plan {
            let Some(rest) = d.strip_prefix("SCAN ") else {
                continue;
            };
            let target = rest.split_whitespace().next().unwrap_or("");
            // The partial index holds only the unread items.
            assert!(
                !["i", "inbox_items"].contains(&target) || rest.ends_with("INDEX inbox_unread"),
                "{name} ({stats}) scans inbox_items:\n{text}"
            );
            if ["i", "inbox_items"].contains(&target) {
                continue;
            }
            // A table function, the FTS index, or a capped count's own subquery.
            assert!(
                rest.contains("VIRTUAL TABLE") || target.starts_with("(subquery-"),
                "{name} ({stats}) scans {target}:\n{text}"
            );
        }
    }
}

const QUESTIONS: usize = 2000;

/// Mostly closed history, a few open, a third mirrored from hooks.
fn seed_questions(c: &Connection, ts: &dyn Fn(usize) -> String) {
    let statuses = ["answered", "declined", "withdrawn", "released", "answered"];
    for i in 0..QUESTIONS {
        let open = i % 40 == 0;
        let status = if open { "open" } else { statuses[i % 5] };
        let hook = i % 3 == 0;
        c.execute(
            "INSERT INTO questions (id, session_id, artifact_id, source, tool_use_id,
                questions_json, status, created_at, closed_at, taken_at)
             VALUES (?1, ?2, ?3, ?4, ?5, '[]', ?6, ?7, ?8, ?9)",
            params![
                format!("Q{i:05}"),
                format!("s{}", i % 20),
                (i % 2 == 0).then(|| format!("art{:04}", i % ARTIFACTS)),
                if hook { "hook" } else { "ask" },
                hook.then(|| format!("toolu_{i}")),
                status,
                ts(i),
                (!open).then(|| ts(i + 1)),
                (i % 4 != 0 && !open).then(|| ts(i + 2)),
            ],
        )
        .unwrap();
    }
}

/// Each question query, its parameters, and the indexes its plan must name.
/// None sorts in a temporary B-tree; one that walks an index in order
/// (bounded by its `LIMIT`) scans only that index.
fn question_queries() -> Vec<Hot> {
    let t = |s: &str| Value::Text(s.into());
    let i = Value::Integer;
    vec![
        (
            "question by ID",
            q::BY_ID.into(),
            vec![t("Q00040")],
            &["sqlite_autoindex_questions_1"],
        ),
        (
            "question by tool use",
            q::BY_TOOL_USE.into(),
            vec![t("s3"), t("toolu_3")],
            &["questions_by_tool_use"],
        ),
        (
            "open count",
            q::OPEN_COUNT.into(),
            vec![],
            &["questions_by_status"],
        ),
        (
            "open count of a session",
            q::OPEN_COUNT_OF_SESSION.into(),
            vec![t("s0")],
            &["questions_by_session"],
        ),
        (
            "open, oldest first",
            q::OPEN_OLDEST.into(),
            vec![i(50)],
            &["questions_by_status"],
        ),
        (
            "open, newest first",
            q::OPEN_NEWEST.into(),
            vec![i(50)],
            &["questions_by_status"],
        ),
        (
            "closed, newest first",
            q::CLOSED_NEWEST.into(),
            vec![i(50)],
            &["questions_closed"],
        ),
        (
            "closed before a cursor",
            q::CLOSED_BEFORE.into(),
            vec![t("2026-01-01T00:00:01.000Z"), t("Q01000"), i(50)],
            &["questions_closed"],
        ),
        (
            "late answers",
            q::LATE_ANSWERS.into(),
            vec![t("s1")],
            &["questions_by_session"],
        ),
        (
            "open of a session",
            q::OPEN_OF_SESSION.into(),
            vec![t("s1")],
            &["questions_by_session"],
        ),
        (
            "withdraw a session's",
            q::WITHDRAW_SESSION.into(),
            vec![t("s1"), t("2026-01-01T00:00:00.000Z")],
            &["questions_by_session"],
        ),
        (
            "open hook questions",
            q::OPEN_HOOKS.into(),
            vec![],
            &["questions_by_status"],
        ),
        (
            "withdraw hook questions",
            q::WITHDRAW_HOOKS.into(),
            vec![t("2026-01-01T00:00:00.000Z")],
            &["questions_by_status"],
        ),
        (
            "close a question",
            q::CLOSE.into(),
            vec![
                t("Q00040"),
                t("open"),
                t("answered"),
                Value::Null,
                Value::Null,
                t("x"),
            ],
            &["sqlite_autoindex_questions_1"],
        ),
        (
            "take a question",
            q::TAKE.into(),
            vec![t("Q00040"), t("x")],
            &["sqlite_autoindex_questions_1"],
        ),
    ]
}

fn check_questions(c: &Connection, stats: &str) {
    for (name, sql, args, indexes) in question_queries() {
        let plan = plan(c, &sql, &args);
        let text = plan.join("\n");
        for ix in indexes {
            assert!(
                plan.iter().any(|d| d.split_whitespace().any(|w| w == *ix)),
                "{name} ({stats}) does not use {ix}:\n{text}"
            );
        }
        assert!(
            !text.contains("TEMP B-TREE"),
            "{name} ({stats}) sorts:\n{text}"
        );
        for d in &plan {
            if let Some(rest) = d.strip_prefix("SCAN questions") {
                assert!(
                    indexes
                        .iter()
                        .any(|ix| rest.ends_with(&format!("INDEX {ix}"))),
                    "{name} ({stats}) scans questions:\n{text}"
                );
            }
        }
    }
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
                "versions_by_session",
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
                "versions_by_session",
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
            "pending addresses of threads",
            PENDING_OF_MANY.into(),
            vec![tids.clone()],
            &["live_pending_by_thread"],
        ),
        (
            "pending address of a thread",
            PENDING_OF.into(),
            vec![t("art0002t0")],
            &["live_pending_by_thread"],
        ),
        (
            "scope watches of an origin",
            SCOPES_OF_ORIGIN.into(),
            vec![t("https://o1.test")],
            &["live_watches_by_origin"],
        ),
        (
            "live pages of a site",
            SITE_PAGES.into(),
            vec![t("https://o1.test")],
            &["sqlite_autoindex_live_pages_2"],
        ),
        (
            "live pages of an origin",
            PAGES_OF_ORIGIN.into(),
            vec![t("https://o1.test")],
            &["sqlite_autoindex_live_pages_2"],
        ),
        (
            "threads of a site's pages",
            threads_of_many_sql(),
            vec![t(r#"["art0001","art0006","art0011"]"#)],
            &["threads_by_artifact"],
        ),
        (
            "threads a rule may take",
            REFILE_CANDIDATES.into(),
            vec![t(r#"["art0001","art0006","art0011"]"#)],
            &["threads_by_artifact"],
        ),
        (
            "threads to move back when a rule goes",
            TO_UNMERGE.into(),
            vec![aid.clone()],
            &["threads_by_artifact"],
        ),
        (
            "pending threads made at a path",
            PENDING_AT_PATH.into(),
            vec![tids.clone(), t("https://o1.test"), t("/p1")],
            &["sqlite_autoindex_threads_1"],
        ),
        (
            "thread paths of a page",
            THREAD_PATHS_OF_PAGE.into(),
            vec![aid.clone()],
            &["threads_by_artifact"],
        ),
        (
            "a moved thread's version links",
            LINKS_OF_THREAD.into(),
            vec![t("art0001t0")],
            &["version_threads_by_thread"],
        ),
        (
            "a moved thread's pending address",
            PENDING_TO.into(),
            vec![t("art0002t0"), aid.clone()],
            &["live_pending_by_thread"],
        ),
        (
            "a moved thread's pick",
            PICKS_TO.into(),
            vec![t("art0001t0"), aid.clone()],
            &["live_picks_by_thread"],
        ),
        (
            "a moved thread's other picks",
            PICKS_LEFT.into(),
            vec![t("art0001t0"), aid.clone()],
            &["live_picks_by_thread"],
        ),
        (
            "a moved thread's page watchers",
            WATCHES_TO.into(),
            vec![aid.clone(), t("art0002"), t("2026-01-01T00:00:00.000Z")],
            &["watches_by_artifact"],
        ),
        (
            "a moved thread's agents",
            TARGETS_TO.into(),
            vec![t("art0001t0"), t("art0002"), t("2026-01-01T00:00:00.000Z")],
            &["feedback_by_thread"],
        ),
        (
            "merge rules of an origin",
            RULES_OF_ORIGIN.into(),
            vec![t("https://o1.test")],
            &["sqlite_autoindex_live_rules_2"],
        ),
        (
            "live paths of threads",
            LIVE_PATHS_OF_MANY.into(),
            vec![tids.clone()],
            &["sqlite_autoindex_threads_1"],
        ),
        (
            "moves of threads",
            MOVES_OF_MANY.into(),
            vec![tids.clone()],
            &["thread_moves_by_thread"],
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
            "audit events after a seq",
            EVENTS_AFTER.into(),
            vec![Value::Integer(1000), Value::Integer(512)],
            &["PRIMARY"],
        ),
        (
            "audit events of a tool call",
            EVENTS_FOR_CALL.into(),
            vec![t("call7")],
            &["audit_events_call"],
        ),
        ("newest audit seq", NEWEST_SEQ.into(), vec![], &[]),
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

/// Aliases and names the hot queries give `comments`, `threads` and
/// `audit_events`, none of which a hot query may scan in full.
const BIG_TABLES: &[&str] = &["c", "r", "t", "comments", "threads", "audit_events"];

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
    for (name, sql, args, indexes) in hot_queries().into_iter().chain(inbox_participation()) {
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
        if GALLERY_QUERIES.iter().any(|g| name.starts_with(g)) {
            for d in &plan {
                for ix in [
                    "comments_by_thread",
                    "version_threads_by_thread",
                    "versions_by_session",
                ] {
                    assert!(
                        !d.split_whitespace().any(|w| w == ix)
                            || d.contains(&format!("COVERING INDEX {ix}")),
                        "{name} ({stats}) reads a table row per {ix} entry:\n{text}"
                    );
                }
            }
        }
    }
}

/// The gallery's whole-home queries, by name prefix: they read comments and
/// version links through covering indexes, never a row per entry.
const GALLERY_QUERIES: &[&str] = &["attention of", "people of", "agents of"];

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
        check_questions(c, "no statistics");
        check_inbox(c, "no statistics");
        c.execute_batch("ANALYZE")?;
        check_all(c, "after ANALYZE");
        check_questions(c, "after ANALYZE");
        check_inbox(c, "after ANALYZE");
        // A home upgraded by migration 22: its tables have statistics, the
        // indexes it rebuilt or added have none yet.
        let stat4: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'sqlite_stat4')",
            [],
            |r| r.get(0),
        )?;
        let rebuilt = "('comments_by_thread', 'version_threads_by_thread', 'versions_by_session')";
        c.execute_batch(&format!(
            "DELETE FROM sqlite_stat1 WHERE idx IN {rebuilt}; ANALYZE sqlite_schema;"
        ))?;
        if stat4 {
            c.execute_batch(&format!(
                "DELETE FROM sqlite_stat4 WHERE idx IN {rebuilt}; ANALYZE sqlite_schema;"
            ))?;
        }
        check_all(c, "rebuilt indexes without statistics");
        check_questions(c, "rebuilt indexes without statistics");
        check_inbox(c, "rebuilt indexes without statistics");
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

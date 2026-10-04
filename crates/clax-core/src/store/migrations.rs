//! Schema versions applied in order on `Store::open`.

pub const MIGRATIONS: &[&str] = &[
    // 1: phase 1 tables
    "CREATE TABLE artifacts (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        description TEXT,
        icon TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        current_version INTEGER NOT NULL DEFAULT 0,
        owner_session_id TEXT,
        pinned INTEGER NOT NULL DEFAULT 0,
        capabilities_json TEXT NOT NULL DEFAULT '{}',
        contract_version TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE TABLE versions (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        n INTEGER NOT NULL,
        label TEXT,
        created_at TEXT NOT NULL,
        session_id TEXT,
        files_json TEXT NOT NULL,
        PRIMARY KEY (artifact_id, n)
    );
    CREATE TABLE assets (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        content_type TEXT NOT NULL,
        size INTEGER NOT NULL,
        ext TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX assets_by_artifact ON assets(artifact_id);",
    // 2: sessions
    "CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        harness TEXT NOT NULL,
        harness_session_id TEXT,
        cwd TEXT NOT NULL,
        pid INTEGER,
        parent_pid INTEGER,
        started_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL,
        ended_at TEXT
    );
    CREATE UNIQUE INDEX sessions_harness_id ON sessions(harness, harness_session_id)
        WHERE harness_session_id IS NOT NULL AND ended_at IS NULL;",
    // 3: comments and the feedback loop
    "CREATE TABLE watches (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        replies_armed INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, artifact_id)
    );
    CREATE INDEX watches_by_artifact ON watches(artifact_id);
    CREATE TABLE threads (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        version_n INTEGER NOT NULL,
        anchor_json TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
        sent_to_agent INTEGER NOT NULL DEFAULT 0,
        has_clip INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        resolved_at TEXT,
        resolved_by TEXT
    );
    CREATE INDEX threads_by_artifact ON threads(artifact_id, created_at, id);
    CREATE TABLE comments (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        author_kind TEXT NOT NULL CHECK (author_kind IN ('viewer', 'agent')),
        author_name TEXT NOT NULL,
        via_session_id TEXT,
        body TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX comments_by_thread ON comments(thread_id, created_at, id);
    CREATE TABLE feedback (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        comment_id TEXT NOT NULL REFERENCES comments(id),
        target_session_id TEXT,
        created_at TEXT NOT NULL,
        delivered_at TEXT,
        delivery_tier TEXT CHECK (delivery_tier IN
            ('piggyback', 'stop_hook', 'prompt_hook', 'wait', 'queue', 'inject')),
        acknowledged_at TEXT,
        resend_count INTEGER NOT NULL DEFAULT 0,
        last_sent_at TEXT,
        untargeted_at TEXT
    );
    CREATE UNIQUE INDEX feedback_comment_target ON feedback(comment_id, target_session_id)
        WHERE target_session_id IS NOT NULL;
    CREATE INDEX feedback_by_target ON feedback(target_session_id, delivered_at);
    CREATE INDEX feedback_by_thread ON feedback(thread_id, created_at, id);
    CREATE TABLE viewers (
        id TEXT PRIMARY KEY,
        display_name TEXT,
        created_at TEXT NOT NULL
    );
    -- Per-session environment the daemon needs to push to a harness (Codex tier 5).
    CREATE TABLE session_env (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        codex_home TEXT
    );",
    // 4: when `codex queue` failed to take a row (timeout or spawn failure);
    // such a row is left to the in-band tiers and not queued again.
    "ALTER TABLE feedback ADD COLUMN push_failed_at TEXT;",
    // 5: public IDs for viewers. The cookie is the viewer's credential and
    // never leaves the daemon; `resolved_by` names viewers by public ID and
    // agents by harness (`agent:<harness>`), never by session ID. A cookie
    // that resolved a thread without a viewer row gets one, so its thread can
    // be rewritten.
    "INSERT OR IGNORE INTO viewers (id, display_name, created_at)
        SELECT DISTINCT substr(resolved_by, 8), NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        FROM threads WHERE resolved_by LIKE 'viewer:%' AND resolved_by <> 'viewer:anonymous';
    ALTER TABLE viewers ADD COLUMN public_id TEXT;
    UPDATE viewers SET public_id = 'u_' || lower(hex(randomblob(11)));
    CREATE UNIQUE INDEX viewers_public_id ON viewers(public_id);
    UPDATE threads SET resolved_by = 'viewer:' ||
        (SELECT public_id FROM viewers WHERE id = substr(threads.resolved_by, 8))
        WHERE resolved_by LIKE 'viewer:%' AND resolved_by <> 'viewer:anonymous';
    UPDATE threads SET resolved_by = 'agent:' ||
        COALESCE((SELECT harness FROM sessions WHERE id = substr(threads.resolved_by, 7)), 'unknown')
        WHERE resolved_by LIKE 'agent:%';",
    // 6: the last `codex queue` failure for a session (cleared by a success).
    "ALTER TABLE session_env ADD COLUMN push_error TEXT;
    ALTER TABLE session_env ADD COLUMN push_error_at TEXT;",
    // 7: the db capability's documents and leases.
    "CREATE TABLE docs (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        path TEXT NOT NULL,
        collection TEXT NOT NULL,
        json TEXT NOT NULL,
        version INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path)
    );
    CREATE INDEX docs_by_collection ON docs(artifact_id, collection, path);
    CREATE TABLE leases (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        path TEXT NOT NULL,
        holder TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path)
    );",
    // 8: each artifact's document version sequence. A column rather than a
    // table: it lives and dies with the artifact row (soft deletes keep it,
    // hard deletes take it), so no cleanup path has another row to erase.
    // Existing documents seed it with their artifact's highest version.
    "ALTER TABLE artifacts ADD COLUMN doc_seq INTEGER NOT NULL DEFAULT 0;
    UPDATE artifacts SET doc_seq =
        COALESCE((SELECT MAX(version) FROM docs WHERE docs.artifact_id = artifacts.id), 0);",
    // 9: comments a page wrote through the `comments` capability, as the viewer.
    "ALTER TABLE comments ADD COLUMN via_page INTEGER NOT NULL DEFAULT 0;",
    // 10: when `clax feedback follow` announced the row to its target session
    // (a notice, not a delivery); cleared when the row is retargeted.
    "ALTER TABLE feedback ADD COLUMN notified_at TEXT;",
    // 11: the version changelog: each version's note, the threads it
    // addressed, and the latest version each viewer has viewed. No
    // foreign key to `artifacts`: the doctor's hard deletes of broken artifact
    // rows must not trip on them; artifact deletion removes them explicitly.
    "ALTER TABLE versions ADD COLUMN note TEXT;
    CREATE TABLE version_threads (
        artifact_id TEXT NOT NULL,
        version_n INTEGER NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('working', 'explicit', 'resolve')),
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, version_n, thread_id)
    );
    CREATE INDEX version_threads_by_thread ON version_threads(thread_id);
    CREATE TABLE viewer_seen (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        artifact_id TEXT NOT NULL,
        seen_n INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, artifact_id)
    );
    CREATE INDEX viewer_seen_by_age ON viewer_seen(viewer_id, updated_at);",
    // 12: participants. A comment's author (the viewer's public ID), the
    // viewers a comment mentions, each viewer's last look at a thread, and an
    // opaque handle per session so the shell can name and target an agent
    // without a session ID. Existing sessions get a handle; existing comments
    // stay unattributed.
    "ALTER TABLE comments ADD COLUMN author_public_id TEXT;
    CREATE INDEX comments_by_author ON comments(author_public_id);
    CREATE TABLE mentions (
        comment_id TEXT NOT NULL REFERENCES comments(id),
        public_id TEXT NOT NULL,
        PRIMARY KEY (comment_id, public_id)
    );
    CREATE INDEX mentions_by_viewer ON mentions(public_id);
    CREATE TABLE viewer_threads (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        looked_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, thread_id)
    );
    ALTER TABLE sessions ADD COLUMN agent_handle TEXT;
    UPDATE sessions SET agent_handle = 'a_' || lower(hex(randomblob(11)));
    CREATE UNIQUE INDEX sessions_by_handle ON sessions(agent_handle);",
    // 13: batch sends: the batch (its note and who sent it), its threads, and
    // the batch each feedback row came from.
    "CREATE TABLE send_batches (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        note TEXT,
        sent_by TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE batch_threads (
        batch_id TEXT NOT NULL REFERENCES send_batches(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        PRIMARY KEY (batch_id, thread_id)
    );
    CREATE INDEX batch_threads_by_thread ON batch_threads(thread_id);
    ALTER TABLE feedback ADD COLUMN batch_id TEXT;",
    // 14: the session a thread was last sent to with `to`. Later viewer
    // comments on the thread follow it while it is live. Never served.
    "ALTER TABLE threads ADD COLUMN target_session_id TEXT;",
];

#[cfg(test)]
mod tests {
    use super::MIGRATIONS;
    use crate::{Home, Store, is_public_id};
    use rusqlite::{Connection, params};

    const COOKIE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const LOST: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const SID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";

    /// A version-4 database with a named viewer, threads resolved by that
    /// viewer, by a cookie with no viewer row, anonymously, and by an agent.
    fn version_4(home: &Home) {
        home.ensure_dirs().unwrap();
        let c = Connection::open(home.db_path()).unwrap();
        for sql in &MIGRATIONS[..4] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", 4).unwrap();
        c.execute_batch(&format!(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 1, '0');
             INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at) VALUES ('{SID}', 'codex', '/w', 'x', 'x');
             INSERT INTO viewers (id, display_name, created_at) VALUES ('{COOKIE}', 'Alex', 'x');"
        ))
        .unwrap();
        for (tid, by) in [
            ("t1", format!("viewer:{COOKIE}")),
            ("t2", format!("viewer:{LOST}")),
            ("t3", "viewer:anonymous".to_string()),
            ("t4", format!("agent:{SID}")),
        ] {
            c.execute(
                "INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, created_at, resolved_at, resolved_by)
                 VALUES (?1, '7q3k9mzx2b4t', 1, '{}', 'resolved', 'x', 'x', ?2)",
                params![tid, by],
            )
            .unwrap();
        }
    }

    #[test]
    fn migration_5_gives_viewers_public_ids_and_rewrites_resolved_by() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        version_4(&home);
        let st = Store::open(&home).unwrap();
        let alex = st.get_viewer(COOKIE).unwrap().unwrap();
        assert!(is_public_id(&alex.public_id), "{}", alex.public_id);
        let lost = st
            .get_viewer(LOST)
            .unwrap()
            .expect("a row for the cookie that resolved");
        assert!(is_public_id(&lost.public_id));
        assert_ne!(lost.public_id, alex.public_id);
        let by = |tid: &str| {
            st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT resolved_by FROM threads WHERE id = ?1",
                    params![tid],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap()
        };
        assert_eq!(by("t1"), format!("viewer:{}", alex.public_id));
        assert_eq!(by("t2"), format!("viewer:{}", lost.public_id));
        assert_eq!(by("t3"), "viewer:anonymous");
        assert_eq!(by("t4"), "agent:codex");
        let all = st
            .with_read(|c| {
                let mut s = c.prepare("SELECT resolved_by FROM threads")?;
                Ok(s.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap()
            .join(" ");
        for secret in [COOKIE, LOST, SID] {
            assert!(!all.contains(secret), "{all}");
        }
    }

    #[test]
    fn migration_8_starts_each_sequence_above_its_highest_version() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..7] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", 7).unwrap();
            c.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 1, '0');
                 INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                    VALUES ('7q3k9mzx2b4t', 't/a', 't', '{}', 5, 'x'), ('7q3k9mzx2b4t', 't/b', 't', '{}', 2, 'x');",
            )
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let version: u32 = st
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32);
        let id = crate::ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        let admin = crate::db::Caller {
            level: crate::db::Level::Admin,
            viewer: None,
        };
        let pin = crate::store::docs::Pin {
            if_version: None,
            lww: true,
        };
        let w = st
            .doc_set(&id, "t/c", serde_json::json!({}), pin, &admin)
            .unwrap();
        assert_eq!(w.doc.unwrap().version, 6);
    }

    /// The last migration shared with installs that predate the changelog,
    /// participants and batch sends: `feedback.notified_at`.
    const NOTIFIED_AT: usize = 10;

    #[test]
    fn a_database_at_notified_at_upgrades_to_the_latest_schema() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        assert!(MIGRATIONS[NOTIFIED_AT - 1].contains("notified_at"));
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..NOTIFIED_AT] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", NOTIFIED_AT as u32)
                .unwrap();
            c.execute_batch(&format!(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 1, '0');
                 INSERT INTO versions (artifact_id, n, created_at, files_json)
                    VALUES ('7q3k9mzx2b4t', 1, 'x', '[]');
                 INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at)
                    VALUES ('{SID}', 'grok', '/w', 'x', 'x');
                 INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at)
                    VALUES ('t1', '7q3k9mzx2b4t', 1, '{{}}', 'x');
                 INSERT INTO comments (id, thread_id, author_kind, author_name, body, created_at)
                    VALUES ('c1', 't1', 'viewer', 'Alex', 'hi', 'x');
                 INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at, notified_at)
                    VALUES ('f1', 't1', 'c1', '{SID}', 'x', 'y');"
            ))
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let row = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT user_version FROM pragma_user_version),
                            f.notified_at, f.batch_id,
                            (SELECT agent_handle FROM sessions WHERE id = ?1),
                            (SELECT note FROM versions WHERE n = 1),
                            (SELECT author_public_id FROM comments WHERE id = 'c1'),
                            (SELECT target_session_id FROM threads WHERE id = 't1')
                     FROM feedback f WHERE f.id = 'f1'",
                    params![SID],
                    |r| {
                        Ok((
                            r.get::<_, u32>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, Option<String>>(6)?,
                        ))
                    },
                )?)
            })
            .unwrap();
        assert_eq!(row.0, MIGRATIONS.len() as u32);
        assert_eq!(row.1.as_deref(), Some("y"), "notified_at survives");
        assert_eq!(row.2, None);
        let handle = row.3.expect("existing sessions get a handle");
        assert!(handle.starts_with("a_"), "{handle}");
        assert_eq!((row.4, row.5, row.6), (None, None, None));
        for table in [
            "version_threads",
            "viewer_seen",
            "mentions",
            "viewer_threads",
            "send_batches",
            "batch_threads",
        ] {
            let n: i64 = st
                .with_read(|c| {
                    Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }
}

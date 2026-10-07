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
    // 15: the owner identity. One viewer row (`owner = 1`) stands for the
    // person who owns this install: every browser of theirs, the CLI and any
    // other owner credential act as it. `claimed` marks an owner row that
    // holds a browser's identity (one made for the CLI first gives way to the
    // first browser claimed). `minted_local` records whether the daemon minted
    // the viewer's cookie for a request from this machine (1), from elsewhere
    // (0), or before this was recorded (NULL); only a locally minted viewer is
    // ever claimed for the owner, so no existing viewer is.
    "ALTER TABLE viewers ADD COLUMN owner INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE viewers ADD COLUMN claimed INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE viewers ADD COLUMN minted_local INTEGER;
    CREATE UNIQUE INDEX viewers_one_owner ON viewers(owner) WHERE owner = 1;",
    // 16: live pages (spec 2026-10-05-chrome-overlay-design §5.1): the
    // artifact kind, each live page's key, scope watches (and the watches
    // they made), addresses waiting for a live page's next snapshot, and the
    // threads recent picks made (so a retried comment makes no second one).
    "ALTER TABLE artifacts ADD COLUMN kind TEXT NOT NULL DEFAULT 'html'
        CHECK (kind IN ('html', 'live'));
    CREATE TABLE live_pages (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE (origin, path)
    );
    CREATE TABLE live_watches (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        replies_armed INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, origin, path)
    );
    ALTER TABLE watches ADD COLUMN source TEXT NOT NULL DEFAULT 'direct'
        CHECK (source IN ('direct', 'scope'));
    CREATE TABLE live_pending (
        artifact_id TEXT NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('explicit', 'resolve')),
        harness TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, thread_id)
    );
    CREATE INDEX live_pending_by_thread ON live_pending(thread_id);
    CREATE INDEX live_watches_by_origin ON live_watches(origin, path);
    CREATE TABLE live_picks (
        artifact_id TEXT NOT NULL,
        pick_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, pick_id)
    );
    CREATE INDEX live_picks_by_time ON live_picks(created_at);",
    // 17: the Clax Chrome extension's credentials (spec
    // 2026-10-05-chrome-overlay-design §5.3), as their SHA-256 only. A
    // credential names no viewer: the extension acts as the owner identity.
    "CREATE TABLE extension_credentials (
        id TEXT PRIMARY KEY,
        extension_id TEXT NOT NULL,
        secret_sha256 TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        last_used_at TEXT NOT NULL,
        revoked_at TEXT
    );
    CREATE INDEX extension_credentials_by_extension
        ON extension_credentials(extension_id, created_at);",
    // 18: site-wide threads (spec 2026-10-05-chrome-overlay-design §7.1):
    // the path a live page's thread was made at when its page's path differs
    // (a rule mapped the URL, or a merge re-filed the thread), the per-origin
    // rules that map paths to one canonical page (`deleted_at` while a
    // deleted rule's threads are still being moved back), each thread's
    // moves between pages (`kind`: the owner's move, a rule's merge, or a
    // deleted rule's un-merge), and picks by thread (a move carries them).
    "ALTER TABLE threads ADD COLUMN live_path TEXT;
    CREATE TABLE live_rules (
        id TEXT PRIMARY KEY,
        origin TEXT NOT NULL,
        pattern TEXT NOT NULL,
        created_at TEXT NOT NULL,
        deleted_at TEXT,
        UNIQUE (origin, pattern)
    );
    CREATE INDEX live_picks_by_thread ON live_picks(thread_id);
    CREATE TABLE thread_moves (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        from_artifact_id TEXT NOT NULL,
        from_url TEXT NOT NULL,
        to_artifact_id TEXT NOT NULL,
        to_url TEXT NOT NULL,
        moved_by TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('move', 'merge', 'unmerge')),
        rule_id TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX thread_moves_by_thread ON thread_moves(thread_id, created_at, id);",
    // Joined sites (spec 2026-10-05-chrome-overlay-design §7.2). Renumbered
    // when agent-questions (19, 20) and toolpath-audit (21) land — goes
    // last. Each origin the owner joined to a site, with the site's key (the
    // origin whose live pages and rules hold the site's; it has a row of its
    // own), when it joined and when Clax last used it; a joined origin's page
    // merged into the site's page of its path once its threads moved there,
    // kept whole (its artifact and snapshots) but no longer a live page's key;
    // the owner's answers to suggested joins (`never`, or `later` until a
    // time), per pair of origins in order; and `join` as a kind of thread
    // move (a thread re-filed onto the site's page of the same path).
    "CREATE TABLE live_sites (
        origin TEXT PRIMARY KEY,
        site TEXT NOT NULL,
        joined_at TEXT NOT NULL,
        last_used_at TEXT NOT NULL
    );
    CREATE INDEX live_sites_by_site ON live_sites(site, origin);
    CREATE TABLE live_merged_pages (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        merged_into TEXT NOT NULL,
        merged_at TEXT NOT NULL
    );
    CREATE TABLE live_site_answers (
        a TEXT NOT NULL,
        b TEXT NOT NULL,
        answer TEXT NOT NULL CHECK (answer IN ('never', 'later')),
        until TEXT,
        created_at TEXT NOT NULL,
        PRIMARY KEY (a, b),
        CHECK (a < b)
    );
    CREATE TABLE thread_moves_22 (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        from_artifact_id TEXT NOT NULL,
        from_url TEXT NOT NULL,
        to_artifact_id TEXT NOT NULL,
        to_url TEXT NOT NULL,
        moved_by TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('move', 'merge', 'unmerge', 'join')),
        rule_id TEXT,
        created_at TEXT NOT NULL
    );
    INSERT INTO thread_moves_22 SELECT id, thread_id, from_artifact_id, from_url,
        to_artifact_id, to_url, moved_by, kind, rule_id, created_at FROM thread_moves;
    DROP TABLE thread_moves;
    ALTER TABLE thread_moves_22 RENAME TO thread_moves;
    CREATE INDEX thread_moves_by_thread ON thread_moves(thread_id, created_at, id);",
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
    fn migration_15_flags_no_existing_viewer_as_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..14] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", 14).unwrap();
            c.execute_batch(&format!(
                "INSERT INTO viewers (id, public_id, display_name, created_at)
                    VALUES ('{COOKIE}', 'u_00000000000000000000aa', 'Alex', 'x');"
            ))
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        assert_eq!(st.owner().unwrap(), None, "stored data cannot tell");
        assert_eq!(
            st.claim_for_owner(COOKIE).unwrap(),
            crate::store::viewers::Claim::Nothing,
            "a viewer of unknown origin is never claimed"
        );
        let owner = st.owner_viewer(true).unwrap();
        assert_ne!(owner.id, COOKIE);
        assert_eq!(owner.display_name, None);
        // Only one row may be the owner.
        let second = st.with_write(|c| {
            Ok(c.execute(
                &format!("UPDATE viewers SET owner = 1 WHERE id = '{COOKIE}'"),
                [],
            )?)
        });
        assert!(second.is_err());
    }

    #[test]
    fn migration_16_marks_existing_artifacts_html() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let mut c = Connection::open(home.db_path()).unwrap();
            let tx = c.transaction().unwrap();
            for sql in &MIGRATIONS[..15] {
                tx.execute_batch(sql).unwrap();
            }
            tx.pragma_update(None, "user_version", 15).unwrap();
            tx.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                 VALUES ('7q3k9mzx2b4t', 'T', 'x', 'x', 1, '0.2.61')",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let st = Store::open(&home).unwrap();
        let kind: String = st
            .with_read(|c| Ok(c.query_row("SELECT kind FROM artifacts", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(kind, "html");
    }

    #[test]
    fn the_joined_sites_migration_keeps_thread_moves_and_admits_joins() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let mut c = Connection::open(home.db_path()).unwrap();
            let tx = c.transaction().unwrap();
            // Every migration before joined sites, the last.
            let before = MIGRATIONS.len() - 1;
            for sql in &MIGRATIONS[..before] {
                tx.execute_batch(sql).unwrap();
            }
            tx.pragma_update(None, "user_version", before as u32)
                .unwrap();
            tx.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 'T', 'x', 'x', 1, '0.2.61');
                 INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, created_at)
                    VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAV', '7q3k9mzx2b4t', 1, '{}', 'open', 'x');
                 INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url, to_artifact_id,
                    to_url, moved_by, kind, rule_id, created_at)
                    VALUES ('m1', '01ARZ3NDEKTSV4RRFFQ69G5FAV', 'a', 'u', 'b', 'v', 'viewer:x', 'merge', 'r', 'x');",
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let st = Store::open(&home).unwrap();
        st.with_write(|c| {
            let kept: String = c.query_row("SELECT kind FROM thread_moves WHERE id = 'm1'", [], |r| r.get(0))?;
            assert_eq!(kept, "merge");
            c.execute(
                "INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url, to_artifact_id,
                    to_url, moved_by, kind, created_at)
                 VALUES ('m2', '01ARZ3NDEKTSV4RRFFQ69G5FAV', 'a', 'u', 'b', 'v', 'viewer:x', 'join', 'x')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
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

    #[test]
    fn a_database_newer_than_this_binary_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        drop(Store::open(&home).unwrap());
        let newer = MIGRATIONS.len() as u32 + 1;
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.pragma_update(None, "user_version", newer).unwrap();
        }
        let err = match Store::open(&home) {
            Ok(_) => panic!("a newer schema must be refused"),
            Err(e) => e,
        };
        assert!(
            matches!(err, crate::CoreError::SchemaNewer { found, known }
                if found == newer && known == MIGRATIONS.len() as u32),
            "{err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains(&newer.to_string()), "{msg}");
        assert!(msg.contains(&MIGRATIONS.len().to_string()), "{msg}");
        assert!(msg.contains("upgrade clax"), "{msg}");
        let c = Connection::open(home.db_path()).unwrap();
        let v: u32 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, newer, "the refused database is not touched");
    }

    /// Opens `home` from `n` threads at once; every open must succeed and
    /// leave the database at the latest schema.
    fn open_concurrently(home: &Home, n: usize) {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));
        let opens: Vec<_> = (0..n)
            .map(|_| {
                let (home, barrier) = (home.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open(&home).map(drop)
                })
            })
            .collect();
        for open in opens {
            open.join()
                .unwrap()
                .expect("every concurrent open succeeds");
        }
        let c = Connection::open(home.db_path()).unwrap();
        let v: u32 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as u32);
    }

    #[test]
    fn concurrent_opens_of_an_old_database_migrate_it_once() {
        for _ in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            home.ensure_dirs().unwrap();
            {
                let c = Connection::open(home.db_path()).unwrap();
                c.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
                for sql in &MIGRATIONS[..15] {
                    c.execute_batch(sql).unwrap();
                }
                c.pragma_update(None, "user_version", 15).unwrap();
            }
            open_concurrently(&home, 6);
        }
    }

    #[test]
    fn concurrent_opens_of_a_fresh_database_migrate_it_once() {
        for _ in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            home.ensure_dirs().unwrap();
            open_concurrently(&home, 6);
        }
    }
}

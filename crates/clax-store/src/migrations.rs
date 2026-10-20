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
    // Joined sites (spec 2026-10-05-chrome-overlay-design §7.2). Each origin the owner joined to a site, with the site's key (the
    // origin whose live pages and rules hold the site's; it has a row of its
    // own), when it joined and when Clax last used it; a joined origin's page
    // merged into the site's page of its path once its threads moved there,
    // kept whole (its artifact and snapshots) but no longer a live page's key
    // (`merged_into` NULL once that page is deleted: released, listed again);
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
        merged_into TEXT,
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
    // 20: agent questions (spec 2026-10-06-agent-questions-and-inbox-design
    // §5.1): a question an agent asked (`ask`) or Claude Code's
    // AskUserQuestion that the hook mirrored (`hook`, keyed by the call's
    // tool_use_id), what it asks, how it closed, and when its session
    // received the answer. No foreign key to `artifacts`, as for
    // `version_threads` (11). `questions_closed` serves the closed listing,
    // newest first, without sorting the history.
    "CREATE TABLE questions (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        artifact_id TEXT,
        source TEXT NOT NULL CHECK (source IN ('ask', 'hook')),
        tool_use_id TEXT,
        questions_json TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'open'
            CHECK (status IN ('open', 'answered', 'declined', 'released', 'withdrawn')),
        answers_json TEXT,
        answered_via TEXT CHECK (answered_via IN ('shell', 'extension', 'cli', 'terminal')),
        created_at TEXT NOT NULL,
        closed_at TEXT,
        taken_at TEXT
    );
    CREATE INDEX questions_by_status ON questions(status, created_at, id);
    CREATE INDEX questions_by_session ON questions(session_id, status);
    CREATE INDEX questions_by_artifact ON questions(artifact_id, status)
        WHERE artifact_id IS NOT NULL;
    CREATE UNIQUE INDEX questions_by_tool_use ON questions(session_id, tool_use_id)
        WHERE tool_use_id IS NOT NULL;
    CREATE INDEX questions_closed ON questions(closed_at, id) WHERE status <> 'open';",
    // 21: the inbox (spec 2026-10-06-agent-questions-and-inbox-design §7.3):
    // one item per thing an agent sent the owner, referencing its source by
    // key (a finished working record keeps its message in `detail_json`, as
    // its source lives in memory), with its read time; and a contentless
    // FTS5 index of each item's search text, which also indexes every
    // word's first one to three characters so a short prefix reads one
    // entry rather than every word it begins. The history before this
    // migration is filled in, read (§7.5), indexed as items made later are,
    // leaving out deleted artifacts. `inbox_by_session` serves the
    // agent filter by handle.
    "CREATE TABLE inbox_items (
        seq INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK (kind IN ('reply', 'version', 'published', 'question', 'finished')),
        key TEXT NOT NULL UNIQUE,
        artifact_id TEXT,
        thread_id TEXT,
        comment_id TEXT,
        version_n INTEGER,
        question_id TEXT,
        session_id TEXT,
        harness TEXT,
        detail_json TEXT,
        created_at TEXT NOT NULL,
        read_at TEXT
    );
    CREATE INDEX inbox_unread ON inbox_items(seq) WHERE read_at IS NULL;
    CREATE INDEX inbox_by_artifact ON inbox_items(artifact_id, seq);
    CREATE INDEX inbox_by_kind ON inbox_items(kind, seq);
    CREATE INDEX inbox_by_harness ON inbox_items(harness, seq);
    CREATE INDEX inbox_by_session ON inbox_items(session_id, seq);
    CREATE INDEX inbox_by_created ON inbox_items(created_at, seq);
    CREATE INDEX inbox_by_thread ON inbox_items(thread_id) WHERE thread_id IS NOT NULL;
    CREATE INDEX inbox_by_question ON inbox_items(question_id) WHERE question_id IS NOT NULL;
    CREATE VIRTUAL TABLE inbox_fts USING fts5(
        text, content='', contentless_delete=1, prefix='1 2 3',
        tokenize='unicode61 remove_diacritics 2');
    CREATE TEMP TABLE IF NOT EXISTS _owner AS
        SELECT public_id AS pid FROM viewers WHERE owner = 1;
    INSERT INTO inbox_items (id, kind, key, artifact_id, thread_id, comment_id, version_n, session_id, harness, created_at, read_at)
    SELECT 'b' || lower(hex(randomblob(12))), kind, key, artifact_id, thread_id, comment_id, version_n,
           session_id, harness, created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    FROM (
        SELECT 'reply' AS kind, 'reply:' || c.id AS key, t.artifact_id, t.id AS thread_id, c.id AS comment_id,
               NULL AS version_n, c.via_session_id AS session_id, s.harness, c.created_at
        FROM comments c JOIN threads t ON t.id = c.thread_id
        JOIN artifacts a ON a.id = t.artifact_id AND a.deleted_at IS NULL
        LEFT JOIN sessions s ON s.id = c.via_session_id
        CROSS JOIN _owner o
        WHERE c.author_kind = 'agent'
          AND (EXISTS (SELECT 1 FROM comments m WHERE m.thread_id = t.id AND m.author_public_id = o.pid)
               OR EXISTS (SELECT 1 FROM comments m JOIN mentions x ON x.comment_id = m.id
                          WHERE m.thread_id = t.id AND x.public_id = o.pid)
               OR t.resolved_by = 'viewer:' || o.pid)
        UNION ALL
        SELECT 'version', 'version:' || v.artifact_id || ':' || v.n, v.artifact_id, NULL, NULL, v.n,
               v.session_id, s.harness, v.created_at
        FROM versions v JOIN sessions s ON s.id = v.session_id
        JOIN artifacts a ON a.id = v.artifact_id AND a.deleted_at IS NULL
        CROSS JOIN _owner o
        WHERE v.n > 1 AND EXISTS (SELECT 1 FROM threads t JOIN comments m ON m.thread_id = t.id
                                  WHERE t.artifact_id = v.artifact_id AND m.author_public_id = o.pid)
        UNION ALL
        SELECT 'published', 'published:' || a.id, a.id, NULL, NULL, 1, a.owner_session_id, s.harness, a.created_at
        FROM artifacts a JOIN sessions s ON s.id = a.owner_session_id
        WHERE a.kind = 'html' AND a.deleted_at IS NULL
    )
    ORDER BY created_at, key;
    INSERT INTO inbox_fts (rowid, text)
        SELECT i.seq, coalesce(a.title, '') || ' ' || coalesce(i.harness, '') || ' ' ||
               CASE i.kind
                 WHEN 'reply' THEN coalesce(c.body, '')
                 WHEN 'version' THEN coalesce(v.note, '') || ' ' || coalesce(
                     (SELECT group_concat(CASE WHEN json_valid(t.anchor_json)
                                               THEN json_extract(t.anchor_json, '$.quote') END, ' ')
                        FROM version_threads vt JOIN threads t ON t.id = vt.thread_id
                       WHERE vt.artifact_id = i.artifact_id AND vt.version_n = i.version_n), '')
                 ELSE coalesce(a.description, '')
               END
        FROM inbox_items i
        LEFT JOIN artifacts a ON a.id = i.artifact_id
        LEFT JOIN comments c ON c.id = i.comment_id
        LEFT JOIN versions v ON v.artifact_id = i.artifact_id AND v.n = i.version_n;
    DROP TABLE _owner;",
    // 22: covering indexes for the gallery's whole-home reads (attention,
    // participants): each per-thread or per-artifact lookup reads what it
    // needs from the index instead of one table row per comment or version.
    // `comments_by_thread` keeps its leading columns, so every query that
    // used it orders and seeks as before.
    "DROP INDEX comments_by_thread;
    CREATE INDEX comments_by_thread ON comments(thread_id, created_at, id, author_public_id, via_session_id);
    DROP INDEX version_threads_by_thread;
    CREATE INDEX version_threads_by_thread ON version_threads(thread_id, created_at, version_n);
    CREATE INDEX versions_by_session ON versions(artifact_id, session_id, created_at);",
    // 23: the audit journal (spec 2026-10-06-toolpath-audit-design §5.1):
    // append-only events in commit order, the opaque install ID (128
    // random bits, minted by this migration's Rust step, `install_id`), each
    // version's content hash and each session's transcript path. Events
    // name artifacts without a foreign key, since they outlive deletion.
    // `Store::open` then records the history before it once (`backfill`).
    "CREATE TABLE audit_events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at TEXT NOT NULL,
        kind TEXT NOT NULL,
        actor TEXT NOT NULL,
        artifact_id TEXT,
        artifact2_id TEXT,
        thread_id TEXT,
        session_id TEXT,
        question_id TEXT,
        call_id TEXT,
        origin TEXT,
        body TEXT NOT NULL,
        backfilled INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX audit_events_artifact ON audit_events(artifact_id, seq);
    CREATE INDEX audit_events_artifact2 ON audit_events(artifact2_id, seq) WHERE artifact2_id IS NOT NULL;
    CREATE INDEX audit_events_session ON audit_events(session_id, seq);
    CREATE INDEX audit_events_call ON audit_events(call_id) WHERE call_id IS NOT NULL;
    CREATE INDEX audit_events_at ON audit_events(at);
    CREATE TABLE install (k TEXT PRIMARY KEY, v TEXT NOT NULL);
    ALTER TABLE versions ADD COLUMN content_sha256 TEXT;
    ALTER TABLE sessions ADD COLUMN transcript_path TEXT;",
    // 24: people, grants, sessions bound to their credential, and authors
    // (spec 2026-10-08-multi-agent-multi-person §5.1). A connected person is
    // a viewer with `person = 1`; `person` columns hold public IDs, NULL for
    // the owner, so every existing row stays the owner's. A session's
    // harness session ID is unique within its credential (`grant_id`, NULL
    // for the daemon token), never across credentials (M4). A grant's
    // session keeps the SHA-256 of its secret (`secret_sha256`, §6.8),
    // cleared when it ends. A browser's `clax_viewer` cookie is a random
    // secret kept as its SHA-256 (`viewer_cookies`, §21.4); until now it was
    // the viewer's row ID, which the migration's Rust step hashes for every
    // viewer but the owner as a `legacy` cookie, so existing LAN viewers
    // keep working until their cookie is replaced; a replaced legacy
    // cookie keeps naming its viewer for a short while (`replaced_at`),
    // for the requests already in flight with it.
    "ALTER TABLE viewers ADD COLUMN person INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE viewers ADD COLUMN person_revoked_at TEXT;
    ALTER TABLE viewers ADD COLUMN rights_json TEXT;

    CREATE TABLE viewer_cookies (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        created_at TEXT NOT NULL,
        legacy INTEGER NOT NULL DEFAULT 0,
        replaced_at TEXT
    );
    CREATE INDEX viewer_cookies_by_viewer ON viewer_cookies(viewer_id);

    CREATE TABLE invites (
        id TEXT PRIMARY KEY,
        code_sha256 TEXT NOT NULL UNIQUE,
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        used_at TEXT,
        grant_id TEXT,
        revoked_at TEXT
    );

    CREATE TABLE grants (
        id TEXT PRIMARY KEY,
        viewer_id TEXT REFERENCES viewers(id),
        status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'revoked', 'expired')),
        via TEXT NOT NULL CHECK (via IN ('invite', 'request')),
        client_key TEXT NOT NULL,
        client_name TEXT NOT NULL,
        peer_ip TEXT NOT NULL,
        user_code_sha256 TEXT,
        wrong_codes INTEGER NOT NULL DEFAULT 0,
        continue_sha256 TEXT NOT NULL,
        continue_t INTEGER,
        continue_nonce TEXT,
        manage_sha256 TEXT,
        narrow_json TEXT,
        created_at TEXT NOT NULL,
        decided_at TEXT,
        last_used_at TEXT,
        ended_at TEXT,
        ended_by TEXT,
        browser_sha256 TEXT,
        browser_expires_at TEXT
    );
    CREATE UNIQUE INDEX grants_browser ON grants(browser_sha256) WHERE browser_sha256 IS NOT NULL;
    CREATE UNIQUE INDEX grants_live_key ON grants(client_key) WHERE status IN ('pending', 'approved');
    CREATE INDEX grants_by_viewer ON grants(viewer_id, status);
    CREATE INDEX grants_pending ON grants(created_at) WHERE status = 'pending';

    CREATE TABLE grant_tokens (
        sha256 TEXT PRIMARY KEY,
        grant_id TEXT NOT NULL REFERENCES grants(id),
        created_at TEXT NOT NULL,
        revoked_at TEXT
    );
    CREATE INDEX grant_tokens_by_grant ON grant_tokens(grant_id);

    ALTER TABLE sessions ADD COLUMN grant_id TEXT;
    ALTER TABLE sessions ADD COLUMN person TEXT;
    ALTER TABLE sessions ADD COLUMN name TEXT;
    ALTER TABLE sessions ADD COLUMN name_source TEXT CHECK (name_source IN ('derived', 'agent', 'person'));
    ALTER TABLE sessions ADD COLUMN narrow_json TEXT;
    ALTER TABLE sessions ADD COLUMN ended_reason TEXT
        CHECK (ended_reason IN ('idle', 'exit', 'revoked', 'person'));
    ALTER TABLE sessions ADD COLUMN ended_watches_json TEXT;
    ALTER TABLE sessions ADD COLUMN secret_sha256 TEXT;
    CREATE TABLE session_elevations (
        session_id TEXT NOT NULL,
        granter TEXT,
        access_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        version TEXT NOT NULL DEFAULT ''
    );
    CREATE UNIQUE INDEX session_elevations_key ON session_elevations(session_id, COALESCE(granter, ''));
    CREATE INDEX session_elevations_by_granter ON session_elevations(granter) WHERE granter IS NOT NULL;
    CREATE UNIQUE INDEX sessions_live_name ON sessions(name COLLATE NOCASE)
        WHERE ended_at IS NULL AND name IS NOT NULL;
    CREATE INDEX sessions_by_grant ON sessions(grant_id, harness, parent_pid) WHERE grant_id IS NOT NULL;
    DROP INDEX sessions_harness_id;
    CREATE UNIQUE INDEX sessions_harness_id ON sessions(harness, harness_session_id, COALESCE(grant_id, ''))
        WHERE harness_session_id IS NOT NULL AND ended_at IS NULL;

    ALTER TABLE artifacts ADD COLUMN person TEXT;
    ALTER TABLE versions ADD COLUMN person TEXT;
    ALTER TABLE versions ADD COLUMN by_page INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE versions ADD COLUMN file_authors_json TEXT;
    ALTER TABLE assets ADD COLUMN person TEXT;",
    // 25: visibility (spec 2026-10-08-multi-agent-multi-person §5.2): who
    // approved whose words reaching their agents, the requests for such
    // approvals, each document's authors, the authors a page's writes
    // carry, an artifact's text's authors, a batch's sender and the person
    // of the agent that resolved a thread. A document
    // without `doc_authors` rows is the owner's, a `via_page` comment
    // without `page_authors` rows was written on an owner's page, and an
    // artifact without text authors is its creator's, so existing rows stay
    // the owner's; a batch without a sender, and an agent's resolve without
    // a person, are `*unconnected`. The owner's
    // one approval, of `*unconnected` everywhere, keeps LAN viewers' words
    // reaching the owner's agents (O6, M14): the migration's Rust step
    // makes it, with a ULID. Requests pile up (each resolve withdraws its
    // thread's), so the reads of a pair's or a thread's open and declined
    // requests go through `approval_requests_by_target` and
    // `approval_requests_by_thread`.
    "CREATE TABLE approvals (
        id TEXT PRIMARY KEY,
        approver TEXT,
        author TEXT NOT NULL,
        scope TEXT NOT NULL CHECK (scope IN ('thread', 'artifact', 'everywhere')),
        artifact_id TEXT,
        thread_id TEXT,
        created_at TEXT NOT NULL,
        created_via TEXT NOT NULL CHECK (created_via IN ('shell', 'extension', 'connect', 'migration')),
        request_id TEXT,
        revoked_at TEXT
    );
    CREATE UNIQUE INDEX approvals_live ON approvals(
        COALESCE(approver, ''), author, scope, COALESCE(artifact_id, ''), COALESCE(thread_id, ''))
        WHERE revoked_at IS NULL;
    CREATE INDEX approvals_by_approver ON approvals(COALESCE(approver, ''), author) WHERE revoked_at IS NULL;

    CREATE TABLE approval_requests (
        id TEXT PRIMARY KEY,
        approver TEXT,
        author TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        thread_id TEXT,
        session_id TEXT,
        cause TEXT NOT NULL CHECK (cause IN ('ask', 'routing')),
        count INTEGER NOT NULL DEFAULT 1,
        status TEXT NOT NULL DEFAULT 'open'
            CHECK (status IN ('open', 'approved', 'declined', 'withdrawn')),
        decided_scope TEXT CHECK (decided_scope IN ('thread', 'artifact', 'everywhere')),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        decided_at TEXT
    );
    CREATE UNIQUE INDEX approval_requests_open ON approval_requests(
        COALESCE(approver, ''), author, artifact_id, COALESCE(thread_id, ''))
        WHERE status = 'open';
    CREATE INDEX approval_requests_by_session ON approval_requests(session_id, status);
    CREATE INDEX approval_requests_by_target ON approval_requests(
        COALESCE(approver, ''), artifact_id, author, status);
    CREATE INDEX approval_requests_by_thread ON approval_requests(thread_id, status);

    CREATE TABLE doc_authors (
        artifact_id TEXT NOT NULL,
        path TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path, author)
    );
    CREATE TABLE page_authors (
        kind TEXT NOT NULL CHECK (kind IN ('comment', 'asset')),
        id TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (kind, id, author)
    );
    ALTER TABLE feedback ADD COLUMN placeholder_at TEXT;
    ALTER TABLE feedback ADD COLUMN no_access_at TEXT;
    ALTER TABLE artifacts ADD COLUMN text_authors_json TEXT;
    ALTER TABLE send_batches ADD COLUMN sender TEXT;
    ALTER TABLE threads ADD COLUMN resolved_by_person TEXT;",
    // 26: the per-person inbox (spec 2026-10-08-multi-agent-multi-person
    // §5.3, M21): each item names its recipient, the connected person it is
    // for (NULL: the owner, so every existing item stays the owner's), and
    // three kinds join: `approval` (a request, by `request_id`),
    // `connection` (a grant, by `grant_id`) and `ended` (in `detail_json`).
    // The `kind` CHECK cannot be altered, so the table is rebuilt as
    // migration 19 rebuilt `thread_moves`, keeping every `seq`, so the
    // contentless `inbox_fts` keeps naming its items. Each index a list or
    // a read mark walks is led by the recipient; `inbox_by_created` (the
    // newest stamp, and the `seq` bounds a date range gives, which hold for
    // every recipient as stamps follow `seq`), `inbox_by_thread` and
    // `inbox_by_question` serve their sources, whatever the recipient.
    // `viewers_people` lists the connected people an item may go to.
    // Each question names the person whose agent asked it (the asking
    // session's, NULL: the owner's), filled in from its session, so a
    // person's question lists walk their own questions by index.
    "CREATE TABLE inbox_items_26 (
        seq INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK (kind IN ('reply', 'version', 'published', 'question', 'finished',
                                           'approval', 'connection', 'ended')),
        key TEXT NOT NULL UNIQUE,
        recipient TEXT,
        artifact_id TEXT,
        thread_id TEXT,
        comment_id TEXT,
        version_n INTEGER,
        question_id TEXT,
        session_id TEXT,
        harness TEXT,
        request_id TEXT,
        grant_id TEXT,
        detail_json TEXT,
        created_at TEXT NOT NULL,
        read_at TEXT
    );
    INSERT INTO inbox_items_26 (seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
        question_id, session_id, harness, detail_json, created_at, read_at)
        SELECT seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
               question_id, session_id, harness, detail_json, created_at, read_at FROM inbox_items;
    DROP TABLE inbox_items;
    ALTER TABLE inbox_items_26 RENAME TO inbox_items;
    CREATE INDEX inbox_unread ON inbox_items(COALESCE(recipient, ''), seq) WHERE read_at IS NULL;
    CREATE INDEX inbox_by_recipient ON inbox_items(COALESCE(recipient, ''), seq);
    CREATE INDEX inbox_by_artifact ON inbox_items(COALESCE(recipient, ''), artifact_id, seq);
    CREATE INDEX inbox_by_kind ON inbox_items(COALESCE(recipient, ''), kind, seq);
    CREATE INDEX inbox_by_harness ON inbox_items(COALESCE(recipient, ''), harness, seq);
    CREATE INDEX inbox_by_session ON inbox_items(COALESCE(recipient, ''), session_id, seq);
    CREATE INDEX inbox_by_created ON inbox_items(created_at, seq);
    CREATE INDEX inbox_by_thread ON inbox_items(thread_id) WHERE thread_id IS NOT NULL;
    CREATE INDEX inbox_by_question ON inbox_items(question_id) WHERE question_id IS NOT NULL;
    CREATE INDEX inbox_by_request ON inbox_items(request_id) WHERE request_id IS NOT NULL;
    CREATE INDEX viewers_people ON viewers(public_id) WHERE person = 1;
    ALTER TABLE questions ADD COLUMN person TEXT;
    UPDATE questions SET person = (SELECT s.person FROM sessions s WHERE s.id = questions.session_id);
    CREATE INDEX questions_open_by_person ON questions(COALESCE(person, ''), created_at, id)
        WHERE status = 'open';
    CREATE INDEX questions_closed_by_person ON questions(COALESCE(person, ''), closed_at, id)
        WHERE status <> 'open';",
    // 27: hosted mode's sign-in (`crate::hosted`). `hosted_setup`
    // holds the one first-person setup (its row, once made, closes setup);
    // `browser_sessions` and `signin_links` hold the SHA-256 of a browser
    // session's cookie and of a one-use sign-in link's code, with the viewer
    // row each signs in as (the owner's row, a person's or a guest's; no
    // foreign key, as the owner row's ID is rewritten by a claim). A guest
    // is a named viewer of a page shared by link (`viewers.guest`); a
    // bearer grant is an agent token a person minted in the browser
    // (`grants.bearer`), its token kept in `grant_tokens`; an artifact's
    // `link_access` is what anyone with its link may do (NULL: nothing).
    // Local daemons write none of these: every existing row reads as
    // before.
    "CREATE TABLE hosted_setup (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        viewer_id TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE browser_sessions (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        ended_at TEXT
    );
    CREATE INDEX browser_sessions_by_viewer ON browser_sessions(viewer_id) WHERE ended_at IS NULL;
    CREATE TABLE signin_links (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        used_at TEXT
    );
    ALTER TABLE viewers ADD COLUMN guest INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE grants ADD COLUMN bearer INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE artifacts ADD COLUMN link_access TEXT CHECK (link_access IN ('viewer', 'commenter'));",
    // 28: forks (`crate::forks`): the artifact and version a fork's
    // version 1 was made from. No foreign key: the source may be deleted,
    // and the fork still says where it came from. NULL on every artifact
    // that is not a fork.
    "ALTER TABLE artifacts ADD COLUMN forked_from_id TEXT;
    ALTER TABLE artifacts ADD COLUMN forked_from_version INTEGER;",
    // 29: the MCP client name an agent token's session registered under
    // (`clientInfo.name`, spec 2026-10-08 §6.9): a new session of the token
    // starts within the narrowings of its live sessions of the same name.
    // NULL for every other session, and for one registered before.
    "ALTER TABLE sessions ADD COLUMN client_name TEXT;",
    // 30: publishing to a public Clax (`crate::device`,
    // `crate::remotes`). On a hosted daemon, `device_codes` holds
    // the device sign-ins of other machines' Clax (the SHA-256 of the
    // device code each polls with and of the user code a person types),
    // the machine's own name, and the person who approved it with the
    // bearer grant made for it. On a local daemon, `remotes` holds each
    // public Clax this one signed in to and the agent token it holds
    // there (an outbound credential, so kept as it is: the database is in
    // the home, which only its user reads); `remote_links` each local
    // artifact's public twin, the last local version pushed and the public
    // version that push made; `remote_assets` the public address each
    // asset was uploaded to on a public artifact; `push_approvals` the
    // owner's question each agent's push waits on and the version it asked
    // about. On a hosted daemon, `publish_keys` holds the idempotency key
    // an agent token's publish named and the version it made, so a retried
    // push answers that version again. A daemon writes only its own side's
    // tables. The inbox is rebuilt as migration 26 left it, its kinds with
    // `push` (an automatic push that a public Clax refused).
    "CREATE TABLE device_codes (
        id TEXT PRIMARY KEY,
        device_sha256 TEXT NOT NULL UNIQUE,
        user_sha256 TEXT NOT NULL UNIQUE,
        client_name TEXT NOT NULL,
        peer_ip TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending'
            CHECK (status IN ('pending', 'approved', 'denied', 'issued')),
        viewer_id TEXT,
        grant_id TEXT,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        decided_at TEXT,
        polled_at TEXT
    );
    CREATE INDEX device_codes_by_expiry ON device_codes(expires_at);
    CREATE TABLE remotes (
        id TEXT PRIMARY KEY,
        origin TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL UNIQUE,
        status TEXT NOT NULL CHECK (status IN ('pending', 'connected')),
        token TEXT,
        device_code TEXT,
        user_code TEXT,
        verification_url TEXT,
        sign_in_expires_at TEXT,
        poll_interval_s INTEGER,
        person_public_id TEXT,
        person_name TEXT,
        grant_id TEXT,
        created_at TEXT NOT NULL,
        connected_at TEXT
    );
    CREATE TABLE remote_links (
        artifact_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        url TEXT NOT NULL,
        pushed_n INTEGER NOT NULL,
        remote_n INTEGER NOT NULL,
        auto INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        pushed_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, remote_id)
    );
    CREATE TABLE remote_assets (
        remote_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        asset_id TEXT NOT NULL,
        url TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (remote_id, remote_artifact_id, asset_id)
    );
    CREATE TABLE push_approvals (
        question_id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        upto INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        used_at TEXT
    );
    CREATE TABLE publish_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        n INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );
    CREATE TABLE inbox_items_30 (
        seq INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK (kind IN ('reply', 'version', 'published', 'question', 'finished',
                                           'approval', 'connection', 'ended', 'push')),
        key TEXT NOT NULL UNIQUE,
        recipient TEXT,
        artifact_id TEXT,
        thread_id TEXT,
        comment_id TEXT,
        version_n INTEGER,
        question_id TEXT,
        session_id TEXT,
        harness TEXT,
        request_id TEXT,
        grant_id TEXT,
        detail_json TEXT,
        created_at TEXT NOT NULL,
        read_at TEXT
    );
    INSERT INTO inbox_items_30 SELECT seq, id, kind, key, recipient, artifact_id, thread_id,
        comment_id, version_n, question_id, session_id, harness, request_id, grant_id, detail_json,
        created_at, read_at FROM inbox_items;
    DROP TABLE inbox_items;
    ALTER TABLE inbox_items_30 RENAME TO inbox_items;
    CREATE INDEX inbox_unread ON inbox_items(COALESCE(recipient, ''), seq) WHERE read_at IS NULL;
    CREATE INDEX inbox_by_recipient ON inbox_items(COALESCE(recipient, ''), seq);
    CREATE INDEX inbox_by_artifact ON inbox_items(COALESCE(recipient, ''), artifact_id, seq);
    CREATE INDEX inbox_by_kind ON inbox_items(COALESCE(recipient, ''), kind, seq);
    CREATE INDEX inbox_by_harness ON inbox_items(COALESCE(recipient, ''), harness, seq);
    CREATE INDEX inbox_by_session ON inbox_items(COALESCE(recipient, ''), session_id, seq);
    CREATE INDEX inbox_by_created ON inbox_items(created_at, seq);
    CREATE INDEX inbox_by_thread ON inbox_items(thread_id) WHERE thread_id IS NOT NULL;
    CREATE INDEX inbox_by_question ON inbox_items(question_id) WHERE question_id IS NOT NULL;
    CREATE INDEX inbox_by_request ON inbox_items(request_id) WHERE request_id IS NOT NULL;",
    // 31: GitHub sign-in on a hosted daemon (`crate::github`).
    // `github_identities` links a GitHub account, by its numeric user ID
    // (stable across renames), to the viewer row it signs in as: the
    // owner's, or a person made at their first sign-in. `login`, `name`
    // and `avatar_url` are GitHub's, as of the last sign-in, for display.
    // `blocked_at` is set when the owner revokes the person: the account
    // may not sign in or sign up again. No foreign key, as for `browser_sessions`. A local daemon writes
    // none.
    "CREATE TABLE github_identities (
        github_id INTEGER PRIMARY KEY,
        viewer_id TEXT NOT NULL UNIQUE,
        login TEXT NOT NULL,
        name TEXT,
        avatar_url TEXT,
        created_at TEXT NOT NULL,
        signed_in_at TEXT NOT NULL,
        blocked_at TEXT
    );",
    // 32: one artifact shared with one person (`crate::shares`).
    // A share names its person by viewer row (`viewer_id`), or, until that
    // GitHub account first signs in, by its login alone (`github_login`,
    // lowercased; kept for display once bound). `access` is what the person
    // may do there. `artifacts.link_listed` lists an artifact shared by link
    // in the public gallery. No foreign key on `viewer_id`, as for
    // `github_identities`; a purged artifact takes its shares with it.
    "CREATE TABLE artifact_shares (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        viewer_id TEXT,
        github_login TEXT CHECK (github_login IS NULL OR github_login = lower(github_login)),
        access TEXT NOT NULL CHECK (access IN ('viewer', 'commenter', 'editor')),
        created_at TEXT NOT NULL,
        created_by TEXT,
        CHECK (viewer_id IS NOT NULL OR github_login IS NOT NULL)
    );
    CREATE UNIQUE INDEX artifact_shares_by_viewer ON artifact_shares(artifact_id, viewer_id)
        WHERE viewer_id IS NOT NULL;
    CREATE UNIQUE INDEX artifact_shares_pending ON artifact_shares(artifact_id, github_login)
        WHERE viewer_id IS NULL;
    CREATE INDEX artifact_shares_of_viewer ON artifact_shares(viewer_id) WHERE viewer_id IS NOT NULL;
    CREATE INDEX artifact_shares_of_login ON artifact_shares(github_login) WHERE viewer_id IS NULL;
    ALTER TABLE artifacts ADD COLUMN link_listed INTEGER NOT NULL DEFAULT 0;",
    // 33: linked threads (`crate::linked`): a local artifact's
    // comments kept in step with its public twin's. On a local daemon,
    // `remote_versions` records each local version a push made into a
    // public version (the anchor translation); `linked_threads` pairs a
    // local thread with its twin on a remote (`origin`: the side it started
    // on; `remote_thread_id` is NULL until an up-sent thread is
    // acknowledged; `remote_version` and `twin` say which public version
    // it is on and whether a local version is that one's twin;
    // `synced_status` the status both sides last agreed on; `unlinked_at`
    // once the link or remote is gone, when it is read-only history;
    // `remote_name` and `remote_origin` kept for that history);
    // `linked_comments` pairs comments the same way, with the idempotency
    // key an up-send carries and, for a comment from the remote, its
    // author there (display only: never a local person); `linked_outbox`
    // holds the local writes not yet acknowledged by the remote, in order.
    // `remote_links` gains the download-only switch and the sync cursor.
    // On a hosted daemon, `comment_keys` holds the idempotency key an
    // agent token's comment or thread named and what it made, so a resend
    // answers that again. A daemon writes only its own side's tables.
    "CREATE TABLE remote_versions (
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        local_n INTEGER NOT NULL,
        remote_n INTEGER NOT NULL,
        PRIMARY KEY (remote_id, remote_artifact_id, remote_n)
    );
    CREATE INDEX remote_versions_by_local ON remote_versions(remote_id, artifact_id, local_n);
    CREATE TABLE linked_threads (
        thread_id TEXT PRIMARY KEY,
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        remote_thread_id TEXT,
        origin TEXT NOT NULL CHECK (origin IN ('remote', 'local')),
        remote_version INTEGER,
        twin INTEGER NOT NULL DEFAULT 1,
        synced_status TEXT NOT NULL DEFAULT 'open',
        remote_name TEXT NOT NULL,
        remote_origin TEXT NOT NULL,
        unlinked_at TEXT,
        created_at TEXT NOT NULL
    );
    CREATE UNIQUE INDEX linked_threads_by_remote ON linked_threads(remote_id, remote_thread_id)
        WHERE remote_thread_id IS NOT NULL;
    CREATE INDEX linked_threads_by_artifact ON linked_threads(artifact_id, remote_id);
    CREATE TABLE linked_comments (
        comment_id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        remote_comment_id TEXT,
        idem_key TEXT NOT NULL UNIQUE,
        origin TEXT NOT NULL CHECK (origin IN ('remote', 'local')),
        author_kind TEXT,
        author_public_id TEXT,
        author_name TEXT,
        created_at TEXT NOT NULL
    );
    CREATE UNIQUE INDEX linked_comments_by_remote ON linked_comments(remote_id, remote_comment_id)
        WHERE remote_comment_id IS NOT NULL;
    CREATE INDEX linked_comments_by_thread ON linked_comments(thread_id);
    CREATE TABLE linked_outbox (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('thread', 'comment', 'resolve', 'reopen')),
        comment_id TEXT,
        idem_key TEXT NOT NULL UNIQUE,
        agent_name TEXT,
        agent_harness TEXT,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        failed_at TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX linked_outbox_by_thread ON linked_outbox(thread_id, seq);
    ALTER TABLE remote_links ADD COLUMN download_only INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE remote_links ADD COLUMN sync_cursor TEXT;
    CREATE TABLE comment_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        comment_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );",
    // 34: a linked artifact's threads go up from `threads_since` on
    // (`crate::linked`): set when a download-only link is made
    // two-way again, so a thread started while it was download only stays
    // local. NULL: since the link was made.
    "ALTER TABLE remote_links ADD COLUMN threads_since TEXT;",
    // 35: linked questions (`crate::linked`). On a local daemon,
    // `linked_questions` pairs an `ask` question of the owner's agents about
    // a linked artifact with its copy on the remote: `idem_key` is the
    // Idempotency-Key its creation there carries (a new one each time the
    // copy is asked again after the remote withdrew it), `remote_question_id`
    // is NULL until that creation is acknowledged, `attempts` counts tries
    // whose answer was never read, `failed_at` marks it not sent (Retry),
    // `done_at` is set once both sides are closed (`answered_there` when the
    // remote's answer or decline closed it here). On a hosted daemon,
    // `question_keys` holds the Idempotency-Key an agent token's question
    // named and the question it made, so a resend answers that again and
    // the token's other sessions may read and withdraw it.
    "CREATE TABLE linked_questions (
        question_id TEXT PRIMARY KEY,
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        remote_question_id TEXT,
        idem_key TEXT NOT NULL UNIQUE,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        failed_at TEXT,
        answered_there INTEGER NOT NULL DEFAULT 0,
        done_at TEXT,
        remote_name TEXT NOT NULL,
        remote_origin TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX linked_questions_live ON linked_questions(remote_id, question_id)
        WHERE done_at IS NULL;
    CREATE TABLE question_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        question_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );
    CREATE INDEX question_keys_by_question ON question_keys(question_id, grant_id);",
    // 36: a member's own-space rights (`crate::github::member_rights`)
    // gain `clax-ask`, so their agents may ask them. Rows holding exactly the
    // earlier member rights, as stored, are given the new ones; rights the
    // owner set otherwise are left as they are. A watch of another Clax's
    // sync session names the local agent it relays for there
    // (`relay_name`, `relay_harness`; `crate::watches`), which the
    // artifact's participants show in place of the sync's own name.
    r#"UPDATE viewers SET rights_json = '[{"actions":["create"],"locations":["*"],"type":"clax-artifacts"},{"actions":["delete","read","update"],"locations":["own"],"type":"clax-artifacts"},{"actions":["read","reply","resolve","send","watch","write"],"locations":["own"],"type":"clax-comments"},{"actions":["read","write"],"level":"admin","locations":["own"],"type":"clax-db"},{"actions":["ask"],"type":"clax-ask"}]'
        WHERE rights_json = '[{"actions":["create"],"locations":["*"],"type":"clax-artifacts"},{"actions":["delete","read","update"],"locations":["own"],"type":"clax-artifacts"},{"actions":["read","reply","resolve","send","watch","write"],"locations":["own"],"type":"clax-comments"},{"actions":["read","write"],"level":"admin","locations":["own"],"type":"clax-db"}]';
    ALTER TABLE watches ADD COLUMN relay_name TEXT;
    ALTER TABLE watches ADD COLUMN relay_harness TEXT;"#,
    // 37: the repair of a database that builds of unmerged branches
    // migrated with earlier copies of migrations 24 to 26, 30 and 36 under
    // the same numbers, so the final ones never ran on it. Its Rust step
    // (`repair`) does all of it, as what it does depends on what the
    // database holds: it makes every table, column and index those
    // migrations make in their final form that is missing, with the data
    // their steps fill in (each viewer's legacy cookie when it makes
    // `viewer_cookies`, the `*unconnected` approval when it makes
    // `approvals`, each question's person when it adds `questions.person`),
    // moves the owner's elevation of each live agent from
    // `sessions.elevate_json` to `session_elevations` and drops the column,
    // and rebuilds `push_approvals` without `upto` and `inbox_items` without
    // the `push` kind. On a database the final migrations made it changes
    // nothing. A repaired table's added columns come last, so its column
    // order may differ from a new database's; no read depends on it.
    "",
    // 38: where an agent's person is paying attention (spec
    // 2026-10-16-ask-where-attention-is-design §1-§3). `questions.thread_id`
    // places a question in a thread of its artifact; `in_terminal` marks a
    // row that says a question waits in the terminal or in chat and takes
    // no answer in Clax; `pulled_at` is when the person asked for such a
    // question in Clax instead ("Ask me here"), handed to the session once,
    // as a late answer is. On `sessions`: `terminal_at`, the person's last
    // prompt to the session in the terminal (`terminal_inferred` when the
    // harness reports no prompts and a turn with no Clax delivery stood in
    // for one); `turn_at`, the end of the last turn such a harness reported;
    // `delivered_*`, its last delivery from Clax and the artifact and thread
    // it was about; `delivered_since_turn`, set by a delivery and cleared by
    // the next reported turn, which then came from Clax, not the terminal;
    // `turn_open`, set by a poll from within a turn and cleared by its end;
    // `queued_since_turn`, a Codex queue delivery that reached the session
    // while a turn was open and so starts the next turn, which the turn's
    // end moves into `delivered_since_turn`;
    // `delivered_written_at`, when the session's own person did what the
    // delivery carries in Clax, and `delivered_act`, what: `wrote` the
    // delivered comment (by hand, not through a page), or `answered`,
    // `skipped` or `pulled` the session's question; the delivery's only
    // part in the person's Clax activity. Every time is the daemon's.
    "ALTER TABLE questions ADD COLUMN thread_id TEXT;
    ALTER TABLE questions ADD COLUMN in_terminal INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE questions ADD COLUMN pulled_at TEXT;
    ALTER TABLE sessions ADD COLUMN terminal_at TEXT;
    ALTER TABLE sessions ADD COLUMN terminal_inferred INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN turn_at TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_at TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_artifact_id TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_thread_id TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_since_turn INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN turn_open INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN queued_since_turn INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN delivered_written_at TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_act TEXT;",
    // 39: a person agreeing to a hosted Clax's Terms of Service and Privacy
    // Policy (`crate::legal`): one row per viewer row and version of the
    // documents, kept when the documents change, with when and how
    // (`signin`: a browser sign-in under the sign-in page's notice;
    // `prompt`: the signed-in prompt). No foreign key, as for
    // `browser_sessions`. A local daemon writes none.
    "CREATE TABLE legal_agreements (
        viewer_id TEXT NOT NULL,
        version TEXT NOT NULL,
        agreed_at TEXT NOT NULL,
        via TEXT NOT NULL,
        PRIMARY KEY (viewer_id, version)
    );",
    // 40: artifacts a person started from a brief in their browser
    // (`crate::briefs`): who started each (`starter`, their public ID, or
    // `*owner`), and the `Idempotency-Key` the request named, unique per
    // starter, so a retried request answers the artifact the first made.
    "CREATE TABLE briefs (
        artifact_id TEXT PRIMARY KEY,
        starter TEXT NOT NULL,
        key TEXT,
        created_at TEXT NOT NULL
    );
    CREATE UNIQUE INDEX briefs_by_key ON briefs (starter, key) WHERE key IS NOT NULL;",
    // 41: cross-app read grants (spec 2026-10-12-flutter-app-design §3.8): the
    // owner let `reader_id` read `source_id`'s documents. Deleting either
    // artifact removes the row: the foreign keys on a row delete, the trigger
    // on the soft delete (`deleted_at` set), which is how artifacts are deleted.
    "CREATE TABLE db_read_grants (
        reader_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        granted_at TEXT NOT NULL,
        PRIMARY KEY (reader_id, source_id)
    ) WITHOUT ROWID;
    CREATE INDEX db_read_grants_by_source ON db_read_grants(source_id);
    CREATE TRIGGER db_read_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM db_read_grants WHERE reader_id = NEW.id OR source_id = NEW.id;
    END;",
    // 42: draft artifacts (spec 2026-10-12-flutter-app-round2-design §5.1):
    // an agent's work in progress, rendered but never listed. `hidden` keeps
    // a row out of every listing; `draft_of` names the app a draft changes
    // (NULL for a new app's draft), whose documents it copies.
    "ALTER TABLE artifacts ADD COLUMN draft_of TEXT;
    ALTER TABLE artifacts ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX artifacts_hidden ON artifacts(hidden, created_at) WHERE hidden = 1;",
    // 43: network grants (spec 2026-10-12-flutter-real-apps-design C6): the
    // owner let `artifact_id` fetch from `host`, one of the hosts its
    // `capabilities.net.hosts` declared when the owner allowed it. Deleting
    // the artifact removes its rows (the foreign key on a row delete, the
    // trigger on the soft delete).
    "CREATE TABLE net_grants (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        host TEXT NOT NULL,
        granted_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, host)
    ) WITHOUT ROWID;
    CREATE TRIGGER net_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM net_grants WHERE artifact_id = NEW.id;
    END;",
    // 44: model use by apps (spec 2026-10-12-flutter-real-apps-design C7):
    // the owner let `artifact_id` call `sample()` (kept until revoked), and
    // what its calls cost each local day, in millionths of a US dollar, for
    // the daily cost cap. Deleting the artifact removes its rows (the
    // foreign keys on a row delete, the trigger on the soft delete).
    "CREATE TABLE sample_grants (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id) ON DELETE CASCADE,
        granted_at TEXT NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE sample_spend (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        day TEXT NOT NULL,
        micro_usd INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (artifact_id, day)
    ) WITHOUT ROWID;
    CREATE TRIGGER sample_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM sample_grants WHERE artifact_id = NEW.id;
        DELETE FROM sample_spend WHERE artifact_id = NEW.id;
    END;",
    // 45: whether a draft fetches under its live app's network grants
    // (spec 2026-10-12-flutter-real-apps-design C6): 1 for the agent's
    // drafts, 0 for a bundle update's check draft, which stays offline.
    // Meaningless for an artifact that is no draft.
    "ALTER TABLE artifacts ADD COLUMN draft_inherit_net INTEGER NOT NULL DEFAULT 1;",
];

/// The audit journal's migration, as a schema version (the
/// `user_version` it leaves).
pub const AUDIT_MIGRATION: u32 = 23;

/// The migration that adds people, grants and their tokens, as a schema
/// version (spec 2026-10-08-multi-agent-multi-person §5.1).
pub const PEOPLE_MIGRATION: u32 = 24;

/// The migration that adds approvals, approval requests and document
/// authors, as a schema version (spec 2026-10-08-multi-agent-multi-person
/// §5.2).
pub const VISIBILITY_MIGRATION: u32 = 25;

/// The migration that gives every inbox item a recipient, as a schema
/// version (spec 2026-10-08-multi-agent-multi-person §5.3).
pub const INBOX_MIGRATION: u32 = 26;

/// The migration that adds hosted mode's sign-in (browser sessions, sign-in
/// links, guests, bearer grants and link access), as a schema version.
pub const HOSTED_MIGRATION: u32 = 27;

/// The migration that repairs a database migrated by builds with earlier
/// copies of migrations 24 to 26, 30 and 36 ([`repair`]), as a schema
/// version.
pub const REPAIR_MIGRATION: u32 = 37;

/// The migration that records where an agent's person is paying attention
/// (`questions.thread_id`, `in_terminal`, `pulled_at` and the sessions'
/// engagement columns), as a schema version.
pub const WHERE_TO_ASK_MIGRATION: u32 = 38;

/// The first of the Clax app's migrations (cross-app read grants), as a
/// schema version: the one after main's above [`WHERE_TO_ASK_MIGRATION`]
/// ([`MAIN_MARKS_ABOVE_38`]); the app's follow it in
/// [`APP_MIGRATION_MARKS`]'s order.
pub const APP_MIGRATION: u32 = NUMBERING.app_first();

/// What each of main's migrations after [`WHERE_TO_ASK_MIGRATION`] and
/// before [`APP_MIGRATION`] leaves in the database, in order, so adoption
/// tells from it which of them a database lacks. Taking a new migration
/// of main's below the app's is adding it to [`MIGRATIONS`] and its
/// [`Mark`] here, nothing else.
const MAIN_MARKS_ABOVE_38: [Mark<'static>; 2] =
    [Mark::Table("legal_agreements"), Mark::Table("briefs")];

/// What a migration leaves in a database, by which adoption tells whether
/// a database holds it whatever its `user_version`. It must be absent
/// before the migration and present after it on any database, an empty one
/// included, and stay present after every later migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark<'a> {
    /// A table the migration makes.
    Table(&'a str),
    /// A column the migration adds: the table, then the column.
    Column(&'a str, &'a str),
    /// An index the migration makes.
    Index(&'a str),
    /// For a migration that changes only rows: a row of `table` that
    /// `predicate` (an SQL expression over its columns, the `WHERE` of a
    /// `SELECT` from it) finds after the migration. Such a migration must
    /// leave one even on an empty database, for example a row it inserts
    /// into `install`.
    Row {
        /// The table the row is in.
        table: &'a str,
        /// The SQL expression that finds it.
        predicate: &'a str,
    },
}

impl std::fmt::Display for Mark<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mark::Table(name) => write!(f, "table {name}"),
            Mark::Column(table, column) => write!(f, "column {table}.{column}"),
            Mark::Index(name) => write!(f, "index {name}"),
            Mark::Row { table, predicate } => write!(f, "row of {table} where {predicate}"),
        }
    }
}

/// A list of migrations and the marks of main's among them above
/// [`WHERE_TO_ASK_MIGRATION`], which the app's follow: this build's
/// ([`NUMBERING`]), or in tests a later build's.
pub(crate) struct Numbering<'a> {
    /// Every migration in order; the `user_version` each leaves is its
    /// position from 1.
    pub(crate) migrations: &'a [&'a str],
    /// What each of main's migrations after [`WHERE_TO_ASK_MIGRATION`]
    /// leaves in the schema, in order; the app's ([`APP_MIGRATION_MARKS`])
    /// follow them.
    pub(crate) main_marks: &'a [Mark<'a>],
}

impl Numbering<'_> {
    /// The number of the app's first migration.
    pub(crate) const fn app_first(&self) -> u32 {
        WHERE_TO_ASK_MIGRATION + 1 + self.main_marks.len() as u32
    }
}

/// This build's migrations ([`MIGRATIONS`], [`MAIN_MARKS_ABOVE_38`]).
pub(crate) const NUMBERING: Numbering<'static> = Numbering {
    migrations: MIGRATIONS,
    main_marks: &MAIN_MARKS_ABOVE_38,
};

/// What each of the Clax app's migrations ([`APP_MIGRATION`] onwards, in
/// order) leaves in the schema.
const APP_MIGRATION_MARKS: [Mark<'static>; 5] = [
    Mark::Table("db_read_grants"),
    Mark::Column("artifacts", "draft_of"),
    Mark::Table("net_grants"),
    Mark::Table("sample_grants"),
    Mark::Column("artifacts", "draft_inherit_net"),
];

/// Brings a database written by an earlier `flutter-app` build, which
/// numbered the app's migrations differently, into this numbering, in
/// `tx`; `Ok(None)` for any other database, left untouched. Returns the
/// `user_version` it had and the one it has now.
///
/// Those builds numbered the app's migrations (see [`APP_MIGRATION_MARKS`])
/// from 24, before the multi-person migrations ([`PEOPLE_MIGRATION`] to
/// [`INBOX_MIGRATION`]) were under them; later from 27, right after them,
/// before hosted mode's sign-in ([`HOSTED_MIGRATION`]) to the repair
/// ([`REPAIR_MIGRATION`]); later from 38, right after the repair, before
/// the where-to-ask migration ([`WHERE_TO_ASK_MIGRATION`]); and since then
/// right after main's migrations above it, of which a build held some
/// number k (the first k of [`MAIN_MARKS_ABOVE_38`]; k is 0 for the builds
/// that numbered them from 39, 2 for this one). Such a database is
/// recognised by its schema, not its `user_version` alone: the
/// `db_read_grants` table, and either no `questions.thread_id` column
/// (which [`WHERE_TO_ASK_MIGRATION`] adds) or fewer than all of main's
/// marks above it. With `questions.thread_id` and the first k of main's
/// marks its app migrations start at `38 + k + 1`, for any k; else with
/// the `hosted_setup` table (which [`HOSTED_MIGRATION`] makes) at 38;
/// else with the `invites` table (which [`PEOPLE_MIGRATION`] makes) at
/// 27, without it at 24; its version must name one of them. Each app
/// migration's mark must be present exactly up to the one its version
/// names, or the database is refused
/// ([`clax_core::CoreError::MigrationFailed`]) and left as it was. A
/// database with the app's tables whose main marks above 38 are not the
/// first k of them, or that holds any without `questions.thread_id`, was
/// made by no build, and is refused the same way. One with
/// `questions.thread_id` and all of them is this numbering's.
///
/// The migrations it lacks below [`APP_MIGRATION`] run on it, SQL and Rust
/// step each: the multi-person ones when it has no `invites`, then
/// [`HOSTED_MIGRATION`] to [`REPAIR_MIGRATION`] when it has no
/// `hosted_setup`, then [`WHERE_TO_ASK_MIGRATION`] when it has no
/// `questions.thread_id`, then main's above it after the first k; none of
/// them touches the app's tables or adds a column the app's migrations
/// add, and the repair gives a database of an earlier revision of the
/// multi-person migrations their final schema.
/// `user_version` then becomes the app migration it already holds in this
/// numbering.
///
/// `numbering` is this build's ([`NUMBERING`]), or in tests a later one's.
pub(super) fn adopt_earlier_flutter_app(
    tx: &rusqlite::Transaction<'_>,
    numbering: &Numbering<'_>,
) -> clax_core::Result<Option<(u32, u32)>> {
    let app_first = numbering.app_first();
    let version: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if !has_mark(tx, &APP_MIGRATION_MARKS[0])? {
        return Ok(None);
    }
    let refuse = |why: String| clax_core::CoreError::MigrationFailed {
        at: version,
        detail: format!(
            "a database from an earlier Clax app build at version {version} {why}; it was left unchanged"
        ),
    };
    let asked = has_column(tx, "questions", "thread_id")?;
    // How many of main's migrations above the where-to-ask one the
    // database holds: the first `main` of them, and none after.
    let mut main = 0;
    for (i, mark) in numbering.main_marks.iter().enumerate() {
        if !has_mark(tx, mark)? {
            continue;
        }
        if i > main {
            let lacked = numbering.main_marks[main];
            return Err(refuse(format!(
                "holds the {mark} but lacks the {lacked} of an earlier migration"
            )));
        }
        main += 1;
    }
    if asked && main == numbering.main_marks.len() {
        return Ok(None);
    }
    if !asked && main > 0 {
        let mark = numbering.main_marks[0];
        return Err(refuse(format!(
            "holds the {mark} but lacks questions.thread_id of an earlier migration"
        )));
    }
    let hosted = has_table(tx, "hosted_setup")?;
    let people = has_table(tx, "invites")?;
    // The first app migration in the database's numbering, and the first
    // migration of this list's it lacks below the app's.
    let (first, below) = if asked {
        let first = WHERE_TO_ASK_MIGRATION + 1 + main as u32;
        (first, first)
    } else if hosted {
        (REPAIR_MIGRATION + 1, WHERE_TO_ASK_MIGRATION)
    } else if people {
        (INBOX_MIGRATION + 1, HOSTED_MIGRATION)
    } else {
        (PEOPLE_MIGRATION, PEOPLE_MIGRATION)
    };
    let last = first + APP_MIGRATION_MARKS.len() as u32 - 1;
    if !(first..=last).contains(&version) {
        return Err(refuse(format!(
            "holds the app's tables, but its version is not one of that build's ({first} to {last})"
        )));
    }
    let held = (version - first + 1) as usize;
    for (i, mark) in APP_MIGRATION_MARKS.iter().enumerate() {
        if has_mark(tx, mark)? != (i < held) {
            return Err(refuse(format!(
                "{} the {mark}",
                if i < held { "lacks" } else { "already has" }
            )));
        }
    }
    for v in below..app_first {
        tx.execute_batch(numbering.migrations[v as usize - 1])?;
        rust_step(tx, v)?;
    }
    let now = app_first - 1 + held as u32;
    tx.pragma_update(None, "user_version", now)?;
    Ok(Some((version, now)))
}

/// Whether the database holds `mark`.
pub(crate) fn has_mark(conn: &rusqlite::Connection, mark: &Mark<'_>) -> clax_core::Result<bool> {
    match *mark {
        Mark::Table(name) => has_table(conn, name),
        Mark::Column(table, column) => has_column(conn, table, column),
        Mark::Index(name) => Ok(conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
            [name],
            |r| r.get(0),
        )?),
        Mark::Row { table, predicate } => {
            if !has_table(conn, table)? {
                return Ok(false);
            }
            let table = table.replace('"', "\"\"");
            Ok(conn.query_row(
                &format!("SELECT EXISTS (SELECT 1 FROM \"{table}\" WHERE {predicate})"),
                [],
                |r| r.get(0),
            )?)
        }
    }
}

/// Runs the Rust part of the migration that brings the schema to
/// `version`, in that migration's transaction, after its SQL.
pub(super) fn rust_step(tx: &rusqlite::Transaction<'_>, version: u32) -> clax_core::Result<()> {
    if version == AUDIT_MIGRATION {
        install_id(tx)?;
    }
    if version == PEOPLE_MIGRATION {
        legacy_viewer_cookies(tx)?;
    }
    if version == VISIBILITY_MIGRATION {
        unconnected_approval(tx)?;
    }
    if version == REPAIR_MIGRATION {
        repair(tx)?;
    }
    Ok(())
}

/// The owner's one approval of `*unconnected` everywhere (migration 25).
fn unconnected_approval(tx: &rusqlite::Transaction<'_>) -> clax_core::Result<()> {
    tx.execute(
        "INSERT INTO approvals (id, approver, author, scope, created_at, created_via)
         VALUES (?1, NULL, ?2, 'everywhere', ?3, 'migration')",
        rusqlite::params![
            clax_core::new_ulid(),
            clax_core::visibility::UNCONNECTED,
            clax_core::time::stamp_now()
        ],
    )?;
    Ok(())
}

/// The tables migrations 24, 25 and 30 make, as their final text makes
/// them, for [`repair`] to make where missing.
const REPAIR_TABLES: &str = "CREATE TABLE IF NOT EXISTS viewer_cookies (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        created_at TEXT NOT NULL,
        legacy INTEGER NOT NULL DEFAULT 0,
        replaced_at TEXT
    );
    CREATE TABLE IF NOT EXISTS invites (
        id TEXT PRIMARY KEY,
        code_sha256 TEXT NOT NULL UNIQUE,
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        used_at TEXT,
        grant_id TEXT,
        revoked_at TEXT
    );
    CREATE TABLE IF NOT EXISTS grants (
        id TEXT PRIMARY KEY,
        viewer_id TEXT REFERENCES viewers(id),
        status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'revoked', 'expired')),
        via TEXT NOT NULL CHECK (via IN ('invite', 'request')),
        client_key TEXT NOT NULL,
        client_name TEXT NOT NULL,
        peer_ip TEXT NOT NULL,
        user_code_sha256 TEXT,
        wrong_codes INTEGER NOT NULL DEFAULT 0,
        continue_sha256 TEXT NOT NULL,
        continue_t INTEGER,
        continue_nonce TEXT,
        manage_sha256 TEXT,
        narrow_json TEXT,
        created_at TEXT NOT NULL,
        decided_at TEXT,
        last_used_at TEXT,
        ended_at TEXT,
        ended_by TEXT,
        browser_sha256 TEXT,
        browser_expires_at TEXT
    );
    CREATE TABLE IF NOT EXISTS grant_tokens (
        sha256 TEXT PRIMARY KEY,
        grant_id TEXT NOT NULL REFERENCES grants(id),
        created_at TEXT NOT NULL,
        revoked_at TEXT
    );
    CREATE TABLE IF NOT EXISTS session_elevations (
        session_id TEXT NOT NULL,
        granter TEXT,
        access_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        version TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS approvals (
        id TEXT PRIMARY KEY,
        approver TEXT,
        author TEXT NOT NULL,
        scope TEXT NOT NULL CHECK (scope IN ('thread', 'artifact', 'everywhere')),
        artifact_id TEXT,
        thread_id TEXT,
        created_at TEXT NOT NULL,
        created_via TEXT NOT NULL CHECK (created_via IN ('shell', 'extension', 'connect', 'migration')),
        request_id TEXT,
        revoked_at TEXT
    );
    CREATE TABLE IF NOT EXISTS approval_requests (
        id TEXT PRIMARY KEY,
        approver TEXT,
        author TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        thread_id TEXT,
        session_id TEXT,
        cause TEXT NOT NULL CHECK (cause IN ('ask', 'routing')),
        count INTEGER NOT NULL DEFAULT 1,
        status TEXT NOT NULL DEFAULT 'open'
            CHECK (status IN ('open', 'approved', 'declined', 'withdrawn')),
        decided_scope TEXT CHECK (decided_scope IN ('thread', 'artifact', 'everywhere')),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        decided_at TEXT
    );
    CREATE TABLE IF NOT EXISTS doc_authors (
        artifact_id TEXT NOT NULL,
        path TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path, author)
    );
    CREATE TABLE IF NOT EXISTS page_authors (
        kind TEXT NOT NULL CHECK (kind IN ('comment', 'asset')),
        id TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (kind, id, author)
    );
    CREATE TABLE IF NOT EXISTS remote_assets (
        remote_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        asset_id TEXT NOT NULL,
        url TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (remote_id, remote_artifact_id, asset_id)
    );
    CREATE TABLE IF NOT EXISTS publish_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        n INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );";

/// The columns migrations 24 to 26 and 36 add to existing tables, and
/// those a table of 24 gained after its first copy, as (table, column,
/// definition) in their final text, for [`repair`] to add where missing.
const REPAIR_COLUMNS: &[(&str, &str, &str)] = &[
    ("viewers", "person", "INTEGER NOT NULL DEFAULT 0"),
    ("viewers", "person_revoked_at", "TEXT"),
    ("viewers", "rights_json", "TEXT"),
    ("viewer_cookies", "replaced_at", "TEXT"),
    ("grants", "continue_t", "INTEGER"),
    ("grants", "continue_nonce", "TEXT"),
    ("grants", "browser_sha256", "TEXT"),
    ("grants", "browser_expires_at", "TEXT"),
    ("sessions", "grant_id", "TEXT"),
    ("sessions", "person", "TEXT"),
    ("sessions", "name", "TEXT"),
    (
        "sessions",
        "name_source",
        "TEXT CHECK (name_source IN ('derived', 'agent', 'person'))",
    ),
    ("sessions", "narrow_json", "TEXT"),
    (
        "sessions",
        "ended_reason",
        "TEXT\n        CHECK (ended_reason IN ('idle', 'exit', 'revoked', 'person'))",
    ),
    ("sessions", "ended_watches_json", "TEXT"),
    ("sessions", "secret_sha256", "TEXT"),
    ("session_elevations", "version", "TEXT NOT NULL DEFAULT ''"),
    ("artifacts", "person", "TEXT"),
    ("versions", "person", "TEXT"),
    ("versions", "by_page", "INTEGER NOT NULL DEFAULT 0"),
    ("versions", "file_authors_json", "TEXT"),
    ("assets", "person", "TEXT"),
    ("feedback", "placeholder_at", "TEXT"),
    ("feedback", "no_access_at", "TEXT"),
    ("artifacts", "text_authors_json", "TEXT"),
    ("send_batches", "sender", "TEXT"),
    ("threads", "resolved_by_person", "TEXT"),
    ("questions", "person", "TEXT"),
    ("watches", "relay_name", "TEXT"),
    ("watches", "relay_harness", "TEXT"),
];

/// `push_approvals` as migration 30 makes it, for [`repair`] to rebuild one
/// without `upto`: its rows are copied out, and back into the table made
/// with migration 30's text (not renamed into place, which would store its
/// name quoted). A kept row approves up to version 0, so its push asks
/// again rather than reach a version no one approved.
const REPAIR_PUSH_APPROVALS: &str = "CREATE TABLE push_approvals_37 AS
        SELECT question_id, artifact_id, remote_id, session_id, created_at, used_at
        FROM push_approvals;
    DROP TABLE push_approvals;
    CREATE TABLE push_approvals (
        question_id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        upto INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        used_at TEXT
    );
    INSERT INTO push_approvals (question_id, artifact_id, remote_id, session_id, upto,
        created_at, used_at)
        SELECT question_id, artifact_id, remote_id, session_id, 0, created_at, used_at
        FROM push_approvals_37;
    DROP TABLE push_approvals_37;";

/// `inbox_items` as migration 30 rebuilds it, with the `push` kind, for
/// [`repair`] to rebuild one without it, keeping every `seq` so the
/// contentless `inbox_fts` keeps naming its items; its indexes follow in
/// [`REPAIR_INDEXES`].
const REPAIR_INBOX: &str = "CREATE TABLE inbox_items_37 (
        seq INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK (kind IN ('reply', 'version', 'published', 'question', 'finished',
                                           'approval', 'connection', 'ended', 'push')),
        key TEXT NOT NULL UNIQUE,
        recipient TEXT,
        artifact_id TEXT,
        thread_id TEXT,
        comment_id TEXT,
        version_n INTEGER,
        question_id TEXT,
        session_id TEXT,
        harness TEXT,
        request_id TEXT,
        grant_id TEXT,
        detail_json TEXT,
        created_at TEXT NOT NULL,
        read_at TEXT
    );
    INSERT INTO inbox_items_37 SELECT seq, id, kind, key, recipient, artifact_id, thread_id,
        comment_id, version_n, question_id, session_id, harness, request_id, grant_id, detail_json,
        created_at, read_at FROM inbox_items;
    DROP TABLE inbox_items;
    ALTER TABLE inbox_items_37 RENAME TO inbox_items;";

/// The indexes migrations 24 to 26, 30 and 35 make, as their final text
/// makes them, for [`repair`] to make where missing, after its columns.
const REPAIR_INDEXES: &str = "CREATE INDEX IF NOT EXISTS viewer_cookies_by_viewer ON viewer_cookies(viewer_id);
    CREATE UNIQUE INDEX IF NOT EXISTS grants_browser ON grants(browser_sha256) WHERE browser_sha256 IS NOT NULL;
    CREATE UNIQUE INDEX IF NOT EXISTS grants_live_key ON grants(client_key) WHERE status IN ('pending', 'approved');
    CREATE INDEX IF NOT EXISTS grants_by_viewer ON grants(viewer_id, status);
    CREATE INDEX IF NOT EXISTS grants_pending ON grants(created_at) WHERE status = 'pending';
    CREATE INDEX IF NOT EXISTS grant_tokens_by_grant ON grant_tokens(grant_id);
    CREATE UNIQUE INDEX IF NOT EXISTS session_elevations_key ON session_elevations(session_id, COALESCE(granter, ''));
    CREATE INDEX IF NOT EXISTS session_elevations_by_granter ON session_elevations(granter) WHERE granter IS NOT NULL;
    CREATE UNIQUE INDEX IF NOT EXISTS sessions_live_name ON sessions(name COLLATE NOCASE)
        WHERE ended_at IS NULL AND name IS NOT NULL;
    CREATE INDEX IF NOT EXISTS sessions_by_grant ON sessions(grant_id, harness, parent_pid) WHERE grant_id IS NOT NULL;
    CREATE UNIQUE INDEX IF NOT EXISTS approvals_live ON approvals(
        COALESCE(approver, ''), author, scope, COALESCE(artifact_id, ''), COALESCE(thread_id, ''))
        WHERE revoked_at IS NULL;
    CREATE INDEX IF NOT EXISTS approvals_by_approver ON approvals(COALESCE(approver, ''), author) WHERE revoked_at IS NULL;
    CREATE UNIQUE INDEX IF NOT EXISTS approval_requests_open ON approval_requests(
        COALESCE(approver, ''), author, artifact_id, COALESCE(thread_id, ''))
        WHERE status = 'open';
    CREATE INDEX IF NOT EXISTS approval_requests_by_session ON approval_requests(session_id, status);
    CREATE INDEX IF NOT EXISTS approval_requests_by_target ON approval_requests(
        COALESCE(approver, ''), artifact_id, author, status);
    CREATE INDEX IF NOT EXISTS approval_requests_by_thread ON approval_requests(thread_id, status);
    CREATE INDEX IF NOT EXISTS inbox_unread ON inbox_items(COALESCE(recipient, ''), seq) WHERE read_at IS NULL;
    CREATE INDEX IF NOT EXISTS inbox_by_recipient ON inbox_items(COALESCE(recipient, ''), seq);
    CREATE INDEX IF NOT EXISTS inbox_by_artifact ON inbox_items(COALESCE(recipient, ''), artifact_id, seq);
    CREATE INDEX IF NOT EXISTS inbox_by_kind ON inbox_items(COALESCE(recipient, ''), kind, seq);
    CREATE INDEX IF NOT EXISTS inbox_by_harness ON inbox_items(COALESCE(recipient, ''), harness, seq);
    CREATE INDEX IF NOT EXISTS inbox_by_session ON inbox_items(COALESCE(recipient, ''), session_id, seq);
    CREATE INDEX IF NOT EXISTS inbox_by_created ON inbox_items(created_at, seq);
    CREATE INDEX IF NOT EXISTS inbox_by_thread ON inbox_items(thread_id) WHERE thread_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS inbox_by_question ON inbox_items(question_id) WHERE question_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS inbox_by_request ON inbox_items(request_id) WHERE request_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS viewers_people ON viewers(public_id) WHERE person = 1;
    CREATE INDEX IF NOT EXISTS questions_open_by_person ON questions(COALESCE(person, ''), created_at, id)
        WHERE status = 'open';
    CREATE INDEX IF NOT EXISTS questions_closed_by_person ON questions(COALESCE(person, ''), closed_at, id)
        WHERE status <> 'open';
    CREATE INDEX IF NOT EXISTS question_keys_by_question ON question_keys(question_id, grant_id);";

fn has_table(tx: &rusqlite::Connection, table: &str) -> clax_core::Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |r| r.get(0),
    )?)
}

fn has_column(tx: &rusqlite::Connection, table: &str, column: &str) -> clax_core::Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        [table, column],
        |r| r.get(0),
    )?)
}

/// Brings a database that builds with earlier copies of migrations 24 to
/// 26, 30 and 36 migrated to the schema the final ones make (migration
/// 37); changes nothing on a database they made. Each step checks what is
/// there first, so a second run changes nothing either.
///
/// - Makes the missing tables ([`REPAIR_TABLES`]) and adds the missing
///   columns ([`REPAIR_COLUMNS`]), then makes the missing indexes
///   ([`REPAIR_INDEXES`]).
/// - Where it makes `viewer_cookies`, keeps each viewer's row-ID cookie
///   working as migration 24's step does ([`legacy_viewer_cookies`]);
///   where it makes `approvals`, makes migration 25's approval
///   ([`unconnected_approval`]); where it adds `questions.person`, fills
///   it in from each question's session as migration 26 does.
/// - Where `sessions.elevate_json` exists, the earlier copy of migration
///   24's owner elevation of an agent, makes each live session's into its
///   owner row of `session_elevations` (granter NULL, a new version), as
///   the owner's raise makes it now, and drops the column. An ended
///   session's is not kept, as an elevation ends with its session.
/// - Rebuilds `push_approvals` without `upto` ([`REPAIR_PUSH_APPROVALS`])
///   and `inbox_items` without the `push` kind ([`REPAIR_INBOX`]).
fn repair(tx: &rusqlite::Transaction<'_>) -> clax_core::Result<()> {
    let had_cookies = has_table(tx, "viewer_cookies")?;
    let had_approvals = has_table(tx, "approvals")?;
    tx.execute_batch(REPAIR_TABLES)?;
    for &(table, column, def) in REPAIR_COLUMNS {
        if has_column(tx, table, column)? {
            continue;
        }
        tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {def};"))?;
        if (table, column) == ("questions", "person") {
            tx.execute_batch(
                "UPDATE questions SET person = (SELECT s.person FROM sessions s WHERE s.id = questions.session_id);",
            )?;
        }
    }
    if !had_cookies {
        legacy_viewer_cookies(tx)?;
    }
    if !had_approvals {
        unconnected_approval(tx)?;
    }
    if has_column(tx, "sessions", "elevate_json")? {
        let raised: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, elevate_json FROM sessions
                 WHERE elevate_json IS NOT NULL AND ended_at IS NULL",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let now = clax_core::time::stamp_now();
        for (sid, access) in raised {
            tx.execute(
                "INSERT OR IGNORE INTO session_elevations
                    (session_id, granter, access_json, created_at, version)
                 VALUES (?1, NULL, ?2, ?3, ?4)",
                rusqlite::params![sid, access, now, clax_core::new_ulid()],
            )?;
        }
        tx.execute_batch("ALTER TABLE sessions DROP COLUMN elevate_json;")?;
    }
    if has_table(tx, "push_approvals")? && !has_column(tx, "push_approvals", "upto")? {
        tx.execute_batch(REPAIR_PUSH_APPROVALS)?;
    }
    let inbox: String = tx.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'inbox_items'",
        [],
        |r| r.get(0),
    )?;
    if !inbox.contains("'push'") {
        tx.execute_batch(REPAIR_INBOX)?;
    }
    tx.execute_batch(REPAIR_INDEXES)?;
    Ok(())
}

/// Mints the install ID once: 128 random bits in lowercase hex. It holds
/// no timestamp, so an exported reference does not date the install.
/// Keeps every existing viewer's cookie, its row ID, working (spec
/// 2026-10-08 §21.4): its SHA-256 as a `legacy` cookie of that viewer. The
/// owner's row is skipped: no browser holds its ID as a cookie.
fn legacy_viewer_cookies(tx: &rusqlite::Transaction<'_>) -> clax_core::Result<()> {
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, created_at FROM viewers WHERE owner = 0")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, created_at) in rows {
        tx.execute(
            "INSERT OR IGNORE INTO viewer_cookies (sha256, viewer_id, created_at, legacy)
             VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![clax_core::secret::sha256_hex(&id), id, created_at],
        )?;
    }
    Ok(())
}

fn install_id(tx: &rusqlite::Transaction<'_>) -> clax_core::Result<()> {
    let id: String = rand::random::<[u8; 16]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    tx.execute(
        "INSERT OR IGNORE INTO install (k, v) VALUES ('id', ?1)",
        [id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        APP_MIGRATION, AUDIT_MIGRATION, INBOX_MIGRATION, MAIN_MARKS_ABOVE_38, MIGRATIONS,
        PEOPLE_MIGRATION, REPAIR_MIGRATION, VISIBILITY_MIGRATION, WHERE_TO_ASK_MIGRATION,
    };
    use crate::Store;
    use crate::test_util::DAEMON;
    use clax_core::{Home, is_public_id};
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
    fn audit_migration_creates_audit_tables() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let before = AUDIT_MIGRATION as usize - 1;
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..before] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", before as u32)
                .unwrap();
            c.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('a1', 'T', 'x', 'x', 1, '1');
                 INSERT INTO versions (artifact_id, n, created_at, files_json) VALUES ('a1', 1, 'x', '{}');
                 INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, agent_handle)
                    VALUES ('s1', 'claude', '/w', 'x', 'x', 'a_1');",
            )
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let (schema, old) = st
            .with_read(|c| {
                let mut q = c.prepare(
                    "SELECT name FROM sqlite_schema WHERE name LIKE 'audit%' OR name = 'install'
                     ORDER BY name",
                )?;
                let names = q
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let old: (Option<String>, Option<String>, i64) = c.query_row(
                    "SELECT (SELECT content_sha256 FROM versions WHERE artifact_id = 'a1'),
                            (SELECT transcript_path FROM sessions WHERE id = 's1'),
                            (SELECT COUNT(*) FROM audit_events WHERE backfilled = 1)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                Ok((names, old))
            })
            .unwrap();
        assert_eq!(
            schema,
            [
                "audit_events",
                "audit_events_artifact",
                "audit_events_artifact2",
                "audit_events_at",
                "audit_events_call",
                "audit_events_session",
                "install",
            ]
        );
        // The backfill hashed the version (no files: the empty manifest)
        // and recorded the session, the artifact and its version.
        assert_eq!(
            old,
            (
                Some(format!("sha256:{}", clax_core::audit::sha256_hex(b""))),
                None,
                3
            )
        );
        assert!(crate::audit::tests::is_install_id(
            &st.install_id().unwrap()
        ));
        let version: u32 = st
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32);
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
            st.claim_for_owner(clax_core::audit::Via::Shell, COOKIE)
                .unwrap(),
            crate::viewers::Claim::Nothing,
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
    fn migration_38_adds_where_to_ask() {
        let (_d, st) = crate::test_util::store();
        let cols = |t: &str| -> Vec<String> {
            st.with_read(|c| {
                let mut s = c.prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))?;
                Ok(s.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .unwrap()
        };
        let q = cols("questions");
        for c in ["thread_id", "in_terminal", "pulled_at"] {
            assert!(q.iter().any(|x| x == c), "questions.{c}");
        }
        let s = cols("sessions");
        for c in [
            "terminal_at",
            "terminal_inferred",
            "turn_at",
            "delivered_at",
            "delivered_artifact_id",
            "delivered_thread_id",
            "delivered_since_turn",
            "turn_open",
            "queued_since_turn",
            "delivered_written_at",
            "delivered_act",
        ] {
            assert!(s.iter().any(|x| x == c), "sessions.{c}");
        }
    }

    #[test]
    fn migration_20_adds_questions_to_a_19_database() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..19] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", 19).unwrap();
        }
        let st = Store::open(&home).unwrap();
        let n: i64 = st
            .with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM questions", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 0);
        let indexes: Vec<String> = st
            .with_read(|c| {
                let mut s = c.prepare(
                    "SELECT name FROM sqlite_schema WHERE type = 'index'
                     AND tbl_name = 'questions' AND sql IS NOT NULL ORDER BY name",
                )?;
                Ok(s.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(
            indexes,
            [
                "questions_by_artifact",
                "questions_by_session",
                "questions_by_status",
                "questions_by_tool_use",
                "questions_closed",
                // Migration 26's.
                "questions_closed_by_person",
                "questions_open_by_person",
            ]
        );
    }

    /// The index of the inbox migration in [`MIGRATIONS`] (its version is one more).
    fn inbox_migration() -> usize {
        MIGRATIONS
            .iter()
            .position(|m| m.contains("CREATE TABLE inbox_items"))
            .expect("the inbox migration")
    }

    /// A database at the version before the inbox: artifact A (published by
    /// an agent, then two more agent versions) and B (made by the same
    /// agent); live page L; a thread on A where the owner (when `owner`)
    /// commented and the agent replied; a stranger's thread on A with an
    /// agent reply.
    fn before_inbox(home: &Home, owner: bool) {
        home.ensure_dirs().unwrap();
        let v = inbox_migration();
        let c = Connection::open(home.db_path()).unwrap();
        for sql in &MIGRATIONS[..v] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", v as u32).unwrap();
        let owner_pid = if owner { "u_owner" } else { "u_nobody" };
        if owner {
            c.execute_batch(&format!(
                "INSERT INTO viewers (id, display_name, created_at, public_id, owner, claimed)
                    VALUES ('{COOKIE}', 'Alex', 'x', 'u_owner', 1, 1);"
            ))
            .unwrap();
        }
        c.execute_batch(&format!(
            "INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, agent_handle)
                VALUES ('{SID}', 'claude', '/w', 'x', 'x', 'a_1');
             INSERT INTO artifacts (id, title, description, created_at, updated_at, current_version, owner_session_id, contract_version)
                VALUES ('aaaaaaaaaaaa', 'Quarterly Review', 'Numbers', '2026-01-01T00:00:01.000Z', 'x', 3, '{SID}', '0'),
                       ('bbbbbbbbbbbb', 'Roadmap', NULL, '2026-01-01T00:00:09.000Z', 'x', 1, '{SID}', '0');
             INSERT INTO artifacts (id, title, created_at, updated_at, current_version, owner_session_id, contract_version, deleted_at)
                VALUES ('dddddddddddd', 'Deleted', '2026-01-01T00:00:08.000Z', 'x', 1, '{SID}', '0', 'x');
             INSERT INTO artifacts (id, title, created_at, updated_at, current_version, owner_session_id, contract_version, kind)
                VALUES ('llllllllllll', 'Live', '2026-01-01T00:00:02.000Z', 'x', 1, '{SID}', '0', 'live');
             INSERT INTO versions (artifact_id, n, created_at, session_id, files_json, note) VALUES
                ('aaaaaaaaaaaa', 1, '2026-01-01T00:00:01.000Z', '{SID}', '{{}}', 'Firstnote'),
                ('dddddddddddd', 1, '2026-01-01T00:00:08.000Z', '{SID}', '{{}}', NULL),
                ('aaaaaaaaaaaa', 2, '2026-01-01T00:00:05.000Z', '{SID}', '{{}}', 'Two columns'),
                ('aaaaaaaaaaaa', 3, '2026-01-01T00:00:07.000Z', '{SID}', '{{}}', NULL),
                ('bbbbbbbbbbbb', 1, '2026-01-01T00:00:09.000Z', '{SID}', '{{}}', NULL),
                ('llllllllllll', 1, '2026-01-01T00:00:02.000Z', NULL, '{{}}', NULL);
             INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at) VALUES
                ('t1', 'aaaaaaaaaaaa', 1, '{{\"quote\": \"Headline\"}}', '2026-01-01T00:00:03.000Z'),
                ('t2', 'aaaaaaaaaaaa', 1, '{{}}', '2026-01-01T00:00:03.500Z');
             INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, via_session_id, body, created_at) VALUES
                ('c1', 't1', 'viewer', 'Alex', '{owner_pid}', NULL, 'make it blue', '2026-01-01T00:00:03.000Z'),
                ('c2', 't1', 'agent', 'claude', NULL, '{SID}', 'Done: the header is teal', '2026-01-01T00:00:04.000Z'),
                ('c3', 't2', 'viewer', 'Mia', 'u_stranger', NULL, 'and the footer?', '2026-01-01T00:00:03.500Z'),
                ('c4', 't2', 'agent', 'claude', NULL, '{SID}', 'footer fixed', '2026-01-01T00:00:06.000Z');
             INSERT INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                VALUES ('aaaaaaaaaaaa', 2, 't1', 'explicit', '2026-01-01T00:00:05.000Z');"
        ))
        .unwrap();
    }

    fn inbox_rows(st: &Store) -> Vec<(String, String, Option<String>, bool)> {
        st.with_read(|c| {
            let mut s = c.prepare(
                "SELECT id, kind, artifact_id, read_at IS NOT NULL FROM inbox_items ORDER BY seq",
            )?;
            Ok(
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            )
        })
        .unwrap()
    }

    #[test]
    fn the_inbox_migration_fills_the_history_read() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        before_inbox(&home, true);
        let st = Store::open(&home).unwrap();
        let rows = inbox_rows(&st);
        let kinds: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|r| (r.1.as_str(), r.2.as_deref()))
            .collect();
        // In time order: A published, the reply, v2, v3, B published.
        assert_eq!(
            kinds,
            [
                ("published", Some("aaaaaaaaaaaa")),
                ("reply", Some("aaaaaaaaaaaa")),
                ("version", Some("aaaaaaaaaaaa")),
                ("version", Some("aaaaaaaaaaaa")),
                ("published", Some("bbbbbbbbbbbb")),
            ]
        );
        assert!(rows.iter().all(|r| r.3), "the history is read");
        for (id, ..) in &rows {
            assert!(
                id.len() == 25
                    && id.starts_with('b')
                    && id[1..]
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{id}"
            );
        }
        let found = |q: &str| -> Vec<String> {
            st.with_read(|c| {
                let mut s = c.prepare(
                    "SELECT i.kind FROM inbox_fts f JOIN inbox_items i ON i.seq = f.rowid
                     WHERE inbox_fts MATCH ?1 ORDER BY f.rowid",
                )?;
                Ok(s.query_map([q], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap()
        };
        assert_eq!(found("teal"), ["reply"]);
        assert_eq!(found("columns"), ["version"]);
        assert_eq!(found("headline"), ["version"], "the quotes it addressed");
        assert_eq!(found("numbers"), ["published"], "by the description");
        assert_eq!(
            found("firstnote"),
            Vec::<String>::new(),
            "not by version 1's note"
        );
        assert_eq!(found("deleted"), Vec::<String>::new(), "a deleted artifact");
        assert_eq!(found("roadmap"), ["published"]);
        assert_eq!(found("footer"), Vec::<String>::new(), "a stranger's thread");
        assert_eq!(st.inbox_unread(None).unwrap(), 0);
        assert_eq!(st.inbox_list(&Default::default()).unwrap().0.len(), 5);
    }

    #[test]
    fn the_inbox_migration_without_an_owner_fills_only_published() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        before_inbox(&home, false);
        let st = Store::open(&home).unwrap();
        let kinds: Vec<String> = inbox_rows(&st).into_iter().map(|r| r.1).collect();
        assert_eq!(kinds, ["published", "published"]);
        let temp: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM temp.sqlite_schema WHERE name = '_owner'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(temp, 0);
    }

    #[test]
    fn sqlite_version_has_contentless_delete() {
        let c = Connection::open_in_memory().unwrap();
        let v: String = c
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .unwrap();
        let parts: Vec<u32> = v.split('.').map(|p| p.parse().unwrap()).collect();
        assert!(parts[..2] >= [3, 43][..], "SQLite {v}");
        c.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(x, content='', contentless_delete=1);
             INSERT INTO t (rowid, x) VALUES (1, 'a b');
             DELETE FROM t WHERE rowid = 1;",
        )
        .unwrap();
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
            // Every migration before joined sites (19).
            let before = 18;
            assert!(MIGRATIONS[before].contains("live_sites"));
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
        let id = clax_core::ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        let admin = clax_core::db::Caller::new(clax_core::db::Level::Admin, None);
        let pin = crate::docs::Pin {
            if_version: None,
            lww: true,
        };
        let w = st
            .doc_set(DAEMON, &id, "t/c", serde_json::json!({}), pin, &admin)
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
            matches!(err, clax_core::CoreError::SchemaNewer { found, known }
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

    /// The draft migration (the app's second) adds the draft columns: every
    /// artifact it finds stays listed (`hidden = 0`) and is a draft of
    /// nothing.
    #[test]
    fn the_draft_migration_keeps_every_artifact_listed() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..APP_MIGRATION as usize] {
                c.execute_batch(sql).unwrap();
            }
            c.pragma_update(None, "user_version", APP_MIGRATION)
                .unwrap();
            c.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 1, '0');",
            )
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let row: (i64, Option<String>) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT hidden, draft_of FROM artifacts WHERE id = '7q3k9mzx2b4t'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(row, (0, None));
        assert!(!st.is_hidden("7q3k9mzx2b4t"));
        assert_eq!(st.list_artifacts().unwrap().len(), 1);
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

    #[test]
    fn migration_24_keeps_rows_and_makes_them_the_owners() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let before = PEOPLE_MIGRATION as usize - 1;
        assert!(MIGRATIONS[before].contains("CREATE TABLE grants"));
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..before] {
                c.execute_batch(sql).unwrap();
            }
            // The audit migration's Rust step, which this raw replay skips.
            c.execute(
                "INSERT INTO install (k, v) VALUES ('id', ?1)",
                ["0".repeat(32)],
            )
            .unwrap();
            c.pragma_update(None, "user_version", before as u32)
                .unwrap();
            c.execute_batch(&format!(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 2, '0');
                 INSERT INTO versions (artifact_id, n, created_at, files_json) VALUES ('7q3k9mzx2b4t', 1, 'x', '{{}}');
                 INSERT INTO versions (artifact_id, n, created_at, files_json) VALUES ('7q3k9mzx2b4t', 2, 'x', '{{}}');
                 INSERT INTO assets (id, artifact_id, content_type, size, ext, created_at)
                    VALUES ('as1', '7q3k9mzx2b4t', 'image/png', 1, 'png', 'x');
                 INSERT INTO sessions (id, harness, harness_session_id, cwd, started_at, last_seen_at, agent_handle)
                    VALUES ('{SID}', 'claude', 'h1', '/w', 'x', 'x', 'a_00');
                 INSERT INTO viewers (id, public_id, display_name, created_at)
                    VALUES ('{COOKIE}', 'u_0000000000000000000000', 'Lan', 'x');"
            ))
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let sessions = st.list_sessions(false).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions.iter().all(|s| s.person.is_none()
            && s.grant_id.is_none()
            && s.name.is_none()
            && s.name_source.is_none()));
        let people: Vec<Option<String>> = st
            .with_read(|c| {
                let mut stmt = c.prepare(
                    "SELECT person FROM artifacts UNION ALL SELECT person FROM versions
                     UNION ALL SELECT file_authors_json FROM versions
                     UNION ALL SELECT person FROM assets",
                )?;
                Ok(stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(people.len(), 6);
        assert!(people.iter().all(Option::is_none));
        // A main viewer's cookie, its row ID, keeps naming it, as a legacy
        // cookie kept by its hash (spec 2026-10-08 §21.4).
        let v = st.get_viewer(COOKIE).unwrap().unwrap();
        assert!(!v.person && v.person_revoked_at.is_none());
        let kept: (String, bool) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT sha256, legacy FROM viewer_cookies WHERE viewer_id = ?1",
                    [COOKIE],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(kept, (clax_core::secret::sha256_hex(COOKIE), true));
        let json = serde_json::to_value(&v).unwrap();
        assert!(
            json.get("person").is_none(),
            "unchanged JSON for a viewer: {json}"
        );
        let s = serde_json::to_value(&sessions[0]).unwrap();
        assert!(
            s.get("person").is_none() && s.get("name").is_none() && s.get("grant_id").is_none()
        );
    }
    #[test]
    fn migration_36_gives_members_with_the_earlier_own_space_rights_clax_ask() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let before = 35;
        assert!(MIGRATIONS[before].contains("clax-ask"));
        let earlier = clax_core::rights::Rights::parse(&serde_json::json!([
            {"type": "clax-artifacts", "actions": ["create"], "locations": ["*"]},
            {"type": "clax-artifacts", "actions": ["read", "update", "delete"], "locations": ["own"]},
            {"type": "clax-comments", "actions": clax_core::rights::COMMENT_ACTIONS, "locations": ["own"]},
            {"type": "clax-db", "actions": ["read", "write"], "level": "admin", "locations": ["own"]},
        ]))
        .unwrap();
        let custom = super::super::people::write_rights(
            &clax_core::rights::Rights::preset(clax_core::rights::PRESET_READER).unwrap(),
        );
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..before] {
                c.execute_batch(sql).unwrap();
            }
            c.execute(
                "INSERT INTO install (k, v) VALUES ('id', ?1)",
                ["0".repeat(32)],
            )
            .unwrap();
            c.pragma_update(None, "user_version", before as u32)
                .unwrap();
            for (id, rights) in [
                ("v1", Some(super::super::people::write_rights(&earlier))),
                ("v2", Some(custom.clone())),
                ("v3", None),
            ] {
                c.execute(
                    "INSERT INTO viewers (id, public_id, display_name, created_at, person, rights_json)
                        VALUES (?1, ?2, ?1, 'x', 1, ?3)",
                    rusqlite::params![id, format!("u_{id}"), rights],
                )
                .unwrap();
            }
        }
        let st = Store::open(&home).unwrap();
        let rights = |id: &str| -> Option<String> {
            st.with_read(|c| {
                Ok(
                    c.query_row("SELECT rights_json FROM viewers WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap()
        };
        assert_eq!(
            rights("v1"),
            Some(super::super::people::write_rights(
                &crate::github::member_rights()
            )),
            "a member with the earlier own-space rights gets the current ones"
        );
        assert_eq!(rights("v2"), Some(custom), "rights set otherwise are kept");
        assert_eq!(rights("v3"), None, "default rights stay the default");
    }

    #[test]
    fn migration_25_keeps_rows_the_owners_and_lets_lan_viewers_reach_the_owners_agents() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let before = VISIBILITY_MIGRATION as usize - 1;
        assert!(MIGRATIONS[before].contains("CREATE TABLE approvals"));
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..before] {
                c.execute_batch(sql).unwrap();
            }
            c.execute(
                "INSERT INTO install (k, v) VALUES ('id', ?1)",
                ["0".repeat(32)],
            )
            .unwrap();
            c.pragma_update(None, "user_version", before as u32)
                .unwrap();
            c.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 1, '0');
                 INSERT INTO versions (artifact_id, n, created_at, files_json) VALUES ('7q3k9mzx2b4t', 1, 'x', '{}');
                 INSERT INTO threads (id, artifact_id, version_n, anchor_json, status, created_at)
                    VALUES ('t1', '7q3k9mzx2b4t', 1, '{}', 'open', 'x');
                 INSERT INTO comments (id, thread_id, author_kind, author_name, via_page, body, created_at)
                    VALUES ('c1', 't1', 'viewer', 'Lan', 1, 'b', 'x');
                 INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                    VALUES ('7q3k9mzx2b4t', 'notes/1', 'notes', '{}', 1, 'x');",
            )
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        type Live = Vec<(Option<String>, String, String, String)>;
        let (page_authors, doc_authors, approvals): (i64, i64, Live) = st
            .with_read(|c| {
                let pp = c.query_row("SELECT COUNT(*) FROM page_authors", [], |r| r.get(0))?;
                let da = c.query_row("SELECT COUNT(*) FROM doc_authors", [], |r| r.get(0))?;
                let mut stmt = c.prepare("SELECT approver, author, scope, created_via FROM approvals WHERE revoked_at IS NULL")?;
                let a = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok((pp, da, a))
            })
            .unwrap();
        assert_eq!(
            page_authors, 0,
            "an existing page comment was written on the owner's page"
        );
        let a = &st.live_approvals(None).unwrap()[0];
        assert!(clax_core::is_ulid(&a.id), "{}", a.id);
        let (text, sender): (Option<String>, Option<String>) = st
            .with_read(|c| {
                Ok((
                    c.query_row("SELECT text_authors_json FROM artifacts", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(sender) FROM send_batches", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .map(|n| (n > 0).then(|| "some".to_string()))?,
                ))
            })
            .unwrap();
        assert_eq!(
            (text, sender),
            (None, None),
            "an artifact's text is its creator's; batches have no sender"
        );
        assert_eq!(doc_authors, 0, "an existing document is the owner's");
        assert_eq!(
            approvals,
            vec![(
                None,
                "*unconnected".into(),
                "everywhere".into(),
                "migration".into()
            )]
        );
        assert!(
            st.owner_sees_all().unwrap(),
            "a single-owner install takes the fast path"
        );
    }

    #[test]
    fn migration_26_keeps_seq_ids_keys_read_state_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let before = INBOX_MIGRATION as usize - 1;
        assert!(MIGRATIONS[before].contains("CREATE TABLE inbox_items_26"));
        type Row = (
            i64,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        );
        let rows = |c: &Connection| -> Vec<Row> {
            let mut st = c
                .prepare(
                    "SELECT seq, id, kind, key, artifact_id, detail_json, created_at, read_at
                     FROM inbox_items ORDER BY seq",
                )
                .unwrap();
            st.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
        };
        let old = {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..before] {
                c.execute_batch(sql).unwrap();
            }
            c.execute(
                "INSERT INTO install (k, v) VALUES ('id', ?1)",
                ["0".repeat(32)],
            )
            .unwrap();
            c.pragma_update(None, "user_version", before as u32)
                .unwrap();
            // Gaps in `seq` (as items made and rolled back leave), every
            // kind, read and unread, an item filled in from the history.
            c.execute_batch(
                "INSERT INTO inbox_items (seq, id, kind, key, artifact_id, thread_id, comment_id, created_at, read_at)
                    VALUES (3, 'b0123456789abcdef01234567', 'reply', 'reply:c1', '7q3k9mzx2b4t', 't1', 'c1',
                            '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:01.000Z');
                 INSERT INTO inbox_items (seq, id, kind, key, artifact_id, version_n, session_id, harness, created_at)
                    VALUES (7, '01ARZ3NDEKTSV4RRFFQ69G5FB1', 'published', 'published:7q3k9mzx2b4t', '7q3k9mzx2b4t', 1,
                            's1', 'claude', '2026-01-01T00:00:02.000Z');
                 INSERT INTO inbox_items (seq, id, kind, key, artifact_id, session_id, harness, detail_json, created_at)
                    VALUES (8, '01ARZ3NDEKTSV4RRFFQ69G5FB2', 'finished', 'finished:k', '7q3k9mzx2b4t', 's1', 'pi',
                            '{\"message\":\"done\",\"thread_ids\":[]}', '2026-01-01T00:00:03.000Z');
                 INSERT INTO inbox_items (seq, id, kind, key, question_id, session_id, created_at)
                    VALUES (12, '01ARZ3NDEKTSV4RRFFQ69G5FB3', 'question', 'question:q1', 'q1', 's1',
                            '2026-01-01T00:00:04.000Z');
                 INSERT INTO inbox_fts (rowid, text) VALUES (3, 'Quarterly make it bluer');
                 INSERT INTO inbox_fts (rowid, text) VALUES (7, 'Quarterly claude chart');
                 INSERT INTO inbox_fts (rowid, text) VALUES (8, 'Quarterly pi done');
                 INSERT INTO inbox_fts (rowid, text) VALUES (12, 'which layout');
                 INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, person)
                    VALUES ('s1', 'claude', '/w', 'x', 'x', NULL), ('s2', 'claude', '/w', 'x', 'x', 'u_sam');
                 INSERT INTO questions (id, session_id, source, questions_json, status, created_at)
                    VALUES ('q1', 's1', 'ask', '[]', 'open', 'x'), ('q2', 's2', 'ask', '[]', 'open', 'x');",
            )
            .unwrap();
            rows(&c)
        };
        let st = Store::open(&home).unwrap();
        let new = st.with_read(|c| Ok(rows(c))).unwrap();
        assert_eq!(
            new, old,
            "every item keeps its seq, ID, kind, key, detail and read state"
        );
        let recipients: i64 = st
            .with_read(|c| {
                Ok(c.query_row("SELECT COUNT(recipient) FROM inbox_items", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(recipients, 0, "every existing item is the owner's");
        let q = |text: &str| crate::inbox::InboxQuery {
            text: Some(text.into()),
            ..Default::default()
        };
        let seqs = |text: &str| -> Vec<i64> {
            st.inbox_list(&q(text))
                .unwrap()
                .0
                .iter()
                .map(|i| i.seq)
                .collect()
        };
        assert_eq!(
            seqs("bluer"),
            vec![3],
            "the index still names each item's seq"
        );
        assert_eq!(seqs("quarterly"), vec![8, 7, 3]);
        assert_eq!(st.inbox_unread(None).unwrap(), 3);
        let kinds = |seq: i64| {
            st.with_read(|c| {
                Ok(
                    c.query_row("SELECT kind FROM inbox_items WHERE seq = ?1", [seq], |r| {
                        r.get::<_, String>(0)
                    })?,
                )
            })
            .unwrap()
        };
        assert_eq!(kinds(12), "question");
        // Each question names its asking session's person.
        let people: Vec<(String, Option<String>)> = st
            .with_read(|c| {
                let mut q = c.prepare("SELECT id, person FROM questions ORDER BY id")?;
                Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(
            people,
            vec![("q1".into(), None), ("q2".into(), Some("u_sam".into()))]
        );
        // The new kinds are accepted, each with its own reference (the
        // transaction is rolled back).
        let r = st.with_tx(|tx| {
            tx.execute_batch(
                "INSERT INTO inbox_items (id, kind, key, request_id, recipient, created_at)
                    VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FB4', 'approval', 'approval:r1@u_1', 'r1', 'u_1', 'x');
                 INSERT INTO inbox_items (id, kind, key, grant_id, created_at)
                    VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FB5', 'connection', 'connection:g_1', 'g_1', 'x');
                 INSERT INTO inbox_items (id, kind, key, detail_json, created_at)
                    VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FB6', 'ended', 'ended:s1', '{}', 'x');",
            )?;
            Err::<(), _>(clax_core::CoreError::NotFound)
        });
        assert!(matches!(r, Err(clax_core::CoreError::NotFound)), "{r:?}");
    }

    /// Migration 37 on databases that builds with earlier copies of
    /// migrations 24 to 26, 30 and 36 migrated (`tests/fixtures/old-migrations`).
    mod repair {
        use super::super::{MIGRATIONS, REPAIR_MIGRATION, rust_step, unconnected_approval};
        use crate::Store;
        use clax_core::Home;
        use clax_core::ids::ArtifactId;
        use clax_core::rights::{PRESET_READER, Rights};
        use clax_core::visibility::Reader;
        use rusqlite::{Connection, params};
        use std::collections::BTreeMap;

        /// Each fixture: an older build's migrations from its `first` to its
        /// `user_version`, as that build wrote them.
        pub(crate) const OLD_BUILDS: &[(&str, &str)] = &[
            (
                "pre-rebase-84b5e5c28",
                include_str!("../tests/fixtures/old-migrations/pre-rebase-84b5e5c28.sql"),
            ),
            (
                "rebased-d1a1c089",
                include_str!("../tests/fixtures/old-migrations/rebased-d1a1c089.sql"),
            ),
            (
                "rust-int-e1808eb5",
                include_str!("../tests/fixtures/old-migrations/rust-int-e1808eb5.sql"),
            ),
            (
                "rust-int-c308495e6",
                include_str!("../tests/fixtures/old-migrations/rust-int-c308495e6.sql"),
            ),
            (
                "rust-int-fc6a2add6",
                include_str!("../tests/fixtures/old-migrations/rust-int-fc6a2add6.sql"),
            ),
            (
                "main-086356acb",
                include_str!("../tests/fixtures/old-migrations/main-086356acb.sql"),
            ),
            (
                "main-396a8dad5",
                include_str!("../tests/fixtures/old-migrations/main-396a8dad5.sql"),
            ),
            (
                "main-96137d133",
                include_str!("../tests/fixtures/old-migrations/main-96137d133.sql"),
            ),
            (
                "pre-main-rebase-95d2a88db",
                include_str!("../tests/fixtures/old-migrations/pre-main-rebase-95d2a88db.sql"),
            ),
        ];

        const AID: &str = "7q3k9mzx2b4t";
        /// The LAN viewer's row ID: its `clax_viewer` cookie before
        /// `viewer_cookies`.
        const SAM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        const SAM_PUBLIC: &str = "u_0123456789abcdef012345";
        const LIVE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
        const ENDED: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";

        fn header(sql: &str, key: &str) -> u32 {
            let tag = format!("-- {key}: ");
            sql.lines()
                .find_map(|l| l.strip_prefix(&tag))
                .unwrap_or_else(|| panic!("no {key} line"))
                .trim()
                .parse()
                .unwrap()
        }

        fn has_column(c: &Connection, table: &str, column: &str) -> bool {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
                params![table, column],
                |r| r.get(0),
            )
            .unwrap()
        }

        fn has_table(c: &Connection, table: &str) -> bool {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |r| r.get(0),
            )
            .unwrap()
        }

        fn rights() -> String {
            Rights::preset(PRESET_READER).unwrap().to_json().to_string()
        }

        /// A database as the build of fixture `sql` left it: the current
        /// migrations before its first (with their Rust steps), then its own,
        /// each with the step every older build ran after migration 25 (the
        /// `*unconnected` approval), then rows of every kind the threads list
        /// reads: an owner and a LAN viewer, a live and an ended agent session
        /// (each with the owner's elevation where `sessions.elevate_json`
        /// holds it), a thread with the viewer's comment, the agent's reply,
        /// and the viewer's comment's delivery to the agent.
        pub(crate) fn old_build(home: &Home, sql: &str) {
            home.ensure_dirs().unwrap();
            let mut c = Connection::open(home.db_path()).unwrap();
            let first = header(sql, "first");
            let last = header(sql, "user_version");
            for v in 1..first {
                let tx = c.transaction().unwrap();
                tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
                rust_step(&tx, v).unwrap();
                tx.commit().unwrap();
            }
            let mut parts = sql.split("\n-- migration: ").skip(1);
            for v in first..=last {
                let part = parts.next().unwrap_or_else(|| panic!("no migration {v}"));
                let (n, body) = part.split_once('\n').unwrap();
                assert_eq!(n.trim().parse::<u32>().unwrap(), v);
                let tx = c.transaction().unwrap();
                tx.execute_batch(body).unwrap();
                if v == 25 {
                    unconnected_approval(&tx).unwrap();
                }
                tx.commit().unwrap();
            }
            assert!(parts.next().is_none());
            c.pragma_update(None, "user_version", last).unwrap();
            let anchor = serde_json::to_string(&crate::test_util::anchor()).unwrap();
            c.execute_batch(&format!(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('{AID}', 'Plan', '2026-10-15T10:00:00.000Z', '2026-10-15T10:00:00.000Z', 1, '0');
                 INSERT INTO versions (artifact_id, n, created_at, files_json)
                    VALUES ('{AID}', 1, '2026-10-15T10:00:00.000Z', '{{}}');
                 INSERT INTO viewers (id, display_name, created_at, public_id, owner, claimed)
                    VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAO', 'Alex', '2026-10-15T09:00:00.000Z',
                            'u_ffffffffffffffffffff00', 1, 1);
                 INSERT INTO viewers (id, display_name, created_at, public_id)
                    VALUES ('{SAM}', 'Sam', '2026-10-15T09:30:00.000Z', '{SAM_PUBLIC}');
                 INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, agent_handle)
                    VALUES ('{LIVE}', 'claude-code', '/w', '2026-10-15T10:00:00.000Z',
                            '2026-10-15T10:05:00.000Z', 'h_live');
                 INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at, ended_at, agent_handle)
                    VALUES ('{ENDED}', 'codex', '/w', '2026-10-15T09:00:00.000Z',
                            '2026-10-15T09:05:00.000Z', '2026-10-15T09:05:00.000Z', 'h_ended');
                 INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at)
                    VALUES ('t1', '{AID}', 1, '{anchor}', '2026-10-15T10:01:00.000Z');
                 INSERT INTO comments (id, thread_id, author_kind, author_name, author_public_id, body, created_at)
                    VALUES ('c1', 't1', 'viewer', 'Sam', '{SAM_PUBLIC}', 'Shorter, please',
                            '2026-10-15T10:01:00.000Z');
                 INSERT INTO comments (id, thread_id, author_kind, author_name, via_session_id, body, created_at)
                    VALUES ('c2', 't1', 'agent', 'claude-code', '{LIVE}', 'Done', '2026-10-15T10:02:00.000Z');
                 INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                    VALUES ('f1', 't1', 'c1', '{LIVE}', '2026-10-15T10:01:00.000Z');"
            ))
            .unwrap();
            if has_column(&c, "sessions", "elevate_json") {
                c.execute("UPDATE sessions SET elevate_json = ?1", [rights()])
                    .unwrap();
            }
        }

        /// Every schema object but SQLite's own, by (type, name), as text with
        /// its whitespace collapsed; a table's column and constraint
        /// definitions sorted, as a repaired table's added columns come last.
        fn schema(c: &Connection) -> BTreeMap<(String, String), String> {
            let mut q = c
                .prepare(
                    "SELECT type, name, sql FROM sqlite_master
                     WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL",
                )
                .unwrap();
            q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .map(|r| {
                let (ty, name, sql) = r.unwrap();
                let sql = sql.split_whitespace().collect::<Vec<_>>().join(" ");
                let sql = if ty == "table" {
                    sorted_definitions(&sql)
                } else {
                    sql
                };
                ((ty, name), sql)
            })
            .collect()
        }

        /// `CREATE TABLE name (a, b, ...)` with its top-level definitions
        /// sorted.
        fn sorted_definitions(sql: &str) -> String {
            let Some(open) = sql.find('(') else {
                return sql.to_string();
            };
            let (head, body) = sql.split_at(open);
            let body = &body[1..body.rfind(')').unwrap()];
            let mut defs = Vec::new();
            let (mut depth, mut start, mut quoted) = (0, 0, false);
            for (i, ch) in body.char_indices() {
                match ch {
                    '\'' => quoted = !quoted,
                    '(' if !quoted => depth += 1,
                    ')' if !quoted => depth -= 1,
                    ',' if !quoted && depth == 0 => {
                        defs.push(body[start..i].trim().to_string());
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            defs.push(body[start..].trim().to_string());
            defs.sort();
            format!("{}({})", head.trim_end(), defs.join(", "))
        }

        fn fresh_schema() -> BTreeMap<(String, String), String> {
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            drop(Store::open(&home).unwrap());
            schema(&Connection::open(home.db_path()).unwrap())
        }

        fn diff(
            got: &BTreeMap<(String, String), String>,
            want: &BTreeMap<(String, String), String>,
        ) -> Vec<String> {
            let mut out = Vec::new();
            for (k, v) in want {
                match got.get(k) {
                    None => out.push(format!("missing {k:?}")),
                    Some(g) if g != v => {
                        out.push(format!("differs {k:?}:\n  got  {g}\n  want {v}"))
                    }
                    _ => {}
                }
            }
            for k in got.keys().filter(|k| !want.contains_key(*k)) {
                out.push(format!("extra {k:?}"));
            }
            out
        }

        fn user_version(c: &Connection) -> u32 {
            c.query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap()
        }

        #[test]
        fn each_fixture_is_an_older_schema_than_the_current_one() {
            let fresh = fresh_schema();
            for (name, sql) in OLD_BUILDS {
                let dir = tempfile::tempdir().unwrap();
                let home = Home::at(dir.path().join("ax"));
                old_build(&home, sql);
                let c = Connection::open(home.db_path()).unwrap();
                assert!(
                    !diff(&schema(&c), &fresh).is_empty(),
                    "{name} differs from the current schema"
                );
            }
        }

        #[test]
        fn migration_37_brings_each_older_build_to_the_current_schema_and_keeps_its_rows() {
            let fresh = fresh_schema();
            for (name, sql) in OLD_BUILDS {
                let dir = tempfile::tempdir().unwrap();
                let home = Home::at(dir.path().join("ax"));
                old_build(&home, sql);
                let (had_cookies, had_elevate) = {
                    let c = Connection::open(home.db_path()).unwrap();
                    (
                        has_table(&c, "viewer_cookies"),
                        has_column(&c, "sessions", "elevate_json"),
                    )
                };
                let st = Store::open(&home).unwrap();
                let c = Connection::open(home.db_path()).unwrap();
                assert_eq!(user_version(&c), MIGRATIONS.len() as u32, "{name}");
                let d = diff(&schema(&c), &fresh);
                assert!(d.is_empty(), "{name}:\n{}", d.join("\n"));

                // The viewer, the thread and its comments are kept, and the
                // threads list reads them as `GET /api/artifacts/{aid}/threads`
                // does: the visibility for each reader, then the threads, then
                // each thread's extras, owner's person and live page.
                let kept: String = c
                    .query_row("SELECT public_id FROM viewers WHERE id = ?1", [SAM], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(kept, SAM_PUBLIC, "{name}: the viewer is kept");
                if !had_cookies {
                    let sam = st
                        .get_viewer(SAM)
                        .unwrap()
                        .expect("the row-ID cookie names the viewer");
                    assert_eq!(sam.public_id, SAM_PUBLIC, "{name}");
                }
                let aid = ArtifactId::parse(AID).unwrap();
                st.owner_sees_all().unwrap();
                crate::visibility::Visibility::for_reader(&st, &Reader::Owner, 0).unwrap();
                crate::visibility::Visibility::for_reader(
                    &st,
                    &Reader::Person(SAM_PUBLIC.into()),
                    0,
                )
                .unwrap();
                let (threads, _) = st.list_threads(&aid, true, None, 50).unwrap();
                assert_eq!(threads.len(), 1, "{name}");
                let bodies: Vec<&str> = threads[0]
                    .comments
                    .iter()
                    .map(|c| c.body.as_str())
                    .collect();
                assert_eq!(bodies, ["Shorter, please", "Done"], "{name}");
                for codex in [false, true] {
                    assert_eq!(
                        st.thread_extras(&threads, codex).unwrap().len(),
                        1,
                        "{name}"
                    );
                }
                st.agent_person(None).unwrap();
                st.live_page_of(&aid).unwrap();

                // The LAN viewer's row-ID cookie names it where the repair made
                // `viewer_cookies`; where the table was there, no cookie of
                // theirs is added back.
                let legacy: Vec<(String, i64)> = {
                    let mut q = c
                        .prepare("SELECT viewer_id, legacy FROM viewer_cookies WHERE sha256 = ?1")
                        .unwrap();
                    q.query_map([clax_core::secret::sha256_hex(SAM)], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
                };
                if had_cookies {
                    assert!(legacy.is_empty(), "{name}: {legacy:?}");
                } else {
                    assert_eq!(legacy, [(SAM.to_string(), 1)], "{name}");
                    let owner_cookies: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM viewer_cookies k JOIN viewers v ON v.id = k.viewer_id
                             WHERE v.owner = 1",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert_eq!(owner_cookies, 0, "{name}: no owner cookie");
                }

                // The live agent keeps the owner's elevation `elevate_json`
                // held; the ended one has none.
                let live = st.agent_rights(LIVE).unwrap().expect("a live agent");
                let raised = live.elevation_by(None).map(|r| r.to_json().to_string());
                assert_eq!(raised, had_elevate.then(rights), "{name}");
                if had_elevate {
                    assert_eq!(
                        live.elevations[0].version.len(),
                        26,
                        "{name}: a ULID version"
                    );
                }
                let ended: i64 = c
                    .query_row(
                        "SELECT COUNT(*) FROM session_elevations WHERE session_id = ?1",
                        [ENDED],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(ended, 0, "{name}");

                // Exactly one `*unconnected` approval.
                let approvals: i64 = c
                    .query_row(
                        "SELECT COUNT(*) FROM approvals WHERE author = ?1 AND created_via = 'migration'",
                        [clax_core::visibility::UNCONNECTED],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(approvals, 1, "{name}");
                assert_eq!(
                    c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                        .unwrap(),
                    "ok",
                    "{name}"
                );
            }
        }

        #[test]
        fn migration_37_changes_nothing_on_a_current_database_and_nothing_when_run_again() {
            let mut homes = vec![("new", None)];
            homes.extend(OLD_BUILDS.iter().map(|(n, sql)| (*n, Some(*sql))));
            for (name, sql) in homes {
                let dir = tempfile::tempdir().unwrap();
                let home = Home::at(dir.path().join("ax"));
                if let Some(sql) = sql {
                    old_build(&home, sql);
                }
                drop(Store::open(&home).unwrap());
                let mut c = Connection::open(home.db_path()).unwrap();
                let rows = |c: &Connection| -> Vec<i64> {
                    [
                        "viewer_cookies",
                        "session_elevations",
                        "approvals",
                        "inbox_items",
                        "push_approvals",
                        "sessions",
                    ]
                    .iter()
                    .map(|t| {
                        c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                            .unwrap()
                    })
                    .collect()
                };
                let (before, rows_before) = (schema(&c), rows(&c));
                let raw = |c: &Connection| -> Vec<String> {
                    let mut q = c
                        .prepare("SELECT sql FROM sqlite_master ORDER BY name")
                        .unwrap();
                    q.query_map([], |r| r.get::<_, Option<String>>(0))
                        .unwrap()
                        .map(|r| r.unwrap().unwrap_or_default())
                        .collect()
                };
                let raw_before = raw(&c);
                let tx = c.transaction().unwrap();
                rust_step(&tx, REPAIR_MIGRATION).unwrap();
                tx.commit().unwrap();
                assert_eq!(raw(&c), raw_before, "{name}: the schema text is unchanged");
                assert_eq!(schema(&c), before, "{name}");
                assert_eq!(rows(&c), rows_before, "{name}");
                assert_eq!(user_version(&c), MIGRATIONS.len() as u32, "{name}");
            }
        }

        #[test]
        fn migration_37_leaves_a_database_the_current_migrations_made_as_it_is() {
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            home.ensure_dirs().unwrap();
            let mut c = Connection::open(home.db_path()).unwrap();
            for v in 1..REPAIR_MIGRATION {
                let tx = c.transaction().unwrap();
                tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
                rust_step(&tx, v).unwrap();
                tx.pragma_update(None, "user_version", v).unwrap();
                tx.commit().unwrap();
            }
            let dump = |c: &Connection| -> Vec<(String, Option<String>)> {
                let mut q = c
                    .prepare("SELECT name, sql FROM sqlite_master ORDER BY name")
                    .unwrap();
                q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            };
            let before = dump(&c);
            let tx = c.transaction().unwrap();
            tx.execute_batch(MIGRATIONS[REPAIR_MIGRATION as usize - 1])
                .unwrap();
            rust_step(&tx, REPAIR_MIGRATION).unwrap();
            tx.commit().unwrap();
            assert_eq!(dump(&c), before);
        }

        #[test]
        fn migration_37_carries_inbox_items_and_push_approvals_through_their_rebuilds() {
            let (_, sql) = OLD_BUILDS
                .iter()
                .find(|(n, _)| *n == "rust-int-c308495e6")
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            old_build(&home, sql);
            {
                let c = Connection::open(home.db_path()).unwrap();
                c.execute_batch(&format!(
                    "INSERT INTO inbox_items (seq, id, kind, key, artifact_id, created_at)
                        VALUES (5, '01ARZ3NDEKTSV4RRFFQ69G5FB1', 'published', 'published:{AID}', '{AID}', 'x');
                     INSERT INTO push_approvals (question_id, artifact_id, remote_id, session_id, created_at)
                        VALUES ('q1', '{AID}', 'r1', '{LIVE}', 'x');"
                ))
                .unwrap();
            }
            let st = Store::open(&home).unwrap();
            let c = Connection::open(home.db_path()).unwrap();
            let item: (i64, String) = c
                .query_row("SELECT seq, kind FROM inbox_items", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(item, (5, "published".to_string()));
            let upto: i64 = c
                .query_row(
                    "SELECT upto FROM push_approvals WHERE question_id = 'q1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(upto, 0, "a kept approval reaches no version");
            // The `push` kind is accepted (the transaction is rolled back).
            let r = st.with_tx(|tx| {
                tx.execute_batch(
                    "INSERT INTO inbox_items (id, kind, key, created_at)
                        VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FB2', 'push', 'push:x', 'x');",
                )?;
                Err::<(), _>(clax_core::CoreError::NotFound)
            });
            assert!(matches!(r, Err(clax_core::CoreError::NotFound)), "{r:?}");
        }
    }

    /// The rows a renumbering or upgrade must keep, table by table, each
    /// as one text per row in a stable order.
    fn kept_rows(c: &Connection, app: usize) -> Vec<(String, Vec<String>)> {
        let mut queries = vec![
            (
                "artifacts",
                "SELECT id || '|' || title || '|' || current_version || '|' || doc_seq FROM artifacts ORDER BY id",
            ),
            (
                "versions",
                "SELECT artifact_id || '|' || n || '|' || files_json FROM versions ORDER BY 1",
            ),
            (
                "docs",
                "SELECT artifact_id || '|' || path || '|' || json || '|' || version FROM docs ORDER BY 1",
            ),
            (
                "comments",
                "SELECT id || '|' || thread_id || '|' || body FROM comments ORDER BY 1",
            ),
            (
                "inbox_items",
                "SELECT seq || '|' || id || '|' || kind || '|' || key FROM inbox_items ORDER BY seq",
            ),
            (
                "db_read_grants",
                "SELECT reader_id || '|' || source_id FROM db_read_grants ORDER BY 1",
            ),
        ];
        if app >= 2 {
            queries.push((
                "drafts",
                "SELECT id || '|' || hidden || '|' || coalesce(draft_of, '') FROM artifacts ORDER BY id",
            ));
        }
        if app >= 3 {
            queries.push((
                "net_grants",
                "SELECT artifact_id || '|' || host FROM net_grants ORDER BY 1",
            ));
        }
        if app >= 4 {
            queries.push((
                "sample_grants",
                "SELECT artifact_id FROM sample_grants UNION ALL
                 SELECT artifact_id || '|' || day || '|' || micro_usd FROM sample_spend",
            ));
        }
        if app >= 5 {
            queries.push((
                "draft_inherit_net",
                "SELECT id || '|' || draft_inherit_net FROM artifacts ORDER BY id",
            ));
        }
        queries
            .into_iter()
            .map(|(name, sql)| {
                let mut stmt = c.prepare(sql).unwrap();
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                (name.to_string(), rows)
            })
            .collect()
    }

    /// A database as a `flutter-app` build from before the multi-person
    /// migrations (24 to 26) left it: migrations 1 to 23, then the first
    /// `app` of the app's migrations ([`APP_MIGRATION`] onwards here),
    /// numbered from 24, with the rows of [`app_rows`]. Returns its rows.
    fn pre_rebase_flutter_app(home: &Home, app: usize) -> Vec<(String, Vec<String>)> {
        home.ensure_dirs().unwrap();
        let c = Connection::open(home.db_path()).unwrap();
        c.pragma_update(None, "journal_mode", "wal").unwrap();
        for sql in &MIGRATIONS[..AUDIT_MIGRATION as usize] {
            c.execute_batch(sql).unwrap();
        }
        let first = APP_MIGRATION as usize - 1;
        for sql in &MIGRATIONS[first..first + app] {
            c.execute_batch(sql).unwrap();
        }
        c.execute(
            "INSERT INTO install (k, v) VALUES ('id', ?1)",
            ["0".repeat(32)],
        )
        .unwrap();
        c.pragma_update(None, "user_version", AUDIT_MIGRATION + app as u32)
            .unwrap();
        app_rows(&c, app)
    }

    /// A database as this branch's builds before akesling/clax-artifacts's
    /// migrations 27 to 37 left it: migrations 1 to 26 with their Rust
    /// steps, then the first `app` of the app's migrations ([`APP_MIGRATION`]
    /// onwards here), numbered from 27, with the rows of [`app_rows`].
    /// Returns its rows.
    fn earlier_app_numbering(home: &Home, app: usize) -> Vec<(String, Vec<String>)> {
        let c = earlier_app_schema(home, app);
        app_rows(&c, app)
    }

    /// [`earlier_app_numbering`]'s schema and version, with no rows but the
    /// ones the migrations' Rust steps write.
    fn earlier_app_schema(home: &Home, app: usize) -> Connection {
        home.ensure_dirs().unwrap();
        let mut c = Connection::open(home.db_path()).unwrap();
        c.pragma_update(None, "journal_mode", "wal").unwrap();
        let tx = c.transaction().unwrap();
        for v in 1..=INBOX_MIGRATION {
            tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
            super::rust_step(&tx, v).unwrap();
        }
        let first = APP_MIGRATION as usize - 1;
        for sql in &MIGRATIONS[first..first + app] {
            tx.execute_batch(sql).unwrap();
        }
        tx.pragma_update(None, "user_version", INBOX_MIGRATION + app as u32)
            .unwrap();
        tx.commit().unwrap();
        c
    }

    /// A database as this branch's builds before the where-to-ask
    /// migration ([`WHERE_TO_ASK_MIGRATION`]) left it: migrations 1 to 37
    /// with their Rust steps, then the first `app` of the app's migrations
    /// ([`APP_MIGRATION`] onwards here), numbered from 38, with the rows of
    /// [`app_rows`]. Returns its rows.
    fn app_numbered_from_38(home: &Home, app: usize) -> Vec<(String, Vec<String>)> {
        home.ensure_dirs().unwrap();
        let mut c = Connection::open(home.db_path()).unwrap();
        c.pragma_update(None, "journal_mode", "wal").unwrap();
        let tx = c.transaction().unwrap();
        for v in 1..=REPAIR_MIGRATION {
            tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
            super::rust_step(&tx, v).unwrap();
        }
        let first = APP_MIGRATION as usize - 1;
        for sql in &MIGRATIONS[first..first + app] {
            tx.execute_batch(sql).unwrap();
        }
        tx.pragma_update(None, "user_version", REPAIR_MIGRATION + app as u32)
            .unwrap();
        tx.commit().unwrap();
        app_rows(&c, app)
    }

    /// Apps, versions, documents, a comment, an inbox item and the grants
    /// and drafts the first `app` of the app's migrations hold, written to
    /// `c`; returns its rows ([`kept_rows`]).
    fn app_rows(c: &Connection, app: usize) -> Vec<(String, Vec<String>)> {
        c.execute_batch(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version, doc_seq)
                VALUES ('7q3k9mzx2b4t', 'Budget', 'x', 'x', 2, '0', 3),
                       ('8r4m2nzy3c5v', 'Notes', 'x', 'x', 1, '0', 1);
             INSERT INTO versions (artifact_id, n, created_at, files_json) VALUES
                ('7q3k9mzx2b4t', 1, 'x', '{\"index.html\":\"a\"}'),
                ('7q3k9mzx2b4t', 2, 'x', '{\"index.html\":\"b\"}'),
                ('8r4m2nzy3c5v', 1, 'x', '{\"index.html\":\"c\"}');
             INSERT INTO docs (artifact_id, path, collection, json, version, updated_at) VALUES
                ('7q3k9mzx2b4t', 'rows/1', 'rows', '{\"amount\":12}', 2, 'x'),
                ('7q3k9mzx2b4t', 'rows/2', 'rows', '{\"amount\":30}', 3, 'x'),
                ('8r4m2nzy3c5v', 'notes/1', 'notes', '{\"text\":\"hi\"}', 1, 'x');
             INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at)
                VALUES ('t1', '7q3k9mzx2b4t', 1, '{}', 'x');
             INSERT INTO comments (id, thread_id, author_kind, author_name, body, created_at)
                VALUES ('c1', 't1', 'viewer', 'Alex', 'make it green', 'x');
             INSERT INTO inbox_items (id, kind, key, artifact_id, created_at)
                VALUES ('b1', 'published', 'published:7q3k9mzx2b4t', '7q3k9mzx2b4t', 'x');
             INSERT INTO db_read_grants (reader_id, source_id, granted_at)
                VALUES ('8r4m2nzy3c5v', '7q3k9mzx2b4t', 'x');",
        )
        .unwrap();
        if app >= 2 {
            c.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version, draft_of, hidden)
                    VALUES ('9s5n3pza4d6w', 'Budget draft', 'x', 'x', 1, '0', '7q3k9mzx2b4t', 1);
                 INSERT INTO versions (artifact_id, n, created_at, files_json)
                    VALUES ('9s5n3pza4d6w', 1, 'x', '{\"index.html\":\"d\"}');",
            )
            .unwrap();
        }
        if app >= 3 {
            c.execute_batch(
                "INSERT INTO net_grants (artifact_id, host, granted_at)
                    VALUES ('7q3k9mzx2b4t', 'api.example.com', 'x');",
            )
            .unwrap();
        }
        if app >= 4 {
            c.execute_batch(
                "INSERT INTO sample_grants (artifact_id, granted_at) VALUES ('7q3k9mzx2b4t', 'x');
                 INSERT INTO sample_spend (artifact_id, day, micro_usd)
                    VALUES ('7q3k9mzx2b4t', '2026-10-14', 1200);",
            )
            .unwrap();
        }
        if app >= 5 {
            c.execute_batch(
                "UPDATE artifacts SET draft_inherit_net = 0 WHERE id = '9s5n3pza4d6w';",
            )
            .unwrap();
        }
        kept_rows(c, app)
    }

    fn assert_pre_rebase_flutter_app_upgrades(app: usize) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let before = pre_rebase_flutter_app(&home, app);
        let old = AUDIT_MIGRATION + app as u32;
        let st = Store::open(&home).unwrap();
        let (version, after, marks) = st
            .with_read(|c| {
                let v: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
                let marks: i64 = c.query_row(
                    "SELECT (SELECT COUNT(*) FROM sqlite_schema WHERE name IN
                                ('invites', 'grants', 'approvals', 'doc_authors'))
                          + (SELECT COUNT(*) FROM pragma_table_info('inbox_items')
                                WHERE name = 'recipient')
                          + (SELECT COUNT(*) FROM pragma_table_info('artifacts')
                                WHERE name IN ('person', 'text_authors_json', 'draft_inherit_net'))",
                    [],
                    |r| r.get(0),
                )?;
                Ok((v, kept_rows(c, app), marks))
            })
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32, "from {old}");
        assert_eq!(after, before, "every row kept from {old}");
        assert_eq!(marks, 8, "the multi-person and app schema, from {old}");
        let listed = st.list_artifacts().unwrap().len();
        assert_eq!(listed, 2, "the draft stays unlisted, from {old}");
        let approvals: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM approvals WHERE created_via = 'migration'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(approvals, 1, "migration 25's Rust step ran, from {old}");
        drop(st);
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
        let name = copies[0].file_name().unwrap().to_str().unwrap().to_string();
        assert!(name.starts_with(&format!("clax-{old}-")), "{name}");
        let copy = Connection::open(&copies[0]).unwrap();
        let v: u32 = copy
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, old);
        assert_eq!(kept_rows(&copy, app), before, "the copy holds the old rows");
    }

    #[test]
    fn a_pre_rebase_flutter_app_database_at_27_keeps_every_row_and_is_copied_first() {
        assert_pre_rebase_flutter_app_upgrades(4);
    }

    #[test]
    fn a_pre_rebase_flutter_app_database_at_28_keeps_every_row_and_is_copied_first() {
        assert_pre_rebase_flutter_app_upgrades(5);
    }

    #[test]
    fn pre_rebase_flutter_app_databases_at_24_to_26_upgrade() {
        for app in 1..=3 {
            assert_pre_rebase_flutter_app_upgrades(app);
        }
    }

    /// Each of `numbering`'s main marks above 38 is absent from an empty
    /// database before its migration, present after it (Rust step and
    /// all), and still present after every later migration.
    fn assert_marks_made(numbering: &super::Numbering<'_>) {
        let main = WHERE_TO_ASK_MIGRATION as usize;
        let mut c = Connection::open_in_memory().unwrap();
        let tx = c.transaction().unwrap();
        for v in 1..=numbering.migrations.len() as u32 {
            let made = v as usize > main && v < numbering.app_first();
            let mark = made.then(|| numbering.main_marks[v as usize - main - 1]);
            if let Some(mark) = mark {
                assert!(!super::has_mark(&tx, &mark).unwrap(), "{mark} before {v}");
            }
            tx.execute_batch(numbering.migrations[v as usize - 1])
                .unwrap();
            super::rust_step(&tx, v).unwrap();
            if let Some(mark) = mark {
                assert!(super::has_mark(&tx, &mark).unwrap(), "{mark} after {v}");
            }
        }
        for mark in numbering.main_marks {
            assert!(super::has_mark(&tx, mark).unwrap(), "{mark} at the end");
        }
    }

    #[test]
    fn the_app_migrations_follow_mains_in_this_order() {
        assert_eq!(WHERE_TO_ASK_MIGRATION, REPAIR_MIGRATION + 1);
        assert!(MIGRATIONS[WHERE_TO_ASK_MIGRATION as usize - 1].contains("ADD COLUMN thread_id"));
        // Main's migrations after the where-to-ask one, each making its mark.
        let main = WHERE_TO_ASK_MIGRATION as usize;
        assert_eq!(APP_MIGRATION as usize, main + MAIN_MARKS_ABOVE_38.len() + 1);
        assert_marks_made(&super::NUMBERING);
        // Adoption runs them on a database holding the app's tables: they
        // name none of them.
        for sql in &MIGRATIONS[main..APP_MIGRATION as usize - 1] {
            for mark in super::APP_MIGRATION_MARKS {
                let (super::Mark::Table(name) | super::Mark::Column(_, name)) = mark else {
                    unreachable!("the app's marks are tables and columns")
                };
                assert!(!sql.contains(name), "{mark}");
            }
        }
        assert_eq!(MIGRATIONS.len() as u32, APP_MIGRATION + 4);
        let first = APP_MIGRATION as usize - 1;
        assert!(MIGRATIONS[first].contains("CREATE TABLE db_read_grants"));
        assert!(MIGRATIONS[first + 1].contains("ADD COLUMN draft_of"));
        assert!(MIGRATIONS[first + 2].contains("CREATE TABLE net_grants"));
        assert!(MIGRATIONS[first + 3].contains("CREATE TABLE sample_grants"));
        assert!(MIGRATIONS[first + 4].contains("ADD COLUMN draft_inherit_net"));
    }

    #[test]
    fn a_pre_rebase_database_whose_schema_disagrees_with_its_version_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        // Version 27 names sample grants, which this database lacks.
        pre_rebase_flutter_app(&home, 3);
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.pragma_update(None, "user_version", 27).unwrap();
        }
        let err = match Store::open(&home) {
            Ok(_) => panic!("an inconsistent database must be refused"),
            Err(e) => e,
        };
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at: 27, detail }
                if detail.contains("sample_grants")),
            "{err:?}"
        );
        let c = Connection::open(home.db_path()).unwrap();
        let (v, invites): (u32, i64) = c
            .query_row(
                "SELECT (SELECT user_version FROM pragma_user_version),
                        (SELECT COUNT(*) FROM sqlite_schema WHERE name = 'invites')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((v, invites), (27, 0));
    }

    #[test]
    fn a_failing_migration_is_reported_and_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let at = REPAIR_MIGRATION as usize;
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..at] {
                c.execute_batch(sql).unwrap();
            }
            c.execute(
                "INSERT INTO install (k, v) VALUES ('id', ?1)",
                ["0".repeat(32)],
            )
            .unwrap();
            // The draft migration adds this column: it fails.
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN draft_of TEXT;")
                .unwrap();
            c.pragma_update(None, "user_version", at as u32).unwrap();
        }
        let err = match Store::open(&home) {
            Ok(_) => panic!("the migration must fail"),
            Err(e) => e,
        };
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at, detail }
                if *at == APP_MIGRATION && detail.contains("draft_of")),
            "{err:?}"
        );
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert!(
            err.to_string().contains(&copies[0].display().to_string()),
            "the failure names the copy: {err}"
        );
        let c = Connection::open(home.db_path()).unwrap();
        let v: u32 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            v, APP_MIGRATION,
            "the migrations up to the app's first committed, the draft one rolled back"
        );
    }

    #[test]
    fn a_base_database_at_26_upgrades_and_is_copied_first() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let at = INBOX_MIGRATION as usize;
        {
            let c = Connection::open(home.db_path()).unwrap();
            for sql in &MIGRATIONS[..at] {
                c.execute_batch(sql).unwrap();
            }
            c.execute(
                "INSERT INTO install (k, v) VALUES ('id', ?1)",
                ["0".repeat(32)],
            )
            .unwrap();
            c.pragma_update(None, "user_version", at as u32).unwrap();
            c.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 't', 'x', 'x', 1, '0');
                 INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                    VALUES ('7q3k9mzx2b4t', 'rows/1', 'rows', '{}', 1, 'x');",
            )
            .unwrap();
        }
        let st = Store::open(&home).unwrap();
        let (v, docs, inherit): (u32, i64, i64) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT user_version FROM pragma_user_version),
                            (SELECT COUNT(*) FROM docs),
                            (SELECT draft_inherit_net FROM artifacts)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!((v, docs, inherit), (MIGRATIONS.len() as u32, 1, 1));
        assert_eq!(st.list_artifacts().unwrap().len(), 1);
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
    }

    #[test]
    fn a_fresh_database_migrates_without_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let st = Store::open(&home).unwrap();
        let v: u32 = st
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as u32);
        drop(st);
        assert!(!crate::backup::dir(&home).exists());
        drop(Store::open(&home).unwrap());
        assert!(!crate::backup::dir(&home).exists(), "nothing to migrate");
    }

    #[test]
    fn upgrades_keep_the_newest_three_copies() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let backups = crate::backup::dir(&home);
        std::fs::create_dir_all(&backups).unwrap();
        for at in [
            "20261001T000000000Z",
            "20261002T000000000Z",
            "20261003T000000000Z",
        ] {
            std::fs::write(backups.join(format!("clax-20-{at}.db")), b"old").unwrap();
        }
        std::fs::write(backups.join("notes.txt"), b"kept").unwrap();
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
        }
        let clock: std::sync::Arc<dyn clax_core::working::Clock> =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-15T12:00:00Z"));
        drop(Store::open_with_clock(&home, clock).unwrap());
        let names: Vec<String> = crate::backup::list(&backups)
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "clax-20-20261002T000000000Z.db",
                "clax-20-20261003T000000000Z.db",
                "clax-1-20261015T120000000Z.db",
            ]
        );
        assert!(backups.join("notes.txt").exists());
    }

    /// Every schema object outside SQLite's own, as `(type, name, sql)`
    /// with the SQL's whitespace collapsed, in name order.
    fn schema_of(c: &Connection) -> Vec<(String, String, String)> {
        let mut stmt = c
            .prepare(
                "SELECT type, name, COALESCE(sql, '') FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .unwrap();
        stmt.query_map([], |r| {
            let sql: String = r.get(2)?;
            Ok((
                r.get(0)?,
                r.get(1)?,
                sql.split_whitespace().collect::<Vec<_>>().join(" "),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    const LAN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";

    /// A database as this branch's builds of the earlier revision of the
    /// multi-person migrations left it, at the earlier app numbering's last
    /// version (31): that schema without what the final revision added
    /// (`viewer_cookies`, two `approval_requests` indexes and
    /// `feedback.no_access_at`, which migration 37's repair makes), with an
    /// owner, a LAN viewer whose cookie is its row ID, an artifact, a thread
    /// sent to an agent, an approval request and a read grant.
    fn earlier_multi_person(home: &Home) {
        let c = earlier_app_schema(home, 5);
        c.execute_batch(&format!(
            "DROP TABLE viewer_cookies;
             DROP INDEX approval_requests_by_target;
             DROP INDEX approval_requests_by_thread;
             ALTER TABLE feedback DROP COLUMN no_access_at;
             DELETE FROM viewers;
             INSERT INTO viewers (id, public_id, display_name, created_at, owner, claimed)
                VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAZ', 'u_owner', 'Alex', 'x', 1, 1),
                       ('{LAN}', 'u_lan', 'Mia', 'x', 0, 0);
             INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version, doc_seq)
                VALUES ('7q3k9mzx2b4t', 'Budget', 'x', 'x', 1, '0', 1),
                       ('8r4m2nzy3c5v', 'Notes', 'x', 'x', 1, '0', 0);
             INSERT INTO versions (artifact_id, n, created_at, files_json)
                VALUES ('7q3k9mzx2b4t', 1, 'x', '{{\"index.html\":\"a\"}}'),
                       ('8r4m2nzy3c5v', 1, 'x', '{{\"index.html\":\"b\"}}');
             INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                VALUES ('7q3k9mzx2b4t', 'rows/1', 'rows', '{{\"amount\":12}}', 1, 'x');
             INSERT INTO sessions (id, harness, cwd, started_at, last_seen_at)
                VALUES ('{SID}', 'claude', '/w', 'x', 'x');
             INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at)
                VALUES ('t1', '7q3k9mzx2b4t', 1, '{{}}', 'x');
             INSERT INTO comments (id, thread_id, author_kind, author_name, body, created_at)
                VALUES ('c1', 't1', 'viewer', 'Mia', 'make it green', 'x');
             INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                VALUES ('f1', 't1', 'c1', '{SID}', 'x');
             INSERT INTO approval_requests (id, author, artifact_id, thread_id, cause, created_at, updated_at)
                VALUES ('r1', 'u_lan', '7q3k9mzx2b4t', 't1', 'routing', 'x', 'x');
             INSERT INTO db_read_grants (reader_id, source_id, granted_at)
                VALUES ('8r4m2nzy3c5v', '7q3k9mzx2b4t', 'x');"
        ))
        .unwrap();
    }

    fn earlier_rows(c: &Connection) -> Vec<String> {
        let mut stmt = c
            .prepare(
                "SELECT 'viewer ' || id || ' ' || public_id FROM viewers
                 UNION ALL SELECT 'artifact ' || id FROM artifacts
                 UNION ALL SELECT 'doc ' || path || ' ' || json FROM docs
                 UNION ALL SELECT 'comment ' || id || ' ' || body FROM comments
                 UNION ALL SELECT 'feedback ' || id || ' ' || target_session_id FROM feedback
                 UNION ALL SELECT 'request ' || id || ' ' || status FROM approval_requests
                 UNION ALL SELECT 'grant ' || reader_id || ' ' || source_id FROM db_read_grants
                 ORDER BY 1",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// Each table's columns (in name order, as `pragma_table_info` gives
    /// them) and every other schema object as [`schema_of`] gives it: a
    /// schema up to the order of its columns, which an upgrade's added
    /// columns change.
    fn shape_of(c: &Connection) -> Vec<String> {
        schema_of(c)
            .into_iter()
            .map(|(kind, name, sql)| {
                if kind != "table" {
                    return format!("{kind} {name} {sql}");
                }
                let mut stmt = c
                    .prepare(
                        "SELECT name || ' ' || type || ' ' || \"notnull\" || ' ' || COALESCE(dflt_value, '') || ' ' || pk
                         FROM pragma_table_info(?1) ORDER BY name",
                    )
                    .unwrap();
                let cols: Vec<String> = stmt
                    .query_map([&name], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                format!("table {name} {}", cols.join(", "))
            })
            .collect()
    }

    /// A fresh database's [`shape_of`].
    fn fresh_shape() -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        drop(Store::open(&home).unwrap());
        shape_of(&Connection::open(home.db_path()).unwrap())
    }

    #[test]
    fn a_database_of_the_earlier_multi_person_revision_gets_the_final_schema_and_keeps_its_rows() {
        let fresh = fresh_shape();
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        earlier_multi_person(&home);
        let before = earlier_rows(&Connection::open(home.db_path()).unwrap());
        assert_ne!(
            shape_of(&Connection::open(home.db_path()).unwrap()),
            fresh,
            "the fixture lacks the final revision"
        );

        let st = Store::open(&home).unwrap();
        let (version, shape, after, ok) = st
            .with_read(|c| {
                let v: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
                let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
                Ok((v, shape_of(c), earlier_rows(c), ok))
            })
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32);
        assert_eq!(ok, "ok");
        assert_eq!(shape, fresh, "the final schema");
        assert_eq!(after, before, "every row kept");
        // The LAN viewer's cookie, its row ID, keeps naming it.
        let v = st.get_viewer(LAN).unwrap().unwrap();
        assert_eq!(v.public_id, "u_lan");
        drop(st);

        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "copied before the upgrade: {copies:?}");
        let copy = Connection::open(&copies[0]).unwrap();
        assert_eq!(earlier_rows(&copy), before, "the copy holds the old rows");
        let cookies: i64 = copy
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'viewer_cookies'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cookies, 0, "the copy is from before the repair");

        // Opening it again changes nothing and copies nothing.
        drop(Store::open(&home).unwrap());
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
    }

    /// A database this branch's builds left at version `26 + app` of the
    /// earlier app numbering opens at the latest version in this numbering
    /// with every row kept, the final schema up to column order, the
    /// multi-person migrations' Rust steps not run again, and a copy of it
    /// as it was taken first; opening it again changes nothing.
    fn assert_earlier_app_numbering_upgrades(app: usize) {
        assert_numbered_app_database_upgrades(app, INBOX_MIGRATION, earlier_app_numbering);
    }

    /// Makes a database at a version of an earlier numbering in `home`,
    /// with the first `app` of the app's migrations, returning each table's
    /// rows.
    type MakeNumbered = fn(&Home, usize) -> Vec<(String, Vec<String>)>;

    /// As [`assert_earlier_app_numbering_upgrades`], for the database
    /// `make` leaves at version `base + app` with the first `app` of the
    /// app's migrations, returning its rows.
    fn assert_numbered_app_database_upgrades(app: usize, base: u32, make: MakeNumbered) {
        let fresh = fresh_shape();
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let before = make(&home, app);
        let old = base + app as u32;
        let st = Store::open(&home).unwrap();
        let (version, after, shape, approvals, ok) = st
            .with_read(|c| {
                let v: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
                let approvals: i64 = c.query_row(
                    "SELECT COUNT(*) FROM approvals WHERE created_via = 'migration'",
                    [],
                    |r| r.get(0),
                )?;
                let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
                Ok((v, kept_rows(c, app), shape_of(c), approvals, ok))
            })
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32, "from {old}");
        assert_eq!(after, before, "every row kept from {old}");
        assert_eq!(ok, "ok", "from {old}");
        assert_eq!(shape, fresh, "the final schema, from {old}");
        assert_eq!(approvals, 1, "migration 25's step ran once, from {old}");
        assert_eq!(
            st.list_artifacts().unwrap().len(),
            2,
            "the draft stays unlisted, from {old}"
        );
        drop(st);
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
        let name = copies[0].file_name().unwrap().to_str().unwrap().to_string();
        assert!(name.starts_with(&format!("clax-{old}-")), "{name}");
        let copy = Connection::open(&copies[0]).unwrap();
        let v: u32 = copy
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, old);
        assert_eq!(kept_rows(&copy, app), before, "the copy holds the old rows");
        drop(Store::open(&home).unwrap());
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "opened again: {copies:?}");
    }

    #[test]
    fn a_database_at_the_earlier_app_numbering_31_keeps_every_row_and_is_copied_first() {
        assert_earlier_app_numbering_upgrades(5);
    }

    #[test]
    fn databases_at_the_earlier_app_numbering_27_to_30_upgrade() {
        for app in 1..=4 {
            assert_earlier_app_numbering_upgrades(app);
        }
    }

    #[test]
    fn databases_at_the_app_numbering_from_38_upgrade_with_every_row_and_a_copy_first() {
        for app in 1..=5 {
            assert_numbered_app_database_upgrades(app, REPAIR_MIGRATION, app_numbered_from_38);
        }
    }

    #[test]
    fn an_app_numbering_from_38_database_whose_schema_disagrees_with_its_version_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        // Version 42 names a draft's network inheritance, which this
        // database lacks.
        app_numbered_from_38(&home, 4);
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.pragma_update(None, "user_version", 42).unwrap();
        }
        let err = match Store::open(&home) {
            Ok(_) => panic!("an inconsistent database must be refused"),
            Err(e) => e,
        };
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at: 42, detail }
                if detail.contains("draft_inherit_net")),
            "{err:?}"
        );
        let c = Connection::open(home.db_path()).unwrap();
        let (v, asked): (u32, i64) = c
            .query_row(
                "SELECT (SELECT user_version FROM pragma_user_version),
                        (SELECT COUNT(*) FROM pragma_table_info('questions') WHERE name = 'thread_id')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((v, asked), (42, 0));
    }

    #[test]
    fn an_earlier_app_numbering_database_whose_schema_disagrees_with_its_version_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        // Version 31 names a draft's network inheritance, which this
        // database lacks.
        earlier_app_numbering(&home, 4);
        {
            let c = Connection::open(home.db_path()).unwrap();
            c.pragma_update(None, "user_version", 31).unwrap();
        }
        let err = match Store::open(&home) {
            Ok(_) => panic!("an inconsistent database must be refused"),
            Err(e) => e,
        };
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at: 31, detail }
                if detail.contains("draft_inherit_net")),
            "{err:?}"
        );
        let c = Connection::open(home.db_path()).unwrap();
        let (v, hosted): (u32, i64) = c
            .query_row(
                "SELECT (SELECT user_version FROM pragma_user_version),
                        (SELECT COUNT(*) FROM sqlite_schema WHERE name = 'hosted_setup')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((v, hosted), (31, 0));
    }

    #[test]
    fn a_database_of_the_earlier_multi_person_revision_below_the_visibility_migration_gets_only_its_part()
     {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let c = Connection::open(home.db_path()).unwrap();
        for sql in &MIGRATIONS[..PEOPLE_MIGRATION as usize] {
            c.execute_batch(sql).unwrap();
        }
        c.execute(
            "INSERT INTO install (k, v) VALUES ('id', ?1)",
            ["0".repeat(32)],
        )
        .unwrap();
        c.execute_batch(&format!(
            "DROP TABLE viewer_cookies;
             INSERT INTO viewers (id, public_id, display_name, created_at) VALUES ('{LAN}', 'u_lan', 'Mia', 'x');"
        ))
        .unwrap();
        c.pragma_update(None, "user_version", PEOPLE_MIGRATION)
            .unwrap();
        drop(c);
        let st = Store::open(&home).unwrap();
        assert_eq!(st.get_viewer(LAN).unwrap().unwrap().public_id, "u_lan");
        let v: u32 = st
            .with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as u32);
    }

    /// The Clax app's migrations as its installed builds numbered them, at
    /// 39 to 43, before main's 39 and 40 (the owner's phone and Mac are at
    /// its 43).
    const INSTALLED_AT_43: &str =
        include_str!("../tests/fixtures/old-migrations/flutter-app-4508df19f.sql");

    /// The SQL of each migration in [`INSTALLED_AT_43`], in order.
    fn installed_migrations() -> Vec<&'static str> {
        INSTALLED_AT_43
            .split("\n-- migration: ")
            .skip(1)
            .map(|part| part.split_once('\n').unwrap().1)
            .collect()
    }

    /// Each table of `c` (SQLite's own aside) with its columns, in order.
    type Columns = Vec<(String, Vec<String>)>;

    fn columns_of(c: &Connection) -> Columns {
        let tables: Vec<String> = {
            let mut q = c
                .prepare(
                    "SELECT name FROM sqlite_schema WHERE type = 'table'
                     AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .unwrap();
            q.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        tables
            .into_iter()
            .map(|t| {
                let mut q = c
                    .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                    .unwrap();
                let cols = q
                    .query_map([&t], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                (t, cols)
            })
            .collect()
    }

    /// The rows of `c` in the tables and columns `cols` names, table by
    /// table, each row as the debug text of its values, sorted: with the
    /// [`columns_of`] a database had before an upgrade, what the upgrade
    /// must keep.
    fn rows_in(c: &Connection, cols: &Columns) -> Vec<(String, Vec<String>)> {
        cols.iter()
            .map(|(t, cols)| {
                let list = cols
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut q = c.prepare(&format!("SELECT {list} FROM \"{t}\"")).unwrap();
                let mut rows: Vec<String> = q
                    .query_map([], |r| {
                        (0..cols.len())
                            .map(|i| r.get::<_, rusqlite::types::Value>(i))
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .map(|v| format!("{v:?}"))
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                rows.sort();
                (t.clone(), rows)
            })
            .collect()
    }

    /// Every row of every table of `c` ([`rows_in`] its own columns).
    fn every_row(c: &Connection) -> Vec<(String, Vec<String>)> {
        rows_in(c, &columns_of(c))
    }

    /// A home as the installed Clax app's `clax` left it at its 43:
    /// migrations 1 to 38, then [`INSTALLED_AT_43`]'s, with the rows of
    /// `old_build`, a draft, and a row in each of the app's grant tables;
    /// in write-ahead-log mode, with the audit backfill and the migration
    /// approval's record marked done, as a home its store has opened.
    fn installed_at_43(home: &Home) {
        repair::old_build(home, INSTALLED_AT_43);
        let c = Connection::open(home.db_path()).unwrap();
        c.execute_batch(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                VALUES ('8r4m2nzy3c5v', 'Notes', 'x', 'x', 1, '0');
             INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version,
                                    draft_of, hidden, draft_inherit_net)
                VALUES ('9s5n3pza4d6w', 'Plan draft', 'x', 'x', 1, '0', '7q3k9mzx2b4t', 1, 0);
             INSERT INTO versions (artifact_id, n, created_at, files_json)
                VALUES ('8r4m2nzy3c5v', 1, 'x', '{}'), ('9s5n3pza4d6w', 1, 'x', '{}');
             INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                VALUES ('7q3k9mzx2b4t', 'rows/1', 'rows', '{\"amount\":12}', 1, 'x');
             INSERT INTO db_read_grants (reader_id, source_id, granted_at)
                VALUES ('8r4m2nzy3c5v', '7q3k9mzx2b4t', 'x');
             INSERT INTO net_grants (artifact_id, host, granted_at)
                VALUES ('7q3k9mzx2b4t', 'api.example.com', 'x');
             INSERT INTO sample_grants (artifact_id, granted_at) VALUES ('7q3k9mzx2b4t', 'x');
             INSERT INTO sample_spend (artifact_id, day, micro_usd)
                VALUES ('7q3k9mzx2b4t', '2026-10-18', 1200);",
        )
        .unwrap();
        c.execute_batch(OPENED_BEFORE).unwrap();
        c.pragma_update(None, "journal_mode", "wal").unwrap();
    }

    /// What a home's first open by its store leaves in `install`: the audit
    /// backfill and the migration approval's record done.
    const OPENED_BEFORE: &str = "INSERT INTO install (k, v)
        VALUES ('backfill', '2026-10-18T09:00:00.000Z'), ('migration_approval', '1');";

    fn user_version(c: &Connection) -> u32 {
        c.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// A connection to `home`'s database that neither checkpoints as it
    /// writes nor when it closes: what it writes stays in the write-ahead
    /// log, as a process that ended without a checkpoint leaves it.
    fn connection_leaving_wal_frames(home: &Home) -> Connection {
        let c = Connection::open(home.db_path()).unwrap();
        c.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
        c.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        c
    }

    /// The size of `home`'s write-ahead log in bytes (0 when it has none).
    fn wal_frames(home: &Home) -> u64 {
        let wal = format!("{}-wal", home.db_path().display());
        std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0)
    }

    fn table_count(c: &Connection, name: &str) -> i64 {
        c.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn the_app_at_39_to_43_is_adopted_into_mains_numbering() {
        let fresh = fresh_shape();
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        installed_at_43(&home);
        let (cols, before) = {
            let old = Connection::open(home.db_path()).unwrap();
            assert_eq!(user_version(&old), 43);
            (columns_of(&old), every_row(&old))
        };

        drop(Store::open(&home).unwrap());
        let c = Connection::open(home.db_path()).unwrap();
        assert_eq!(user_version(&c), APP_MIGRATION + 4);
        assert_eq!(user_version(&c), MIGRATIONS.len() as u32);
        for table in [
            "legal_agreements",
            "briefs",
            "db_read_grants",
            "net_grants",
            "sample_grants",
        ] {
            assert_eq!(table_count(&c, table), 1, "{table}");
        }
        assert_eq!(shape_of(&c), fresh, "the final schema");
        assert_eq!(rows_in(&c, &cols), before, "every row kept");
        let approvals: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM approvals WHERE created_via = 'migration'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approvals, 1, "migration 25's step is not run again");
        assert_eq!(
            c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );

        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
        let name = copies[0].file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("clax-43-"), "{name}");
        let copy = Connection::open(&copies[0]).unwrap();
        assert_eq!(user_version(&copy), 43);
        assert_eq!(
            every_row(&copy),
            before,
            "the copy holds the home as it was"
        );
        assert_eq!(table_count(&copy, "legal_agreements"), 0);
    }

    #[test]
    fn an_adopted_home_is_never_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        installed_at_43(&home);
        drop(Store::open(&home).unwrap());
        let (version, schema, rows) = {
            let c = Connection::open(home.db_path()).unwrap();
            (user_version(&c), schema_of(&c), every_row(&c))
        };
        {
            let mut c = Connection::open(home.db_path()).unwrap();
            let tx = c.transaction().unwrap();
            assert_eq!(
                super::adopt_earlier_flutter_app(&tx, &super::NUMBERING).unwrap(),
                None
            );
        }
        drop(Store::open(&home).unwrap());
        let c = Connection::open(home.db_path()).unwrap();
        assert_eq!(user_version(&c), version);
        assert_eq!(schema_of(&c), schema);
        assert_eq!(every_row(&c), rows);
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "no second copy: {copies:?}");
    }

    /// A database at the installed builds' numbering holding the first
    /// `app` of [`INSTALLED_AT_43`]'s migrations, at version `38 + app`,
    /// with the rows of [`app_rows`]. Returns its rows.
    fn app_numbered_from_39(home: &Home, app: usize) -> Vec<(String, Vec<String>)> {
        home.ensure_dirs().unwrap();
        let mut c = Connection::open(home.db_path()).unwrap();
        c.pragma_update(None, "journal_mode", "wal").unwrap();
        let tx = c.transaction().unwrap();
        for v in 1..=WHERE_TO_ASK_MIGRATION {
            tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
            super::rust_step(&tx, v).unwrap();
        }
        for sql in &installed_migrations()[..app] {
            tx.execute_batch(sql).unwrap();
        }
        tx.pragma_update(None, "user_version", WHERE_TO_ASK_MIGRATION + app as u32)
            .unwrap();
        tx.commit().unwrap();
        app_rows(&c, app)
    }

    #[test]
    fn each_app_prefix_at_the_old_numbering_is_adopted() {
        assert_eq!(installed_migrations().len(), 5);
        for app in 1..=5 {
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            app_numbered_from_39(&home, app);
            let mut c = Connection::open(home.db_path()).unwrap();
            let tx = c.transaction().unwrap();
            let old = WHERE_TO_ASK_MIGRATION + app as u32;
            assert_eq!(
                super::adopt_earlier_flutter_app(&tx, &super::NUMBERING).unwrap(),
                Some((old, APP_MIGRATION - 1 + app as u32)),
                "from {old}"
            );
        }
        for app in 1..=5 {
            assert_numbered_app_database_upgrades(
                app,
                WHERE_TO_ASK_MIGRATION,
                app_numbered_from_39,
            );
        }
    }

    #[test]
    fn an_old_numbering_home_whose_marks_disagree_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        installed_at_43(&home);
        // The change is left in the write-ahead log, as a home whose last
        // process ended without a checkpoint has it. The store's connection
        // closing last checkpoints those frames into the file, so the file's
        // bytes change while its content does not: what is compared is the
        // content.
        let before = {
            let c = connection_leaving_wal_frames(&home);
            c.execute_batch("DROP TABLE sample_grants;").unwrap();
            (user_version(&c), schema_of(&c), every_row(&c))
        };
        assert!(wal_frames(&home) > 0, "the home holds frames in its WAL");
        let err = match Store::open(&home) {
            Ok(_) => panic!("a home whose schema disagrees with its version must be refused"),
            Err(e) => e,
        };
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at: 43, detail }
                if detail.contains("sample_grants")),
            "{err:?}"
        );
        let c = Connection::open(home.db_path()).unwrap();
        let after = (user_version(&c), schema_of(&c), every_row(&c));
        assert!(after == before, "left as it was: {after:?}");
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "copied before anything: {copies:?}");
        let clax_core::CoreError::MigrationFailed { detail, .. } = &err else {
            unreachable!()
        };
        assert!(
            detail.contains(&copies[0].display().to_string()),
            "the refusal names the copy: {detail}"
        );
        let copy = Connection::open(&copies[0]).unwrap();
        let copied = (user_version(&copy), schema_of(&copy), every_row(&copy));
        assert!(
            copied == before,
            "the copy holds the WAL's frames too: {copied:?}"
        );
    }

    #[test]
    fn a_home_of_this_numbering_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        drop(Store::open(&home).unwrap());
        let mut c = Connection::open(home.db_path()).unwrap();
        let tx = c.transaction().unwrap();
        assert_eq!(
            super::adopt_earlier_flutter_app(&tx, &super::NUMBERING).unwrap(),
            None
        );
        drop(tx);
        // Nor one part way through the app's migrations in this numbering.
        for v in WHERE_TO_ASK_MIGRATION..=MIGRATIONS.len() as u32 {
            let dir = tempfile::tempdir().unwrap();
            let home = Home::at(dir.path().join("ax"));
            home.ensure_dirs().unwrap();
            let mut c = Connection::open(home.db_path()).unwrap();
            let tx = c.transaction().unwrap();
            for n in 1..=v {
                tx.execute_batch(MIGRATIONS[n as usize - 1]).unwrap();
                super::rust_step(&tx, n).unwrap();
            }
            tx.pragma_update(None, "user_version", v).unwrap();
            assert_eq!(
                super::adopt_earlier_flutter_app(&tx, &super::NUMBERING).unwrap(),
                None,
                "at {v}"
            );
        }
    }

    #[test]
    fn a_home_at_mains_40_gets_the_apps_migrations_after_a_copy() {
        let fresh = fresh_shape();
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let main = APP_MIGRATION - 1;
        {
            let mut c = Connection::open(home.db_path()).unwrap();
            c.pragma_update(None, "journal_mode", "wal").unwrap();
            let tx = c.transaction().unwrap();
            for v in 1..=main {
                tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
                super::rust_step(&tx, v).unwrap();
            }
            tx.pragma_update(None, "user_version", main).unwrap();
            tx.execute_batch(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                    VALUES ('7q3k9mzx2b4t', 'Budget', 'x', 'x', 1, '0');
                 INSERT INTO versions (artifact_id, n, created_at, files_json)
                    VALUES ('7q3k9mzx2b4t', 1, 'x', '{}');
                 INSERT INTO threads (id, artifact_id, version_n, anchor_json, created_at)
                    VALUES ('t1', '7q3k9mzx2b4t', 1, '{}', 'x');
                 INSERT INTO comments (id, thread_id, author_kind, author_name, body, created_at)
                    VALUES ('c1', 't1', 'viewer', 'Alex', 'make it green', 'x');
                 INSERT INTO legal_agreements (viewer_id, version, agreed_at, via)
                    VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAO', '2026-10-01', 'x', 'signin');
                 INSERT INTO briefs (artifact_id, starter, key, created_at)
                    VALUES ('7q3k9mzx2b4t', '*owner', 'k1', 'x');",
            )
            .unwrap();
            tx.execute_batch(OPENED_BEFORE).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(main, 40, "main's numbering ends at 40");
        let (cols, before) = {
            let old = Connection::open(home.db_path()).unwrap();
            (columns_of(&old), every_row(&old))
        };

        drop(Store::open(&home).unwrap());
        let c = Connection::open(home.db_path()).unwrap();
        assert_eq!(user_version(&c), MIGRATIONS.len() as u32);
        assert_eq!(shape_of(&c), fresh, "the final schema");
        assert_eq!(rows_in(&c, &cols), before, "every row kept");
        let copies = crate::backup::list(&crate::backup::dir(&home));
        assert_eq!(copies.len(), 1, "{copies:?}");
        let name = copies[0].file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("clax-40-"), "{name}");
        assert_eq!(every_row(&Connection::open(&copies[0]).unwrap()), before);
    }

    /// A migration main might add after its 40, for tests of a later
    /// numbering: a plain table, which fails if it is run twice.
    const SIMULATED_MAIN_41: &str = "CREATE TABLE simulated_main_41 (
        id TEXT PRIMARY KEY,
        created_at TEXT NOT NULL
    );";

    /// Migrations main might add after its 40 that make no table, each with
    /// its mark: a column of a table other than `artifacts`, an index, and
    /// one that changes only rows.
    const SIMULATED_MAINS_41_WITHOUT_A_TABLE: [(&str, super::Mark<'static>); 3] = [
        (
            "ALTER TABLE sessions ADD COLUMN simulated_main_41 TEXT;",
            super::Mark::Column("sessions", "simulated_main_41"),
        ),
        (
            "CREATE INDEX simulated_main_41 ON comments(created_at);",
            super::Mark::Index("simulated_main_41"),
        ),
        (
            "UPDATE artifacts SET title = trim(title);
             INSERT INTO install (k, v) VALUES ('simulated_main_41', '1');",
            super::Mark::Row {
                table: "install",
                predicate: "k = 'simulated_main_41'",
            },
        ),
    ];

    /// [`MIGRATIONS`] as a build would have them after taking a 41 more
    /// of main's ([`SIMULATED_MAIN_41`]): main's 1 to 40, it, then the
    /// app's from 42; and main's marks above 38 with its mark last.
    fn with_a_main_41() -> (Vec<&'static str>, Vec<super::Mark<'static>>) {
        with_main_41(SIMULATED_MAIN_41, super::Mark::Table("simulated_main_41"))
    }

    /// [`MIGRATIONS`] after taking `sql` as main's 41, with its `mark`.
    fn with_main_41(
        sql: &'static str,
        mark: super::Mark<'static>,
    ) -> (Vec<&'static str>, Vec<super::Mark<'static>>) {
        let app = APP_MIGRATION as usize - 1;
        let mut migrations = MIGRATIONS[..app].to_vec();
        migrations.push(sql);
        migrations.extend_from_slice(&MIGRATIONS[app..]);
        let mut marks = MAIN_MARKS_ABOVE_38.to_vec();
        marks.push(mark);
        (migrations, marks)
    }

    /// A fresh database's [`shape_of`] under `numbering`.
    fn fresh_shape_of(numbering: &super::Numbering<'_>) -> Vec<String> {
        let mut c = Connection::open_in_memory().unwrap();
        crate::migrate_with(&mut c, numbering).unwrap();
        shape_of(&c)
    }

    /// Opens `home` as a build with `numbering` does: a copy first, then
    /// the migrations. Returns the copy.
    fn open_under(home: &Home, numbering: &super::Numbering<'_>) -> std::path::PathBuf {
        let mut c = Connection::open(home.db_path()).unwrap();
        let now = "2026-11-02T00:00:00Z".parse().unwrap();
        let copy = crate::backup::before_migrate(&c, home, numbering.migrations.len() as u32, now)
            .unwrap()
            .expect("a copy before the upgrade");
        crate::migrate_with(&mut c, numbering).unwrap();
        copy
    }

    #[test]
    fn a_home_of_this_numbering_takes_a_later_main_migration_and_the_apps_renumbering() {
        let (migrations, marks) = with_a_main_41();
        let later = super::Numbering {
            migrations: &migrations,
            main_marks: &marks,
        };
        assert_eq!(later.app_first(), APP_MIGRATION + 1);
        let fresh = fresh_shape_of(&later);

        // The owner's home after this build's first open: 43 adopted to 45.
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        installed_at_43(&home);
        drop(Store::open(&home).unwrap());
        let (cols, before) = {
            let c = Connection::open(home.db_path()).unwrap();
            assert_eq!(user_version(&c), 45);
            (columns_of(&c), every_row(&c))
        };

        let copy = open_under(&home, &later);
        let c = Connection::open(home.db_path()).unwrap();
        assert_eq!(user_version(&c), 46, "the app's 45 is its 46 now");
        assert_eq!(shape_of(&c), fresh, "the later build's schema");
        assert_eq!(rows_in(&c, &cols), before, "every row kept");
        assert_eq!(table_count(&c, "simulated_main_41"), 1);
        let approvals: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM approvals WHERE created_via = 'migration'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approvals, 1, "no earlier migration runs again");
        assert_eq!(
            c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        let copied = Connection::open(&copy).unwrap();
        assert_eq!(user_version(&copied), 45);
        assert_eq!(
            every_row(&copied),
            before,
            "the copy holds the home as it was"
        );

        // Opened again by the later build, nothing changes.
        drop(c);
        let mut c = Connection::open(home.db_path()).unwrap();
        let (schema, rows) = (schema_of(&c), every_row(&c));
        crate::migrate_with(&mut c, &later).unwrap();
        assert_eq!(
            (user_version(&c), schema_of(&c), every_row(&c)),
            (46, schema, rows)
        );
    }

    #[test]
    fn every_layout_adopts_into_a_later_main_numbering() {
        let (migrations, marks) = with_a_main_41();
        let later = super::Numbering {
            migrations: &migrations,
            main_marks: &marks,
        };
        let fresh = fresh_shape_of(&later);
        let last = migrations.len() as u32;
        // Each layout: how a home of it is made, and the version it has
        // with the first `app` of the app's migrations.
        type Layout = (&'static str, MakeNumbered, fn(usize) -> u32);
        let layouts: [Layout; 5] = [
            ("from 24", pre_rebase_flutter_app, |app| {
                AUDIT_MIGRATION + app as u32
            }),
            ("from 27", earlier_app_numbering, |app| {
                INBOX_MIGRATION + app as u32
            }),
            ("from 38", app_numbered_from_38, |app| {
                REPAIR_MIGRATION + app as u32
            }),
            ("from 39", app_numbered_from_39, |app| {
                WHERE_TO_ASK_MIGRATION + app as u32
            }),
            ("from 41", app_numbered_from_41, |app| {
                APP_MIGRATION - 1 + app as u32
            }),
        ];
        for (layout, make, version) in layouts {
            for app in 1..=super::APP_MIGRATION_MARKS.len() {
                let dir = tempfile::tempdir().unwrap();
                let home = Home::at(dir.path().join("ax"));
                let before = make(&home, app);
                let old = version(app);
                {
                    let mut c = Connection::open(home.db_path()).unwrap();
                    assert_eq!(user_version(&c), old, "{layout}, {app}");
                    let tx = c.transaction().unwrap();
                    assert_eq!(
                        super::adopt_earlier_flutter_app(&tx, &later).unwrap(),
                        Some((old, later.app_first() - 1 + app as u32)),
                        "{layout}, {app}"
                    );
                }
                open_under(&home, &later);
                let c = Connection::open(home.db_path()).unwrap();
                assert_eq!(user_version(&c), last, "{layout}, {app}");
                assert_eq!(shape_of(&c), fresh, "{layout}, {app}");
                assert_eq!(kept_rows(&c, app), before, "{layout}, {app}");
            }
        }
    }

    /// The Clax app's migrations as this numbering has them, at 41 to 45
    /// after main's 39 and 40, frozen: the layout the owner's devices hold
    /// once this build has opened their homes.
    const APP_AT_41: &str =
        include_str!("../tests/fixtures/old-migrations/flutter-app-4479a586c.sql");

    /// The SQL of each of the app's migrations in [`APP_AT_41`], in order.
    fn app_migrations() -> Vec<&'static str> {
        APP_AT_41
            .split("\n-- migration: ")
            .skip(1)
            .map(|part| part.split_once('\n').unwrap().1)
            .collect()
    }

    /// A database as a build holding the first `k` of main's migrations
    /// above the where-to-ask one left it: migrations 1 to `38 + k` with
    /// their Rust steps, then the first `app` of the app's ([`APP_AT_41`]),
    /// numbered from `38 + k + 1`, with the rows of [`app_rows`]. Returns
    /// its rows.
    fn app_after_main(home: &Home, k: usize, app: usize) -> Vec<(String, Vec<String>)> {
        home.ensure_dirs().unwrap();
        let mut c = Connection::open(home.db_path()).unwrap();
        c.pragma_update(None, "journal_mode", "wal").unwrap();
        let tx = c.transaction().unwrap();
        let main = WHERE_TO_ASK_MIGRATION + k as u32;
        for v in 1..=main {
            tx.execute_batch(MIGRATIONS[v as usize - 1]).unwrap();
            super::rust_step(&tx, v).unwrap();
        }
        for sql in &app_migrations()[..app] {
            tx.execute_batch(sql).unwrap();
        }
        tx.pragma_update(None, "user_version", main + app as u32)
            .unwrap();
        tx.commit().unwrap();
        app_rows(&c, app)
    }

    /// A database at this build's numbering, the app's at 41 to 45 after
    /// main's 39 and 40, holding the first `app` of the app's migrations,
    /// at version `40 + app`, with the rows of [`app_rows`]. Returns its
    /// rows.
    fn app_numbered_from_41(home: &Home, app: usize) -> Vec<(String, Vec<String>)> {
        app_after_main(home, 2, app)
    }

    #[test]
    fn the_frozen_app_migrations_are_this_numberings() {
        let frozen = app_migrations();
        assert_eq!(frozen.len(), super::APP_MIGRATION_MARKS.len());
        let first = APP_MIGRATION as usize - 1;
        for (i, sql) in frozen.iter().enumerate() {
            // Each is followed by the line break before the next.
            assert_eq!(
                sql.trim_end(),
                MIGRATIONS[first + i],
                "the app's migration {}",
                i + 1
            );
        }
    }

    /// Every layout of the app's migrations after the first k of main's
    /// above 38, for k up to the number this numbering has, opens under it:
    /// copied first, adopted for a k below that number, migrated to the
    /// final schema, every row kept.
    #[test]
    fn the_app_after_every_k_of_mains_migrations_opens_under_this_numbering() {
        let fresh = fresh_shape();
        for k in 0..=MAIN_MARKS_ABOVE_38.len() {
            for app in 1..=super::APP_MIGRATION_MARKS.len() {
                let dir = tempfile::tempdir().unwrap();
                let home = Home::at(dir.path().join("ax"));
                let before = app_after_main(&home, k, app);
                let old = WHERE_TO_ASK_MIGRATION + (k + app) as u32;
                {
                    let mut c = Connection::open(home.db_path()).unwrap();
                    assert_eq!(user_version(&c), old, "k {k}, app {app}");
                    let tx = c.transaction().unwrap();
                    let adopted = (k < MAIN_MARKS_ABOVE_38.len())
                        .then_some((old, APP_MIGRATION - 1 + app as u32));
                    assert_eq!(
                        super::adopt_earlier_flutter_app(&tx, &super::NUMBERING).unwrap(),
                        adopted,
                        "k {k}, app {app}"
                    );
                }
                drop(Store::open(&home).unwrap());
                let c = Connection::open(home.db_path()).unwrap();
                assert_eq!(
                    user_version(&c),
                    MIGRATIONS.len() as u32,
                    "k {k}, app {app}"
                );
                assert_eq!(shape_of(&c), fresh, "k {k}, app {app}");
                assert_eq!(kept_rows(&c, app), before, "k {k}, app {app}");
                let copies = crate::backup::list(&crate::backup::dir(&home));
                let copied = copies.iter().any(|p| {
                    p.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with(&format!("clax-{old}-"))
                });
                let last = MIGRATIONS.len() as u32;
                assert_eq!(copied, old < last, "k {k}, app {app}: {copies:?}");
            }
        }
    }

    #[test]
    fn a_home_holding_mains_later_migrations_without_an_earlier_one_is_refused() {
        let (migrations, marks) = with_a_main_41();
        let later = super::Numbering {
            migrations: &migrations,
            main_marks: &marks,
        };
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        app_numbered_from_41(&home, 5);
        let mut c = Connection::open(home.db_path()).unwrap();
        c.execute_batch("DROP TABLE legal_agreements;").unwrap();
        let before = (schema_of(&c), every_row(&c));
        let tx = c.transaction().unwrap();
        let err = super::adopt_earlier_flutter_app(&tx, &later).unwrap_err();
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at: 45, detail }
                if detail.contains("briefs") && detail.contains("legal_agreements")),
            "{err:?}"
        );
        drop(tx);
        assert_eq!((schema_of(&c), every_row(&c)), before);
    }

    #[test]
    fn a_home_holding_a_main_mark_above_38_without_questions_thread_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        // The app's from 38: no questions.thread_id, which main's 38 adds
        // before its 39 makes legal_agreements.
        app_numbered_from_38(&home, 5);
        let before = {
            let c = connection_leaving_wal_frames(&home);
            c.execute_batch(MIGRATIONS[WHERE_TO_ASK_MIGRATION as usize])
                .unwrap();
            assert_eq!(table_count(&c, "legal_agreements"), 1);
            (user_version(&c), schema_of(&c), every_row(&c))
        };
        let err = match Store::open(&home) {
            Ok(_) => panic!("a home holding main's 39 without its 38 must be refused"),
            Err(e) => e,
        };
        let at = REPAIR_MIGRATION + 5;
        assert!(
            matches!(&err, clax_core::CoreError::MigrationFailed { at: a, detail }
                if *a == at
                    && detail.contains("legal_agreements")
                    && detail.contains("lacks questions.thread_id")),
            "{err:?}"
        );
        let c = Connection::open(home.db_path()).unwrap();
        let after = (user_version(&c), schema_of(&c), every_row(&c));
        assert!(after == before, "left as it was: {after:?}");
    }

    #[test]
    fn a_mark_is_a_table_a_column_of_any_table_an_index_or_a_row() {
        use super::Mark;
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE install (k TEXT PRIMARY KEY, v TEXT);
             CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT);
             CREATE INDEX sessions_by_cwd ON sessions(cwd);
             INSERT INTO install (k, v) VALUES ('id', 'x');",
        )
        .unwrap();
        let held = |m: Mark<'_>| super::has_mark(&c, &m).unwrap();
        assert!(held(Mark::Table("sessions")));
        assert!(!held(Mark::Table("briefs")));
        assert!(held(Mark::Column("sessions", "cwd")));
        assert!(!held(Mark::Column("sessions", "harness")));
        assert!(!held(Mark::Column("artifacts", "cwd")));
        assert!(held(Mark::Index("sessions_by_cwd")));
        assert!(!held(Mark::Index("sessions")), "a table is no index");
        assert!(
            !held(Mark::Table("sessions_by_cwd")),
            "an index is no table"
        );
        let row = |predicate| Mark::Row {
            table: "install",
            predicate,
        };
        assert!(held(row("k = 'id' AND v = 'x'")));
        assert!(!held(row("k = 'backfill'")));
        assert!(
            !held(Mark::Row {
                table: "briefs",
                predicate: "1"
            }),
            "no row of a table the database lacks"
        );
    }

    /// A main migration above 38 that makes no table is marked by the
    /// column, index or row it leaves: a home of this numbering takes it as
    /// it takes one that makes a table, and every earlier layout of the
    /// app's migrations is adopted around it.
    #[test]
    fn a_later_main_migration_making_no_table_is_marked_and_adopted_around() {
        for (sql, mark) in SIMULATED_MAINS_41_WITHOUT_A_TABLE {
            let (migrations, marks) = with_main_41(sql, mark);
            let later = super::Numbering {
                migrations: &migrations,
                main_marks: &marks,
            };
            assert_marks_made(&later);
            let fresh = fresh_shape_of(&later);
            for (k, app) in [(2, 5), (2, 1), (1, 3), (0, 5)] {
                let dir = tempfile::tempdir().unwrap();
                let home = Home::at(dir.path().join("ax"));
                let before = app_after_main(&home, k, app);
                let old = WHERE_TO_ASK_MIGRATION + (k + app) as u32;
                {
                    let mut c = Connection::open(home.db_path()).unwrap();
                    let tx = c.transaction().unwrap();
                    assert_eq!(
                        super::adopt_earlier_flutter_app(&tx, &later).unwrap(),
                        Some((old, later.app_first() - 1 + app as u32)),
                        "{mark}: k {k}, app {app}"
                    );
                }
                open_under(&home, &later);
                let mut c = Connection::open(home.db_path()).unwrap();
                assert_eq!(
                    user_version(&c),
                    migrations.len() as u32,
                    "{mark}: k {k}, app {app}"
                );
                assert_eq!(shape_of(&c), fresh, "{mark}: k {k}, app {app}");
                assert_eq!(kept_rows(&c, app), before, "{mark}: k {k}, app {app}");
                assert!(super::has_mark(&c, &mark).unwrap(), "{mark}");
                // Opened again, it is this numbering's: nothing runs twice.
                let (schema, rows) = (schema_of(&c), every_row(&c));
                crate::migrate_with(&mut c, &later).unwrap();
                assert_eq!((schema_of(&c), every_row(&c)), (schema, rows), "{mark}");
            }
        }
    }
}

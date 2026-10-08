-- The single-owner golden's fixture: a migration-21 Clax database and
-- the files its rows name. Written by golden_single_owner.rs's
-- write_fixture; loaded by clax_server::testing::load_fixture.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE artifacts (
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
    , doc_seq INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'html'
        CHECK (kind IN ('html', 'live')));
CREATE TABLE versions (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        n INTEGER NOT NULL,
        label TEXT,
        created_at TEXT NOT NULL,
        session_id TEXT,
        files_json TEXT NOT NULL, note TEXT,
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
CREATE INDEX assets_by_artifact ON assets(artifact_id);
CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        harness TEXT NOT NULL,
        harness_session_id TEXT,
        cwd TEXT NOT NULL,
        pid INTEGER,
        parent_pid INTEGER,
        started_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL,
        ended_at TEXT
    , agent_handle TEXT);
CREATE UNIQUE INDEX sessions_harness_id ON sessions(harness, harness_session_id)
        WHERE harness_session_id IS NOT NULL AND ended_at IS NULL;
CREATE TABLE watches (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        replies_armed INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'direct'
        CHECK (source IN ('direct', 'scope')),
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
    , target_session_id TEXT, live_path TEXT);
CREATE INDEX threads_by_artifact ON threads(artifact_id, created_at, id);
CREATE TABLE comments (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        author_kind TEXT NOT NULL CHECK (author_kind IN ('viewer', 'agent')),
        author_name TEXT NOT NULL,
        via_session_id TEXT,
        body TEXT NOT NULL,
        created_at TEXT NOT NULL
    , via_page INTEGER NOT NULL DEFAULT 0, author_public_id TEXT);
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
    , push_failed_at TEXT, notified_at TEXT, batch_id TEXT);
CREATE UNIQUE INDEX feedback_comment_target ON feedback(comment_id, target_session_id)
        WHERE target_session_id IS NOT NULL;
CREATE INDEX feedback_by_target ON feedback(target_session_id, delivered_at);
CREATE INDEX feedback_by_thread ON feedback(thread_id, created_at, id);
CREATE TABLE viewers (
        id TEXT PRIMARY KEY,
        display_name TEXT,
        created_at TEXT NOT NULL
    , public_id TEXT, owner INTEGER NOT NULL DEFAULT 0, claimed INTEGER NOT NULL DEFAULT 0, minted_local INTEGER);
CREATE TABLE session_env (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        codex_home TEXT
    , push_error TEXT, push_error_at TEXT);
CREATE UNIQUE INDEX viewers_public_id ON viewers(public_id);
CREATE TABLE docs (
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
    );
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
CREATE INDEX viewer_seen_by_age ON viewer_seen(viewer_id, updated_at);
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
CREATE UNIQUE INDEX sessions_by_handle ON sessions(agent_handle);
CREATE TABLE send_batches (
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
CREATE UNIQUE INDEX viewers_one_owner ON viewers(owner) WHERE owner = 1;
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
CREATE INDEX live_picks_by_time ON live_picks(created_at);
CREATE TABLE extension_credentials (
        id TEXT PRIMARY KEY,
        extension_id TEXT NOT NULL,
        secret_sha256 TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        last_used_at TEXT NOT NULL,
        revoked_at TEXT
    );
CREATE INDEX extension_credentials_by_extension
        ON extension_credentials(extension_id, created_at);
CREATE TABLE live_rules (
        id TEXT PRIMARY KEY,
        origin TEXT NOT NULL,
        pattern TEXT NOT NULL,
        created_at TEXT NOT NULL,
        deleted_at TEXT,
        UNIQUE (origin, pattern)
    );
CREATE INDEX live_picks_by_thread ON live_picks(thread_id);
CREATE TABLE live_sites (
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
CREATE TABLE "thread_moves" (
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
CREATE INDEX thread_moves_by_thread ON thread_moves(thread_id, created_at, id);
CREATE TABLE questions (
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
CREATE INDEX questions_closed ON questions(closed_at, id) WHERE status <> 'open';
CREATE TABLE inbox_items (
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
INSERT INTO "artifacts" ("id", "title", "description", "icon", "created_at", "updated_at", "current_version", "owner_session_id", "pinned", "capabilities_json", "contract_version", "deleted_at", "doc_seq", "kind") VALUES ('8nzk3a9npfmy', 'Launch Plan', 'Steps to ship v2', NULL, '2026-09-14T09:01:00.000Z', '2026-09-14T09:01:00.000Z', 1, '01M4ER09NFCF39DKTE1PBC3Y2A', 0, '{}', '0.2.61', NULL, 0, 'html');
INSERT INTO "artifacts" ("id", "title", "description", "icon", "created_at", "updated_at", "current_version", "owner_session_id", "pinned", "capabilities_json", "contract_version", "deleted_at", "doc_seq", "kind") VALUES ('ky9gdxtxwxqm', 'Quarterly Review', 'Q3 numbers for the board', NULL, '2026-09-14T09:03:40.000Z', '2026-09-14T09:06:30.000Z', 2, '01M4ER09P52ATB4HTRYZ2CM0CF', 0, '{}', '0.2.61', NULL, 0, 'html');
INSERT INTO "artifacts" ("id", "title", "description", "icon", "created_at", "updated_at", "current_version", "owner_session_id", "pinned", "capabilities_json", "contract_version", "deleted_at", "doc_seq", "kind") VALUES ('zafbtm2ff2jg', 'Team Tracker', NULL, NULL, '2026-09-14T09:04:10.000Z', '2026-09-14T09:04:10.000Z', 1, '01M4ER09P52ATB4HTRYZ2CM0CF', 0, '{"comments":{},"db":{}}', '0.2.61', NULL, 4, 'html');
INSERT INTO "versions" ("artifact_id", "n", "label", "created_at", "session_id", "files_json", "note") VALUES ('8nzk3a9npfmy', 1, NULL, '2026-09-14T09:01:00.000Z', '01M4ER09NFCF39DKTE1PBC3Y2A', '{"index.html":{"content_type":"text/html","size":151}}', NULL);
INSERT INTO "versions" ("artifact_id", "n", "label", "created_at", "session_id", "files_json", "note") VALUES ('ky9gdxtxwxqm', 1, NULL, '2026-09-14T09:03:40.000Z', '01M4ER09P52ATB4HTRYZ2CM0CF', '{"index.html":{"content_type":"text/html","size":247},"style.css":{"content_type":"text/css","size":26}}', NULL);
INSERT INTO "versions" ("artifact_id", "n", "label", "created_at", "session_id", "files_json", "note") VALUES ('zafbtm2ff2jg', 1, NULL, '2026-09-14T09:04:10.000Z', '01M4ER09P52ATB4HTRYZ2CM0CF', '{"index.html":{"content_type":"text/html","size":126}}', NULL);
INSERT INTO "versions" ("artifact_id", "n", "label", "created_at", "session_id", "files_json", "note") VALUES ('ky9gdxtxwxqm', 2, NULL, '2026-09-14T09:06:30.000Z', '01M4ER09P52ATB4HTRYZ2CM0CF', '{"index.html":{"content_type":"text/html","size":293},"style.css":{"content_type":"text/css","size":64}}', 'Sticky header');
INSERT INTO "sessions" ("id", "harness", "harness_session_id", "cwd", "pid", "parent_pid", "started_at", "last_seen_at", "ended_at", "agent_handle") VALUES ('01M4ER09NFCF39DKTE1PBC3Y2A', 'codex', 'codex-thread-1', '/work/site', 51000, 50999, '2026-09-14T09:01:00.000Z', '2026-09-14T09:01:00.000Z', '2026-09-14T09:17:45.000Z', 'a_953636eda21ef28dae1d09');
INSERT INTO "sessions" ("id", "harness", "harness_session_id", "cwd", "pid", "parent_pid", "started_at", "last_seen_at", "ended_at", "agent_handle") VALUES ('01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', 'golden-live', '/work/web', 4242, 4241, '2026-09-14T09:03:40.000Z', '2026-09-14T09:03:40.000Z', NULL, 'a_2798ad9cb299e79c529faa');
INSERT INTO "watches" ("session_id", "artifact_id", "replies_armed", "created_at", "source") VALUES ('01M4ER09P52ATB4HTRYZ2CM0CF', 'ky9gdxtxwxqm', 1, '2026-09-14T09:03:40.000Z', 'direct');
INSERT INTO "watches" ("session_id", "artifact_id", "replies_armed", "created_at", "source") VALUES ('01M4ER09P52ATB4HTRYZ2CM0CF', 'zafbtm2ff2jg', 1, '2026-09-14T09:04:10.000Z', 'direct');
INSERT INTO "watches" ("session_id", "artifact_id", "replies_armed", "created_at", "source") VALUES ('01M4ER09P52ATB4HTRYZ2CM0CF', '8nzk3a9npfmy', 0, '2026-09-14T09:04:40.000Z', 'direct');
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09NMMK2SRX0YZGWMACN4', '8nzk3a9npfmy', 1, '{"kind":"element","selector":"body > main > h2","quote":"Launch steps","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:02:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09NZD56H6PRNQSZ17610', '8nzk3a9npfmy', 1, '{"kind":"element","selector":"body > main > ol > li:nth-child(1)","quote":"Freeze","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:03:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09P1JHJCFYM4ZZ1B508B', '8nzk3a9npfmy', 1, '{"kind":"element","selector":"body > main > ol","quote":"Ship","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:03:30.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09PEN8CKDJKB6RXG5PSH', 'ky9gdxtxwxqm', 1, '{"kind":"element","selector":"body > main > h2","quote":"Quarterly goals","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'resolved', 1, 1, '2026-09-14T09:05:10.000Z', '2026-09-14T09:06:50.000Z', 'agent:claude', NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09Q0VECY3D0CH3FSAQRJ', 'ky9gdxtxwxqm', 2, '{"kind":"element","selector":"body > main > table","quote":"Q3","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:07:50.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09Q9S422V6HNVYSWP9XW', 'ky9gdxtxwxqm', 2, '{"kind":"element","selector":"body > header","quote":"Quarterly Review","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:14:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09QEC13JNFQPNE4PX7B4', 'ky9gdxtxwxqm', 2, '{"kind":"element","selector":"body > main > p.lede","quote":"Revenue grew 12% on Q2.","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:14:30.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09QJ1BAKE4FM5RH8TBJS', 'zafbtm2ff2jg', 1, '{"kind":"element","selector":"body > main > h2","quote":"Tasks","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:15:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09QYYMTW0D3DPAQHHFG8', 'zafbtm2ff2jg', 1, '{"kind":"element","selector":"body > main > h2","quote":"Tasks","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:15:35.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "threads" ("id", "artifact_id", "version_n", "anchor_json", "status", "sent_to_agent", "has_clip", "created_at", "resolved_at", "resolved_by", "target_session_id", "live_path") VALUES ('01M4ER09R121ZFF148BQ9MTJM7', 'zafbtm2ff2jg', 1, '{"kind":"element","selector":"#tasks","quote":"","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}', 'open', 1, 0, '2026-09-14T09:15:35.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09NN8X91G50FNX8AG2YZ', '01M4ER09NMMK2SRX0YZGWMACN4', 'viewer', 'Alex', NULL, 'Add a rollback step.', '2026-09-14T09:02:00.000Z', 0, NULL);
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09NWWE7FZN0MH8APN275', '01M4ER09NMMK2SRX0YZGWMACN4', 'agent', 'codex', '01M4ER09NFCF39DKTE1PBC3Y2A', 'Added a rollback step after Ship.', '2026-09-14T09:02:30.000Z', 0, NULL);
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09NZD56H6PRNQSZ17611', '01M4ER09NZD56H6PRNQSZ17610', 'viewer', 'Alex', NULL, 'Freeze on Thursday, not Friday.', '2026-09-14T09:03:00.000Z', 0, NULL);
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09P1JHJCFYM4ZZ1B508C', '01M4ER09P1JHJCFYM4ZZ1B508B', 'viewer', 'Alex', NULL, 'Name the release owner.', '2026-09-14T09:03:30.000Z', 0, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09PEN8CKDJKB6RXG5PSJ', '01M4ER09PEN8CKDJKB6RXG5PSH', 'viewer', 'Alex', NULL, 'Make the header sticky.', '2026-09-14T09:05:10.000Z', 0, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09PMKXXVACB2PXJXYZ09', '01M4ER09PEN8CKDJKB6RXG5PSH', 'agent', 'claude', '01M4ER09P52ATB4HTRYZ2CM0CF', 'Done: the header now sticks to the top.', '2026-09-14T09:06:10.000Z', 0, NULL);
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09Q0VECY3D0CH3FSAQRK', '01M4ER09Q0VECY3D0CH3FSAQRJ', 'viewer', 'Mia', NULL, 'The Q3 number looks off; finance has 3.9M.', '2026-09-14T09:07:50.000Z', 0, 'u_0443938d6040967e3d00ee');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09Q2MVQG8S9868SBETX8', '01M4ER09Q0VECY3D0CH3FSAQRJ', 'viewer', 'Viewer', NULL, 'Agreed, and Q2 too.', '2026-09-14T09:08:20.000Z', 0, 'u_64421e510b74e6c1d71ce1');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09Q9S422V6HNVYSWP9XX', '01M4ER09Q9S422V6HNVYSWP9XW', 'viewer', 'Alex', NULL, 'Use the brand blue for the header.', '2026-09-14T09:14:00.000Z', 0, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09QEC13JNFQPNE4PX7B5', '01M4ER09QEC13JNFQPNE4PX7B4', 'viewer', 'Alex', NULL, 'Say "year on year" here.', '2026-09-14T09:14:30.000Z', 0, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09QJ1BAKE4FM5RH8TBJT', '01M4ER09QJ1BAKE4FM5RH8TBJS', 'viewer', 'Alex', NULL, 'Task "Ship v2" is blocked on review.', '2026-09-14T09:15:00.000Z', 1, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09QMCNQ2BJR6JS0Y7EP0', '01M4ER09QJ1BAKE4FM5RH8TBJS', 'viewer', 'Alex', NULL, 'Unblocked: review done.', '2026-09-14T09:15:00.000Z', 1, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09QYYMTW0D3DPAQHHFG9', '01M4ER09QYYMTW0D3DPAQHHFG8', 'viewer', 'Alex', NULL, 'Group tasks by owner.', '2026-09-14T09:15:35.000Z', 0, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "comments" ("id", "thread_id", "author_kind", "author_name", "via_session_id", "body", "created_at", "via_page", "author_public_id") VALUES ('01M4ER09R121ZFF148BQ9MTJM8', '01M4ER09R121ZFF148BQ9MTJM7', 'viewer', 'Alex', NULL, 'Show done tasks struck through.', '2026-09-14T09:15:35.000Z', 0, 'u_5b345bfb8aecd190a917ae');
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09NQT2ERSWPKKKX7TG6X', '01M4ER09NMMK2SRX0YZGWMACN4', '01M4ER09NN8X91G50FNX8AG2YZ', '01M4ER09NFCF39DKTE1PBC3Y2A', '2026-09-14T09:02:00.000Z', '2026-09-14T09:02:00.000Z', 'piggyback', '2026-09-14T09:02:00.000Z', 0, '2026-09-14T09:02:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09P3566VJPK6EXDH7MBE', '01M4ER09P1JHJCFYM4ZZ1B508B', '01M4ER09P1JHJCFYM4ZZ1B508C', NULL, '2026-09-14T09:03:30.000Z', NULL, NULL, NULL, 0, NULL, '2026-09-14T09:17:45.000Z', NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09PGZ19M0E3WX9R77ESV', '01M4ER09PEN8CKDJKB6RXG5PSH', '01M4ER09PEN8CKDJKB6RXG5PSJ', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:05:10.000Z', '2026-09-14T09:05:30.000Z', 'piggyback', '2026-09-14T09:05:30.000Z', 0, '2026-09-14T09:05:30.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09Q4T5M7YDR1K37GBHQF', '01M4ER09Q0VECY3D0CH3FSAQRJ', '01M4ER09Q0VECY3D0CH3FSAQRK', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:08:50.000Z', '2026-09-14T09:09:00.000Z', 'prompt_hook', NULL, 0, '2026-09-14T09:09:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09Q5PPSHR5ADQS6C73T2', '01M4ER09Q0VECY3D0CH3FSAQRJ', '01M4ER09Q2MVQG8S9868SBETX8', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:08:50.000Z', '2026-09-14T09:09:00.000Z', 'prompt_hook', NULL, 0, '2026-09-14T09:09:00.000Z', NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09QAQH2CQM9QKZXZR540', '01M4ER09Q9S422V6HNVYSWP9XW', '01M4ER09Q9S422V6HNVYSWP9XX', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:14:00.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, '2026-09-14T09:14:00.000Z', NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09QG6EC5BYDNF3R1M17J', '01M4ER09QEC13JNFQPNE4PX7B4', '01M4ER09QEC13JNFQPNE4PX7B5', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:14:30.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09QPH4HF4YMWCQSVTGAY', '01M4ER09QJ1BAKE4FM5RH8TBJS', '01M4ER09QJ1BAKE4FM5RH8TBJT', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:15:00.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09QPH4HF4YMWCQSVTGAZ', '01M4ER09QJ1BAKE4FM5RH8TBJS', '01M4ER09QMCNQ2BJR6JS0Y7EP0', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:15:00.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, NULL);
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09R307VDPE2QK1VRRYB9', '01M4ER09QYYMTW0D3DPAQHHFG8', '01M4ER09QYYMTW0D3DPAQHHFG9', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:15:35.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, '01M4ER09R2AY07ZF41MP5Z0GY8');
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09R307VDPE2QK1VRRYBA', '01M4ER09R121ZFF148BQ9MTJM7', '01M4ER09R121ZFF148BQ9MTJM8', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:15:35.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, '01M4ER09R2AY07ZF41MP5Z0GY8');
INSERT INTO "feedback" ("id", "thread_id", "comment_id", "target_session_id", "created_at", "delivered_at", "delivery_tier", "acknowledged_at", "resend_count", "last_sent_at", "untargeted_at", "push_failed_at", "notified_at", "batch_id") VALUES ('01M4ER09R6VZKHW6K5MTPPZVCH', '01M4ER09NZD56H6PRNQSZ17610', '01M4ER09NZD56H6PRNQSZ17611', '01M4ER09P52ATB4HTRYZ2CM0CF', '2026-09-14T09:15:55.000Z', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, NULL);
INSERT INTO "viewers" ("id", "display_name", "created_at", "public_id", "owner", "claimed", "minted_local") VALUES ('01M4ER09NCY7YD841TPSPTRCG8', 'Alex', '2026-09-14T09:00:00.000Z', 'u_5b345bfb8aecd190a917ae', 1, 1, NULL);
INSERT INTO "viewers" ("id", "display_name", "created_at", "public_id", "owner", "claimed", "minted_local") VALUES ('01M4ER09PWFX2V1FG69BR7WDCP', 'Mia', '2026-09-14T09:07:50.000Z', 'u_0443938d6040967e3d00ee', 0, 0, 0);
INSERT INTO "viewers" ("id", "display_name", "created_at", "public_id", "owner", "claimed", "minted_local") VALUES ('01M4ER09PZ8QGB6FSDH7BZ5ACM', NULL, '2026-09-14T09:07:50.000Z', 'u_64421e510b74e6c1d71ce1', 0, 0, 0);
INSERT INTO "docs" ("artifact_id", "path", "collection", "json", "version", "updated_at") VALUES ('zafbtm2ff2jg', 'tasks/t1', 'tasks', '{"done":false,"owner":"Alex","title":"Ship v2"}', 1, '2026-09-14T09:15:00.000Z');
INSERT INTO "docs" ("artifact_id", "path", "collection", "json", "version", "updated_at") VALUES ('zafbtm2ff2jg', 'tasks/t2', 'tasks', '{"done":true,"title":"Write the changelog"}', 2, '2026-09-14T09:15:05.000Z');
INSERT INTO "docs" ("artifact_id", "path", "collection", "json", "version", "updated_at") VALUES ('zafbtm2ff2jg', 'notes/n1', 'notes', '{"text":"Demo on Friday at 10."}', 3, '2026-09-14T09:15:10.000Z');
INSERT INTO "docs" ("artifact_id", "path", "collection", "json", "version", "updated_at") VALUES ('zafbtm2ff2jg', 'tasks/t3', 'tasks', '{"done":false,"title":"Add filters"}', 4, '2026-09-14T09:15:15.000Z');
INSERT INTO "version_threads" ("artifact_id", "version_n", "thread_id", "source", "created_at") VALUES ('ky9gdxtxwxqm', 2, '01M4ER09PEN8CKDJKB6RXG5PSH', 'explicit', '2026-09-14T09:06:30.000Z');
INSERT INTO "send_batches" ("id", "artifact_id", "note", "sent_by", "size", "created_at") VALUES ('01M4ER09R2AY07ZF41MP5Z0GY8', 'zafbtm2ff2jg', 'Before the demo', 'Alex', 2, '2026-09-14T09:15:35.000Z');
INSERT INTO "batch_threads" ("batch_id", "thread_id") VALUES ('01M4ER09R2AY07ZF41MP5Z0GY8', '01M4ER09QYYMTW0D3DPAQHHFG8');
INSERT INTO "batch_threads" ("batch_id", "thread_id") VALUES ('01M4ER09R2AY07ZF41MP5Z0GY8', '01M4ER09R121ZFF148BQ9MTJM7');
INSERT INTO "questions" ("id", "session_id", "artifact_id", "source", "tool_use_id", "questions_json", "status", "answers_json", "answered_via", "created_at", "closed_at", "taken_at") VALUES ('01M4ER09R8FS06A5PCG14N7GFW', '01M4ER09P52ATB4HTRYZ2CM0CF', 'ky9gdxtxwxqm', 'ask', NULL, '[{"question":"Which Chart should the review use?","header":"Chart","options":[{"label":"First","description":"the first one"},{"label":"Second"}],"multi_select":false,"other":true}]', 'answered', '[{"selected":["Second"],"text":null}]', 'shell', '2026-09-14T09:16:15.000Z', '2026-09-14T09:16:30.000Z', NULL);
INSERT INTO "questions" ("id", "session_id", "artifact_id", "source", "tool_use_id", "questions_json", "status", "answers_json", "answered_via", "created_at", "closed_at", "taken_at") VALUES ('01M4ER09RCMZN90JP2SXB7ZAVA', '01M4ER09P52ATB4HTRYZ2CM0CF', 'ky9gdxtxwxqm', 'ask', NULL, '[{"question":"Which Colour should the review use?","header":"Colour","options":[{"label":"First","description":"the first one"},{"label":"Second"}],"multi_select":false,"other":true}]', 'declined', NULL, NULL, '2026-09-14T09:16:45.000Z', '2026-09-14T09:17:00.000Z', NULL);
INSERT INTO "questions" ("id", "session_id", "artifact_id", "source", "tool_use_id", "questions_json", "status", "answers_json", "answered_via", "created_at", "closed_at", "taken_at") VALUES ('01M4ER09RGFZPBW00EBZ3S5ADB', '01M4ER09P52ATB4HTRYZ2CM0CF', 'ky9gdxtxwxqm', 'ask', NULL, '[{"question":"Which Date should the review use?","header":"Date","options":[{"label":"First","description":"the first one"},{"label":"Second"}],"multi_select":false,"other":true}]', 'open', NULL, NULL, '2026-09-14T09:17:15.000Z', NULL, NULL);
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (1, '01M4ER09NJF8EZB1NV6J6G7DY7', 'published', 'published:8nzk3a9npfmy', '8nzk3a9npfmy', NULL, NULL, 1, NULL, '01M4ER09NFCF39DKTE1PBC3Y2A', 'codex', NULL, '2026-09-14T09:01:00.000Z', '2026-09-14T09:17:55.000Z');
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (2, '01M4ER09NWWE7FZN0MH8APN276', 'reply', 'reply:01M4ER09NWWE7FZN0MH8APN275', '8nzk3a9npfmy', '01M4ER09NMMK2SRX0YZGWMACN4', '01M4ER09NWWE7FZN0MH8APN275', NULL, NULL, '01M4ER09NFCF39DKTE1PBC3Y2A', 'codex', NULL, '2026-09-14T09:02:30.000Z', NULL);
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (3, '01M4ER09P7MZ1BDXPTEWF3SMAS', 'published', 'published:ky9gdxtxwxqm', 'ky9gdxtxwxqm', NULL, NULL, 1, NULL, '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:03:40.000Z', NULL);
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (4, '01M4ER09PAJ5VYDKAWXFZNG5ZV', 'published', 'published:zafbtm2ff2jg', 'zafbtm2ff2jg', NULL, NULL, 1, NULL, '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:04:10.000Z', NULL);
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (5, '01M4ER09PMKXXVACB2PXJXYZ0A', 'reply', 'reply:01M4ER09PMKXXVACB2PXJXYZ09', 'ky9gdxtxwxqm', '01M4ER09PEN8CKDJKB6RXG5PSH', '01M4ER09PMKXXVACB2PXJXYZ09', NULL, NULL, '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:06:10.000Z', NULL);
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (6, '01M4ER09PRJGR5CCY8H38D99YT', 'version', 'version:ky9gdxtxwxqm:2', 'ky9gdxtxwxqm', NULL, NULL, 2, NULL, '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:06:30.000Z', NULL);
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (7, '01M4ER09R8FS06A5PCG14N7GFX', 'question', 'question:01M4ER09R8FS06A5PCG14N7GFW', 'ky9gdxtxwxqm', NULL, NULL, NULL, '01M4ER09R8FS06A5PCG14N7GFW', '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:16:15.000Z', '2026-09-14T09:16:30.000Z');
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (8, '01M4ER09RCMZN90JP2SXB7ZAVB', 'question', 'question:01M4ER09RCMZN90JP2SXB7ZAVA', 'ky9gdxtxwxqm', NULL, NULL, NULL, '01M4ER09RCMZN90JP2SXB7ZAVA', '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:16:45.000Z', '2026-09-14T09:17:00.000Z');
INSERT INTO "inbox_items" ("seq", "id", "kind", "key", "artifact_id", "thread_id", "comment_id", "version_n", "question_id", "session_id", "harness", "detail_json", "created_at", "read_at") VALUES (9, '01M4ER09RGFZPBW00EBZ3S5ADC', 'question', 'question:01M4ER09RGFZPBW00EBZ3S5ADB', 'ky9gdxtxwxqm', NULL, NULL, NULL, '01M4ER09RGFZPBW00EBZ3S5ADB', '01M4ER09P52ATB4HTRYZ2CM0CF', 'claude', NULL, '2026-09-14T09:17:15.000Z', NULL);
DELETE FROM "inbox_fts_data";
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (1, X'0B6A');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (10, X'00000000FF000001010B0D000B0101010101000001020101020200000103010103030000010401010404000001050101050500000106010106060000010701010707010101080101080800000109010109090101010A01010A0A0000010B01010B0B000001');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (137438953473, X'000000C10630636F64657801020401066C61756E63680102020104706C616E0102030104736869700102070204746570730102050102746F010206010276320102080002316301020401016C01020201017001020301017301040504010174010206010176010208000332636F01020401026C610102020102706C010203010273680102070201740102050102746F01020601027632010208000433636F6401020401036C61750102020103706C61010203010373686901020702027465010205040A0B09090907070706060706060807070706070709080808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (274877906945, X'000000F60230610202060204646465640202050204667465720202090105636F64657802020401066C61756E63680202020104706C616E0202030108726F6C6C6261636B02020701047368697002020A020374657002020800023161020605030501016302020401016C0202020101700202030101720202070101730204080400033261640202050201660202090102636F02020401026C610202020102706C0202030102726F0202070102736802020A020174020208000433616464020205020266740202090103636F6402020401036C61750202020103706C610202030103726F6C020207010373686902020A02027465020208040609090A0B090D0908090606060607080607070707070609070808080808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (412316860417, X'000000F10630626F6172640302090106636C617564650302040103666F7203020701076E756D626572730302060102713303020502087561727465726C79030202010672657669657703020301037468650302080002316203020901016303020401016603020701016E03020601017103040205010172030203010174030208000332626F0302090102636C0302040102666F03020701026E75030206010271330302050201750302020102726503020301027468030208000433626F610302090103636C610302040103666F7203020701036E756D030206010371756103020201037265760302030103746865030208040A0B080C070D0B08070606060706060807070707060707090808080808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (549755813889, X'0000005E0730636C6175646504020401047465616D04020202067261636B65720402030002316304020401017404040203000332636C04020401027465040202020172040203000433636C61040204010374656104020202027261040203040B090B07070807060908');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (687194767361, X'000001220730636C617564650502040104646F6E65050205010668656164657205020701036E6F770502080109717561727465726C7905020201067265766965770502030106737469636B7305020901037468650504060702016F05020A03017005020C0002316305020401016405020501016805020701016E050208010171050202010172050203010173050209010174050806060303000332636C0502040102646F0502050102686505020701026E6F050208010271750502020102726505020301027374050209010274680504060702016F05040A04000433636C610502040103646F6E050205010368656105020701036E6F7705020801037175610502020103726576050203010373746905020901037468650504060702026F7005020C040B090B080E0B0B09060607060606060606090807070707070708070908080808080809');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (824633720833, X'000000CD0730636C617564650602040105676F616C7306020801066865616465720602060109717561727465726C790604020701067265766965770602030106737469636B790602050002316306020401016706020801016806020601017106040207010172060203010173060205000332636C0602040102676F0602080102686506020601027175060402070102726506020301027374060205000433636C610602040103676F61060208010368656106020601037175610604020701037265760602030103737469060205040B0A0B0F0B0B0706060706060807070807070908080908');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (962072674305, X'00000139063063686172740704060702056C617564650702040105666972737407020C0109717561727465726C7907020201067265766965770704030801067365636F6E6407020D0205686F756C640702070103746865070208010375736507020A0105776869636807020500023163070604040701016607020C010171070202010172070403080101730704070801017407020801017507020A01017707020500033263680704060702016C0702040102666907020C0102717507020201027265070403080102736507020D020168070207010274680702080102757307020A010277680702050004336368610704060702026C61070204010366697207020C0103717561070202010372657607040308010373656307020D0202686F0702070103746865070208010375736507020A0103776869070205040B0A0A0E0C0B0A08080A0906060707060606090607070807060707070A0708080908070808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (1099511627777, X'0000013D063063686172740704060702056C617564650702040105666972737407020C0109717561727465726C7907020201067265766965770704030801067365636F6E6407040D030205686F756C640702070103746865070208010375736507020A0105776869636807020500023163070604040701016607020C01017107020201017207040308010173070607080301017407020801017507020A01017707020500033263680704060702016C0702040102666907020C0102717507020201027265070403080102736507040D03020168070207010274680702080102757307020A010277680702050004336368610704060702026C61070204010366697207020C0103717561070202010372657607040308010373656307040D030202686F0702070103746865070208010375736507020A0103776869070205040B0A0A0E0C0C0A08080A0906060708060606090607070808060707070A0708080909070808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (1236950581249, X'0000013A0730636C6175646508020402056F6C6F7572080406070105666972737408020C0109717561727465726C7908020201067265766965770804030801067365636F6E6408020D0205686F756C640802070103746865080208010375736508020A0105776869636808020500023163080604040701016608020C010171080202010172080403080101730804070801017408020801017508020A010177080205000332636C08020402016F080406070102666908020C0102717508020201027265080403080102736508020D020168080207010274680802080102757308020A01027768080205000433636C6108020402026F6C08040607010366697208020C0103717561080202010372657608040308010373656308020D0202686F0802070103746865080208010375736508020A0103776869080205040B0B0A0E0C0B0A08080A090606070706060608070707080706070707090808080908070808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (1374389534721, X'0000013A0730636C6175646508020402056F6C6F7572080406070105666972737408020C0109717561727465726C7908020201067265766965770804030801067365636F6E6408020D0205686F756C640802070103746865080208010375736508020A0105776869636808020500023163080604040701016608020C010171080202010172080403080101730804070801017408020801017508020A010177080205000332636C08020402016F080406070102666908020C0102717508020201027265080403080102736508020D020168080207010274680802080102757308020A01027768080205000433636C6108020402026F6C08040607010366697208020C0103717561080202010372657608040308010373656308020D0202686F0802070103746865080208010375736508020A0103776869080205040B0B0A0E0C0B0A08080A090606070706060608070707080706070707090808080908070808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (1511828488193, X'000001400730636C61756465090204010464617465090406070105666972737409020C0109717561727465726C7909020201067265766965770904030801067365636F6E6409020D0205686F756C640902070103746865090208010375736509020A01057768696368090205000231630902040101640904060701016609020C010171090202010172090403080101730904070801017409020801017509020A010177090205000332636C09020401026461090406070102666909020C0102717509020201027265090403080102736509020D020168090207010274680902080102757309020A01027768090205000433636C61090204010364617409040607010366697209020C0103717561090202010372657609040308010373656309020D0202686F0902070103746865090208010375736509020A0103776869090205040B0A0A0E0C0B0A08080A07070606070706060608080707080706070707090908080908070808');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (9008161327415296, X'04000000000000010000000000000000000000000000000000000000000000000000000000000007000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000');
INSERT INTO "inbox_fts_data" ("id", "block") VALUES (9008436205322240, X'04000000000000010000000000000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000');
DELETE FROM "inbox_fts_idx";
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (1, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (2, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (3, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (4, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (5, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (6, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (7, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (8, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (9, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (10, X'', 2);
INSERT INTO "inbox_fts_idx" ("segid", "term", "pgno") VALUES (11, X'', 2);
DELETE FROM "inbox_fts_docsize";
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (1, X'07', 1);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (2, X'09', 2);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (3, X'08', 3);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (4, X'03', 4);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (5, X'0B', 5);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (6, X'07', 6);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (7, X'0D', 8);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (8, X'0C', 10);
INSERT INTO "inbox_fts_docsize" ("id", "sz", "origin") VALUES (9, X'0C', 11);
DELETE FROM "inbox_fts_config";
INSERT INTO "inbox_fts_config" ("k", "v") VALUES ('version', 4);
PRAGMA user_version = 21;
CREATE TABLE fixture_files (path TEXT PRIMARY KEY, content BLOB NOT NULL);
INSERT INTO fixture_files (path, content) VALUES ('artifacts/8nzk3a9npfmy/versions/1/index.html', X'3C21646F63747970652068746D6C3E3C68746D6C3E3C686561643E3C7469746C653E4C61756E636820506C616E3C2F7469746C653E3C2F686561643E3C626F64793E3C6D61696E3E3C68323E4C61756E63682073746570733C2F68323E3C6F6C3E3C6C693E467265657A653C2F6C693E3C6C693E536869703C2F6C693E3C2F6F6C3E3C2F6D61696E3E3C2F626F64793E3C2F68746D6C3E');
INSERT INTO fixture_files (path, content) VALUES ('artifacts/ky9gdxtxwxqm/clips/01M4ER09PEN8CKDJKB6RXG5PSH.png', X'89504E470D0A1A0A636C69702D6F662D7468652D686561646572');
INSERT INTO fixture_files (path, content) VALUES ('artifacts/ky9gdxtxwxqm/versions/1/files/style.css', X'6D61696E207B206D61782D77696474683A20363072656D3B207D');
INSERT INTO fixture_files (path, content) VALUES ('artifacts/ky9gdxtxwxqm/versions/1/index.html', X'3C21646F63747970652068746D6C3E3C68746D6C3E3C686561643E3C7469746C653E517561727465726C79205265766965773C2F7469746C653E3C6C696E6B2072656C3D7374796C65736865657420687265663D7374796C652E6373733E3C2F686561643E3C626F64793E3C6D61696E3E3C68323E517561727465726C7920676F616C733C2F68323E3C7020636C6173733D6C6564653E526576656E7565206772657720313225206F6E2051322E3C2F703E3C7461626C653E3C74723E3C74643E51333C2F74643E3C74643E342E314D3C2F74643E3C2F74723E3C2F7461626C653E3C2F6D61696E3E3C2F626F64793E3C2F68746D6C3E');
INSERT INTO fixture_files (path, content) VALUES ('artifacts/ky9gdxtxwxqm/versions/2/files/style.css', X'6D61696E207B206D61782D77696474683A20363072656D3B207D202E737469636B79207B20706F736974696F6E3A20737469636B793B20746F703A20303B207D');
INSERT INTO fixture_files (path, content) VALUES ('artifacts/ky9gdxtxwxqm/versions/2/index.html', X'3C21646F63747970652068746D6C3E3C68746D6C3E3C686561643E3C7469746C653E517561727465726C79205265766965773C2F7469746C653E3C6C696E6B2072656C3D7374796C65736865657420687265663D7374796C652E6373733E3C2F686561643E3C626F64793E3C68656164657220636C6173733D737469636B793E517561727465726C79205265766965773C2F6865616465723E3C6D61696E3E3C68323E517561727465726C7920676F616C733C2F68323E3C7020636C6173733D6C6564653E526576656E7565206772657720313225206F6E2051322E3C2F703E3C7461626C653E3C74723E3C74643E51333C2F74643E3C74643E342E314D3C2F74643E3C2F74723E3C2F7461626C653E3C2F6D61696E3E3C2F626F64793E3C2F68746D6C3E');
INSERT INTO fixture_files (path, content) VALUES ('artifacts/zafbtm2ff2jg/versions/1/index.html', X'3C21646F63747970652068746D6C3E3C68746D6C3E3C686561643E3C7469746C653E5465616D20547261636B65723C2F7469746C653E3C2F686561643E3C626F64793E3C6D61696E3E3C68323E5461736B733C2F68323E3C756C2069643D7461736B733E3C2F756C3E3C2F6D61696E3E3C2F626F64793E3C2F68746D6C3E');
COMMIT;

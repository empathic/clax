-- Migrations 24 to 29 as commit e1808eb563d2 (2026-10-14, branch rust-int)
-- wrote them in crates/clax-core/src/store/migrations.rs (blob 6ad52300ff7c). Migrations 1 to 23 there
-- are the current ones, character for character.
-- Differs from the current schema: sessions.elevate_json in place of session_elevations; no viewer_cookies; no approval_requests_by_target or _by_thread; no feedback.no_access_at.
-- first: 24
-- user_version: 29

-- migration: 24
ALTER TABLE viewers ADD COLUMN person INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE viewers ADD COLUMN person_revoked_at TEXT;
    ALTER TABLE viewers ADD COLUMN rights_json TEXT;

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
    ALTER TABLE sessions ADD COLUMN elevate_json TEXT;
    ALTER TABLE sessions ADD COLUMN ended_reason TEXT
        CHECK (ended_reason IN ('idle', 'exit', 'revoked', 'person'));
    ALTER TABLE sessions ADD COLUMN ended_watches_json TEXT;
    ALTER TABLE sessions ADD COLUMN secret_sha256 TEXT;
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
    ALTER TABLE assets ADD COLUMN person TEXT;

-- migration: 25
CREATE TABLE approvals (
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
    ALTER TABLE artifacts ADD COLUMN text_authors_json TEXT;
    ALTER TABLE send_batches ADD COLUMN sender TEXT;
    ALTER TABLE threads ADD COLUMN resolved_by_person TEXT;

-- migration: 26
CREATE TABLE inbox_items_26 (
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
        WHERE status <> 'open';

-- migration: 27
CREATE TABLE hosted_setup (
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
    ALTER TABLE artifacts ADD COLUMN link_access TEXT CHECK (link_access IN ('viewer', 'commenter'));

-- migration: 28
ALTER TABLE artifacts ADD COLUMN forked_from_id TEXT;
    ALTER TABLE artifacts ADD COLUMN forked_from_version INTEGER;

-- migration: 29
ALTER TABLE sessions ADD COLUMN client_name TEXT;

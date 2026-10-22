-- Migrations 41 to 46 as commit 01fcc2dd299a (2026-10-20, branch flutter-app)
-- wrote them in crates/clax-store/src/migrations.rs (blob dddde1375aee). Migrations 1 to 40 there
-- are the current ones, character for character.
-- The Clax app's five after main's 39 and 40, and its jobs migration: a home this branch's
-- builds up to 01fcc2dd2 migrated, before main's 41 (tags) moved the app's to 42 to 47.
-- first: 41
-- user_version: 46

-- migration: 41
CREATE TABLE db_read_grants (
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
    END;

-- migration: 42
ALTER TABLE artifacts ADD COLUMN draft_of TEXT;
    ALTER TABLE artifacts ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX artifacts_hidden ON artifacts(hidden, created_at) WHERE hidden = 1;

-- migration: 43
CREATE TABLE net_grants (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        host TEXT NOT NULL,
        granted_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, host)
    ) WITHOUT ROWID;
    CREATE TRIGGER net_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM net_grants WHERE artifact_id = NEW.id;
    END;

-- migration: 44
CREATE TABLE sample_grants (
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
    END;

-- migration: 45
ALTER TABLE artifacts ADD COLUMN draft_inherit_net INTEGER NOT NULL DEFAULT 1;

-- migration: 46
CREATE TABLE jobs (
        id TEXT PRIMARY KEY,
        client TEXT NOT NULL,              -- 'app' (who runs it)
        kind TEXT NOT NULL,                -- 'make' | 'change' (opaque to the core)
        artifact_id TEXT REFERENCES artifacts(id) ON DELETE CASCADE,
        state TEXT NOT NULL CHECK (state IN ('queued','running','waiting','paused','stopped','failed','cancelled','done')),
        step TEXT,                         -- the current step's key
        title TEXT,
        input_json TEXT NOT NULL CHECK (json_valid(input_json)),  -- the request, the threads and comments answered
        plan_json TEXT CHECK (json_valid(plan_json)),  -- a make's plan (was plans.json)
        carry_json TEXT CHECK (json_valid(carry_json)),  -- Keep going's carried draft and notes (was unfinished.json)
        error_json TEXT CHECK (json_valid(error_json)),
        rev INTEGER NOT NULL DEFAULT 1 CHECK (rev >= 1),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX jobs_open ON jobs(state) WHERE state NOT IN ('cancelled','done');
    CREATE INDEX jobs_by_artifact ON jobs(artifact_id) WHERE artifact_id IS NOT NULL;
    CREATE TABLE job_steps (
        job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
        key TEXT NOT NULL,
        result_json TEXT NOT NULL CHECK (json_valid(result_json)),
        version_n INTEGER,                 -- the version a publish step made
        created_at TEXT NOT NULL,
        PRIMARY KEY (job_id, key)
    );
    CREATE TABLE job_threads (            -- was the ledger's answered and stopped.json
        job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        comment_id TEXT NOT NULL,          -- the newest comment the job answers
        stopped INTEGER NOT NULL DEFAULT 0 CHECK (stopped IN (0, 1)),
        PRIMARY KEY (job_id, thread_id)
    );
    CREATE INDEX job_threads_by_thread ON job_threads(thread_id);
    CREATE TABLE job_questions (          -- was asked.json
        job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
        question_id TEXT NOT NULL UNIQUE REFERENCES questions(id),
        outcome TEXT CHECK (outcome IN ('answered','declined','withdrawn')),  -- NULL until the question closes
        answers_json TEXT CHECK (json_valid(answers_json)),  -- the answers, as the question keeps them
        PRIMARY KEY (job_id, question_id),
        CHECK ((outcome IS 'answered') = (answers_json IS NOT NULL))
    );
    CREATE TABLE agent_read_consents (    -- was agent-read-consent.json
        reader TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        source TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        created_at TEXT NOT NULL,
        PRIMARY KEY (reader, source)
    );
    CREATE INDEX agent_read_consents_by_source ON agent_read_consents(source);
    CREATE TABLE draft_renders (          -- the render gate (§8.2)
        draft_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        digest TEXT NOT NULL,
        clean INTEGER NOT NULL CHECK (clean IN (0, 1)),
        created_at TEXT NOT NULL,
        PRIMARY KEY (draft_id, digest)
    );
    CREATE TABLE client_imports (         -- one row per one-time import (§9)
        key TEXT PRIMARY KEY,
        done_at TEXT NOT NULL,
        summary_json TEXT NOT NULL CHECK (json_valid(summary_json))
    );
    CREATE TABLE job_step_marks (         -- each publish step's mark
        nonce TEXT PRIMARY KEY,            -- 32 hex characters from the operating system's random source
        job_id TEXT NOT NULL,
        key TEXT NOT NULL,                 -- the step's key
        artifact_id TEXT NOT NULL,         -- the artifact it writes
        version_n INTEGER NOT NULL CHECK (version_n >= 1),  -- the version it writes
        session_id TEXT,                   -- the session it publishes from
        created_at TEXT NOT NULL,
        UNIQUE (job_id, key)
    );

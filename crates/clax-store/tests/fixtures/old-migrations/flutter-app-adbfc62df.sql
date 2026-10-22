-- Migration 46 as branch flutter-app writes it in crates/clax-store/src/migrations.rs, first
-- frozen as commit adbfc62dfbe4 (2026-10-20, blob 4f6501970e16) wrote it and since edited in place
-- with it while it is unmerged, so the two stay equal: the Clax app's jobs migration, the sixth
-- and last of its migrations, after its five at 41 to 45 (flutter-app-4479a586c.sql). Migrations 1 to 45
-- there are the current ones, character for character. Frozen with those five so that the layouts
-- holding the app's migrations after the first k of main's migrations above 38 can still be built
-- once main adds more.
-- first: 46
-- user_version: 46

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

-- Migration 46 as branch flutter-app writes it in crates/clax-store/src/migrations.rs, first
-- frozen as commit adbfc62dfbe4 (2026-10-20, blob 4f6501970e16) wrote it and since edited in place
-- with it while it is unmerged, so the two stay equal: the Clax app's jobs migration, the sixth
-- of its migrations, after its five at 41 to 45 (flutter-app-4479a586c.sql). Migrations 1 to 45
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
        artifact_id TEXT REFERENCES artifacts(id),
        state TEXT NOT NULL CHECK (state IN ('queued','running','waiting','paused','stopped','failed','cancelled','done')),
        step TEXT,                         -- the current step's key
        title TEXT,
        input_json TEXT NOT NULL,          -- the request, the threads and comments answered
        plan_json TEXT,                    -- a make's plan (was plans.json)
        carry_json TEXT,                   -- Keep going's carried draft and notes (was unfinished.json)
        error_json TEXT,
        rev INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX jobs_open ON jobs(state) WHERE state NOT IN ('cancelled','done');
    CREATE TABLE job_steps (
        job_id TEXT NOT NULL REFERENCES jobs(id),
        key TEXT NOT NULL,
        result_json TEXT NOT NULL,
        version_n INTEGER,                 -- the version a publish step made
        created_at TEXT NOT NULL,
        PRIMARY KEY (job_id, key)
    );
    CREATE TABLE job_threads (            -- was the ledger's answered and stopped.json
        job_id TEXT NOT NULL REFERENCES jobs(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        comment_id TEXT NOT NULL,          -- the newest comment the job answers
        stopped INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (job_id, thread_id)
    );
    CREATE TABLE job_questions (          -- was asked.json
        job_id TEXT NOT NULL REFERENCES jobs(id),
        question_id TEXT NOT NULL UNIQUE REFERENCES questions(id),
        outcome TEXT CHECK (outcome IN ('answered','declined','withdrawn')),  -- NULL until the question closes
        answers_json TEXT,                 -- the answers, as the question keeps them
        PRIMARY KEY (job_id, question_id),
        CHECK ((outcome IS 'answered') = (answers_json IS NOT NULL))
    );
    CREATE TABLE agent_read_consents (    -- was agent-read-consent.json
        reader TEXT NOT NULL,
        source TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (reader, source)
    );
    CREATE TABLE draft_renders (          -- the render gate (§8.2)
        draft_id TEXT NOT NULL,
        digest TEXT NOT NULL,
        clean INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (draft_id, digest)
    );
    CREATE TABLE client_imports (         -- one row per one-time import (§9)
        key TEXT PRIMARY KEY,
        done_at TEXT NOT NULL,
        summary_json TEXT NOT NULL
    );

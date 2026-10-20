-- Migration 47 as branch flutter-app writes it in crates/clax-store/src/migrations.rs, frozen
-- and edited in place with it while it is unmerged, so the two stay equal: the Clax app's step
-- marks migration, the seventh of its migrations, after its jobs migration at 46
-- (flutter-app-adbfc62df.sql) and its five at 41 to 45 (flutter-app-4479a586c.sql). Frozen with
-- them so that the layouts holding the app's migrations after the first k of main's migrations
-- above 38 can still be built once main adds more.
-- first: 47
-- user_version: 47

-- migration: 47
CREATE TABLE job_step_marks (
        nonce TEXT PRIMARY KEY,            -- 32 hex characters from the operating system's random source
        job_id TEXT NOT NULL,
        key TEXT NOT NULL,                 -- the step's key
        artifact_id TEXT NOT NULL,         -- the artifact it writes
        version_n INTEGER NOT NULL CHECK (version_n >= 1),  -- the version it writes
        session_id TEXT,                   -- the session it publishes from
        created_at TEXT NOT NULL,
        UNIQUE (job_id, key)
    );

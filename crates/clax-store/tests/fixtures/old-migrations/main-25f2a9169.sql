-- Migrations 39 to 41 as commit 25f2a916903a (2026-10-21, branch main)
-- wrote them in crates/clax-store/src/migrations.rs (blob cc3ec71125ee). Migrations 1 to 38 there
-- are the current ones, character for character.
-- Main's tip: legal_agreements (39), briefs (40) and tags (41), and none of the app's.
-- fa19f4ab8, which added tags, has the same migrations.rs.
-- first: 39
-- user_version: 41

-- migration: 39
CREATE TABLE legal_agreements (
        viewer_id TEXT NOT NULL,
        version TEXT NOT NULL,
        agreed_at TEXT NOT NULL,
        via TEXT NOT NULL,
        PRIMARY KEY (viewer_id, version)
    );

-- migration: 40
CREATE TABLE briefs (
        artifact_id TEXT PRIMARY KEY,
        starter TEXT NOT NULL,
        key TEXT,
        created_at TEXT NOT NULL
    );
    CREATE UNIQUE INDEX briefs_by_key ON briefs (starter, key) WHERE key IS NOT NULL;

-- migration: 41
CREATE TABLE tags (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        fold TEXT NOT NULL UNIQUE,
        colour TEXT NOT NULL,
        description TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE TABLE artifact_tags (
        artifact_id TEXT NOT NULL,
        tag_id TEXT NOT NULL,
        PRIMARY KEY (artifact_id, tag_id)
    );
    CREATE INDEX artifact_tags_by_tag ON artifact_tags (tag_id, artifact_id);

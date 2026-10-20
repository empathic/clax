-- Migrations 41 to 45 as commit 4479a586c809 (2026-10-19, branch flutter-app)
-- wrote them in crates/clax-store/src/migrations.rs (blob 8436e775617c), the Clax app's five
-- after main's 39 and 40. Migrations 1 to 40 there are the current ones, character for character.
-- The app's migrations as this numbering has them, frozen so that the layouts holding them after
-- the first k of main's migrations above 38 can still be built once main adds more.
-- first: 41
-- user_version: 45

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

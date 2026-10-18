-- Migrations 36 to 36 as commit 95d2a88dbbc9 (2026-10-16, branch akesling/clax-artifacts-pre-main-rebase-2026-10-17)
-- wrote them in crates/clax-core/src/store/migrations.rs (blob acb9dd397e77). Migrations 1 to 35 there
-- are the current ones, character for character.
-- Differs from the current schema: migration 36 without watches.relay_name and relay_harness.
-- first: 36
-- user_version: 36

-- migration: 36
UPDATE viewers SET rights_json = '[{"actions":["create"],"locations":["*"],"type":"clax-artifacts"},{"actions":["delete","read","update"],"locations":["own"],"type":"clax-artifacts"},{"actions":["read","reply","resolve","send","watch","write"],"locations":["own"],"type":"clax-comments"},{"actions":["read","write"],"level":"admin","locations":["own"],"type":"clax-db"},{"actions":["ask"],"type":"clax-ask"}]'
        WHERE rights_json = '[{"actions":["create"],"locations":["*"],"type":"clax-artifacts"},{"actions":["delete","read","update"],"locations":["own"],"type":"clax-artifacts"},{"actions":["read","reply","resolve","send","watch","write"],"locations":["own"],"type":"clax-comments"},{"actions":["read","write"],"level":"admin","locations":["own"],"type":"clax-db"}]';

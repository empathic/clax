//! The stand-in's tables, in one SQLite database (`RepoDO`'s, through
//! `clax-do-vfs` in a Worker; an in-memory connection in tests):
//!
//! - `git_repos(name)`: each repository;
//! - `git_objects(id, kind, data)`: every object, content-addressed and
//!   stored once, whole;
//! - `git_held(repo, id)`: the objects each repository holds. A repository
//!   answers only these, so one cannot read another's objects by ID, and a
//!   fork copies these rows, not the objects;
//! - `git_refs(repo, name, id)`: each repository's refs;
//! - `git_tokens(id, repo, scope, expires_ms, revoked_ms, hash, person,
//!   session)`: every repository token minted, with the SHA-256 of a
//!   stand-in token (`hash`; none for a Cloudflare Artifacts token, which
//!   Artifacts checks) and whom it was handed to (`person`, a connected
//!   person's public ID, none for the owner; `session`, the agent session,
//!   for an agent's; `link` 1 for a read token handed to whoever holds a
//!   page's link);
//! - `git_tokens_due(id, repo, expires_ms)`: the must-revoke list, tokens
//!   whose revocation was asked and has not yet succeeded;
//! - `git_pushes(repo, commit, person, session, at_ms)`: who made each push
//!   that left `main` at `commit` (as the push's token's holder), the
//!   newest [`MAX_PUSHES_KEPT`] per repository.
//!
//! Every function takes the connection and does its work synchronously; a
//! change of more than one row is one transaction.

use std::collections::BTreeMap;

use clax_git::repo::ATOMIC_FAILED;
use clax_git::{GitError, Object, ObjectId, ObjectKind, RefStatus, RefUpdate};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS git_repos (name TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS git_objects (
    id BLOB NOT NULL UNIQUE, kind INTEGER NOT NULL, data BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS git_held (
    repo TEXT NOT NULL, id BLOB NOT NULL, PRIMARY KEY (repo, id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS git_refs (
    repo TEXT NOT NULL, name TEXT NOT NULL, id BLOB NOT NULL,
    PRIMARY KEY (repo, name)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS git_tokens (
    id TEXT PRIMARY KEY, repo TEXT NOT NULL, scope TEXT NOT NULL,
    expires_ms INTEGER NOT NULL, revoked_ms INTEGER, hash BLOB UNIQUE);
CREATE TABLE IF NOT EXISTS git_tokens_due (
    id TEXT PRIMARY KEY, repo TEXT NOT NULL, expires_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS git_pushes (
    repo TEXT NOT NULL, commit_id TEXT NOT NULL, person TEXT, session TEXT,
    at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS git_pushes_by_commit ON git_pushes (repo, commit_id);
";

/// The most pushes [`record_push`] keeps per repository: far more than the
/// 64 commits one reconcile records one by one, so a push is named until
/// well after the instance has recorded it.
pub const MAX_PUSHES_KEPT: i64 = 256;

pub type Result<T> = clax_git::Result<T>;

/// The most object bytes one [`apply`] writes: under `clax-do-vfs`'s
/// 16 MiB transaction cap (`MAX_TRANSACTION_BYTES`), with room for the
/// rows, indexes and pages around them, so a write the cap would refuse
/// part-way is refused whole, with a reason, before it starts.
pub const MAX_WRITE_BYTES: usize = 12 << 20;

fn sql(e: rusqlite::Error) -> GitError {
    GitError::RepoUnavailable(format!("The repository store failed: {e}"))
}

/// Creates the tables when they are absent, and the columns a ledger made
/// before them lacks.
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA).map_err(sql)?;
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('git_tokens')")
        .and_then(|mut s| {
            s.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .map_err(sql)?;
    for (column, kind) in [
        ("person", "TEXT"),
        ("session", "TEXT"),
        ("link", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !columns.iter().any(|c| c == column) {
            conn.execute_batch(&format!(
                "ALTER TABLE git_tokens ADD COLUMN {column} {kind}"
            ))
            .map_err(sql)?;
        }
    }
    Ok(())
}

pub fn exists(conn: &Connection, repo: &str) -> Result<bool> {
    conn.query_row(
        "SELECT 1 FROM git_repos WHERE name = ?1",
        [repo],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
    .map_err(sql)
}

fn require(conn: &Connection, repo: &str) -> Result<()> {
    if exists(conn, repo)? {
        Ok(())
    } else {
        Err(GitError::NotFound(format!("No repository {repo}")))
    }
}

/// Creates the empty repository `repo`; `invalid_argument` when it exists.
pub fn create(conn: &Connection, repo: &str) -> Result<()> {
    if exists(conn, repo)? {
        return Err(GitError::InvalidArgument(format!(
            "Repository {repo} already exists"
        )));
    }
    conn.execute("INSERT INTO git_repos (name) VALUES (?1)", [repo])
        .map(drop)
        .map_err(sql)
}

/// Creates `to` holding what `from` holds, with its refs.
pub fn fork(conn: &Connection, from: &str, to: &str) -> Result<()> {
    require(conn, from)?;
    if exists(conn, to)? {
        return Err(GitError::InvalidArgument(format!(
            "Repository {to} already exists"
        )));
    }
    let tx = conn.unchecked_transaction().map_err(sql)?;
    tx.execute("INSERT INTO git_repos (name) VALUES (?1)", [to])
        .map_err(sql)?;
    tx.execute(
        "INSERT INTO git_held (repo, id) SELECT ?1, id FROM git_held WHERE repo = ?2",
        [to, from],
    )
    .map_err(sql)?;
    tx.execute(
        "INSERT INTO git_refs (repo, name, id) SELECT ?1, name, id FROM git_refs WHERE repo = ?2",
        [to, from],
    )
    .map_err(sql)?;
    tx.commit().map_err(sql)
}

/// Deletes `repo` (nothing when it is missing), revokes its tokens so a
/// repository later created under the name does not take them, and drops
/// the objects no repository holds any more.
pub fn delete(conn: &Connection, repo: &str, now_ms: i64) -> Result<()> {
    let tx = conn.unchecked_transaction().map_err(sql)?;
    tx.execute("DELETE FROM git_refs WHERE repo = ?1", [repo])
        .map_err(sql)?;
    tx.execute("DELETE FROM git_held WHERE repo = ?1", [repo])
        .map_err(sql)?;
    tx.execute("DELETE FROM git_repos WHERE name = ?1", [repo])
        .map_err(sql)?;
    tx.execute("DELETE FROM git_pushes WHERE repo = ?1", [repo])
        .map_err(sql)?;
    tx.execute(
        "UPDATE git_tokens SET revoked_ms = ?2 WHERE repo = ?1 AND revoked_ms IS NULL",
        params![repo, now_ms],
    )
    .map_err(sql)?;
    tx.execute(
        "DELETE FROM git_objects WHERE id NOT IN (SELECT id FROM git_held)",
        [],
    )
    .map_err(sql)?;
    tx.commit().map_err(sql)
}

/// Drops from `repo` every object none of its refs reaches
/// ([`clax_git::reachable_from`]): what a fork copied of its source beyond
/// the version it was made from. The objects stay stored for the
/// repositories that still hold them.
pub fn prune(conn: &Connection, repo: &str) -> Result<()> {
    let tx = conn.unchecked_transaction().map_err(sql)?;
    let tips = refs(&tx, repo)?;
    let keep = clax_git::reachable_from(tips.into_values(), |id| read_object(&tx, repo, id))?;
    let held: Vec<ObjectId> = {
        let mut st = tx
            .prepare("SELECT id FROM git_held WHERE repo = ?1")
            .map_err(sql)?;
        let rows = st
            .query_map([repo], |r| object_id(r.get(0)?))
            .map_err(sql)?;
        rows.collect::<rusqlite::Result<_>>().map_err(sql)?
    };
    {
        let mut drop = tx
            .prepare("DELETE FROM git_held WHERE repo = ?1 AND id = ?2")
            .map_err(sql)?;
        for id in held.iter().filter(|id| !keep.contains(id)) {
            drop.execute(params![repo, id.as_bytes().as_slice()])
                .map_err(sql)?;
        }
    }
    tx.commit().map_err(sql)
}

fn object_id(bytes: Vec<u8>) -> rusqlite::Result<ObjectId> {
    ObjectId::from_bytes(&bytes).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            "not a 20-byte object ID".into(),
        )
    })
}

/// Every ref of `repo`.
pub fn refs(conn: &Connection, repo: &str) -> Result<BTreeMap<String, ObjectId>> {
    require(conn, repo)?;
    let mut st = conn
        .prepare_cached("SELECT name, id FROM git_refs WHERE repo = ?1")
        .map_err(sql)?;
    let rows = st
        .query_map([repo], |r| {
            Ok((r.get::<_, String>(0)?, object_id(r.get(1)?)?))
        })
        .map_err(sql)?;
    rows.collect::<rusqlite::Result<_>>().map_err(sql)
}

/// The object `id` when `repo` holds it.
pub fn read_object(conn: &Connection, repo: &str, id: &ObjectId) -> Result<Option<Object>> {
    require(conn, repo)?;
    let row = conn
        .query_row(
            "SELECT o.kind, o.data FROM git_held h JOIN git_objects o ON o.id = h.id
             WHERE h.repo = ?1 AND h.id = ?2",
            params![repo, id.as_bytes().as_slice()],
            |r| Ok((r.get::<_, u8>(0)?, r.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(sql)?;
    match row {
        None => Ok(None),
        Some((code, data)) => {
            let kind = ObjectKind::from_pack_code(code).ok_or_else(|| {
                GitError::RepoUnavailable(format!("Object {id} has an unknown type"))
            })?;
            Ok(Some(Object::new(kind, data)))
        }
    }
}

/// The type of object `id` when `repo` holds it.
pub fn object_kind(conn: &Connection, repo: &str, id: &ObjectId) -> Result<Option<ObjectKind>> {
    require(conn, repo)?;
    let code = conn
        .query_row(
            "SELECT o.kind FROM git_held h JOIN git_objects o ON o.id = h.id
             WHERE h.repo = ?1 AND h.id = ?2",
            params![repo, id.as_bytes().as_slice()],
            |r| r.get::<_, u8>(0),
        )
        .optional()
        .map_err(sql)?;
    Ok(code.and_then(ObjectKind::from_pack_code))
}

fn held(conn: &Connection, repo: &str, id: &ObjectId) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM git_held WHERE repo = ?1 AND id = ?2",
        params![repo, id.as_bytes().as_slice()],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
}

fn ref_at(conn: &Connection, repo: &str, name: &str) -> rusqlite::Result<ObjectId> {
    let id = conn
        .query_row(
            "SELECT id FROM git_refs WHERE repo = ?1 AND name = ?2",
            [repo, name],
            |r| object_id(r.get(0)?),
        )
        .optional()?;
    Ok(id.unwrap_or(ObjectId::ZERO))
}

/// Writes `objects` into `repo`, then applies `updates`, each only when
/// its ref holds `old`; with `atomic`, all or none. The semantics of
/// [`clax_git::Repo::apply`], in one transaction. Objects over
/// [`MAX_WRITE_BYTES`] in all are refused, `invalid_argument`, and nothing
/// is written.
pub fn apply(
    conn: &Connection,
    repo: &str,
    objects: &[Object],
    updates: &[RefUpdate],
    atomic: bool,
) -> Result<Vec<RefStatus>> {
    require(conn, repo)?;
    let bytes: usize = objects.iter().map(|o| o.data.len()).sum();
    if bytes > MAX_WRITE_BYTES {
        return Err(GitError::InvalidArgument(format!(
            "The repository stand-in writes at most {MAX_WRITE_BYTES} bytes of objects at once; this write has {bytes}"
        )));
    }
    let tx = conn.unchecked_transaction().map_err(sql)?;
    for o in objects {
        let id = o.id();
        let id = id.as_bytes().as_slice();
        tx.execute(
            "INSERT OR IGNORE INTO git_objects (id, kind, data) VALUES (?1, ?2, ?3)",
            params![id, o.kind.pack_code(), o.data],
        )
        .map_err(sql)?;
        tx.execute(
            "INSERT OR IGNORE INTO git_held (repo, id) VALUES (?1, ?2)",
            params![repo, id],
        )
        .map_err(sql)?;
    }
    let check = |u: &RefUpdate| -> rusqlite::Result<RefStatus> {
        if ref_at(&tx, repo, &u.name)? != u.old {
            return Ok(Err("stale info".to_string()));
        }
        if !u.new.is_zero() && !held(&tx, repo, &u.new)? {
            return Ok(Err("missing necessary objects".to_string()));
        }
        Ok(Ok(()))
    };
    let mut statuses = updates
        .iter()
        .map(check)
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql)?;
    if atomic && statuses.iter().any(|s| s.is_err()) {
        for s in &mut statuses {
            if s.is_ok() {
                *s = Err(ATOMIC_FAILED.to_string());
            }
        }
    } else {
        for (u, s) in updates.iter().zip(&mut statuses) {
            if s.is_err() {
                continue;
            }
            // An earlier update in the same call may have moved this ref.
            if ref_at(&tx, repo, &u.name).map_err(sql)? != u.old {
                *s = Err("stale info".to_string());
                continue;
            }
            if u.new.is_zero() {
                tx.execute(
                    "DELETE FROM git_refs WHERE repo = ?1 AND name = ?2",
                    [repo, u.name.as_str()],
                )
            } else {
                tx.execute(
                    "INSERT INTO git_refs (repo, name, id) VALUES (?1, ?2, ?3)
                     ON CONFLICT (repo, name) DO UPDATE SET id = excluded.id",
                    params![repo, u.name, u.new.as_bytes().as_slice()],
                )
            }
            .map_err(sql)?;
        }
    }
    tx.commit().map_err(sql)?;
    Ok(statuses)
}

// ── Tokens ──────────────────────────────────────────────────────────────────

/// A token as the ledger keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRow {
    pub id: String,
    pub repo: String,
    pub scope: String,
    pub expires_ms: i64,
    pub revoked_ms: Option<i64>,
    /// Whom it was handed to.
    pub holder: super::tokens::Holder,
}

/// Enters a minted token in the ledger; `hash` is the SHA-256 of a
/// stand-in token's plaintext.
pub fn insert_token(conn: &Connection, row: &TokenRow, hash: Option<&[u8]>) -> Result<()> {
    conn.execute(
        "INSERT INTO git_tokens
             (id, repo, scope, expires_ms, revoked_ms, hash, person, session, link)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8)",
        params![
            row.id,
            row.repo,
            row.scope,
            row.expires_ms,
            hash,
            row.holder.person,
            row.holder.session,
            row.holder.link
        ],
    )
    .map(drop)
    .map_err(sql)
}

pub fn token(conn: &Connection, id: &str) -> Result<Option<TokenRow>> {
    conn.query_row(
        "SELECT id, repo, scope, expires_ms, revoked_ms, person, session, link FROM git_tokens
         WHERE id = ?1",
        [id],
        |r| {
            Ok(TokenRow {
                id: r.get(0)?,
                repo: r.get(1)?,
                scope: r.get(2)?,
                expires_ms: r.get(3)?,
                revoked_ms: r.get(4)?,
                holder: super::tokens::Holder {
                    person: r.get(5)?,
                    session: r.get(6)?,
                    link: r.get(7)?,
                },
            })
        },
    )
    .optional()
    .map_err(sql)
}

/// The token whose plaintext hashes to `hash`, when it is live (neither
/// revoked nor expired at `now_ms`).
pub fn live_token_by_hash(conn: &Connection, hash: &[u8], now_ms: i64) -> Result<Option<TokenRow>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM git_tokens WHERE hash = ?1 AND revoked_ms IS NULL AND expires_ms > ?2",
            params![hash, now_ms],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql)?;
    match id {
        Some(id) => token(conn, &id),
        None => Ok(None),
    }
}

/// The tokens of `repo` live at `now_ms`, oldest first.
pub fn live_tokens(conn: &Connection, repo: &str, now_ms: i64) -> Result<Vec<TokenRow>> {
    let ids: Vec<String> = conn
        .prepare(
            "SELECT id FROM git_tokens WHERE repo = ?1 AND revoked_ms IS NULL AND expires_ms > ?2
             ORDER BY rowid",
        )
        .and_then(|mut s| {
            s.query_map(params![repo, now_ms], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .map_err(sql)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(row) = token(conn, &id)? {
            out.push(row);
        }
    }
    Ok(out)
}

/// Records that `holder` made a push leaving `main` of `repo` at `commit`
/// at `now_ms`, keeping the newest [`MAX_PUSHES_KEPT`] of the repository.
pub fn record_push(
    conn: &Connection,
    repo: &str,
    commit: &ObjectId,
    holder: &super::tokens::Holder,
    now_ms: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction().map_err(sql)?;
    tx.execute(
        "INSERT INTO git_pushes (repo, commit_id, person, session, at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![repo, commit.to_hex(), holder.person, holder.session, now_ms],
    )
    .map_err(sql)?;
    tx.execute(
        "DELETE FROM git_pushes WHERE repo = ?1 AND rowid NOT IN (
             SELECT rowid FROM git_pushes WHERE repo = ?1 ORDER BY rowid DESC LIMIT ?2)",
        params![repo, MAX_PUSHES_KEPT],
    )
    .map_err(sql)?;
    tx.commit().map_err(sql)
}

/// Who made the newest push leaving `main` of `repo` at `commit` (hex), or
/// `None` when no kept push did.
pub fn pusher(
    conn: &Connection,
    repo: &str,
    commit: &str,
) -> Result<Option<super::tokens::Holder>> {
    conn.query_row(
        "SELECT person, session FROM git_pushes WHERE repo = ?1 AND commit_id = ?2
         ORDER BY rowid DESC LIMIT 1",
        params![repo, commit],
        |r| {
            Ok(super::tokens::Holder {
                person: r.get(0)?,
                session: r.get(1)?,
                link: false,
            })
        },
    )
    .optional()
    .map_err(sql)
}

/// Marks token `id` of `repo` revoked; answers whether it was live.
pub fn revoke_token(conn: &Connection, repo: &str, id: &str, now_ms: i64) -> Result<bool> {
    conn.execute(
        "UPDATE git_tokens SET revoked_ms = ?3
         WHERE id = ?1 AND repo = ?2 AND revoked_ms IS NULL AND expires_ms > ?3",
        params![id, repo, now_ms],
    )
    .map(|n| n > 0)
    .map_err(sql)
}

/// Enters token `id` in the must-revoke list.
pub fn add_due(conn: &Connection, repo: &str, id: &str, expires_ms: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO git_tokens_due (id, repo, expires_ms) VALUES (?1, ?2, ?3)",
        params![id, repo, expires_ms],
    )
    .map(drop)
    .map_err(sql)
}

pub fn remove_due(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM git_tokens_due WHERE id = ?1", [id])
        .map(drop)
        .map_err(sql)
}

/// The must-revoke list, as `(repo, id)`, after dropping the tokens that
/// have expired by `now_ms` (an expired token needs no revoking).
pub fn due(conn: &Connection, now_ms: i64) -> Result<Vec<(String, String)>> {
    conn.execute(
        "DELETE FROM git_tokens_due WHERE expires_ms <= ?1",
        [now_ms],
    )
    .map_err(sql)?;
    let mut st = conn
        .prepare("SELECT repo, id FROM git_tokens_due ORDER BY id")
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(sql)?;
    rows.collect::<rusqlite::Result<_>>().map_err(sql)
}

/// Every token live at `now_ms` (neither revoked nor expired) that was
/// handed to a person, an agent session or a page's link, oldest first:
/// what the server rechecks when rights, visibility, links or sessions
/// change. The owner's own tokens (no person, no session, no link) are not
/// listed.
pub fn live_held_tokens(conn: &Connection, now_ms: i64) -> Result<Vec<TokenRow>> {
    let ids: Vec<String> = conn
        .prepare(
            "SELECT id FROM git_tokens WHERE revoked_ms IS NULL AND expires_ms > ?1
               AND (person IS NOT NULL OR session IS NOT NULL OR link = 1)
             ORDER BY rowid",
        )
        .and_then(|mut s| {
            s.query_map(params![now_ms], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .map_err(sql)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(row) = token(conn, &id)? {
            out.push(row);
        }
    }
    Ok(out)
}

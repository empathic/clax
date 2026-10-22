//! Browser viewers, found by the `clax_viewer` cookie (a random secret,
//! kept as its SHA-256 in `viewer_cookies`; never the row ID) and named to
//! others by a public ID.

use super::Store;
use clax_core::audit::{Actor, AuditCtx, AuditKind, AuditRecord, Via, sha256_hex};
use clax_core::gitctx::GitField;
use clax_core::ids::{is_public_id, is_ulid, new_public_id};
use clax_core::model::Viewer;
use clax_core::secret::{Secret, is_viewer_cookie, viewer_cookie};
use clax_core::visibility::OWNER;
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Longest accepted display name, in characters.
pub const MAX_NAME_CHARS: usize = 60;

/// Most named viewers one [`Store::search_viewers`] call reads. The match runs
/// in Rust (SQLite folds ASCII case only), so a search costs time linear in the
/// names it reads; past this many it stops, and names later in name order are
/// not found by that search.
pub const MAX_SEARCH_SCAN: usize = 10_000;

const VIEWER_SELECT: &str =
    "SELECT id, public_id, display_name, created_at, person, person_revoked_at FROM viewers";

/// The viewer a cookie's hash (`?1`) names: never the owner's row, never a
/// person on a legacy cookie (spec 2026-10-08 §21.4), and not on a legacy
/// cookie replaced at or before `?2` ([`LEGACY_SWAP_GRACE_SECS`]).
const VIEWER_BY_COOKIE: &str =
    "SELECT v.id, v.public_id, v.display_name, v.created_at, v.person, v.person_revoked_at
     FROM viewer_cookies c JOIN viewers v ON v.id = c.viewer_id
     WHERE c.sha256 = ?1 AND v.owner = 0 AND (c.legacy = 0 OR v.person = 0)
       AND (c.replaced_at IS NULL OR c.replaced_at > ?2)";

/// How long a legacy cookie that [`Store::reissue_viewer_cookie`] replaced
/// keeps naming its viewer: a browser's other tabs and the requests in
/// flight when it was replaced still carry it, and each such `GET
/// /api/viewers/me` gets a fresh secret for the same viewer rather than a
/// new viewer.
pub const LEGACY_SWAP_GRACE_SECS: i64 = 600;

/// Whether `cookie` has a form a `clax_viewer` cookie may take: a secret
/// the daemon mints ([`viewer_cookie`]), or a legacy ULID (a viewer's row
/// ID, the cookie before migration 24).
pub fn is_cookie(cookie: &str) -> bool {
    is_viewer_cookie(cookie) || is_ulid(cookie)
}

/// Whether `cookie` is of the legacy form: a ULID.
pub fn is_legacy_cookie(cookie: &str) -> bool {
    is_ulid(cookie)
}

fn viewer_by_cookie(
    st: &Store,
    c: &rusqlite::Connection,
    cookie: &str,
) -> rusqlite::Result<Option<Viewer>> {
    c.query_row(
        VIEWER_BY_COOKIE,
        params![clax_core::secret::sha256_hex(cookie), swap_cutoff(st)],
        row_to_viewer,
    )
    .optional()
}

/// The replacement time at or before which a replaced legacy cookie names
/// no one.
fn swap_cutoff(st: &Store) -> String {
    clax_core::time::stamp(st.now_utc() - chrono::Duration::seconds(LEGACY_SWAP_GRACE_SECS))
}

/// Records `cookie` (its hash) as a cookie of viewer row `id`.
fn add_cookie(tx: &rusqlite::Transaction<'_>, id: &str, cookie: &str, now: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO viewer_cookies (sha256, viewer_id, created_at, legacy) VALUES (?1, ?2, ?3, ?4)",
        params![clax_core::secret::sha256_hex(cookie), id, now, is_legacy_cookie(cookie)],
    )?;
    Ok(())
}

pub(super) fn row_to_viewer(r: &rusqlite::Row<'_>) -> rusqlite::Result<Viewer> {
    Ok(Viewer {
        id: r.get(0)?,
        public_id: r.get(1)?,
        display_name: r.get(2)?,
        created_at: r.get(3)?,
        person: r.get(4)?,
        person_revoked_at: r.get(5)?,
    })
}

/// Checks a display name: at most [`MAX_NAME_CHARS`] characters, no control
/// characters, and no bidirectional embedding, override or isolate
/// controls ([`clax_core::names::has_bidi_control`]), which can make it render
/// as another name. Any script, or mix of scripts, is fine. Returns it
/// trimmed, `None` for an empty one.
pub(super) fn checked_name(name: &str) -> Result<Option<&str>> {
    let n = name.trim();
    if n.chars().any(char::is_control) || n.chars().count() > MAX_NAME_CHARS {
        return Err(CoreError::invalid(
            "invalid_name",
            format!(
                "a display name is at most {MAX_NAME_CHARS} characters with no control characters"
            ),
        ));
    }
    if clax_core::names::has_bidi_control(n) {
        return Err(CoreError::invalid(
            "invalid_name",
            "a display name cannot hold bidirectional embedding, override or isolate characters (U+202A to U+202E, U+2066 to U+2069), which change how it reads",
        ));
    }
    Ok(Some(n).filter(|n| !n.is_empty()))
}

/// Refuses viewer `id` the display name `name` (spec §9.6) when it looks
/// like ([`clax_core::names::CollisionKeys`]) a protected name other than its
/// own: the owner's, a connected person's, or a live agent's.
///
/// # Errors
/// `name_taken` as above.
pub(super) fn check_viewer_name(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    name: &str,
) -> Result<()> {
    let key = clax_core::names::CollisionKeys::of(name);
    let taken = || CoreError::invalid("name_taken", format!("the name '{name}' is taken"));
    let mut stmt = tx.prepare_cached(
        "SELECT display_name FROM viewers WHERE display_name IS NOT NULL AND id != ?1
           AND (owner = 1 OR (person = 1 AND person_revoked_at IS NULL))",
    )?;
    for other in stmt.query_map(params![id], |r| r.get::<_, String>(0))? {
        if clax_core::names::CollisionKeys::of(&other?).meets(&key) {
            return Err(taken());
        }
    }
    if super::sessions::live_name_looks_like(tx, &key)? {
        return Err(taken());
    }
    Ok(())
}

impl Store {
    /// Creates the viewer behind `cookie` when there is none, with a new
    /// row ID and a new public ID ([`clax_core::new_public_id`]) that it keeps
    /// for good; the cookie's SHA-256 names it from then on. `display_name`:
    /// `None` keeps the current name, `Some("")` (after trimming) clears it,
    /// any other value replaces it. The owner's row is never reached this
    /// way (a viewer cookie never names it).
    ///
    /// # Errors
    /// `invalid_viewer` when `cookie` has no cookie's form ([`is_cookie`]),
    /// or is a legacy cookie of a viewer since made a person; `invalid_name`
    /// for a name with control characters or longer than [`MAX_NAME_CHARS`];
    /// `name_taken` for the owner's or a connected person's name
    /// ([`check_viewer_name`]).
    pub fn upsert_viewer(&self, cookie: &str, display_name: Option<&str>) -> Result<Viewer> {
        self.upsert_minted(cookie, display_name, None)
    }

    /// [`Store::upsert_viewer`] for a cookie the daemon is minting now, on a
    /// request from this machine (`local`: a loopback peer naming a literal
    /// local host) or not. Only a viewer minted locally may later be claimed
    /// for the owner ([`Store::claim_for_owner`]).
    pub fn mint_viewer(&self, cookie: &str, local: bool) -> Result<Viewer> {
        self.upsert_minted(cookie, None, Some(local))
    }

    fn upsert_minted(
        &self,
        cookie: &str,
        display_name: Option<&str>,
        local: Option<bool>,
    ) -> Result<Viewer> {
        if !is_cookie(cookie) {
            return Err(CoreError::invalid(
                "invalid_viewer",
                "a viewer cookie is a cxv_ secret",
            ));
        }
        let name = display_name.map(checked_name).transpose()?;
        let no_viewer = || CoreError::invalid("invalid_viewer", "that cookie names no viewer");
        self.with_tx(|tx| {
            let hash = clax_core::secret::sha256_hex(cookie);
            let known: Option<String> = tx
                .query_row(
                    "SELECT viewer_id FROM viewer_cookies WHERE sha256 = ?1",
                    params![hash],
                    |r| r.get(0),
                )
                .optional()?;
            let id = match known {
                Some(_) => viewer_by_cookie(self, tx, cookie)?.ok_or_else(no_viewer)?.id,
                None => {
                    let id = clax_core::new_ulid();
                    let now = self.now();
                    tx.execute(
                        "INSERT INTO viewers (id, public_id, created_at, minted_local) VALUES (?1, ?2, ?3, ?4)",
                        params![id, new_public_id(), now, local],
                    )?;
                    add_cookie(tx, &id, cookie, &now)?;
                    id
                }
            };
            if let Some(n) = name {
                if let Some(n) = n {
                    check_viewer_name(tx, &id, n)?;
                }
                tx.execute(
                    "UPDATE viewers SET display_name = ?2 WHERE id = ?1 AND owner = 0",
                    params![id, n],
                )?;
            }
            tx.query_row(&format!("{VIEWER_SELECT} WHERE id = ?1 AND owner = 0"), params![id], row_to_viewer)
                .optional()?
                .ok_or_else(no_viewer)
        })
    }

    /// A new cookie for viewer row `viewer_id` (a browser it gains: a
    /// person's browser link, spec §6.5), its hash recorded; the viewer's
    /// other cookies keep working.
    ///
    /// # Errors
    /// `invalid_viewer` for the owner's row or an unknown one.
    pub fn issue_viewer_cookie(&self, viewer_id: &str) -> Result<Secret> {
        self.with_tx(|tx| issue_cookie_in(self, tx, viewer_id))
    }

    /// Replaces legacy cookie `old` (a ULID, see [`is_legacy_cookie`]) with
    /// a fresh secret naming the same viewer: `None` when it names none. The
    /// old cookie keeps naming that viewer (never with a person's authority)
    /// for [`LEGACY_SWAP_GRACE_SECS`] after its first replacement, and each
    /// call in that time gets another fresh secret for the same viewer, so
    /// requests already in flight with it, and a browser's other tabs, never
    /// make a new viewer; after it, the old cookie names no one. Expired
    /// replaced cookies are removed.
    pub fn reissue_viewer_cookie(&self, old: &str) -> Result<Option<(Viewer, Secret)>> {
        self.with_tx(|tx| {
            let cutoff = swap_cutoff(self);
            tx.execute(
                "DELETE FROM viewer_cookies WHERE legacy = 1 AND replaced_at IS NOT NULL AND replaced_at <= ?1",
                params![cutoff],
            )?;
            let Some(v) = viewer_by_cookie(self, tx, old)? else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE viewer_cookies SET replaced_at = COALESCE(replaced_at, ?2) WHERE sha256 = ?1",
                params![clax_core::secret::sha256_hex(old), self.now()],
            )?;
            let fresh = issue_cookie_in(self, tx, &v.id)?;
            Ok(Some((v, fresh)))
        })
    }

    /// Every display name viewers have, the owner's included, for telling
    /// a person's `@` mention from an agent's (spec §11).
    pub fn viewer_display_names(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT display_name FROM viewers WHERE display_name IS NOT NULL AND display_name != ''",
            )?;
            Ok(stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?)
        })
    }

    /// The viewer cookie `cookie` names, looked up by its SHA-256, never by
    /// row ID; never the owner's row, and never a person on a legacy
    /// cookie.
    pub fn get_viewer(&self, cookie: &str) -> Result<Option<Viewer>> {
        if !is_cookie(cookie) {
            return Ok(None);
        }
        self.with_read(|c| Ok(viewer_by_cookie(self, c, cookie)?))
    }

    /// The viewer whose public ID is `public_id`; `None` for anything else,
    /// including a cookie value.
    pub fn viewer_by_public_id(&self, public_id: &str) -> Result<Option<Viewer>> {
        if !is_public_id(public_id) {
            return Ok(None);
        }
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{VIEWER_SELECT} WHERE public_id = ?1"),
                params![public_id],
                row_to_viewer,
            )
            .optional()?)
        })
    }
}

/// What [`Store::claim_for_owner`] did with a browser's viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Nothing: the cookie names no viewer minted on this machine.
    Nothing,
    /// The viewer became the owner, keeping its public ID, name and history.
    /// `retired` is the public ID of an owner row that held no browser's
    /// identity yet (made for the CLI): its history moved to this viewer and
    /// that ID names no one from now on.
    Adopted { retired: Option<String> },
    /// The viewer was folded into the owner and removed; its public ID
    /// (given here) names no one from now on.
    Merged(String),
}

fn owner_row(c: &rusqlite::Connection) -> rusqlite::Result<Option<(Viewer, bool)>> {
    c.query_row(
        "SELECT id, public_id, display_name, created_at, person, person_revoked_at, claimed
         FROM viewers WHERE owner = 1",
        [],
        |r| Ok((row_to_viewer(r)?, r.get::<_, bool>(6)?)),
    )
    .optional()
}

/// Names the owner's row `owner_id` `name` (a [`checked_name`]; `None`
/// clears it) in `tx`, and returns the row.
///
/// # Errors
/// `name_taken` as [`check_viewer_name`].
pub(super) fn name_owner_in(
    tx: &rusqlite::Transaction<'_>,
    owner_id: &str,
    name: Option<&str>,
) -> Result<Viewer> {
    if let Some(n) = name {
        check_viewer_name(tx, owner_id, n)?;
    }
    tx.execute(
        "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
        params![owner_id, name],
    )?;
    Ok(owner_row(tx)?.expect("the owner row exists").0)
}

/// Moves `from`'s history to `into` and deletes `from`: its comments'
/// author, its mentions, the threads it resolved, its seen marks (the higher
/// of the two, pruned to [`clax_core::changelog::MAX_SEEN_PER_VIEWER`]), its
/// looked-at marks (the later), its private documents
/// (`data/users/<from>/...` move under `data/users/<into>/` with a new
/// version, their authors with them; one whose destination exists stays
/// where it is), its authorship as a person (`person` columns, which
/// become NULL when `into` is the owner; documents' and pages' authors,
/// batch senders, agents' resolves, versions' file authors and
/// artifacts' text authors, which become `*owner`), its approvals given and received and its
/// approval requests (see `fold_authorship`), its invites, grants and
/// device-code decisions, its hosted rows (see `fold_hosted`) and its
/// artifact shares ([`super::shares::fold_shares`]). The elevations it gave
/// as an artifact's creator are dropped. A row of `from` left in any table
/// with a `viewer_id` column fails the fold (see `assert_folded`), so a
/// table added later cannot be missed silently. When `from` is the owner's
/// row, `into` becomes the owner, as the owner's credentials move to it.
fn fold(
    st: &Store,
    ctx: &AuditCtx,
    tx: &rusqlite::Transaction<'_>,
    from: &Viewer,
    into: &Viewer,
) -> Result<()> {
    let (old, new) = (from.public_id.as_str(), into.public_id.as_str());
    let into_owner: bool = tx.query_row(
        "SELECT owner FROM viewers WHERE id = ?1",
        params![into.id],
        |r| r.get(0),
    )?;
    let from_owner: bool = tx.query_row(
        "SELECT owner FROM viewers WHERE id = ?1",
        params![from.id],
        |r| r.get(0),
    )?;
    let person = (!into_owner).then_some(new);
    // The raises `from` gave name artifacts it no longer created: they go
    // (spec §14).
    super::elevations::drop_granted_in(st, tx, ctx, old, "viewer_folded")?;
    for (table, column) in [
        ("sessions", "person"),
        ("artifacts", "person"),
        ("versions", "person"),
        ("assets", "person"),
    ] {
        tx.execute(
            &format!("UPDATE {table} SET {column} = ?2 WHERE {column} = ?1"),
            params![old, person],
        )?;
    }
    for table in ["invites", "grants", "device_codes"] {
        tx.execute(
            &format!("UPDATE {table} SET viewer_id = ?2 WHERE viewer_id = ?1"),
            params![from.id, into.id],
        )?;
    }
    fold_authorship(st, ctx, tx, old, new, into_owner)?;
    fold_hosted(tx, from, into, from_owner)?;
    super::shares::fold_shares(st, ctx, tx, (&from.id, old), (&into.id, new))?;
    tx.execute(
        "UPDATE comments SET author_public_id = ?2 WHERE author_public_id = ?1",
        params![old, new],
    )?;
    tx.execute(
        "UPDATE OR IGNORE mentions SET public_id = ?2 WHERE public_id = ?1",
        params![old, new],
    )?;
    tx.execute("DELETE FROM mentions WHERE public_id = ?1", params![old])?;
    tx.execute(
        "UPDATE threads SET resolved_by = 'viewer:' || ?2 WHERE resolved_by = 'viewer:' || ?1",
        params![old, new],
    )?;
    tx.execute(
        "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at)
         SELECT ?2, artifact_id, seen_n, updated_at FROM viewer_seen WHERE viewer_id = ?1 AND true
         ON CONFLICT (viewer_id, artifact_id) DO UPDATE SET
           seen_n = MAX(seen_n, excluded.seen_n), updated_at = MAX(updated_at, excluded.updated_at)",
        params![from.id, into.id],
    )?;
    tx.execute(
        "DELETE FROM viewer_seen WHERE viewer_id = ?1",
        params![from.id],
    )?;
    tx.execute(
        "DELETE FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id NOT IN
           (SELECT artifact_id FROM viewer_seen WHERE viewer_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2)",
        params![into.id, clax_core::changelog::MAX_SEEN_PER_VIEWER as i64],
    )?;
    tx.execute(
        "INSERT INTO viewer_threads (viewer_id, thread_id, looked_at)
         SELECT ?2, thread_id, looked_at FROM viewer_threads WHERE viewer_id = ?1 AND true
         ON CONFLICT (viewer_id, thread_id) DO UPDATE SET looked_at = MAX(looked_at, excluded.looked_at)",
        params![from.id, into.id],
    )?;
    tx.execute(
        "DELETE FROM viewer_threads WHERE viewer_id = ?1",
        params![from.id],
    )?;
    move_private_docs(st, ctx, tx, old, new)?;
    forget_cookies(tx, &from.id)?;
    assert_folded(tx, &from.id)?;
    tx.execute("DELETE FROM viewers WHERE id = ?1", params![from.id])?;
    if from_owner {
        tx.execute(
            "UPDATE viewers SET owner = 1 WHERE id = ?1",
            params![into.id],
        )?;
    }
    Ok(())
}

/// Moves `from`'s hosted rows to `into` in a fold. The owner's row keeps
/// its credentials: when `from` is the owner's (`from_owner`), its browser
/// sessions, unused sign-in links, setup and GitHub account move to
/// `into`, which [`fold`] then makes the owner (a GitHub account `into`
/// already has stays, and `from`'s is unlinked). Any other row's credentials are forgotten, as
/// its cookies are, so a browser that held them does not become `into`.
/// Its agreements to the legal documents move, `into`'s own kept.
fn fold_hosted(
    tx: &rusqlite::Transaction<'_>,
    from: &Viewer,
    into: &Viewer,
    from_owner: bool,
) -> Result<()> {
    let ids = params![from.id, into.id];
    if from_owner {
        for table in ["browser_sessions", "signin_links", "hosted_setup"] {
            tx.execute(
                &format!("UPDATE {table} SET viewer_id = ?2 WHERE viewer_id = ?1"),
                ids,
            )?;
        }
        tx.execute(
            "UPDATE OR IGNORE github_identities SET viewer_id = ?2 WHERE viewer_id = ?1",
            ids,
        )?;
    }
    for table in [
        "browser_sessions",
        "signin_links",
        "hosted_setup",
        "github_identities",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE viewer_id = ?1"),
            params![from.id],
        )?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO legal_agreements (viewer_id, version, agreed_at, via)
         SELECT ?2, version, agreed_at, via FROM legal_agreements WHERE viewer_id = ?1",
        ids,
    )?;
    tx.execute(
        "DELETE FROM legal_agreements WHERE viewer_id = ?1",
        params![from.id],
    )?;
    Ok(())
}

/// Fails unless no table with a `viewer_id` column has a row naming
/// viewer row `id`: what a fold checks before it deletes the row.
fn assert_folded(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<()> {
    let tables: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) p
              WHERE m.type = 'table' AND p.name = 'viewer_id' ORDER BY m.name",
        )?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for table in tables {
        let left: bool = tx.query_row(
            &format!("SELECT EXISTS (SELECT 1 FROM \"{table}\" WHERE viewer_id = ?1)"),
            params![id],
            |r| r.get(0),
        )?;
        if left {
            return Err(CoreError::invalid(
                "fold_incomplete",
                format!("a folded viewer still has rows in {table}"),
            ));
        }
    }
    Ok(())
}

/// `json` (a map of author lists) with `old` as `token`, each author once
/// per list; `None` when it does not parse.
fn rewrite_authors(json: &str, old: &str, token: &str) -> Option<String> {
    let mut map: std::collections::BTreeMap<String, Vec<Option<String>>> =
        serde_json::from_str(json).ok()?;
    for authors in map.values_mut() {
        let mut out: Vec<Option<String>> = Vec::with_capacity(authors.len());
        for a in authors.drain(..) {
            let a = if a.as_deref() == Some(old) {
                Some(token.to_string())
            } else {
                a
            };
            if !out.contains(&a) {
                out.push(a);
            }
        }
        *authors = out;
    }
    Some(serde_json::to_string(&map).expect("serialisable map"))
}

/// Moves the authorship of migration 25 (document and page authors, batch
/// senders, agents' resolves, artifact text authors, approvals and their
/// requests) and
/// `file_authors_json` from public ID `old` to `new` (to `*owner`, or NULL
/// for an approver, when `into_owner`).
///
/// Approvals and requests: one that would repeat a live one ends first,
/// so the unique indexes hold; and folding into the owner ends every live
/// approval and open request whose author is `old`, since another
/// person's approval of a LAN viewer must not become one of all the
/// owner's content. Each approval ended is recorded as `approval.revoke`
/// under `ctx`.
fn fold_authorship(
    st: &Store,
    ctx: &AuditCtx,
    tx: &rusqlite::Transaction<'_>,
    old: &str,
    new: &str,
    into_owner: bool,
) -> Result<()> {
    let token = if into_owner { OWNER } else { new };
    for (table, key) in [("doc_authors", "author"), ("page_authors", "author")] {
        tx.execute(
            &format!("UPDATE OR IGNORE {table} SET {key} = ?2 WHERE {key} = ?1"),
            params![old, token],
        )?;
        tx.execute(
            &format!("DELETE FROM {table} WHERE {key} = ?1"),
            params![old],
        )?;
    }
    tx.execute(
        "UPDATE send_batches SET sender = ?2 WHERE sender = ?1",
        params![old, token],
    )?;
    tx.execute(
        "UPDATE threads SET resolved_by_person = ?2 WHERE resolved_by_person = ?1",
        params![old, token],
    )?;
    for (table, column) in [
        ("versions", "file_authors_json"),
        ("artifacts", "text_authors_json"),
    ] {
        let rows: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT rowid, {column} FROM {table} WHERE {column} IS NOT NULL AND instr({column}, ?1) > 0"
            ))?;
            stmt.query_map(params![old], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (rowid, json) in rows {
            if let Some(json) = rewrite_authors(&json, old, token) {
                tx.execute(
                    &format!("UPDATE {table} SET {column} = ?2 WHERE rowid = ?1"),
                    params![rowid, json],
                )?;
            }
        }
    }
    let approver = (!into_owner).then_some(new);
    let now = st.now();
    let ending: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM approvals WHERE revoked_at IS NULL AND (approver = ?1 OR author = ?1)
               AND ((?4 AND author = ?1)
                 OR EXISTS (SELECT 1 FROM approvals b WHERE b.revoked_at IS NULL AND b.id <> approvals.id
                   AND COALESCE(b.approver, '') = COALESCE(CASE WHEN approvals.approver = ?1 THEN ?2 ELSE approvals.approver END, '')
                   AND b.author = CASE WHEN approvals.author = ?1 THEN ?3 ELSE approvals.author END
                   AND b.scope = approvals.scope
                   AND COALESCE(b.artifact_id, '') = COALESCE(approvals.artifact_id, '')
                   AND COALESCE(b.thread_id, '') = COALESCE(approvals.thread_id, '')))
             ORDER BY created_at, id",
        )?;
        stmt.query_map(params![old, approver, new, into_owner], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for id in ending {
        super::approvals::revoke_in(st, tx, ctx, &id, &now)?;
    }
    tx.execute(
        "UPDATE approval_requests SET status = 'withdrawn', updated_at = ?3
         WHERE status = 'open' AND (approver = ?1 OR author = ?1)
           AND ((?5 AND author = ?1)
             OR EXISTS (SELECT 1 FROM approval_requests b WHERE b.status = 'open' AND b.id <> approval_requests.id
               AND COALESCE(b.approver, '') = COALESCE(CASE WHEN approval_requests.approver = ?1 THEN ?2 ELSE approval_requests.approver END, '')
               AND b.author = CASE WHEN approval_requests.author = ?1 THEN ?4 ELSE approval_requests.author END
               AND b.artifact_id = approval_requests.artifact_id
               AND COALESCE(b.thread_id, '') = COALESCE(approval_requests.thread_id, '')))",
        params![old, approver, now, new, into_owner],
    )?;
    for table in ["approvals", "approval_requests"] {
        tx.execute(
            &format!("UPDATE {table} SET approver = ?2 WHERE approver = ?1"),
            params![old, approver],
        )?;
        tx.execute(
            &format!("UPDATE {table} SET author = ?2 WHERE author = ?1"),
            params![old, new],
        )?;
    }
    Ok(())
}

/// Records `viewer.claim`: `from`'s public ID is retired in favour of `to`'s.
fn record_claim(
    st: &Store,
    tx: &rusqlite::Transaction<'_>,
    ctx: &AuditCtx,
    from: &Viewer,
    to: &Viewer,
) -> Result<()> {
    let rec = AuditRecord::new(AuditKind::ViewerClaim, st.now())
        .with("from_public_id", from.public_id.as_str())
        .with("to_public_id", to.public_id.as_str());
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Moves every `data/users/<old>/...` document to `data/users/<new>/...`,
/// each with the next version of its artifact; a document whose destination
/// exists is left in place (and logged), and the destination kept. Each move
/// is recorded as `doc.move` under `ctx`, with both paths and the document's
/// hash, never its content.
fn move_private_docs(
    st: &Store,
    ctx: &AuditCtx,
    tx: &rusqlite::Transaction<'_>,
    old: &str,
    new: &str,
) -> Result<()> {
    let from = format!("data/users/{old}/");
    let to = format!("data/users/{new}/");
    let rows: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT artifact_id, path, json FROM docs WHERE substr(path, 1, length(?1)) = ?1",
        )?;
        stmt.query_map(params![from], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (aid, path, json) in rows {
        let dest = format!("{to}{}", &path[from.len()..]);
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM docs WHERE artifact_id = ?1 AND path = ?2)",
            params![aid, dest],
            |r| r.get(0),
        )?;
        if taken {
            tracing::warn!(artifact = %aid, %path, %dest, "a private document stays at its old path: the owner already has one there");
            continue;
        }
        let dp = clax_core::db::doc_path(&dest)?;
        let version: i64 = tx.query_row(
            "UPDATE artifacts SET doc_seq = doc_seq + 1 WHERE id = ?1 RETURNING doc_seq",
            params![aid],
            |r| r.get(0),
        )?;
        let now = st.now();
        let version: i64 = tx.query_row(
            "UPDATE docs SET path = ?3, collection = ?4, version = MAX(version + 1, ?5), updated_at = ?6
             WHERE artifact_id = ?1 AND path = ?2 RETURNING version",
            params![aid, path, dest, dp.collection, version, now],
            |r| r.get(0),
        )?;
        let mut rec = AuditRecord::new(AuditKind::DocMove, now.as_str())
            .with("from", path.as_str())
            .with("to", dest.as_str())
            .with("collection", dp.collection.as_str())
            .with("doc_id", dp.id.as_str())
            .with("version", version)
            .with("sha256", format!("sha256:{}", sha256_hex(json.as_bytes())));
        rec.ids.artifact = Some(aid.clone());
        st.record_audit(tx, ctx, rec)?;
        tx.execute(
            "DELETE FROM leases WHERE artifact_id = ?1 AND path = ?2",
            params![aid, path],
        )?;
        tx.execute(
            "UPDATE doc_authors SET path = ?3 WHERE artifact_id = ?1 AND path = ?2",
            params![aid, path, dest],
        )?;
    }
    Ok(())
}

/// A new cookie for viewer row `viewer_id`, its hash recorded.
pub(super) fn issue_cookie_in(
    st: &Store,
    tx: &rusqlite::Transaction<'_>,
    viewer_id: &str,
) -> Result<Secret> {
    let found: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM viewers WHERE id = ?1 AND owner = 0)",
        params![viewer_id],
        |r| r.get(0),
    )?;
    if !found {
        return Err(CoreError::invalid("invalid_viewer", "no such viewer"));
    }
    let cookie = viewer_cookie();
    add_cookie(tx, viewer_id, cookie.expose(), &st.now())?;
    Ok(cookie)
}

/// Forgets every cookie of viewer row `id`: none names it afterwards.
pub(super) fn forget_cookies(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM viewer_cookies WHERE viewer_id = ?1",
        params![id],
    )?;
    Ok(())
}

impl Store {
    /// The owner identity, when it exists: the one viewer that every owner
    /// credential (the bearer token, the owner and events cookies of the
    /// owner's browsers) acts as. Its `id` is never handed to anyone as a
    /// cookie.
    pub fn owner(&self) -> Result<Option<Viewer>> {
        self.with_read(|c| Ok(owner_row(c)?.map(|(v, _)| v)))
    }

    /// The owner identity, made when missing, with a new public ID and no
    /// name. `browser`: a browser of the owner's asks; its row then holds a
    /// browser's identity (a later claim folds into it, see
    /// [`Store::claim_for_owner`]). Without it (the CLI acting before any
    /// browser) a row made here gives way to the first browser claimed.
    /// Making the row bumps the visibility generation once an agent's view
    /// of who is connected has been read ([`Store::connected`]), as that
    /// view no longer holds.
    pub fn owner_viewer(&self, browser: bool) -> Result<Viewer> {
        if let Some((v, claimed)) = self.with_read(|c| Ok(owner_row(c)?))?
            && (claimed || !browser)
        {
            return Ok(v);
        }
        self.with_tx(|tx| {
            let made = tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at, owner, claimed)
                 SELECT ?1, ?2, NULL, ?3, 1, ?4 WHERE NOT EXISTS (SELECT 1 FROM viewers WHERE owner = 1)",
                params![clax_core::new_ulid(), new_public_id(), self.now(), browser],
            )?;
            // A new owner's row changes who is connected (spec §9.6): an
            // agent's view read before it marks the owner's own words as
            // an unconnected author's until the generation moves. With no
            // view read yet, none is stale, and nothing is rechecked.
            if made > 0 && self.connected_was_read() {
                self.visibility_changed();
            }
            if browser {
                tx.execute("UPDATE viewers SET claimed = 1 WHERE owner = 1", [])?;
            }
            Ok(owner_row(tx)?.expect("the owner row exists").0)
        })
    }

    /// Sets the owner's display name (made when missing, as for the CLI:
    /// see [`Store::owner_viewer`]); `""` clears it. `browser` as there.
    ///
    /// # Errors
    /// `invalid_name` as for [`Store::upsert_viewer`]; `name_taken` for a
    /// connected person's name or a live agent's ([`check_viewer_name`]).
    pub fn set_owner_name(&self, name: &str, browser: bool) -> Result<Viewer> {
        let name = checked_name(name)?;
        let v = self.owner_viewer(browser)?;
        self.with_tx(|tx| name_owner_in(tx, &v.id, name))
    }

    /// Claims the viewer behind `cookie` for the owner, on the shell's token
    /// request (an owner credential, so the cookie's browser is the owner's).
    /// Only a viewer the daemon minted for a request from this machine
    /// qualifies: a cookie another page planted, a LAN viewer's, one from
    /// before such origins were recorded, or a connected person's (revoked
    /// or not) claims nothing.
    ///
    /// With no owner yet, or one that holds no browser's identity (made for
    /// the CLI), the viewer becomes the owner, keeping its public ID, name
    /// and history ([`Claim::Adopted`]; such an earlier owner row is folded
    /// into it, its name kept when it has one). Otherwise the viewer is folded
    /// into the owner, which keeps its name when it has one
    /// ([`Claim::Merged`]). Either way the claimed viewer's cookies are
    /// forgotten, so the cookie names nothing afterwards. Each private document that follows
    /// a folded viewer to the owner's public ID is recorded as `doc.move`,
    /// and a claim that retires a public ID (the folded row's) as one
    /// `viewer.claim {from_public_id, to_public_id}`, so records naming the
    /// retired ID resolve to its successor; all made by the owner (as it is
    /// after the claim) through `via`, in the claim's transaction. A claim
    /// that claims anything bumps the visibility generation.
    pub fn claim_for_owner(&self, via: Via, cookie: &str) -> Result<Claim> {
        let by = |owner: &Viewer| AuditCtx {
            actor: Actor::Owner {
                public_id: owner.public_id.clone(),
            },
            via,
            git: GitField::Absent,
            call: None,
        };
        self.with_tx(|tx| {
            let legacy = match viewer_by_cookie(self, tx, cookie)? {
                Some(v) if !v.person => {
                    let local: bool = tx.query_row(
                        "SELECT minted_local = 1 FROM viewers WHERE id = ?1",
                        params![v.id],
                        |r| r.get::<_, Option<bool>>(0).map(|b| b.unwrap_or(false)),
                    )?;
                    if !local {
                        return Ok(Claim::Nothing);
                    }
                    v
                }
                _ => return Ok(Claim::Nothing),
            };
            // Authorship moves (M17).
            self.visibility_changed();
            match owner_row(tx)? {
                Some((owner, true)) => {
                    fold(self, &by(&owner), tx, &legacy, &owner)?;
                    record_claim(self, tx, &by(&owner), &legacy, &owner)?;
                    if owner.display_name.is_none() && legacy.display_name.is_some() {
                        tx.execute(
                            "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
                            params![owner.id, legacy.display_name],
                        )?;
                    }
                    Ok(Claim::Merged(legacy.public_id))
                }
                earlier => {
                    let retired = match earlier {
                        Some((cli, _)) => {
                            fold(self, &by(&legacy), tx, &cli, &legacy)?;
                            record_claim(self, tx, &by(&legacy), &cli, &legacy)?;
                            if cli.display_name.is_some() {
                                tx.execute(
                                    "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
                                    params![legacy.id, cli.display_name],
                                )?;
                            }
                            Some(cli.public_id)
                        }
                        None => None,
                    };
                    tx.execute(
                        "UPDATE viewers SET owner = 1, claimed = 1 WHERE id = ?1",
                        params![legacy.id],
                    )?;
                    forget_cookies(tx, &legacy.id)?;
                    Ok(Claim::Adopted { retired })
                }
            }
        })
    }
}

impl Store {
    /// The viewers with these public IDs, in the order given; unknown IDs,
    /// and anything that is not a public ID, are skipped.
    pub fn viewers_by_public_ids(&self, ids: &[String]) -> Result<Vec<Viewer>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!("{VIEWER_SELECT} WHERE public_id = ?1"))?;
            let mut out = Vec::new();
            for id in ids.iter().filter(|i| is_public_id(i)) {
                if let Some(v) = stmt.query_row(params![id], row_to_viewer).optional()? {
                    out.push(v);
                }
            }
            Ok(out)
        })
    }

    /// Up to `limit` named viewers whose name contains `q` as literal text,
    /// ignoring case in every script (Unicode lowercase on both sides),
    /// ordered by name (then public ID). Reads at most [`MAX_SEARCH_SCAN`]
    /// named viewers, in that order: its cost is linear in the names read.
    pub fn search_viewers(&self, q: &str, limit: usize) -> Result<Vec<Viewer>> {
        let needle = q.to_lowercase();
        self.with_read(|c| {
            // SQLite's lower() folds ASCII only, so names are matched here,
            // streaming in name order and stopping at `limit` hits or
            // MAX_SEARCH_SCAN names read.
            let mut stmt = c.prepare(&format!(
                "{VIEWER_SELECT} WHERE display_name IS NOT NULL
                 ORDER BY display_name COLLATE NOCASE, public_id"
            ))?;
            let mut out = Vec::new();
            for v in stmt.query_map([], row_to_viewer)?.take(MAX_SEARCH_SCAN) {
                if out.len() >= limit {
                    break;
                }
                let v = v?;
                if v.display_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&needle))
                {
                    out.push(v);
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Claim, MAX_NAME_CHARS, MAX_SEARCH_SCAN};
    use crate::test_util::{DAEMON, artifact, store};
    use clax_core::audit::Via;
    use clax_core::{CoreError, new_ulid};

    #[test]
    fn making_the_owners_row_after_an_agent_read_connects_the_owner() {
        use clax_core::visibility::Author;
        let (_d, st) = store();
        // An agent reads first: who is connected is cached with no owner.
        assert!(
            st.connected()
                .unwrap()
                .is_unconnected(&Author::Person("u_x".into()))
        );
        let g = st.visibility_generation();
        let owner = st.owner_viewer(true).unwrap();
        assert!(
            st.visibility_generation() > g,
            "the owner's row bumps the generation"
        );
        let c = st.connected().unwrap();
        assert_eq!(
            c.label("Alex", &Author::Person(owner.public_id.clone())),
            "Alex",
            "the owner's comments are not marked not connected"
        );
        // Asking again makes nothing, and bumps nothing.
        let g = st.visibility_generation();
        st.owner_viewer(true).unwrap();
        assert_eq!(st.visibility_generation(), g);
    }

    #[test]
    fn the_owners_row_made_after_agents_looked_is_connected_to_them() {
        use clax_core::visibility::Author;
        let (_d, st) = store();
        // An agent's view is read before any browser of the owner's came.
        let before = st.connected().unwrap();
        assert_eq!(st.owner().unwrap(), None);
        let v = st.owner_viewer(true).unwrap();
        assert!(before.is_unconnected(&Author::Person(v.public_id.clone())));
        // Making the owner's row is a change to who is connected.
        let after = st.connected().unwrap();
        assert!(!after.is_unconnected(&Author::Person(v.public_id.clone())));
        assert_eq!(after.label("Alex", &Author::Person(v.public_id)), "Alex");
        // Before any view was read, making the row moves nothing.
        let (_d2, fresh) = store();
        let g = fresh.visibility_generation();
        fresh.owner_viewer(true).unwrap();
        assert_eq!(fresh.visibility_generation(), g);
    }

    #[test]
    fn the_owner_is_one_viewer_no_cookie_reaches() {
        let (_d, st) = store();
        assert_eq!(st.owner().unwrap(), None, "made on first use only");
        let a = st.owner_viewer(false).unwrap();
        assert_eq!(st.owner_viewer(true).unwrap(), a);
        assert_eq!(st.owner().unwrap().as_ref(), Some(&a));
        assert_eq!(
            st.get_viewer(&a.id).unwrap(),
            None,
            "a cookie never names the owner"
        );
        // The owner's row ID as a cookie is an unknown cookie: a new
        // viewer, never the owner.
        let m = st.upsert_viewer(&a.id, Some("Mallory")).unwrap();
        assert_ne!(
            (m.id.as_str(), m.public_id.as_str()),
            (a.id.as_str(), a.public_id.as_str())
        );
        assert_eq!(
            st.set_owner_name(" Alex ", true)
                .unwrap()
                .display_name
                .as_deref(),
            Some("Alex")
        );
        assert_eq!(st.owner().unwrap().unwrap().public_id, a.public_id);
    }

    #[test]
    fn making_the_owner_row_marks_its_words_connected_at_once() {
        let (_d, st) = store();
        // Cached while there is no owner yet.
        assert_eq!(st.connected().unwrap().as_ref(), &Default::default());
        let owner = st.owner_viewer(true).unwrap();
        let author = clax_core::visibility::Author::Person(owner.public_id.clone());
        assert!(
            !st.connected().unwrap().is_unconnected(&author),
            "the owner's own words read as unconnected"
        );
    }

    fn private_doc(st: &crate::Store, aid: &clax_core::ArtifactId, path: &str, json: &str) {
        st.with_write(|c| {
            c.execute(
                "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 'x')",
                rusqlite::params![aid.as_str(), path, clax_core::db::doc_path(path).unwrap().collection, json],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn doc_json(st: &crate::Store, aid: &clax_core::ArtifactId, path: &str) -> Option<String> {
        use rusqlite::OptionalExtension;
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT json FROM docs WHERE artifact_id = ?1 AND path = ?2",
                rusqlite::params![aid.as_str(), path],
                |r| r.get(0),
            )
            .optional()?)
        })
        .unwrap()
    }

    #[test]
    fn only_a_viewer_minted_on_this_machine_is_claimed() {
        let (_d, st) = store();
        // Before origins were recorded, from elsewhere (a LAN viewer, whose
        // cookie another page could plant), or unknown: never the owner.
        let unknown = st.upsert_viewer(&new_ulid(), Some("Old")).unwrap();
        let lan = new_ulid();
        st.mint_viewer(&lan, false).unwrap();
        for c in [
            unknown.id.as_str(),
            lan.as_str(),
            "01J9Z3K4M5N6P7Q8R9S0T1V2W3",
        ] {
            assert_eq!(
                st.claim_for_owner(Via::Shell, c).unwrap(),
                Claim::Nothing,
                "{c}"
            );
        }
        assert_eq!(st.owner().unwrap(), None);
        assert!(
            st.get_viewer(&lan).unwrap().is_some(),
            "the LAN viewer is untouched"
        );
        assert!(claims(&st).is_empty());
    }

    #[test]
    fn the_first_claimed_viewer_becomes_the_owner_under_a_new_private_id() {
        let (_d, st) = store();
        let cookie = new_ulid();
        let chrome = st.mint_viewer(&cookie, true).unwrap();
        st.upsert_viewer(&cookie, Some("Alex")).unwrap();
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Adopted { retired: None }
        );
        let owner = st.owner().unwrap().unwrap();
        assert_eq!(
            (owner.public_id.as_str(), owner.display_name.as_deref()),
            (chrome.public_id.as_str(), Some("Alex")),
            "the adopted viewer keeps its public ID and name"
        );
        assert_ne!(owner.id, cookie, "the old cookie no longer names the row");
        assert_eq!(st.get_viewer(&cookie).unwrap(), None);
        assert!(st.upsert_viewer(&cookie, Some("x")).unwrap().public_id != owner.public_id);
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Nothing,
            "a fresh row is not local"
        );
        assert!(claims(&st).is_empty(), "no public ID was retired");
    }

    #[test]
    fn a_browser_claimed_after_the_cli_keeps_its_ids_and_takes_the_clis_history() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let cli = st.set_owner_name("Alex", false).unwrap();
        st.create_thread(DAEMON, &aid, thread_by(&cli.public_id, "from the CLI"))
            .unwrap();
        private_doc(
            &st,
            &aid,
            &format!("data/users/{}/prefs", cli.public_id),
            "{\"cli\":1}",
        );
        let cookie = new_ulid();
        let chrome = st.mint_viewer(&cookie, true).unwrap();
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Adopted {
                retired: Some(cli.public_id.clone())
            }
        );
        let owner = st.owner().unwrap().unwrap();
        assert_eq!(
            owner.public_id, chrome.public_id,
            "the browser's user ID stays"
        );
        assert_eq!(
            owner.display_name.as_deref(),
            Some("Alex"),
            "the name set in the CLI stays"
        );
        assert_eq!(st.viewer_by_public_id(&cli.public_id).unwrap(), None);
        let author: String = st
            .with_read(|c| {
                Ok(c.query_row("SELECT author_public_id FROM comments", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(author, chrome.public_id);
        assert_eq!(
            doc_json(&st, &aid, &format!("data/users/{}/prefs", chrome.public_id)).as_deref(),
            Some("{\"cli\":1}")
        );
        // The move is the owner's, through the shell, and names no content.
        let moves = doc_moves(&st);
        assert_eq!(moves.len(), 1);
        let (actor, artifact, body) = &moves[0];
        assert_eq!(
            actor,
            &serde_json::json!({"type": "owner", "public_id": chrome.public_id})
        );
        assert_eq!(artifact.as_deref(), Some(aid.as_str()));
        assert_eq!(body["via"], "shell");
        assert_eq!(body["from"], format!("data/users/{}/prefs", cli.public_id));
        assert_eq!(body["to"], format!("data/users/{}/prefs", chrome.public_id));
        assert_eq!(
            body["sha256"],
            format!("sha256:{}", clax_core::audit::sha256_hex(b"{\"cli\":1}"))
        );
        // The CLI's retired public ID resolves to the browser's.
        assert_eq!(
            claims(&st),
            vec![(
                chrome.public_id.clone(),
                cli.public_id.clone(),
                chrome.public_id.clone()
            )]
        );
        assert!(!body.to_string().contains("\\\"cli\\\""), "{body}");
    }

    /// The events of `kind`: actor, artifact ID and body.
    fn events_of(
        st: &crate::Store,
        kind: &str,
    ) -> Vec<(serde_json::Value, Option<String>, serde_json::Value)> {
        st.events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                (
                    serde_json::from_str(&e.actor).unwrap(),
                    e.ids.artifact,
                    serde_json::from_str(&e.body).unwrap(),
                )
            })
            .collect()
    }

    fn doc_moves(st: &crate::Store) -> Vec<(serde_json::Value, Option<String>, serde_json::Value)> {
        events_of(st, "doc.move")
    }

    /// The `viewer.claim` events as (actor's public ID, from, to).
    fn claims(st: &crate::Store) -> Vec<(String, String, String)> {
        events_of(st, "viewer.claim")
            .into_iter()
            .map(|(actor, _, b)| {
                let s = |v: &serde_json::Value| v.as_str().unwrap().to_string();
                (
                    s(&actor["public_id"]),
                    s(&b["from_public_id"]),
                    s(&b["to_public_id"]),
                )
            })
            .collect()
    }

    fn thread_by(public_id: &str, body: &str) -> crate::threads::NewThread {
        crate::threads::NewThread {
            author_public_id: Some(public_id.to_string()),
            version_n: 1,
            anchor: crate::test_util::anchor(),
            author_name: "Alex S".into(),
            body: body.into(),
            clip: None,
            via_page: false,
        }
    }

    #[test]
    fn a_later_claimed_viewer_merges_into_the_owner() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let owner = st.owner_viewer(true).unwrap();
        let cookie = new_ulid();
        let safari = st.mint_viewer(&cookie, true).unwrap();
        st.upsert_viewer(&cookie, Some("Alex S")).unwrap();
        let t = st
            .create_thread(DAEMON, &aid, thread_by(&safari.public_id, "hi"))
            .unwrap();
        st.resolve_thread(DAEMON, &t.id, &format!("viewer:{}", safari.public_id))
            .unwrap();
        st.mark_seen(&safari.id, &aid, 1).unwrap();
        st.mark_looked(&safari.id, &aid, std::slice::from_ref(&t.id))
            .unwrap();
        st.with_write(|c| {
            c.execute(
                "INSERT INTO mentions (comment_id, public_id) SELECT id, ?1 FROM comments",
                rusqlite::params![safari.public_id],
            )?;
            Ok(())
        })
        .unwrap();
        let (old, new) = (
            format!("data/users/{}/", safari.public_id),
            format!("data/users/{}/", owner.public_id),
        );
        private_doc(&st, &aid, &format!("{old}profile"), "{\"pick\":3}");
        private_doc(&st, &aid, &format!("{old}prefs"), "{\"theme\":\"safari\"}");
        private_doc(&st, &aid, &format!("{new}prefs"), "{\"theme\":\"owner\"}");
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Merged(safari.public_id.clone())
        );
        assert_eq!(st.get_viewer(&cookie).unwrap(), None, "the row is gone");
        let now = st.owner().unwrap().unwrap();
        assert_eq!(now.public_id, owner.public_id);
        assert_eq!(
            now.display_name.as_deref(),
            Some("Alex S"),
            "an unnamed owner takes the name"
        );
        assert_eq!(st.seen(&now.id, &aid).unwrap(), Some(1));
        let (author, resolved_by, mention, looked): (String, String, String, i64) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT author_public_id FROM comments), (SELECT resolved_by FROM threads),
                            (SELECT public_id FROM mentions),
                            (SELECT COUNT(*) FROM viewer_threads WHERE viewer_id = ?1)",
                    rusqlite::params![now.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .unwrap();
        assert_eq!(author, owner.public_id);
        assert_eq!(resolved_by, format!("viewer:{}", owner.public_id));
        assert_eq!(mention, owner.public_id);
        assert_eq!(looked, 1);
        // Private documents move; on a conflict the owner's stays and the old one is left.
        assert_eq!(
            doc_json(&st, &aid, &format!("{new}profile")).as_deref(),
            Some("{\"pick\":3}")
        );
        assert_eq!(doc_json(&st, &aid, &format!("{old}profile")), None);
        assert_eq!(
            doc_json(&st, &aid, &format!("{new}prefs")).as_deref(),
            Some("{\"theme\":\"owner\"}")
        );
        assert_eq!(
            doc_json(&st, &aid, &format!("{old}prefs")).as_deref(),
            Some("{\"theme\":\"safari\"}")
        );
        // The merged viewer's public ID resolves to the owner's.
        assert_eq!(
            claims(&st),
            vec![(
                owner.public_id.clone(),
                safari.public_id.clone(),
                owner.public_id.clone()
            )]
        );
        // Only the document that moved is recorded, as the owner's.
        let moves = doc_moves(&st);
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].0["public_id"], owner.public_id.as_str());
        assert_eq!(
            (&moves[0].2["from"], &moves[0].2["to"]),
            (
                &serde_json::json!(format!("{old}profile")),
                &serde_json::json!(format!("{new}profile"))
            )
        );
        // A named owner keeps its name.
        let laptop = new_ulid();
        st.mint_viewer(&laptop, true).unwrap();
        st.upsert_viewer(&laptop, Some("Other")).unwrap();
        st.claim_for_owner(Via::Shell, &laptop).unwrap();
        assert_eq!(
            st.owner().unwrap().unwrap().display_name.as_deref(),
            Some("Alex S")
        );
    }

    /// Every table with a `viewer_id` column, each of which `fold` handles.
    /// A new one fails `every_table_holding_a_viewer_row_is_folded` until
    /// `fold` handles it and it is listed here.
    const FOLDED_TABLES: &[&str] = &[
        "artifact_shares",
        "browser_sessions",
        "device_codes",
        "github_identities",
        "grants",
        "hosted_setup",
        "invites",
        "legal_agreements",
        "signin_links",
        "viewer_cookies",
        "viewer_seen",
        "viewer_threads",
    ];

    #[test]
    fn every_table_holding_a_viewer_row_is_folded() {
        let (_d, st) = store();
        let tables: Vec<String> = st
            .with_read(|c| {
                let mut stmt = c.prepare(
                    "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) p
                      WHERE m.type = 'table' AND p.name = 'viewer_id' ORDER BY m.name",
                )?;
                Ok(stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(tables, FOLDED_TABLES);
    }

    #[test]
    fn a_merged_viewers_hosted_rows_move_or_go_and_none_is_left() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let other = artifact(&st, None);
        let owner = st.owner_viewer(true).unwrap();
        let cookie = new_ulid();
        let safari = st.mint_viewer(&cookie, true).unwrap();
        st.with_write(|c| {
            c.execute_batch(&format!(
                "INSERT INTO browser_sessions (sha256, viewer_id, created_at, expires_at)
                     VALUES ('s1', '{f}', 'x', 'y');
                 INSERT INTO signin_links (sha256, viewer_id, created_at, expires_at)
                     VALUES ('l1', '{f}', 'x', 'y');
                 INSERT INTO github_identities (github_id, viewer_id, login, created_at, signed_in_at)
                     VALUES (5, '{f}', 'safari', 'x', 'x');
                 INSERT INTO legal_agreements (viewer_id, version, agreed_at, via)
                     VALUES ('{f}', 'v1', 'safari', 'prompt'), ('{f}', 'v2', 'safari', 'prompt'),
                            ('{o}', 'v1', 'owner', 'signin');
                 INSERT INTO device_codes (device_sha256, user_sha256, client_name, peer_ip,
                         status, viewer_id, created_at, expires_at)
                     VALUES ('d1', 'u1', 'laptop', '127.0.0.1', 'denied', '{f}', 'x', 'y');
                 INSERT INTO artifact_shares (id, artifact_id, viewer_id, access, created_at)
                     VALUES ('sh1', '{a}', '{f}', 'editor', 'x');
                 INSERT INTO artifact_shares (id, artifact_id, github_login, access, created_at, created_by)
                     VALUES ('sh2', '{b}', 'mia', 'viewer', 'x', '{fp}');",
                f = safari.id,
                o = owner.id,
                a = aid.as_str(),
                b = other.as_str(),
                fp = safari.public_id,
            ))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Merged(safari.public_id.clone())
        );
        let count = |sql: &str, id: &str| -> i64 {
            st.with_read(|c| Ok(c.query_row(sql, rusqlite::params![id], |r| r.get(0))?))
                .unwrap()
        };
        for table in FOLDED_TABLES {
            assert_eq!(
                count(
                    &format!("SELECT COUNT(*) FROM {table} WHERE viewer_id = ?1"),
                    &safari.id
                ),
                0,
                "{table}"
            );
        }
        // Another browser's credentials do not become the owner's.
        for table in ["browser_sessions", "signin_links", "github_identities"] {
            assert_eq!(
                count(
                    &format!("SELECT COUNT(*) FROM {table} WHERE viewer_id = ?1"),
                    &owner.id
                ),
                0,
                "{table}"
            );
        }
        let text = |sql: &str, id: &str| -> Option<String> {
            st.with_read(|c| Ok(c.query_row(sql, rusqlite::params![id], |r| r.get(0))?))
                .unwrap()
        };
        assert_eq!(
            text(
                "SELECT group_concat(version || ':' || agreed_at, ' ') FROM
                   (SELECT * FROM legal_agreements WHERE viewer_id = ?1 ORDER BY version)",
                &owner.id
            )
            .as_deref(),
            Some("v1:owner v2:safari"),
            "agreements move, the owner's own kept"
        );
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM device_codes WHERE viewer_id = ?1",
                &owner.id
            ),
            1
        );
        // The owner holds no share; one made by the folded viewer is now
        // the owner's.
        let shares: Vec<_> = [aid.as_str(), other.as_str()]
            .iter()
            .flat_map(|a| st.artifact_shares(a).unwrap())
            .map(|s| (s.id, s.person, s.created_by))
            .collect();
        assert_eq!(
            shares,
            vec![("sh2".to_string(), None, Some(owner.public_id.clone()))]
        );
    }

    #[test]
    fn a_fold_that_leaves_a_row_naming_the_folded_viewer_is_refused_whole() {
        let (_d, st) = store();
        let cli = st.set_owner_name("Alex", false).unwrap();
        st.with_write(|c| {
            c.execute_batch(&format!(
                "CREATE TABLE extra (viewer_id TEXT);
                 INSERT INTO extra (viewer_id) VALUES ('{}');",
                cli.id
            ))?;
            Ok(())
        })
        .unwrap();
        let cookie = new_ulid();
        let chrome = st.mint_viewer(&cookie, true).unwrap();
        match st.claim_for_owner(Via::Shell, &cookie) {
            Err(CoreError::Invalid { code, .. }) => assert_eq!(code, "fold_incomplete"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            st.owner().unwrap().map(|o| o.public_id),
            Some(cli.public_id.clone()),
            "the owner row and its flag are unchanged"
        );
        let still = st
            .get_viewer(&cookie)
            .unwrap()
            .expect("the cookie still names its viewer");
        assert_eq!(still.public_id, chrome.public_id);
        let owner_flag: bool = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT owner FROM viewers WHERE public_id = ?1",
                    rusqlite::params![chrome.public_id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(!owner_flag, "the browser's viewer did not become the owner");
        assert!(claims(&st).is_empty(), "no claim is recorded");
    }

    #[test]
    fn folding_the_owners_row_makes_its_successor_the_owner() {
        let (_d, st) = store();
        let cli = st.set_owner_name("Alex", false).unwrap();
        let chrome = st.mint_viewer(&new_ulid(), true).unwrap();
        st.with_tx(|tx| super::fold(&st, DAEMON, tx, &cli, &chrome))
            .unwrap();
        assert_eq!(
            st.owner().unwrap().map(|o| o.public_id),
            Some(chrome.public_id)
        );
    }

    #[test]
    fn the_owner_rows_credentials_follow_it_when_a_browser_adopts_it() {
        let (_d, st) = store();
        let cli = st.set_owner_name("Alex", false).unwrap();
        st.with_write(|c| {
            c.execute_batch(&format!(
                "INSERT INTO browser_sessions (sha256, viewer_id, created_at, expires_at)
                     VALUES ('s1', '{f}', 'x', 'y');
                 INSERT INTO hosted_setup (id, viewer_id, created_at) VALUES (1, '{f}', 'x');
                 INSERT INTO github_identities (github_id, viewer_id, login, created_at, signed_in_at)
                     VALUES (5, '{f}', 'alex', 'x', 'x');",
                f = cli.id,
            ))?;
            Ok(())
        })
        .unwrap();
        let cookie = new_ulid();
        st.mint_viewer(&cookie, true).unwrap();
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Adopted {
                retired: Some(cli.public_id.clone())
            }
        );
        let owner = st.owner().unwrap().unwrap();
        for table in ["browser_sessions", "hosted_setup", "github_identities"] {
            let n: i64 = st
                .with_read(|c| {
                    Ok(c.query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE viewer_id = ?1"),
                        rusqlite::params![owner.id],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn upsert_creates_renames_and_clears() {
        let (_d, st) = store();
        let id = new_ulid();
        let v = st.upsert_viewer(&id, None).unwrap();
        assert_eq!(v.display_name, None);
        assert_eq!(
            st.upsert_viewer(&id, Some("  Alex  "))
                .unwrap()
                .display_name
                .as_deref(),
            Some("Alex")
        );
        assert_eq!(
            st.upsert_viewer(&id, None).unwrap().display_name.as_deref(),
            Some("Alex"),
            "None keeps the name"
        );
        assert_eq!(
            st.upsert_viewer(&id, Some("")).unwrap().display_name,
            None,
            "empty clears"
        );
        assert_eq!(
            st.get_viewer(&id).unwrap().unwrap().created_at,
            v.created_at
        );
    }

    #[test]
    fn a_viewer_has_a_stable_public_id_distinct_from_its_cookie() {
        let (_d, st) = store();
        let id = new_ulid();
        let v = st.upsert_viewer(&id, None).unwrap();
        assert!(clax_core::is_public_id(&v.public_id), "{}", v.public_id);
        assert_ne!(v.public_id, id);
        assert_eq!(
            st.upsert_viewer(&id, Some("Alex")).unwrap().public_id,
            v.public_id
        );
        let other = st.upsert_viewer(&new_ulid(), None).unwrap();
        assert_ne!(other.public_id, v.public_id);
        let found = st.viewer_by_public_id(&v.public_id).unwrap().unwrap();
        assert_eq!(
            (found.id.as_str(), found.display_name.as_deref()),
            (v.id.as_str(), Some("Alex"))
        );
        assert_eq!(
            st.viewer_by_public_id("u_0123456789abcdef012345").unwrap(),
            None
        );
        assert_eq!(
            st.viewer_by_public_id(&id).unwrap(),
            None,
            "the cookie is not a public ID"
        );
        let json = serde_json::to_value(&v).unwrap();
        assert!(
            !json.to_string().contains(&id),
            "a serialised viewer never carries its cookie: {json}"
        );
        assert_eq!(json["public_id"], v.public_id.as_str());
    }

    #[test]
    fn bad_ids_and_names_are_refused() {
        let (_d, st) = store();
        assert!(matches!(
            st.upsert_viewer("not-a-ulid", None),
            Err(CoreError::Invalid {
                code: "invalid_viewer",
                ..
            })
        ));
        let id = new_ulid();
        for bad in ["a\nb".to_string(), "x".repeat(MAX_NAME_CHARS + 1)] {
            assert!(matches!(
                st.upsert_viewer(&id, Some(&bad)),
                Err(CoreError::Invalid {
                    code: "invalid_name",
                    ..
                })
            ));
        }
    }

    #[test]
    fn lookups_by_public_id_and_name_search() {
        let (_d, st) = store();
        let alex = st.upsert_viewer(&new_ulid(), Some("Alex Chen")).unwrap();
        let sam = st.upsert_viewer(&new_ulid(), Some("Sam")).unwrap();
        let anon = st.upsert_viewer(&new_ulid(), None).unwrap();
        let found: Vec<_> = st
            .viewers_by_public_ids(&[
                anon.public_id.clone(),
                "u_ffffffffffffffffffffff".into(),
                alex.public_id.clone(),
                "not a public ID".into(),
            ])
            .unwrap()
            .into_iter()
            .map(|v| v.public_id)
            .collect();
        assert_eq!(
            found,
            [anon.public_id.clone(), alex.public_id.clone()],
            "in the order asked, unknown and malformed IDs skipped"
        );
        let names: Vec<_> = st
            .search_viewers("A", 8)
            .unwrap()
            .into_iter()
            .map(|v| v.display_name.unwrap())
            .collect();
        assert_eq!(
            names,
            ["Alex Chen", "Sam"],
            "case-insensitive substring, by name, named only"
        );
        assert_eq!(
            st.search_viewers("chen", 8).unwrap()[0].public_id,
            alex.public_id
        );
        assert!(st.search_viewers("zz", 8).unwrap().is_empty());
        assert_eq!(st.search_viewers("a", 1).unwrap().len(), 1);
        let _ = sam;
    }

    #[test]
    fn search_folds_case_beyond_ascii_and_matches_wildcards_literally() {
        let (_d, st) = store();
        let umlaut = st.upsert_viewer(&new_ulid(), Some("Ärger")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("Arno")).unwrap();
        let pct = st.upsert_viewer(&new_ulid(), Some("100% sure")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("100 x sure")).unwrap();
        let under = st.upsert_viewer(&new_ulid(), Some("a_b")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("axb")).unwrap();
        let ids = |q: &str| -> Vec<String> {
            st.search_viewers(q, 8)
                .unwrap()
                .into_iter()
                .map(|v| v.public_id)
                .collect()
        };
        assert_eq!(ids("ärger"), std::slice::from_ref(&umlaut.public_id));
        assert_eq!(ids("ÄRGER"), std::slice::from_ref(&umlaut.public_id));
        assert_eq!(ids("0%"), std::slice::from_ref(&pct.public_id));
        assert_eq!(ids("a_b"), std::slice::from_ref(&under.public_id));
        assert!(ids("%").len() == 1 && ids("_").len() == 1);
    }
    #[test]
    fn search_scans_at_most_max_search_scan_names() {
        assert_eq!(MAX_SEARCH_SCAN, 10_000);
        let (_d, st) = store();
        // MAX_SEARCH_SCAN names sorting before one more, "zed", which lies past the scan.
        st.with_write(|c| {
            let tx = c.unchecked_transaction()?;
            {
                let mut ins = tx.prepare(
                    "INSERT INTO viewers (id, public_id, display_name, created_at)
                     VALUES (?1, 'u_' || lower(hex(randomblob(11))), ?2, '2026-01-01T00:00:00Z')",
                )?;
                for i in 0..=MAX_SEARCH_SCAN {
                    let name = if i == MAX_SEARCH_SCAN {
                        "zed".to_string()
                    } else {
                        format!("a{i:05}")
                    };
                    ins.execute(rusqlite::params![format!("v{i}"), name])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            st.search_viewers("a09999", 8).unwrap().len(),
            1,
            "the last scanned name is found"
        );
        assert!(
            st.search_viewers("zed", 8).unwrap().is_empty(),
            "a name past the scan is not reached"
        );
    }
}

//! Live pages (spec 2026-10-05-chrome-overlay-design §5): artifacts of kind
//! `live`, keyed by origin and path, whose versions are snapshots.

use super::Store;
use super::threads::NewThread;
use super::watches::{Cause, Wrote, page_watch_records};
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::live::{KIND_LIVE, PageKey, placeholder_html};
use crate::model::{Artifact, CONTRACT_VERSION, Thread, Version};
use crate::publish::{Encoding, FileInput, INDEX, PublishRequest, ValidatedPublish};
use crate::{ArtifactId, CoreError, Result};
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// A live page's key and its artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LivePage {
    pub artifact_id: String,
    pub origin: String,
    pub path: String,
}

/// What [`Store::ensure_live_page`] found or made.
#[derive(Clone, Debug)]
pub struct EnsuredPage {
    pub artifact: Artifact,
    /// The origin the page is kept under: its site's key (spec §7.2).
    pub origin: String,
    /// The version a thread made now belongs on.
    pub version: Version,
    /// The page did not exist before this call.
    pub created: bool,
    /// This call wrote `version`.
    pub new_version: bool,
    /// The threads whose pending addresses were linked to `version` in the
    /// transaction that wrote it ([`Store::ensure_live_page_linking`]).
    pub linked: Vec<String>,
}

/// How long a pick ID keeps naming the thread it made
/// ([`Store::picked_thread`]): far longer than any retry of the request.
pub const PICK_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// The oldest `created_at` of a pick still kept.
fn pick_cutoff() -> String {
    let ttl = chrono::Duration::from_std(PICK_TTL).expect("the TTL fits");
    (chrono::Utc::now() - ttl).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// How many times [`Store::store_snapshot`] tries to write a version while
/// other snapshots of the page take the version number it read.
pub const SNAPSHOT_ATTEMPTS: u32 = 32;

fn row_to_page(r: &Row<'_>) -> rusqlite::Result<LivePage> {
    Ok(LivePage {
        artifact_id: r.get(0)?,
        origin: r.get(1)?,
        path: r.get(2)?,
    })
}

/// The live page `key` names: the page of its path on its origin's site
/// (spec §7.2: a joined origin's pages are its site's).
fn page_by_key(c: &Connection, key: &PageKey) -> Result<Option<LivePage>> {
    Ok(c.query_row(
        "SELECT p.artifact_id, p.origin, p.path FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
         WHERE p.origin = COALESCE((SELECT site FROM live_sites WHERE origin = ?1), ?1)
            AND p.path = ?2 AND a.deleted_at IS NULL",
        params![key.origin, key.path],
        row_to_page,
    )
    .optional()?)
}

/// The publish of a snapshot: `html` as the only file of version
/// `expected + 1`, titled `title`, noted `snapshot`.
fn snapshot_publish(expected: u32, title: &str, html: &[u8]) -> Result<ValidatedPublish> {
    snapshot_publish_noted(expected, title, html, "snapshot")
}

/// [`snapshot_publish`], noted `note`.
pub(super) fn snapshot_publish_noted(
    expected: u32,
    title: &str,
    html: &[u8],
    note: &str,
) -> Result<ValidatedPublish> {
    crate::publish::validate(PublishRequest {
        title: Some(title.to_string()),
        note: Some(note.to_string()),
        if_version: Some(expected),
        files: BTreeMap::from([(
            INDEX.to_string(),
            Some(FileInput {
                content: base64::engine::general_purpose::STANDARD.encode(html),
                encoding: Encoding::Base64,
                content_type: None,
            }),
        )]),
        ..Default::default()
    })
}

/// [`Store::mark_pending`] inside a transaction.
pub(crate) fn mark_pending_in(
    tx: &Transaction<'_>,
    id: &ArtifactId,
    tid: &str,
    source: &str,
    harness: &str,
) -> Result<bool> {
    if source == "resolve" {
        let linked: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM version_threads WHERE thread_id = ?1)
                OR EXISTS(SELECT 1 FROM live_pending WHERE thread_id = ?1)",
            params![tid],
            |r| r.get(0),
        )?;
        if linked {
            return Ok(false);
        }
    }
    tx.execute(
        "INSERT INTO live_pending (artifact_id, thread_id, source, harness, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(artifact_id, thread_id) DO UPDATE SET source = excluded.source,
            harness = excluded.harness, created_at = excluded.created_at",
        params![id.as_str(), tid, source, harness, Store::now()],
    )?;
    Ok(true)
}

/// Whether any thread of `tids` has a pending address on the live page `id`.
fn any_pending(c: &Connection, id: &ArtifactId, tids: &[String]) -> Result<bool> {
    let arr = serde_json::to_string(tids).expect("strings serialise");
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM json_each(?2) j
            JOIN live_pending lp ON lp.thread_id = j.value WHERE lp.artifact_id = ?1)",
        params![id.as_str(), arr],
        |r| r.get(0),
    )?)
}

/// Links the pending addresses of the live page `id` (only those of the
/// threads `only` names, when given) to its version `n`, each with its
/// source, and deletes them; returns the linked thread IDs, oldest address
/// first.
fn link_pending_in(
    tx: &Transaction<'_>,
    id: &ArtifactId,
    n: u32,
    only: Option<&[String]>,
) -> Result<Vec<String>> {
    let only = only.map(|t| serde_json::to_string(t).expect("strings serialise"));
    let ids: Vec<String> = {
        let mut st = tx.prepare(
            "SELECT thread_id FROM live_pending
             WHERE artifact_id = ?1 AND (?2 IS NULL OR thread_id IN (SELECT value FROM json_each(?2)))
             ORDER BY created_at, thread_id",
        )?;
        st.query_map(params![id.as_str(), only], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let now = Store::now();
    for tid in &ids {
        tx.execute(
            "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
             SELECT artifact_id, ?2, thread_id, source, ?3 FROM live_pending
             WHERE artifact_id = ?1 AND thread_id = ?4",
            params![id.as_str(), n, now, tid],
        )?;
        tx.execute(
            "DELETE FROM live_pending WHERE artifact_id = ?1 AND thread_id = ?2",
            params![id.as_str(), tid],
        )?;
    }
    Ok(ids)
}

/// A session's scope watch (spec L2): it covers the live pages of `origin`
/// whose path is `path` or below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveWatch {
    pub session_id: String,
    pub origin: String,
    pub path: String,
    pub replies_armed: bool,
    pub created_at: String,
}

/// Makes `sid` a watcher of `aid` through a scope watch with `armed`
/// arming; a direct watch keeps its own row and arming. What it wrote: a
/// new watch, a scope watch's new arming, or nothing.
fn scope_row(tx: &Connection, sid: &str, aid: &str, armed: bool) -> Result<Wrote> {
    let before: Option<(bool, String)> = tx
        .prepare_cached(
            "SELECT replies_armed, source FROM watches WHERE session_id = ?1 AND artifact_id = ?2",
        )?
        .query_row(params![sid, aid], |r| {
            Ok((r.get::<_, i64>(0)? != 0, r.get(1)?))
        })
        .optional()?;
    let wrote = match before {
        None => Wrote::Inserted,
        Some((a, source)) if source == "scope" && a != armed => Wrote::Rearmed,
        Some(_) => return Ok(Wrote::Unchanged),
    };
    tx.prepare_cached(
        "INSERT INTO watches (session_id, artifact_id, replies_armed, created_at, source)
         VALUES (?1, ?2, ?3, ?4, 'scope') ON CONFLICT(session_id, artifact_id)
         DO UPDATE SET replies_armed = excluded.replies_armed WHERE watches.source = 'scope'",
    )?
    .execute(params![sid, aid, armed, Store::now()])?;
    Ok(wrote)
}

/// The live pages of the site of origin `?1` whose artifact is not deleted
/// (the pages its site's key holds: spec §7.2).
pub(crate) const PAGES_OF_ORIGIN: &str =
    "SELECT p.artifact_id, p.origin, p.path FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
    WHERE p.origin = COALESCE((SELECT site FROM live_sites WHERE origin = ?1), ?1)
        AND a.deleted_at IS NULL";

/// The live pages of `origin`'s site whose artifact is not deleted.
pub(super) fn pages_of(tx: &Connection, origin: &str) -> Result<Vec<LivePage>> {
    let mut st = tx.prepare_cached(PAGES_OF_ORIGIN)?;
    let rows = st
        .query_map(params![origin], row_to_page)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The live page whose artifact is `aid`, on a given connection.
pub(super) fn live_page_of_conn(c: &Connection, aid: &str) -> Result<Option<LivePage>> {
    Ok(
        c.prepare_cached(
            "SELECT artifact_id, origin, path FROM live_pages WHERE artifact_id = ?1",
        )?
        .query_row(params![aid], row_to_page)
        .optional()?,
    )
}

/// The scope watches of live sessions on any origin of the site whose key
/// is `?1` (spec §7.2: a watch on one of a joined site's origins covers
/// them all), oldest first: session, path and arming. `CROSS JOIN` keeps
/// `live_watches` outer, so the origin index bounds the work however many
/// sessions have ended.
pub(crate) const SCOPES_OF_ORIGIN: &str = "SELECT lw.session_id, lw.path, lw.replies_armed
    FROM (SELECT origin FROM live_sites WHERE site = ?1 UNION SELECT ?1) o
    CROSS JOIN live_watches lw ON lw.origin = o.origin
    CROSS JOIN sessions s ON s.id = lw.session_id
    WHERE s.ended_at IS NULL ORDER BY lw.created_at, lw.session_id, lw.path";
/// The pending address of thread `?1`: harness and time.
pub(crate) const PENDING_OF: &str =
    "SELECT harness, created_at FROM live_pending WHERE thread_id = ?1";

/// Makes every live session whose scope watch covers `key` a watcher of the
/// new page `aid`, armed when any of its covering scopes is. Returns what
/// it wrote for each covering session, as `(session, arming, wrote)`.
pub(super) fn materialize(
    tx: &Connection,
    aid: &str,
    key: &PageKey,
) -> Result<Vec<(String, bool, Wrote)>> {
    let rows: Vec<(String, String, bool)> = {
        let mut st = tx.prepare(SCOPES_OF_ORIGIN)?;
        st.query_map(params![key.origin], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    // Each covering session once, in order, armed when any of its covering
    // scopes is.
    let mut out: Vec<(String, bool)> = Vec::new();
    for (sid, path, armed) in rows {
        let scope = PageKey {
            origin: key.origin.clone(),
            path,
        };
        if !key.covered_by(&scope) {
            continue;
        }
        match out.iter_mut().find(|(s, _)| *s == sid) {
            Some((_, a)) => *a |= armed,
            None => out.push((sid, armed)),
        }
    }
    let mut written = Vec::new();
    for (sid, armed) in out {
        let wrote = scope_row(tx, &sid, aid, armed)?;
        written.push((sid, armed, wrote));
    }
    Ok(written)
}

/// The paths of the threads of live page `?1` made at another path than
/// the page's (a merge rule's canonical page).
pub(crate) const THREAD_PATHS_OF_PAGE: &str = "SELECT DISTINCT live_path FROM threads
    WHERE artifact_id = ?1 AND live_path IS NOT NULL";

/// The keys a scope watch may cover the live page `p` by: its own, and
/// the path of each of its threads made at another path (spec 2026-10-05
/// §7.1: a scope on `/users/1` covers the canonical page `/users/:id` once
/// it holds a thread made at `/users/1`).
fn page_keys(tx: &Connection, p: &LivePage) -> Result<Vec<PageKey>> {
    let mut keys = vec![PageKey {
        origin: p.origin.clone(),
        path: p.path.clone(),
    }];
    let mut st = tx.prepare_cached(THREAD_PATHS_OF_PAGE)?;
    for path in st.query_map(params![p.artifact_id], |r| r.get::<_, String>(0))? {
        keys.push(PageKey {
            origin: p.origin.clone(),
            path: path?,
        });
    }
    Ok(keys)
}

/// Makes every live session whose scope watch covers the live page `p`
/// (by its own path or a path of a thread on it) a watcher of it: what a
/// join does for each page of the site (spec §7.2), as the scopes of every
/// origin of the site now cover it.
pub(super) fn rematerialize(tx: &Connection, p: &LivePage) -> Result<()> {
    let keys = page_keys(tx, p)?;
    let rows: Vec<(String, String, bool)> = {
        let mut st = tx.prepare_cached(SCOPES_OF_ORIGIN)?;
        st.query_map(params![p.origin], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut out: Vec<(String, bool)> = Vec::new();
    for (sid, path, armed) in rows {
        let scope = PageKey {
            origin: p.origin.clone(),
            path,
        };
        if !keys.iter().any(|k| k.covered_by(&scope)) {
            continue;
        }
        match out.iter_mut().find(|(s, _)| *s == sid) {
            Some((_, a)) => *a |= armed,
            None => out.push((sid, armed)),
        }
    }
    for (sid, armed) in &out {
        scope_row(tx, sid, &p.artifact_id, *armed)?;
    }
    Ok(())
}

/// Whether any scope watch of `sid` covering any of `keys` (one page's,
/// keyed by its site's key) has replies armed, or `None` when none covers
/// them. A scope on any origin of the site counts.
fn scope_arming(tx: &Connection, sid: &str, keys: &[PageKey]) -> Result<Option<bool>> {
    let Some(origin) = keys.first().map(|k| k.origin.clone()) else {
        return Ok(None);
    };
    let mut st = tx.prepare(
        "SELECT path, replies_armed FROM live_watches WHERE session_id = ?1
            AND origin IN (SELECT origin FROM live_sites WHERE site = ?2 UNION SELECT ?2)",
    )?;
    let rows: Vec<(String, bool)> = st
        .query_map(params![sid, origin], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter(|(path, _)| {
            let scope = PageKey {
                origin: origin.clone(),
                path: path.clone(),
            };
            keys.iter().any(|k| k.covered_by(&scope))
        })
        .map(|(_, armed)| armed)
        .reduce(|a, b| a || b))
}

impl Store {
    /// The live page `key` names, if it exists (including one whose first
    /// version is still being written).
    pub fn find_live_page(&self, key: &PageKey) -> Result<Option<LivePage>> {
        self.with_read(|c| page_by_key(c, key))
    }

    /// The live page whose artifact is `id`, or `None` for any other artifact.
    pub fn live_page_of(&self, id: &ArtifactId) -> Result<Option<LivePage>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT artifact_id, origin, path FROM live_pages WHERE artifact_id = ?1",
                params![id.as_str()],
                row_to_page,
            )
            .optional()?)
        })
    }

    /// Every live page whose artifact is not deleted.
    pub fn live_pages(&self) -> Result<Vec<LivePage>> {
        self.with_read(|c| {
            let mut st = c.prepare(
                "SELECT p.artifact_id, p.origin, p.path FROM live_pages p
                 JOIN artifacts a ON a.id = p.artifact_id WHERE a.deleted_at IS NULL",
            )?;
            let pages = st
                .query_map([], row_to_page)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(pages)
        })
    }

    /// The artifact IDs of every live page that is not deleted.
    pub fn live_page_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .live_pages()?
            .into_iter()
            .map(|p| p.artifact_id)
            .collect())
    }

    /// Finds or creates the live page `key` and gives it the version a new
    /// thread belongs on: a new page's version 1 is `snapshot`, or the
    /// placeholder without one; an existing page takes `snapshot` as a new
    /// version when it differs from the current one ([`Store::store_snapshot`]).
    /// Concurrent first calls for one key settle on one page. Under `ctx`,
    /// making the page records `artifact.create` and `live.page` in its
    /// transaction, and each version written records `live.snapshot` in its
    /// own.
    pub fn ensure_live_page(
        &self,
        ctx: &AuditCtx,
        key: &PageKey,
        title: &str,
        snapshot: Option<&[u8]>,
    ) -> Result<EnsuredPage> {
        self.ensure_live_page_linking(ctx, key, title, snapshot, &[])
    }

    /// [`Store::ensure_live_page`] that, when it writes a version, links to
    /// it the threads of `pending` still pending on the page, in the
    /// version's transaction (spec L11, as ruled 2026-10-05: a snapshot names
    /// the pending threads it covers); other pending addresses stay pending.
    pub fn ensure_live_page_linking(
        &self,
        ctx: &AuditCtx,
        key: &PageKey,
        title: &str,
        snapshot: Option<&[u8]>,
        pending: &[String],
    ) -> Result<EnsuredPage> {
        let (id, created, origin) = self.with_tx(|tx| {
            if let Some(p) = page_by_key(tx, key)? {
                return Ok((ArtifactId::parse(&p.artifact_id)?, false, p.origin));
            }
            // A joined origin's new page is its site's (spec §7.2).
            let key = &PageKey {
                origin: super::joined::site_key(tx, &key.origin)?,
                path: key.path.clone(),
            };
            let id = ArtifactId::generate();
            let now = Store::now();
            tx.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, pinned,
                    capabilities_json, contract_version, kind)
                 VALUES (?1, ?2, ?3, ?3, 0, 0, '{}', ?4, 'live')",
                params![id.as_str(), title, now, CONTRACT_VERSION],
            )?;
            tx.execute(
                "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id.as_str(), key.origin, key.path, now],
            )?;
            let watching = materialize(tx, id.as_str(), key)?;
            let a = tx.query_row(
                &format!("{} WHERE id = ?1", super::artifacts::SELECT),
                params![id.as_str()],
                super::artifacts::row_to_artifact,
            )??;
            self.record_audit(tx, ctx, super::artifacts::create_record(&a, &now))?;
            let mut rec = AuditRecord::new(AuditKind::LivePage, now)
                .with("origin", key.origin.as_str())
                .with("path", key.path.as_str());
            rec.ids.artifact = Some(id.as_str().to_string());
            rec.ids.origin = Some(key.origin.clone());
            self.record_audit(tx, ctx, rec)?;
            self.record_page_watches(
                tx,
                ctx,
                id.as_str(),
                &watching,
                "scope",
                Some(Cause::Scope),
            )?;
            Ok((id, true, key.origin.clone()))
        })?;
        let current: u32 = self.with_read(|c| {
            Ok(c.query_row(
                "SELECT current_version FROM artifacts WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )?)
        })?;
        let unchanged = |n: u32| -> Result<(Version, bool, Vec<String>)> {
            Ok((
                self.get_version(&id, n)?.ok_or(CoreError::NotFound)?,
                false,
                Vec::new(),
            ))
        };
        let (version, new_version, linked) = if current == 0 {
            let html = snapshot
                .map(<[u8]>::to_vec)
                .unwrap_or_else(|| placeholder_html(key).into_bytes());
            match self.write_snapshot(ctx, &id, 0, title, &html, pending) {
                Ok((v, linked)) => (v, true, linked),
                // Another first call wrote version 1 meanwhile: build on it.
                Err(CoreError::Conflict { .. }) => match snapshot {
                    Some(s) => self.snapshot_linking(ctx, &id, title, s, false, pending)?,
                    None => unchanged(1)?,
                },
                Err(e) => return Err(e),
            }
        } else if let Some(s) = snapshot {
            self.snapshot_linking(ctx, &id, title, s, false, pending)?
        } else {
            unchanged(current)?
        };
        let artifact = self.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        Ok(EnsuredPage {
            artifact,
            origin,
            version,
            created,
            new_version,
            linked,
        })
    }

    /// Stores `html` as the next version of the live page `id`, titled
    /// `title`, unless it is byte-identical to the current version's
    /// `index.html` and `force` is false; then the current version is
    /// returned. The flag says whether a version was written. When another
    /// snapshot of the page lands first, this one is compared with it and
    /// written after it, so concurrent snapshots each get a version. A
    /// version written is recorded as `live.snapshot` under `ctx`.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `not_live` for an
    /// artifact that is not a live page; `Conflict` only when other
    /// snapshots keep landing first [`SNAPSHOT_ATTEMPTS`] times in a row.
    pub fn store_snapshot(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        title: &str,
        html: &[u8],
        force: bool,
    ) -> Result<(Version, bool)> {
        self.snapshot_linking(ctx, id, title, html, force, &[])
            .map(|(v, new, _)| (v, new))
    }

    /// [`Store::store_snapshot`], linking to a version it writes the threads
    /// of `pending` still pending on the page, in the version's transaction;
    /// also returns the linked thread IDs.
    fn snapshot_linking(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        title: &str,
        html: &[u8],
        force: bool,
        pending: &[String],
    ) -> Result<(Version, bool, Vec<String>)> {
        let mut attempt = 1;
        loop {
            let a = self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
            if a.kind != KIND_LIVE {
                return Err(CoreError::invalid(
                    "not_live",
                    format!("{id} is not a live page"),
                ));
            }
            if !force {
                let current =
                    std::fs::read(self.home.version_dir(id, a.current_version).join(INDEX)).ok();
                if current.as_deref() == Some(html) {
                    return Ok((
                        self.get_version(id, a.current_version)?
                            .ok_or(CoreError::NotFound)?,
                        false,
                        Vec::new(),
                    ));
                }
            }
            match self.write_snapshot(ctx, id, a.current_version, title, html, pending) {
                Ok((v, linked)) => return Ok((v, true, linked)),
                // Another snapshot took this version number: build on it.
                Err(CoreError::Conflict { .. }) if attempt < SNAPSHOT_ATTEMPTS => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// Records that the agent of `harness` addressed thread `tid` of the
    /// live page `id`, to be linked to the page's next snapshot (spec L11).
    /// `source` is `explicit` (a reply with `addressed`) or `resolve` (an
    /// agent resolve). A `resolve` is recorded only when the thread has no
    /// version link and no pending address yet; an `explicit` one replaces a
    /// pending `resolve`. Returns whether a row was written.
    ///
    /// # Errors
    /// Database errors only.
    pub fn mark_pending(
        &self,
        id: &ArtifactId,
        tid: &str,
        source: &str,
        harness: &str,
    ) -> Result<bool> {
        self.with_tx(|tx| mark_pending_in(tx, id, tid, source, harness))
    }

    /// Links every pending address of the live page `id` to its version `n`
    /// (each with its `source`) and clears them; returns the linked thread
    /// IDs, oldest address first. Records nothing: production links inside
    /// a snapshot's transaction, and the snapshot lists them.
    #[cfg(test)]
    pub(crate) fn link_pending(&self, id: &ArtifactId, n: u32) -> Result<Vec<String>> {
        self.with_tx(|tx| link_pending_in(tx, id, n, None))
    }

    /// Stores `html` as a new version of the live page `id` (even when
    /// identical to the current one) and links to it the threads of
    /// `pending` that still have a pending address on the page (spec L11, as
    /// ruled 2026-10-05: the snapshot names the pending threads it covers).
    /// The check, the version and the links are one transaction. Returns the
    /// version and the linked thread IDs (oldest address first), or `None`,
    /// writing nothing, when none of `pending` is pending. Other pending
    /// addresses stay pending. The version is recorded as `live.snapshot`
    /// under `ctx`.
    ///
    /// # Errors
    /// As [`Store::store_snapshot`].
    pub fn snapshot_pending(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        title: &str,
        html: &[u8],
        pending: &[String],
    ) -> Result<Option<(Version, Vec<String>)>> {
        const NONE_PENDING: &str = "nothing_pending";
        if !self.with_read(|c| any_pending(c, id, pending))? {
            return Ok(None);
        }
        let mut attempt = 1;
        loop {
            let a = self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
            if a.kind != KIND_LIVE {
                return Err(CoreError::invalid(
                    "not_live",
                    format!("{id} is not a live page"),
                ));
            }
            let p = snapshot_publish(a.current_version, title, html)?;
            let written = self.write_version_then(
                ctx,
                id,
                a.current_version,
                &p,
                &BTreeMap::new(),
                None,
                |tx, n| {
                    let linked = link_pending_in(tx, id, n, Some(pending))?;
                    if linked.is_empty() {
                        // Rolls the version back: nothing named is pending now.
                        return Err(CoreError::invalid(NONE_PENDING, "nothing pending"));
                    }
                    Ok(linked)
                },
            );
            match written {
                Ok((_, v, linked)) => return Ok(Some((v, linked))),
                Err(CoreError::Invalid { code, .. }) if code == NONE_PENDING => return Ok(None),
                Err(CoreError::Conflict { .. }) if attempt < SNAPSHOT_ATTEMPTS => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// The pending address of thread `tid`: the addressing agent's harness
    /// and when it addressed the thread.
    ///
    /// # Errors
    /// Database errors only.
    pub fn pending_address(&self, tid: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| {
            Ok(
                c.query_row(PENDING_OF, params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?,
            )
        })
    }

    /// The thread pick `pick` made on the live page `key` within
    /// [`PICK_TTL`], when it still exists: the page's artifact ID and the
    /// thread ID.
    ///
    /// # Errors
    /// Database errors only.
    pub fn picked_thread(&self, key: &PageKey, pick: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT p.artifact_id, k.thread_id FROM live_pages p
                 JOIN artifacts a ON a.id = p.artifact_id
                 JOIN live_picks k ON k.artifact_id = p.artifact_id
                 JOIN threads t ON t.id = k.thread_id
                 WHERE p.origin = COALESCE((SELECT site FROM live_sites WHERE origin = ?1), ?1)
                    AND p.path = ?2 AND a.deleted_at IS NULL
                    AND k.pick_id = ?3 AND k.created_at >= ?4",
                params![key.origin, key.path, pick, pick_cutoff()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// [`Store::create_thread`] on the live page `id` for pick `pick`,
    /// recording in the same transaction that the pick made it (and
    /// forgetting picks older than [`PICK_TTL`], and this pick when its
    /// thread was deleted). `None`, with nothing
    /// written, when the pick already made a thread on the page: a repeat of
    /// the request that raced this one.
    ///
    /// # Errors
    /// As [`Store::create_thread`].
    pub fn create_picked_thread(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        pick: &str,
    ) -> Result<Option<Thread>> {
        self.create_live_thread(ctx, id, t, Some(pick), None)
    }

    /// [`Store::create_thread`] on the live page `id`, as
    /// [`Store::create_picked_thread`] when `pick` is given, recording in
    /// the same transaction `live_path`: the path of the URL the thread was
    /// made at, when a merge rule mapped it to this page (spec 2026-10-05
    /// §7.1). `None` only for a repeated pick.
    ///
    /// # Errors
    /// As [`Store::create_thread`].
    pub fn create_live_thread(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        t: NewThread,
        pick: Option<&str>,
        live_path: Option<&str>,
    ) -> Result<Option<Thread>> {
        let taken = std::cell::Cell::new(false);
        let made = self.create_thread_then(ctx, id, t, |tx, tid| {
            let mut after = Vec::new();
            if let Some(path) = live_path {
                tx.execute(
                    "UPDATE threads SET live_path = ?2 WHERE id = ?1",
                    params![tid, path],
                )?;
                // Scope watches covering the path now cover the page.
                if let Some(p) = live_page_of_conn(tx, id.as_str())? {
                    let key = PageKey {
                        origin: p.origin,
                        path: path.to_string(),
                    };
                    // Recorded after the thread that caused them.
                    let watching = materialize(tx, id.as_str(), &key)?;
                    after = page_watch_records(
                        tx,
                        id.as_str(),
                        &watching,
                        "scope",
                        Some(Cause::Scope),
                    )?;
                }
            }
            let set = live_path.map(str::to_string);
            let Some(pick) = pick else {
                return Ok((set, after));
            };
            tx.execute(
                "DELETE FROM live_picks WHERE created_at < ?1
                    OR (artifact_id = ?2 AND pick_id = ?3
                        AND NOT EXISTS(SELECT 1 FROM threads WHERE id = live_picks.thread_id))",
                params![pick_cutoff(), id.as_str(), pick],
            )?;
            let n = tx.execute(
                "INSERT OR IGNORE INTO live_picks (artifact_id, pick_id, thread_id, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id.as_str(), pick, tid, Store::now()],
            )?;
            if n == 0 {
                taken.set(true);
                return Err(CoreError::Conflict { current: 0 });
            }
            Ok((set, after))
        });
        match made {
            Err(_) if taken.get() => Ok(None),
            r => r.map(Some),
        }
    }

    /// Whether the live page `id` has addresses waiting for a snapshot.
    ///
    /// # Errors
    /// Database errors only.
    pub fn has_pending(&self, id: &ArtifactId) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_pending WHERE artifact_id = ?1)",
                params![id.as_str()],
                |r| r.get(0),
            )?)
        })
    }

    /// Creates or updates live session `sid`'s scope watch on `scope` and
    /// makes it a watcher of every live page the scope covers. A scope-made
    /// watch is armed while any of the session's scope watches covering the
    /// page is; a direct watch keeps its own arming. Returns the watch and
    /// the covered pages' artifact IDs. Under `ctx`, a new scope watch
    /// records `watch.start` (target `scope`) and a change of its arming
    /// `watch.update`; each page watch it starts or re-arms records its own
    /// (target `page`, source and cause `scope`).
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session.
    pub fn live_watch(
        &self,
        ctx: &AuditCtx,
        sid: &str,
        scope: &PageKey,
        replies_armed: bool,
    ) -> Result<(LiveWatch, Vec<String>)> {
        self.with_tx(|tx| {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND ended_at IS NULL)",
                params![sid],
                |r| r.get(0),
            )?;
            if !live {
                return Err(CoreError::invalid(
                    "unknown_session",
                    format!("no live session {sid}"),
                ));
            }
            let before: Option<bool> = tx
                .prepare_cached(
                    "SELECT replies_armed FROM live_watches
                     WHERE session_id = ?1 AND origin = ?2 AND path = ?3",
                )?
                .query_row(params![sid, scope.origin, scope.path], |r| {
                    Ok(r.get::<_, i64>(0)? != 0)
                })
                .optional()?;
            tx.execute(
                "INSERT INTO live_watches (session_id, origin, path, replies_armed, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id, origin, path) DO UPDATE SET replies_armed = excluded.replies_armed
                 WHERE live_watches.replies_armed <> excluded.replies_armed",
                params![sid, scope.origin, scope.path, replies_armed, Store::now()],
            )?;
            let rec = match before {
                None => Some(super::watches::scope_watch_record(
                    AuditKind::WatchStart,
                    sid,
                    &scope.origin,
                    &scope.path,
                    replies_armed,
                )),
                Some(armed) if armed != replies_armed => Some(super::watches::scope_update_record(
                    sid,
                    &scope.origin,
                    &scope.path,
                    replies_armed,
                )),
                Some(_) => None,
            };
            if let Some(rec) = rec {
                self.record_audit(tx, ctx, rec)?;
            }
            // The scope covers the pages of its origin's site (spec §7.2).
            let site_scope = PageKey {
                origin: super::joined::site_key(tx, &scope.origin)?,
                path: scope.path.clone(),
            };
            let mut covered = Vec::new();
            for p in pages_of(tx, &scope.origin)? {
                let keys = page_keys(tx, &p)?;
                if keys.iter().any(|k| k.covered_by(&site_scope)) {
                    let armed = scope_arming(tx, sid, &keys)?.unwrap_or(replies_armed);
                    let wrote = scope_row(tx, sid, &p.artifact_id, armed)?;
                    self.record_page_watches(
                        tx,
                        ctx,
                        &p.artifact_id,
                        &[(sid.to_string(), armed, wrote)],
                        "scope",
                        Some(Cause::Scope),
                    )?;
                    covered.push(p.artifact_id);
                }
            }
            let w = tx.query_row(
                "SELECT session_id, origin, path, replies_armed, created_at FROM live_watches
                 WHERE session_id = ?1 AND origin = ?2 AND path = ?3",
                params![sid, scope.origin, scope.path],
                |r| {
                    Ok(LiveWatch {
                        session_id: r.get(0)?,
                        origin: r.get(1)?,
                        path: r.get(2)?,
                        replies_armed: r.get::<_, i64>(3)? != 0,
                        created_at: r.get(4)?,
                    })
                },
            )?;
            Ok((w, covered))
        })
    }

    /// Removes `sid`'s scope watch on `scope` and the scope-made watches it
    /// alone justified: a page another scope watch of the session covers
    /// keeps its watch, armed as those scopes say, and direct watches are
    /// never removed. Returns the artifact IDs of the pages no longer watched.
    /// Under `ctx`, removing the scope watch records `watch.stop` (target
    /// `scope`), each page watch removed records its own, and each one
    /// re-armed records `watch.update` (cause `scope`).
    ///
    /// # Errors
    /// Database errors only.
    pub fn live_unwatch(&self, ctx: &AuditCtx, sid: &str, scope: &PageKey) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let gone: Option<bool> = tx
                .query_row(
                    "DELETE FROM live_watches WHERE session_id = ?1 AND origin = ?2 AND path = ?3
                     RETURNING replies_armed",
                    params![sid, scope.origin, scope.path],
                    |r| Ok(r.get::<_, i64>(0)? != 0),
                )
                .optional()?;
            if let Some(armed) = gone {
                let rec = super::watches::scope_watch_record(
                    AuditKind::WatchStop,
                    sid,
                    &scope.origin,
                    &scope.path,
                    armed,
                );
                self.record_audit(tx, ctx, rec)?;
            }
            let site_scope = PageKey {
                origin: super::joined::site_key(tx, &scope.origin)?,
                path: scope.path.clone(),
            };
            let mut removed = Vec::new();
            for p in pages_of(tx, &scope.origin)? {
                let keys = page_keys(tx, &p)?;
                // A scope-made watch a move carried to a page its scopes do
                // not cover goes with the scopes too.
                let carried: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM watches
                        WHERE session_id = ?1 AND artifact_id = ?2 AND source = 'scope')",
                    params![sid, p.artifact_id],
                    |r| r.get(0),
                )?;
                if !keys.iter().any(|k| k.covered_by(&site_scope)) && !carried {
                    continue;
                }
                match scope_arming(tx, sid, &keys)? {
                    // Another scope of the session still covers it: its
                    // arming now follows the scopes left.
                    Some(armed) => {
                        let wrote = scope_row(tx, sid, &p.artifact_id, armed)?;
                        self.record_page_watches(
                            tx,
                            ctx,
                            &p.artifact_id,
                            &[(sid.to_string(), armed, wrote)],
                            "scope",
                            Some(Cause::Scope),
                        )?;
                    }
                    None => {
                        let gone: Option<bool> = tx
                            .query_row(
                                "DELETE FROM watches WHERE session_id = ?1 AND artifact_id = ?2 AND source = 'scope'
                                 RETURNING replies_armed",
                                params![sid, p.artifact_id],
                                |r| Ok(r.get::<_, i64>(0)? != 0),
                            )
                            .optional()?;
                        if let Some(armed) = gone {
                            let rec = super::watches::artifact_watch_record(
                                tx,
                                AuditKind::WatchStop,
                                sid,
                                &p.artifact_id,
                                armed,
                                "scope",
                            )?;
                            self.record_audit(tx, ctx, rec)?;
                            removed.push(p.artifact_id);
                        }
                    }
                }
            }
            // Scope-made watches on pages of other sites (an origin split
            // off the site the scope covered, spec §7.2): gone when no scope
            // of the session covers them any more.
            let others: Vec<LivePage> = {
                let mut st = tx.prepare(
                    "SELECT p.artifact_id, p.origin, p.path FROM watches w
                     JOIN live_pages p ON p.artifact_id = w.artifact_id
                     WHERE w.session_id = ?1 AND w.source = 'scope'
                        AND p.origin <> ?2",
                )?;
                st.query_map(params![sid, site_scope.origin], row_to_page)?
                    .collect::<rusqlite::Result<_>>()?
            };
            for p in others {
                let keys = page_keys(tx, &p)?;
                if scope_arming(tx, sid, &keys)?.is_none() {
                    tx.execute(
                        "DELETE FROM watches WHERE session_id = ?1 AND artifact_id = ?2 AND source = 'scope'",
                        params![sid, p.artifact_id],
                    )?;
                    removed.push(p.artifact_id);
                }
            }
            Ok(removed)
        })
    }

    /// Writes `html` as version `expected + 1` (its only file), noted
    /// `snapshot`, linking to it, in its transaction, the threads of
    /// `pending` still pending on the page; returns them with the version.
    fn write_snapshot(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        expected: u32,
        title: &str,
        html: &[u8],
        pending: &[String],
    ) -> Result<(Version, Vec<String>)> {
        let p = snapshot_publish(expected, title, html)?;
        let (_, v, linked) =
            self.write_version_then(ctx, id, expected, &p, &BTreeMap::new(), None, |tx, n| {
                link_pending_in(tx, id, n, Some(pending))
            })?;
        Ok((v, linked))
    }
}

#[cfg(test)]
mod tests {
    use crate::live::{KIND_LIVE, PageKey};
    use crate::store::test_util::DAEMON;
    use crate::{ArtifactId, Home, Store};

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }
    fn key(path: &str) -> PageKey {
        PageKey {
            origin: "http://localhost:5173".into(),
            path: path.into(),
        }
    }
    fn index(st: &Store, id: &ArtifactId, n: u32) -> String {
        std::fs::read_to_string(st.home().version_dir(id, n).join("index.html")).unwrap()
    }

    #[test]
    fn live_page_ids_name_the_pages_that_are_not_deleted() {
        let (_d, st) = store();
        let a = st.ensure_live_page(DAEMON, &key("/a"), "a", None).unwrap();
        let b = st.ensure_live_page(DAEMON, &key("/b"), "b", None).unwrap();
        let mut ids = st.live_page_ids().unwrap();
        ids.sort();
        let mut want = vec![a.artifact.id.clone(), b.artifact.id.clone()];
        want.sort();
        assert_eq!(ids, want);
        st.delete_artifact(DAEMON, &ArtifactId::parse(&a.artifact.id).unwrap())
            .unwrap();
        assert_eq!(st.live_page_ids().unwrap(), vec![b.artifact.id.clone()]);
        let pages = st.live_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "/b");
    }

    #[test]
    fn a_page_created_without_a_snapshot_gets_a_placeholder_version() {
        let (_d, st) = store();
        let e = st
            .ensure_live_page(DAEMON, &key("/"), "localhost:5173/", None)
            .unwrap();
        assert!(e.created && e.new_version);
        assert_eq!(e.artifact.kind, KIND_LIVE);
        assert_eq!(e.version.n, 1);
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        assert!(index(&st, &id, 1).contains("No snapshot yet"));
        assert_eq!(
            st.find_live_page(&key("/")).unwrap().unwrap().artifact_id,
            e.artifact.id
        );
        assert!(
            st.list_artifacts()
                .unwrap()
                .iter()
                .any(|a| a.id == e.artifact.id)
        );
    }

    #[test]
    fn a_first_comment_makes_its_snapshot_version_one_and_identical_snapshots_reuse_it() {
        let (_d, st) = store();
        let e = st
            .ensure_live_page(DAEMON, &key("/s"), "Settings", Some(b"<!doctype html><p>a"))
            .unwrap();
        assert_eq!((e.version.n, e.created, e.new_version), (1, true, true));
        let again = st
            .ensure_live_page(DAEMON, &key("/s"), "Settings", Some(b"<!doctype html><p>a"))
            .unwrap();
        assert_eq!(
            (again.version.n, again.created, again.new_version),
            (1, false, false)
        );
        let changed = st
            .ensure_live_page(DAEMON, &key("/s"), "Settings", Some(b"<!doctype html><p>b"))
            .unwrap();
        assert_eq!((changed.version.n, changed.new_version), (2, true));
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let (v, new) = st
            .store_snapshot(DAEMON, &id, "Settings", b"<!doctype html><p>b", true)
            .unwrap();
        assert_eq!(
            (v.n, new),
            (3, true),
            "force makes a version even when identical"
        );
        assert_eq!(index(&st, &id, 3), "<!doctype html><p>b");
    }

    #[test]
    fn concurrent_snapshots_of_one_page_each_get_a_version() {
        let (_d, st) = store();
        let e = st.ensure_live_page(DAEMON, &key("/"), "x", None).unwrap();
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        const N: usize = 8;
        let start = std::sync::Barrier::new(N);
        let mut got: Vec<u32> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..N)
                .map(|i| {
                    let (st, id, start) = (&st, &id, &start);
                    s.spawn(move || {
                        start.wait();
                        let html = format!("<!doctype html><p>{i}");
                        let (v, new) = st
                            .store_snapshot(DAEMON, id, "x", html.as_bytes(), false)
                            .unwrap();
                        assert!(new);
                        assert_eq!(index(st, id, v.n), html);
                        v.n
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        got.sort_unstable();
        assert_eq!(got, (2..2 + N as u32).collect::<Vec<_>>());
    }

    #[test]
    fn concurrent_comments_for_one_pick_make_one_thread() {
        let (_d, st) = store();
        let e = st
            .ensure_live_page(DAEMON, &key("/"), "x", Some(b"<!doctype html><p>a"))
            .unwrap();
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let pick = "0123456789abcdef0123456789abcdef";
        const N: usize = 6;
        let start = std::sync::Barrier::new(N);
        let made: Vec<Option<String>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..N)
                .map(|_| {
                    let (st, id, start) = (&st, &id, &start);
                    s.spawn(move || {
                        let mut anchor = crate::store::test_util::anchor();
                        anchor.route = Some("#/".into());
                        let t = super::NewThread {
                            version_n: 1,
                            anchor,
                            author_name: "Ana".into(),
                            author_public_id: None,
                            body: "x".into(),
                            clip: None,
                            via_page: false,
                        };
                        start.wait();
                        st.create_picked_thread(DAEMON, id, t, pick)
                            .unwrap()
                            .map(|t| t.id)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let made: Vec<String> = made.into_iter().flatten().collect();
        assert_eq!(made.len(), 1, "one thread is made, the rest find it");
        assert_eq!(
            st.picked_thread(&key("/"), pick).unwrap(),
            Some((e.artifact.id.clone(), made[0].clone()))
        );
        assert_eq!(st.picked_thread(&key("/other"), pick).unwrap(), None);
    }

    #[test]
    fn an_agent_resolve_and_its_pending_address_are_written_together() {
        let (_d, st) = store();
        let e = st
            .ensure_live_page(DAEMON, &key("/"), "x", Some(b"<!doctype html><p>a"))
            .unwrap();
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let mut anchor = crate::store::test_util::anchor();
        anchor.route = Some("#/".into());
        let t = st
            .create_thread(
                DAEMON,
                &id,
                super::NewThread {
                    version_n: 1,
                    anchor,
                    author_name: "Ana".into(),
                    author_public_id: None,
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        // The address cannot be written (the thread is not one of `other`):
        // the resolve is not written either.
        let other = crate::store::test_util::artifact(&st, None);
        let err = st
            .resolve_thread_addressed(DAEMON, &other, &t.id, "agent:claude", "claude", true)
            .unwrap_err();
        assert!(matches!(err, crate::CoreError::NotFound), "{err:?}");
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap().status, "open");
        assert_eq!(st.pending_address(&t.id).unwrap(), None);

        let (r, _) = st
            .resolve_thread_addressed(DAEMON, &id, &t.id, "agent:claude", "claude", true)
            .unwrap();
        assert_eq!(r.status, "resolved");
        assert_eq!(
            st.pending_address(&t.id)
                .unwrap()
                .map(|(h, _)| h)
                .as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn deleting_a_live_page_frees_its_key() {
        let (_d, st) = store();
        let e = st.ensure_live_page(DAEMON, &key("/"), "x", None).unwrap();
        st.delete_artifact(DAEMON, &ArtifactId::parse(&e.artifact.id).unwrap())
            .unwrap();
        assert!(st.find_live_page(&key("/")).unwrap().is_none());
        let again = st.ensure_live_page(DAEMON, &key("/"), "x", None).unwrap();
        assert_ne!(again.artifact.id, e.artifact.id);
    }

    #[test]
    fn snapshots_are_refused_on_html_artifacts() {
        let (_d, st) = store();
        let id = st.insert_artifact_for_test("T", "2026-10-05T00:00:00.000Z");
        let err = st
            .store_snapshot(DAEMON, &id, "T", b"<p>", false)
            .unwrap_err();
        assert!(matches!(err, crate::CoreError::Invalid { code, .. } if code == "not_live"));
    }

    #[test]
    fn a_route_is_refused_on_threads_of_html_artifacts() {
        let (_d, st) = store();
        let id = st.insert_artifact_for_test("T", "2026-10-05T00:00:00.000Z");
        let mut anchor: crate::Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "body", "file": "index.html"
        }))
        .unwrap();
        anchor.route = Some("?a=1".into());
        let err = st
            .create_thread(
                DAEMON,
                &id,
                crate::store::threads::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor,
                    author_name: "A".into(),
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap_err();
        assert!(matches!(err, crate::CoreError::Invalid { code, .. } if code == "invalid_anchor"));
    }
    /// A thread on version 1 of a new live page `/p`.
    fn live_thread(st: &Store) -> (ArtifactId, String) {
        let e = st.ensure_live_page(DAEMON, &key("/p"), "p", None).unwrap();
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let anchor: crate::Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "body", "file": "index.html"
        }))
        .unwrap();
        let t = st
            .create_thread(
                DAEMON,
                &id,
                crate::store::threads::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor,
                    author_name: "A".into(),
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        (id, t.id)
    }

    fn link_source(st: &Store, tid: &str) -> Option<String> {
        use rusqlite::OptionalExtension as _;
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT source FROM version_threads WHERE thread_id = ?1",
                rusqlite::params![tid],
                |r| r.get(0),
            )
            .optional()?)
        })
        .unwrap()
    }

    #[test]
    fn a_pending_address_links_to_the_next_snapshot_with_its_source() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        assert!(!st.has_pending(&id).unwrap());
        assert!(st.mark_pending(&id, &tid, "explicit", "claude").unwrap());
        assert!(st.has_pending(&id).unwrap());
        let (harness, at) = st.pending_address(&tid).unwrap().unwrap();
        assert_eq!(harness, "claude");
        assert!(!at.is_empty());
        assert!(
            !st.mark_pending(&id, &tid, "resolve", "codex").unwrap(),
            "a resolve leaves an explicit address alone"
        );
        assert_eq!(st.pending_address(&tid).unwrap().unwrap().0, "claude");
        let (v, _) = st.store_snapshot(DAEMON, &id, "p", b"<p>2", true).unwrap();
        assert_eq!(st.link_pending(&id, v.n).unwrap(), vec![tid.clone()]);
        assert_eq!(st.addressed_in(&tid).unwrap(), vec![v.n]);
        assert_eq!(link_source(&st, &tid).as_deref(), Some("explicit"));
        assert!(st.pending_address(&tid).unwrap().is_none());
        assert!(!st.has_pending(&id).unwrap());
        assert!(st.link_pending(&id, v.n).unwrap().is_empty());
        assert!(
            !st.mark_pending(&id, &tid, "resolve", "claude").unwrap(),
            "a resolve of a linked thread records nothing"
        );
    }

    #[test]
    fn an_explicit_address_replaces_a_pending_resolve() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        assert!(st.mark_pending(&id, &tid, "resolve", "codex").unwrap());
        assert!(st.mark_pending(&id, &tid, "explicit", "claude").unwrap());
        assert_eq!(st.pending_address(&tid).unwrap().unwrap().0, "claude");
        st.link_pending(&id, 1).unwrap();
        assert_eq!(link_source(&st, &tid).as_deref(), Some("explicit"));
    }

    #[test]
    fn a_resolve_alone_links_as_a_resolve() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        assert!(st.mark_pending(&id, &tid, "resolve", "codex").unwrap());
        st.link_pending(&id, 1).unwrap();
        assert_eq!(link_source(&st, &tid).as_deref(), Some("resolve"));
    }

    #[test]
    fn deleting_a_thread_deletes_its_pending_address() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        st.mark_pending(&id, &tid, "explicit", "claude").unwrap();
        st.delete_thread(DAEMON, &tid).unwrap();
        assert!(st.pending_address(&tid).unwrap().is_none());
        assert!(!st.has_pending(&id).unwrap());
    }
    /// A second thread on the live page `id`, version 1.
    fn another_thread(st: &Store, id: &ArtifactId) -> String {
        let anchor: crate::Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "main", "file": "index.html"
        }))
        .unwrap();
        st.create_thread(
            DAEMON,
            id,
            crate::store::threads::NewThread {
                author_public_id: None,
                version_n: 1,
                anchor,
                author_name: "B".into(),
                body: "y".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn a_pending_snapshot_links_only_the_named_threads_still_pending() {
        let (_d, st) = store();
        let (id, a) = live_thread(&st);
        let b = another_thread(&st, &id);
        let c = another_thread(&st, &id);
        st.mark_pending(&id, &a, "explicit", "claude").unwrap();
        st.mark_pending(&id, &b, "resolve", "codex").unwrap();
        let (v, linked) = st
            .snapshot_pending(
                DAEMON,
                &id,
                "p",
                b"<p>1",
                &[b.clone(), a.clone(), c.clone()],
            )
            .unwrap()
            .expect("two named threads are pending");
        assert_eq!(v.n, 2);
        assert_eq!(linked, vec![a.clone(), b.clone()], "oldest address first");
        assert_eq!(v.addresses, vec![a.clone(), b.clone()]);
        assert_eq!(link_source(&st, &b).as_deref(), Some("resolve"));
        assert!(st.addressed_in(&c).unwrap().is_empty());
        assert!(!st.has_pending(&id).unwrap());
    }

    #[test]
    fn an_unnamed_address_waits_for_a_later_snapshot() {
        let (_d, st) = store();
        let (id, a) = live_thread(&st);
        let b = another_thread(&st, &id);
        st.mark_pending(&id, &a, "explicit", "claude").unwrap();
        st.mark_pending(&id, &b, "explicit", "claude").unwrap();
        let (_, linked) = st
            .snapshot_pending(DAEMON, &id, "p", b"<p>1", std::slice::from_ref(&a))
            .unwrap()
            .unwrap();
        assert_eq!(linked, vec![a.clone()]);
        assert_eq!(st.pending_address(&b).unwrap().unwrap().0, "claude");
        let (v, linked) = st
            .snapshot_pending(DAEMON, &id, "p", b"<p>1", std::slice::from_ref(&b))
            .unwrap()
            .unwrap();
        assert_eq!((v.n, linked), (3, vec![b.clone()]));
    }

    #[test]
    fn a_snapshot_naming_nothing_pending_writes_no_version() {
        let (_d, st) = store();
        let (id, a) = live_thread(&st);
        let b = another_thread(&st, &id);
        st.mark_pending(&id, &b, "explicit", "claude").unwrap();
        assert!(
            st.snapshot_pending(DAEMON, &id, "p", b"<p>1", std::slice::from_ref(&a))
                .unwrap()
                .is_none()
        );
        assert!(
            st.snapshot_pending(DAEMON, &id, "p", b"<p>1", &[])
                .unwrap()
                .is_none()
        );
        let a_ = st.get_artifact(&id).unwrap().unwrap();
        assert_eq!(a_.current_version, 1, "no version was written");
        assert!(!st.home().version_dir(&id, 2).exists());
        assert!(st.pending_address(&b).unwrap().is_some());
    }

    #[test]
    fn an_addressed_reply_writes_its_comment_and_its_mark_together() {
        let (_d, st) = store();
        let (id, tid) = live_thread(&st);
        let reply = |body: &str| crate::store::threads::NewComment {
            author_kind: crate::store::threads::AUTHOR_AGENT,
            author_name: "claude".into(),
            author_public_id: None,
            via_session_id: None,
            body: body.into(),
            via_page: false,
        };
        let c = st
            .add_addressed_reply(DAEMON, &id, &tid, reply("Fixed"), "claude")
            .unwrap();
        assert_eq!(c.body, "Fixed");
        assert_eq!(st.pending_address(&tid).unwrap().unwrap().0, "claude");
        // A thread of another page: neither the comment nor a mark is written.
        let (other, _) = {
            let e = st.ensure_live_page(DAEMON, &key("/q"), "q", None).unwrap();
            (ArtifactId::parse(&e.artifact.id).unwrap(), ())
        };
        let b = another_thread(&st, &id);
        let err = st
            .add_addressed_reply(DAEMON, &other, &b, reply("Fixed"), "claude")
            .unwrap_err();
        assert!(matches!(err, crate::CoreError::NotFound));
        assert!(st.pending_address(&b).unwrap().is_none());
        assert_eq!(st.get_thread(&b).unwrap().unwrap().comments.len(), 1);
    }

    fn watched(st: &Store, sid: &str) -> Vec<String> {
        st.list_watches(sid)
            .unwrap()
            .into_iter()
            .map(|w| w.artifact_id)
            .collect()
    }

    #[test]
    fn a_new_page_is_watched_by_the_scopes_that_cover_it() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        let other = crate::store::test_util::session(&st, "claude", "h2");
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        st.live_watch(DAEMON, &other, &key("/docs"), false).unwrap();
        let e = st
            .ensure_live_page(DAEMON, &key("/new"), "n", None)
            .unwrap();
        let w = st.list_watches(&sid).unwrap();
        assert!(
            w.iter()
                .any(|w| w.artifact_id == e.artifact.id && w.replies_armed)
        );
        assert!(!watched(&st, &other).contains(&e.artifact.id));
        let d = st
            .ensure_live_page(DAEMON, &key("/docs/a"), "d", None)
            .unwrap();
        assert!(watched(&st, &sid).contains(&d.artifact.id));
        assert!(watched(&st, &other).contains(&d.artifact.id));
        // Each scope watch the new page starts is recorded after the page.
        let ev: Vec<(String, serde_json::Value, Option<String>)> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.ids.artifact.as_deref() == Some(d.artifact.id.as_str()))
            .map(|e| {
                (
                    e.kind,
                    serde_json::from_str(&e.body).unwrap(),
                    e.ids.session,
                )
            })
            .collect();
        let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "artifact.create",
                "live.page",
                "watch.start",
                "watch.start",
                "live.snapshot"
            ]
        );
        for (_, body, session) in &ev[2..4] {
            assert_eq!(
                (&body["target"], &body["source"], &body["path"]),
                (
                    &serde_json::json!("page"),
                    &serde_json::json!("scope"),
                    &serde_json::json!("/docs/a")
                )
            );
            let armed = session.as_deref() == Some(sid.as_str());
            assert_eq!(body["replies_armed"], armed, "{session:?}");
        }
        let armed = st
            .list_watches(&other)
            .unwrap()
            .into_iter()
            .find(|w| w.artifact_id == d.artifact.id)
            .unwrap()
            .replies_armed;
        assert!(!armed, "the scope's arming");
        st.unwatch(DAEMON, &sid, &ArtifactId::parse(&e.artifact.id).unwrap())
            .unwrap();
        let again = st
            .ensure_live_page(DAEMON, &key("/new"), "n", None)
            .unwrap();
        assert!(!again.created);
        assert!(
            !watched(&st, &sid).contains(&e.artifact.id),
            "only a new page is materialized"
        );
    }

    /// The watch events after `seq`: kind, target, path, fields or arming,
    /// and cause.
    fn watch_events(st: &Store, seq: i64) -> Vec<(String, String, String, String, String)> {
        st.events_after(seq, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("watch."))
            .map(|e| {
                let b: serde_json::Value = serde_json::from_str(&e.body).unwrap();
                let what = if e.kind == "watch.update" {
                    b["fields"].to_string()
                } else {
                    b["replies_armed"].to_string()
                };
                (
                    e.kind,
                    b["target"].as_str().unwrap().to_string(),
                    b["path"].as_str().unwrap_or("").to_string(),
                    what,
                    b["cause"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn scope_watch_changes_record_starts_updates_and_stops() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        st.ensure_live_page(DAEMON, &key("/docs/x"), "d", None)
            .unwrap();
        let e = |k: &str, t: &str, p: &str, w: &str, c: &str| {
            (
                k.to_string(),
                t.to_string(),
                p.to_string(),
                w.to_string(),
                c.to_string(),
            )
        };
        let up = r#"{"replies_armed":false}"#;
        let down_to_up = r#"{"replies_armed":true}"#;
        let seq = st.newest_seq().unwrap();
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        assert_eq!(
            watch_events(&st, seq),
            [
                e("watch.start", "scope", "/", "true", ""),
                e("watch.start", "page", "/docs/x", "true", "scope")
            ]
        );
        // A second scope that changes no page's arming; the first again.
        let seq = st.newest_seq().unwrap();
        st.live_watch(DAEMON, &sid, &key("/docs"), false).unwrap();
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        assert_eq!(
            watch_events(&st, seq),
            [e("watch.start", "scope", "/docs", "false", "")]
        );
        // Removing the armed scope: the page follows the one left.
        let seq = st.newest_seq().unwrap();
        st.live_unwatch(DAEMON, &sid, &key("/")).unwrap();
        assert_eq!(
            watch_events(&st, seq),
            [
                e("watch.stop", "scope", "/", "true", ""),
                e("watch.update", "page", "/docs/x", up, "scope")
            ]
        );
        // Arming the scope left re-arms it and its page.
        let seq = st.newest_seq().unwrap();
        st.live_watch(DAEMON, &sid, &key("/docs"), true).unwrap();
        assert_eq!(
            watch_events(&st, seq),
            [
                e("watch.update", "scope", "/docs", down_to_up, ""),
                e("watch.update", "page", "/docs/x", down_to_up, "scope")
            ]
        );
        // A direct watch of the page takes it over: its source changes.
        let id = st
            .find_live_page(&key("/docs/x"))
            .unwrap()
            .unwrap()
            .artifact_id;
        let seq = st.newest_seq().unwrap();
        st.watch(DAEMON, &sid, &ArtifactId::parse(&id).unwrap(), true)
            .unwrap();
        assert_eq!(
            watch_events(&st, seq),
            [e(
                "watch.update",
                "page",
                "/docs/x",
                r#"{"source":"direct"}"#,
                ""
            )]
        );
    }

    #[test]
    fn a_scope_watch_covers_existing_pages_and_its_removal_keeps_what_else_justifies_a_watch() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        let root = st
            .ensure_live_page(DAEMON, &key("/"), "r", None)
            .unwrap()
            .artifact
            .id;
        let docs = st
            .ensure_live_page(DAEMON, &key("/docs/x"), "d", None)
            .unwrap()
            .artifact
            .id;
        let direct = st
            .ensure_live_page(DAEMON, &key("/z"), "z", None)
            .unwrap()
            .artifact
            .id;
        let (w, mut covered) = st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        assert_eq!(
            (w.origin.as_str(), w.path.as_str()),
            ("http://localhost:5173", "/")
        );
        covered.sort();
        let mut all = vec![root.clone(), docs.clone(), direct.clone()];
        all.sort();
        assert_eq!(covered, all);
        st.live_watch(DAEMON, &sid, &key("/docs"), true).unwrap();
        st.watch(DAEMON, &sid, &ArtifactId::parse(&direct).unwrap(), true)
            .unwrap();
        let removed = st.live_unwatch(DAEMON, &sid, &key("/")).unwrap();
        assert_eq!(removed, vec![root.clone()]);
        let left = watched(&st, &sid);
        assert!(left.contains(&docs), "the /docs scope still covers it");
        assert!(left.contains(&direct), "a direct watch stays");
        assert!(!left.contains(&root));
    }

    #[test]
    fn a_direct_watch_is_not_turned_into_a_scope_row() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        let p = st
            .ensure_live_page(DAEMON, &key("/p"), "p", None)
            .unwrap()
            .artifact
            .id;
        let id = ArtifactId::parse(&p).unwrap();
        st.watch(DAEMON, &sid, &id, false).unwrap();
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        let w = st.list_watches(&sid).unwrap();
        assert!(!w[0].replies_armed, "the direct watch keeps its arming");
        let q = st
            .ensure_live_page(DAEMON, &key("/q"), "q", None)
            .unwrap()
            .artifact
            .id;
        st.live_watch(DAEMON, &sid, &key("/"), false).unwrap();
        let w = st.list_watches(&sid).unwrap();
        let armed = |aid: &str| {
            w.iter()
                .find(|w| w.artifact_id == aid)
                .unwrap()
                .replies_armed
        };
        assert!(
            !armed(&q),
            "watching the scope again sets its pages' arming"
        );
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        let w = st.list_watches(&sid).unwrap();
        let armed = |aid: &str| {
            w.iter()
                .find(|w| w.artifact_id == aid)
                .unwrap()
                .replies_armed
        };
        assert!(armed(&q));
        assert!(!armed(&p), "but not a direct watch's");
        st.live_unwatch(DAEMON, &sid, &key("/")).unwrap();
        assert_eq!(watched(&st, &sid), vec![p]);
    }

    #[test]
    fn ending_a_session_ends_its_scope_watches() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        st.end_session(DAEMON, &sid).unwrap();
        let rows: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM live_watches WHERE session_id = ?1",
                    rusqlite::params![sid],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(rows, 0);
        let e = st
            .ensure_live_page(DAEMON, &key("/later"), "l", None)
            .unwrap();
        assert!(!watched(&st, &sid).contains(&e.artifact.id));
        assert!(matches!(
            st.live_watch(DAEMON, &sid, &key("/"), true),
            Err(crate::CoreError::Invalid { .. })
        ));
    }

    #[test]
    fn a_page_is_armed_while_any_scope_covering_it_is() {
        let (_d, st) = store();
        let sid = crate::store::test_util::session(&st, "claude", "h1");
        let armed = |aid: &str| {
            st.list_watches(&sid)
                .unwrap()
                .into_iter()
                .find(|w| w.artifact_id == aid)
                .unwrap()
                .replies_armed
        };
        st.live_watch(DAEMON, &sid, &key("/"), true).unwrap();
        st.live_watch(DAEMON, &sid, &key("/docs"), false).unwrap();
        let a = st
            .ensure_live_page(DAEMON, &key("/docs/a"), "a", None)
            .unwrap()
            .artifact
            .id;
        assert!(armed(&a), "the armed / scope covers it");
        st.live_watch(DAEMON, &sid, &key("/docs"), false).unwrap();
        assert!(
            armed(&a),
            "re-watching /docs unarmed leaves it armed through /"
        );
        let b = st
            .ensure_live_page(DAEMON, &key("/docs/b"), "b", None)
            .unwrap()
            .artifact
            .id;
        assert!(armed(&b));
        st.live_unwatch(DAEMON, &sid, &key("/")).unwrap();
        assert!(!armed(&a), "only the unarmed /docs scope covers it now");
        assert!(!armed(&b));
        st.live_watch(DAEMON, &sid, &key("/docs"), true).unwrap();
        assert!(armed(&a));
    }
}

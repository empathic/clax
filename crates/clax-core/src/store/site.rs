//! Site-wide threads of live pages (spec 2026-10-05-chrome-overlay-design
//! §7.1): every live page of an origin with its threads, the merge rules
//! that map paths to one canonical page, and moving threads between pages.

use super::Store;
use super::live::{LivePage, PAGES_OF_ORIGIN, materialize};
use super::threads::threads_of_many;
use crate::anchor::Anchor;
use crate::live::{PageKey, PathPattern, winning_rule};
use crate::model::{Thread, Version};
use crate::publish::INDEX;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Merge rules one origin may hold.
pub const MAX_RULES: usize = 64;
/// Threads one rule application (or un-merge) re-files; the caller repeats
/// the request for the rest.
pub const MAX_REFILE: usize = 200;
/// How many times [`Store::refile_threads`] plans again when a thread
/// changes under it.
const REFILE_ATTEMPTS: u32 = 4;
/// The note of a version a move copied to its new page.
pub const MOVED_NOTE: &str = "moved";
/// A thread's move by the owner.
pub const KIND_MOVE: &str = "move";
/// A thread's move by a merge rule.
pub const KIND_MERGE: &str = "merge";
/// A thread's move back to its own path's page when its rule was deleted.
pub const KIND_UNMERGE: &str = "unmerge";

/// A merge rule: the live pages of `origin` whose path `pattern` matches
/// are one page, the canonical page whose path is `pattern`. `deleting`
/// while a deleted rule's threads are still being moved back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveRule {
    pub id: String,
    pub origin: String,
    pub pattern: String,
    pub created_at: String,
    pub deleting: bool,
}

/// The rules of origin `?1` in force, oldest first.
pub(crate) const RULES_OF_ORIGIN: &str =
    "SELECT id, origin, pattern, created_at, deleted_at IS NOT NULL FROM live_rules
    WHERE origin = ?1 AND deleted_at IS NULL ORDER BY created_at, id";
const RULE_COLUMNS: &str =
    "SELECT id, origin, pattern, created_at, deleted_at IS NOT NULL FROM live_rules";
/// The live pages of origin `?1` with their title and current version.
pub(crate) const SITE_PAGES: &str =
    "SELECT p.artifact_id, p.origin, p.path, a.title, a.current_version
    FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
    WHERE p.origin = ?1 AND a.deleted_at IS NULL";
/// The threads of the pages of `?1` (a JSON array of artifact IDs), without
/// comments: ID, page, `live_path` and anchor.
pub(crate) const REFILE_CANDIDATES: &str = "SELECT t.id, t.artifact_id, t.live_path, t.anchor_json
    FROM threads t JOIN artifacts a ON a.id = t.artifact_id
    WHERE a.deleted_at IS NULL AND t.artifact_id IN (SELECT value FROM json_each(?1))
    ORDER BY t.created_at, t.id";
/// The threads of the canonical page `?1` that a rule put there (merged, or
/// made at a path it mapped), not the owner (whose latest move is a
/// `move`): ID, page, `live_path` and anchor.
pub(crate) const TO_UNMERGE: &str = "SELECT t.id, t.artifact_id, t.live_path, t.anchor_json
    FROM threads t WHERE t.artifact_id = ?1
        AND NOT EXISTS(SELECT 1 FROM thread_moves m WHERE m.id =
            (SELECT m2.id FROM thread_moves m2 WHERE m2.thread_id = t.id
             ORDER BY m2.created_at DESC, m2.id DESC LIMIT 1) AND m.kind = 'move')
    ORDER BY t.created_at, t.id";
/// Of the threads of `?1` (a JSON array), those on a live page of origin
/// `?2` made at path `?3` (their `live_path`, else their page's path).
pub(crate) const PENDING_AT_PATH: &str = "SELECT t.id FROM json_each(?1) j
    CROSS JOIN threads t ON t.id = j.value
    JOIN live_pages p ON p.artifact_id = t.artifact_id
    WHERE p.origin = ?2 AND COALESCE(t.live_path, p.path) = ?3";
/// The version links of thread `?1`.
pub(crate) const LINKS_OF_THREAD: &str =
    "SELECT artifact_id, version_n, source, created_at FROM version_threads WHERE thread_id = ?1";
/// A moved thread's pending address follows it to page `?2`.
pub(crate) const PENDING_TO: &str = "UPDATE live_pending SET artifact_id = ?2 WHERE thread_id = ?1";
/// A moved thread's pick follows it to page `?2`...
pub(crate) const PICKS_TO: &str =
    "UPDATE OR IGNORE live_picks SET artifact_id = ?2 WHERE thread_id = ?1";
/// ...and a pick that could not (page `?2` has its pick ID) is dropped.
pub(crate) const PICKS_LEFT: &str =
    "DELETE FROM live_picks WHERE thread_id = ?1 AND artifact_id <> ?2";
/// The live sessions watching page `?1` watch page `?2` too, as they did.
pub(crate) const WATCHES_TO: &str =
    "INSERT OR IGNORE INTO watches (session_id, artifact_id, replies_armed, created_at, source)
    SELECT w.session_id, ?2, w.replies_armed, ?3, w.source FROM watches w
    CROSS JOIN sessions s ON s.id = w.session_id
    WHERE w.artifact_id = ?1 AND s.ended_at IS NULL";
/// The live sessions thread `?1` was sent to, or has feedback not yet
/// acknowledged for, watch page `?2`.
pub(crate) const TARGETS_TO: &str =
    "INSERT OR IGNORE INTO watches (session_id, artifact_id, replies_armed, created_at, source)
    SELECT x.sid, ?2, 1, ?3, 'direct' FROM
        (SELECT target_session_id AS sid FROM threads WHERE id = ?1
         UNION SELECT target_session_id FROM feedback
            WHERE thread_id = ?1 AND acknowledged_at IS NULL) x
    CROSS JOIN sessions s ON s.id = x.sid
    WHERE x.sid IS NOT NULL AND s.ended_at IS NULL";

fn row_to_rule(r: &rusqlite::Row<'_>) -> rusqlite::Result<LiveRule> {
    Ok(LiveRule {
        id: r.get(0)?,
        origin: r.get(1)?,
        pattern: r.get(2)?,
        created_at: r.get(3)?,
        deleting: r.get(4)?,
    })
}

fn rules_in(c: &Connection, origin: &str) -> Result<Vec<LiveRule>> {
    let mut st = c.prepare_cached(RULES_OF_ORIGIN)?;
    let rules = st
        .query_map(params![origin], row_to_rule)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rules)
}

fn rule_by_id(c: &Connection, id: &str) -> Result<Option<LiveRule>> {
    Ok(c.query_row(
        &format!("{RULE_COLUMNS} WHERE id = ?1"),
        params![id],
        row_to_rule,
    )
    .optional()?)
}

/// Where a page key's comments go: the canonical page of the rule that maps
/// it, or the key itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The live page's key.
    pub key: PageKey,
    /// The path the URL named, when a rule mapped it to another path.
    pub live_path: Option<String>,
    /// The rule that mapped it (or whose canonical page it is).
    pub rule: Option<LiveRule>,
}

/// A live page of a site, with its title, current version and threads
/// (resolved ones too, oldest first).
#[derive(Clone, Debug)]
pub struct SitePage {
    pub page: LivePage,
    pub title: String,
    pub current_version: u32,
    pub threads: Vec<Thread>,
}

/// One thread to re-file: where on its new page it now is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refile {
    pub thread_id: String,
    /// The thread's path, when it is not its new page's path.
    pub live_path: Option<String>,
    /// The thread's route on its new page.
    pub route: Option<String>,
}

/// Who re-files threads, and why: `by` is `viewer:<public ID>` of the
/// owner; `kind` is [`KIND_MOVE`], [`KIND_MERGE`] or [`KIND_UNMERGE`], with
/// the rule for the last two.
#[derive(Clone, Debug)]
pub struct MoveBy {
    pub by: String,
    pub kind: &'static str,
    pub rule_id: Option<String>,
}

/// One re-filed thread: its ID, the page it left and the page it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub thread_id: String,
    pub from: String,
    pub to: String,
}

/// What [`Store::refile_threads`] did.
#[derive(Clone, Debug, Default)]
pub struct Refiled {
    /// Each thread it re-filed (its page is unchanged when only its path
    /// or route changed).
    pub moved: Vec<Moved>,
    /// The versions it wrote, oldest first per page.
    pub versions: Vec<Version>,
}

/// A thread's re-filing, as planned.
struct Plan {
    tid: String,
    from: String,
    from_url: String,
    to: LivePage,
    version_n: u32,
    anchor: Anchor,
    has_clip: bool,
    live_path: Option<String>,
    route: Option<String>,
}

/// A file copied ahead of a transaction; unless kept, removed on
/// drop.
struct Pending {
    path: PathBuf,
    keep: bool,
}

impl Pending {
    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A staging directory, removed on drop unless renamed away.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The on-disk path of `path` in a version directory.
fn version_file(dir: &Path, path: &str) -> PathBuf {
    if path == INDEX {
        dir.join(INDEX)
    } else {
        dir.join("files").join(path)
    }
}

/// The files of version `n` of artifact `aid`, as its `files_json` and
/// parsed.
fn version_files(
    tx: &Connection,
    aid: &str,
    n: u32,
) -> Result<(String, BTreeMap<String, serde_json::Value>)> {
    let files_json: String = tx.query_row(
        "SELECT files_json FROM versions WHERE artifact_id = ?1 AND n = ?2",
        params![aid, n],
        |r| r.get(0),
    )?;
    let files = serde_json::from_str(&files_json).map_err(|_| CoreError::Corrupt {
        artifact_id: aid.to_string(),
        column: "files_json",
        version: Some(n),
    })?;
    Ok((files_json, files))
}

/// A version of live page `to` with the same files, byte for byte, as
/// version `n` of live page `src`, newest first: a move reuses it instead
/// of writing the same bytes again (a thread moved back, or a page holding
/// that snapshot already). Sizes are compared before any file is read.
fn same_version(
    tx: &Connection,
    st: &Store,
    (src, n): (&str, u32),
    to: &str,
) -> Result<Option<u32>> {
    let (src_json, src_files) = version_files(tx, src, n)?;
    let src_dir = st.home.version_dir(&ArtifactId::parse(src)?, n);
    let to_id = ArtifactId::parse(to)?;
    let candidates: Vec<(u32, String)> = {
        let mut q = tx.prepare_cached(
            "SELECT n, files_json FROM versions WHERE artifact_id = ?1 ORDER BY n DESC",
        )?;
        q.query_map(params![to], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut src_bytes: HashMap<&str, Vec<u8>> = HashMap::new();
    for (m, json) in candidates {
        if json != src_json {
            continue; // other paths, sizes or types
        }
        let dir = st.home.version_dir(&to_id, m);
        let mut same = true;
        for path in src_files.keys() {
            if !src_bytes.contains_key(path.as_str()) {
                src_bytes.insert(path, std::fs::read(version_file(&src_dir, path))?);
            }
            let theirs = std::fs::read(version_file(&dir, path)).ok();
            if theirs.as_deref() != src_bytes.get(path.as_str()).map(Vec::as_slice) {
                same = false;
                break;
            }
        }
        if same {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// The URL of a thread at `path` (or its page's path) with `route`.
fn thread_url(page: &LivePage, live_path: Option<&str>, route: Option<&str>) -> String {
    format!(
        "{}{}{}",
        page.origin,
        live_path.unwrap_or(&page.path),
        route.unwrap_or("")
    )
}

fn plan_refile(
    c: &Connection,
    targets: &HashMap<String, LivePage>,
    moves: &[(String, Refile)],
) -> Result<Vec<Plan>> {
    let mut out = Vec::new();
    for (to, m) in moves {
        let to = targets.get(to).ok_or(CoreError::NotFound)?;
        let row: Option<(String, u32, String, bool, Option<String>)> = c
            .query_row(
                "SELECT t.artifact_id, t.version_n, t.anchor_json, t.has_clip, t.live_path
                 FROM threads t JOIN artifacts a ON a.id = t.artifact_id
                 WHERE t.id = ?1 AND a.deleted_at IS NULL",
                params![m.thread_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((from, version_n, anchor_json, has_clip, live_path)) = row else {
            return Err(CoreError::NotFound);
        };
        let Some(page) = super::live::live_page_of_conn(c, &from)? else {
            return Err(CoreError::invalid(
                "not_live",
                format!("thread {} is not on a live page", m.thread_id),
            ));
        };
        if page.origin != to.origin {
            return Err(CoreError::invalid(
                "cross_origin",
                "a thread moves only to a page of its own origin",
            ));
        }
        let anchor: Anchor =
            serde_json::from_str(&anchor_json).map_err(|_| CoreError::Corrupt {
                artifact_id: from.clone(),
                column: "anchor_json",
                version: Some(version_n),
            })?;
        if from == to.artifact_id && live_path == m.live_path && anchor.route == m.route {
            continue;
        }
        out.push(Plan {
            tid: m.thread_id.clone(),
            from_url: thread_url(&page, live_path.as_deref(), anchor.route.as_deref()),
            from,
            to: to.clone(),
            version_n,
            anchor,
            has_clip,
            live_path: m.live_path.clone(),
            route: m.route.clone(),
        });
    }
    Ok(out)
}

/// Copies version `n` of live page `src` as version `new_n` of live page
/// `to`, noted `note`; records the directory in `renamed` once it is in
/// place.
fn copy_version(
    tx: &Connection,
    st: &Store,
    (src, n): (&str, u32),
    (to, new_n): (&str, u32),
    note: Option<&str>,
    renamed: &mut Vec<PathBuf>,
) -> Result<()> {
    let src_id = ArtifactId::parse(src)?;
    let to_id = ArtifactId::parse(to)?;
    let (files_json, files) = version_files(tx, src, n)?;
    let versions = st.home.artifact_dir(&to_id).join("versions");
    let staging = Staging(versions.join(format!(".tmp-{}", new_ulid())));
    std::fs::create_dir_all(staging.0.join("files"))?;
    let src_dir = st.home.version_dir(&src_id, n);
    for path in files.keys() {
        let dest = version_file(&staging.0, path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(version_file(&src_dir, path), &dest)?;
    }
    tx.execute(
        "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json, note)
         VALUES (?1, ?2, NULL, ?3, NULL, ?4, ?5)",
        params![to, new_n, Store::now(), files_json, note],
    )?;
    let vdir = st.home.version_dir(&to_id, new_n);
    std::fs::rename(&staging.0, &vdir)?;
    renamed.push(vdir);
    Ok(())
}

/// Re-files the planned threads, in one transaction: for each new page, the
/// versions its threads name are found there (same bytes) or copied there,
/// and after a copy the page's own latest snapshot is put back on top; then
/// each thread names those versions, takes its
/// pending address, pick and watchers along, and its move is recorded.
/// `Conflict` when a thread changed page since it was planned.
fn commit_refile(
    tx: &rusqlite::Transaction<'_>,
    st: &Store,
    plans: &[Plan],
    how: &MoveBy,
    fresh: &[String],
    renamed: &mut Vec<PathBuf>,
) -> Result<Vec<(String, u32)>> {
    let now = Store::now();
    let mut written = Vec::new();
    let mut order: Vec<&LivePage> = Vec::new();
    for p in plans {
        if !order.iter().any(|t| t.artifact_id == p.to.artifact_id) {
            order.push(&p.to);
        }
    }
    for to in order {
        let group: Vec<&Plan> = plans
            .iter()
            .filter(|p| p.to.artifact_id == to.artifact_id)
            .collect();
        let mut links: HashMap<String, Vec<(String, u32, String, String)>> = HashMap::new();
        let mut needed: BTreeSet<(String, u32)> = BTreeSet::new();
        for p in &group {
            let from: Option<String> = tx
                .query_row(
                    "SELECT artifact_id FROM threads WHERE id = ?1",
                    params![p.tid],
                    |r| r.get(0),
                )
                .optional()?;
            if from.as_deref() != Some(p.from.as_str()) {
                return Err(CoreError::Conflict { current: 0 });
            }
            if p.from == to.artifact_id {
                continue;
            }
            needed.insert((p.from.clone(), p.version_n));
            let mut q = tx.prepare_cached(LINKS_OF_THREAD)?;
            let ls: Vec<(String, u32, String, String)> = q
                .query_map(params![p.tid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (a, n, _, _) in &ls {
                needed.insert((a.clone(), *n));
            }
            links.insert(p.tid.clone(), ls);
        }
        let mut current: u32 = tx.query_row(
            "SELECT current_version FROM artifacts WHERE id = ?1",
            params![to.artifact_id],
            |r| r.get(0),
        )?;
        let mut map: HashMap<(String, u32), u32> = HashMap::new();
        let mut copied = false;
        for (src, n) in &needed {
            if let Some(m) = same_version(tx, st, (src, *n), &to.artifact_id)? {
                map.insert((src.clone(), *n), m);
                continue;
            }
            current += 1;
            copy_version(
                tx,
                st,
                (src, *n),
                (&to.artifact_id, current),
                Some(MOVED_NOTE),
                renamed,
            )?;
            map.insert((src.clone(), *n), current);
            written.push((to.artifact_id.clone(), current));
            copied = true;
        }
        if copied {
            // The page's own latest snapshot goes back on top, so its
            // current version is still its own (a page made for this move
            // has only its placeholder, which stays below).
            let own: Option<(u32, Option<String>)> = if fresh.contains(&to.artifact_id) {
                None
            } else {
                tx.query_row(
                    "SELECT n, note FROM versions WHERE artifact_id = ?1
                        AND (note IS NULL OR note <> ?2) ORDER BY n DESC LIMIT 1",
                    params![to.artifact_id, MOVED_NOTE],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
            };
            if let Some((n, note)) = own {
                current += 1;
                copy_version(
                    tx,
                    st,
                    (&to.artifact_id, n),
                    (&to.artifact_id, current),
                    note.as_deref(),
                    renamed,
                )?;
                written.push((to.artifact_id.clone(), current));
            }
            tx.execute(
                "UPDATE artifacts SET current_version = ?2, updated_at = ?3 WHERE id = ?1",
                params![to.artifact_id, current, now],
            )?;
        }
        for p in group {
            let mut version_n = p.version_n;
            if p.from != to.artifact_id {
                version_n = map[&(p.from.clone(), p.version_n)];
                tx.execute(
                    "DELETE FROM version_threads WHERE thread_id = ?1",
                    params![p.tid],
                )?;
                for (a, n, source, at) in links.remove(&p.tid).unwrap_or_default() {
                    tx.execute(
                        "INSERT OR IGNORE INTO version_threads
                            (artifact_id, version_n, thread_id, source, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![to.artifact_id, map[&(a, n)], p.tid, source, at],
                    )?;
                }
                tx.execute(PENDING_TO, params![p.tid, to.artifact_id])?;
                tx.execute(PICKS_TO, params![p.tid, to.artifact_id])?;
                tx.execute(PICKS_LEFT, params![p.tid, to.artifact_id])?;
                // Its agents keep hearing of it on its new page.
                tx.execute(WATCHES_TO, params![p.from, to.artifact_id, now])?;
                tx.execute(TARGETS_TO, params![p.tid, to.artifact_id, now])?;
            }
            let key = PageKey {
                origin: to.origin.clone(),
                path: p.live_path.clone().unwrap_or_else(|| to.path.clone()),
            };
            materialize(tx, &to.artifact_id, &key)?;
            let mut anchor = p.anchor.clone();
            anchor.route = p.route.clone();
            tx.execute(
                "UPDATE threads SET artifact_id = ?2, version_n = ?3, anchor_json = ?4,
                    live_path = ?5 WHERE id = ?1",
                params![
                    p.tid,
                    to.artifact_id,
                    version_n,
                    serde_json::to_string(&anchor).expect("anchors serialise"),
                    p.live_path
                ],
            )?;
            tx.execute(
                "INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url,
                    to_artifact_id, to_url, moved_by, kind, rule_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    new_ulid(),
                    p.tid,
                    p.from,
                    p.from_url,
                    to.artifact_id,
                    thread_url(to, p.live_path.as_deref(), p.route.as_deref()),
                    how.by,
                    how.kind,
                    how.rule_id,
                    now
                ],
            )?;
        }
    }
    Ok(written)
}

/// Candidates of a rule application or un-merge: each thread's ID, page,
/// `live_path` and route.
type Candidate = (String, String, Option<String>, Option<String>);

fn candidates(c: &Connection, sql: &str, arg: &str) -> Result<Vec<Candidate>> {
    let mut st = c.prepare_cached(sql)?;
    let rows = st
        .query_map(params![arg], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, aid, live_path, anchor)| {
            let route = serde_json::from_str::<Anchor>(&anchor)
                .ok()
                .and_then(|a| a.route);
            (id, aid, live_path, route)
        })
        .collect())
}

impl Store {
    /// The merge rules of `origin` in force, oldest first.
    ///
    /// # Errors
    /// Database errors only.
    pub fn live_rules(&self, origin: &str) -> Result<Vec<LiveRule>> {
        self.with_read(|c| rules_in(c, origin))
    }

    /// The rule `id`, in force or being deleted.
    ///
    /// # Errors
    /// Database errors only.
    pub fn live_rule(&self, id: &str) -> Result<Option<LiveRule>> {
        self.with_read(|c| rule_by_id(c, id))
    }

    /// Adds the rule `pattern` to `origin`; a rule already there is answered
    /// as it is (`false`: not new), and one being deleted is put back in
    /// force.
    ///
    /// # Errors
    /// `too_many_rules` past [`MAX_RULES`] rules in force for the origin.
    pub fn add_live_rule(&self, origin: &str, pattern: &PathPattern) -> Result<(LiveRule, bool)> {
        self.with_tx(|tx| {
            let old = tx
                .query_row(
                    &format!("{RULE_COLUMNS} WHERE origin = ?1 AND pattern = ?2"),
                    params![origin, pattern.as_str()],
                    row_to_rule,
                )
                .optional()?;
            if let Some(mut r) = old {
                tx.execute(
                    "UPDATE live_rules SET deleted_at = NULL WHERE id = ?1",
                    params![r.id],
                )?;
                r.deleting = false;
                return Ok((r, false));
            }
            if rules_in(tx, origin)?.len() >= MAX_RULES {
                return Err(CoreError::invalid(
                    "too_many_rules",
                    format!("an origin holds at most {MAX_RULES} rules"),
                ));
            }
            let r = LiveRule {
                id: new_ulid(),
                origin: origin.to_string(),
                pattern: pattern.as_str().to_string(),
                created_at: Store::now(),
                deleting: false,
            };
            tx.execute(
                "INSERT INTO live_rules (id, origin, pattern, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![r.id, r.origin, r.pattern, r.created_at],
            )?;
            Ok((r, true))
        })
    }

    /// Takes the rule `id` out of force (it maps nothing from now on) while
    /// its threads are moved back ([`Store::unmerge_candidates`]); `None`
    /// when there is no such rule.
    ///
    /// # Errors
    /// Database errors only.
    pub fn mark_rule_deleted(&self, id: &str) -> Result<Option<LiveRule>> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE live_rules SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, Store::now()],
            )?;
            rule_by_id(tx, id)
        })
    }

    /// Removes the rule `id` for good, once its threads are moved back.
    ///
    /// # Errors
    /// Database errors only.
    pub fn drop_rule(&self, id: &str) -> Result<()> {
        self.with_write(|c| {
            c.execute("DELETE FROM live_rules WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Where `key`'s comments go: the canonical page of the rule of its
    /// origin that maps its path ([`resolve_with`]), or `key` itself.
    ///
    /// # Errors
    /// Database errors only.
    pub fn resolve_live_key(&self, key: &PageKey) -> Result<Resolved> {
        let rules = self.live_rules(&key.origin)?;
        Ok(resolve_with(&rules, key))
    }

    /// Of `threads`, those on a live page of `origin` made at `path` (their
    /// `live_path`, else their page's path): the pending addresses a
    /// snapshot of that path may settle.
    ///
    /// # Errors
    /// Database errors only.
    pub fn pending_at_path(
        &self,
        threads: &[String],
        origin: &str,
        path: &str,
    ) -> Result<Vec<String>> {
        if threads.is_empty() {
            return Ok(Vec::new());
        }
        self.with_read(|c| {
            let mut st = c.prepare_cached(PENDING_AT_PATH)?;
            let ids = st
                .query_map(
                    params![super::feedback::id_array(threads), origin, path],
                    |r| r.get(0),
                )?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(ids)
        })
    }

    /// Every live page of `origin` with its threads.
    ///
    /// # Errors
    /// Database errors only.
    pub fn site_pages(&self, origin: &str) -> Result<Vec<SitePage>> {
        self.with_read(|c| {
            let pages: Vec<(LivePage, String, u32)> = {
                let mut st = c.prepare_cached(SITE_PAGES)?;
                st.query_map(params![origin], |r| {
                    Ok((
                        LivePage {
                            artifact_id: r.get(0)?,
                            origin: r.get(1)?,
                            path: r.get(2)?,
                        },
                        r.get(3)?,
                        r.get(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let ids: Vec<String> = pages.iter().map(|p| p.0.artifact_id.clone()).collect();
            let mut by_page: HashMap<String, Vec<Thread>> = HashMap::new();
            for t in threads_of_many(c, &ids)? {
                by_page.entry(t.artifact_id.clone()).or_default().push(t);
            }
            Ok(pages
                .into_iter()
                .map(|(page, title, current_version)| SitePage {
                    threads: by_page.remove(&page.artifact_id).unwrap_or_default(),
                    page,
                    title,
                    current_version,
                })
                .collect())
        })
    }

    /// The first `limit` threads of live pages of `rule`'s origin that the
    /// rule (in force) now maps and that are not on its canonical page yet,
    /// oldest first: each whose path (its `live_path`, else its page's) the
    /// rule wins ([`resolve_with`]), placed on the canonical page; and how
    /// many more there are.
    ///
    /// # Errors
    /// Database errors only.
    pub fn merge_candidates(&self, rule: &LiveRule, limit: usize) -> Result<(Vec<Refile>, usize)> {
        let rules = self.live_rules(&rule.origin)?;
        let all: Vec<Refile> = self.with_read(|c| {
            let pages: HashMap<String, String> = {
                let mut st = c.prepare_cached(PAGES_OF_ORIGIN)?;
                st.query_map(params![rule.origin], |r| Ok((r.get(0)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let ids: Vec<String> = pages.keys().cloned().collect();
            let mut out = Vec::new();
            for (tid, aid, live_path, route) in
                candidates(c, REFILE_CANDIDATES, &super::feedback::id_array(&ids))?
            {
                let Some(page_path) = pages.get(&aid) else {
                    continue;
                };
                if *page_path == rule.pattern {
                    continue;
                }
                let key = PageKey {
                    origin: rule.origin.clone(),
                    path: live_path.unwrap_or_else(|| page_path.clone()),
                };
                let r = resolve_with(&rules, &key);
                if r.rule.as_ref().map(|r| &r.id) == Some(&rule.id) {
                    out.push(Refile {
                        thread_id: tid,
                        live_path: r.live_path,
                        route,
                    });
                }
            }
            Ok(out)
        })?;
        let remaining = all.len().saturating_sub(limit);
        Ok((all.into_iter().take(limit).collect(), remaining))
    }

    /// The first `limit` threads of `rule`'s (being deleted) canonical page
    /// that a rule put there rather than the owner ([`TO_UNMERGE`]), each
    /// with the page key of the path it was made at under the rules still
    /// in force (its own page again, unless another rule maps it) and its
    /// place there; and how many more there are. A thread made at the
    /// pattern's own path stays.
    ///
    /// # Errors
    /// Database errors only.
    pub fn unmerge_candidates(
        &self,
        rule: &LiveRule,
        limit: usize,
    ) -> Result<(Vec<(PageKey, Refile)>, usize)> {
        let rules = self.live_rules(&rule.origin)?;
        let canonical = PageKey {
            origin: rule.origin.clone(),
            path: rule.pattern.clone(),
        };
        let Some(canonical) = self.find_live_page(&canonical)? else {
            return Ok((Vec::new(), 0));
        };
        let all: Vec<(PageKey, Refile)> = self.with_read(|c| {
            let mut out = Vec::new();
            for (tid, aid, live_path, route) in candidates(c, TO_UNMERGE, &canonical.artifact_id)? {
                let Some(page) = super::live::live_page_of_conn(c, &aid)? else {
                    continue;
                };
                let key = PageKey {
                    origin: page.origin.clone(),
                    path: live_path.unwrap_or_else(|| page.path.clone()),
                };
                let r = resolve_with(&rules, &key);
                if r.key.path == page.path {
                    continue;
                }
                out.push((
                    r.key,
                    Refile {
                        thread_id: tid,
                        live_path: r.live_path,
                        route,
                    },
                ));
            }
            Ok(out)
        })?;
        let remaining = all.len().saturating_sub(limit);
        Ok((all.into_iter().take(limit).collect(), remaining))
    }

    /// Re-files threads (spec 2026-10-05-chrome-overlay-design §7.1): each
    /// of `moves` under its live page, placed as its [`Refile`] says, its
    /// move recorded as `how` says. A thread already where its `Refile` puts
    /// it is left alone. `fresh` names pages made for this re-filing.
    ///
    /// A thread changing page keeps its comments, sends, feedback and
    /// history; its snapshot version, and each version that addressed it,
    /// becomes a version of the new page: one the page already holds with
    /// the same bytes, else a copy written as a new version noted
    /// [`MOVED_NOTE`] (one per source version, whatever number of threads
    /// name it), and the thread names those; its pending address, pick and
    /// clip follow it, and so do its agents: the sessions watching the page
    /// it left, it was sent to, or with feedback of it not yet acknowledged,
    /// watch the new page too, as do the scope watches covering its path.
    /// When a page that existed got copies, its newest version not noted
    /// [`MOVED_NOTE`] is written once more on top, keeping its own latest
    /// snapshot current. All of it is one transaction.
    ///
    /// # Errors
    /// `NotFound` for a missing thread or page; `not_live` for a thread not
    /// on a live page; `cross_origin` for a thread of another origin;
    /// `Conflict` when the threads keep changing under the move.
    pub fn refile_threads(
        &self,
        moves: &[(String, Refile)],
        how: &MoveBy,
        fresh: &[String],
    ) -> Result<Refiled> {
        let mut targets: HashMap<String, LivePage> = HashMap::new();
        for (to, _) in moves {
            if !targets.contains_key(to) {
                let p = self
                    .live_page_of(&ArtifactId::parse(to)?)?
                    .ok_or(CoreError::NotFound)?;
                targets.insert(to.clone(), p);
            }
        }
        for _ in 0..REFILE_ATTEMPTS {
            let plans = self.with_read(|c| plan_refile(c, &targets, moves))?;
            if plans.is_empty() {
                return Ok(Refiled::default());
            }
            let mut clips = Vec::new();
            for p in plans
                .iter()
                .filter(|p| p.has_clip && p.from != p.to.artifact_id)
            {
                let to = ArtifactId::parse(&p.to.artifact_id)?;
                let src = self.home.clip_path(&ArtifactId::parse(&p.from)?, &p.tid);
                let dest = self.home.clip_path(&to, &p.tid);
                std::fs::create_dir_all(self.home.clips_dir(&to))?;
                let _ = std::fs::remove_file(&dest);
                match std::fs::copy(&src, &dest).map(|_| ()) {
                    Ok(()) => clips.push((
                        Pending {
                            path: dest,
                            keep: false,
                        },
                        src,
                    )),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let mut renamed = Vec::new();
            let result =
                self.with_tx(|tx| commit_refile(tx, self, &plans, how, fresh, &mut renamed));
            match result {
                Ok(written) => {
                    for (copy, src) in clips {
                        copy.keep();
                        let _ = std::fs::remove_file(src);
                    }
                    let mut versions = Vec::new();
                    for (aid, n) in written {
                        if let Some(v) = self.get_version(&ArtifactId::parse(&aid)?, n)? {
                            versions.push(v);
                        }
                    }
                    let moved = plans
                        .into_iter()
                        .map(|p| Moved {
                            thread_id: p.tid,
                            from: p.from,
                            to: p.to.artifact_id,
                        })
                        .collect();
                    return Ok(Refiled { moved, versions });
                }
                Err(e) => {
                    // Nothing committed: the versions put in place go too.
                    for d in renamed {
                        let _ = std::fs::remove_dir_all(d);
                    }
                    if !matches!(e, CoreError::Conflict { .. }) {
                        return Err(e);
                    }
                }
            }
        }
        Err(CoreError::Conflict { current: 0 })
    }
}

/// [`Store::resolve_live_key`] against `rules`, the origin's in force,
/// oldest first. A rule's own canonical path resolves to that rule's page,
/// whatever other rule matches it too.
pub fn resolve_with(rules: &[LiveRule], key: &PageKey) -> Resolved {
    let own = rules.iter().find(|r| r.pattern == key.path);
    match own.or_else(|| winning_rule(rules, |r| &r.pattern, &key.path)) {
        Some(r) => Resolved {
            key: PageKey {
                origin: key.origin.clone(),
                path: r.pattern.clone(),
            },
            live_path: (key.path != r.pattern).then(|| key.path.clone()),
            rule: Some(r.clone()),
        },
        None => Resolved {
            key: key.clone(),
            live_path: None,
            rule: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{anchor, session, store};
    use crate::store::threads::NewThread;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png-body";
    const ORIGIN: &str = "http://localhost:5173";

    fn key(path: &str) -> PageKey {
        PageKey {
            origin: ORIGIN.into(),
            path: path.into(),
        }
    }

    fn page(st: &Store, path: &str, html: &str) -> ArtifactId {
        let e = st
            .ensure_live_page(&key(path), path, Some(html.as_bytes()))
            .unwrap();
        ArtifactId::parse(&e.artifact.id).unwrap()
    }

    fn new_thread(st: &Store, id: &ArtifactId, route: Option<&str>) -> NewThread {
        let a = st.get_artifact(id).unwrap().unwrap();
        let mut an = anchor();
        an.route = route.map(str::to_string);
        NewThread {
            version_n: a.current_version,
            anchor: an,
            author_name: "Ana".into(),
            author_public_id: None,
            body: "Fix this".into(),
            clip: Some(PNG.to_vec()),
            via_page: false,
        }
    }

    fn thread(st: &Store, id: &ArtifactId, route: Option<&str>) -> Thread {
        st.create_thread(id, new_thread(st, id, route)).unwrap()
    }

    fn index(st: &Store, id: &ArtifactId, n: u32) -> String {
        std::fs::read_to_string(st.home().version_dir(id, n).join(INDEX)).unwrap()
    }

    fn to(id: &ArtifactId, tid: &str) -> (String, Refile) {
        (
            id.to_string(),
            Refile {
                thread_id: tid.into(),
                live_path: None,
                route: None,
            },
        )
    }

    fn by(kind: &'static str) -> MoveBy {
        MoveBy {
            by: "viewer:u_x".into(),
            kind,
            rule_id: None,
        }
    }

    fn rule(st: &Store, pattern: &str) -> LiveRule {
        st.add_live_rule(ORIGIN, &PathPattern::parse(pattern).unwrap())
            .unwrap()
            .0
    }

    fn watchers(st: &Store, id: &ArtifactId) -> Vec<String> {
        let mut w: Vec<String> = st
            .watchers(id)
            .unwrap()
            .into_iter()
            .map(|w| w.session_id)
            .collect();
        w.sort();
        w
    }

    #[test]
    fn a_moved_thread_takes_its_snapshots_links_address_clip_and_agents() {
        let (_d, st) = store();
        let a = page(&st, "/a", "<p>a1");
        let b = page(&st, "/b", "<p>b1");
        let watcher = session(&st, "claude", "w");
        let target = session(&st, "claude", "t");
        st.watch(&watcher, &a, true).unwrap();
        let t = thread(&st, &a, Some("?x=1"));
        // An address linked to a later snapshot of /a, and one pending.
        st.mark_pending(&a, &t.id, "explicit", "claude").unwrap();
        st.ensure_live_page_linking(
            &key("/a"),
            "/a",
            Some(b"<p>a2"),
            std::slice::from_ref(&t.id),
        )
        .unwrap();
        st.mark_pending(&a, &t.id, "explicit", "claude").unwrap();
        st.with_write(|c| {
            c.execute(
                "UPDATE threads SET target_session_id = ?2 WHERE id = ?1",
                params![t.id, target],
            )?;
            Ok(())
        })
        .unwrap();
        let old_clip = st.home().clip_path(&a, &t.id);

        let done = st
            .refile_threads(&[to(&b, &t.id)], &by(KIND_MOVE), &[])
            .unwrap();
        assert_eq!(
            done.moved,
            vec![Moved {
                thread_id: t.id.clone(),
                from: a.to_string(),
                to: b.to_string()
            }]
        );
        // /a's version 1 (the thread's) and 2 (its address), then /b's own
        // version 1 again, keeping its note.
        let ns: Vec<(u32, Option<String>)> = done
            .versions
            .iter()
            .map(|v| (v.n, v.note.clone()))
            .collect();
        assert_eq!(
            ns,
            vec![
                (2, Some(MOVED_NOTE.into())),
                (3, Some(MOVED_NOTE.into())),
                (4, Some("snapshot".into()))
            ]
        );
        assert_eq!(index(&st, &b, 2), "<p>a1");
        assert_eq!(index(&st, &b, 3), "<p>a2");
        assert_eq!(index(&st, &b, 4), "<p>b1");
        assert_eq!(st.get_artifact(&b).unwrap().unwrap().current_version, 4);
        let moved = st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(moved.artifact_id, b.as_str());
        assert_eq!(moved.version_n, 2);
        assert_eq!(moved.anchor.route, None);
        assert_eq!(moved.comments, t.comments);
        let x = st
            .thread_extras(std::slice::from_ref(&moved), false)
            .unwrap();
        assert_eq!(x[0].addressed_in, vec![3]);
        assert!(x[0].addressed_pending.is_some());
        assert!(st.has_pending(&b).unwrap() && !st.has_pending(&a).unwrap());
        let m = &x[0].moves[0];
        assert_eq!(
            (m.from_url.as_str(), m.to_url.as_str(), m.kind.as_str()),
            (
                "http://localhost:5173/a?x=1",
                "http://localhost:5173/b",
                "move"
            )
        );
        assert!(!old_clip.exists());
        assert_eq!(std::fs::read(st.home().clip_path(&b, &t.id)).unwrap(), PNG);
        let mut want = vec![watcher.clone(), target.clone()];
        want.sort();
        assert_eq!(watchers(&st, &b), want, "its agents follow it");

        // Moving it again to where it is changes nothing.
        let again = st
            .refile_threads(&[to(&b, &t.id)], &by(KIND_MOVE), &[])
            .unwrap();
        assert!(again.moved.is_empty() && again.versions.is_empty());
        // Deleting the page it left keeps it whole.
        st.delete_artifact(&a).unwrap();
        let kept = st.get_thread(&t.id).unwrap().unwrap();
        let x = st
            .thread_extras(std::slice::from_ref(&kept), false)
            .unwrap();
        assert_eq!((x[0].moves.len(), x[0].addressed_in.clone()), (1, vec![3]));
        assert_eq!(index(&st, &b, 2), "<p>a1");
        st.delete_thread(&t.id).unwrap();
    }

    #[test]
    fn a_move_reuses_versions_the_page_already_holds() {
        let (_d, st) = store();
        let a = page(&st, "/a", "<p>a");
        let b = page(&st, "/b", "<p>b");
        let t = thread(&st, &a, None);
        let there = st
            .refile_threads(&[to(&b, &t.id)], &by(KIND_MOVE), &[])
            .unwrap();
        assert_eq!(there.versions.len(), 2, "a copy, and /b's own on top");
        // Back to /a: its version 1 has the same bytes; nothing is written.
        let back = st
            .refile_threads(&[to(&a, &t.id)], &by(KIND_MOVE), &[])
            .unwrap();
        assert!(back.versions.is_empty());
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap().version_n, 1);
        assert_eq!(st.get_artifact(&a).unwrap().unwrap().current_version, 1);
        // And to /b again: its copy is reused, and its current version
        // stays.
        let again = st
            .refile_threads(&[to(&b, &t.id)], &by(KIND_MOVE), &[])
            .unwrap();
        assert!(again.versions.is_empty());
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap().version_n, 2);
        assert_eq!(st.get_artifact(&b).unwrap().unwrap().current_version, 3);
    }

    #[test]
    fn a_failed_refiling_writes_nothing() {
        let (_d, st) = store();
        let a = page(&st, "/a", "<p>a");
        let c = page(&st, "/c", "<p>c");
        let b = page(&st, "/b", "<p>b");
        let t1 = thread(&st, &a, None);
        let t2 = thread(&st, &c, None);
        // /c's snapshot is gone from disk: its link fails mid-way.
        std::fs::remove_file(st.home().version_dir(&c, 1).join(INDEX)).unwrap();
        let err = st.refile_threads(&[to(&b, &t1.id), to(&b, &t2.id)], &by(KIND_MOVE), &[]);
        assert!(err.is_err());
        assert_eq!(st.get_artifact(&b).unwrap().unwrap().current_version, 1);
        assert_eq!(st.list_versions(&b).unwrap().len(), 1);
        let dirs = std::fs::read_dir(st.home().artifact_dir(&b).join("versions"))
            .unwrap()
            .count();
        assert_eq!(dirs, 1, "no version directory left behind");
        assert_eq!(
            st.get_thread(&t1.id).unwrap().unwrap().artifact_id,
            a.as_str()
        );
    }

    #[test]
    fn a_thread_moves_only_within_its_origin_and_only_from_a_live_page() {
        let (_d, st) = store();
        let a = page(&st, "/a", "<p>a");
        let t = thread(&st, &a, None);
        let other = st
            .ensure_live_page(
                &PageKey {
                    origin: "http://localhost:3000".into(),
                    path: "/".into(),
                },
                "o",
                None,
            )
            .unwrap();
        let other = ArtifactId::parse(&other.artifact.id).unwrap();
        let err = st
            .refile_threads(&[to(&other, &t.id)], &by(KIND_MOVE), &[])
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid { code, .. } if code == "cross_origin"));
        let html = crate::store::test_util::artifact(&st, None);
        let h = st
            .create_thread(
                &html,
                NewThread {
                    version_n: 1,
                    anchor: anchor(),
                    author_name: "Ana".into(),
                    author_public_id: None,
                    body: "x".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        let err = st
            .refile_threads(&[to(&a, &h.id)], &by(KIND_MOVE), &[])
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid { code, .. } if code == "not_live"));
        assert!(matches!(
            st.refile_threads(&[to(&a, "nope")], &by(KIND_MOVE), &[]),
            Err(CoreError::NotFound)
        ));
        assert_eq!(st.get_artifact(&other).unwrap().unwrap().current_version, 1);
    }

    #[test]
    fn rules_map_lookups_merge_in_batches_and_unmerge() {
        let (_d, st) = store();
        let one = page(&st, "/users/1", "<p>1");
        let two = page(&st, "/users/2", "<p>2");
        let t1 = thread(&st, &one, None);
        let t2 = thread(&st, &two, Some("#/tab"));
        let all = rule(&st, "/users/*");
        let r = st.resolve_live_key(&key("/users/9/x")).unwrap();
        assert_eq!(
            (r.key, r.live_path.as_deref()),
            (key("/users/*"), Some("/users/9/x"))
        );
        assert_eq!(r.rule.as_ref(), Some(&all));
        // A more specific rule does not capture another rule's own path.
        let id = rule(&st, "/users/:id");
        assert_eq!(
            st.resolve_live_key(&key("/users/*")).unwrap().rule,
            Some(all.clone())
        );
        assert_eq!(
            st.resolve_live_key(&key("/users/3")).unwrap().rule,
            Some(id.clone())
        );

        let (batch, remaining) = st.merge_candidates(&id, 1).unwrap();
        assert_eq!((batch.len(), remaining), (1, 1));
        assert_eq!(batch[0].thread_id, t1.id);
        assert_eq!(batch[0].live_path.as_deref(), Some("/users/1"));
        let canon = st.ensure_live_page(&key("/users/:id"), "U", None).unwrap();
        let canon_id = ArtifactId::parse(&canon.artifact.id).unwrap();
        let merge = MoveBy {
            by: "viewer:u_x".into(),
            kind: KIND_MERGE,
            rule_id: Some(id.id.clone()),
        };
        let moves = |b: Vec<Refile>| -> Vec<(String, Refile)> {
            b.into_iter().map(|r| (canon_id.to_string(), r)).collect()
        };
        let fresh = [canon_id.to_string()];
        st.refile_threads(&moves(batch), &merge, &fresh).unwrap();
        let (batch, remaining) = st.merge_candidates(&id, 1).unwrap();
        assert_eq!((batch.len(), remaining), (1, 0));
        assert_eq!(batch[0].route.as_deref(), Some("#/tab"));
        st.refile_threads(&moves(batch), &merge, &fresh).unwrap();
        assert!(st.merge_candidates(&id, 1).unwrap().0.is_empty());
        // The placeholder of the page made for the merge stays below.
        let v = st.list_versions(&canon_id).unwrap();
        assert!(
            v.iter()
                .skip(1)
                .all(|v| v.note.as_deref() == Some(MOVED_NOTE))
        );

        // Deleting the rule: lookups stop mapping, then the threads go back.
        let gone = st.mark_rule_deleted(&id.id).unwrap().unwrap();
        assert!(gone.deleting);
        assert_eq!(
            st.resolve_live_key(&key("/users/3")).unwrap().rule,
            Some(all.clone())
        );
        let (back, remaining) = st.unmerge_candidates(&gone, 10).unwrap();
        assert_eq!(remaining, 0);
        let mut targets: Vec<String> = back.iter().map(|(k, _)| k.path.clone()).collect();
        targets.sort();
        // `/users/*` still maps them: they go to its canonical page.
        assert_eq!(targets, vec!["/users/*", "/users/*"]);
        st.drop_rule(&id.id).unwrap();
        assert_eq!(st.live_rule(&id.id).unwrap(), None);
        // Without any rule, a thread goes back to its own path's page.
        st.mark_rule_deleted(&all.id).unwrap();
        let (back, _) = st.unmerge_candidates(&all, 10).unwrap();
        assert!(back.is_empty(), "/users/* merged nothing itself");
        let (back, _) = st.unmerge_candidates(&gone, 10).unwrap();
        let mut targets: Vec<String> = back.iter().map(|(k, _)| k.path.clone()).collect();
        targets.sort();
        assert_eq!(targets, vec!["/users/1", "/users/2"]);
        let unmerge = MoveBy {
            by: "viewer:u_x".into(),
            kind: KIND_UNMERGE,
            rule_id: Some(id.id.clone()),
        };
        let moves: Vec<(String, Refile)> = back
            .into_iter()
            .map(|(k, r)| (st.find_live_page(&k).unwrap().unwrap().artifact_id, r))
            .collect();
        st.refile_threads(&moves, &unmerge, &[]).unwrap();
        assert_eq!(
            st.get_thread(&t1.id).unwrap().unwrap().artifact_id,
            one.as_str()
        );
        let x = st
            .thread_extras(&[st.get_thread(&t2.id).unwrap().unwrap()], false)
            .unwrap();
        assert_eq!(x[0].live_path, None);
        assert_eq!(x[0].moves.last().unwrap().kind, KIND_UNMERGE);
        assert!(st.unmerge_candidates(&gone, 10).unwrap().0.is_empty());
        // Adding a deleted rule again puts it back in force.
        let (again, new) = st
            .add_live_rule(ORIGIN, &PathPattern::parse("/users/*").unwrap())
            .unwrap();
        assert!(!new && !again.deleting && again.id == all.id);
    }

    #[test]
    fn scope_watches_cover_a_merged_page_by_its_threads_paths() {
        let (_d, st) = store();
        let one = session(&st, "claude", "one");
        let two = session(&st, "claude", "two");
        st.live_watch(&one, &key("/users/1"), true).unwrap();
        let canon = st.ensure_live_page(&key("/users/:id"), "U", None).unwrap();
        let canon = ArtifactId::parse(&canon.artifact.id).unwrap();
        assert!(watchers(&st, &canon).is_empty());
        st.create_live_thread(
            &canon,
            new_thread(&st, &canon, None),
            None,
            Some("/users/1"),
        )
        .unwrap();
        assert_eq!(watchers(&st, &canon), vec![one.clone()]);
        // A scope made later covers it by the same path; removing it too.
        let (_, covered) = st.live_watch(&two, &key("/users/1"), true).unwrap();
        assert_eq!(covered, vec![canon.to_string()]);
        assert_eq!(
            st.live_unwatch(&two, &key("/users/1")).unwrap(),
            vec![canon.to_string()]
        );
        assert_eq!(watchers(&st, &canon), vec![one]);
        // Pending addresses settle only for threads made at the path.
        let ids: Vec<String> = st
            .list_threads(&canon, true, None, 10)
            .unwrap()
            .0
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(st.pending_at_path(&ids, ORIGIN, "/users/1").unwrap(), ids);
        assert!(
            st.pending_at_path(&ids, ORIGIN, "/users/2")
                .unwrap()
                .is_empty()
        );
        assert!(
            st.pending_at_path(&ids, "http://x", "/users/1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_origin_holds_a_bounded_number_of_rules() {
        let (_d, st) = store();
        for i in 0..MAX_RULES {
            st.add_live_rule(
                "http://x",
                &PathPattern::parse(&format!("/r{i}/:id")).unwrap(),
            )
            .unwrap();
        }
        let err = st
            .add_live_rule("http://x", &PathPattern::parse("/last/:id").unwrap())
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid { code, .. } if code == "too_many_rules"));
        st.add_live_rule("http://y", &PathPattern::parse("/last/:id").unwrap())
            .unwrap();
    }
}

//! Participants and each viewer's attention per artifact (spec §10,
//! "Participants and attention"). Looked-at marks and attention are the
//! viewer's own: nothing here is served to anyone else.

use super::Store;
use crate::{ArtifactId, Result};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Person {
    pub public_id: String,
    pub display_name: Option<String>,
    /// The latest version this person viewed at the artifact's latest URL. Public (spec §14).
    pub seen: Option<u32>,
}
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct AgentView {
    pub handle: String,
    pub harness: String,
    pub live: bool,
}
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct Participants {
    pub people: Vec<Person>,
    pub agents: Vec<AgentView>,
}
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct AttentionSummary {
    pub addressed: Vec<String>,
    pub addressed_v: Option<u32>,
    pub new_replies: Vec<String>,
    pub open_in: Vec<String>,
    pub seen: Option<u32>,
}
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct Attention {
    #[serde(flatten)]
    pub summary: AttentionSummary,
    pub looked: BTreeMap<String, String>,
}

/// Most agents a participant list names.
pub const MAX_AGENTS: usize = 10;
/// Most threads one looked-at write may name.
pub const MAX_LOOKED: usize = 50;

/// The artifacts a set-based query covers: one artifact (`:aid`, whether
/// live or not) or every live artifact. Each yields `arts(id, owner)`.
macro_rules! arts_one {
    () => {
        "SELECT id, owner_session_id FROM artifacts WHERE id = :aid"
    };
}
macro_rules! arts_live {
    () => {
        "SELECT id, owner_session_id FROM artifacts WHERE deleted_at IS NULL AND current_version > 0"
    };
}

/// Per-thread attention rows for the threads of `arts` the viewer (cookie
/// `:viewer`, public ID `:pid`) is in: wrote a comment, is mentioned, or
/// resolved it. Columns: artifact, thread, open, the highest version
/// addressing it since the viewer's last look (`NULL` when none), and
/// whether someone else commented since that look. Ordered by artifact,
/// then thread age.
///
/// The join order is fixed (`CROSS JOIN`) and the unary `+` keeps
/// `comments_by_author` and `mentions_by_viewer` out of the per-thread
/// subqueries, which reach comments through `comments_by_thread` only.
macro_rules! attention_sql {
    ($arts:expr) => {
        concat!(
            "WITH arts(id, owner) AS (", $arts, ")
            SELECT t.artifact_id, t.id, t.status = 'open',
                   (SELECT MAX(vt.version_n) FROM version_threads vt
                     WHERE vt.thread_id = t.id AND vt.created_at >= COALESCE(l.looked_at, '')),
                   EXISTS (SELECT 1 FROM comments r
                            WHERE r.thread_id = t.id AND r.created_at > COALESCE(l.looked_at, '')
                              AND (+r.author_public_id IS NULL OR +r.author_public_id != :pid))
              FROM arts a
             CROSS JOIN threads t ON t.artifact_id = a.id
              LEFT JOIN viewer_threads l ON l.viewer_id = :viewer AND l.thread_id = t.id
             WHERE EXISTS (SELECT 1 FROM comments c WHERE c.thread_id = t.id AND +c.author_public_id = :pid)
                OR EXISTS (SELECT 1 FROM comments c CROSS JOIN mentions m ON m.comment_id = c.id
                            WHERE c.thread_id = t.id AND +m.public_id = :pid)
                OR t.resolved_by = 'viewer:' || :pid
             ORDER BY t.artifact_id, t.created_at, t.id"
        )
    };
}

/// People of `arts`: each viewer who wrote a comment on one of its threads,
/// with the latest version they viewed there. Ordered by artifact, then by
/// when the viewer first appeared.
macro_rules! people_sql {
    ($arts:expr) => {
        concat!(
            "WITH arts(id, owner) AS (",
            $arts,
            ")
            SELECT x.aid, v.public_id, v.display_name, s.seen_n
              FROM (SELECT DISTINCT a.id AS aid, c.author_public_id AS p
                      FROM arts a
                     CROSS JOIN threads t ON t.artifact_id = a.id
                     CROSS JOIN comments c ON c.thread_id = t.id
                     WHERE +c.author_public_id IS NOT NULL) x
             CROSS JOIN viewers v INDEXED BY viewers_public_id ON v.public_id = x.p
              LEFT JOIN viewer_seen s ON s.viewer_id = v.id AND s.artifact_id = x.aid
             ORDER BY x.aid, v.created_at"
        )
    };
}

/// Agents of `arts`: its owner, its watchers and the sessions that published
/// a version of it, at most `:limit` per artifact. `live`: a send can reach
/// it (live, and the owner or a watcher), as `live_agent` checks. Activity:
/// the newest of its versions, its comments on the artifact's threads and its
/// watch, else its registration. SQLite's many-argument MAX is NULL when any
/// argument is, hence the COALESCEs. Ordered by artifact, then live first,
/// most recently active first.
macro_rules! agents_sql {
    ($arts:expr) => {
        concat!(
            "WITH arts(id, owner) AS (", $arts, "),
            rel(aid, owner, sid) AS (
                SELECT a.id, a.owner, a.owner FROM arts a WHERE a.owner IS NOT NULL
                UNION SELECT a.id, a.owner, w.session_id FROM arts a CROSS JOIN watches w ON w.artifact_id = a.id
                UNION SELECT a.id, a.owner, v.session_id FROM arts a CROSS JOIN versions v ON v.artifact_id = a.id
                       WHERE v.session_id IS NOT NULL),
            ranked AS (
                SELECT k.aid, s.id, s.agent_handle, s.harness,
                       (s.ended_at IS NULL AND (s.id IS k.owner OR EXISTS
                          (SELECT 1 FROM watches w WHERE w.session_id = s.id AND w.artifact_id = k.aid))) AS live,
                       MAX(COALESCE((SELECT MAX(v.created_at) FROM versions v WHERE v.artifact_id = k.aid AND v.session_id = s.id), ''),
                           COALESCE((SELECT MAX(c.created_at) FROM threads t CROSS JOIN comments c ON c.thread_id = t.id
                                      WHERE t.artifact_id = k.aid AND +c.via_session_id = s.id), ''),
                           COALESCE((SELECT w.created_at FROM watches w WHERE w.session_id = s.id AND w.artifact_id = k.aid), ''),
                           s.started_at) AS active_at
                  FROM rel k CROSS JOIN sessions s ON s.id = k.sid)
            SELECT aid, agent_handle, harness, live FROM
              (SELECT aid, agent_handle, harness, live,
                      ROW_NUMBER() OVER (PARTITION BY aid ORDER BY live DESC, active_at DESC, id) AS rn
                 FROM ranked)
             WHERE rn <= :limit ORDER BY aid, rn"
        )
    };
}

pub(crate) const ATTENTION_ONE: &str = attention_sql!(arts_one!());
pub(crate) const ATTENTION_LIVE: &str = attention_sql!(arts_live!());
pub(crate) const PEOPLE_ONE: &str = people_sql!(arts_one!());
pub(crate) const PEOPLE_LIVE: &str = people_sql!(arts_live!());
pub(crate) const AGENTS_ONE: &str = agents_sql!(arts_one!());
pub(crate) const AGENTS_LIVE: &str = agents_sql!(arts_live!());
/// The viewer's looked-at marks on the threads of `:aid`.
pub(crate) const LOOKED_ONE: &str = "SELECT vt.thread_id, vt.looked_at FROM threads t
    CROSS JOIN viewer_threads vt ON vt.viewer_id = :viewer AND vt.thread_id = t.id
    WHERE t.artifact_id = :aid";

impl Store {
    pub fn participants(&self, aid: &ArtifactId) -> Result<Participants> {
        self.with_read(|c| {
            Ok(participants_in(c, Some(aid.as_str()))?
                .remove(aid.as_str())
                .unwrap_or_default())
        })
    }

    /// [`Store::participants`] of every live artifact, by artifact ID; an
    /// artifact with neither people nor agents may be missing.
    pub fn participants_all(&self) -> Result<BTreeMap<String, Participants>> {
        self.with_read(|c| participants_in(c, None))
    }

    /// The live owner or watcher of `aid` whose handle is `handle`, as a session ID.
    pub fn live_agent(&self, aid: &ArtifactId, handle: &str) -> Result<Option<String>> {
        self.with_read(|c| Ok(c.query_row(
            "SELECT id FROM sessions WHERE agent_handle = ?2 AND ended_at IS NULL AND
               (id = (SELECT owner_session_id FROM artifacts WHERE id = ?1) OR id IN (SELECT session_id FROM watches WHERE artifact_id = ?1))",
            params![aid.as_str(), handle], |r| r.get(0)).optional()?))
    }

    /// Records that the viewer looked at `thread_ids` (threads of other
    /// artifacts are ignored) now; answers the viewer's marks on `aid`.
    pub fn mark_looked(
        &self,
        viewer_id: &str,
        aid: &ArtifactId,
        thread_ids: &[String],
    ) -> Result<BTreeMap<String, String>> {
        let now = Store::now();
        self.with_tx(|tx| {
            for tid in thread_ids.iter().take(MAX_LOOKED) {
                tx.execute(
                    "INSERT INTO viewer_threads (viewer_id, thread_id, looked_at)
                     SELECT ?1, id, ?3 FROM threads WHERE id = ?2 AND artifact_id = ?4
                     ON CONFLICT (viewer_id, thread_id) DO UPDATE SET looked_at = excluded.looked_at",
                    params![viewer_id, tid, now, aid.as_str()],
                )?;
            }
            looked_in(tx, viewer_id, aid.as_str())
        })
    }

    pub fn attention(&self, viewer_id: &str, aid: &ArtifactId) -> Result<Attention> {
        self.with_read(|c| {
            let Some(p) = public_id_of(c, viewer_id)? else {
                return Ok(Attention::default());
            };
            let looked = looked_in(c, viewer_id, aid.as_str())?;
            let summary = summaries_in(c, viewer_id, &p, Some(aid.as_str()))?
                .remove(aid.as_str())
                .unwrap_or_default();
            Ok(Attention { summary, looked })
        })
    }

    /// The viewer's attention summary on every live artifact, by artifact ID.
    pub fn attention_all(&self, viewer_id: &str) -> Result<BTreeMap<String, AttentionSummary>> {
        let ids: Vec<String> = self.list_artifacts()?.into_iter().map(|a| a.id).collect();
        let mut found = self.with_read(|c| match public_id_of(c, viewer_id)? {
            Some(p) => summaries_in(c, viewer_id, &p, None),
            None => Ok(BTreeMap::new()),
        })?;
        Ok(ids
            .into_iter()
            .map(|id| {
                let s = found.remove(&id).unwrap_or_default();
                (id, s)
            })
            .collect())
    }
}

fn public_id_of(c: &Connection, viewer_id: &str) -> Result<Option<String>> {
    Ok(
        c.prepare_cached("SELECT public_id FROM viewers WHERE id = ?1")?
            .query_row(params![viewer_id], |r| r.get(0))
            .optional()?,
    )
}

fn looked_in(c: &Connection, viewer_id: &str, aid: &str) -> Result<BTreeMap<String, String>> {
    let mut st = c.prepare_cached(LOOKED_ONE)?;
    let rows = st.query_map(named_params! {":viewer": viewer_id, ":aid": aid}, |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Attention summaries of `aid` (or of every live artifact), by artifact ID.
/// Artifacts the viewer has neither threads nor a seen mark on are missing.
fn summaries_in(
    c: &Connection,
    viewer_id: &str,
    public_id: &str,
    aid: Option<&str>,
) -> Result<BTreeMap<String, AttentionSummary>> {
    let mut out: BTreeMap<String, AttentionSummary> = BTreeMap::new();
    let mut add = |r: &rusqlite::Row<'_>| -> rusqlite::Result<()> {
        let (artifact, tid): (String, String) = (r.get(0)?, r.get(1)?);
        let (open, addressed_v, replied): (bool, Option<u32>, bool) =
            (r.get(2)?, r.get(3)?, r.get(4)?);
        let s = out.entry(artifact).or_default();
        if open {
            s.open_in.push(tid.clone());
            if addressed_v.is_some() {
                s.addressed_v = s.addressed_v.max(addressed_v);
                s.addressed.push(tid.clone());
            }
        }
        if replied {
            s.new_replies.push(tid);
        }
        Ok(())
    };
    match aid {
        Some(aid) => {
            let mut st = c.prepare_cached(ATTENTION_ONE)?;
            let mut rows =
                st.query(named_params! {":aid": aid, ":viewer": viewer_id, ":pid": public_id})?;
            while let Some(r) = rows.next()? {
                add(r)?;
            }
            let seen: Option<u32> = c
                .prepare_cached(
                    "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                )?
                .query_row(params![viewer_id, aid], |r| r.get(0))
                .optional()?;
            if let Some(n) = seen {
                out.entry(aid.to_string()).or_default().seen = Some(n);
            }
        }
        None => {
            let mut st = c.prepare_cached(ATTENTION_LIVE)?;
            let mut rows = st.query(named_params! {":viewer": viewer_id, ":pid": public_id})?;
            while let Some(r) = rows.next()? {
                add(r)?;
            }
            let mut st = c.prepare_cached(
                "SELECT artifact_id, seen_n FROM viewer_seen WHERE viewer_id = ?1",
            )?;
            let mut rows = st.query(params![viewer_id])?;
            while let Some(r) = rows.next()? {
                out.entry(r.get(0)?).or_default().seen = Some(r.get(1)?);
            }
        }
    }
    Ok(out)
}

/// Participants of `aid` (or of every live artifact), by artifact ID.
fn participants_in(c: &Connection, aid: Option<&str>) -> Result<BTreeMap<String, Participants>> {
    let mut out: BTreeMap<String, Participants> = BTreeMap::new();
    let limit = MAX_AGENTS as i64;
    let (people, agents) = match aid {
        Some(_) => (PEOPLE_ONE, AGENTS_ONE),
        None => (PEOPLE_LIVE, AGENTS_LIVE),
    };
    let mut st = c.prepare_cached(people)?;
    let mut rows = match aid {
        Some(aid) => st.query(named_params! {":aid": aid})?,
        None => st.query([])?,
    };
    while let Some(r) = rows.next()? {
        out.entry(r.get(0)?).or_default().people.push(Person {
            public_id: r.get(1)?,
            display_name: r.get(2)?,
            seen: r.get(3)?,
        });
    }
    drop(rows);
    let mut st = c.prepare_cached(agents)?;
    let mut rows = match aid {
        Some(aid) => st.query(named_params! {":aid": aid, ":limit": limit})?,
        None => st.query(named_params! {":limit": limit})?,
    };
    while let Some(r) = rows.next()? {
        out.entry(r.get(0)?).or_default().agents.push(AgentView {
            handle: r.get(1)?,
            harness: r.get(2)?,
            live: r.get(3)?,
        });
    }
    Ok(out)
}

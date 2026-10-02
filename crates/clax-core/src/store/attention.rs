//! Participants and each viewer's attention per artifact (spec §10,
//! "Participants and attention"). Looked-at marks and attention are the
//! viewer's own: nothing here is served to anyone else.

use super::Store;
use crate::{ArtifactId, Result};
use rusqlite::{OptionalExtension, params};
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

/// Threads of `?1` (artifact) the viewer with public ID `?2` is in: wrote a
/// comment, is mentioned, or resolved it.
const IN_THREAD: &str = "SELECT t.id, t.status FROM threads t WHERE t.artifact_id = ?1 AND (
    EXISTS (SELECT 1 FROM comments c WHERE c.thread_id = t.id AND c.author_public_id = ?2)
    OR EXISTS (SELECT 1 FROM mentions m JOIN comments c ON c.id = m.comment_id WHERE c.thread_id = t.id AND m.public_id = ?2)
    OR t.resolved_by = 'viewer:' || ?2) ORDER BY t.created_at, t.id";

impl Store {
    pub fn participants(&self, aid: &ArtifactId) -> Result<Participants> {
        self.with_conn(|c| {
            // `seen` is public by design (decided: Q7); looked-at marks are not read here.
            let people = c.prepare(
                "SELECT v.public_id, v.display_name, s.seen_n FROM viewers v
                   LEFT JOIN viewer_seen s ON s.viewer_id = v.id AND s.artifact_id = ?1
                  WHERE v.public_id IN
                   (SELECT c.author_public_id FROM comments c JOIN threads t ON t.id = c.thread_id WHERE t.artifact_id = ?1 AND c.author_public_id IS NOT NULL)
                 ORDER BY v.created_at",
            )?.query_map(params![aid.as_str()], |r| Ok(Person { public_id: r.get(0)?, display_name: r.get(1)?, seen: r.get(2)? }))?
              .collect::<rusqlite::Result<Vec<_>>>()?;
            // `live`: a send can reach it (live, and the owner or a watcher), as `live_agent` checks.
            // Activity: the newest of its versions, its comments on this artifact's threads and its
            // watch, else its registration. SQLite's many-argument MAX is NULL when any argument is,
            // hence the COALESCEs.
            let agents = c.prepare(
                "SELECT s.agent_handle, s.harness,
                        (s.ended_at IS NULL AND (s.id = (SELECT owner_session_id FROM artifacts WHERE id = ?1)
                           OR s.id IN (SELECT session_id FROM watches WHERE artifact_id = ?1))) AS live,
                        MAX(COALESCE((SELECT MAX(created_at) FROM versions WHERE session_id = s.id AND artifact_id = ?1), ''),
                            COALESCE((SELECT MAX(c.created_at) FROM comments c JOIN threads t ON t.id = c.thread_id
                                       WHERE c.via_session_id = s.id AND t.artifact_id = ?1), ''),
                            COALESCE((SELECT created_at FROM watches WHERE session_id = s.id AND artifact_id = ?1), ''),
                            s.started_at) AS active_at
                   FROM sessions s
                  WHERE s.id = (SELECT owner_session_id FROM artifacts WHERE id = ?1)
                     OR s.id IN (SELECT session_id FROM watches WHERE artifact_id = ?1)
                     OR s.id IN (SELECT session_id FROM versions WHERE artifact_id = ?1)
                  ORDER BY live DESC, active_at DESC, s.id LIMIT ?2",
            )?.query_map(params![aid.as_str(), MAX_AGENTS as i64], |r| Ok(AgentView { handle: r.get(0)?, harness: r.get(1)?, live: r.get(2)? }))?
              .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Participants { people, agents })
        })
    }

    /// The live owner or watcher of `aid` whose handle is `handle`, as a session ID.
    pub fn live_agent(&self, aid: &ArtifactId, handle: &str) -> Result<Option<String>> {
        self.with_conn(|c| Ok(c.query_row(
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
        self.with_conn(|c| {
            let public_id: Option<String> = c
                .query_row(
                    "SELECT public_id FROM viewers WHERE id = ?1",
                    params![viewer_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(p) = public_id else {
                return Ok(Attention::default());
            };
            let looked = looked_in(c, viewer_id, aid.as_str())?;
            let summary = summary(c, viewer_id, &p, aid.as_str(), &looked)?;
            Ok(Attention { summary, looked })
        })
    }

    pub fn attention_all(&self, viewer_id: &str) -> Result<BTreeMap<String, AttentionSummary>> {
        let ids: Vec<String> = self.list_artifacts()?.into_iter().map(|a| a.id).collect();
        let mut out = BTreeMap::new();
        for id in ids {
            let a = self.attention(viewer_id, &ArtifactId::parse(&id)?)?;
            out.insert(id, a.summary);
        }
        Ok(out)
    }
}

fn looked_in(
    c: &rusqlite::Connection,
    viewer_id: &str,
    aid: &str,
) -> Result<BTreeMap<String, String>> {
    let mut st = c.prepare("SELECT vt.thread_id, vt.looked_at FROM viewer_threads vt JOIN threads t ON t.id = vt.thread_id WHERE vt.viewer_id = ?1 AND t.artifact_id = ?2")?;
    Ok(st
        .query_map(params![viewer_id, aid], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn summary(
    c: &rusqlite::Connection,
    viewer_id: &str,
    public_id: &str,
    aid: &str,
    looked: &BTreeMap<String, String>,
) -> Result<AttentionSummary> {
    let mut s = AttentionSummary::default();
    let in_threads: Vec<(String, String)> = c
        .prepare(IN_THREAD)?
        .query_map(params![aid, public_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (tid, status) in in_threads {
        let since = looked.get(&tid).map(String::as_str).unwrap_or("");
        let open = status == "open";
        if open {
            s.open_in.push(tid.clone());
        }
        let addressed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM version_threads WHERE thread_id = ?1 AND created_at >= ?2)", params![tid, since], |r| r.get(0))?;
        if open && addressed {
            let v: Option<u32> = c.query_row("SELECT MAX(version_n) FROM version_threads WHERE thread_id = ?1 AND created_at >= ?2", params![tid, since], |r| r.get(0))?;
            s.addressed_v = s.addressed_v.max(v);
            s.addressed.push(tid.clone());
        }
        let replied: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM comments WHERE thread_id = ?1 AND created_at > ?2 AND (author_public_id IS NULL OR author_public_id != ?3))",
            params![tid, since, public_id], |r| r.get(0))?;
        if replied {
            s.new_replies.push(tid);
        }
    }
    s.seen = c
        .query_row(
            "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
            params![viewer_id, aid],
            |r| r.get(0),
        )
        .optional()?;
    Ok(s)
}

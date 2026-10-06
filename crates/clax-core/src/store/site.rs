//! Site-wide threads of live pages (spec 2026-10-05-chrome-overlay-design
//! §7.1): every live page of an origin with its threads, the merge rules
//! that map paths to one canonical page, and moving threads between pages.

use super::Store;
use super::live::{LivePage, PAGES_OF_ORIGIN, SNAPSHOT_ATTEMPTS, snapshot_publish_noted};
use super::threads::threads_of_many;
use crate::anchor::Anchor;
use crate::live::{PageKey, PathPattern, winning_rule};
use crate::model::{Thread, Version};
use crate::publish::INDEX;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Merge rules one origin may hold.
pub const MAX_RULES: usize = 64;
/// How many times [`Store::refile_threads`] plans again when a thread
/// changes under it.
const REFILE_ATTEMPTS: u32 = 4;
/// The note of a version a move copied to its new page.
pub const MOVED_NOTE: &str = "moved";

/// A merge rule: the live pages of `origin` whose path `pattern` matches
/// are one page, the canonical page whose path is `pattern`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveRule {
    pub id: String,
    pub origin: String,
    pub pattern: String,
    pub created_at: String,
}

/// The rules of origin `?1`, oldest first.
pub(crate) const RULES_OF_ORIGIN: &str = "SELECT id, origin, pattern, created_at FROM live_rules
    WHERE origin = ?1 ORDER BY created_at, id";
/// The live pages of origin `?1` with their title and current version.
pub(crate) const SITE_PAGES: &str =
    "SELECT p.artifact_id, p.origin, p.path, a.title, a.current_version
    FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
    WHERE p.origin = ?1 AND a.deleted_at IS NULL";

fn row_to_rule(r: &rusqlite::Row<'_>) -> rusqlite::Result<LiveRule> {
    Ok(LiveRule {
        id: r.get(0)?,
        origin: r.get(1)?,
        pattern: r.get(2)?,
        created_at: r.get(3)?,
    })
}

fn rules_in(c: &Connection, origin: &str) -> Result<Vec<LiveRule>> {
    let mut st = c.prepare_cached(RULES_OF_ORIGIN)?;
    let rules = st
        .query_map(params![origin], row_to_rule)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rules)
}

/// Where a page key's comments go: the canonical page of the rule that maps
/// it, or the key itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The live page's key.
    pub key: PageKey,
    /// The path the URL named, when a rule mapped it to another path.
    pub live_path: Option<String>,
    /// The rule that mapped it.
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

/// What [`Store::refile_threads`] did.
#[derive(Clone, Debug, Default)]
pub struct Refiled {
    /// Each thread it moved, with the page it left (that page, when only its
    /// path or route changed).
    pub moved: Vec<(String, String)>,
    /// The versions it wrote on the new page, oldest first.
    pub versions: Vec<Version>,
}

/// A thread's re-filing, as planned before any version is copied.
struct Plan {
    tid: String,
    from: String,
    from_url: String,
    version_n: u32,
    anchor: Anchor,
    has_clip: bool,
    /// The snapshot versions the thread names (its own and its addressed
    /// versions) that must be copied to the new page.
    needed: Vec<(String, u32)>,
    live_path: Option<String>,
    route: Option<String>,
}

/// A clip copied to a thread's new page ahead of the move's transaction;
/// unless kept, removed on drop.
struct CopiedClip {
    path: std::path::PathBuf,
    keep: bool,
}

impl CopiedClip {
    /// Keeps the copy.
    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for CopiedClip {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
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

fn plan_refile(c: &Connection, to: &LivePage, moves: &[Refile]) -> Result<Vec<Plan>> {
    let mut out = Vec::new();
    for m in moves {
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
        let mut needed = Vec::new();
        if from != to.artifact_id {
            needed.push((from.clone(), version_n));
            let mut st = c.prepare_cached(
                "SELECT artifact_id, version_n FROM version_threads WHERE thread_id = ?1",
            )?;
            for r in st.query_map(params![m.thread_id], |r| Ok((r.get(0)?, r.get(1)?)))? {
                needed.push(r?);
            }
        }
        out.push(Plan {
            tid: m.thread_id.clone(),
            from_url: thread_url(&page, live_path.as_deref(), anchor.route.as_deref()),
            from,
            version_n,
            anchor,
            has_clip,
            needed,
            live_path: m.live_path.clone(),
            route: m.route.clone(),
        });
    }
    Ok(out)
}

/// Re-files the planned threads under `to`, in one transaction: their
/// version (and addressed versions) become the copies `map` names, and
/// their pending address and pick follow them. `Conflict` when a thread
/// changed since it was planned (a caller plans again).
fn commit_refile(
    tx: &rusqlite::Transaction<'_>,
    to: &LivePage,
    plans: &[Plan],
    map: &HashMap<(String, u32), u32>,
    by: &str,
    rule_id: Option<&str>,
) -> Result<()> {
    let now = Store::now();
    let retry = || CoreError::Conflict { current: 0 };
    for p in plans {
        let from: Option<String> = tx
            .query_row(
                "SELECT artifact_id FROM threads WHERE id = ?1",
                params![p.tid],
                |r| r.get(0),
            )
            .optional()?;
        if from.as_deref() != Some(p.from.as_str()) {
            return Err(retry());
        }
        let mut version_n = p.version_n;
        if p.from != to.artifact_id {
            version_n = *map.get(&(p.from.clone(), p.version_n)).ok_or_else(retry)?;
            let links: Vec<(String, u32, String, String)> = {
                let mut st = tx.prepare_cached(
                    "SELECT artifact_id, version_n, source, created_at FROM version_threads
                     WHERE thread_id = ?1",
                )?;
                st.query_map(params![p.tid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            tx.execute(
                "DELETE FROM version_threads WHERE thread_id = ?1",
                params![p.tid],
            )?;
            for (a, n, source, at) in links {
                let n = *map.get(&(a, n)).ok_or_else(retry)?;
                tx.execute(
                    "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![to.artifact_id, n, p.tid, source, at],
                )?;
            }
            tx.execute(
                "UPDATE live_pending SET artifact_id = ?2 WHERE thread_id = ?1",
                params![p.tid, to.artifact_id],
            )?;
            tx.execute(
                "UPDATE OR IGNORE live_picks SET artifact_id = ?2 WHERE thread_id = ?1",
                params![p.tid, to.artifact_id],
            )?;
            tx.execute(
                "DELETE FROM live_picks WHERE thread_id = ?1 AND artifact_id <> ?2",
                params![p.tid, to.artifact_id],
            )?;
        }
        let mut anchor = p.anchor.clone();
        anchor.route = p.route.clone();
        tx.execute(
            "UPDATE threads SET artifact_id = ?2, version_n = ?3, anchor_json = ?4, live_path = ?5
             WHERE id = ?1",
            params![
                p.tid,
                to.artifact_id,
                version_n,
                serde_json::to_string(&anchor).expect("anchors serialise"),
                p.live_path
            ],
        )?;
        tx.execute(
            "INSERT INTO thread_moves (id, thread_id, from_artifact_id, from_url, to_artifact_id,
                to_url, moved_by, rule_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                new_ulid(),
                p.tid,
                p.from,
                p.from_url,
                to.artifact_id,
                thread_url(to, p.live_path.as_deref(), p.route.as_deref()),
                by,
                rule_id,
                now
            ],
        )?;
    }
    Ok(())
}

impl Store {
    /// The merge rules of `origin`, oldest first.
    ///
    /// # Errors
    /// Database errors only.
    pub fn live_rules(&self, origin: &str) -> Result<Vec<LiveRule>> {
        self.with_read(|c| rules_in(c, origin))
    }

    /// Adds the rule `pattern` to `origin`; a rule already there is answered
    /// as it is (`false`: not new).
    ///
    /// # Errors
    /// `too_many_rules` past [`MAX_RULES`] rules for the origin.
    pub fn add_live_rule(&self, origin: &str, pattern: &PathPattern) -> Result<(LiveRule, bool)> {
        self.with_tx(|tx| {
            let rules = rules_in(tx, origin)?;
            if let Some(r) = rules.iter().find(|r| r.pattern == pattern.as_str()) {
                return Ok((r.clone(), false));
            }
            if rules.len() >= MAX_RULES {
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
            };
            tx.execute(
                "INSERT INTO live_rules (id, origin, pattern, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![r.id, r.origin, r.pattern, r.created_at],
            )?;
            Ok((r, true))
        })
    }

    /// Deletes the rule `id`; `None` when there is none. The threads it
    /// re-filed stay where they are.
    ///
    /// # Errors
    /// Database errors only.
    pub fn delete_live_rule(&self, id: &str) -> Result<Option<LiveRule>> {
        self.with_tx(|tx| {
            let r = tx
                .query_row(
                    "SELECT id, origin, pattern, created_at FROM live_rules WHERE id = ?1",
                    params![id],
                    row_to_rule,
                )
                .optional()?;
            if r.is_some() {
                tx.execute("DELETE FROM live_rules WHERE id = ?1", params![id])?;
            }
            Ok(r)
        })
    }

    /// Where `key`'s comments go: the canonical page of the rule of its
    /// origin that maps its path ([`winning_rule`]), or `key` itself.
    ///
    /// # Errors
    /// Database errors only.
    pub fn resolve_live_key(&self, key: &PageKey) -> Result<Resolved> {
        let rules = self.live_rules(&key.origin)?;
        Ok(resolve_with(&rules, key))
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

    /// The threads of live pages of `origin` that `rule` (one of the
    /// origin's rules) now maps and that are not on its canonical page yet:
    /// each thread whose path (its `live_path`, else its page's) the rule
    /// wins ([`winning_rule`]), with where it goes on the canonical page.
    /// Also the title of the page that holds the first of them.
    ///
    /// # Errors
    /// Database errors only.
    pub fn rule_refiles(&self, rule: &LiveRule) -> Result<(Vec<Refile>, Option<String>)> {
        let rules = self.live_rules(&rule.origin)?;
        let mut out = Vec::new();
        let mut title = None;
        self.with_read(|c| {
            let pages: Vec<LivePage> = {
                let mut st = c.prepare_cached(PAGES_OF_ORIGIN)?;
                st.query_map(params![rule.origin], |r| {
                    Ok(LivePage {
                        artifact_id: r.get(0)?,
                        origin: r.get(1)?,
                        path: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let by_id: HashMap<&str, &LivePage> =
                pages.iter().map(|p| (p.artifact_id.as_str(), p)).collect();
            let ids: Vec<String> = pages.iter().map(|p| p.artifact_id.clone()).collect();
            let mut st = c.prepare_cached(
                "SELECT id, live_path FROM threads WHERE id IN (SELECT value FROM json_each(?1))",
            )?;
            let threads = threads_of_many(c, &ids)?;
            let tids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
            let live_paths: HashMap<String, Option<String>> = st
                .query_map(params![super::feedback::id_array(&tids)], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for t in threads {
                let Some(page) = by_id.get(t.artifact_id.as_str()) else {
                    continue;
                };
                if page.path == rule.pattern {
                    continue;
                }
                let path = live_paths
                    .get(&t.id)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| page.path.clone());
                if winning_rule(&rules, |r| &r.pattern, &path).map(|r| &r.id) != Some(&rule.id) {
                    continue;
                }
                if title.is_none() {
                    title = c
                        .query_row(
                            "SELECT title FROM artifacts WHERE id = ?1",
                            params![page.artifact_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                }
                out.push(Refile {
                    thread_id: t.id,
                    live_path: (path != rule.pattern).then_some(path),
                    route: t.anchor.route,
                });
            }
            Ok(())
        })?;
        Ok((out, title))
    }

    /// Re-files threads under the live page `to` (spec 2026-10-05
    /// §7.1), each as its [`Refile`] says, recording for each a move by `by`
    /// (`viewer:<public ID>`), from the merge rule `rule_id` when one made
    /// it. A thread already where its `Refile` puts it is left alone.
    ///
    /// A thread changing page keeps its comments, sends, feedback and
    /// history; its snapshot version, and each version that addressed it, is
    /// copied to `to` as a new version noted [`MOVED_NOTE`] (one copy per
    /// source version, whatever number of threads name it), and the thread
    /// names the copies; its pending address, pick and clip follow it. When
    /// `restore` is set and a copy was written, `to`'s version current
    /// before the move is copied once more, so the page's current version is
    /// still its own latest snapshot. The re-filing is one transaction; the
    /// copies are written before it.
    ///
    /// # Errors
    /// `NotFound` for a missing thread or page; `not_live` for a thread not
    /// on a live page; `cross_origin` for a thread of another origin;
    /// `Conflict` when the threads keep changing under the move.
    pub fn refile_threads(
        &self,
        to: &ArtifactId,
        moves: &[Refile],
        by: &str,
        rule_id: Option<&str>,
        restore: bool,
    ) -> Result<Refiled> {
        let target = self.live_page_of(to)?.ok_or(CoreError::NotFound)?;
        let before = self
            .get_artifact(to)?
            .ok_or(CoreError::NotFound)?
            .current_version;
        let mut map: HashMap<(String, u32), u32> = HashMap::new();
        let mut done = Refiled::default();
        for _ in 0..REFILE_ATTEMPTS {
            let plans = self.with_read(|c| plan_refile(c, &target, moves))?;
            if plans.is_empty() {
                return Ok(done);
            }
            let needed: BTreeSet<(String, u32)> = plans
                .iter()
                .flat_map(|p| p.needed.iter().cloned())
                .filter(|k| !map.contains_key(k))
                .collect();
            for (src, n) in &needed {
                let v = self.copy_snapshot(src, *n, to)?;
                map.insert((src.clone(), *n), v.n);
                done.versions.push(v);
            }
            if restore && !needed.is_empty() && before > 0 {
                done.versions
                    .push(self.copy_snapshot(to.as_str(), before, to)?);
            }
            let mut clips = Vec::new();
            for p in plans.iter().filter(|p| p.has_clip && p.from != target.artifact_id) {
                let src = self.home.clip_path(&ArtifactId::parse(&p.from)?, &p.tid);
                let dest = self.home.clip_path(to, &p.tid);
                std::fs::create_dir_all(self.home.clips_dir(to))?;
                match std::fs::copy(&src, &dest) {
                    Ok(_) => clips.push((
                        CopiedClip {
                            path: dest,
                            keep: false,
                        },
                        src,
                    )),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
            match self.with_tx(|tx| commit_refile(tx, &target, &plans, &map, by, rule_id)) {
                Ok(()) => {
                    for (copy, src) in clips {
                        copy.keep();
                        let _ = std::fs::remove_file(src);
                    }
                    done.moved = plans.into_iter().map(|p| (p.tid, p.from)).collect();
                    return Ok(done);
                }
                Err(CoreError::Conflict { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(CoreError::Conflict { current: 0 })
    }

    /// Writes version `n` of the live page `src`'s `index.html` as the next
    /// version of the live page `to`, noted [`MOVED_NOTE`], keeping `to`'s
    /// title.
    fn copy_snapshot(&self, src: &str, n: u32, to: &ArtifactId) -> Result<Version> {
        let html = std::fs::read(self.home.version_dir(&ArtifactId::parse(src)?, n).join(INDEX))?;
        let mut attempt = 1;
        loop {
            let a = self.get_artifact(to)?.ok_or(CoreError::NotFound)?;
            let p = snapshot_publish_noted(a.current_version, &a.title, &html, MOVED_NOTE)?;
            match self.write_version_then(to, a.current_version, &p, &BTreeMap::new(), None, |_, _| {
                Ok(())
            }) {
                Ok((_, v, ())) => return Ok(v),
                Err(CoreError::Conflict { .. }) if attempt < SNAPSHOT_ATTEMPTS => attempt += 1,
                Err(e) => return Err(e),
            }
        }
    }
}

/// [`Store::resolve_live_key`] against `rules`, the origin's, oldest first.
pub fn resolve_with(rules: &[LiveRule], key: &PageKey) -> Resolved {
    match winning_rule(rules, |r| &r.pattern, &key.path) {
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
    use crate::store::test_util::{anchor, store};
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

    fn thread(st: &Store, id: &ArtifactId, route: Option<&str>) -> Thread {
        let a = st.get_artifact(id).unwrap().unwrap();
        let mut an = anchor();
        an.route = route.map(str::to_string);
        st.create_thread(
            id,
            NewThread {
                version_n: a.current_version,
                anchor: an,
                author_name: "Ana".into(),
                author_public_id: None,
                body: "Fix this".into(),
                clip: Some(PNG.to_vec()),
                via_page: false,
            },
        )
        .unwrap()
    }

    fn index(st: &Store, id: &ArtifactId, n: u32) -> String {
        std::fs::read_to_string(st.home().version_dir(id, n).join(INDEX)).unwrap()
    }

    fn refile(tid: &str) -> Refile {
        Refile {
            thread_id: tid.into(),
            live_path: None,
            route: None,
        }
    }

    fn rule(st: &Store, pattern: &str) -> LiveRule {
        st.add_live_rule(ORIGIN, &PathPattern::parse(pattern).unwrap())
            .unwrap()
            .0
    }

    #[test]
    fn a_moved_thread_takes_its_snapshot_links_pending_address_and_clip() {
        let (_d, st) = store();
        let a = page(&st, "/a", "<p>a1");
        let b = page(&st, "/b", "<p>b1");
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
        let old_clip = st.home().clip_path(&a, &t.id);
        assert!(old_clip.exists());

        let done = st
            .refile_threads(&b, &[refile(&t.id)], "viewer:u_x", None, true)
            .unwrap();
        assert_eq!(done.moved, vec![(t.id.clone(), a.to_string())]);
        // /a's version 1 (the thread's) and 2 (its address), then /b's own
        // version 1 again, so /b's current version is still its own.
        let ns: Vec<u32> = done.versions.iter().map(|v| v.n).collect();
        assert_eq!(ns, vec![2, 3, 4]);
        assert_eq!(index(&st, &b, 2), "<p>a1");
        assert_eq!(index(&st, &b, 3), "<p>a2");
        assert_eq!(index(&st, &b, 4), "<p>b1");
        assert!(
            done.versions
                .iter()
                .all(|v| v.note.as_deref() == Some(MOVED_NOTE))
        );

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
        assert_eq!(x[0].moves.len(), 1);
        let m = &x[0].moves[0];
        assert_eq!(m.from_url, "http://localhost:5173/a?x=1");
        assert_eq!(m.to_url, "http://localhost:5173/b");
        assert_eq!(m.moved_by, "viewer:u_x");
        assert!(!old_clip.exists());
        assert_eq!(std::fs::read(st.home().clip_path(&b, &t.id)).unwrap(), PNG);
        assert!(st.list_threads(&a, true, None, 50).unwrap().0.is_empty());

        // Moving it again to where it is changes nothing.
        let again = st
            .refile_threads(&b, &[refile(&t.id)], "viewer:u_x", None, true)
            .unwrap();
        assert!(again.moved.is_empty() && again.versions.is_empty());
        // Deleting the page it left keeps it whole.
        st.delete_artifact(&a).unwrap();
        let kept = st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(kept.artifact_id, b.as_str());
        let x = st.thread_extras(std::slice::from_ref(&kept), false).unwrap();
        assert_eq!((x[0].moves.len(), x[0].addressed_in.clone()), (1, vec![3]));
        // Deleting the thread deletes its moves.
        st.delete_thread(&t.id).unwrap();
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
            .refile_threads(&other, &[refile(&t.id)], "viewer:u_x", None, true)
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
            .refile_threads(&a, &[refile(&h.id)], "viewer:u_x", None, true)
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid { code, .. } if code == "not_live"));
        assert!(matches!(
            st.refile_threads(&a, &[refile("nope")], "viewer:u_x", None, true),
            Err(CoreError::NotFound)
        ));
        assert_eq!(st.get_artifact(&other).unwrap().unwrap().current_version, 1);
    }

    #[test]
    fn rules_map_lookups_and_name_the_threads_they_take() {
        let (_d, st) = store();
        let one = page(&st, "/users/1", "<p>1");
        let two = page(&st, "/users/2", "<p>2");
        let deep = page(&st, "/users/2/edit", "<p>e");
        let t1 = thread(&st, &one, None);
        let t2 = thread(&st, &two, Some("#/tab"));
        let t3 = thread(&st, &deep, None);
        let all = rule(&st, "/users/*");
        let (again, new) = st
            .add_live_rule(ORIGIN, &PathPattern::parse("/users/*").unwrap())
            .unwrap();
        assert!(!new && again == all);
        let r = st.resolve_live_key(&key("/users/9/x")).unwrap();
        assert_eq!(r.key, key("/users/*"));
        assert_eq!(r.live_path.as_deref(), Some("/users/9/x"));
        assert_eq!(r.rule.as_ref(), Some(&all));
        let r = st.resolve_live_key(&key("/teams/1")).unwrap();
        assert_eq!((r.key, r.live_path, r.rule), (key("/teams/1"), None, None));

        let (refiles, title) = st.rule_refiles(&all).unwrap();
        assert_eq!(refiles.len(), 3);
        assert!(title.is_some());
        let canon = st.ensure_live_page(&key("/users/*"), "Users", None).unwrap();
        let canon = ArtifactId::parse(&canon.artifact.id).unwrap();
        let done = st
            .refile_threads(&canon, &refiles, "viewer:u_x", Some(&all.id), false)
            .unwrap();
        assert_eq!(done.moved.len(), 3);
        let x = st
            .thread_extras(&[st.get_thread(&t2.id).unwrap().unwrap()], false)
            .unwrap();
        assert_eq!(x[0].live_path.as_deref(), Some("/users/2"));
        assert_eq!(x[0].moves[0].rule_id.as_deref(), Some(all.id.as_str()));
        assert_eq!(x[0].moves[0].to_url, "http://localhost:5173/users/2#/tab");
        assert!(st.rule_refiles(&all).unwrap().0.is_empty());

        // A more specific rule takes what it wins, from the other rule's
        // canonical page too.
        let one_seg = rule(&st, "/users/:id");
        let (refiles, _) = st.rule_refiles(&one_seg).unwrap();
        let mut got: Vec<_> = refiles
            .iter()
            .map(|r| (r.thread_id.clone(), r.live_path.clone(), r.route.clone()))
            .collect();
        got.sort();
        let mut want = vec![
            (t1.id.clone(), Some("/users/1".to_string()), None),
            (
                t2.id.clone(),
                Some("/users/2".to_string()),
                Some("#/tab".to_string()),
            ),
        ];
        want.sort();
        assert_eq!(got, want);
        let canon2 = st.ensure_live_page(&key("/users/:id"), "U", None).unwrap();
        let canon2 = ArtifactId::parse(&canon2.artifact.id).unwrap();
        st.refile_threads(&canon2, &refiles, "viewer:u_x", Some(&one_seg.id), false)
            .unwrap();

        let site = st.site_pages(ORIGIN).unwrap();
        let counts: HashMap<String, usize> = site
            .iter()
            .map(|p| (p.page.path.clone(), p.threads.len()))
            .collect();
        assert_eq!(counts["/users/:id"], 2);
        assert_eq!(counts["/users/*"], 1);
        assert_eq!(counts["/users/1"], 0);
        assert_eq!(
            st.get_thread(&t3.id).unwrap().unwrap().artifact_id,
            canon.as_str()
        );

        assert_eq!(st.delete_live_rule(&one_seg.id).unwrap(), Some(one_seg.clone()));
        assert_eq!(st.delete_live_rule(&one_seg.id).unwrap(), None);
        assert_eq!(
            st.resolve_live_key(&key("/users/9")).unwrap().key,
            key("/users/*")
        );
        assert_eq!(
            st.get_thread(&t1.id).unwrap().unwrap().artifact_id,
            canon2.as_str(),
            "deleting a rule moves nothing back"
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

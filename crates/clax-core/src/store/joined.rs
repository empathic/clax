//! Joined sites (spec 2026-10-05-chrome-overlay-design §7.2, owner
//! decisions 2026-10-06): origins the owner declared one app, such as a dev
//! server that moved from port 7702 to 7703. A joined site's live pages and
//! merge rules are kept under one of its origins, its key; every lookup by
//! one of its origins resolves to the key's pages by path, every scope watch
//! on one of them covers them all, and its threads list together.
//!
//! `live_sites` holds a row per origin of a joined site (the key's own
//! included); an origin without a row is a site of its own, whose key is
//! itself. Joining an origin to another's site re-keys the origin's pages
//! whose path the site lacks, and leaves the rest pending: their threads are
//! re-filed onto the site's page of the same path as moves are, a batch at
//! a time ([`Store::join_candidates`]), and an emptied page is merged away
//! (kept whole, out of the listings)
//! ([`Store::settle_joined_pages`]). Splitting an origin off removes its row
//! from then on; what the site holds stays with the site (the key moves to
//! the newest origin left when the key itself is split off).

use super::Store;
use super::live::{LivePage, rematerialize};
use super::site::{MAX_RULES, Refile, WATCHES_TO};
use crate::live::same_host_family;
use crate::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// The most origins one joined site holds.
pub const MAX_SITE_ORIGINS: usize = 16;
/// The most suggestions one request answers.
pub const MAX_SUGGESTIONS: usize = 3;
/// The most recently active sites a suggestion looks at.
pub const MAX_SUGGEST_SITES: usize = 64;
/// The owner's answer to a suggested join: never suggest the pair again.
pub const ANSWER_NEVER: &str = "never";
/// The owner's answer to a suggested join: not now (for [`LATER`]).
pub const ANSWER_LATER: &str = "later";
/// How long "Not now" keeps a pair from being suggested again.
pub const LATER: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// One origin of a site: when it joined the site and when Clax last used
/// it (`None` for an origin that is a site of its own).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SiteOrigin {
    pub origin: String,
    pub joined_at: Option<String>,
    pub last_used_at: Option<String>,
}

/// A site: its key (the origin its pages and rules are kept under) and its
/// origins, the most recently used first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JoinedSite {
    pub key: String,
    pub origins: Vec<SiteOrigin>,
}

impl JoinedSite {
    /// Whether the site joins more than one origin.
    pub fn joined(&self) -> bool {
        self.origins.len() > 1
    }

    /// The most recently used origin: what the site is named after.
    pub fn newest(&self) -> &str {
        self.origins.first().map_or(&self.key, |o| &o.origin)
    }

    /// The site's origins, the most recently used first.
    pub fn origin_names(&self) -> Vec<String> {
        self.origins.iter().map(|o| o.origin.clone()).collect()
    }
}

/// What [`Store::join_origins`] did.
#[derive(Clone, Debug)]
pub struct Joined {
    /// The site after the join.
    pub site: JoinedSite,
    /// Whether the call changed the site (false: already joined).
    pub changed: bool,
    /// The live pages it re-keyed to the site's key.
    pub rekeyed: Vec<String>,
}

/// What [`Store::split_origin`] did.
#[derive(Clone, Debug)]
pub struct Split {
    /// The site before the split.
    pub before: JoinedSite,
    /// What is left of it (its key moves when the key was split off).
    pub site: JoinedSite,
    /// The live pages re-keyed to the new key.
    pub rekeyed: Vec<String>,
}

/// A live page a join merged away (spec §7.2), kept whole: `merged_into`
/// is the page it was merged into, `None` once that page was deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedPage {
    pub artifact_id: String,
    pub origin: String,
    pub path: String,
    pub merged_into: Option<String>,
}

/// A site in the listing: its pages and threads, and its last activity.
#[derive(Clone, Debug, Serialize)]
pub struct SiteSummary {
    pub site: JoinedSite,
    pub pages: u64,
    pub threads: u64,
    pub last_activity: Option<String>,
}

/// A suggested join: the site, the origin it is named by (its newest), and
/// why (`path`: a live page of the path; `title`: one of the title).
#[derive(Clone, Debug, Serialize)]
pub struct Suggestion {
    pub site: JoinedSite,
    pub origin: String,
    pub reason: &'static str,
    pub path: Option<String>,
}

/// The key of `origin`'s site: the site it joined, else itself.
pub(crate) fn site_key(c: &Connection, origin: &str) -> Result<String> {
    Ok(c.query_row(
        "SELECT site FROM live_sites WHERE origin = ?1",
        params![origin],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or_else(|| origin.to_string()))
}

/// The site whose key is `key`, its origins the most recently used first.
fn site_of_key(c: &Connection, key: &str) -> Result<JoinedSite> {
    let mut st = c.prepare_cached(
        "SELECT origin, joined_at, last_used_at FROM live_sites WHERE site = ?1
         ORDER BY last_used_at DESC, joined_at DESC, origin",
    )?;
    let mut origins = st
        .query_map(params![key], |r| {
            Ok(SiteOrigin {
                origin: r.get(0)?,
                joined_at: Some(r.get(1)?),
                last_used_at: Some(r.get(2)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if origins.is_empty() {
        origins.push(SiteOrigin {
            origin: key.to_string(),
            joined_at: None,
            last_used_at: None,
        });
    }
    Ok(JoinedSite {
        key: key.to_string(),
        origins,
    })
}

/// The site of `origin`.
fn site_in(c: &Connection, origin: &str) -> Result<JoinedSite> {
    site_of_key(c, &site_key(c, origin)?)
}

/// The live pages of `key`'s other origins (a join not finished): each
/// with the site's page of its path, when there is one.
fn pending_pages(c: &Connection, key: &str) -> Result<Vec<(LivePage, Option<String>)>> {
    let mut st = c.prepare_cached(
        "SELECT p.artifact_id, p.origin, p.path,
            (SELECT q.artifact_id FROM live_pages q JOIN artifacts b ON b.id = q.artifact_id
             WHERE q.origin = ?1 AND q.path = p.path AND b.deleted_at IS NULL)
         FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
         JOIN live_sites s ON s.origin = p.origin
         WHERE s.site = ?1 AND p.origin <> ?1 AND a.deleted_at IS NULL
         ORDER BY p.created_at, p.artifact_id",
    )?;
    let rows = st
        .query_map(params![key], |r| {
            Ok((
                LivePage {
                    artifact_id: r.get(0)?,
                    origin: r.get(1)?,
                    path: r.get(2)?,
                },
                r.get(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The threads of the pending pages of the site `?1` that the site has a
/// page of the path of (`q`), from `FROM` on.
const PENDING_THREADS: &str = "FROM live_pages p
    JOIN artifacts a ON a.id = p.artifact_id AND a.deleted_at IS NULL
    JOIN live_sites s ON s.origin = p.origin
    JOIN live_pages q ON q.origin = ?1 AND q.path = p.path
    JOIN artifacts b ON b.id = q.artifact_id AND b.deleted_at IS NULL
    JOIN threads t ON t.artifact_id = p.artifact_id
    WHERE s.site = ?1 AND p.origin <> ?1";

/// Refused while a join of the site is not finished (spec §7.2).
fn joining() -> CoreError {
    CoreError::invalid(
        "joining",
        "this site's join is not finished; finish it (Continue joining) first",
    )
}

/// `a` and `b` in order, as `live_site_answers` keys a pair.
fn pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a < b { (a, b) } else { (b, a) }
}

fn answer_in(
    c: &Connection,
    a: &str,
    b: &str,
    answer: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let (a, b) = pair(a, b);
    let until = (answer == ANSWER_LATER).then(|| {
        let d = chrono::Duration::from_std(LATER).expect("the delay fits");
        (now + d).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });
    c.execute(
        "INSERT INTO live_site_answers (a, b, answer, until, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(a, b) DO UPDATE SET answer = excluded.answer, until = excluded.until,
            created_at = excluded.created_at",
        params![a, b, answer, until, Store::now()],
    )?;
    Ok(())
}

impl Store {
    /// The site of `origin`: its key and origins (only `origin`, when it
    /// joined none).
    ///
    /// # Errors
    /// Database errors only.
    pub fn joined_site(&self, origin: &str) -> Result<JoinedSite> {
        self.with_read(|c| site_in(c, origin))
    }

    /// Every live page merged away by a join (spec §7.2): its artifact ID,
    /// origin, path, and the page it was merged into (`None` once that page
    /// was deleted: released, listed again). They stay live pages for the
    /// LAN rule.
    ///
    /// # Errors
    /// Database errors only.
    pub fn merged_live_pages(&self) -> Result<Vec<MergedPage>> {
        self.with_read(|c| {
            let mut st = c.prepare(
                "SELECT m.artifact_id, m.origin, m.path, m.merged_into FROM live_merged_pages m
                 JOIN artifacts a ON a.id = m.artifact_id WHERE a.deleted_at IS NULL",
            )?;
            let rows = st
                .query_map([], |r| {
                    Ok(MergedPage {
                        artifact_id: r.get(0)?,
                        origin: r.get(1)?,
                        path: r.get(2)?,
                        merged_into: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The live page `id` merged away by a join: its origin, path and the
    /// page it was merged into; `None` for any other artifact.
    ///
    /// # Errors
    /// Database errors only.
    pub fn merged_live_page(&self, id: &str) -> Result<Option<(String, String, Option<String>)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT origin, path, merged_into FROM live_merged_pages WHERE artifact_id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
        })
    }

    /// Every origin of a joined site with the site's key.
    ///
    /// # Errors
    /// Database errors only.
    pub fn site_memberships(&self) -> Result<Vec<(String, String, String)>> {
        self.with_read(|c| {
            let mut st = c.prepare("SELECT origin, site, last_used_at FROM live_sites")?;
            let rows = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Records that Clax used `origin` now, when it is an origin of a
    /// joined site; whether it is.
    ///
    /// # Errors
    /// Database errors only.
    pub fn touch_site_origin(&self, origin: &str) -> Result<bool> {
        self.with_write(|c| {
            Ok(c.execute(
                "UPDATE live_sites SET last_used_at = ?2 WHERE origin = ?1",
                params![origin, Store::now()],
            )? > 0)
        })
    }

    /// Every site with live pages, the most recently active first, with
    /// its pages and threads.
    ///
    /// # Errors
    /// Database errors only.
    pub fn live_sites(&self) -> Result<Vec<SiteSummary>> {
        self.with_read(|c| {
            let rows: Vec<(String, u64, u64, Option<String>)> = {
                let mut st = c.prepare(
                    "SELECT COALESCE(s.site, p.origin) AS k, COUNT(DISTINCT p.artifact_id),
                        COUNT(t.id), MAX(COALESCE(t.created_at, a.updated_at))
                     FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
                     LEFT JOIN live_sites s ON s.origin = p.origin
                     LEFT JOIN threads t ON t.artifact_id = p.artifact_id
                     WHERE a.deleted_at IS NULL
                     GROUP BY k ORDER BY MAX(COALESCE(t.created_at, a.updated_at)) DESC, k",
                )?;
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            rows.into_iter()
                .map(|(k, pages, threads, last_activity)| {
                    Ok(SiteSummary {
                        site: site_of_key(c, &k)?,
                        pages,
                        threads,
                        last_activity,
                    })
                })
                .collect()
        })
    }

    /// Joins `origin` (with the site it is in, when it is in one) to the
    /// site of `with`, whose key stays the site's key: each page of the
    /// joining origins is re-keyed to the site's key, unless the site has a
    /// page of its path (that page's threads are re-filed onto the site's
    /// by [`Store::join_candidates`]); their merge rules are the site's
    /// (one the site has already is dropped); every scope watch of an origin
    /// of the site covers every page of it. A join of two origins already
    /// one site changes nothing.
    ///
    /// # Errors
    /// `same_origin` when they are one origin; `unknown_site` when `with`'s
    /// site has no live page; `too_many_origins` past
    /// [`MAX_SITE_ORIGINS`]; `too_many_rules` when the site would hold more
    /// than [`MAX_RULES`] rules in force.
    pub fn join_origins(&self, origin: &str, with: &str) -> Result<Joined> {
        if origin == with {
            return Err(CoreError::invalid(
                "same_origin",
                "an origin is already its own site",
            ));
        }
        self.with_tx(|tx| {
            let kx = site_key(tx, origin)?;
            let ky = site_key(tx, with)?;
            let known: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
                    WHERE p.origin = ?1 AND a.deleted_at IS NULL)
                    OR EXISTS(SELECT 1 FROM live_sites WHERE site = ?1)",
                params![ky],
                |r| r.get(0),
            )?;
            if !known {
                return Err(CoreError::invalid(
                    "unknown_site",
                    format!("Clax has no live page of {with} yet"),
                ));
            }
            if kx == ky {
                return Ok(Joined {
                    site: site_of_key(tx, &ky)?,
                    changed: false,
                    rekeyed: Vec::new(),
                });
            }
            // A join not finished would leave two pages of one path pending.
            if !pending_pages(tx, &kx)?.is_empty() || !pending_pages(tx, &ky)?.is_empty() {
                return Err(joining());
            }
            let mx = site_of_key(tx, &kx)?.origin_names();
            let my = site_of_key(tx, &ky)?.origin_names();
            if mx.len() + my.len() > MAX_SITE_ORIGINS {
                return Err(CoreError::invalid(
                    "too_many_origins",
                    format!("a site joins at most {MAX_SITE_ORIGINS} origins"),
                ));
            }
            let patterns = |k: &str| -> Result<Vec<(String, String, bool)>> {
                let mut st = tx.prepare(
                    "SELECT id, pattern, deleted_at IS NULL FROM live_rules WHERE origin = ?1",
                )?;
                let rows = st
                    .query_map(params![k], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            };
            let (rx, ry) = (patterns(&kx)?, patterns(&ky)?);
            // An un-merge under way would lose its rule, or the joining one.
            if let Some((_, p, _)) = rx.iter().chain(&ry).find(|r| !r.2) {
                return Err(CoreError::invalid(
                    "unmerging",
                    format!("{p} is being un-merged; let it finish (Un-merge again), then join"),
                ));
            }
            let fresh = rx
                .iter()
                .filter(|(_, p, live)| *live && !ry.iter().any(|(_, q, _)| q == p))
                .count();
            if ry.iter().filter(|r| r.2).count() + fresh > MAX_RULES {
                return Err(CoreError::invalid(
                    "too_many_rules",
                    format!("a site holds at most {MAX_RULES} rules"),
                ));
            }
            let at = chrono::Utc::now();
            let stamp = |t: chrono::DateTime<chrono::Utc>| {
                t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            };
            let now = stamp(at);
            // The site's key, joined now, was used just before the origin
            // joining it, which is the newest.
            let before = stamp(at - chrono::Duration::milliseconds(1));
            tx.execute(
                "INSERT INTO live_sites (origin, site, joined_at, last_used_at) VALUES (?1, ?1, ?2, ?2)
                 ON CONFLICT(origin) DO NOTHING",
                params![ky, before],
            )?;
            for o in &mx {
                tx.execute(
                    "INSERT INTO live_sites (origin, site, joined_at, last_used_at) VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT(origin) DO UPDATE SET site = excluded.site, joined_at = excluded.joined_at",
                    params![o, ky, now],
                )?;
            }
            tx.execute(
                "UPDATE live_sites SET last_used_at = ?2 WHERE origin = ?1",
                params![origin, now],
            )?;
            let mut rekeyed = Vec::new();
            for (page, target) in pending_pages(tx, &ky)? {
                if target.is_none() {
                    tx.execute(
                        "UPDATE live_pages SET origin = ?2 WHERE artifact_id = ?1",
                        params![page.artifact_id, ky],
                    )?;
                    rekeyed.push(page.artifact_id);
                }
            }
            for (id, pattern, _) in rx {
                if ry.iter().any(|(_, q, _)| *q == pattern) {
                    tx.execute("DELETE FROM live_rules WHERE id = ?1", params![id])?;
                } else {
                    tx.execute(
                        "UPDATE live_rules SET origin = ?2 WHERE id = ?1",
                        params![id, ky],
                    )?;
                }
            }
            // Each scope of an origin of the site now covers every page of
            // it: only when the site has a scope watch at all.
            let scoped: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_watches lw CROSS JOIN sessions s ON s.id = lw.session_id
                    WHERE lw.origin IN (SELECT origin FROM live_sites WHERE site = ?1 UNION SELECT ?1)
                    AND s.ended_at IS NULL)",
                params![ky],
                |r| r.get(0),
            )?;
            if scoped {
                for p in super::live::pages_of(tx, &ky)? {
                    rematerialize(tx, &p)?;
                }
            }
            Ok(Joined {
                site: site_of_key(tx, &ky)?,
                changed: true,
                rekeyed,
            })
        })
    }

    /// The first `limit` threads of the pages a join of the site `key` left
    /// pending, oldest first, each with the site's page of its path to be
    /// re-filed onto (keeping the path it was made at and its route); and
    /// how many more there are.
    ///
    /// # Errors
    /// Database errors only.
    pub fn join_candidates(
        &self,
        key: &str,
        limit: usize,
    ) -> Result<(Vec<(String, Refile)>, usize)> {
        self.with_read(|c| {
            let rows: Vec<(String, String, Option<String>, String)> = {
                let mut st = c.prepare_cached(&format!(
                    "SELECT q.artifact_id, t.id, t.live_path, t.anchor_json {PENDING_THREADS}
                     ORDER BY t.created_at, t.id LIMIT ?2"
                ))?;
                st.query_map(params![key, limit as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let total: i64 = c.query_row(
                &format!("SELECT COUNT(*) {PENDING_THREADS}"),
                params![key],
                |r| r.get(0),
            )?;
            let out: Vec<(String, Refile)> = rows
                .into_iter()
                .map(|(to, tid, live_path, anchor)| {
                    let route = serde_json::from_str::<crate::anchor::Anchor>(&anchor)
                        .ok()
                        .and_then(|a| a.route);
                    (
                        to,
                        Refile {
                            thread_id: tid,
                            live_path,
                            route,
                        },
                    )
                })
                .collect();
            let remaining = usize::try_from(total)
                .unwrap_or(0)
                .saturating_sub(out.len());
            Ok((out, remaining))
        })
    }

    /// How many threads of the site `key`'s pending pages (a join not
    /// finished) are still to be re-filed.
    ///
    /// # Errors
    /// Database errors only.
    pub fn joining_threads(&self, key: &str) -> Result<usize> {
        self.with_read(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM threads t JOIN live_pages p ON p.artifact_id = t.artifact_id
                 JOIN artifacts a ON a.id = p.artifact_id AND a.deleted_at IS NULL
                 JOIN live_sites s ON s.origin = p.origin
                 WHERE s.site = ?1 AND p.origin <> ?1",
                params![key],
                |r| r.get(0),
            )?;
            Ok(usize::try_from(n).unwrap_or(0))
        })
    }

    /// Settles the pages a join of the site `key` left pending: one whose
    /// path the site has no page of any more is re-keyed to the site's key;
    /// one with no thread left hands its watchers to the site's page of its
    /// path and is merged into it: it stops being a live page's key (its row
    /// moves to `live_merged_pages`, naming that page) and is kept whole,
    /// its artifact, link and snapshots intact, out of the listings.
    /// Returns the pages merged away and the pages re-keyed.
    ///
    /// # Errors
    /// Database errors only.
    pub fn settle_joined_pages(&self, key: &str) -> Result<(Vec<String>, Vec<String>)> {
        self.with_tx(|tx| {
            let (mut empty, mut rekeyed) = (Vec::new(), Vec::new());
            for (page, target) in pending_pages(tx, key)? {
                match target {
                    None => {
                        tx.execute(
                            "UPDATE live_pages SET origin = ?2 WHERE artifact_id = ?1",
                            params![page.artifact_id, key],
                        )?;
                        rematerialize(
                            tx,
                            &LivePage {
                                origin: key.to_string(),
                                ..page.clone()
                            },
                        )?;
                        rekeyed.push(page.artifact_id);
                    }
                    Some(to) => {
                        let threads: bool = tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM threads WHERE artifact_id = ?1)",
                            params![page.artifact_id],
                            |r| r.get(0),
                        )?;
                        if !threads {
                            let now = Store::now();
                            tx.execute(WATCHES_TO, params![page.artifact_id, to, now])?;
                            tx.execute(
                                "INSERT INTO live_merged_pages (artifact_id, origin, path, merged_into, merged_at)
                                 VALUES (?1, ?2, ?3, ?4, ?5)",
                                params![page.artifact_id, page.origin, page.path, to, now],
                            )?;
                            tx.execute(
                                "DELETE FROM live_pages WHERE artifact_id = ?1",
                                params![page.artifact_id],
                            )?;
                            empty.push(page.artifact_id);
                        }
                    }
                }
            }
            Ok((empty, rekeyed))
        })
    }

    /// Splits `origin` off its site: from now on it is a site of its own,
    /// keyed by itself, and the site's pages, threads and rules stay with
    /// the site (when `origin` was the site's key, the key moves to the
    /// most recently used origin left, and the pages and rules with it). A
    /// site left with one origin is that origin's own again. The pair is
    /// not suggested again. `None` when `origin` joined no site.
    ///
    /// # Errors
    /// `joining` while a join of the site is not finished (its pending
    /// pages would be split between the site and the origin).
    pub fn split_origin(&self, origin: &str) -> Result<Option<Split>> {
        self.with_tx(|tx| {
            let Some(key): Option<String> = tx
                .query_row(
                    "SELECT site FROM live_sites WHERE origin = ?1",
                    params![origin],
                    |r| r.get(0),
                )
                .optional()?
            else {
                return Ok(None);
            };
            let before = site_of_key(tx, &key)?;
            if !pending_pages(tx, &key)?.is_empty() {
                return Err(joining());
            }
            let mut new_key = key.clone();
            let mut rekeyed = Vec::new();
            if origin == key {
                let Some(next) = before.origins.iter().find(|o| o.origin != origin) else {
                    return Ok(None);
                };
                new_key = next.origin.clone();
                tx.execute(
                    "UPDATE live_sites SET site = ?2 WHERE site = ?1",
                    params![key, new_key],
                )?;
                let mut st = tx.prepare("SELECT artifact_id FROM live_pages WHERE origin = ?1")?;
                rekeyed = st
                    .query_map(params![key], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                tx.execute(
                    "UPDATE live_pages SET origin = ?2 WHERE origin = ?1",
                    params![key, new_key],
                )?;
                tx.execute(
                    "UPDATE live_rules SET origin = ?2 WHERE origin = ?1",
                    params![key, new_key],
                )?;
            }
            tx.execute("DELETE FROM live_sites WHERE origin = ?1", params![origin])?;
            let left: i64 = tx.query_row(
                "SELECT COUNT(*) FROM live_sites WHERE site = ?1",
                params![new_key],
                |r| r.get(0),
            )?;
            if left <= 1 {
                tx.execute("DELETE FROM live_sites WHERE site = ?1", params![new_key])?;
            }
            for o in &before.origins {
                if o.origin != origin {
                    answer_in(tx, origin, &o.origin, ANSWER_NEVER, chrono::Utc::now())?;
                }
            }
            Ok(Some(Split {
                site: site_of_key(tx, &new_key)?,
                before,
                rekeyed,
            }))
        })
    }

    /// Records the owner's answer to a suggestion that `origin` joins the
    /// site `with` names: [`ANSWER_NEVER`], or [`ANSWER_LATER`] (for
    /// [`LATER`]).
    ///
    /// # Errors
    /// `invalid_answer` for any other answer; `same_origin`.
    pub fn answer_join(&self, origin: &str, with: &str, answer: &str) -> Result<()> {
        self.answer_join_at(origin, with, answer, chrono::Utc::now())
    }

    /// [`Store::answer_join`] as at `now`.
    ///
    /// # Errors
    /// As [`Store::answer_join`].
    pub fn answer_join_at(
        &self,
        origin: &str,
        with: &str,
        answer: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        if answer != ANSWER_NEVER && answer != ANSWER_LATER {
            return Err(CoreError::invalid(
                "invalid_answer",
                "the answer is never or later",
            ));
        }
        if origin == with {
            return Err(CoreError::invalid("same_origin", "one origin is no pair"));
        }
        self.with_write(|c| answer_in(c, origin, with, answer, now))
    }

    /// The sites `origin` may be the same app as, when it is a site of its
    /// own (spec §7.2, owner decision 2026-10-06: Clax suggests, the owner
    /// confirms): each other site with an origin of the same host family
    /// ([`same_host_family`]) that has a live page of `path`, or titled
    /// `title`, or of a path `origin`'s own pages have; none the owner
    /// answered for (never, or not now); the most recently active first, at
    /// most [`MAX_SUGGESTIONS`] of the [`MAX_SUGGEST_SITES`] most recently
    /// active sites. Each is named by its newest origin.
    ///
    /// # Errors
    /// Database errors only.
    pub fn join_suggestions(
        &self,
        origin: &str,
        path: &str,
        title: &str,
    ) -> Result<Vec<Suggestion>> {
        self.join_suggestions_at(origin, path, title, chrono::Utc::now())
    }

    /// [`Store::join_suggestions`] as at `now`.
    ///
    /// # Errors
    /// Database errors only.
    pub fn join_suggestions_at(
        &self,
        origin: &str,
        path: &str,
        title: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<Suggestion>> {
        let now = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let title = title.trim();
        self.with_read(|c| {
            let joined: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_sites WHERE origin = ?1)",
                params![origin],
                |r| r.get(0),
            )?;
            if joined {
                return Ok(Vec::new());
            }
            let keys: Vec<String> = {
                let mut st = c.prepare(
                    "SELECT COALESCE(s.site, p.origin) AS k FROM live_pages p
                     JOIN artifacts a ON a.id = p.artifact_id
                     LEFT JOIN live_sites s ON s.origin = p.origin
                     WHERE a.deleted_at IS NULL AND p.origin <> ?1
                     GROUP BY k ORDER BY MAX(a.updated_at) DESC, k LIMIT ?2",
                )?;
                st.query_map(params![origin, MAX_SUGGEST_SITES as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut out = Vec::new();
            for k in keys {
                if out.len() == MAX_SUGGESTIONS {
                    break;
                }
                let site = site_of_key(c, &k)?;
                if !site
                    .origins
                    .iter()
                    .any(|o| same_host_family(origin, &o.origin))
                {
                    continue;
                }
                let (answered, at_path, titled, shared): (bool, bool, bool, Option<String>) = c
                    .query_row(
                        "WITH m(o) AS (SELECT origin FROM live_sites WHERE site = ?2 UNION SELECT ?2),
                              pg AS (SELECT p.path, a.title FROM live_pages p
                                     JOIN artifacts a ON a.id = p.artifact_id AND a.deleted_at IS NULL
                                     WHERE p.origin = ?2)
                         SELECT
                           EXISTS(SELECT 1 FROM live_site_answers x
                              WHERE ((x.a = ?1 AND x.b IN (SELECT o FROM m)) OR (x.b = ?1 AND x.a IN (SELECT o FROM m)))
                              AND (x.answer = 'never' OR x.until > ?5)),
                           EXISTS(SELECT 1 FROM pg WHERE path = ?3),
                           ?4 <> '' AND EXISTS(SELECT 1 FROM pg WHERE title = ?4 COLLATE NOCASE),
                           (SELECT pg.path FROM pg JOIN live_pages own ON own.path = pg.path AND own.origin = ?1
                              JOIN artifacts oa ON oa.id = own.artifact_id AND oa.deleted_at IS NULL
                              ORDER BY pg.path LIMIT 1)",
                        params![origin, k, path, title, now],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )?;
                if answered {
                    continue;
                }
                let (reason, at) = if at_path {
                    ("path", Some(path.to_string()))
                } else if titled {
                    ("title", None)
                } else if let Some(p) = shared {
                    ("path", Some(p))
                } else {
                    continue;
                };
                out.push(Suggestion {
                    origin: site.newest().to_string(),
                    site,
                    reason,
                    path: at,
                });
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArtifactId;
    use crate::live::PageKey;
    use crate::store::site::{KIND_JOIN, MoveBy};
    use crate::store::test_util::{anchor, session, store};
    use crate::store::threads::NewThread;

    const A: &str = "http://localhost:7702";
    const B: &str = "http://localhost:7703";
    const C: &str = "http://127.0.0.1:7704";

    fn key(origin: &str, path: &str) -> PageKey {
        PageKey {
            origin: origin.into(),
            path: path.into(),
        }
    }

    fn page(st: &Store, origin: &str, path: &str, title: &str) -> ArtifactId {
        let e = st
            .ensure_live_page(
                &key(origin, path),
                title,
                Some(format!("<p>{origin}{path}").as_bytes()),
            )
            .unwrap();
        ArtifactId::parse(&e.artifact.id).unwrap()
    }

    fn thread(st: &Store, id: &ArtifactId) -> String {
        let n = st.get_artifact(id).unwrap().unwrap().current_version;
        st.create_live_thread(
            id,
            NewThread {
                author_public_id: None,
                version_n: n,
                anchor: anchor(),
                author_name: "Ann".into(),
                body: "a note".into(),
                clip: None,
                via_page: false,
            },
            None,
            None,
        )
        .unwrap()
        .unwrap()
        .id
    }

    fn by() -> MoveBy {
        MoveBy {
            by: "viewer:x".into(),
            kind: KIND_JOIN,
            rule_id: None,
        }
    }

    /// Runs a join to its end as the route does: batches of re-filing,
    /// then settling.
    fn finish(st: &Store, key: &str) -> Vec<String> {
        let mut merged = Vec::new();
        loop {
            let (moves, remaining) = st.join_candidates(key, 2).unwrap();
            if !moves.is_empty() {
                st.refile_threads(&moves, &by(), &[]).unwrap();
            }
            let (empty, _) = st.settle_joined_pages(key).unwrap();
            merged.extend(empty);
            if remaining == 0 && st.join_candidates(key, 2).unwrap().0.is_empty() {
                return merged;
            }
        }
    }

    /// One batch of a join, as the route sends it: a join left unfinished.
    fn one_batch(st: &Store, key: &str) {
        let (moves, _) = st.join_candidates(key, 1).unwrap();
        st.refile_threads(&moves, &by(), &[]).unwrap();
        st.settle_joined_pages(key).unwrap();
    }

    fn code_of<T: std::fmt::Debug>(r: Result<T>) -> String {
        match r {
            Err(CoreError::Invalid { code, .. }) => code.to_string(),
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_joined_origin_resolves_to_its_site_by_path() {
        let (_d, st) = store();
        let home = page(&st, A, "/", "App");
        let about = page(&st, B, "/about", "About");
        let j = st.join_origins(B, A).unwrap();
        assert!(j.changed);
        assert_eq!(j.site.key, A);
        assert_eq!(j.site.newest(), B, "the joining origin was used last");
        assert_eq!(j.rekeyed, vec![about.as_str().to_string()]);
        // Either origin names the site's page of a path.
        for o in [A, B] {
            assert_eq!(
                st.find_live_page(&key(o, "/"))
                    .unwrap()
                    .unwrap()
                    .artifact_id,
                home.as_str()
            );
            let p = st.find_live_page(&key(o, "/about")).unwrap().unwrap();
            assert_eq!(
                (p.artifact_id.as_str(), p.origin.as_str()),
                (about.as_str(), A)
            );
        }
        // A new page of either origin is the site's.
        let e = st.ensure_live_page(&key(B, "/new"), "New", None).unwrap();
        assert_eq!(e.origin, A);
        assert_eq!(
            st.find_live_page(&key(A, "/new"))
                .unwrap()
                .unwrap()
                .artifact_id,
            e.artifact.id
        );
        assert_eq!(st.site_pages(B).unwrap().len(), 3);
        // Again: nothing changes.
        let again = st.join_origins(B, A).unwrap();
        assert!(!again.changed && again.rekeyed.is_empty());
        let again = st.join_origins(A, B).unwrap();
        assert!(!again.changed);
    }

    #[test]
    fn same_path_pages_merge_their_threads_in_batches_and_the_emptied_page_is_kept_whole() {
        let (_d, st) = store();
        let a = page(&st, A, "/", "App");
        let ta = thread(&st, &a);
        let b = page(&st, B, "/", "App");
        // A snapshot no thread names: kept with its page.
        st.store_snapshot(&b, "App", b"<p>only B's", false).unwrap();
        let tb: Vec<String> = (0..3).map(|_| thread(&st, &b)).collect();
        let j = st.join_origins(B, A).unwrap();
        assert!(j.rekeyed.is_empty(), "the site has a page of /");
        let (first, remaining) = st.join_candidates(A, 2).unwrap();
        assert_eq!((first.len(), remaining), (2, 1));
        assert!(first.iter().all(|(to, _)| to == a.as_str()));
        let merged = finish(&st, A);
        assert_eq!(merged, vec![b.as_str().to_string()]);
        // Merged away, not deleted: its artifact and every snapshot stay,
        // out of the site's pages and no longer a key.
        assert!(st.get_artifact(&b).unwrap().is_some());
        let (file, _) = st.file_path(&b, 2, "index.html").unwrap().unwrap();
        assert_eq!(std::fs::read(file).unwrap(), b"<p>only B's");
        assert!(st.live_page_of(&b).unwrap().is_none());
        assert_eq!(st.site_pages(B).unwrap().len(), 1);
        assert!(st.live_sites().unwrap().iter().all(|x| x.pages == 1));
        // Split off, B's path is its own again: a new page, the old kept.
        st.split_origin(B).unwrap().unwrap();
        let fresh = st.ensure_live_page(&key(B, "/"), "App", None).unwrap();
        assert_ne!(fresh.artifact.id, b.as_str());
        let mut on_a: Vec<String> = st
            .list_threads(&a, true, None, 50)
            .unwrap()
            .0
            .into_iter()
            .map(|t| t.id)
            .collect();
        on_a.sort();
        let mut want = tb.clone();
        want.push(ta);
        want.sort();
        assert_eq!(on_a, want);
        // Each move is recorded as a join, and the thread keeps its history.
        let kinds: Vec<String> = st
            .with_read(|c| {
                let mut s = c.prepare("SELECT kind FROM thread_moves WHERE thread_id = ?1")?;
                let v = s
                    .query_map(params![tb[0]], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(v)
            })
            .unwrap();
        assert_eq!(kinds, vec!["join".to_string()]);
        assert!(st.join_candidates(A, 2).unwrap().0.is_empty());
    }

    #[test]
    fn a_join_or_split_waits_for_an_unfinished_join() {
        let (_d, st) = store();
        let a = page(&st, A, "/x", "App");
        thread(&st, &a);
        let b = page(&st, B, "/x", "App");
        thread(&st, &b);
        thread(&st, &b);
        page(&st, C, "/y", "C");
        st.join_origins(B, A).unwrap();
        one_batch(&st, A);
        assert_eq!(st.joining_threads(A).unwrap(), 1);
        // Pending pages stay listed, marked so.
        let pages = st.site_pages(A).unwrap();
        assert!(
            pages
                .iter()
                .any(|p| p.pending && p.page.artifact_id == b.as_str())
        );
        // Another join, either way, and any split wait for it.
        assert_eq!(code_of(st.join_origins(C, A)), "joining");
        assert_eq!(code_of(st.join_origins(A, C)), "joining");
        assert_eq!(code_of(st.split_origin(B)), "joining");
        assert_eq!(code_of(st.split_origin(A)), "joining");
        // The same join again finishes it; then they go through.
        assert!(!st.join_origins(B, A).unwrap().changed);
        finish(&st, A);
        assert_eq!(st.joining_threads(A).unwrap(), 0);
        assert!(st.join_origins(C, A).unwrap().changed);
        assert!(st.split_origin(B).unwrap().is_some());
    }

    #[test]
    fn a_join_waits_for_an_unmerge_under_way() {
        let (_d, st) = store();
        page(&st, A, "/users/1", "App");
        page(&st, B, "/", "App");
        let pat = crate::live::PathPattern::parse("/users/:id").unwrap();
        let (rule, _) = st.add_live_rule(A, &pat).unwrap();
        st.add_live_rule(B, &pat).unwrap();
        st.mark_rule_deleted(&rule.id).unwrap();
        assert_eq!(code_of(st.join_origins(B, A)), "unmerging");
        assert_eq!(code_of(st.join_origins(A, B)), "unmerging");
        st.drop_rule(&rule.id).unwrap();
        st.join_origins(B, A).unwrap();
        assert_eq!(st.live_rules(A).unwrap().len(), 1, "B's rule is the site's");
    }

    #[test]
    fn not_now_keeps_a_pair_unsuggested_for_a_day() {
        let (_d, st) = store();
        page(&st, A, "/", "App");
        let t0 = chrono::Utc::now();
        st.answer_join_at(B, A, ANSWER_LATER, t0).unwrap();
        let at = |h: i64| {
            st.join_suggestions_at(B, "/", "", t0 + chrono::Duration::hours(h))
                .unwrap()
                .len()
        };
        assert_eq!(at(0), 0);
        assert_eq!(at(23), 0);
        assert_eq!(at(25), 1, "suggested again after a day");
        st.answer_join_at(B, A, ANSWER_NEVER, t0).unwrap();
        assert_eq!(at(24 * 365), 0);
    }

    #[test]
    fn removing_a_scope_removes_what_it_made_on_an_origin_split_off() {
        let (_d, st) = store();
        let a = page(&st, A, "/", "App");
        st.join_origins(B, A).unwrap();
        let sid = session(&st, "claude", "h2");
        st.live_watch(&sid, &key(A, "/"), true).unwrap();
        // A, the key, is split off: the site's pages go with the key to B.
        st.split_origin(A).unwrap().unwrap();
        assert_eq!(st.live_page_of(&a).unwrap().unwrap().origin, B);
        let removed = st.live_unwatch(&sid, &key(A, "/")).unwrap();
        assert_eq!(removed, vec![a.as_str().to_string()]);
    }

    #[test]
    fn a_scope_watch_on_any_origin_covers_the_whole_site_and_origins_joined_later() {
        let (_d, st) = store();
        let a = page(&st, A, "/", "App");
        let sid = session(&st, "claude", "h1");
        st.live_watch(&sid, &key(B, "/"), true).unwrap();
        let watching = |id: &ArtifactId| {
            st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM watches WHERE session_id = ?1 AND artifact_id = ?2)",
                    params![sid, id.as_str()],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .unwrap()
        };
        assert!(!watching(&a), "not joined yet");
        st.join_origins(B, A).unwrap();
        assert!(watching(&a), "the join makes B's scope cover A's pages");
        let later = page(&st, A, "/later", "Later");
        assert!(watching(&later), "and pages made later");
        let c_page = page(&st, C, "/c", "C");
        st.join_origins(C, B).unwrap();
        assert!(watching(&c_page), "and the pages of an origin joined later");
        // Removing the scope removes what it alone justified.
        let removed = st.live_unwatch(&sid, &key(B, "/")).unwrap();
        assert_eq!(removed.len(), 3);
    }

    #[test]
    fn splitting_keeps_history_with_the_site_and_restores_per_origin_keying() {
        let (_d, st) = store();
        let a = page(&st, A, "/", "App");
        st.join_origins(B, A).unwrap();
        st.join_origins(C, A).unwrap();
        assert_eq!(st.joined_site(C).unwrap().origins.len(), 3);
        // A member: its new pages are its own again; the site keeps its pages.
        let s = st.split_origin(B).unwrap().unwrap();
        assert_eq!(s.site.key, A);
        assert_eq!(s.site.origins.len(), 2);
        assert!(st.find_live_page(&key(B, "/")).unwrap().is_none());
        assert_eq!(
            st.find_live_page(&key(C, "/"))
                .unwrap()
                .unwrap()
                .artifact_id,
            a.as_str()
        );
        assert!(st.split_origin(B).unwrap().is_none(), "split already");
        // The key: the site's pages move to the newest origin left, which is
        // then alone, so the site is that origin's own.
        let s = st.split_origin(A).unwrap().unwrap();
        assert_eq!(s.site.key, C);
        assert!(!s.site.joined());
        assert_eq!(s.rekeyed, vec![a.as_str().to_string()]);
        assert_eq!(
            st.find_live_page(&key(C, "/"))
                .unwrap()
                .unwrap()
                .artifact_id,
            a.as_str()
        );
        assert!(st.find_live_page(&key(A, "/")).unwrap().is_none());
        // A split pair is not suggested again.
        let none = st.join_suggestions(A, "/", "App").unwrap();
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn joining_carries_rules_and_refuses_what_it_cannot_hold() {
        let (_d, st) = store();
        page(&st, A, "/", "App");
        page(&st, B, "/users/1", "U");
        let pat = crate::live::PathPattern::parse("/users/:id").unwrap();
        st.add_live_rule(B, &pat).unwrap();
        st.join_origins(B, A).unwrap();
        let rules = st.live_rules(A).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].origin, A);
        assert_eq!(st.live_rules(B).unwrap(), rules, "B's rules are its site's");
        let r = st.resolve_live_key(&key(B, "/users/2")).unwrap();
        assert_eq!(r.key, key(A, "/users/:id"));
        assert!(
            matches!(st.join_origins(A, A), Err(CoreError::Invalid { code, .. }) if code == "same_origin")
        );
        assert!(
            matches!(st.join_origins(A, "http://localhost:9"), Err(CoreError::Invalid { code, .. }) if code == "unknown_site")
        );
    }

    #[test]
    fn a_move_between_origins_of_one_site_is_allowed_and_not_across_sites() {
        let (_d, st) = store();
        let a = page(&st, A, "/", "App");
        let t = thread(&st, &a);
        let other = page(&st, "http://localhost:9999", "/", "Other");
        let mv = |to: &ArtifactId| {
            st.refile_threads(
                &[(
                    to.as_str().to_string(),
                    Refile {
                        thread_id: t.clone(),
                        live_path: None,
                        route: None,
                    },
                )],
                &MoveBy {
                    by: "viewer:x".into(),
                    kind: crate::store::site::KIND_MOVE,
                    rule_id: None,
                },
                &[],
            )
        };
        assert!(
            matches!(mv(&other), Err(CoreError::Invalid { code, .. }) if code == "cross_origin")
        );
        st.join_origins("http://localhost:9999", A).unwrap();
        // Joined: the other origin's page of / is pending, and a move onto it is within the site.
        assert!(mv(&other).is_ok());
    }

    #[test]
    fn suggestions_name_same_family_sites_with_a_matching_path_or_title() {
        let (_d, st) = store();
        page(&st, A, "/settings", "My App");
        page(&st, "http://example.com", "/settings", "My App");
        let s = st.join_suggestions(B, "/settings", "").unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(
            (s[0].origin.as_str(), s[0].reason, s[0].path.as_deref()),
            (A, "path", Some("/settings"))
        );
        let s = st.join_suggestions(B, "/nowhere", "my app").unwrap();
        assert_eq!((s.len(), s[0].reason), (1, "title"));
        assert!(
            st.join_suggestions(B, "/nowhere", "Other")
                .unwrap()
                .is_empty()
        );
        // B's own pages' paths count too.
        page(&st, B, "/settings", "x");
        assert_eq!(st.join_suggestions(B, "/nowhere", "").unwrap().len(), 1);
        // Answered: not now, then never.
        st.answer_join(B, A, ANSWER_LATER).unwrap();
        assert!(st.join_suggestions(B, "/settings", "").unwrap().is_empty());
        st.answer_join(B, A, ANSWER_NEVER).unwrap();
        assert!(st.join_suggestions(B, "/settings", "").unwrap().is_empty());
        assert!(st.answer_join(B, A, "maybe").is_err());
        // A joined origin is offered nothing.
        st.join_origins(C, A).unwrap();
        assert!(st.join_suggestions(C, "/settings", "").unwrap().is_empty());
    }

    #[test]
    fn the_listing_has_one_entry_per_site() {
        let (_d, st) = store();
        let a = page(&st, A, "/", "App");
        thread(&st, &a);
        page(&st, B, "/b", "B");
        page(&st, "http://localhost:9999", "/", "Other");
        assert_eq!(st.live_sites().unwrap().len(), 3);
        st.join_origins(B, A).unwrap();
        let sites = st.live_sites().unwrap();
        assert_eq!(sites.len(), 2);
        let s = sites.iter().find(|s| s.site.key == A).unwrap();
        assert_eq!((s.pages, s.threads, s.site.origins.len()), (2, 1, 2));
        assert!(st.touch_site_origin(A).unwrap());
        assert!(!st.touch_site_origin("http://localhost:9999").unwrap());
    }
}

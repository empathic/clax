//! Site analytics on a hosted Clax (migration 68): each artifact's setting
//! (`artifacts.analytics`), its daily totals (`analytics_days`), totals
//! per dimension value (`analytics_dims`) and load timing histograms
//! (`analytics_timings`), and the current day's secret
//! (`analytics_day_secrets`) and seen set (`analytics_seen`).
//!
//! - **Writes come in batches** ([`Store::analytics_record`]): the server
//!   sums views in memory and writes them in one transaction, so a busy
//!   site costs a few page writes a minute, not several a view. A batch
//!   for an artifact deleted meanwhile adds nothing.
//! - **Distinct values are capped**: an artifact's day takes at most
//!   [`VALUE_CAP`] values of each [`Dim::capped`] dimension; a later value
//!   is counted as [`OTHER`].
//! - **The day's secret and seen set live a day**: the rollover
//!   ([`Store::analytics_rollover`]) deletes every earlier day's secret at
//!   once and their seen rows in bounded passes, after which no hash of
//!   that day can be recomputed or matched.
//! - **Totals are kept [`RETENTION_MONTHS`](clax_core::analytics::RETENTION_MONTHS) calendar months**
//!   ([`Store::analytics_prune`], in bounded passes).
//! - A deleted artifact's totals go in its deletion's transaction
//!   (`Store::delete_artifact`).
//!
//! Every time is the store's clock's ([`Store::now_utc`]); days are UTC.

use super::Store;
use chrono::{Days, NaiveDate};
use clax_core::analytics::{
    Batch, Counts, DayTotals, Dim, Metric, OTHER, RangeTotals, Seen, VALUE_CAP, VersionMark,
    day_key, oldest_kept,
};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::HashMap;

/// What a bounded deletion pass did: how many rows it deleted, and whether
/// rows it may delete are left for the next pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pass {
    pub deleted: usize,
    pub more: bool,
}

impl Store {
    /// Whether artifact `aid`'s views are counted by its setting: it exists,
    /// is not deleted, is not a draft (`artifacts.hidden`; a draft is never
    /// counted) and its `analytics` is `on`.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_on(&self, aid: &ArtifactId) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT 1 FROM artifacts
                  WHERE id = ?1 AND deleted_at IS NULL AND hidden = 0 AND analytics = 'on'",
                params![aid.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })
    }

    /// Sets artifact `aid`'s analytics setting (`on` when `on`), recorded
    /// under `ctx` as `artifact.update` with `fields.analytics` when it
    /// changes. What was counted is kept either way.
    ///
    /// # Errors
    /// `NotFound` when the artifact does not exist or is deleted; the
    /// database's.
    pub fn set_analytics(&self, ctx: &AuditCtx, aid: &ArtifactId, on: bool) -> Result<()> {
        let value = if on { "on" } else { "off" };
        self.with_tx(|tx| {
            let before: Option<String> = tx
                .query_row(
                    "SELECT analytics FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                    params![aid.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(before) = before else {
                return Err(CoreError::NotFound);
            };
            if before == value {
                return Ok(());
            }
            tx.execute(
                "UPDATE artifacts SET analytics = ?2 WHERE id = ?1",
                params![aid.as_str(), value],
            )?;
            let mut fields = serde_json::Map::new();
            fields.insert("analytics".into(), value.into());
            let mut rec =
                AuditRecord::new(AuditKind::ArtifactUpdate, self.now()).with("fields", fields);
            rec.ids.artifact = Some(aid.as_str().to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// The secret of day `day` (`YYYY-MM-DD`), made of 32 random bytes the
    /// first time it is asked for and kept until the day's rollover.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_day_secret(&self, day: &str) -> Result<[u8; 32]> {
        let read = |c: &rusqlite::Connection| -> Result<Option<[u8; 32]>> {
            let v: Option<Vec<u8>> = c
                .query_row(
                    "SELECT secret FROM analytics_day_secrets WHERE day = ?1",
                    params![day],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(v.and_then(|v| <[u8; 32]>::try_from(v).ok()))
        };
        if let Some(s) = self.with_read(read)? {
            return Ok(s);
        }
        // Read again in the write's turn: another request may have made it.
        self.with_tx(|tx| {
            if let Some(s) = read(tx)? {
                return Ok(s);
            }
            let s = clax_core::secret::random_bytes::<32>();
            tx.execute(
                "INSERT OR REPLACE INTO analytics_day_secrets (day, secret) VALUES (?1, ?2)",
                params![day, s.as_slice()],
            )?;
            Ok(s)
        })
    }

    /// What the stored seen set of day `day` says of a view of `path` by
    /// `visitor`: its first of the day unless the visitor's own row
    /// (`path` `""`) is there, its first of the page unless the page's row
    /// is.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_seen(&self, day: &str, visitor: &[u8; 16], path: &str) -> Result<Seen> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT 1 FROM analytics_seen WHERE day = ?1 AND visitor = ?2 AND path = ?3",
            )?;
            let mut has = |p: &str| -> Result<bool> {
                Ok(stmt
                    .query_row(params![day, visitor.as_slice(), p], |_| Ok(()))
                    .optional()?
                    .is_some())
            };
            let first_of_day = !has("")?;
            let first_on_page = !has(path)?;
            Ok(Seen {
                first_of_day,
                first_on_page,
            })
        })
    }

    /// Writes `batch` in one transaction: adds its sums to each artifact's
    /// day (skipping an artifact deleted since), each value of a capped
    /// dimension past the day's [`VALUE_CAP`] counted as [`OTHER`], and
    /// adds its seen rows.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_record(&self, batch: &Batch) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        self.with_tx(|tx| record_in(tx, batch))
    }

    /// The day's rollover, in one transaction: writes `batch` (the old
    /// day's views not yet written, or an empty one), then deletes the
    /// secret of every day before today and at most `limit` of those days'
    /// seen rows. `more` says seen rows of those days are left for the next
    /// pass.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_rollover(&self, batch: &Batch, limit: usize) -> Result<Pass> {
        let today = day_key(self.now_utc());
        self.with_tx(|tx| {
            record_in(tx, batch)?;
            let mut deleted = tx.execute(
                "DELETE FROM analytics_day_secrets WHERE day < ?1",
                params![today],
            )?;
            deleted += tx.execute(
                "DELETE FROM analytics_seen WHERE (day, visitor, path) IN
                    (SELECT day, visitor, path FROM analytics_seen WHERE day < ?1 LIMIT ?2)",
                params![today, limit_of(limit)],
            )?;
            let more = tx
                .query_row(
                    "SELECT 1 FROM analytics_seen WHERE day < ?1 LIMIT 1",
                    params![today],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            Ok(Pass { deleted, more })
        })
    }

    /// The earliest day whose secret or seen rows are still stored, if any:
    /// a day before today's means a rollover is due.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_open_day(&self) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT min(day) FROM (
                    SELECT min(day) AS day FROM analytics_day_secrets
                    UNION ALL SELECT min(day) FROM analytics_seen)",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// Deletes at most `limit` rows of daily totals (`analytics_days`,
    /// `analytics_dims`, `analytics_timings`) of days before the oldest one
    /// kept ([`oldest_kept`]: [`RETENTION_MONTHS`](clax_core::analytics::RETENTION_MONTHS) calendar months before
    /// today). `more` says such rows are left for the next pass.
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_prune(&self, limit: usize) -> Result<Pass> {
        let cutoff = oldest_kept(self.now_utc().date_naive())
            .format("%Y-%m-%d")
            .to_string();
        self.with_tx(|tx| {
            let mut left = limit;
            let mut deleted = 0;
            for (table, key) in [
                ("analytics_days", "artifact_id, day"),
                ("analytics_dims", "artifact_id, day, dim, value"),
                ("analytics_timings", "artifact_id, day, metric, bucket"),
            ] {
                if left == 0 {
                    break;
                }
                let n = tx.execute(
                    &format!(
                        "DELETE FROM {table} WHERE ({key}) IN
                            (SELECT {key} FROM {table} WHERE day < ?1 LIMIT ?2)"
                    ),
                    params![cutoff, limit_of(left)],
                )?;
                deleted += n;
                left -= n.min(left);
            }
            let more = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM analytics_days WHERE day < ?1)
                         OR EXISTS(SELECT 1 FROM analytics_dims WHERE day < ?1)
                         OR EXISTS(SELECT 1 FROM analytics_timings WHERE day < ?1)",
                params![cutoff],
                |r| r.get(0),
            )?;
            Ok(Pass { deleted, more })
        })
    }

    /// Artifact `aid`'s stored totals from day `from` to day `to`, both
    /// included: every day of the range (zeros included), each dimension's
    /// values and each metric's histogram summed over it, and the versions
    /// published in it. Dimensions this build does not know are left out.
    /// The batch not yet written is the caller's to merge
    /// ([`RangeTotals::merge_day`]).
    ///
    /// # Errors
    /// The database's.
    pub fn analytics_range(
        &self,
        aid: &ArtifactId,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<RangeTotals> {
        let key = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        let (f, t) = (key(from), key(to));
        let mut out = RangeTotals::default();
        let mut d = from;
        while d <= to {
            out.days.push(DayTotals {
                day: key(d),
                views: 0,
                visitors: 0,
                framed: 0,
            });
            let Some(next) = d.checked_add_days(Days::new(1)) else {
                break;
            };
            d = next;
        }
        self.with_read(|c| {
            let a = aid.as_str();
            let mut stmt = c.prepare(
                "SELECT day, views, visitors, framed FROM analytics_days
                  WHERE artifact_id = ?1 AND day BETWEEN ?2 AND ?3",
            )?;
            let rows = stmt.query_map(params![a, f, t], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, u64>(3)?,
                ))
            })?;
            let index: HashMap<String, usize> = out
                .days
                .iter()
                .enumerate()
                .map(|(i, d)| (d.day.clone(), i))
                .collect();
            for row in rows {
                let (day, views, visitors, framed) = row?;
                if let Some(&i) = index.get(&day) {
                    let d = &mut out.days[i];
                    (d.views, d.visitors, d.framed) = (views, visitors, framed);
                }
            }
            let mut stmt = c.prepare(
                "SELECT dim, value, sum(views), sum(visitors) FROM analytics_dims
                  WHERE artifact_id = ?1 AND day BETWEEN ?2 AND ?3
                  GROUP BY dim, value",
            )?;
            let rows = stmt.query_map(params![a, f, t], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, u64>(3)?,
                ))
            })?;
            for row in rows {
                let (dim, value, views, visitors) = row?;
                if let Some(dim) = Dim::parse(&dim) {
                    out.dims
                        .entry(dim)
                        .or_default()
                        .insert(value, Counts { views, visitors });
                }
            }
            let mut stmt = c.prepare(
                "SELECT metric, bucket, sum(n) FROM analytics_timings
                  WHERE artifact_id = ?1 AND day BETWEEN ?2 AND ?3
                  GROUP BY metric, bucket",
            )?;
            let rows = stmt.query_map(params![a, f, t], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u8>(1)?,
                    r.get::<_, u64>(2)?,
                ))
            })?;
            for row in rows {
                let (metric, b, n) = row?;
                if let Some(m) = Metric::parse(&metric) {
                    out.timings.entry(m).or_default().add(b, n);
                }
            }
            let mut stmt = c.prepare(
                "SELECT n, created_at FROM versions
                  WHERE artifact_id = ?1 AND substr(created_at, 1, 10) BETWEEN ?2 AND ?3
                  ORDER BY n",
            )?;
            out.versions = stmt
                .query_map(params![a, f, t], |r| {
                    Ok(VersionMark {
                        n: r.get(0)?,
                        published_at: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(())
        })?;
        Ok(out)
    }
}

/// `limit` as SQLite's `LIMIT` takes it.
fn limit_of(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// Deletes artifact `aid`'s daily totals, in its deletion's transaction.
pub(crate) fn delete_artifact_in(tx: &Transaction<'_>, aid: &str) -> Result<()> {
    for table in ["analytics_days", "analytics_dims", "analytics_timings"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE artifact_id = ?1"),
            params![aid],
        )?;
    }
    Ok(())
}

/// [`Store::analytics_record`]'s writes, in `tx`.
fn record_in(tx: &Transaction<'_>, batch: &Batch) -> Result<()> {
    let mut live =
        tx.prepare_cached("SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL")?;
    let mut day = tx.prepare_cached(
        "INSERT INTO analytics_days (artifact_id, day, views, visitors, framed)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (artifact_id, day) DO UPDATE SET
            views = views + excluded.views,
            visitors = visitors + excluded.visitors,
            framed = framed + excluded.framed",
    )?;
    let mut has_value = tx.prepare_cached(
        "SELECT 1 FROM analytics_dims
          WHERE artifact_id = ?1 AND day = ?2 AND dim = ?3 AND value = ?4",
    )?;
    let mut values = tx.prepare_cached(
        "SELECT count(*) FROM analytics_dims
          WHERE artifact_id = ?1 AND day = ?2 AND dim = ?3 AND value <> ?4",
    )?;
    let mut dim_row = tx.prepare_cached(
        "INSERT INTO analytics_dims (artifact_id, day, dim, value, views, visitors)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (artifact_id, day, dim, value) DO UPDATE SET
            views = views + excluded.views,
            visitors = visitors + excluded.visitors",
    )?;
    let mut timing = tx.prepare_cached(
        "INSERT INTO analytics_timings (artifact_id, day, metric, bucket, n)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (artifact_id, day, metric, bucket) DO UPDATE SET n = n + excluded.n",
    )?;
    for ((aid, d), sums) in &batch.sums {
        if sums.is_empty()
            || live
                .query_row(params![aid], |_| Ok(()))
                .optional()?
                .is_none()
        {
            continue;
        }
        if sums.views > 0 || sums.visitors > 0 || sums.framed > 0 {
            day.execute(params![aid, d, sums.views, sums.visitors, sums.framed])?;
        }
        // Distinct values each capped dimension holds today, counted once
        // and kept up to date as values are added.
        let mut held: HashMap<Dim, usize> = HashMap::new();
        for ((dim, value), c) in &sums.dims {
            let mut value = value.as_str();
            if dim.capped() && value != OTHER {
                let known = has_value
                    .query_row(params![aid, d, dim.as_str(), value], |_| Ok(()))
                    .optional()?
                    .is_some();
                if !known {
                    let n = match held.get(dim) {
                        Some(&n) => n,
                        None => values.query_row(params![aid, d, dim.as_str(), OTHER], |r| {
                            r.get::<_, usize>(0)
                        })?,
                    };
                    if n >= VALUE_CAP {
                        value = OTHER;
                        held.insert(*dim, n);
                    } else {
                        held.insert(*dim, n + 1);
                    }
                }
            }
            dim_row.execute(params![aid, d, dim.as_str(), value, c.views, c.visitors])?;
        }
        for (&(metric, b), &n) in &sums.timings {
            timing.execute(params![aid, d, metric.as_str(), b, n])?;
        }
    }
    let mut seen = tx.prepare_cached(
        "INSERT OR IGNORE INTO analytics_seen (day, visitor, path) VALUES (?1, ?2, ?3)",
    )?;
    for row in &batch.seen {
        seen.execute(params![row.day, row.visitor.as_slice(), row.path])?;
    }
    Ok(())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, store_with_clock};
    use clax_core::analytics::{DaySums, Device, SeenRow, View, bucket, bucket_middle};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn artifact(st: &Store) -> ArtifactId {
        let p = clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "title": "Doc",
                "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let (a, _) = st.create_artifact(DAEMON, p, None).unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    fn view(path: &str) -> View {
        View {
            path: path.into(),
            referrer: "news.example.com".into(),
            country: "DE".into(),
            device: Device::Desktop,
            version: 1,
            framed: false,
            load_ms: Some(840),
            lcp_ms: Some(1210),
        }
    }

    fn day(s: &str) -> NaiveDate {
        clax_core::analytics::parse_day(s).unwrap()
    }

    /// A batch of `views` for `aid` on `d`, each by a new visitor.
    fn batch_of(aid: &ArtifactId, d: &str, views: &[View]) -> Batch {
        let mut sums = DaySums::default();
        for v in views {
            sums.add(
                v,
                Seen {
                    first_of_day: true,
                    first_on_page: true,
                },
            );
        }
        let mut b = Batch::default();
        b.sums
            .insert((aid.as_str().to_string(), d.to_string()), sums);
        b
    }

    fn count(st: &Store, sql: &str) -> i64 {
        st.with_read(|c| Ok(c.query_row(sql, [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn a_batch_is_read_back_by_day_and_dimension() {
        let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
        let (_d, st) = store_with_clock(clock);
        let a = artifact(&st);
        let other = artifact(&st);
        let mut b = batch_of(
            &a,
            "2026-11-18",
            &[
                view("index.html"),
                View {
                    framed: true,
                    country: "US".into(),
                    ..view("b.html")
                },
            ],
        );
        b.sums
            .extend(batch_of(&a, "2026-11-19", &[view("index.html")]).sums);
        b.sums
            .extend(batch_of(&other, "2026-11-19", &[view("x.html")]).sums);
        st.analytics_record(&b).unwrap();
        // Written again, it adds again.
        st.analytics_record(&batch_of(&a, "2026-11-19", &[view("index.html")]))
            .unwrap();

        let r = st
            .analytics_range(&a, day("2026-11-17"), day("2026-11-19"))
            .unwrap();
        let days: Vec<_> = r
            .days
            .iter()
            .map(|d| (d.day.as_str(), d.views, d.visitors, d.framed))
            .collect();
        assert_eq!(
            days,
            vec![
                ("2026-11-17", 0, 0, 0),
                ("2026-11-18", 2, 2, 1),
                ("2026-11-19", 2, 2, 0)
            ]
        );
        let totals = r.totals();
        assert_eq!((totals.views, totals.visitors, totals.framed), (4, 4, 1));
        let paths = &r.dims[&Dim::Path];
        assert_eq!(
            paths["index.html"],
            Counts {
                views: 3,
                visitors: 3
            }
        );
        assert_eq!(
            paths["b.html"],
            Counts {
                views: 1,
                visitors: 1
            }
        );
        assert!(!paths.contains_key("x.html"), "another artifact's");
        assert_eq!(r.dims[&Dim::Country]["US"].views, 1);
        assert_eq!(r.dims[&Dim::Country]["DE"].views, 3);
        assert_eq!(r.dims[&Dim::Device]["desktop"].views, 4);
        assert_eq!(r.dims[&Dim::Referrer]["news.example.com"].views, 4);
        assert_eq!(r.dims[&Dim::Version]["1"].visitors, 4);
        assert_eq!(r.median(Metric::Load), Some(bucket_middle(bucket(840))));
        assert_eq!(r.median(Metric::Lcp), Some(bucket_middle(bucket(1210))));
        // The version published today is the range's marker.
        let marks: Vec<u32> = r.versions.iter().map(|v| v.n).collect();
        assert_eq!(marks, vec![1]);
        assert!(
            r.versions[0]
                .published_at
                .starts_with("2026-11-19T10:00:00")
        );

        // A narrower range sees its days alone.
        let r = st
            .analytics_range(&a, day("2026-11-18"), day("2026-11-18"))
            .unwrap();
        assert_eq!(r.days.len(), 1);
        assert_eq!(r.dims[&Dim::Path]["index.html"].views, 1);
        assert!(r.versions.is_empty());
        // A dimension this build does not know is left out.
        st.with_tx(|tx| {
            tx.execute(
                "INSERT INTO analytics_dims (artifact_id, day, dim, value, views)
                 VALUES (?1, '2026-11-18', 'event:signup', '', 1)",
                params![a.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        let r = st
            .analytics_range(&a, day("2026-11-18"), day("2026-11-18"))
            .unwrap();
        assert_eq!(r.dims.len(), Dim::ALL.len());
    }

    #[test]
    fn a_capped_dimension_counts_values_past_500_as_other() {
        let (_d, st) = store_with_clock(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        let a = artifact(&st);
        let first: Vec<View> = (0..VALUE_CAP)
            .map(|i| view(&format!("p{i}.html")))
            .collect();
        st.analytics_record(&batch_of(&a, "2026-11-19", &first))
            .unwrap();
        st.analytics_record(&batch_of(
            &a,
            "2026-11-19",
            &[view("p0.html"), view("new1.html"), view("new2.html")],
        ))
        .unwrap();
        // Another day has its own cap.
        st.analytics_record(&batch_of(&a, "2026-11-18", &[view("new1.html")]))
            .unwrap();
        let r = st
            .analytics_range(&a, day("2026-11-19"), day("2026-11-19"))
            .unwrap();
        let paths = &r.dims[&Dim::Path];
        assert_eq!(paths.len(), VALUE_CAP + 1);
        assert_eq!(
            paths["p0.html"].views, 2,
            "a value held is counted as itself"
        );
        assert_eq!(
            paths[OTHER],
            Counts {
                views: 2,
                visitors: 2
            }
        );
        assert!(!paths.contains_key("new1.html"));
        // The referrer had one value: no cap reached; countries are not capped.
        assert_eq!(r.dims[&Dim::Referrer].len(), 1);
        let r = st
            .analytics_range(&a, day("2026-11-18"), day("2026-11-18"))
            .unwrap();
        assert_eq!(r.dims[&Dim::Path]["new1.html"].views, 1);
    }

    #[test]
    fn the_seen_set_says_first_of_day_and_first_of_page() {
        let (_d, st) = store_with_clock(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        let v = [7u8; 16];
        let w = [8u8; 16];
        let both = Seen {
            first_of_day: true,
            first_on_page: true,
        };
        assert_eq!(st.analytics_seen("2026-11-19", &v, "a.html").unwrap(), both);
        let mut b = Batch::default();
        for path in ["", "a.html"] {
            b.seen.push(SeenRow {
                day: "2026-11-19".into(),
                visitor: v,
                path: path.into(),
            });
        }
        st.analytics_record(&b).unwrap();
        // Written twice, a row is kept once.
        st.analytics_record(&b).unwrap();
        assert_eq!(count(&st, "SELECT count(*) FROM analytics_seen"), 2);
        assert_eq!(
            st.analytics_seen("2026-11-19", &v, "a.html").unwrap(),
            Seen::default()
        );
        assert_eq!(
            st.analytics_seen("2026-11-19", &v, "b.html").unwrap(),
            Seen {
                first_of_day: false,
                first_on_page: true,
            }
        );
        assert_eq!(st.analytics_seen("2026-11-19", &w, "a.html").unwrap(), both);
        assert_eq!(st.analytics_seen("2026-11-20", &v, "a.html").unwrap(), both);
    }

    #[test]
    fn the_rollover_deletes_earlier_days_secrets_and_seen_rows_in_passes() {
        let clock = Arc::new(ManualClock::at("2026-11-18T23:59:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let a = artifact(&st);
        assert_eq!(st.analytics_open_day().unwrap(), None);
        let old = st.analytics_day_secret("2026-11-18").unwrap();
        assert_eq!(st.analytics_day_secret("2026-11-18").unwrap(), old, "kept");
        let mut b = Batch::default();
        for i in 0..5u8 {
            b.seen.push(SeenRow {
                day: "2026-11-18".into(),
                visitor: [i; 16],
                path: String::new(),
            });
        }
        st.analytics_record(&b).unwrap();
        assert_eq!(
            st.analytics_open_day().unwrap().as_deref(),
            Some("2026-11-18")
        );

        clock.advance(120);
        let new = st.analytics_day_secret("2026-11-19").unwrap();
        assert_ne!(new, old);
        let mut today = Batch::default();
        today.seen.push(SeenRow {
            day: "2026-11-19".into(),
            visitor: [9; 16],
            path: String::new(),
        });
        st.analytics_record(&today).unwrap();

        // The first pass writes the old day's batch and takes the secret.
        let pending = batch_of(&a, "2026-11-18", &[view("index.html")]);
        let pass = st.analytics_rollover(&pending, 3).unwrap();
        assert_eq!(
            pass,
            Pass {
                deleted: 4,
                more: true
            }
        );
        let r = st
            .analytics_range(&a, day("2026-11-18"), day("2026-11-18"))
            .unwrap();
        assert_eq!(r.days[0].views, 1);
        assert_eq!(
            count(
                &st,
                "SELECT count(*) FROM analytics_day_secrets WHERE day = '2026-11-18'"
            ),
            0
        );
        let pass = st.analytics_rollover(&Batch::default(), 3).unwrap();
        assert_eq!(
            pass,
            Pass {
                deleted: 2,
                more: false
            }
        );
        let pass = st.analytics_rollover(&Batch::default(), 3).unwrap();
        assert_eq!(pass, Pass::default());
        // Today's are kept.
        assert_eq!(st.analytics_day_secret("2026-11-19").unwrap(), new);
        assert_eq!(count(&st, "SELECT count(*) FROM analytics_seen"), 1);
        assert_eq!(
            st.analytics_open_day().unwrap().as_deref(),
            Some("2026-11-19")
        );
    }

    #[test]
    fn retention_keeps_13_calendar_months_in_bounded_passes() {
        let clock = Arc::new(ManualClock::at("2026-03-31T12:00:00Z"));
        let (_d, st) = store_with_clock(clock);
        let a = artifact(&st);
        // 2025-02-28 is the oldest day kept on 2026-03-31.
        for d in ["2025-02-26", "2025-02-27", "2025-02-28", "2026-03-31"] {
            st.analytics_record(&batch_of(&a, d, &[view("index.html")]))
                .unwrap();
        }
        let rows = |st: &Store| {
            count(
                st,
                "SELECT (SELECT count(*) FROM analytics_days WHERE day < '2025-02-28')
                      + (SELECT count(*) FROM analytics_dims WHERE day < '2025-02-28')
                      + (SELECT count(*) FROM analytics_timings WHERE day < '2025-02-28')",
            )
        };
        // Per old day: one day row, five dimension rows, two timing rows.
        assert_eq!(rows(&st), 16);
        let pass = st.analytics_prune(10).unwrap();
        assert_eq!(
            pass,
            Pass {
                deleted: 10,
                more: true
            }
        );
        assert_eq!(rows(&st), 6);
        let pass = st.analytics_prune(10).unwrap();
        assert_eq!(
            pass,
            Pass {
                deleted: 6,
                more: false
            }
        );
        assert_eq!(st.analytics_prune(10).unwrap(), Pass::default());
        let r = st
            .analytics_range(&a, day("2025-02-26"), day("2026-03-31"))
            .unwrap();
        assert_eq!(r.totals().views, 2, "2025-02-28 and today are kept");
    }

    #[test]
    fn the_setting_is_on_until_turned_off_and_each_change_is_audited() {
        let (_d, st) = store_with_clock(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        let a = artifact(&st);
        assert!(st.analytics_on(&a).unwrap());
        st.set_analytics(DAEMON, &a, false).unwrap();
        assert!(!st.analytics_on(&a).unwrap());
        st.set_analytics(DAEMON, &a, false).unwrap();
        st.set_analytics(DAEMON, &a, true).unwrap();
        assert!(st.analytics_on(&a).unwrap());
        let bodies: Vec<serde_json::Value> = st
            .with_read(|c| {
                let mut stmt = c.prepare(
                    "SELECT body FROM audit_events
                      WHERE kind = 'artifact.update' AND artifact_id = ?1 ORDER BY seq",
                )?;
                let rows = stmt
                    .query_map(params![a.as_str()], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows
                    .iter()
                    .map(|b| serde_json::from_str(b).unwrap())
                    .collect())
            })
            .unwrap();
        let fields: Vec<_> = bodies.iter().map(|b| b["fields"].clone()).collect();
        assert_eq!(
            fields,
            vec![
                serde_json::json!({"analytics": "off"}),
                serde_json::json!({"analytics": "on"})
            ],
            "an unchanged setting is not recorded"
        );

        // A draft is never counted; a missing artifact reads as off and
        // cannot be set.
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE artifacts SET hidden = 1 WHERE id = ?1",
                params![a.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(!st.analytics_on(&a).unwrap());
        let missing = ArtifactId::parse("zzzzzzzzzzzz").unwrap();
        assert!(!st.analytics_on(&missing).unwrap());
        assert!(matches!(
            st.set_analytics(DAEMON, &missing, false),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn an_artifacts_totals_go_with_it() {
        let (_d, st) = store_with_clock(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        let a = artifact(&st);
        let kept = artifact(&st);
        st.analytics_record(&batch_of(&a, "2026-11-19", &[view("index.html")]))
            .unwrap();
        st.analytics_record(&batch_of(&kept, "2026-11-19", &[view("index.html")]))
            .unwrap();
        st.delete_artifact(DAEMON, &a).unwrap();
        assert!(!st.analytics_on(&a).unwrap());
        for table in ["analytics_days", "analytics_dims", "analytics_timings"] {
            assert_eq!(
                count(
                    &st,
                    &format!(
                        "SELECT count(*) FROM {table} WHERE artifact_id = '{}'",
                        a.as_str()
                    )
                ),
                0,
                "{table}"
            );
            assert!(
                count(
                    &st,
                    &format!(
                        "SELECT count(*) FROM {table} WHERE artifact_id = '{}'",
                        kept.as_str()
                    )
                ) > 0,
                "{table}"
            );
        }
        // A batch counted before the deletion and written after adds nothing.
        st.analytics_record(&batch_of(&a, "2026-11-19", &[view("index.html")]))
            .unwrap();
        assert_eq!(
            count(
                &st,
                &format!(
                    "SELECT count(*) FROM analytics_days WHERE artifact_id = '{}'",
                    a.as_str()
                )
            ),
            0
        );
    }
}

//! Audit queries (spec 2026-10-21-claxbot-design §9.2): the audit view's
//! reads of `audit_events`, filtered and paged newest first by `seq`, and
//! scoped to what the reader may see. Admins see every event; anyone else
//! sees the events they acted in (themselves, their agents, ClaxBot acting
//! for them), the events on artifacts they created, the events on artifacts
//! shared with them while the share held, and the events of runs acting
//! for them.
//!
//! Also [`Store::record_event`], for events no store write makes: the
//! views (`view.*`) the routes record when someone opens a run, an admin
//! page, the audit trail or a usage page.

use super::Store;
use super::audit::AuditRow;
use clax_core::audit::{Actor, AuditCtx, AuditIds, AuditRecord, KindGroup};
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, ToSql, params_from_iter};
use serde::Serialize;

/// Who reads the audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditReader {
    /// The owner or an admin: every event.
    Admin,
    /// Anyone else, by person key.
    Person(String),
}

/// Whose events an audit query asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActorFilter {
    /// A person (by person key) and their agents, ClaxBot left out.
    Person(String),
    /// ClaxBot, acting for anyone.
    Bot,
}

/// An audit query; every field that is set narrows it.
#[derive(Clone, Debug, Default)]
pub struct AuditQuery {
    /// Words found in the kind, the actor or the body (case-insensitive
    /// for ASCII).
    pub q: Option<String>,
    pub actor: Option<ActorFilter>,
    pub group: Option<KindGroup>,
    pub artifact: Option<String>,
    /// From this time, inclusive (RFC 3339, as the store stamps).
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    /// Until this time, exclusive.
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    /// Events before this `seq` (a page's `next_cursor`).
    pub cursor: Option<i64>,
    /// At most this many (1 to [`MAX_PAGE`]; 0 is [`DEFAULT_PAGE`]).
    pub limit: u32,
    /// When set, the only artifacts whose events are kept, whatever else
    /// is asked (an admin's on a hosted daemon: those in their reach);
    /// events naming no artifact stay. Applied before the words, the
    /// page's limit and its cursor, so neither a page's length nor its
    /// cursor tells the others are there, and an artifact made since the
    /// list was read is left out.
    pub within: Option<Vec<String>>,
    /// Artifacts whose events are left out, as [`AuditQuery::within`]
    /// leaves out the rest (a non-owner's on a hosted daemon: the live
    /// pages, the owner's alone); only `artifact_id` is compared.
    pub hidden: Vec<String>,
    /// When set, a `view.audit` recorded by anyone but this person (a
    /// person key) matches `q` by its kind and actor alone, never by its
    /// body, whose filters hold its viewer's search words.
    pub searcher: Option<String>,
}

pub const DEFAULT_PAGE: u32 = 50;
pub const MAX_PAGE: u32 = 200;

/// One event as the audit view lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuditEvent {
    pub seq: i64,
    pub at: String,
    pub kind: String,
    /// The actor, as stored.
    pub actor: serde_json::Value,
    /// The person ClaxBot acted for, for a ClaxBot event.
    pub on_behalf_of: Option<String>,
    /// The step's one-sentence description, as Toolpath renders it.
    pub summary: String,
    pub artifact_id: Option<String>,
    /// The ClaxBot run the event belongs to.
    pub run: Option<String>,
    /// The body as stored (envelope included), for the event's detail.
    pub body: serde_json::Value,
    #[serde(skip)]
    pub ids: AuditIds,
}

/// A page of events, newest first.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuditPage {
    pub events: Vec<AuditEvent>,
    /// The cursor of the next page, when there is one.
    pub next_cursor: Option<i64>,
}

const COLS: &str = "seq, at, kind, actor, artifact_id, artifact2_id, thread_id, session_id,
    question_id, call_id, origin, body, backfilled";

/// `s` with `\`, `%` and `_` escaped for `LIKE … ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('%');
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// The SQL condition "the event concerns `person`" for a non-admin reader,
/// on `audit_events e`, its one parameter bound to `?{n}`: they acted in it
/// (as themselves, through their agent, or through ClaxBot acting for
/// them), it is on an artifact they created, it belongs to a run acting
/// for them, or it is on an artifact shared with them while the share held.
/// A view (`view.audit`, `view.run`) names its artifact for an admin's
/// reach alone: what someone else looked at is not an event of the
/// artifact's other readers.
///
/// A share's period is read from the trail itself: from an
/// `artifact.update` setting a share naming the person to the next one
/// removing it, or to the person's revocation (`person.revoke`), which
/// leaves the share's row in place but ends what the trail shows them: a
/// person who connects again under the same public ID sees the artifact's
/// later events only once a share is set again. A share made before the
/// person had a public ID (by GitHub login alone) names them only once
/// bound, so the share as it is now also counts, from when it was made,
/// unless the person was revoked since.
fn scope_sql(n: usize) -> String {
    let shared_by_trail = format!(
        "EXISTS (SELECT 1 FROM audit_events s
           WHERE s.artifact_id = e.artifact_id AND s.seq <= e.seq AND s.kind = 'artifact.update'
             AND json_extract(s.body, '$.fields.share.op') = 'set'
             AND json_extract(s.body, '$.fields.share.person') = ?{n}
             AND NOT EXISTS (SELECT 1 FROM audit_events r
               WHERE r.artifact_id = e.artifact_id AND r.seq > s.seq AND r.seq <= e.seq
                 AND r.kind = 'artifact.update'
                 AND json_extract(r.body, '$.fields.share.op') = 'remove'
                 AND json_extract(r.body, '$.fields.share.person') = ?{n})
             AND NOT EXISTS (SELECT 1 FROM audit_events r
               WHERE r.kind = 'person.revoke' AND r.seq > s.seq AND r.seq <= e.seq
                 AND json_extract(r.body, '$.person') = ?{n}))"
    );
    format!(
        "((json_extract(e.actor, '$.type') = 'viewer' AND json_extract(e.actor, '$.public_id') = ?{n})
          OR (json_extract(e.actor, '$.type') = 'agent' AND json_extract(e.actor, '$.person') = ?{n})
          OR (json_extract(e.actor, '$.type') = 'bot' AND json_extract(e.actor, '$.on_behalf_of') = ?{n})
          OR json_extract(e.body, '$.run') IN (SELECT id FROM bot_runs WHERE person = ?{n})
          OR (e.kind NOT IN ('view.audit', 'view.run') AND (
             e.artifact_id IN (SELECT id FROM artifacts WHERE person = ?{n})
             OR e.artifact2_id IN (SELECT id FROM artifacts WHERE person = ?{n})
             OR (e.artifact_id IS NOT NULL AND ({shared_by_trail}
              OR EXISTS (SELECT 1 FROM artifact_shares sh JOIN viewers v ON v.id = sh.viewer_id
                 WHERE sh.artifact_id = e.artifact_id AND v.public_id = ?{n}
                   AND sh.created_at <= e.at
                   AND NOT EXISTS (SELECT 1 FROM audit_events r
                     WHERE r.kind = 'person.revoke' AND r.seq <= e.seq AND r.at >= sh.created_at
                       AND json_extract(r.body, '$.person') = ?{n})))))))"
    )
}

/// The SQL condition of `filter`, its parameter (when any) bound to `?{n}`.
fn actor_sql(filter: &ActorFilter, n: usize) -> (String, Option<String>) {
    match filter {
        ActorFilter::Bot => ("json_extract(actor, '$.type') = 'bot'".into(), None),
        ActorFilter::Person(p) if p == super::roles::OWNER => (
            "(json_extract(actor, '$.type') = 'owner'
              OR (json_extract(actor, '$.type') = 'agent' AND json_extract(actor, '$.person') IS NULL))"
                .into(),
            None,
        ),
        ActorFilter::Person(p) => (
            format!(
                "((json_extract(actor, '$.type') = 'viewer' AND json_extract(actor, '$.public_id') = ?{n})
                  OR (json_extract(actor, '$.type') = 'agent' AND json_extract(actor, '$.person') = ?{n}))"
            ),
            Some(p.clone()),
        ),
    }
}

/// Whether `actor` (as stored) is person `person` (a person key), as
/// [`ActorFilter::Person`] selects it: the person, or their agent.
pub fn acted_as(actor: &serde_json::Value, person: &str) -> bool {
    let ty = actor["type"].as_str();
    if person == super::roles::OWNER {
        return ty == Some("owner") || (ty == Some("agent") && actor["person"].is_null());
    }
    (ty == Some("viewer") && actor["public_id"].as_str() == Some(person))
        || (ty == Some("agent") && actor["person"].as_str() == Some(person))
}

/// The kinds `group` holds, as an SQL condition on `kind` (range scans of
/// `audit_events_kind` for the prefixed groups).
fn group_sql(group: KindGroup) -> &'static str {
    match group {
        KindGroup::Views => "(kind >= 'view.' AND kind < 'view/')",
        KindGroup::Bot => "(kind >= 'bot.' AND kind < 'bot/')",
        KindGroup::Admin => "(kind >= 'admin.' AND kind < 'admin/')",
        KindGroup::Credits => "kind IN ('admin.credits', 'admin.cost_line')",
        KindGroup::Changes => {
            "NOT ((kind >= 'view.' AND kind < 'view/') OR (kind >= 'bot.' AND kind < 'bot/')
                  OR (kind >= 'admin.' AND kind < 'admin/'))"
        }
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditRow> {
    Ok(AuditRow {
        seq: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        actor: r.get(3)?,
        ids: AuditIds {
            artifact: r.get(4)?,
            artifact2: r.get(5)?,
            thread: r.get(6)?,
            session: r.get(7)?,
            question: r.get(8)?,
            call: r.get(9)?,
            origin: r.get(10)?,
        },
        body: r.get(11)?,
        backfilled: r.get(12)?,
    })
}

fn event_of(r: AuditRow, install: &str) -> AuditEvent {
    let actor: serde_json::Value = serde_json::from_str(&r.actor).unwrap_or_default();
    let body: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
    let on_behalf_of = match serde_json::from_str::<Actor>(&r.actor) {
        Ok(Actor::Bot { on_behalf_of }) => Some(on_behalf_of),
        _ => None,
    };
    let run = body.get("run").and_then(|v| v.as_str()).map(str::to_string);
    AuditEvent {
        summary: crate::toolpath::describe(&r, install),
        seq: r.seq,
        at: r.at,
        kind: r.kind,
        actor,
        on_behalf_of,
        artifact_id: r.ids.artifact.clone(),
        run,
        body,
        ids: r.ids,
    }
}

impl Store {
    /// The events `query` selects that `reader` may see, newest first, a
    /// page at a time.
    ///
    /// # Errors
    /// `invalid_audit_query` for a limit over [`MAX_PAGE`]; the store's.
    pub fn query_audit(&self, reader: &AuditReader, query: &AuditQuery) -> Result<AuditPage> {
        let limit = match query.limit {
            0 => DEFAULT_PAGE,
            n if n <= MAX_PAGE => n,
            _ => {
                return Err(CoreError::invalid(
                    "invalid_audit_query",
                    format!("a page holds at most {MAX_PAGE} events"),
                ));
            }
        };
        let mut conds: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        fn bind(args: &mut Vec<Box<dyn ToSql>>, v: Box<dyn ToSql>) -> usize {
            args.push(v);
            args.len()
        }
        if let AuditReader::Person(p) = reader {
            let n = bind(&mut args, Box::new(p.clone()));
            conds.push(scope_sql(n));
        }
        if let Some(within) = &query.within {
            let ids = serde_json::to_string(within).unwrap_or_else(|_| "[]".into());
            let n = bind(&mut args, Box::new(ids));
            conds.push(format!(
                "(artifact_id IS NULL OR artifact_id IN (SELECT value FROM json_each(?{n})))
                 AND (artifact2_id IS NULL OR artifact2_id IN (SELECT value FROM json_each(?{n})))"
            ));
        }
        if !query.hidden.is_empty() {
            let ids = serde_json::to_string(&query.hidden).unwrap_or_else(|_| "[]".into());
            let n = bind(&mut args, Box::new(ids));
            conds.push(format!(
                "(artifact_id IS NULL OR artifact_id NOT IN (SELECT value FROM json_each(?{n})))"
            ));
        }
        if let Some(q) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            let n = bind(&mut args, Box::new(like_escape(q)));
            let body = match &query.searcher {
                Some(p) => {
                    let (own, arg) = actor_sql(&ActorFilter::Person(p.clone()), args.len() + 1);
                    if let Some(arg) = arg {
                        bind(&mut args, Box::new(arg));
                    }
                    format!("(body LIKE ?{n} ESCAPE '\\' AND (kind <> 'view.audit' OR {own}))")
                }
                None => format!("body LIKE ?{n} ESCAPE '\\'"),
            };
            conds.push(format!(
                "(kind LIKE ?{n} ESCAPE '\\' OR actor LIKE ?{n} ESCAPE '\\' OR {body})"
            ));
        }
        if let Some(a) = &query.actor {
            let next = args.len() + 1;
            let (sql, arg) = actor_sql(a, next);
            if let Some(arg) = arg {
                bind(&mut args, Box::new(arg));
            }
            conds.push(sql);
        }
        if let Some(g) = query.group {
            conds.push(group_sql(g).into());
        }
        if let Some(a) = &query.artifact {
            let n = bind(&mut args, Box::new(a.clone()));
            conds.push(format!("(artifact_id = ?{n} OR artifact2_id = ?{n})"));
        }
        if let Some(t) = query.from {
            let n = bind(&mut args, Box::new(clax_core::time::stamp(t)));
            conds.push(format!("at >= ?{n}"));
        }
        if let Some(t) = query.to {
            let n = bind(&mut args, Box::new(clax_core::time::stamp(t)));
            conds.push(format!("at < ?{n}"));
        }
        if let Some(c) = query.cursor {
            let n = bind(&mut args, Box::new(c));
            conds.push(format!("seq < ?{n}"));
        }
        let n = bind(&mut args, Box::new(i64::from(limit) + 1));
        let filter = if conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conds.join(" AND "))
        };
        let sql =
            format!("SELECT {COLS} FROM audit_events e {filter} ORDER BY seq DESC LIMIT ?{n}");
        let mut rows: Vec<AuditRow> = self.with_read(|c| {
            let mut q = c.prepare(&sql)?;
            let rows = q.query_map(params_from_iter(args.iter().map(|a| a.as_ref())), row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let next_cursor = more.then(|| rows.last().map(|r| r.seq)).flatten();
        let install = self.install_id()?;
        Ok(AuditPage {
            events: rows.into_iter().map(|r| event_of(r, &install)).collect(),
            next_cursor,
        })
    }

    /// Event `seq`, when `reader` may see it.
    ///
    /// # Errors
    /// The store's.
    pub fn audit_event(&self, reader: &AuditReader, seq: i64) -> Result<Option<AuditEvent>> {
        let found: Option<AuditRow> = self.with_read(|c| {
            Ok(match reader {
                AuditReader::Admin => c
                    .prepare_cached(&format!("SELECT {COLS} FROM audit_events WHERE seq = ?1"))?
                    .query_row([seq], row)
                    .optional()?,
                AuditReader::Person(p) => c
                    .prepare_cached(&format!(
                        "SELECT {COLS} FROM audit_events e WHERE seq = ?1 AND {}",
                        scope_sql(2)
                    ))?
                    .query_row(rusqlite::params![seq, p], row)
                    .optional()?,
            })
        })?;
        let install = self.install_id()?;
        Ok(found.map(|r| event_of(r, &install)))
    }

    /// Records `rec` under `ctx` on its own: for the events no store write
    /// makes (`view.run`, `view.admin`, `view.audit`, `view.usage`).
    /// Returns its `seq`.
    ///
    /// # Errors
    /// The store's.
    pub fn record_event(&self, ctx: &AuditCtx, rec: AuditRecord) -> Result<i64> {
        self.with_tx(|tx| self.record_audit(tx, ctx, rec))
    }

    /// Records `bot.refused {person, reason, artifact?}` under `ctx` (who
    /// asked): a send to ClaxBot, or a new artifact ClaxBot was to build,
    /// refused before any model call for `reason` (`no_flag`,
    /// `credits_exhausted`, `not_configured`, `model_not_priced`).
    /// Returns its `seq`.
    ///
    /// # Errors
    /// The store's.
    pub fn record_refused(
        &self,
        ctx: &AuditCtx,
        person: &str,
        reason: &str,
        artifact: Option<&str>,
    ) -> Result<i64> {
        let mut rec = AuditRecord::new(clax_core::audit::AuditKind::BotRefused, self.now())
            .with("person", person)
            .with("reason", reason);
        if let Some(a) = artifact {
            rec = rec.with("artifact", a);
            rec.ids.artifact = Some(a.to_string());
        }
        self.record_event(ctx, rec)
    }

    /// The `bot.refused` events of the week `at` falls in (Monday 00:00
    /// UTC to the next), counted by the person refused.
    ///
    /// # Errors
    /// The store's.
    pub fn refused_in_week(
        &self,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<std::collections::BTreeMap<String, i64>> {
        let start = crate::credits::week_start(at);
        let (from, to) = (
            clax_core::time::stamp(start),
            clax_core::time::stamp(crate::credits::week_end(start)),
        );
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT json_extract(body, '$.person'), count(*) FROM audit_events
                  WHERE kind = 'bot.refused' AND at >= ?1 AND at < ?2
                  GROUP BY 1",
            )?;
            let rows = q.query_map(rusqlite::params![from, to], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
            })?;
            let mut out = std::collections::BTreeMap::new();
            for row in rows {
                if let (Some(p), n) = row? {
                    out.insert(p, n);
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_runs::{NewRun, RequestKind};
    use crate::credits::KeySource;
    use crate::roles::OWNER;
    use crate::roles::tests::person;
    use crate::test_util::{DAEMON, artifact, store_with_clock};
    use clax_core::audit::{AgentActor, AuditKind};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn ctx(actor: Actor) -> AuditCtx {
        AuditCtx {
            actor,
            ..DAEMON.clone()
        }
    }

    fn viewer(p: &str) -> Actor {
        Actor::Viewer {
            public_id: p.into(),
            display_name: None,
            person: true,
        }
    }

    #[test]
    fn queries_filter_page_and_scope_what_each_reader_sees() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let sam = person(&st, "Sam", 1);
        let ana = person(&st, "Ana", 2);
        let owners = artifact(&st, None).to_string();
        // An artifact Sam made.
        st.with_write(|c| {
            c.execute(
                "UPDATE artifacts SET person = ?1 WHERE id = ?2",
                rusqlite::params![sam, owners],
            )?;
            Ok(())
        })
        .unwrap();
        let sams = owners.clone();
        let other = artifact(&st, None).to_string();
        clock.advance(60);
        let t1 = st.now_utc();
        // Sam views the audit; Ana's agent views usage; ClaxBot runs for Ana.
        let seen = st
            .record_event(
                &ctx(viewer(&sam)),
                AuditRecord::new(AuditKind::ViewAudit, st.now()).with("filters", "quarterly"),
            )
            .unwrap();
        st.record_event(
            &ctx(Actor::Agent(AgentActor {
                person: Some(ana.clone()),
                ..Default::default()
            })),
            AuditRecord::new(AuditKind::ViewUsage, st.now()).with("subject_person", ana.as_str()),
        )
        .unwrap();
        let run = st
            .create_run(
                &ctx(Actor::Bot {
                    on_behalf_of: ana.clone(),
                }),
                NewRun {
                    artifact_id: other.clone(),
                    person: ana.clone(),
                    request: RequestKind::Brief,
                    thread_ids: vec![],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                },
            )
            .unwrap();
        // A system event naming Ana's run, which only the run ties to her.
        let mut sys = AuditRecord::new(AuditKind::BotCall, st.now()).with("run", run.id.as_str());
        sys.ids.artifact = Some(other.clone());
        st.record_event(DAEMON, sys).unwrap();

        let all = st
            .query_audit(&AuditReader::Admin, &AuditQuery::default())
            .unwrap();
        assert!(all.events.len() >= 6, "{}", all.events.len());
        assert!(
            all.events.windows(2).all(|w| w[0].seq > w[1].seq),
            "newest first"
        );

        // Sam: their own view and everything on their artifact.
        let sams_view = st
            .query_audit(&AuditReader::Person(sam.clone()), &AuditQuery::default())
            .unwrap();
        assert!(sams_view.events.iter().any(|e| e.seq == seen));
        assert!(
            sams_view
                .events
                .iter()
                .any(|e| e.artifact_id.as_deref() == Some(sams.as_str()))
        );
        assert!(
            sams_view
                .events
                .iter()
                .all(|e| e.artifact_id.as_deref() != Some(other.as_str()))
        );
        // Ana: her agent's view, her run's start and its system event.
        let anas = st
            .query_audit(&AuditReader::Person(ana.clone()), &AuditQuery::default())
            .unwrap();
        let kinds: Vec<&str> = anas.events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["bot.call", "bot.run_start", "view.usage"],
            "{kinds:?}"
        );
        assert_eq!(anas.events[1].on_behalf_of.as_deref(), Some(ana.as_str()));
        assert_eq!(anas.events[0].run.as_deref(), Some(run.id.as_str()));
        assert!(!anas.events[1].summary.is_empty());

        // Filters.
        let q = |query: AuditQuery| st.query_audit(&AuditReader::Admin, &query).unwrap().events;
        let views = q(AuditQuery {
            group: Some(KindGroup::Views),
            ..Default::default()
        });
        assert_eq!(views.len(), 2);
        assert_eq!(
            q(AuditQuery {
                group: Some(KindGroup::Bot),
                ..Default::default()
            })
            .len(),
            2
        );
        assert!(
            q(AuditQuery {
                group: Some(KindGroup::Changes),
                ..Default::default()
            })
            .iter()
            .all(|e| !e.kind.starts_with("bot.") && !e.kind.starts_with("view."))
        );
        let by_bot = q(AuditQuery {
            actor: Some(ActorFilter::Bot),
            ..Default::default()
        });
        assert_eq!(by_bot.len(), 1);
        let by_sam = q(AuditQuery {
            actor: Some(ActorFilter::Person(sam.clone())),
            ..Default::default()
        });
        assert_eq!(by_sam.iter().map(|e| e.seq).collect::<Vec<_>>(), [seen]);
        assert!(
            q(AuditQuery {
                actor: Some(ActorFilter::Person(OWNER.into())),
                ..Default::default()
            })
            .is_empty(),
            "the daemon made the rest"
        );
        let text = q(AuditQuery {
            q: Some("QUARTERLY".into()),
            from: Some(t1),
            ..Default::default()
        });
        assert_eq!(text.len(), 1);
        assert!(
            q(AuditQuery {
                q: Some("100%".into()),
                ..Default::default()
            })
            .is_empty(),
            "% is literal"
        );
        let on_other = q(AuditQuery {
            artifact: Some(other.clone()),
            ..Default::default()
        });
        assert!(
            on_other
                .iter()
                .all(|e| e.artifact_id.as_deref() == Some(other.as_str()))
        );
        assert!(on_other.iter().any(|e| e.kind == "bot.call"));
        assert!(on_other.iter().any(|e| e.kind == "artifact.create"));
        assert!(
            q(AuditQuery {
                to: Some(t1),
                group: Some(KindGroup::Views),
                ..Default::default()
            })
            .is_empty()
        );

        // Pages.
        let first = st
            .query_audit(
                &AuditReader::Admin,
                &AuditQuery {
                    limit: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(first.events.len(), 2);
        let second = st
            .query_audit(
                &AuditReader::Admin,
                &AuditQuery {
                    limit: 2,
                    cursor: first.next_cursor,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(second.events[0].seq < first.events[1].seq);
        assert!(
            st.query_audit(
                &AuditReader::Admin,
                &AuditQuery {
                    limit: MAX_PAGE + 1,
                    ..Default::default()
                }
            )
            .is_err()
        );

        // One event, scoped.
        assert!(
            st.audit_event(&AuditReader::Person(sam.clone()), seen)
                .unwrap()
                .is_some()
        );
        assert!(
            st.audit_event(&AuditReader::Person(ana.clone()), seen)
                .unwrap()
                .is_none()
        );
        assert!(st.audit_event(&AuditReader::Admin, seen).unwrap().is_some());
    }

    #[test]
    fn a_share_shows_its_artifact_s_events_while_it_held() {
        use crate::shares::{ShareAccess, ShareTo};
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let ana = person(&st, "Ana", 2);
        let aid = artifact(&st, None).to_string();
        let on = |what: &str| {
            let mut rec = AuditRecord::new(AuditKind::BotCancel, st.now()).with("run", what);
            rec.ids.artifact = Some(aid.clone());
            st.record_event(DAEMON, rec).unwrap()
        };
        let before = on("before");
        clock.advance(1);
        let share = st
            .share_artifact(
                DAEMON,
                &aid,
                &ShareTo::Person(ana.clone()),
                ShareAccess::Viewer,
                None,
            )
            .unwrap();
        clock.advance(1);
        let during = on("during");
        clock.advance(1);
        st.unshare_artifact(DAEMON, &aid, &share.id).unwrap();
        clock.advance(1);
        let after = on("after");
        let anas: Vec<i64> = st
            .query_audit(&AuditReader::Person(ana.clone()), &AuditQuery::default())
            .unwrap()
            .events
            .into_iter()
            .map(|e| e.seq)
            .collect();
        assert!(anas.contains(&during), "{anas:?}");
        assert!(
            !anas.contains(&before) && !anas.contains(&after),
            "{anas:?}"
        );
        let reader = AuditReader::Person(ana);
        assert!(st.audit_event(&reader, during).unwrap().is_some());
        assert!(st.audit_event(&reader, after).unwrap().is_none());
    }

    #[test]
    fn revoking_a_person_ends_their_shares_in_the_trail_even_after_they_reconnect() {
        use crate::people::InvitePerson;
        use crate::people::tests::grant;
        use crate::shares::{ShareAccess, ShareTo};
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let ana = person(&st, "Ana", 2);
        let aid = artifact(&st, None).to_string();
        let on = |what: &str| {
            clock.advance(1);
            let mut rec = AuditRecord::new(AuditKind::BotCancel, st.now()).with("run", what);
            rec.ids.artifact = Some(aid.clone());
            st.record_event(DAEMON, rec).unwrap()
        };
        clock.advance(1);
        st.share_artifact(
            DAEMON,
            &aid,
            &ShareTo::Person(ana.clone()),
            ShareAccess::Viewer,
            None,
        )
        .unwrap();
        let shared = on("shared");
        clock.advance(1);
        st.revoke_person(DAEMON, &ana).unwrap();
        // Revocation keeps the share's row: only the trail ends the period.
        assert_eq!(st.artifact_shares(&aid).unwrap().len(), 1);
        let revoked = on("revoked");
        // Ana connects again under the same public ID.
        clock.advance(1);
        let (_, c) = st
            .create_invite(DAEMON, InvitePerson::Viewer(ana.clone()), None)
            .unwrap();
        st.request_grant(DAEMON, grant([9; 32], Some(c.expose())))
            .unwrap();
        assert!(
            st.people()
                .unwrap()
                .iter()
                .any(|p| p.viewer.public_id == ana && p.viewer.person_revoked_at.is_none())
        );
        let back = on("back");
        let reader = AuditReader::Person(ana.clone());
        let seen: Vec<i64> = st
            .query_audit(&reader, &AuditQuery::default())
            .unwrap()
            .events
            .into_iter()
            .map(|e| e.seq)
            .collect();
        assert!(seen.contains(&shared), "{seen:?}");
        assert!(
            !seen.contains(&revoked) && !seen.contains(&back),
            "{seen:?}"
        );
        // A share set again opens a new period.
        clock.advance(1);
        let again = st.artifact_shares(&aid).unwrap()[0].id.clone();
        st.unshare_artifact(DAEMON, &aid, &again).unwrap();
        clock.advance(1);
        st.share_artifact(
            DAEMON,
            &aid,
            &ShareTo::Person(ana.clone()),
            ShareAccess::Viewer,
            None,
        )
        .unwrap();
        let reshared = on("reshared");
        assert!(st.audit_event(&reader, reshared).unwrap().is_some());
        assert!(st.audit_event(&reader, back).unwrap().is_none());
    }
}

//! `clax audit`: the audit trail (`GET /api/audit`, `GET /api/audit/<seq>`;
//! docs/contract.md, "The audit view"), as the owner by hand reads it. The
//! routes answer the owner and the people they connect, never an agent: run
//! from an agent's session (the agent channel), the command is refused
//! (`not_for_agents`) as the route refuses it, and on a remote Clax, whose
//! requests are a grant's, it is not offered.

use crate::client::Client;
use clax_core::Home;
use clax_core::audit::KindGroup;
use clax_core::audit::view::{AuditEvent, AuditEventDetail, AuditListQuery, AuditPage};
use std::time::Duration;

#[derive(clap::Args)]
pub struct Args {
    /// One event, with its stored body, by its seq.
    pub seq: Option<i64>,
    /// Events at or after this time (RFC 3339, or YYYY-MM-DD).
    #[arg(long, value_name = "TIME")]
    pub since: Option<String>,
    /// Events before this time (RFC 3339, or YYYY-MM-DD).
    #[arg(long, value_name = "TIME")]
    pub until: Option<String>,
    /// A kind (`artifact.publish`), the kinds under a prefix (`bot.*`), or a
    /// group: views, changes, bot, credits or admin.
    #[arg(long, value_name = "KIND")]
    pub kind: Option<String>,
    /// `claxbot`, or a person key (`p_…`): that person and their agents.
    #[arg(long, value_name = "ACTOR")]
    pub actor: Option<String>,
    /// Events naming this artifact, by ID or URL.
    #[arg(long, value_name = "ID|URL")]
    pub artifact: Option<String>,
    /// Words found in the kind, the actor or the body.
    #[arg(long, value_name = "WORDS")]
    pub search: Option<String>,
    /// At most this many events (1 to 200; by default 50).
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// After the events, print each new one as it is recorded, until
    /// interrupted.
    #[arg(long, conflicts_with = "seq")]
    pub follow: bool,
}

/// How long `--follow` waits between polls.
const POLL: Duration = Duration::from_secs(2);

/// The route's query for `a`'s filters. `--kind` names a group when it is
/// one of the five, else a kind.
///
/// # Errors
/// An `--artifact` that is no artifact ID or URL.
pub fn query(a: &Args) -> anyhow::Result<AuditListQuery> {
    let (group, kind) = match a.kind.as_deref() {
        Some(k) => match KindGroup::parse(k) {
            Some(g) => (Some(g), None),
            None => (None, Some(k.to_string())),
        },
        None => (None, None),
    };
    let artifact = match &a.artifact {
        Some(t) => Some(
            clax_mcp::tools::artifact_ref(t)
                .map_err(|_| anyhow::anyhow!("'{t}' is not an artifact ID or URL"))?
                .0,
        ),
        None => None,
    };
    Ok(AuditListQuery {
        q: a.search.clone(),
        actor: a.actor.clone(),
        group,
        kind,
        artifact,
        from: a.since.clone(),
        to: a.until.clone(),
        cursor: None,
        limit: a.limit,
    })
}

/// The events of `page` after `seq`, oldest first.
pub fn newer(page: &AuditPage, seq: i64) -> Vec<AuditEvent> {
    let mut out: Vec<AuditEvent> = page
        .events
        .iter()
        .filter(|e| e.seq > seq)
        .cloned()
        .collect();
    out.sort_by_key(|e| e.seq);
    out
}

/// One poll of `--follow`: the events `q` matches after `last`, oldest
/// first, by `fetch`. It asks `q` itself (no cursor), so the daemon records
/// the view once a window, as any reader's is; only when a whole page is
/// new does it page back, by cursor, to `last`.
///
/// # Errors
/// `fetch`'s.
pub fn poll(
    q: &AuditListQuery,
    last: i64,
    mut fetch: impl FnMut(&AuditListQuery) -> anyhow::Result<AuditPage>,
) -> anyhow::Result<Vec<AuditEvent>> {
    let mut page = fetch(q)?;
    let mut fresh = newer(&page, last);
    while !fresh.is_empty() && page.events.iter().all(|e| e.seq > last) {
        let Some(cursor) = page.next_cursor else {
            break;
        };
        page = fetch(&AuditListQuery {
            cursor: Some(cursor),
            ..q.clone()
        })?;
        let mut older = newer(&page, last);
        older.append(&mut fresh);
        fresh = older;
    }
    Ok(fresh)
}

/// `SEQ  AT  ACTOR  KIND  SUMMARY`.
fn line(e: &AuditEvent) -> String {
    let actor = e.actor.name.as_deref().unwrap_or(&e.actor.id);
    format!(
        "{}  {}  {}  {}  {}",
        e.seq,
        e.at,
        crate::term::clean_line(actor),
        e.kind,
        crate::term::clean_line(&e.summary)
    )
}

fn page(c: &Client, q: &AuditListQuery) -> anyhow::Result<AuditPage> {
    let qs = q.to_query();
    let path = if qs.is_empty() {
        "/api/audit".to_string()
    } else {
        format!("/api/audit?{qs}")
    };
    Ok(serde_json::from_value(c.get(&path)?)?)
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    if let Some(seq) = a.seq {
        let d: AuditEventDetail = serde_json::from_value(c.get(&format!("/api/audit/{seq}"))?)?;
        let v = serde_json::to_value(&d)?;
        super::print(cli, v, |_| {
            format!(
                "{}\n{}",
                line(&d.event),
                serde_json::to_string_pretty(&d.body).unwrap_or_default()
            )
        });
        return Ok(());
    }
    let q = query(a)?;
    let first = page(&c, &q)?;
    if !a.follow {
        let v = serde_json::to_value(&first)?;
        super::print(cli, v, |_| {
            let mut events = first.events.clone();
            events.reverse();
            events.iter().map(line).collect::<Vec<_>>().join("\n")
        });
        return Ok(());
    }
    // Following: one event a line (JSON with --json), oldest first.
    let show = |e: &AuditEvent| {
        if cli.json {
            println!("{}", serde_json::to_string(e).unwrap_or_default());
        } else {
            println!("{}", line(e));
        }
    };
    let mut last = 0;
    for e in newer(&first, 0) {
        last = e.seq;
        show(&e);
    }
    loop {
        std::thread::sleep(POLL);
        for e in poll(&q, last, |q| page(&c, q))? {
            last = last.max(e.seq);
            show(&e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::audit::view::AuditActor;
    use std::cell::RefCell;

    fn ev(seq: i64) -> AuditEvent {
        AuditEvent {
            seq,
            at: "2026-11-02T00:00:00Z".into(),
            actor: AuditActor {
                id: "*owner".into(),
                name: None,
            },
            on_behalf_of: None,
            kind: "artifact.publish".into(),
            summary: format!("event {seq}"),
            artifact_id: None,
            run: None,
        }
    }

    /// Pages of `limit`, newest first, of the events 1 to `newest`.
    fn pages(newest: i64, limit: usize) -> impl Fn(&AuditListQuery) -> AuditPage {
        move |q| {
            let below = q.cursor.unwrap_or(i64::MAX);
            let all: Vec<AuditEvent> = (1..=newest).rev().filter(|s| *s < below).map(ev).collect();
            AuditPage {
                next_cursor: (all.len() > limit).then(|| all[limit - 1].seq),
                events: all.into_iter().take(limit).collect(),
            }
        }
    }

    fn args() -> Args {
        Args {
            seq: None,
            since: None,
            until: None,
            kind: None,
            actor: None,
            artifact: None,
            search: None,
            limit: None,
            follow: false,
        }
    }

    #[test]
    fn kind_names_a_group_or_a_kind() {
        let q = query(&Args {
            kind: Some("bot".into()),
            since: Some("2026-11-01".into()),
            ..args()
        })
        .unwrap();
        assert_eq!((q.group, q.kind.as_deref()), (Some(KindGroup::Bot), None));
        assert_eq!(q.from.as_deref(), Some("2026-11-01"));
        let q = query(&Args {
            kind: Some("bot.*".into()),
            ..args()
        })
        .unwrap();
        assert_eq!((q.group, q.kind.as_deref()), (None, Some("bot.*")));
        assert!(
            query(&Args {
                artifact: Some("not an artifact".into()),
                ..args()
            })
            .is_err()
        );
    }

    #[test]
    fn a_poll_asks_the_query_and_pages_back_only_for_a_whole_new_page() {
        let asked = RefCell::new(Vec::new());
        let q = AuditListQuery {
            kind: Some("artifact.publish".into()),
            ..Default::default()
        };
        let fetch = |newest| {
            let p = pages(newest, 3);
            let asked = &asked;
            move |q: &AuditListQuery| {
                asked.borrow_mut().push(q.cursor);
                Ok(p(q))
            }
        };
        // Nothing new: one ask, no cursor.
        assert!(poll(&q, 5, fetch(5)).unwrap().is_empty());
        // Two new, within a page: one ask.
        let got: Vec<i64> = poll(&q, 5, fetch(7))
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(got, [6, 7]);
        assert_eq!(*asked.borrow(), [None, None]);
        // Five new, more than a page: back by cursor to the last seen.
        asked.borrow_mut().clear();
        let got: Vec<i64> = poll(&q, 5, fetch(10))
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(got, [6, 7, 8, 9, 10]);
        assert_eq!(*asked.borrow(), [None, Some(8)]);
    }
}

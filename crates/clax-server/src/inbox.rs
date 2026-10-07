//! The owner's inbox in the daemon (spec 2026-10-06-agent-questions-and-inbox
//! §8): item views rendered from their sources, and the task that turns
//! committed inbox changes into `inbox` topic events.

use crate::state::AppState;
use clax_core::store::inbox::{InboxChange, ItemRow, Kind, Sources, ThreadRef};
use clax_core::{Event, Store};
use serde_json::{Value, json};

/// A transaction that changed more items than this is announced as one
/// `inbox_read {ids: null}` ("refetch what you show") instead of an event
/// per item.
pub const MAX_ITEM_EVENTS: usize = 50;

/// The view of spec §7.6 for `i`.
pub fn view(st: &Store, i: &ItemRow) -> clax_core::Result<Value> {
    Ok(views(st, std::slice::from_ref(i))?.remove(0))
}

/// The views of `items`, in the same order, their sources read in one
/// snapshot.
pub fn views(st: &Store, items: &[ItemRow]) -> clax_core::Result<Vec<Value>> {
    let sources = st.inbox_sources(items)?;
    items
        .iter()
        .zip(sources)
        .map(|(i, s)| render(st, i, s))
        .collect()
}

fn thread_json(t: &ThreadRef) -> Value {
    json!({"id": t.id, "summary": t.summary})
}

/// The last component of a working directory.
fn project(cwd: &str) -> &str {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// `i`'s view from its sources `s`. A missing source makes the item `gone`;
/// the view then keeps what the item holds (kind, time, agent, IDs).
fn render(st: &Store, i: &ItemRow, s: Sources) -> clax_core::Result<Value> {
    let agent = s
        .agent
        .as_ref()
        .map(|a| json!({"handle": a.handle, "harness": a.harness, "project": project(&a.cwd)}));
    let artifact = match (&s.artifact, &i.artifact_id) {
        (Some(a), _) => {
            json!({"id": a.id, "title": a.title, "kind": a.kind, "page_url": s.page_url})
        }
        (None, Some(aid)) => json!({"id": aid, "title": null, "kind": null, "page_url": null}),
        (None, None) => Value::Null,
    };
    let aid = i.artifact_id.as_deref().unwrap_or_default();
    let mut v = json!({
        "id": i.id,
        "seq": i.seq,
        "kind": i.kind.as_str(),
        "read": i.read_at.is_some(),
        "created_at": i.created_at,
        "agent": agent,
        "artifact": artifact,
        "thread": null,
        "reply": null,
        "version": null,
        "published": null,
        "question": null,
        "work": null,
        "gone": false,
        "url": format!("/a/{aid}"),
    });
    let live = s.artifact.is_some();
    let gone = match i.kind {
        Kind::Reply => {
            let tid = i.thread_id.as_deref().unwrap_or_default();
            v["thread"] = match &s.thread {
                Some(t) => json!({"id": t.id, "summary": t.summary, "status": t.status}),
                None => json!({"id": tid, "summary": null, "status": null}),
            };
            v["reply"] = match &s.reply {
                Some(r) => {
                    json!({"comment_id": i.comment_id, "body": r.body, "addressed": r.addressed})
                }
                None => json!({"comment_id": i.comment_id, "body": null, "addressed": false}),
            };
            v["url"] = json!(format!("/a/{aid}?thread={tid}"));
            s.reply.is_none()
        }
        Kind::Version => {
            let n = i.version_n.unwrap_or_default();
            v["version"] = match &s.version {
                Some(x) => json!({
                    "n": n,
                    "note": x.note,
                    "addressed": x.addressed.iter().map(thread_json).collect::<Vec<_>>(),
                }),
                None => json!({"n": n, "note": null, "addressed": []}),
            };
            v["url"] = json!(format!("/a/{aid}/v/{n}"));
            s.version.is_none()
        }
        Kind::Published => {
            v["published"] = json!({
                "description": s.artifact.as_ref().and_then(|a| a.description.clone()),
            });
            !live
        }
        Kind::Question => {
            let qid = i.question_id.as_deref().unwrap_or_default();
            v["url"] = json!(format!("/inbox?q={qid}"));
            match st.question(qid)? {
                Some(q) => {
                    v["question"] = crate::questions::view(st, &q)?;
                    false
                }
                None => true,
            }
        }
        Kind::Finished => {
            let message = i
                .detail
                .as_ref()
                .map_or(Value::Null, |d| d["message"].clone());
            v["work"] = json!({
                "message": message,
                "threads": s.threads.iter().map(thread_json).collect::<Vec<_>>(),
            });
            !live
        }
    };
    v["gone"] = json!(gone);
    Ok(v)
}

/// What one batch of committed changes comes to.
enum Announce {
    Items(Vec<Value>, u32),
    Refetch(u32),
}

/// The events for `changes`: one view per changed item, or a refetch when
/// more than [`MAX_ITEM_EVENTS`] changed.
fn announcement(db: &Store, changes: &[InboxChange]) -> clax_core::Result<Announce> {
    let unread = db.inbox_unread()?;
    if changes.len() > MAX_ITEM_EVENTS {
        return Ok(Announce::Refetch(unread));
    }
    let seqs: Vec<i64> = changes.iter().map(|c| c.seq).collect();
    let mut rows = db.inbox_items_by_seq(&seqs)?;
    // Oldest first, so a client adding items in arrival order keeps `seq` order.
    rows.reverse();
    Ok(Announce::Items(views(db, &rows)?, unread))
}

/// Installs the store's inbox listener: changes go through a channel to a
/// task that reads their items and publishes one `inbox_item` per item, or
/// one `inbox_read {ids: null}` when a transaction changed more than
/// [`MAX_ITEM_EVENTS`]. Batches are announced in the order their
/// transactions' listeners ran, which may differ from commit order for
/// concurrent transactions; clients order items by `seq`.
pub fn listen(s: &AppState) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<InboxChange>>();
    s.store.set_inbox_listener(Box::new(move |c| {
        let _ = tx.send(c);
    }));
    let s = s.clone();
    tokio::spawn(async move {
        while let Some(changes) = rx.recv().await {
            let out = s.store.call(move |db| announcement(db, &changes)).await;
            match out {
                Ok(Announce::Refetch(unread)) => s.events.publish(Event::InboxRead {
                    ids: None,
                    read: true,
                    unread,
                }),
                Ok(Announce::Items(views, unread)) => {
                    for item in views {
                        s.events.publish(Event::InboxItem { item, unread });
                    }
                }
                Err(e) => tracing::warn!(error = %e, "inbox change not announced"),
            }
        }
    });
}

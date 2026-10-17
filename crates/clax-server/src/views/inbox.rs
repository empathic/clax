//! The inbox in the daemon (spec 2026-10-06-agent-questions-and-inbox §8),
//! one per person (spec 2026-10-08-multi-agent-multi-person §5.3, §18):
//! whose inbox a request reads, item views rendered from their sources, and
//! the task that turns committed inbox changes into each recipient's
//! `inbox` topic events.
//!
//! Views go to people only, the owner's browsers and the CLI by hand, and
//! connected people's browsers, never to an agent (every inbox route
//! refuses agents, spec §7.5): people are not filtered (§8.5, §9.5), so a
//! view carries its sources as they are. A connected person's browser is
//! on the LAN and never sees a live page (L10): no item about one is made
//! for a person, and a view for a person names none.

use crate::error::ApiError;
use crate::identity::Identity;
use crate::state::AppState;
use clax_core::store::inbox::{InboxChange, ItemRow, Kind, Sources, ThreadRef};
use clax_core::{Event, Store};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Whose inbox and questions a request reads (spec §6.1, §18): `None` for
/// the owner (an owner browser credential, or the token outside an agent
/// channel), a connected person's public ID for their browser.
///
/// # Errors
/// 403 `forbidden` for anyone else (a LAN viewer, a revoked person, an
/// anonymous caller, an agent); the caller's resolution errors.
pub async fn recipient_of(s: &AppState, who: &Identity) -> Result<Option<String>, ApiError> {
    use crate::auth::Person;
    let person = match who.quick_caller(None) {
        Some(c) => c.person(),
        None => {
            let who = who.clone();
            s.store_call(move |st| Ok(who.person(st))).await??
        }
    };
    match person {
        Person::Owner => Ok(None),
        Person::Viewer(pid) => Ok(Some(pid)),
        Person::Agent | Person::None => Err(ApiError::forbidden(
            "forbidden",
            "only the owner or a connected person reads an inbox",
        )),
    }
}

/// A transaction that changed more items than this is announced as one
/// `inbox_read {ids: null}` ("refetch what you show") instead of an event
/// per item.
pub const MAX_ITEM_EVENTS: usize = 50;

/// The view of spec §7.6 for `i`.
pub fn view(st: &Store, i: &ItemRow) -> clax_core::Result<Value> {
    Ok(views(st, std::slice::from_ref(i))?.remove(0))
}

/// The views of `items`, in the same order. Their sources are read in
/// one snapshot ([`Store::inbox_sources`]); a question item's question is
/// read after it.
pub fn views(st: &Store, items: &[ItemRow]) -> clax_core::Result<Vec<Value>> {
    let sources = st.inbox_sources(items)?;
    let mut names = HashMap::new();
    items
        .iter()
        .zip(sources)
        .map(|(i, s)| render(st, &mut names, i, s))
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

/// The name of an agent's person (`None`: the owner) as browsers show it,
/// as stored, read once per page into `names`.
fn person_name(
    st: &Store,
    names: &mut HashMap<Option<String>, String>,
    person: &Option<String>,
) -> clax_core::Result<String> {
    if let Some(n) = names.get(person) {
        return Ok(n.clone());
    }
    let n = st.agent_person(person.as_deref())?.name;
    names.insert(person.clone(), n.clone());
    Ok(n)
}

/// `i`'s view from its sources `s`. A missing source makes the item `gone`;
/// the view then keeps what the item holds (kind, time, agent, IDs).
fn render(
    st: &Store,
    names: &mut HashMap<Option<String>, String>,
    i: &ItemRow,
    s: Sources,
) -> clax_core::Result<Value> {
    // An agent's project goes to the owner and to the agent's own person
    // alone (spec §13).
    // Its person, and whether it is the recipient's own person's, name it
    // in the author line (spec §11 "Display").
    let agent = match s.agent.as_ref() {
        Some(a) => {
            let shown = i.recipient.is_none() || i.recipient == a.person;
            let person_name = person_name(st, names, &a.person)?;
            Some(
                json!({"handle": a.handle, "name": a.name, "harness": a.harness,
                   "project": shown.then(|| project(&a.cwd)),
                   "person": a.person, "person_name": person_name,
                   "own": i.recipient == a.person}),
            )
        }
        None => None,
    };
    // A live page is the owner's alone (L10): a person, whose browser may
    // not see live pages, gets an item about one with nothing of it, its
    // ID, link and payload left out, and gone.
    if i.recipient.is_some()
        && s.artifact
            .as_ref()
            .is_some_and(|a| a.kind == clax_core::live::KIND_LIVE)
    {
        return Ok(json!({
            "id": i.id,
            "seq": i.seq,
            "kind": i.kind.as_str(),
            "read": i.read_at.is_some(),
            "created_at": i.created_at,
            "agent": agent,
            "artifact": null,
            "thread": null,
            "reply": null,
            "version": null,
            "published": null,
            "question": null,
            "work": null,
            "gone": true,
            "url": "/inbox",
        }));
    }
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
                    v["question"] = crate::views::question::view(st, &q)?;
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
        // The kinds of spec 2026-10-08-multi-agent-multi-person §5.3 carry
        // their own reference; the older kinds' views are unchanged.
        Kind::Connection => {
            let gid = i.grant_id.as_deref().unwrap_or_default();
            v["url"] = json!("/people");
            match st.grant(gid) {
                Ok(g) => {
                    v["connection"] = crate::views::grant::grant_view(&g);
                    false
                }
                // A request never decided is purged a day after it lapsed.
                Err(clax_core::CoreError::NotFound) => {
                    v["connection"] = Value::Null;
                    true
                }
                Err(e) => return Err(e),
            }
        }
        // The card (spec §9.5): the request, the withheld content itself
        // (people are not filtered) and how it came.
        Kind::Approval => {
            let rid = i.request_id.as_deref().unwrap_or_default();
            v["url"] = json!(format!("/inbox?approval={rid}"));
            if let Some(t) = &s.thread {
                v["thread"] = json!({"id": t.id, "summary": t.summary, "status": t.status});
            }
            match crate::views::approval::card(st, rid, i.recipient.as_deref(), i.detail.as_ref())?
            {
                Some(card) => {
                    v["approval"] = card;
                    false
                }
                None => {
                    v["approval"] = json!({"id": rid});
                    true
                }
            }
        }
        // The note of spec §14 (M25): what the agent's end cleaned up, its
        // released threads with their pages (none on a live page for a
        // person, L10).
        Kind::Ended => {
            v["url"] = json!("/inbox");
            v["ended"] = ended_view(st, i)?;
            false
        }
    };
    v["gone"] = json!(gone);
    Ok(v)
}

/// An `ended` item's `ended`: `{name, reason, text, questions, requests,
/// threads: [{id, artifact_id, title, url}]}`, each released thread with
/// its page's title as its reader may read it now: the current title, or
/// `null` once the page is gone. A person's note names no live page's
/// thread (the store leaves them out, L10), and a person's browser reads
/// every other artifact (spec 2026-10-08 §18).
fn ended_view(st: &Store, i: &ItemRow) -> clax_core::Result<Value> {
    let Some(d) = &i.detail else {
        return Ok(Value::Null);
    };
    let mut threads = Vec::new();
    for t in d["threads"].as_array().into_iter().flatten() {
        let (Some(aid), Some(tid)) = (t["artifact_id"].as_str(), t["thread_id"].as_str()) else {
            continue;
        };
        let art = match clax_core::ArtifactId::parse(aid) {
            Ok(id) => st.get_artifact(&id)?,
            Err(_) => None,
        };
        threads.push(json!({
            "id": tid,
            "artifact_id": aid,
            "title": art.map(|a| a.title),
            "url": format!("/a/{aid}?thread={tid}"),
        }));
    }
    Ok(json!({
        "name": d["name"],
        "reason": d["reason"],
        "text": d["text"],
        "questions": d["questions"],
        "requests": d["requests"],
        "threads": threads,
    }))
}

/// What one batch of committed changes comes to for one recipient
/// (`None`: the owner).
enum Announce {
    /// Each changed item's view, oldest first, and the unread count.
    Items(Option<String>, Vec<Value>, u32),
    /// Refetch what you show, and the unread count.
    Refetch(Option<String>, u32),
}

/// The events for `changes`, for the recipients in `held` (the recipient
/// keys a stream holds the `inbox` topic of): for each, one view per
/// changed item of theirs, or a refetch when more than
/// [`MAX_ITEM_EVENTS`] changed.
fn announcement(
    db: &Store,
    changes: &[InboxChange],
    held: &std::collections::HashSet<String>,
) -> clax_core::Result<Vec<Announce>> {
    use clax_core::store::inbox::recipient_key;
    let seqs: Vec<i64> = changes.iter().map(|c| c.seq).collect();
    let mut out = Vec::new();
    if changes.len() > MAX_ITEM_EVENTS {
        for r in db.inbox_recipients_of(&seqs)? {
            if held.contains(recipient_key(r.as_deref())) {
                let unread = db.inbox_unread(r.as_deref())?;
                out.push(Announce::Refetch(r, unread));
            }
        }
        return Ok(out);
    }
    let mut rows = db.inbox_items_by_seq(&seqs)?;
    rows.retain(|i| held.contains(recipient_key(i.recipient.as_deref())));
    // Oldest first, so a client adding items in arrival order keeps `seq` order.
    rows.reverse();
    let mut by: Vec<(Option<String>, Vec<ItemRow>)> = Vec::new();
    for i in rows {
        match by.iter_mut().find(|(r, _)| *r == i.recipient) {
            Some((_, v)) => v.push(i),
            None => by.push((i.recipient.clone(), vec![i])),
        }
    }
    for (r, rows) in by {
        let unread = db.inbox_unread(r.as_deref())?;
        out.push(Announce::Items(r, views(db, &rows)?, unread));
    }
    Ok(out)
}

/// Installs the store's inbox listener: changes go through a channel to a
/// task that reads their items and publishes, to each recipient's `inbox`
/// topic, one `inbox_item` per item of theirs, or one `inbox_read {ids:
/// null}` when a transaction changed more than [`MAX_ITEM_EVENTS`]. Changes
/// for a recipient no stream holds the topic of are dropped unread (a
/// client that subscribes fetches what it shows). Batches are announced in
/// the order their transactions' listeners ran, which may differ from
/// commit order for concurrent transactions; clients order items by `seq`.
///
/// It also ends a person's streams once the person is no longer connected
/// (revoked), at the visibility generation's bump that the revocation makes
/// (spec M17): their browser is then no one's inbox (§6.4).
pub fn listen(s: &AppState) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<InboxChange>>();
    s.store.set_inbox_listener(Box::new(move |c| {
        let _ = tx.send(c);
    }));
    let st = s.clone();
    tokio::spawn(async move {
        while let Some(changes) = rx.recv().await {
            // Nobody to tell: no view is read or rendered.
            let held = st.stream.inbox_holders();
            if held.is_empty() {
                continue;
            }
            let out = st
                .store
                .call(move |db| announcement(db, &changes, &held))
                .await;
            match out {
                Ok(batches) => {
                    for b in batches {
                        match b {
                            Announce::Refetch(recipient, unread) => {
                                st.events.publish(Event::InboxRead {
                                    ids: None,
                                    read: true,
                                    unread,
                                    recipient,
                                })
                            }
                            Announce::Items(recipient, views, unread) => {
                                for item in views {
                                    st.events.publish(Event::InboxItem {
                                        item,
                                        unread,
                                        recipient: recipient.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "inbox change not announced"),
            }
        }
    });
    let st = s.clone();
    tokio::spawn(async move {
        let mut seen = st.vis_gen.current();
        loop {
            seen = st.vis_gen.wait_after(seen).await;
            let streaming = st.stream.people_streaming();
            if streaming.is_empty() {
                continue;
            }
            match st.store.call(|db| db.connected_people()).await {
                Ok(connected) => {
                    let gone = streaming.difference(&connected).cloned().collect();
                    st.stream.end_streams_of(&gone);
                }
                Err(e) => tracing::warn!(error = %e, "revoked people's streams not checked"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::store::inbox::AgentRef;

    fn item(kind: Kind, recipient: Option<&str>) -> ItemRow {
        ItemRow {
            seq: 7,
            id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".into(),
            kind,
            artifact_id: Some("7q3k9mzx2b4t".into()),
            thread_id: None,
            comment_id: None,
            version_n: None,
            question_id: None,
            session_id: Some("s1".into()),
            harness: Some("claude".into()),
            detail: Some(json!({"message": "LIVECANARYmessage", "thread_ids": []})),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            read_at: None,
            recipient: recipient.map(Into::into),
            request_id: None,
            grant_id: None,
        }
    }

    fn sources(kind: &str, person: Option<&str>) -> Sources {
        let artifact: clax_core::model::Artifact = serde_json::from_value(json!({
            "id": "7q3k9mzx2b4t", "title": "LIVECANARYtitle", "description": null, "icon": null,
            "created_at": "x", "updated_at": "x", "current_version": 1, "pinned": false,
            "capabilities": {}, "contract_version": "1", "owner_session_id": null, "kind": kind,
        }))
        .unwrap();
        Sources {
            agent: Some(AgentRef {
                handle: "a_1".into(),
                harness: "claude".into(),
                cwd: "/home/alex/secret-project".into(),
                person: person.map(Into::into),
                name: Some("web".into()),
            }),
            artifact: Some(artifact),
            page_url: Some("http://localhost:5173/LIVECANARYpath".into()),
            ..Sources::default()
        }
    }

    #[test]
    fn a_persons_view_names_no_live_page_and_no_other_persons_project() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        // A person's item about a live page carries nothing of the page.
        let v = render(
            &st,
            &mut HashMap::new(),
            &item(Kind::Finished, Some("u_sam")),
            sources("live", None),
        )
        .unwrap();
        let text = v.to_string();
        assert!(
            !text.contains("LIVECANARY") && !text.contains("7q3k9mzx2b4t"),
            "{text}"
        );
        assert_eq!(
            (v["gone"].as_bool(), v["url"].as_str()),
            (Some(true), Some("/inbox"))
        );
        // The owner's keeps it.
        let v = render(
            &st,
            &mut HashMap::new(),
            &item(Kind::Finished, None),
            sources("live", None),
        )
        .unwrap();
        assert_eq!(v["work"]["message"], "LIVECANARYmessage");
        // An agent's project: the owner's view and its own person's only.
        let project = |recipient: Option<&str>, person: Option<&str>| {
            render(
                &st,
                &mut HashMap::new(),
                &item(Kind::Finished, recipient),
                sources("html", person),
            )
            .unwrap()["agent"]["project"]
                .clone()
        };
        assert_eq!(project(None, Some("u_sam")), "secret-project");
        assert_eq!(project(Some("u_sam"), Some("u_sam")), "secret-project");
        assert_eq!(project(Some("u_sam"), None), Value::Null);
        assert_eq!(project(Some("u_sam"), Some("u_mia")), Value::Null);
        // Its person, for the author line (spec §11 "Display").
        let agent = |person: Option<&str>| {
            render(
                &st,
                &mut HashMap::new(),
                &item(Kind::Reply, None),
                sources("html", person),
            )
            .unwrap()["agent"]
                .clone()
        };
        assert_eq!(agent(Some("u_sam"))["person"], "u_sam");
        assert_eq!(agent(None)["person"], Value::Null);
        assert_eq!(agent(None)["person_name"], "The owner");
        // Whether it is the recipient's own person's agent.
        assert_eq!(agent(None)["own"], true);
        assert_eq!(agent(Some("u_sam"))["own"], false);
    }
}

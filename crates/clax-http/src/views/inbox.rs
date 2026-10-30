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
use clax_core::Event;
use clax_store::Store;
use clax_store::inbox::{InboxChange, ItemRow, Kind, Sources, ThreadRef};
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

/// The view of spec §7.6 for `i`, on a hosted daemon when `hosted`.
pub fn view(st: &Store, i: &ItemRow, hosted: bool) -> clax_core::Result<Value> {
    Ok(views(st, std::slice::from_ref(i), hosted)?.remove(0))
}

/// The views of `items`, in the same order, on a hosted daemon when
/// `hosted`. Their sources are read in one snapshot
/// ([`Store::inbox_sources`]); a question item's question is read after
/// it. On a hosted daemon a connected person's view carries no comment
/// text (a thread's summary, a reply's body) of an artifact whose comments
/// they may not read ([`crate::hosted::reads_comments`]), as the threads
/// route, the stream, the snapshot and the live socket withhold it.
pub fn views(st: &Store, items: &[ItemRow], hosted: bool) -> clax_core::Result<Vec<Value>> {
    let sources = st.inbox_sources(items)?;
    let mut names = HashMap::new();
    let mut rights = HashMap::new();
    items
        .iter()
        .zip(sources)
        .map(|(i, s)| {
            let comments = !hosted
                || reads_comments(
                    st,
                    &mut rights,
                    i.recipient.as_deref(),
                    i.artifact_id.as_deref(),
                )?;
            let mut v = render(st, &mut names, i, s, comments)?;
            if !hosted {
                linked_tag(st, i, &mut v)?;
            }
            Ok(v)
        })
        .collect()
}

/// On a local daemon, an item on a thread linked to a remote, or on a
/// question asked on a remote too, carries `linked: {remote, unlinked,
/// author}`: the remote's name, whether the link ended, and for a reply
/// that came from there, its author there ([`clax_store::linked`]).
fn linked_tag(st: &Store, i: &ItemRow, v: &mut Value) -> clax_core::Result<()> {
    if let Some(qid) = i.question_id.as_deref()
        && let Some(lq) = st.linked_question_view(qid)?
    {
        v["linked"] = json!({"remote": lq.remote, "unlinked": lq.unlinked, "author": null});
        return Ok(());
    }
    let Some(tid) = i.thread_id.as_deref() else {
        return Ok(());
    };
    let Some(lt) = st.linked_thread(tid)? else {
        return Ok(());
    };
    let author = match i.comment_id.as_deref() {
        Some(cid) => st.remote_author_labels(&[cid.to_string()])?.remove(cid),
        None => None,
    };
    v["linked"] =
        json!({"remote": lt.remote_name, "unlinked": lt.unlinked_at.is_some(), "author": author});
    Ok(())
}

/// The artifacts among `recipient`'s items (a connected person's on a
/// hosted daemon) whose comments they may not read, as
/// [`views`] withholds their text: what a search of their inbox leaves
/// unmatched ([`clax_store::inbox::InboxQuery::text_withheld_at`]).
/// Empty for the owner and on a local daemon.
///
/// # Errors
/// The store's.
pub fn text_withheld_at(
    st: &Store,
    recipient: Option<&str>,
    hosted: bool,
) -> clax_core::Result<Vec<String>> {
    let Some(pid) = recipient.filter(|_| hosted) else {
        return Ok(Vec::new());
    };
    let mut rights = std::collections::HashMap::new();
    let mut out = Vec::new();
    for aid in st.inbox_artifacts(Some(pid))? {
        if !reads_comments(st, &mut rights, Some(pid), Some(&aid))? {
            out.push(aid);
        }
    }
    Ok(out)
}

/// The artifacts among `recipient`'s items (`None`: the owner's) that are
/// out of their reach on a hosted daemon (`hosted`), as their listings
/// decide it ([`crate::hosted::MemberRights::lists`]): what their inbox
/// leaves out, its counts included. Every one for a recipient who is no
/// person; none on a local daemon.
///
/// # Errors
/// The store's.
pub fn out_of_reach(
    st: &Store,
    recipient: Option<&str>,
    hosted: bool,
) -> clax_core::Result<Vec<String>> {
    if !hosted {
        return Ok(Vec::new());
    }
    let scope = match recipient {
        None => crate::hosted::MemberRights::owner(st),
        Some(pid) => crate::hosted::MemberRights::of_person(st, pid),
    };
    let all = st.inbox_artifacts(recipient)?;
    let scope = match scope {
        Ok(m) => m,
        Err(
            clax_core::CoreError::NotFound
            | clax_core::CoreError::Invalid {
                code: "not_a_person",
                ..
            },
        ) => return Ok(all),
        Err(e) => return Err(e),
    };
    Ok(all.into_iter().filter(|a| !scope.lists(st, a)).collect())
}

/// Whether `recipient` (`None`: the owner) reaches artifact `aid` as their
/// listings do ([`crate::hosted::MemberRights::lists`]) on a hosted daemon
/// (`hosted`): what a question or item about it needs to be theirs to see.
/// Always on a local daemon; never for a recipient who is no person.
///
/// # Errors
/// The store's.
pub fn recipient_lists(
    st: &Store,
    recipient: Option<&str>,
    hosted: bool,
    aid: &str,
) -> clax_core::Result<bool> {
    if !hosted {
        return Ok(true);
    }
    let scope = match recipient {
        None => crate::hosted::MemberRights::owner(st),
        Some(pid) => crate::hosted::MemberRights::of_person(st, pid),
    };
    match scope {
        Ok(m) => Ok(m.lists(st, aid)),
        Err(
            clax_core::CoreError::NotFound
            | clax_core::CoreError::Invalid {
                code: "not_a_person",
                ..
            },
        ) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Whether `recipient`, a connected person, may read the comments on
/// `artifact`: by their rights or the page's link, as the threads route
/// decides. The owner and an item about no artifact are not limited, nor a
/// recipient who is no person (a guest, whose link always reads comments);
/// an unknown recipient reads none. `rights` keeps each person's rights
/// across the items of one call.
fn reads_comments(
    st: &Store,
    rights: &mut std::collections::HashMap<String, Option<crate::hosted::MemberRights>>,
    recipient: Option<&str>,
    artifact: Option<&str>,
) -> clax_core::Result<bool> {
    let (Some(pid), Some(aid)) = (recipient, artifact) else {
        return Ok(true);
    };
    if !rights.contains_key(pid) {
        let member = match crate::hosted::MemberRights::of_person(st, pid) {
            Ok(m) => Some(m),
            Err(clax_core::CoreError::Invalid {
                code: "not_a_person",
                ..
            }) => None,
            Err(clax_core::CoreError::NotFound) => return Ok(false),
            Err(e) => return Err(e),
        };
        rights.insert(pid.to_string(), member);
    }
    crate::hosted::reads_comments(st, rights[pid].as_ref(), aid)
}

/// `t` as an item names it; its summary, comment text, only when its
/// reader may read the artifact's comments (`comments`).
fn thread_json(t: &ThreadRef, comments: bool) -> Value {
    json!({"id": t.id, "summary": if comments { json!(t.summary) } else { Value::Null }})
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
/// Without `comments` (its reader may not read the artifact's comments) a
/// thread's summary and a reply's body are `null`.
fn render(
    st: &Store,
    names: &mut HashMap<Option<String>, String>,
    i: &ItemRow,
    s: Sources,
    comments: bool,
) -> clax_core::Result<Value> {
    let text = |t: Value| if comments { t } else { Value::Null };
    // An agent's project goes to the owner and to the agent's own person
    // alone (spec §13).
    // Its person, and whether it is the recipient's own person's, name it
    // in the author line (spec §11 "Display").
    let agent = match s.agent.as_ref() {
        Some(a) => {
            let shown = i.recipient.is_none() || i.recipient == a.person;
            let person_name = person_name(st, names, &a.person)?;
            let mut v = json!({"handle": a.handle, "name": a.name, "harness": a.harness,
               "project": shown.then(|| project(&a.cwd)),
               "person": a.person, "person_name": person_name,
               "own": i.recipient == a.person});
            // Another Clax's agent: whose Clax it relays from.
            if a.relay {
                v["via"] = json!(clax_store::attention::relay_via(
                    v["person_name"].as_str().unwrap_or("Someone")
                ));
            }
            Some(v)
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
                Some(t) => {
                    json!({"id": t.id, "summary": text(json!(t.summary)), "status": t.status})
                }
                None => json!({"id": tid, "summary": null, "status": null}),
            };
            v["reply"] = match &s.reply {
                Some(r) => {
                    json!({"comment_id": i.comment_id, "body": text(json!(r.body)), "addressed": r.addressed})
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
                    "addressed": x.addressed.iter().map(|t| thread_json(t, comments)).collect::<Vec<_>>(),
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
                    let view = crate::views::question::view(st, &q)?;
                    // A thread question opens its thread while it has one.
                    if let (Some(on), Some(tid)) = (
                        view["artifact"]["id"].as_str(),
                        view["thread"]["id"].as_str(),
                    ) {
                        v["url"] = json!(format!("/a/{on}?thread={tid}"));
                    }
                    v["question"] = view;
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
                "threads": s.threads.iter().map(|t| thread_json(t, comments)).collect::<Vec<_>>(),
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
        // A refused automatic push: the remote, the refusal, and the
        // public copy's address.
        Kind::Push => {
            v["push"] = json!({
                "n": i.version_n,
                "origin": i.detail.as_ref().map_or(Value::Null, |d| d["origin"].clone()),
                "code": i.detail.as_ref().map_or(Value::Null, |d| d["code"].clone()),
                "message": i.detail.as_ref().map_or(Value::Null, |d| d["message"].clone()),
                "url": i.detail.as_ref().map_or(Value::Null, |d| d["url"].clone()),
            });
            !live
        }
        // A connection request (spec 2026-10-29-remote-instances §3): the
        // instance, what the agent wanted to read and why, and how the
        // request stands now; in the asking agent's thread when it was
        // working in one.
        Kind::RemoteRequest => {
            let d = i.detail.clone().unwrap_or(Value::Null);
            let rid = d["request_id"].as_str().unwrap_or_default().to_string();
            v["url"] = json!(format!("/inbox?remote_request={rid}"));
            if let (Some(t), Some(tid)) = (&s.thread, &i.thread_id) {
                v["thread"] =
                    json!({"id": tid, "summary": text(json!(t.summary)), "status": t.status});
            }
            let r = st.peek_remote_request(&rid)?;
            v["remote_request"] = json!({
                "id": rid,
                "origin": d["origin"],
                "name": d["name"],
                "url": d["url"],
                "reason": d["reason"],
                "status": r.as_ref().map(|r| r.status.clone()),
                "remote_id": r.as_ref().and_then(|r| r.remote_id.clone()),
                "expires_at": r.as_ref().map(|r| r.expires_at.clone()),
                "askers": r.as_ref().map_or(0, |r| r.askers.len()),
            });
            r.is_none()
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
    hosted: bool,
) -> clax_core::Result<Vec<Announce>> {
    use clax_store::inbox::recipient_key;
    let seqs: Vec<i64> = changes.iter().map(|c| c.seq).collect();
    let mut out = Vec::new();
    if changes.len() > MAX_ITEM_EVENTS {
        for r in db.inbox_recipients_of(&seqs)? {
            if held.contains(recipient_key(r.as_deref())) {
                let hidden = out_of_reach(db, r.as_deref(), hosted)?;
                let unread = db.inbox_unread_but(r.as_deref(), &hidden)?;
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
    for (r, mut rows) in by {
        // An item of an artifact out of the recipient's reach is not theirs
        // to hear of, nor counted.
        let hidden = out_of_reach(db, r.as_deref(), hosted)?;
        rows.retain(|i| i.artifact_id.as_ref().is_none_or(|a| !hidden.contains(a)));
        if rows.is_empty() {
            continue;
        }
        let unread = db.inbox_unread_but(r.as_deref(), &hidden)?;
        out.push(Announce::Items(r, views(db, &rows, hosted)?, unread));
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
/// (spec M17): their browser is then no one's inbox (§6.4). Returns the
/// two tasks' handles.
pub fn listen(s: &AppState) -> Vec<clax_rt::JoinHandle<()>> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<InboxChange>>();
    s.store.set_inbox_listener(Box::new(move |c| {
        let _ = tx.send(c);
    }));
    let st = s.clone();
    let hosted = s.hosted.is_some();
    let announcer = clax_rt::spawn(async move {
        while let Some(changes) = rx.recv().await {
            // Nobody to tell: no view is read or rendered.
            let held = st.stream.inbox_holders();
            if held.is_empty() {
                continue;
            }
            let out = st
                .store
                .call(move |db| announcement(db, &changes, &held, hosted))
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
                                for mut item in views {
                                    crate::views::approval::fill_card_docs(&st, &mut item).await;
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
    let revoker = clax_rt::spawn(async move {
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
    vec![announcer, revoker]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_store::inbox::AgentRef;

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
                relay: false,
            }),
            artifact: Some(artifact),
            page_url: Some("http://localhost:5173/LIVECANARYpath".into()),
            ..Sources::default()
        }
    }

    #[test]
    fn an_item_on_a_mirrored_thread_names_its_remote_and_author_there() {
        use clax_core::audit::AuditCtx;
        use clax_store::linked::{MirrorComment, MirrorThread, RemoteAuthor};
        use clax_store::remotes::{PendingSignIn, SignedInAs};
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        let r = st
            .add_pending_remote(
                "https://clax.dev",
                "clax.dev",
                &PendingSignIn {
                    device_code: clax_core::Secret::new("cxd".into()),
                    user_code: "WXYZ-4821".into(),
                    verification_url: "https://clax.dev/device".into(),
                    expires_at: "2099-01-01T00:00:00.000Z".into(),
                    interval_s: 5,
                },
            )
            .unwrap();
        st.connect_remote(
            &r.id,
            &SignedInAs {
                token: clax_core::Secret::new("cxa".into()),
                person_public_id: "p1".into(),
                person_name: Some("Ada".into()),
                grant_id: "g1".into(),
            },
        )
        .unwrap();
        let p = clax_core::publish::validate(
            serde_json::from_value(json!({"title": "T",
                "files": {"index.html": {"content": "<p>x</p>", "encoding": "utf8"}}}))
            .unwrap(),
        )
        .unwrap();
        let (a, _) = st.create_artifact(&AuditCtx::DAEMON, p, None).unwrap();
        let link = st
            .record_remote_push(
                &a.id,
                &r.id,
                "pub1",
                "https://clax.dev/a/pub1",
                (1, 1),
                None,
            )
            .unwrap();
        let mc = |id: &str, kind: &str, name: &str| MirrorComment {
            remote_comment_id: id.into(),
            author: RemoteAuthor {
                kind: kind.into(),
                public_id: None,
                name: name.into(),
            },
            body: "words".into(),
        };
        let t = st
            .mirror_thread(
                &AuditCtx::DAEMON,
                &link,
                ("clax.dev", "https://clax.dev"),
                1,
                true,
                MirrorThread {
                    remote_thread_id: "rt1".into(),
                    remote_version: 1,
                    anchor: serde_json::from_value(json!({"kind": "artifact"})).unwrap(),
                    first: mc("rc1", "viewer", "Sam"),
                },
            )
            .unwrap();
        let c = st
            .mirror_comment(
                &AuditCtx::DAEMON,
                &t.id,
                &r.id,
                &mc("rc2", "agent", "Fixer"),
            )
            .unwrap()
            .unwrap();
        let mut i = item(Kind::Reply, None);
        i.thread_id = Some(t.id.clone());
        i.comment_id = Some(c.id.clone());
        let mut v = json!({});
        linked_tag(&st, &i, &mut v).unwrap();
        assert_eq!(
            v["linked"],
            json!({"remote": "clax.dev", "unlinked": false, "author": "Fixer (on clax.dev)"})
        );
        // An item on a local thread carries none.
        i.thread_id = Some("t_local".into());
        let mut v = json!({});
        linked_tag(&st, &i, &mut v).unwrap();
        assert!(v.get("linked").is_none());
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
            true,
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
            true,
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
                true,
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
                true,
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

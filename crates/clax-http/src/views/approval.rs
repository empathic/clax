//! Approvals and their requests in the daemon (spec
//! 2026-10-08-multi-agent-multi-person §9): the views people and agents
//! get of them (never the withheld content, which only the approver's own
//! inbox card shows), and the task that turns each committed change to a
//! request into an `approval_request` event on its approver's `inbox`
//! topic and wakes the polls waiting on it.

use crate::state::AppState;
use clax_core::Event;
use clax_core::visibility::{self, Author};
use clax_store::Store;
use clax_store::approvals::{Approval, ApprovalRequest};
use serde_json::{Value, json};

/// `{public_id, name, owner}` of viewer `pid`, or of an author on a public
/// Clax keyed `pid` ([`visibility::remote_author`]): names are not filtered
/// (§21.9). `owner` when `pid` is the owner's, who is named "The owner"
/// until they choose a name, so a person named "Viewer" is not taken for
/// them.
fn person(st: &Store, pid: &str) -> clax_core::Result<Value> {
    if visibility::is_remote_author(pid) {
        // Someone on a public Clax: named as their comments here name them.
        let name = st
            .remote_author_name(pid)?
            .unwrap_or_else(|| clax_core::feedback::display_name(""));
        return Ok(json!({"public_id": pid, "name": name, "owner": false}));
    }
    let owner = st.owner()?.is_some_and(|o| o.public_id == pid);
    let raw = st.viewer_by_public_id(pid)?.and_then(|v| v.display_name);
    let name = clax_core::feedback::person_name(raw.as_deref(), owner);
    Ok(json!({"public_id": pid, "name": name, "owner": owner}))
}

/// The title of live artifact `aid`, or `None` once it is gone.
fn artifact_title(st: &Store, aid: Option<&str>) -> clax_core::Result<Option<String>> {
    let Some(id) = aid.and_then(|a| clax_core::ids::ArtifactId::parse(a).ok()) else {
        return Ok(None);
    };
    Ok(st.get_artifact(&id)?.map(|a| a.title))
}

/// A request's view (§7.5: author, thread, count and state, never the
/// withheld content): its ID, approver, author, artifact and thread IDs,
/// the agent it is about (handle and name), cause, count, status, the
/// scope it was approved at, and its times.
pub fn request_view(st: &Store, r: &ApprovalRequest) -> clax_core::Result<Value> {
    let agent = match &r.session_id {
        Some(sid) => st.get_session(sid)?.map(|s| {
            json!({"handle": s.agent_handle, "name": s.name, "harness": s.harness,
                   "live": s.ended_at.is_none()})
        }),
        None => None,
    };
    Ok(json!({
        "id": r.id,
        "approver": r.approver,
        "author": person(st, &r.author)?,
        "artifact_id": r.artifact_id,
        "thread_id": r.thread_id,
        "agent": agent,
        "cause": r.cause,
        "count": r.count,
        "status": r.status,
        "scope": r.decided_scope,
        "created_at": r.created_at,
        "updated_at": r.updated_at,
        "decided_at": r.decided_at,
    }))
}

/// An approval's view: who may see whose content, at which scope, with
/// its page's title (`artifact_title`) while the page is live.
pub fn approval_view(st: &Store, a: &Approval) -> clax_core::Result<Value> {
    let author = if a.author == clax_core::visibility::UNCONNECTED {
        json!({"public_id": null, "name": "Viewers who are not connected"})
    } else {
        person(st, &a.author)?
    };
    Ok(json!({
        "id": a.id,
        "approver": a.approver,
        "author": author,
        "scope": a.scope,
        "artifact_id": a.artifact_id,
        "artifact_title": artifact_title(st, a.artifact_id.as_deref())?,
        "thread_id": a.thread_id,
        "created_at": a.created_at,
        "created_via": a.created_via,
        "request_id": a.request_id,
    }))
}

/// What an approval card shows its approver (§9.5): the request, the
/// withheld content itself (people are not filtered: the author's
/// comments in the thread, or, outside threads, the artifact's title and
/// description), and, for a page's send, the page's authors and the
/// viewer whose browser ran it ("<publisher>'s page, via <viewer>'s
/// browser"). `None` when the request is gone or is not `approver`'s.
pub fn card(
    st: &Store,
    rid: &str,
    approver: Option<&str>,
    detail: Option<&Value>,
) -> clax_core::Result<Option<Value>> {
    let r = match st.approval_request(rid, approver) {
        Ok(r) => r,
        Err(clax_core::CoreError::NotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut v = request_view(st, &r)?;
    // Exactly what an approval of this request would let through: the
    // comments whose authors include the request's (as the count reads
    // them, an agent's words its person's and a page's its authors'), or,
    // outside threads, what the author holds at the artifact.
    let owner = st.owner_viewer(true)?.public_id;
    let who = if r.author == owner {
        Author::Owner
    } else {
        Author::Person(r.author.clone())
    };
    v["content"] = match &r.thread_id {
        Some(tid) => match st.get_thread(tid)? {
            Some(t) => {
                let comments: Vec<Value> = t
                    .comments
                    .iter()
                    .filter(|c| {
                        visibility::normalise(&visibility::authors_of_comment(c), Some(&owner))
                            .contains(&who)
                    })
                    .map(|c| {
                        json!({"id": c.id, "body": c.body, "created_at": c.created_at,
                               "author_name": c.author_name, "by_agent": c.author_kind == clax_store::threads::AUTHOR_AGENT})
                    })
                    .collect();
                json!({"thread": {"id": t.id, "status": t.status}, "comments": comments})
            }
            None => json!({"thread": null, "comments": []}),
        },
        None => {
            let a = st.get_artifact(&clax_core::ArtifactId::parse(&r.artifact_id)?)?;
            let (files, docs, assets) = match st.authored_outside_threads(&r.artifact_id, &r.author)
            {
                Ok(x) => x,
                Err(clax_core::CoreError::NotFound) => Default::default(),
                Err(e) => return Err(e),
            };
            let assets: Vec<Value> = assets
                .into_iter()
                .map(|(id, content_type, size)| json!({"id": id, "content_type": content_type, "size": size}))
                .collect();
            json!({"title": a.as_ref().map(|a| a.title.clone()),
                   "description": a.and_then(|a| a.description),
                   "files": files, "docs": docs, "assets": assets})
        }
    };
    v["via_page"] = match detail.and_then(|d| d.get("page")) {
        Some(p) => {
            let mut authors = Vec::new();
            for a in p["authors"].as_array().into_iter().flatten() {
                match a.as_str() {
                    Some(clax_core::visibility::OWNER) | None => {
                        authors.push(person(st, &st.owner_viewer(true)?.public_id)?)
                    }
                    Some(pid) => authors.push(person(st, pid)?),
                }
            }
            let viewer = match p["viewer"].as_str() {
                Some(pid) => person(st, pid)?,
                None => json!({"public_id": null, "name": clax_core::feedback::display_name("")}),
            };
            json!({"authors": authors, "viewer": viewer})
        }
        None => Value::Null,
    };
    Ok(Some(v))
}

/// Installs the store's request listener: each committed change to a
/// request wakes the `ask` polls holding it ([`AppState::questions`],
/// keyed by its ID) and publishes its view as `approval_request` to its
/// approver's `inbox` topic. Returns the publishing task's handle.
pub fn listen(s: &AppState) -> clax_rt::JoinHandle<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i64>>();
    let waiters = s.questions.clone();
    let st0 = s.store.clone();
    s.store.set_request_listener(move |rows| {
        // Woken at once, on the writer's thread after the commit, so a
        // poll never waits on the event task.
        if let Ok(rs) = st0.approval_requests_by_rowid(&rows) {
            for r in &rs {
                waiters.wake(&r.id);
            }
        }
        let _ = tx.send(rows);
    });
    let st = s.clone();
    clax_rt::spawn(async move {
        while let Some(rows) = rx.recv().await {
            let out = st
                .store
                .call(move |db| {
                    let rs = db.approval_requests_by_rowid(&rows)?;
                    rs.iter()
                        .map(|r| {
                            let public = r.cause == clax_store::approvals::Cause::Ask;
                            Ok((r.approver.clone(), public, request_view(db, r)?))
                        })
                        .collect::<clax_core::Result<Vec<_>>>()
                })
                .await;
            match out {
                Ok(views) => {
                    for (recipient, public, request) in views {
                        st.events.publish(Event::ApprovalRequest {
                            request,
                            recipient,
                            public,
                        });
                    }
                }
                Err(e) => tracing::warn!(error = %e, "approval request change not announced"),
            }
        }
    })
}

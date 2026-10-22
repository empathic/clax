//! Opening an artifact's site to the world (`artifacts.site_access`), the
//! `site_access` of `PATCH /api/artifacts/<aid>` ([`super::artifacts::patch`]).
//!
//! - `private` closes the site; `world` opens it, so anyone loads it and
//!   uses its runtime as a visitor (`clax_http::site_access`). Either takes
//!   effect on the next request: every site request reads it.
//! - Who may manage the artifact changes it
//!   ([`clax_http::agent::administers`]: the owner, by hand or in their
//!   browser, their agents, and the page's maker); anyone else 403
//!   `admin_only`.
//! - An agent (the owner's with a session, or a person's) opens a site to
//!   the world only with its person's approval, as `publish_public`
//!   publishes ([`super::remotes`]): its first call asks them a question
//!   this daemon writes and answers 202 `{status: "approval_pending",
//!   question_id, inbox_url}`; called again with that `question_id`, it
//!   opens the site once they chose to, from Clax's browser or the
//!   extension, and is 403 `not_approved` when they did not. An approval
//!   is spent once used (409 `approval_used`). An agent without a session
//!   is 400 `no_session`. Closing never asks.

use clax_core::questions::{QOption, Question};
use clax_core::{ArtifactId, CoreError};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::questions::{NewQuestion, Source, Status};
use clax_store::site_sessions::OPEN_TO_WORLD;
use serde_json::{Value, json};

/// The approval question's options.
pub const OPEN: &str = "Open to the world";
pub const KEEP_PRIVATE: &str = "Keep it private";

/// What a `site_access` change came to.
pub enum Change {
    /// It is set.
    Done,
    /// The person has yet to answer question `0`: the 202 body.
    Pending(Value),
}

/// Parses `site_access`: `Some(true)` for `world`, `Some(false)` for
/// `private`; 400 `invalid_argument` for anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<bool, ApiError> {
    match v {
        "world" => Ok(true),
        "private" => Ok(false),
        _ => Err(ApiError::bad_request(
            "invalid_argument",
            format!("site_access is private or world, not '{v}'"),
        )),
    }
}

/// Sets artifact `id`'s site open to the world (`world`) or private, as
/// `caller`, waiting on question `qid` for an agent's opening (see the
/// module docs).
///
/// # Errors
/// As the module docs say, and the store's.
pub async fn change(
    s: &AppState,
    caller: &Caller,
    audit: DeferredAudit,
    id: &ArtifactId,
    world: bool,
    qid: Option<String>,
) -> Result<Change, ApiError> {
    let (c2, i2) = (caller.clone(), id.clone());
    let (manages, already) = s
        .store_call(move |st| {
            st.get_artifact(&i2)?.ok_or(CoreError::NotFound)?;
            Ok((
                clax_http::agent::administers(st, &c2, i2.as_str())?,
                st.site_open_to_world(&i2)?,
            ))
        })
        .await?;
    if !manages {
        return Err(ApiError::forbidden(
            "admin_only",
            "only the owner or the page's maker opens its site to the world or closes it",
        ));
    }
    let agent = match caller {
        Caller::OwnerAgent { session } => Some(session.clone()),
        Caller::PersonAgent { session, .. } => Some(session.clone()),
        _ => None,
    };
    if world
        && !already
        && let Some(session) = agent
    {
        let Some(sid) = session else {
            return Err(ApiError::bad_request(
                "no_session",
                "an agent opens a site to the world with publish_public's site_access, which asks the person first",
            ));
        };
        match qid {
            None => return ask(s, audit, id, &sid).await.map(Change::Pending),
            Some(q) => {
                if let Some(pending) = approved(s, id, &sid, q).await? {
                    return Ok(Change::Pending(pending));
                }
            }
        }
    }
    let id = id.clone();
    s.store_call(move |st| st.set_site_open_to_world(&id, world))
        .await?;
    Ok(Change::Done)
}

fn pending(s: &AppState, qid: &str) -> Value {
    json!({
        "status": "approval_pending",
        "question_id": qid,
        "inbox_url": format!("{}/inbox?q={qid}", s.browser_base),
    })
}

/// Asks session `sid`'s person whether to open `aid`'s site to the world
/// (one question per session and artifact while it waits): the 202 body.
async fn ask(
    s: &AppState,
    audit: DeferredAudit,
    aid: &ArtifactId,
    sid: &str,
) -> Result<Value, ApiError> {
    let (id, sid, st8) = (aid.clone(), sid.to_string(), s.clone());
    let qid = s
        .store_call(move |db| {
            if let Some(q) = db.open_push_approval(&sid, id.as_str(), OPEN_TO_WORLD)? {
                return Ok(q);
            }
            let a = db.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let opt = |label: &str, description: &str| QOption {
                label: label.to_string(),
                description: Some(description.to_string()),
                preview: None,
                recommended: false,
            };
            let questions = vec![Question {
                question: format!(
                    "Open \"{}\" to the world? Anyone with its site's address can then load it and use its data as a visitor, as its rules allow, without signing in.",
                    a.title
                ),
                header: "Open site".to_string(),
                options: vec![
                    opt(OPEN, "Anyone may load its site; close it again at any time"),
                    opt(KEEP_PRIVATE, "Only those it is shared with"),
                ],
                multi_select: false,
                other: false,
            }];
            let ctx = audit.for_session(db, &sid)?;
            let (row, _) = db.create_question(
                &ctx,
                NewQuestion {
                    session_id: sid.clone(),
                    artifact_id: Some(id.as_str().to_string()),
                    source: Source::Ask,
                    tool_use_id: None,
                    questions,
                    released: false,
                    thread_id: None,
                    in_terminal: false,
                },
            )?;
            db.create_push_approval(&row.id, id.as_str(), OPEN_TO_WORLD, &sid, a.current_version)?;
            let v = clax_http::views::question::view(db, &row)?;
            clax_http::views::question::announce(&st8, db, &row, v);
            Ok(row.id)
        })
        .await?;
    Ok(pending(s, &qid))
}

/// Whether question `qid` approves session `sid`'s opening of `aid`:
/// `None` when it does (and is now spent), the 202 body while it waits.
async fn approved(
    s: &AppState,
    aid: &ArtifactId,
    sid: &str,
    qid: String,
) -> Result<Option<Value>, ApiError> {
    use axum::http::StatusCode;
    let (q2, sid2) = (qid.clone(), sid.to_string());
    let (pa, q) = s
        .store_call(move |st| {
            Ok((
                st.push_approval(&q2)?,
                st.question(&q2)?.filter(|q| q.session_id == sid2),
            ))
        })
        .await?;
    let (Some(pa), Some(q)) = (pa.filter(|p| p.remote_id == OPEN_TO_WORLD), q) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_question",
            "no approval of this session's has this question ID: ask without question_id",
        ));
    };
    if pa.artifact_id != aid.as_str() {
        return Err(ApiError::bad_request(
            "invalid_args",
            "this approval is for another artifact",
        ));
    }
    let used = || {
        ApiError::new(
            StatusCode::CONFLICT,
            "approval_used",
            "this approval was used: ask again without question_id",
        )
    };
    if pa.used_at.is_some() {
        return Err(used());
    }
    if q.status == Status::Open {
        return Ok(Some(pending(s, &qid)));
    }
    let q3 = qid.clone();
    if !s.store_call(move |st| st.use_push_approval(&q3)).await? {
        return Err(used());
    }
    let refuse = |why: &str| {
        Err(ApiError::forbidden(
            "not_approved",
            format!("the person did not approve opening this site to the world: {why}"),
        ))
    };
    if q.status != Status::Answered {
        return refuse("they skipped the question");
    }
    // Only the person's own answer counts: Clax's browser or the extension,
    // never the daemon token an agent holds too.
    if !matches!(q.answered_via.as_deref(), Some("shell" | "extension")) {
        return refuse("answer it in Clax in the browser");
    }
    let choice = q
        .answers
        .as_ref()
        .and_then(|a| a.first())
        .and_then(|a| a.selected.first())
        .cloned()
        .unwrap_or_default();
    if choice == OPEN {
        Ok(None)
    } else {
        refuse("they chose to keep it private")
    }
}

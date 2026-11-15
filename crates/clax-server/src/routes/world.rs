//! Opening an artifact's site to the world (`artifacts.site_access`), the
//! `site_access` of `PATCH /api/artifacts/<aid>` ([`super::artifacts::patch`]).
//!
//! - `private` closes the site; `world` opens it, so anyone loads it and
//!   uses its runtime as a visitor (`clax_http::site_access`). Either takes
//!   effect on the next request: every site request reads it. Each change
//!   publishes `Event::SiteAccess`, on which a visitor's room socket closes
//!   (4403 `revoked`) when the site closes, and a holder's is checked again
//!   (`super::room`).
//! - Who may manage the artifact changes it
//!   ([`clax_http::agent::administers`]: the page's maker and their
//!   agents, and on a local daemon the owner, by hand or in their browser,
//!   and their agents); anyone else 403 `admin_only`.
//! - An agent (the owner's with a session, or a person's) opens a site to
//!   the world only with its person's approval, as `publish_public`
//!   publishes ([`super::remotes`]): its first call asks them a question
//!   this daemon writes and answers 202 `{status: "approval_pending",
//!   question_id, inbox_url}`; called again with that `question_id`, it
//!   opens the site once they chose to, from Clax's browser or the
//!   extension, and is 403 `not_approved` when they did not. An approval
//!   is spent once used (409 `approval_used`). An agent without a session
//!   is 400 `no_session`. Closing never asks.
//!
//! Letting the site set its own content policy (`content_policy`,
//! [`super::own_policy`]) waits on the same gate ([`Gate`]).

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

/// What a change of one of a site's switches came to: `site_access`
/// ([`change`]) or `content_policy` ([`super::own_policy::change`]).
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
    if let Some(pending) = GATE.admit(s, caller, audit, id, world, qid).await? {
        return Ok(Change::Pending(pending));
    }
    let i2 = id.clone();
    s.store_call(move |st| st.set_site_open_to_world(&i2, world))
        .await?;
    // Its room sockets of visitors close when it closes.
    s.events.publish(clax_core::Event::SiteAccess {
        artifact_id: id.as_str().to_string(),
        world,
    });
    Ok(Change::Done)
}

/// Opening a site to the world, as [`Gate`] asks it.
pub(crate) static GATE: Gate = Gate {
    kind: OPEN_TO_WORLD,
    widened: clax_store::Store::site_open_to_world,
    to: "world",
    admin_only: "only the page's maker (on a local Clax, the owner too) opens its site to the world or closes it",
    no_session: "an agent opens a site to the world with publish_public's site_access, which asks the person first",
    header: "Open site",
    question: "Open \"{title}\" to the world? Anyone with its site's address can then load it and use its data as a visitor, as its rules allow, without signing in.",
    yes: (OPEN, "Anyone may load its site; close it again at any time"),
    no: (KEEP_PRIVATE, "Only those it is shared with"),
    what: "opening this site to the world",
    declined: "they chose to keep it private",
};

/// A switch that widens what an artifact's site may do or who may open it
/// (its site to the world, [`GATE`]; its own content policy,
/// [`super::own_policy`]), and the one gate both go through: the browser
/// confirmation of the widening ([`Gate::confirm`]), who may turn it
/// ([`clax_http::agent::administers`]), and an agent's turning it only
/// once its person approves ([`Gate::admit`]): the question this daemon
/// asks them, kept as a `push_approvals` row of `remote_id` `kind` and
/// spent once used. Narrowing it asks no one.
pub struct Gate {
    /// The approval's `remote_id` (`clax_store::site_sessions`).
    pub kind: &'static str,
    /// Whether the artifact's switch is widened already.
    pub widened: fn(&clax_store::Store, &ArtifactId) -> clax_core::Result<bool>,
    /// The confirmed decision's `to` ([`clax_http::confirm::Decision::Widen`]).
    pub to: &'static str,
    /// 403 `admin_only`'s message.
    pub admin_only: &'static str,
    /// 400 `no_session`'s message, for an agent without a session.
    pub no_session: &'static str,
    /// The question's header.
    pub header: &'static str,
    /// The question, `{title}` naming the artifact's title.
    pub question: &'static str,
    /// The approving option and its description.
    pub yes: (&'static str, &'static str),
    /// The declining option and its description.
    pub no: (&'static str, &'static str),
    /// What is approved, in `not_approved`'s message.
    pub what: &'static str,
    /// Why, when the person chose [`Gate::no`].
    pub declined: &'static str,
}

impl Gate {
    /// Holds the owner's widening of `id` made with the token alone while
    /// an agent session is live for a click in their browser
    /// ([`clax_http::confirm::require_confirmation`], decision 4), unless
    /// it is widened already.
    ///
    /// # Errors
    /// 202 `confirmation_pending` and the confirmation's refusals, and the
    /// store's.
    pub async fn confirm(
        &'static self,
        s: &AppState,
        caller: &Caller,
        confirming: &clax_http::confirm::Confirming,
        id: &ArtifactId,
    ) -> Result<(), ApiError> {
        let (i2, widened) = (id.clone(), self.widened);
        if s.store_call(move |st| widened(st, &i2)).await? {
            return Ok(());
        }
        let decision = clax_http::confirm::Decision::Widen {
            artifact_id: id.as_str().to_string(),
            to: self.to.into(),
            remote_id: None,
            body: None,
        };
        clax_http::confirm::require_confirmation(s, caller, confirming, decision).await?;
        Ok(())
    }

    /// Whether `caller` may turn the switch of `id` (widening it when
    /// `widen`) now: `None` when it may; the 202 body while an agent's
    /// widening waits on its person's answer to question `qid`, asking them
    /// first when `qid` is `None`. Only who administers the artifact turns
    /// it ([`clax_http::agent::administers`]); an agent widens it only
    /// with its person's approval, and with a session; narrowing, and
    /// widening what is widened already, ask no one.
    ///
    /// # Errors
    /// 404 for an artifact that is gone, 403 `admin_only`, 400
    /// `no_session`, [`Gate::wait`]'s, and the store's.
    pub async fn admit(
        &'static self,
        s: &AppState,
        caller: &Caller,
        audit: DeferredAudit,
        id: &ArtifactId,
        widen: bool,
        qid: Option<String>,
    ) -> Result<Option<Value>, ApiError> {
        let (c2, i2, hosted, widened) =
            (caller.clone(), id.clone(), s.hosted.is_some(), self.widened);
        let (manages, already) = s
            .store_call(move |st| {
                st.get_artifact(&i2)?.ok_or(CoreError::NotFound)?;
                Ok((
                    clax_http::agent::administers(st, &c2, i2.as_str(), hosted)?,
                    widened(st, &i2)?,
                ))
            })
            .await?;
        if !manages {
            return Err(ApiError::forbidden("admin_only", self.admin_only));
        }
        let agent = match caller {
            Caller::OwnerAgent { session } => Some(session.clone()),
            Caller::PersonAgent { session, .. } => Some(session.clone()),
            _ => None,
        };
        if widen
            && !already
            && let Some(session) = agent
        {
            let Some(sid) = session else {
                return Err(ApiError::bad_request("no_session", self.no_session));
            };
            return self.wait(s, audit, id, &sid, qid).await;
        }
        Ok(None)
    }

    /// Whether session `sid` may go ahead on `aid` now: `None` once its
    /// person approved question `qid` (now spent); else the 202 body,
    /// asking them first when `qid` is `None`.
    ///
    /// # Errors
    /// 404 `unknown_question`, 400 `invalid_args` (another artifact's),
    /// 409 `approval_used`, 403 `not_approved`, and the store's.
    pub async fn wait(
        &'static self,
        s: &AppState,
        audit: DeferredAudit,
        aid: &ArtifactId,
        sid: &str,
        qid: Option<String>,
    ) -> Result<Option<Value>, ApiError> {
        match qid {
            None => self.ask(s, audit, aid, sid).await.map(Some),
            Some(q) => self.approved(s, aid, sid, q).await,
        }
    }

    /// Asks session `sid`'s person whether to go ahead on `aid` (one
    /// question per session and artifact while it waits): the 202 body.
    async fn ask(
        &'static self,
        s: &AppState,
        audit: DeferredAudit,
        aid: &ArtifactId,
        sid: &str,
    ) -> Result<Value, ApiError> {
        let (id, sid, st8) = (aid.clone(), sid.to_string(), s.clone());
        let qid = s
            .store_call(move |db| {
                if let Some(q) = db.open_push_approval(&sid, id.as_str(), self.kind)? {
                    return Ok(q);
                }
                let a = db.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
                let opt = |(label, description): (&str, &str)| QOption {
                    label: label.to_string(),
                    description: Some(description.to_string()),
                    preview: None,
                    recommended: false,
                };
                let questions = vec![Question {
                    question: self.question.replace("{title}", &a.title),
                    header: self.header.to_string(),
                    options: vec![opt(self.yes), opt(self.no)],
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
                db.create_push_approval(&row.id, id.as_str(), self.kind, &sid, a.current_version)?;
                let v = clax_http::views::question::view(db, &row)?;
                clax_http::views::question::announce(&st8, db, &row, v);
                Ok(row.id)
            })
            .await?;
        Ok(pending(s, &qid))
    }

    /// Whether question `qid` approves session `sid`'s change of `aid`:
    /// `None` when it does (and is now spent), the 202 body while it waits.
    async fn approved(
        &self,
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
        let (Some(pa), Some(q)) = (pa.filter(|p| p.remote_id == self.kind), q) else {
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
                format!("the person did not approve {}: {why}", self.what),
            ))
        };
        if q.status != Status::Answered {
            return refuse("they skipped the question");
        }
        // Only the person's own answer counts: Clax's browser or the
        // extension, never the daemon token an agent holds too.
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
        if choice == self.yes.0 {
            Ok(None)
        } else {
            refuse(self.declined)
        }
    }
}

fn pending(s: &AppState, qid: &str) -> Value {
    json!({
        "status": "approval_pending",
        "question_id": qid,
        "inbox_url": format!("{}/inbox?q={qid}", s.browser_base),
    })
}

//! A change to one of an artifact's settings that some changes need
//! approval to make, and the one way it is made: who may load its site
//! (`site_access`, [`super::world`]) and the `Content-Security-Policy` Clax
//! sends on its own content (`content_security_policy`,
//! [`super::content_security_policy`]), and whether a hosted Clax counts
//! visits to its site (`analytics`, `on` or `off`, which never needs
//! approval), each a field of `PATCH /api/artifacts/<aid>`
//! ([`super::artifacts::patch`]).
//!
//! A change is admitted first ([`admit`]) and applied only with what
//! admitting it returned ([`apply`] takes an [`Admitted`], which nothing
//! else makes), so the change and the artifact that were admitted are the
//! ones written.
//!
//! - Who may manage the artifact makes a change
//!   ([`clax_http::agent::administers`]); anyone else 403 `admin_only`.
//! - An agent (the owner's with a session, or a person's) makes a change
//!   that needs approval ([`SettingChange::approval`]: `site_access:
//!   world`, `content_security_policy: unrestricted`) only with its
//!   person's approval: its first call asks them a question this daemon
//!   writes and answers 202 `{status: "approval_pending", question_id,
//!   inbox_url}`; called again with that `question_id`, the change is
//!   admitted once they chose to, from Clax's browser or the extension,
//!   and is 403 `not_approved` when they did not. An approval is a
//!   `push_approvals` row of `remote_id` [`Approval::kind`],
//!   spent once used (409 `approval_used`). An agent without a session is
//!   400 `no_session`.
//! - Any other change, and one to the value the setting already has, asks
//!   no one.
//! - The owner's change that needs approval made with the token alone
//!   while an agent session is live waits for a click in their browser
//!   ([`confirm`]).

use clax_core::audit::AuditCtx;
use clax_core::questions::{QOption, Question};
use clax_core::{ArtifactId, CoreError};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::questions::{NewQuestion, Source, Status};
use clax_store::site_sessions::{ContentSecurityPolicy, OPEN_TO_WORLD, UNRESTRICT_CSP};
use serde_json::{Value, json};

use super::world::SiteAccess;

/// One change to one of an artifact's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChange {
    /// Who may load its site becomes this (`site_access`).
    SiteAccess(SiteAccess),
    /// The `Content-Security-Policy` Clax sends on its own content becomes
    /// this (`content_security_policy`).
    ContentSecurityPolicy(ContentSecurityPolicy),
    /// Whether a hosted Clax counts visits to its site (`analytics`: `on`
    /// when true). Never needs approval; what was counted is kept either
    /// way.
    Analytics(bool),
}

/// The approval a change needs: its approvals' `remote_id`
/// (`clax_store::site_sessions`) and what its setting says to the people
/// it asks and refuses.
#[derive(Clone, Copy)]
struct Approval {
    kind: &'static str,
    wording: &'static Wording,
}

/// What the settings whose changes may need approval say to the people
/// they refuse and ask.
struct Wording {
    /// 400 `no_session`'s message, for an agent without a session.
    no_session: &'static str,
    /// The question's header.
    header: &'static str,
    /// The approving option and its description.
    approve: (&'static str, &'static str),
    /// The declining option and its description.
    decline: (&'static str, &'static str),
    /// What is approved, in `not_approved`'s message.
    what: &'static str,
    /// Why, when the person chose [`Wording::decline`].
    declined: &'static str,
}

const SITE_ACCESS: Wording = Wording {
    no_session: "an agent opens a site to the world with publish_public's site_access, which asks the person first",
    header: "Open site",
    approve: (
        super::world::OPEN,
        "Anyone may load its site; close it again at any time",
    ),
    decline: (super::world::KEEP_PRIVATE, "Only those it is shared with"),
    what: "opening this site to the world",
    declined: "they chose to keep it private",
};

const CONTENT_SECURITY_POLICY: Wording = Wording {
    no_session: "an agent serves a site without Clax's Content Security Policy with publish_public's content_security_policy, which asks the person first",
    header: "Content Security Policy",
    approve: (
        super::content_security_policy::SERVE_UNRESTRICTED,
        "Its pages, and every later version's, load and connect where they choose; restore the policy at any time",
    ),
    decline: (
        super::content_security_policy::KEEP_TO_ITS_SITE,
        "Its pages load and connect to its own site alone",
    ),
    what: "serving this site without Clax's Content Security Policy",
    declined: "they chose to keep it to its own site",
};

impl SettingChange {
    /// The approval it needs, if it is one an agent needs its person's
    /// approval to make, and the owner's token alone a browser
    /// confirmation: opening a site to the world, and serving it
    /// unrestricted.
    const fn approval(self) -> Option<Approval> {
        match self {
            SettingChange::SiteAccess(SiteAccess::World) => Some(Approval {
                kind: OPEN_TO_WORLD,
                wording: &SITE_ACCESS,
            }),
            SettingChange::ContentSecurityPolicy(ContentSecurityPolicy::Unrestricted) => {
                Some(Approval {
                    kind: UNRESTRICT_CSP,
                    wording: &CONTENT_SECURITY_POLICY,
                })
            }
            _ => None,
        }
    }

    /// 403 `admin_only`'s message.
    const fn admin_only(self) -> &'static str {
        match self {
            SettingChange::SiteAccess(_) => {
                "only the page's maker (on a local Clax, the owner too) opens its site to the world or closes it"
            }
            SettingChange::ContentSecurityPolicy(_) => {
                "only the page's maker (on a local Clax, the owner too) sets its site's Content Security Policy"
            }
            SettingChange::Analytics(_) => {
                "only those who administer the page turn the counting of its visits on or off"
            }
        }
    }

    /// The value it sets, the confirmed decision's `to`
    /// ([`clax_http::confirm::Decision::Widen`]).
    const fn value(self) -> &'static str {
        match self {
            SettingChange::SiteAccess(SiteAccess::World) => "world",
            SettingChange::SiteAccess(SiteAccess::Private) => "private",
            SettingChange::ContentSecurityPolicy(p) => p.as_str(),
            SettingChange::Analytics(true) => "on",
            SettingChange::Analytics(false) => "off",
        }
    }

    /// Whether artifact `id`'s setting already has the value it sets.
    fn already(self, st: &clax_store::Store, id: &ArtifactId) -> clax_core::Result<bool> {
        Ok(match self {
            SettingChange::SiteAccess(a) => st.site_open_to_world(id)? == (a == SiteAccess::World),
            SettingChange::ContentSecurityPolicy(p) => st.content_security_policy(id)? == p,
            SettingChange::Analytics(on) => st.analytics_on(id)? == on,
        })
    }

    /// The question an agent's person is asked about the artifact titled
    /// `title`, for a change that needs approval.
    fn question(self, title: &str) -> String {
        match self {
            SettingChange::SiteAccess(_) => format!(
                "Open \"{title}\" to the world? Anyone with its site's address can then load it and use its data as a visitor, as its rules allow, without signing in."
            ),
            SettingChange::ContentSecurityPolicy(_) => format!(
                "Serve \"{title}\" without Clax's Content Security Policy? Its pages may then load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website."
            ),
            // Never asked: it needs no approval.
            SettingChange::Analytics(_) => format!("Change how \"{title}\" counts visits?"),
        }
    }
}

/// A change its caller may make now. Only [`admit`] makes one, and
/// [`apply`] writes it.
pub struct Admitted {
    change: SettingChange,
    artifact: ArtifactId,
}

/// What asking to make a change came to.
pub enum Admission {
    /// The caller may make it.
    Admitted(Admitted),
    /// The agent's person has yet to answer: the 202 body.
    Pending(Value),
}

/// Holds the owner's `change` of `id` that needs approval
/// ([`SettingChange::approval`]), made with the token alone while an
/// agent session is live, for a click in their browser
/// ([`clax_http::confirm::require_confirmation`], decision 4), unless the
/// setting already has that value. Any other change passes.
///
/// # Errors
/// 202 `confirmation_pending` and the confirmation's refusals, and the
/// store's.
pub async fn confirm(
    s: &AppState,
    caller: &Caller,
    confirming: &clax_http::confirm::Confirming,
    id: &ArtifactId,
    change: SettingChange,
) -> Result<(), ApiError> {
    if change.approval().is_none() {
        return Ok(());
    }
    let i2 = id.clone();
    if s.store_call(move |st| change.already(st, &i2)).await? {
        return Ok(());
    }
    let decision = clax_http::confirm::Decision::Widen {
        artifact_id: id.as_str().to_string(),
        to: change.value().into(),
        remote_id: None,
        body: None,
    };
    clax_http::confirm::require_confirmation(s, caller, confirming, decision).await?;
    Ok(())
}

/// Whether `caller` may make `change` of `id` now: the change admitted,
/// or the 202 body while an agent's change that needs approval waits on
/// its person's answer to question `qid`, asking them first when `qid` is
/// `None` (the module docs).
///
/// # Errors
/// 404 for an artifact that is gone, 403 `admin_only`, 400 `no_session`,
/// 404 `unknown_question`, 400 `invalid_args` (another artifact's
/// question), 409 `approval_used`, 403 `not_approved`, and the store's.
pub async fn admit(
    s: &AppState,
    caller: &Caller,
    audit: DeferredAudit,
    id: &ArtifactId,
    change: SettingChange,
    qid: Option<String>,
) -> Result<Admission, ApiError> {
    let (c2, i2, hosted) = (caller.clone(), id.clone(), s.hosted.is_some());
    let (manages, already) = s
        .store_call(move |st| {
            st.get_artifact(&i2)?.ok_or(CoreError::NotFound)?;
            Ok((
                clax_http::agent::administers(st, &c2, i2.as_str(), hosted)?,
                change.already(st, &i2)?,
            ))
        })
        .await?;
    if !manages {
        return Err(ApiError::forbidden("admin_only", change.admin_only()));
    }
    let agent = match caller {
        Caller::OwnerAgent { session } => Some(session.clone()),
        Caller::PersonAgent { session, .. } => Some(session.clone()),
        _ => None,
    };
    if let Some(approval) = change.approval()
        && !already
        && let Some(session) = agent
    {
        let Some(sid) = session else {
            return Err(ApiError::bad_request(
                "no_session",
                approval.wording.no_session,
            ));
        };
        let waiting = match qid {
            None => Some(ask(s, audit, id, &sid, change, approval).await?),
            Some(q) => approved(s, id, &sid, q, approval).await?,
        };
        if let Some(body) = waiting {
            return Ok(Admission::Pending(body));
        }
    }
    Ok(Admission::Admitted(Admitted {
        change,
        artifact: id.clone(),
    }))
}

/// Makes the change `admitted` names. A Content Security Policy or
/// analytics change is recorded as `ctx`'s; a site's opening or closing publishes
/// `Event::SiteAccess`, on which its room sockets of visitors close when it
/// closes.
///
/// # Errors
/// The store's.
pub async fn apply(s: &AppState, ctx: AuditCtx, admitted: Admitted) -> Result<(), ApiError> {
    let Admitted { change, artifact } = admitted;
    match change {
        SettingChange::SiteAccess(access) => {
            let world = access == SiteAccess::World;
            let id = artifact.clone();
            s.store_call(move |st| st.set_site_open_to_world(&id, world))
                .await?;
            s.events.publish(clax_core::Event::SiteAccess {
                artifact_id: artifact.as_str().to_string(),
                world,
            });
        }
        SettingChange::ContentSecurityPolicy(policy) => {
            s.store_call(move |st| st.set_content_security_policy(&ctx, &artifact, policy))
                .await?;
        }
        SettingChange::Analytics(on) => {
            s.store_call(move |st| st.set_analytics(&ctx, &artifact, on))
                .await?;
        }
    }
    Ok(())
}

/// Asks session `sid`'s person whether to make `change` of `aid` (one
/// question per session, artifact and setting while it waits): the 202
/// body.
async fn ask(
    s: &AppState,
    audit: DeferredAudit,
    aid: &ArtifactId,
    sid: &str,
    change: SettingChange,
    approval: Approval,
) -> Result<Value, ApiError> {
    let (id, sid, st8) = (aid.clone(), sid.to_string(), s.clone());
    let qid = s
        .store_call(move |db| {
            if let Some(q) = db.open_push_approval(&sid, id.as_str(), approval.kind)? {
                return Ok(q);
            }
            let a = db.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let w = approval.wording;
            let opt = |(label, description): (&str, &str)| QOption {
                label: label.to_string(),
                description: Some(description.to_string()),
                preview: None,
                recommended: false,
            };
            let questions = vec![Question {
                question: change.question(&a.title),
                header: w.header.to_string(),
                options: vec![opt(w.approve), opt(w.decline)],
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
            db.create_push_approval(&row.id, id.as_str(), approval.kind, &sid, a.current_version)?;
            let v = clax_http::views::question::view(db, &row)?;
            clax_http::views::question::announce(&st8, db, &row, v);
            Ok(row.id)
        })
        .await?;
    Ok(pending(s, &qid))
}

/// Whether question `qid` approves session `sid`'s `change` of `aid`:
/// `None` when it does (and is now spent), the 202 body while it waits.
async fn approved(
    s: &AppState,
    aid: &ArtifactId,
    sid: &str,
    qid: String,
    approval: Approval,
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
    let (Some(pa), Some(q)) = (pa.filter(|p| p.remote_id == approval.kind), q) else {
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
    let w = approval.wording;
    let refuse = |why: &str| {
        Err(ApiError::forbidden(
            "not_approved",
            format!("the person did not approve {}: {why}", w.what),
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
    if choice == w.approve.0 {
        Ok(None)
    } else {
        refuse(w.declined)
    }
}

fn pending(s: &AppState, qid: &str) -> Value {
    json!({
        "status": "approval_pending",
        "question_id": qid,
        "inbox_url": format!("{}/inbox?q={qid}", s.browser_base),
    })
}

//! A local daemon's remotes (public Clax instances it signed in to) and
//! pushes of its artifacts to them ([`clax_http::public`]).
//!
//! - `GET /api/remotes` lists them; `POST /api/remotes {url, name?,
//!   machine?}` starts a device sign-in there; `POST
//!   /api/remotes/<id>/poll` polls it once; `DELETE /api/remotes/<id>`
//!   forgets one. Adding, polling and removing are the owner's alone (the
//!   CLI by hand or the owner's browser): 403 `not_for_agents` for an
//!   agent.
//! - `GET /api/artifacts/<aid>/remotes` → the artifact's public twins and
//!   the remotes it could go to.
//! - `POST /api/artifacts/<aid>/push {remote?, mode?, auto?,
//!   question_id?}` pushes it. The owner pushes at once. An agent of the
//!   owner's pushes only with the owner's approval: its first call asks
//!   the owner a question this daemon writes (answered in Clax's browser or
//!   the extension) and answers 202 `{status: "approval_pending",
//!   question_id}`; the call again with that `question_id` pushes when the
//!   owner chose to publish, and 403 `not_approved` when they did not. A
//!   link the owner let follow new versions ("Publish and push new
//!   versions") takes an agent's push of a new version without asking.
//!
//! A hosted daemon has no remotes: every route answers 404 there. Every
//! route is refused to another machine's agents (`DeniedToGrants`).

use super::artifacts::{body, path};
use crate::public::{self, Mode, SignIn};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::questions::{QOption, Question};
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::DeniedToGrants;
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::questions::{NewQuestion, Source, Status};
use clax_store::remotes::Remote;
use serde::Deserialize;
use serde_json::{Value, json};

/// The approval question's options.
pub const PUBLISH_ONCE: &str = "Publish";
pub const PUBLISH_FOLLOW: &str = "Publish and push new versions";
pub const DONT_PUBLISH: &str = "Don't publish";

/// 404 on a hosted daemon, which has no remotes.
fn local_only(s: &AppState) -> Result<(), ApiError> {
    if s.hosted.is_some() {
        Err(ApiError::not_found())
    } else {
        Ok(())
    }
}

/// Who asks: the owner by hand, or an agent of the owner's (with its
/// session, when it names one). Anyone else is refused.
enum Asker {
    Owner,
    Agent(Option<String>),
}

async fn asker(s: &AppState, d: &DeniedToGrants) -> Result<Asker, ApiError> {
    match d.caller(s).await? {
        Caller::Owner { .. } => Ok(Asker::Owner),
        Caller::OwnerAgent { session } => Ok(Asker::Agent(session)),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        _ => Err(ApiError::forbidden(
            "forbidden",
            "publishing to a public Clax is the owner's",
        )),
    }
}

/// A remote as the routes show it: never its token or device code.
fn remote_view(r: &Remote) -> Value {
    json!(r)
}

/// `GET /api/remotes` → `{remotes}`, oldest first.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    who.require_owner("publishes to a public Clax")?;
    asker(&s, &d).await?;
    let rs = s.store_call(|st| st.remotes()).await?;
    Ok(Json(
        json!({"remotes": rs.iter().map(remote_view).collect::<Vec<_>>()}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddBody {
    /// The public Clax's origin (`https://clax.example`; the scheme may be
    /// left out).
    url: String,
    /// Its short name here; its host when absent.
    #[serde(default)]
    name: Option<String>,
    /// What its approval page calls this machine; `Clax on <host name>`
    /// when absent.
    #[serde(default)]
    machine: Option<String>,
}

/// `POST /api/remotes {url, name?, machine?}` (the owner) → 201 `{remote,
/// user_code, verification_url, expires_at, interval}`: a device sign-in
/// started at `url`. Open `verification_url` (signed in there), check the
/// user code and approve; then poll. 400 `invalid_args` for a URL that is
/// not a public origin; 400 `remote_exists` for one signed in to already;
/// 502 for a remote that refused or could not be reached.
pub async fn add(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    req: Result<Json<AddBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("publishes to a public Clax")?;
    asker(&s, &d).await?;
    let b = body(req)?;
    let origin = public::origin_of(&b.url)?;
    let name = b
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| public::default_name(&origin));
    let machine = b
        .machine
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(public::machine_name);
    let o2 = origin.clone();
    if let Some(r) = s.store_call(move |st| st.find_remote(&o2)).await?
        && r.status == "connected"
    {
        return Err(ApiError::bad_request(
            "remote_exists",
            format!(
                "this Clax is signed in to {origin} already as {}: remove it first to sign in again",
                r.person_name.as_deref().unwrap_or("someone")
            ),
        ));
    }
    let sign_in = public::sendable(public::start_sign_in(&origin, &machine)).await?;
    let r = s
        .store_call(move |st| st.add_pending_remote(&origin, &name, &sign_in))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "remote": remote_view(&r),
            "user_code": r.user_code,
            "verification_url": r.verification_url,
            "expires_at": r.sign_in_expires_at,
            "interval": r.poll_interval_s,
        })),
    )
        .into_response())
}

async fn remote_named(s: &AppState, key: String) -> Result<Remote, ApiError> {
    s.store_call(move |st| st.find_remote(&key))
        .await?
        .ok_or_else(ApiError::not_found)
}

/// `POST /api/remotes/<id or name>/poll` (the owner) → `{remote, status}`,
/// `status` `pending` (poll again after the remote's interval),
/// `connected`, or `denied` / `expired` (the remote is forgotten: add it
/// again). 400 `not_pending` for a remote connected already.
pub async fn poll(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("publishes to a public Clax")?;
    asker(&s, &d).await?;
    let r = remote_named(&s, path(id)?).await?;
    let (status, shown) = match public::sendable(public::poll_sign_in(&s, &r)).await? {
        SignIn::Waiting => ("pending", r),
        SignIn::Refused("access_denied") => ("denied", r),
        SignIn::Refused(_) => ("expired", r),
        SignIn::Connected(c) => {
            owner_only(&s.home);
            ("connected", *c)
        }
    };
    Ok(Json(
        json!({"remote": remote_view(&shown), "status": status}),
    ))
}

/// Keeps the home that now holds an outbound agent token its user's alone:
/// the root 0700 and the database's files 0600 (a home made by an older
/// Clax, or copied in, may be readable by others). Native unix only.
fn owner_only(home: &clax_core::Home) {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        use std::os::unix::fs::PermissionsExt;
        let set = |p: &std::path::Path, mode: u32| {
            if let Ok(m) = std::fs::metadata(p)
                && m.permissions().mode() & 0o777 != mode
                && let Err(e) = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
            {
                tracing::warn!(path = %p.display(), "cannot make it the owner's alone: {e}");
            }
        };
        set(home.root(), 0o700);
        let db = home.db_path();
        for suffix in ["", "-wal", "-shm"] {
            let mut p = db.clone().into_os_string();
            p.push(suffix);
            set(std::path::Path::new(&p), 0o600);
        }
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    let _ = home;
}

/// `DELETE /api/remotes/<id or name>` (the owner) → `{remote, revoked}`:
/// revokes its agent token there (`revoked`: whether the remote did) and
/// forgets it, with its artifacts' links. A token the remote could not
/// revoke (unreachable, or revoked there already) stays until revoked on
/// its People page.
pub async fn remove(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("publishes to a public Clax")?;
    asker(&s, &d).await?;
    let r = remote_named(&s, path(id)?).await?;
    let id = r.id.clone();
    let token = s.store_call(move |st| st.remote_token(&id)).await?;
    let revoked = match token {
        Some(t) => match public::sendable(public::revoke(&r, t)).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(remote = r.name.as_str(), code = e.code, "token not revoked");
                false
            }
        },
        None => false,
    };
    let id = r.id.clone();
    let r = s.store_call(move |st| st.remove_remote(&id)).await?;
    s.linked.links_changed();
    Ok(Json(json!({"remote": remote_view(&r), "revoked": revoked})))
}

/// `GET /api/artifacts/<aid>/remotes` → `{links: [{remote_id, remote_name,
/// origin, url, pushed_n, remote_n, auto, behind, download_only, sync,
/// pending, failed, failed_questions}], remotes, unsent}`: the artifact's
/// public twins (`behind`: local versions not pushed yet; `download_only`:
/// local threads stay local; `sync`: how its linked threads' sync stands,
/// with `unreachable_since`; `pending` and `failed`: its writes waiting to
/// go up, and those not sent; `failed_questions`: its open questions whose
/// copies were not sent, which Retry sends too), every connected remote,
/// and `unsent`: the
/// writes not sent to a remote the artifact is no longer linked to, which
/// the owner may drop (`POST /api/artifacts/<aid>/unsent:drop`).
pub async fn for_artifact(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    who.require_owner("publishes to a public Clax")?;
    asker(&s, &d).await?;
    let id = parse_id(&path(aid)?)?;
    let (links, remotes, current, outbox, unsent, questions) = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let links = st.remote_links(id.as_str())?;
            let mut outbox = Vec::new();
            let mut questions = Vec::new();
            for l in &links {
                outbox.extend(
                    st.outbox_of(&l.remote_id)?
                        .into_iter()
                        .filter(|o| o.artifact_id == id.as_str()),
                );
                questions.extend(
                    st.live_linked_questions(&l.remote_id)?
                        .into_iter()
                        .filter(|q| {
                            q.artifact_id == id.as_str()
                                && q.status == "open"
                                && q.failed_at.is_some()
                        }),
                );
            }
            Ok((
                links,
                st.remotes()?,
                a.current_version,
                outbox,
                st.unsent_of(id.as_str())?,
                questions,
            ))
        })
        .await?;
    let links: Vec<Value> = links
        .iter()
        .map(|l| {
            let r = remotes.iter().find(|r| r.id == l.remote_id);
            let mine = outbox.iter().filter(|o| o.remote_id == l.remote_id);
            let failed = mine.clone().filter(|o| o.failed_at.is_some()).count();
            let pending = mine.count() - failed;
            let failed_questions = questions
                .iter()
                .filter(|q| q.remote_id == l.remote_id)
                .count();
            json!({
                "remote_id": l.remote_id,
                "remote_name": r.map(|r| r.name.clone()),
                "origin": r.map(|r| r.origin.clone()),
                "remote_artifact_id": l.remote_artifact_id,
                "url": l.url,
                "pushed_n": l.pushed_n,
                "remote_n": l.remote_n,
                "auto": l.auto,
                "behind": current.saturating_sub(l.pushed_n),
                "pushed_at": l.pushed_at,
                "download_only": l.download_only,
                "sync": s.linked.status(&l.remote_id),
                "pending": pending,
                "failed": failed,
                "failed_questions": failed_questions,
            })
        })
        .collect();
    let connected: Vec<Value> = remotes
        .iter()
        .filter(|r| r.status == "connected")
        .map(remote_view)
        .collect();
    Ok(Json(
        json!({"links": links, "remotes": connected, "unsent": unsent}),
    ))
}

/// The link of `aid` on the remote `rid` names (its ID, name or origin),
/// for the owner.
async fn owned_link(
    s: &AppState,
    d: &DeniedToGrants,
    who: &Identity,
    aid: Result<Path<(String, String)>, PathRejection>,
) -> Result<(ArtifactId, Remote), ApiError> {
    local_only(s)?;
    d.refuse_agents()?;
    who.require_owner("links to a public Clax")?;
    if !matches!(asker(s, d).await?, Asker::Owner) {
        return Err(ApiError::forbidden(
            "forbidden",
            "a link's settings are the owner's",
        ));
    }
    let (aid, rid) = path(aid)?;
    let id = parse_id(&aid)?;
    let r = remote_named(s, rid).await?;
    let (a2, r2) = (id.as_str().to_string(), r.id.clone());
    s.store_call(move |st| st.remote_link(&a2, &r2))
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((id, r))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkPatch {
    /// Local threads stay local; replies and resolves on linked threads
    /// still go up.
    download_only: bool,
}

/// `PATCH /api/artifacts/<aid>/remotes/<remote> {download_only}` (the
/// owner) → `{download_only}`: makes the link download only, or two-way
/// again. 404 for no such link.
pub async fn patch_link(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<LinkPatch>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (id, r) = owned_link(&s, &d, &who, p).await?;
    let b = body(req)?;
    let on = b.download_only;
    let rid = r.id.clone();
    s.store_call(move |st| st.set_link_download_only(id.as_str(), &rid, on))
        .await?;
    s.linked.kick(Some(&r.id));
    Ok(Json(json!({"download_only": on})))
}

/// `DELETE /api/artifacts/<aid>/remotes/<remote>` (the owner) → `{unlinked:
/// true, unsent}`: ends the link. Its public copy stays there; its mirrored
/// threads stay here as read-only history, and its writes not sent yet
/// (`unsent`) wait for the owner to drop them.
pub async fn unlink(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (id, r) = owned_link(&s, &d, &who, p).await?;
    let rid = r.id.clone();
    let unsent = s
        .store_call(move |st| {
            st.unlink_remote(id.as_str(), &rid)?;
            st.unsent_of(id.as_str())
        })
        .await?;
    s.linked.links_changed();
    Ok(Json(json!({"unlinked": true, "unsent": unsent})))
}

/// `POST /api/artifacts/<aid>/remotes/<remote>/retry` (the owner) →
/// `{retried}`: tries the link's writes and question copies not sent
/// again now.
pub async fn retry(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (id, r) = owned_link(&s, &d, &who, p).await?;
    let rid = r.id.clone();
    let n = s
        .store_call(move |st| {
            Ok(st.outbox_retry(&rid, Some(id.as_str()))?
                + st.linked_questions_retry(&rid, Some(id.as_str()))?)
        })
        .await?;
    // Their threads read as sending again, in every open tab, before the
    // sync tries them.
    crate::linked::publish_outbox(&s, &r.id).await;
    s.linked.retry(&r.id);
    Ok(Json(json!({"retried": n})))
}

/// `POST /api/artifacts/<aid>/unsent:drop` (the owner) → `{dropped}`: drops
/// the writes not sent to a remote the artifact is no longer linked to,
/// once the owner confirms.
pub async fn drop_unsent(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("links to a public Clax")?;
    if !matches!(asker(&s, &d).await?, Asker::Owner) {
        return Err(ApiError::forbidden(
            "forbidden",
            "dropping unsent comments is the owner's",
        ));
    }
    let id = parse_id(&path(aid)?)?;
    let n = s.store_call(move |st| st.drop_unsent(id.as_str())).await?;
    Ok(Json(json!({"dropped": n})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PushBody {
    /// The remote's ID, name or origin; the only connected one when absent.
    #[serde(default)]
    remote: Option<String>,
    /// `update` (the default), `new` or `fork`.
    #[serde(default)]
    mode: Option<String>,
    /// The owner's: let agents push new versions of it without asking.
    #[serde(default)]
    auto: Option<bool>,
    /// An agent's: the approval question its first call was answered.
    #[serde(default)]
    question_id: Option<String>,
}

/// The remote a push names, or the only connected one.
async fn push_target(s: &AppState, key: Option<String>) -> Result<Remote, ApiError> {
    let remotes = s.store_call(|st| st.remotes()).await?;
    let connected: Vec<&Remote> = remotes.iter().filter(|r| r.status == "connected").collect();
    let r = match key {
        Some(k) => {
            let k = k.trim().trim_end_matches('/').to_string();
            remotes
                .iter()
                .find(|r| r.id == k || r.name == k || r.origin == k)
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::NOT_FOUND,
                        "no_remote",
                        format!("no remote is named '{k}': `clax remote list` names them"),
                    )
                })?
        }
        None => match connected.as_slice() {
            [one] => *one,
            [] => {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "no_remote",
                    "this Clax is signed in to no public Clax: add one with `clax remote add <url>`",
                ));
            }
            _ => {
                return Err(ApiError::bad_request(
                    "remote_required",
                    "this Clax is signed in to several public Clax instances: name one",
                ));
            }
        },
    };
    if r.status != "connected" {
        return Err(ApiError::bad_request(
            "not_connected",
            format!(
                "the sign-in to {} is not finished: approve it there, then poll",
                r.origin
            ),
        ));
    }
    Ok(r.clone())
}

/// `POST /api/artifacts/<aid>/push {remote?, mode?, auto?, question_id?}`
/// → `{status: "pushed", url, remote, link, pushed: [{n, remote_n}],
/// assets_not_pushed}`, or for an agent waiting on the owner 202
/// `{status: "approval_pending", question_id, inbox_url}` (see the module
/// docs). Refusals: 404 `no_remote`; 400 `remote_required`,
/// `not_connected`, `live_page`, `no_link`; 403 `not_approved` (the owner
/// declined, or answered outside Clax's browser and extension); 409
/// `approval_used` for an approval spent already; 409 `remote_moved` when
/// the public copy has versions this Clax did not push (push with `mode`
/// `new` or `fork`); 502 for the remote's own refusals.
pub async fn push(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    audit: DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<PushBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    local_only(&s)?;
    who.require_owner("publishes to a public Clax")?;
    let by = asker(&s, &d).await?;
    // An agent's CLI (an agent channel with no session) asks no one: it is
    // refused before anything else, and pointed at the tool that asks.
    if let Asker::Agent(None) = by {
        return Err(ApiError::bad_request(
            "no_session",
            "an agent publishes with the publish_public tool, which asks the owner first",
        ));
    }
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let mode = Mode::parse(b.mode.as_deref())?;
    let remote = push_target(&s, b.remote.clone()).await?;
    let (auto, upto) = match by {
        Asker::Owner => (b.auto, None),
        Asker::Agent(session) => {
            if b.auto.is_some() {
                return Err(ApiError::bad_request(
                    "invalid_args",
                    "auto is the owner's to choose, when they approve",
                ));
            }
            let Some(sid) = session else {
                unreachable!("refused above");
            };
            match approval(&s, audit, &id, &remote, &sid, mode, b.question_id).await? {
                Approval::Pending(qid) => {
                    return Ok((
                        StatusCode::ACCEPTED,
                        Json(json!({
                            "status": "approval_pending",
                            "question_id": qid,
                            "inbox_url": format!("{}/inbox?q={qid}", s.browser_base),
                            "remote": remote_view(&remote),
                        })),
                    )
                        .into_response());
                }
                Approval::Granted(auto, upto) => (auto, upto),
            }
        }
    };
    let pushed = public::sendable(public::push(&s, &id, &remote, mode, auto, upto)).await?;
    Ok(Json(json!({
        "status": "pushed",
        "url": pushed.link.url,
        "remote": remote_view(&remote),
        "link": pushed.link,
        "pushed": pushed.versions.iter().map(|(n, r)| json!({"n": n, "remote_n": r})).collect::<Vec<_>>(),
        "assets_not_pushed": pushed.assets_not_pushed,
    }))
    .into_response())
}

enum Approval {
    /// The owner has yet to answer question `0`.
    Pending(String),
    /// The owner approved; `Some(true)` lets agents push new versions
    /// without asking from now on. The push sends versions up to the
    /// second (the one the owner was asked about), or every one.
    Granted(Option<bool>, Option<u32>),
}

/// Whether agent session `sid` may push `aid` to `remote` now: a link that
/// follows new versions (an update), or the owner's answer to the approval
/// question `qid`; else a question for the owner (one per session,
/// artifact and remote while it waits).
async fn approval(
    s: &AppState,
    audit: DeferredAudit,
    aid: &ArtifactId,
    remote: &Remote,
    sid: &str,
    mode: Mode,
    qid: Option<String>,
) -> Result<Approval, ApiError> {
    let (a2, r2) = (aid.as_str().to_string(), remote.id.clone());
    let link = s.store_call(move |st| st.remote_link(&a2, &r2)).await?;
    if mode == Mode::Update && link.as_ref().is_some_and(|l| l.auto) {
        return Ok(Approval::Granted(None, None));
    }
    let Some(qid) = qid else {
        return ask_owner(s, audit, aid, remote, sid).await;
    };
    let (q2, sid2) = (qid.clone(), sid.to_string());
    let (pa, q) = s
        .store_call(move |st| {
            Ok((
                st.push_approval(&q2)?,
                st.question(&q2)?.filter(|q| q.session_id == sid2),
            ))
        })
        .await?;
    let (Some(pa), Some(q)) = (pa, q) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_question",
            "no approval of this session's has this question ID: push without question_id to ask",
        ));
    };
    if pa.artifact_id != aid.as_str() || pa.remote_id != remote.id {
        return Err(ApiError::bad_request(
            "invalid_args",
            "this approval is for another artifact or remote",
        ));
    }
    if pa.used_at.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "approval_used",
            "this approval was used: push without question_id to ask again",
        ));
    }
    let refuse = |why: &str| {
        Err(ApiError::forbidden(
            "not_approved",
            format!("the owner did not approve publishing this artifact: {why}"),
        ))
    };
    if q.status == Status::Open {
        return Ok(Approval::Pending(qid));
    }
    // Decided: spent, whatever the decision, so a refusal asks anew.
    let q3 = qid.clone();
    let fresh = s.store_call(move |st| st.use_push_approval(&q3)).await?;
    if !fresh {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "approval_used",
            "this approval was used: push without question_id to ask again",
        ));
    }
    if q.status != Status::Answered {
        return refuse("they skipped the question");
    }
    // Only a person's own answer counts: Clax's browser or the extension,
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
    match choice.as_str() {
        PUBLISH_ONCE => Ok(Approval::Granted(Some(false), Some(pa.upto))),
        PUBLISH_FOLLOW => Ok(Approval::Granted(Some(true), Some(pa.upto))),
        _ => refuse("they chose not to publish"),
    }
}

/// Asks the owner whether session `sid` may push `aid` to `remote`: a
/// question this daemon writes, in the owner's inbox beside the artifact.
async fn ask_owner(
    s: &AppState,
    audit: DeferredAudit,
    aid: &ArtifactId,
    remote: &Remote,
    sid: &str,
) -> Result<Approval, ApiError> {
    let (id, r, sid, st8) = (aid.clone(), remote.clone(), sid.to_string(), s.clone());
    let qid = s
        .store_call(move |db| {
            if let Some(q) = db.open_push_approval(&sid, id.as_str(), &r.id)? {
                return Ok(q);
            }
            let a = db.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let link = db.remote_link(id.as_str(), &r.id)?;
            let who = r.person_name.as_deref().unwrap_or("you");
            let question = match &link {
                Some(l) => format!(
                    "Push \"{}\" version {} to its public copy on {}? It is public there as {who}: {}",
                    a.title, a.current_version, r.origin, l.url
                ),
                None => format!(
                    "Publish \"{}\" on {}? Its {} version{} become public there, published as {who}; whoever its link reaches can read it.",
                    a.title,
                    r.origin,
                    a.current_version,
                    if a.current_version == 1 { "" } else { "s" }
                ),
            };
            let opt = |label: &str, description: &str, recommended| QOption {
                label: label.to_string(),
                description: Some(description.to_string()),
                preview: None,
                recommended,
            };
            let questions = vec![Question {
                question,
                header: "Publish".to_string(),
                options: vec![
                    opt(PUBLISH_ONCE, "Push what it has now; ask again for later versions", false),
                    opt(
                        PUBLISH_FOLLOW,
                        "Push now, and push each new version without asking",
                        false,
                    ),
                    opt(DONT_PUBLISH, "Keep it on this machine", false),
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
            db.create_push_approval(&row.id, id.as_str(), &r.id, &sid, a.current_version)?;
            let v = clax_http::views::question::view(db, &row)?;
            clax_http::views::question::announce(&st8, db, &row, v);
            Ok(row.id)
        })
        .await?;
    Ok(Approval::Pending(qid))
}

//! A queue's link to a repository, and the owner's view of them all
//! ([`clax_store::queue_repos`]).
//!
//! - `GET /api/queues/<qid>/repo` → `{repo, app, runtime}`: the link or
//!   `null`, whether the instance has an App and where to install it, and
//!   `runtime` ([`runtime_view`], with `operator_target`: an operator's file
//!   names this queue, and a link on it is not what runs).
//! - `PUT /api/queues/<qid>/repo {repo, base?, repo_url?, forge_api_url?,
//!   protected_paths?, preview_url, preview_ready_path?,
//!   preview_names_commit?, preview_timeout_secs?, preview_poll_secs?,
//!   access, public?, allow_open_egress?, reviewed_previews?, wake_on_reply?,
//!   ci_accepted?, if_rev?}` → the same; `DELETE {if_rev}` → 204.
//! - `PUT /api/queues/<qid>/repo/tokens/<read|write> {token, if_rev}` → the
//!   same; `DELETE` the same.
//! - `GET /api/admin/repos` → `{app, links, token_approval, kek, runtime}`:
//!   every link, whoever made it, with all that makes it dangerous.
//! - `PUT /api/admin/repos/app {app_id, slug, api_url?, private_key?}` → the
//!   same; `DELETE` → 204.
//! - `PUT /api/admin/repos/policy {token_approval}` → the same.
//! - `PUT /api/admin/repos/links/<qid> {rev, approved}` → 204: the owner
//!   approves a link as they read it; `DELETE` → 204: the owner disconnects
//!   it, on any queue.
//!
//! The queue routes are a queue admin's, as `queues.rs`'s are: from the
//! daemon's own origin, in a browser or by the owner by hand, agents refused
//! 403 `not_for_agents`, a non-member and an unknown queue 404
//! `not_a_member`, a triage member 403 `not_queue_admin`. A write names the
//! `rev` it was made from, and a link that moved on answers 400
//! `link_changed`. The `/api/admin/repos` routes are the owner's alone, for
//! the App's key reaches every installation: an instance admin gets 403
//! `owner_only`, anyone else who is no admin 403 `forbidden`. Credentials
//! are write-only: no answer, audit record or log carries one, and a stored
//! value is read back only by the workspace
//! ([`clax_store::Store::open_queue_repo`]) and to ask the App's forge which
//! installation holds a repository ([`clax_store::Store::open_github_app`]).
//!
//! An App link names `{kind: "github_app"}` and no installation: the `PUT`
//! asks the forge, signed by the App, and keeps what it answers, for the owner
//! to approve by the account it names. That answers any queue admin whether
//! the App is installed on a repository they name, and on what account.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use clax_core::CoreError;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_core::queue::Role;
use clax_http::agent::DeniedToGrants;
use clax_http::auth::Caller;
use clax_http::bot::{Bot, Names, key_info, may_admin, record_view, role_of, view};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::Store;
use clax_store::queue_repos::{
    GithubAppFields, LinkAccess, RepoAccess, RepoLink, RepoLinkFields, TokenKind,
};
use clax_store::roles::check_may_grant_admin;
use serde::Deserialize;
use serde_json::{Value, json};

/// The caller and the person they act as on a queue: `None` for the owner.
///
/// # Errors
/// As `queues.rs`'s `admin_person`.
pub(super) async fn queue_caller(
    s: &AppState,
    d: &DeniedToGrants,
) -> Result<(Caller, Option<String>), ApiError> {
    let caller = d.caller(s).await?;
    let person = match &caller {
        Caller::Owner { .. } => None,
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        Caller::Anonymous => return Err(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            return Err(clax_http::auth::not_for_agents());
        }
        Caller::Viewer { .. } => {
            return Err(ApiError::forbidden(
                "not_a_person",
                "only the owner and connected people administer queues",
            ));
        }
    };
    Ok((caller, person))
}

/// 404 `not_a_member` for a non-member or no such queue; 403
/// `not_queue_admin` for a triage member.
fn require_admin(st: &Store, qid: &str, person: Option<&str>) -> Result<(), ApiError> {
    match st.queue_role(qid, person)? {
        Some(Role::Admin) => Ok(()),
        Some(Role::Triage) => Err(ApiError::forbidden(
            "not_queue_admin",
            "only a queue's admins read and change its repository",
        )),
        None => Err(CoreError::NotAMember.into()),
    }
}

/// [`require_admin`] on the store's thread.
pub(super) async fn admin_of(
    s: &AppState,
    qid: &str,
    person: Option<String>,
) -> Result<(), ApiError> {
    let qid = qid.to_string();
    s.store_call(move |st| Ok(require_admin(st, &qid, person.as_deref())))
        .await?
}

/// The caller, refused 403 `owner_only` unless the instance's owner.
async fn owner(s: &AppState, d: &DeniedToGrants) -> Result<Caller, ApiError> {
    let caller = d.caller(s).await?;
    let c = caller.clone();
    let role = s.store_call(move |st| Ok(may_admin(st, &c))).await??;
    check_may_grant_admin(role).map_err(|_| {
        ApiError::forbidden(
            "owner_only",
            "only the owner sees and sets the instance's repositories",
        )
    })?;
    Ok(caller)
}

/// The workspace this host is running ([`clax_http::bot::RuntimeFacts`]):
/// `{configured, third_party, egress_held, failed, operator_only}`.
fn runtime_view(bot: &Bot) -> Value {
    let r = bot.runtime();
    json!({
        "configured": r.configured,
        "third_party": r.third_party,
        "egress_held": r.egress_held,
        "failed": r.failed,
        "operator_only": r.operator_only,
    })
}

/// Fills `f`'s App access from the forge: the installation holding its
/// repository, and the account the forge names. 400 `invalid_repo`,
/// `no_github_app` with no App and key, `invalid_app_key` for a key that
/// cannot sign, or `app_not_installed`, naming where to install it; 502
/// `forge_error` when the forge gives no answer; 503 `no_kek`.
#[cfg(not(target_arch = "wasm32"))]
async fn find_installation(s: &AppState, f: &mut RepoLinkFields) -> Result<(), ApiError> {
    use clax_bot_workspace::github_app::GitHubApp;
    use clax_bot_workspace::net::PublicOnly;
    use clax_bot_workspace::runtime::control_client;
    if !matches!(f.access, RepoAccess::GithubApp { .. }) {
        return Ok(());
    }
    let repo = clax_store::queue_repos::check_repo(&f.repo)?;
    let bot = s.bot.clone();
    bot.kek()?;
    let (opened, install_url) = s
        .store_call(move |st| {
            let kek = bot.kek.as_ref().ok_or(CoreError::NotFound)?;
            let url = st.github_app()?.map(|a| a.install_url);
            Ok((st.open_github_app(kek)?, url))
        })
        .await?;
    let Some(app) = opened else {
        return Err(ApiError::bad_request(
            "no_github_app",
            "this instance has no GitHub App with a key",
        ));
    };
    let http = if app.local_forge {
        control_client()
    } else {
        PublicOnly::default().client()
    };
    let client = GitHubApp::new(&app.api_url, app.app_id, app.private_key.expose())
        .map_err(|_| {
            ApiError::bad_request(
                "invalid_app_key",
                "the App's private key cannot sign: its owner must set it again",
            )
        })?
        .with_http(http);
    let found = match client.installation_of(&repo).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(
                error = format!("{e:#}"),
                "asking for an installation failed"
            );
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "forge_error",
                "the App's forge did not answer: try again, or ask the owner of this Clax to look at its key",
            ));
        }
    };
    s.store_call(|st| st.mark_secret_ok(&clax_store::secrets::SecretSlot::GithubApp))
        .await?;
    let Some(found) = found else {
        let install = install_url.unwrap_or_default();
        return Err(ApiError::bad_request(
            "app_not_installed",
            format!("the App is not installed on {repo}: install it at {install}, then try again"),
        ));
    };
    f.access = RepoAccess::GithubApp {
        installation_id: found.id,
        account: found.account,
    };
    Ok(())
}

/// A hosted Worker holds nothing that speaks to a forge: a workspace service
/// may, for its operator's targets, and a link is not one.
#[cfg(target_arch = "wasm32")]
async fn find_installation(_s: &AppState, f: &mut RepoLinkFields) -> Result<(), ApiError> {
    if matches!(f.access, RepoAccess::GithubApp { .. }) {
        return Err(ApiError::bad_request(
            "no_github_app",
            "this Clax cannot reach a GitHub App",
        ));
    }
    Ok(())
}

fn link_view(st: &Store, link: &RepoLink) -> clax_core::Result<Value> {
    let mut names = Names::new(st);
    let access = match &link.access {
        LinkAccess::GithubApp {
            installation_id,
            account,
            approved_by,
            approved_at,
        } => json!({
            "kind": "github_app",
            "installation_id": installation_id,
            "account": account,
            "approved": approved_by.is_some(),
            "approved_by": approved_by.as_deref().map(|p| names.person(p)).transpose()?,
            "approved_at": approved_at,
        }),
        LinkAccess::Token {
            read_token,
            write_token,
            approved_by,
            approved_at,
        } => json!({
            "kind": "token",
            "read_token": key_info(&mut names, read_token.as_deref(), &link.repo_url, "the read token")?,
            "write_token": key_info(&mut names, write_token.as_deref(), &link.repo_url, "the write token")?,
            "approved": approved_by.is_some(),
            "approved_by": approved_by.as_deref().map(|p| names.person(p)).transpose()?,
            "approved_at": approved_at,
        }),
    };
    Ok(json!({
        "rev": link.rev,
        "repo": link.repo,
        "base": link.base,
        "repo_url": link.repo_url,
        "forge_api_url": link.forge_api_url,
        "protected_paths": link.protected_paths,
        "preview_url": link.preview.url,
        "preview_ready_path": link.preview.ready_path,
        "preview_names_commit": link.preview.names_commit,
        "preview_timeout_secs": link.preview.timeout_secs,
        "preview_poll_secs": link.preview.poll_secs,
        "access": access,
        "public": link.public,
        "allow_open_egress": link.allow_open_egress,
        "reviewed_previews": link.reviewed_previews,
        "wake_on_reply": link.wake_on_reply,
        "ci_accepted": link.ci_accepted,
        "status": link.status.as_str(),
        "last_ok_at": link.last_ok_at,
        "last_failure": link.last_failure,
        "last_failure_at": link.last_failure_at,
        "set_by": names.person(&link.set_by)?,
        "set_at": link.set_at,
    }))
}

/// What the queue routes answer of queue `qid`: `link`, the App as a queue
/// admin sees it, and the runtime.
fn answer(st: &Store, bot: &Bot, qid: &str, link: Option<&RepoLink>) -> clax_core::Result<Value> {
    let app = st.github_app()?;
    let mut runtime = runtime_view(bot);
    runtime["operator_target"] = json!(bot.runtime().operator_queues.iter().any(|q| q == qid));
    Ok(json!({
        "repo": link.map(|l| link_view(st, l)).transpose()?,
        "app": {
            "configured": app.as_ref().is_some_and(|a| a.key.is_some()),
            "install_url": app.as_ref().map(|a| a.install_url.as_str()),
            "forge_api_url": app.as_ref().map(|a| a.api_url.as_str()),
        },
        "runtime": runtime,
    }))
}

/// `GET /api/queues/<qid>/repo` (see the module documentation), recorded as
/// `view.admin {page: "queue_repo", queue_id}`, once a window.
pub async fn get(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    qid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    admin_of(&s, &qid, person).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = answer(st, &bot, &qid, st.queue_repo(&qid)?.as_ref())?;
            let rec = view(st, AuditKind::ViewAdmin)
                .with("page", "queue_repo")
                .with("queue_id", qid.as_str());
            record_view(st, &bot, &who, &caller, &format!("queue_repo/{qid}"), rec)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/queues/<qid>/repo`: sets the link. `access` is `{kind:
/// "token"}` or `{kind: "github_app"}`, whose installation comes from the
/// forge; `if_rev` the `rev` this was made from, null or left out for no
/// link. A change to `repo`, `repo_url`, `forge_api_url` or the installation
/// clears the link's tokens, which were set for the old one, voids its
/// approval and leaves `ci_accepted` false whatever the body says. Anyone but
/// the owner changing what else the owner read voids the approval too. 400
/// `link_changed` when the link has moved on, for a field
/// [`clax_store::Store::set_queue_repo`] refuses, or as
/// [`find_installation`] says.
pub async fn put(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<RepoLinkFields>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    let mut b = body(req)?;
    admin_of(&s, &qid, person).await?;
    find_installation(&s, &mut b).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let by_owner = check_may_grant_admin(role_of(st, &caller)?).is_ok();
            let link = st.set_queue_repo(&ctx, &qid, &b, by_owner)?;
            answer(st, &bot, &qid, Some(&link))
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/queues/<qid>/repo` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteBody {
    if_rev: String,
}

/// `DELETE /api/queues/<qid>/repo {if_rev}` → 204: removes the link and its
/// tokens (no link is fine). 400 `link_changed` when it has moved on.
pub async fn delete(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<DeleteBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    let (_, person) = queue_caller(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    s.store_call(move |st| st.clear_queue_repo_at(&ctx, &qid, &b.if_rev))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The token a route's path names, or 404 `not_found`.
fn token_kind(which: &str) -> Result<TokenKind, ApiError> {
    TokenKind::parse(which).ok_or_else(|| CoreError::NotFound.into())
}

/// `PUT /api/queues/<qid>/repo/tokens/<read|write>` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBody {
    token: String,
    if_rev: String,
}

/// `PUT /api/queues/<qid>/repo/tokens/<read|write> {token, if_rev}`: seals
/// the token for the link as read at `if_rev`, so that it goes only where its
/// person saw. 400 `no_repo_link` with no link, `link_changed` when the link
/// has moved on, `wrong_access` for an App link, `invalid_key`, or
/// `same_token` for the other token's value; 503 `no_kek`.
pub async fn put_token(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (_, person) = queue_caller(&s, &d).await?;
    let (qid, which) = path(p)?;
    let kind = token_kind(&which)?;
    let b = body(req)?;
    admin_of(&s, &qid, person).await?;
    let bot = s.bot.clone();
    bot.kek()?;
    let v = s
        .store_call(move |st| {
            let kek = bot.kek.as_ref().ok_or(CoreError::NotFound)?;
            let link = st.set_queue_repo_token(&ctx, kek, &qid, kind, &b.token, &b.if_rev)?;
            answer(st, &bot, &qid, Some(&link))
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/queues/<qid>/repo/tokens/<read|write>`: clears the token.
pub async fn delete_token(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (_, person) = queue_caller(&s, &d).await?;
    let (qid, which) = path(p)?;
    let kind = token_kind(&which)?;
    admin_of(&s, &qid, person).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let link = st.clear_queue_repo_token(&ctx, &qid, kind)?;
            answer(st, &bot, &qid, Some(&link))
        })
        .await?;
    Ok(Json(v))
}

/// What the owner's routes answer.
fn repos_view(st: &Store, bot: &Bot) -> clax_core::Result<Value> {
    let mut names = Names::new(st);
    let app = st
        .github_app()?
        .map(|a| -> clax_core::Result<Value> {
            Ok(json!({
                "app_id": a.app_id,
                "slug": a.slug,
                "api_url": a.api_url,
                "install_url": a.install_url,
                "key": key_info(&mut names, a.key.as_ref(), &a.api_url, "the App's key")?,
                "set_by": names.person(&a.set_by)?,
                "set_at": a.set_at,
            }))
        })
        .transpose()?;
    let links = st
        .repo_links()?
        .into_iter()
        .map(|l| -> clax_core::Result<Value> {
            let admins = l
                .admins
                .iter()
                .map(|p| names.person(p))
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({
                "queue_id": l.queue_id,
                "queue_name": l.queue_name,
                "repo": l.repo,
                "repo_url": l.repo_url,
                "forge_api_url": l.forge_api_url,
                "access": l.access,
                "installation_id": l.installation_id,
                "account": l.account,
                "status": l.status.as_str(),
                "approved": l.approved,
                "approvable": l.approvable,
                "public": l.public,
                "allow_open_egress": l.allow_open_egress,
                "reviewed_previews": l.reviewed_previews,
                "wake_on_reply": l.wake_on_reply,
                "preview_url": l.preview_url,
                "preview_ready_path": l.preview_ready_path,
                "protected_paths": l.protected_paths,
                "ci_accepted": l.ci_accepted,
                "rev": l.rev,
                "maker": names.person(&l.maker)?,
                "admins": admins,
                "set_by": names.person(&l.set_by)?,
                "set_at": l.set_at,
            }))
        })
        .collect::<clax_core::Result<Vec<_>>>()?;
    Ok(json!({
        "app": app,
        "links": links,
        "token_approval": st.repo_token_approval()?,
        "kek": {"set": bot.kek.is_some()},
        "runtime": runtime_view(bot),
    }))
}

/// `GET /api/admin/repos` → `{app, links, token_approval, kek: {set},
/// runtime}`: the owner reads every queue's link, those waiting on them
/// first, each with its addresses, what an approval of it reads (`public`,
/// `allow_open_egress`, `reviewed_previews`, `wake_on_reply`, `preview_url`,
/// `preview_ready_path`, `protected_paths`, `ci_accepted`), its queue's maker
/// and admins and whether an approval decides anything for it, and the App
/// (`null` without one; its key as `key_info`). Recorded as `view.admin
/// {page: "repos"}`, once a window.
pub async fn get_repos(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let caller = owner(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = repos_view(st, &bot)?;
            let rec = view(st, AuditKind::ViewAdmin).with("page", "repos");
            record_view(st, &bot, &who, &caller, "repos", rec)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/admin/repos/app` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppBody {
    app_id: u64,
    slug: String,
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    private_key: Option<String>,
}

/// `PUT /api/admin/repos/app {app_id, slug, api_url?, private_key?}`: the
/// owner sets the App (`api_url` GitHub's when left out). `private_key`, the
/// PEM document, is sealed; left out, the key stays, unless the App moved
/// to another forge or ID, which clears it and voids every App approval. 400
/// `invalid_app`, `invalid_forge_url`, `invalid_app_key`; 503 `no_kek`.
pub async fn put_app(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    req: Result<Json<AppBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    owner(&s, &d).await?;
    let b = body(req)?;
    let bot = s.bot.clone();
    bot.kek()?;
    let v = s
        .store_call(move |st| {
            let kek = bot.kek.as_ref().ok_or(CoreError::NotFound)?;
            let fields = GithubAppFields {
                app_id: b.app_id,
                slug: b.slug,
                api_url: b.api_url,
            };
            st.set_github_app(&ctx, kek, &fields, b.private_key.as_deref())?;
            repos_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/admin/repos/app` → 204: the owner removes the App and its
/// key, and voids every App approval. Links stay, `needs_app`.
pub async fn delete_app(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    owner(&s, &d).await?;
    s.store_call(move |st| st.clear_github_app(&ctx)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/admin/repos/policy` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyBody {
    token_approval: bool,
}

/// `PUT /api/admin/repos/policy {token_approval}`: the owner says whether a
/// token link waits for their approval, as an App link always does.
pub async fn put_policy(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    req: Result<Json<PolicyBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    owner(&s, &d).await?;
    let b = body(req)?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            st.set_repo_token_approval(&ctx, b.token_approval)?;
            repos_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/admin/repos/links/<qid>` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalBody {
    rev: String,
    approved: bool,
}

/// `PUT /api/admin/repos/links/<qid> {rev, approved}` → 204: the owner
/// approves that link as read at `rev`, or withdraws it. 400 `no_repo_link`,
/// `link_changed` when it has moved on, or `no_github_app`.
pub async fn approve(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
    req: Result<Json<ApprovalBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    owner(&s, &d).await?;
    let qid = path(qid)?;
    let b = body(req)?;
    s.store_call(move |st| st.approve_queue_repo(&ctx, &qid, &b.rev, b.approved))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/admin/repos/links/<qid>` → 204: the owner removes a link and
/// its tokens, on any queue, a member of it or not (no link is fine).
pub async fn disconnect(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    ctx: AuditCtx,
    qid: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    d.refuse_agents()?;
    owner(&s, &d).await?;
    let qid = path(qid)?;
    s.store_call(move |st| st.clear_queue_repo(&ctx, &qid))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

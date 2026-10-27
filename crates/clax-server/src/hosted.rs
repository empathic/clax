//! Hosted mode's own routes: browser sign-in, setup, invites, sign-in
//! links, agent tokens, links and sharing, an artifact's Comments switch,
//! the apps library, the public gallery, device sign-in, and the router that mounts them ([`router`]). What the gate,
//! the request identity and the rest of the daemon share of hosted mode is
//! [`clax_http::hosted`].

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};
use clax_core::CoreError;
use clax_core::rights::RightType;
use clax_http::error::ApiError;
use clax_http::hosted::*;
use clax_http::state::AppState;
use clax_store::hosted::{LinkAccess, SessionViewer};
use serde::Deserialize;
use serde_json::{Value, json};

/// Ends the session a browser held before it signed in anew (`old`, its
/// cookie value), so a cookie it no longer holds signs no one in.
fn end_replaced(
    st: &clax_store::Store,
    ctx: &clax_core::audit::AuditCtx,
    old: Option<String>,
) -> clax_core::Result<()> {
    if let Some(t) = old {
        st.end_browser_session(ctx, &t, "replaced")?;
    }
    Ok(())
}

/// Tells the room sockets of site holders that who may read changed
/// (`Event::Access`): at artifact `aid`, or at any (`None`: a browser
/// session ended, and the site sessions made from it with it; a person
/// removed or their rights changed). Each checks again that its holder
/// still reads its artifact.
pub(crate) fn access_changed(s: &AppState, aid: Option<&str>) {
    s.events.publish(clax_core::Event::Access {
        artifact_id: aid.map(str::to_string),
    });
}

fn hosted_of(s: &AppState) -> std::sync::Arc<Hosted> {
    s.hosted
        .clone()
        .expect("hosted routes are served in hosted mode only")
}

/// The browser's session; 401 `sign_in_required` without one.
fn session_of(r: &HostedRequest) -> Result<&SessionViewer, ApiError> {
    r.session.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "sign_in_required",
            "sign in to Clax first",
        )
    })
}

/// The browser's member session (the owner's or a person's); 401 without
/// a session, 403 `forbidden` for a guest.
fn member_of<'a>(r: &'a HostedRequest, what: &str) -> Result<&'a SessionViewer, ApiError> {
    let who = session_of(r)?;
    if who.member() {
        Ok(who)
    } else {
        Err(ApiError::forbidden(
            "forbidden",
            format!("a guest may not {what}"),
        ))
    }
}

/// 403 `browser_required` for a request with `Authorization`: these routes
/// are the browser's.
fn browser_only(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::AUTHORIZATION) {
        Err(ApiError::forbidden(
            "browser_required",
            "this is a browser's route: open Clax in a browser",
        ))
    } else {
        Ok(())
    }
}

/// The person a request on the sharing routes, the Comments switch and the
/// apps library acts for: a person's own device
/// ([`clax_http::agent::PersonDevice`], which reaches these routes alone)
/// speaks for its person; any other request is a browser's
/// ([`browser_only`]) member session ([`member_of`]).
fn browser_or_device(
    headers: &HeaderMap,
    r: &HostedRequest,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
    what: &str,
) -> Result<SessionViewer, ApiError> {
    if let Some(Extension(d)) = device {
        return Ok(d.who);
    }
    browser_only(headers)?;
    Ok(member_of(r, what)?.clone())
}

/// The version of the documents a sign-in now agrees to, when the
/// instance shows any ([`clax_http::hosted::Hosted::with_legal`]).
fn legal_version(h: &Hosted) -> Option<String> {
    h.legal.as_ref().map(|l| l.version().to_string())
}

/// Records a browser sign-in under the sign-in page's notice as its person
/// agreeing to `version` of the documents (`None`: the instance shows
/// none). A guest agrees to nothing.
pub(crate) fn agree_at_sign_in(
    st: &clax_store::Store,
    signed: &clax_store::hosted::SignedIn,
    version: Option<&str>,
) -> clax_core::Result<()> {
    match version {
        Some(v) if !signed.who.guest => st.legal_agree(
            &signed.who.viewer.id,
            v,
            clax_store::legal::AgreedVia::SignIn,
        ),
        _ => Ok(()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgreeBody {
    version: String,
}

/// `POST /api/hosted/legal/agree {version}` (a signed-in person, in a
/// browser): records them agreeing to the documents' current `version`
/// from the signed-in prompt; 204. 404 `not_found` when the instance shows
/// no documents; 409 `legal_changed` when `version` is not the current one
/// (the person read an older text); 403 for a guest or no session.
async fn legal_agree(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    req: Result<Json<AgreeBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let Some(current) = legal_version(&h) else {
        return Err(ApiError::not_found());
    };
    let Some(who) = r.session.as_ref().filter(|v| !v.guest) else {
        return Err(ApiError::forbidden(
            "forbidden",
            "sign in as a person to agree to the documents",
        ));
    };
    let b = crate::routes::artifacts::body(req)?;
    if b.version != current {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "legal_changed",
            "the documents changed since they were shown: read them again",
        ));
    }
    let id = who.viewer.id.clone();
    s.store_call(move |st| st.legal_agree(&id, &current, clax_store::legal::AgreedVia::Prompt))
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /terms` and `GET /privacy`: the bundled documents
/// ([`clax_core::config::Legal::Bundled`]); 404 on an instance that shows
/// none or its own.
async fn legal_page(
    State(s): State<AppState>,
    uri: axum::http::Uri,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    if !hosted_of(&s).serves_legal() {
        return Err(ApiError::not_found());
    }
    let file = if uri.path() == "/privacy" {
        "privacy.html"
    } else {
        "terms.html"
    };
    crate::shell::legal_page(&s, &req, file)
}

/// `GET /api/hosted/me` → `{origin, setup_open, github, signed_in, person,
/// legal}`; `legal` is null when the instance shows no documents, else
/// `{terms_url, privacy_url, version, agreed}`, `agreed` null for a guest
/// or no session.
async fn me(
    State(s): State<AppState>,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    let open = h.setup_secret.is_some() && s.store_call(|st| st.hosted_needs_setup()).await?;
    let legal = match &h.legal {
        None => serde_json::Value::Null,
        Some(l) => {
            let version = l.version().to_string();
            let person = r
                .session
                .as_ref()
                .filter(|v| !v.guest)
                .map(|v| v.viewer.id.clone());
            let agreed = match person {
                Some(id) => Some(
                    s.store_call(move |st| st.legal_agreed(&id, &version))
                        .await?,
                ),
                None => None,
            };
            json!({
                "terms_url": l.terms_url(),
                "privacy_url": l.privacy_url(),
                "version": l.version(),
                "agreed": agreed,
            })
        }
    };
    Ok(no_store(
        Json(json!({
            "origin": h.origin,
            "setup_open": open,
            "github": h.github.is_some(),
            "signed_in": r.session.is_some(),
            "person": r.session.as_ref().map(person_view),
            "legal": legal,
        }))
        .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupBody {
    secret: String,
    name: String,
}

/// `POST /api/hosted/setup {secret, name}`: makes the first person (the
/// owner) and signs them in, once. 404 `not_found` once setup is done, and
/// for a missing, unset or wrong secret alike.
async fn setup(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    audit: clax_http::audit::DeferredAudit,
    req: Result<Json<SetupBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    h.attempt(Attempt::Setup, &headers, &extensions)?;
    let b = crate::routes::artifacts::body(req)?;
    if !h.setup_secret_matches(&b.secret) {
        return Err(ApiError::not_found());
    }
    let old = session_token(&headers, h.cookie_name());
    let replaced = old.is_some();
    let legal = legal_version(&h);
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.hosted_setup(&ctx, &b.name)?;
            agree_at_sign_in(st, &signed, legal.as_deref())?;
            end_replaced(st, &ctx, old)?;
            Ok(signed)
        })
        .await?;
    if replaced {
        access_changed(&s, None);
    }
    Ok(signed_in_response(&h, signed, json!({}), "person"))
}

/// `GET /api/hosted/invites/<code>` → `{name, expires_at}` while the invite
/// may be redeemed in a browser; else 404 `not_found`.
async fn peek_invite(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let peek = s.store_call(move |st| st.peek_invite(&code)).await?;
    match peek {
        Some(p) => Ok(no_store(Json(json!(p)).into_response())),
        None => Err(ApiError::not_found()),
    }
}

/// The body of a redemption (an invite's, a sign-in link's): `{replace}`,
/// or none. `replace` confirms ending the browser's current session.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemBody {
    #[serde(default)]
    replace: bool,
}

fn redeem_body(bytes: &[u8]) -> Result<RedeemBody, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(RedeemBody::default());
    }
    serde_json::from_slice(bytes).map_err(|e| {
        ApiError::bad_request(
            "invalid_json",
            format!("send {{\"replace\": true}} or nothing: {e}"),
        )
    })
}

/// 409 `session_exists`: the browser is signed in as `current`, whose
/// session a redemption would end; it is redone with `{"replace": true}`
/// once the person confirmed. `current` is the person as `GET
/// /api/hosted/me` names them.
fn session_exists(current: &SessionViewer) -> ApiError {
    let name = current
        .viewer
        .display_name
        .as_deref()
        .unwrap_or("an unnamed person");
    let mut e = ApiError::new(
        StatusCode::CONFLICT,
        "session_exists",
        format!("this browser is signed in as {name}: confirm to sign out of {name} first"),
    );
    e.extra.insert("current".into(), person_view(current));
    e
}

/// `POST /api/hosted/invites/<code>/accept {replace?}`: redeems the invite,
/// making a connected person under its name, and signs the browser in as
/// them. 404 `not_found` for an invite that is not live (one answer for
/// every case); 409 [`session_exists`] while the browser holds a live
/// session and `replace` is not set, spending nothing.
async fn accept_invite(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    code: Result<Path<String>, PathRejection>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    h.attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let b = redeem_body(&body)?;
    if let Some(current) = r.session.as_ref().filter(|_| !b.replace) {
        return Err(session_exists(current));
    }
    let old = session_token(&headers, h.cookie_name());
    let replaced = old.is_some();
    let legal = legal_version(&h);
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.redeem_invite_in_browser(&ctx, &code)?;
            agree_at_sign_in(st, &signed, legal.as_deref())?;
            end_replaced(st, &ctx, old)?;
            Ok(signed)
        })
        .await
        .map_err(|e| match e.code {
            "invalid_invite" => ApiError::not_found(),
            _ => e,
        })?;
    if replaced {
        access_changed(&s, None);
    }
    crate::routes::grants::announce(
        &s,
        json!({"what": "person_joined", "person": signed.who.viewer.public_id}),
    );
    // The new person's quickstart: awaited before the response so the first
    // page load sees it; errors are logged only, never failing the accept.
    crate::quickstart::seed(&s, Some(&signed.who.viewer.public_id)).await;
    Ok(signed_in_response(&h, signed, json!({}), "person"))
}

/// `POST /api/hosted/signin-links` (a member) → `{url, expires_at}`: a
/// one-use link, good for ten minutes, that signs another browser in as
/// the caller.
async fn create_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let id = member_of(&r, "make sign-in links")?.viewer.id.clone();
    let (code, expires_at) = s.store_call(move |st| st.create_signin_link(&id)).await?;
    Ok(no_store(
        Json(json!({
            "url": h.url(&format!("/signin/{}", code.expose())),
            "expires_at": expires_at,
        }))
        .into_response(),
    ))
}

/// `POST /api/hosted/owner-signin-link` (the daemon token, in
/// `Authorization: Bearer`) → `{url, expires_at}`: a one-use link, good for
/// ten minutes, that signs a browser in as the owner. It is how the owner
/// gets back in from the machine that holds the deploy's owner token, when
/// no browser of theirs is signed in.
async fn create_owner_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !clax_http::auth::has_token(&headers, &s.token) {
        return Err(ApiError::unauthorized());
    }
    let h = hosted_of(&s);
    let (code, expires_at) = s
        .store_call(move |st| {
            let owner = st.owner_viewer(true)?;
            st.create_signin_link(&owner.id)
        })
        .await?;
    Ok(no_store(
        Json(json!({
            "url": h.url(&format!("/signin/{}", code.expose())),
            "expires_at": expires_at,
        }))
        .into_response(),
    ))
}

/// `GET /api/hosted/signin-links/<code>` → `{public_id, name, owner,
/// expires_at}`: whose account the live link signs into, spending
/// nothing; else 404 `not_found`.
async fn peek_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    match s.store_call(move |st| st.peek_signin_link(&code)).await? {
        Some(p) => Ok(no_store(Json(json!(p)).into_response())),
        None => Err(ApiError::not_found()),
    }
}

/// `POST /api/hosted/signin-links/<code>/accept {replace?}`: signs this
/// browser in as the link's member, once. 404 `not_found` for an unknown,
/// used or expired link; 409 [`session_exists`] while the browser holds a
/// live session of someone else and `replace` is not set, spending
/// nothing (so a link someone sent cannot swap the browser's person
/// without the page saying whose account it signs into).
async fn accept_signin_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    code: Result<Path<String>, PathRejection>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    h.attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let b = redeem_body(&body)?;
    if let Some(current) = r.session.clone().filter(|_| !b.replace) {
        let peek = code.clone();
        let target = s.store_call(move |st| st.peek_signin_link(&peek)).await?;
        if target.is_some_and(|t| t.public_id != current.viewer.public_id) {
            return Err(session_exists(&current));
        }
    }
    let old = session_token(&headers, h.cookie_name());
    let replaced = old.is_some();
    let legal = legal_version(&h);
    let signed = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let signed = st.redeem_signin_link(&ctx, &code)?;
            agree_at_sign_in(st, &signed, legal.as_deref())?;
            end_replaced(st, &ctx, old)?;
            Ok(signed)
        })
        .await?;
    if replaced {
        access_changed(&s, None);
    }
    // A person's quickstart moves to the current release (a seed that
    // failed is retried); the owner's and a guest's gallery is not seeded.
    if !signed.who.owner && !signed.who.guest {
        crate::quickstart::sign_in(&s, &signed.who.viewer.public_id).await;
    }
    Ok(signed_in_response(&h, signed, json!({}), "person"))
}

/// `POST /api/hosted/signout`: ends the browser's session and clears its
/// cookie; 204 whether or not one was live.
async fn sign_out(
    State(s): State<AppState>,
    headers: HeaderMap,
    audit: clax_http::audit::DeferredAudit,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    if let Some(t) = session_token(&headers, h.cookie_name()) {
        s.store_call(move |st| st.end_browser_session(&audit.resolve(st)?, &t, "sign_out"))
            .await?;
        // Its site sessions end with it, and the room sockets opened on them.
        access_changed(&s, None);
    }
    let mut res = no_store(StatusCode::NO_CONTENT.into_response());
    res.headers_mut()
        .insert(header::SET_COOKIE, h.clear_cookie());
    Ok(res)
}

fn token_view(t: &clax_store::hosted::AgentToken) -> Value {
    json!(t)
}

/// `GET /api/hosted/agent-tokens` (a member) → `{tokens}`, newest first,
/// without their tokens, each with its `kind`: `device` for the person's
/// own device (the Clax app's sign-in), which People lists apart, else
/// `agent`.
async fn list_tokens(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "mint agent tokens")?.viewer.id.clone();
    let tokens = s.store_call(move |st| st.agent_tokens(&id)).await?;
    Ok(no_store(
        Json(json!({"tokens": tokens.iter().map(token_view).collect::<Vec<_>>()})).into_response(),
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TokenBody {
    #[serde(default)]
    label: String,
}

/// `POST /api/hosted/agent-tokens {label?}` (a member) → `{id, label,
/// token, mcp_url, claude_command, codex_config, grok_command,
/// created_at}`: an agent token for the caller's remote agents, shown this
/// once, with the commands that add it to Claude Code, Codex and Grok Build
/// (as the MCP server `clax_hosted`, a name apart from the `clax` and
/// `clax_grok` servers the plugins add in Grok). Its rights are the caller's.
/// `mcp_url` is the MCP adapter's, which serves agent tokens
/// (`crate::routes::mcp_http::PATH`: `/mcp/json` on the daemon, `/mcp` on
/// a Durable Object).
async fn mint_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    extensions: axum::http::Extensions,
    req: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let h = hosted_of(&s);
    let id = member_of(&r, "mint agent tokens")?.viewer.id.clone();
    let b = crate::routes::artifacts::body(req)?;
    let peer = clax_http::identity::peer_of(&extensions)
        .map(|p| p.ip().to_string())
        .unwrap_or_default();
    let (t, token) = s
        .store_call(move |st| st.mint_agent_token(&audit.resolve(st)?, &id, &b.label, &peer))
        .await?;
    let mcp_url = h.url(crate::routes::mcp_http::PATH);
    let secret = token.expose();
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(json!({
                "id": t.id,
                "label": t.label,
                "created_at": t.created_at,
                "token": secret,
                "mcp_url": mcp_url,
                "claude_command": format!(
                    "claude mcp add --transport http clax {mcp_url} --header \"Authorization: Bearer {secret}\""
                ),
                "codex_config": format!(
                    "[mcp_servers.clax]\nurl = \"{mcp_url}\"\nhttp_headers = {{ \"Authorization\" = \"Bearer {secret}\" }}\n"
                ),
                "grok_command": format!(
                    "grok mcp add --transport http clax_hosted {mcp_url} --header \"Authorization: Bearer {secret}\""
                ),
            })),
        )
            .into_response(),
    ))
}

/// `DELETE /api/hosted/agent-tokens/<id>` (a member, their own token):
/// revokes it and ends every agent registered under it. 404 `not_found` for
/// a token that is not the caller's.
async fn revoke_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    gid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "revoke agent tokens")?.viewer.id.clone();
    let gid = crate::routes::artifacts::path(gid)?;
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    let t = s
        .store_call(move |st| {
            if !st.agent_token_of(&id, &gid)? {
                return Err(CoreError::NotFound);
            }
            let ended = st.revoke_grant(
                &audit.resolve(st)?,
                &gid,
                clax_store::people::EndedBy::Person,
            )?;
            crate::routes::grants::announce_ended(&state, &ctx, st, &[ended], "person");
            st.agent_tokens(&id)
                .map(|ts| ts.into_iter().find(|t| t.id == gid))
        })
        .await?;
    Ok(no_store(
        Json(json!({"token": t.as_ref().map(token_view)})).into_response(),
    ))
}

/// `DELETE /api/hosted/agent-token` (an agent token, of itself) → 204: the
/// token is revoked, as its person revoking it on People would. Another
/// machine's Clax ends its own sign-in with it (`clax remote remove`). 401
/// for any other caller.
async fn revoke_own_token(
    State(s): State<AppState>,
    bearer: Option<Extension<clax_http::agent::BearerGrant>>,
    audit: clax_http::audit::DeferredAudit,
) -> Result<Response, ApiError> {
    let Some(Extension(b)) = bearer else {
        return Err(ApiError::unauthorized());
    };
    let (state, ctx) = (s.clone(), s.feedback_ctx());
    s.store_call(move |st| {
        let ended = st.revoke_grant(
            &audit.resolve(st)?,
            &b.grant_id,
            clax_store::people::EndedBy::Person,
        )?;
        crate::routes::grants::announce_ended(&state, &ctx, st, &[ended], "person");
        Ok(())
    })
    .await?;
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}

/// `GET /api/hosted/links` (a member) → `{links: [{artifact_id, title,
/// access, url}]}`: the pages shared by link that the caller shares, those
/// they made ([`may_share`], as [`set_link`] allows).
/// A link is the capability to its page, so a member is handed none they
/// could not have made.
async fn list_links(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let who = member_of(&r, "list shared links")?.clone();
    let h = hosted_of(&s);
    let gallery = h.public_gallery;
    let links = s
        .store_call(move |st| {
            let mut out = Vec::new();
            for (aid, access) in st.shared_by_link()? {
                let Ok(id) = clax_core::ArtifactId::parse(&aid) else {
                    continue;
                };
                if !may_share(st, &who, &aid)? {
                    continue;
                }
                if let Some(a) = st.get_artifact(&id)? {
                    let listed = gallery && st.link_listed(&aid)?;
                    out.push((aid, a.title, access, listed));
                }
            }
            Ok(out)
        })
        .await?;
    Ok(no_store(
        Json(
            json!({"links": links.iter().map(|(aid, title, access, listed)| json!({
            "artifact_id": aid,
            "title": title,
            "access": access,
            "listed": listed,
            "url": h.url(&format!("/a/{aid}")),
        })).collect::<Vec<_>>()}),
        )
        .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkBody {
    /// `viewer`, `commenter`, or `null` to stop sharing.
    access: Option<String>,
    /// Whether the public gallery lists it (public); absent keeps it.
    #[serde(default)]
    listed: Option<bool>,
}

/// `PUT /api/hosted/artifacts/<aid>/link {access, listed?}` (the person
/// who made the artifact, the owner for the owner's; [`may_share`]; in a
/// browser or from their own device, [`browser_or_device`]): shares it by
/// link as `viewer` or `commenter`, or stops (`null`, which also unlists
/// it),
/// and with `listed` lists it in the public gallery or not (absent keeps
/// it). → `{artifact_id, access, listed, url}`. The
/// artifact's git tokens (on stopping, every held token: a fork's need
/// read up its lineage) are rechecked before it answers
/// ([`clax_http::git_remotes::recheck`]). 400 `invalid_args` for another
/// access, 404 for no such artifact or one the caller may not read, 403
/// `forbidden` for anyone else, 403 `public_gallery_off` for `listed:
/// true` while the public gallery is off.
async fn set_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    aid: Result<Path<String>, PathRejection>,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
    req: Result<Json<LinkBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let who = browser_or_device(&headers, &r, device, "share pages")?;
    let h = hosted_of(&s);
    let aid = clax_http::headers::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let b = crate::routes::artifacts::body(req)?;
    let access = match b.access.as_deref() {
        None => None,
        Some(a) => Some(LinkAccess::parse(a).ok_or_else(|| {
            ApiError::bad_request("invalid_args", "access is viewer, commenter or null")
        })?),
    };
    if b.listed == Some(true) && !h.public_gallery {
        return Err(ApiError::forbidden(
            "public_gallery_off",
            "this Clax lists no pages publicly",
        ));
    }
    let id = aid.as_str().to_string();
    let listed = b.listed;
    let gallery = h.public_gallery;
    let (_, listed) = s
        .store_call(move |st| {
            sharer(st, &who, &aid)?;
            st.set_general_access(&audit.resolve(st)?, aid.as_str(), access, listed)
        })
        .await
        .map_err(forbidden_code)?;
    // A link viewer's and a guest's site sessions read while it is shared
    // by link.
    access_changed(&s, Some(&id));
    // A person whose rights there came from the link, and the link's own
    // read tokens, lose their git tokens before this answers; when the
    // host fails, the next settle retries. Stopping sharing rechecks every
    // token, as a fork's tokens need read at the page too (its lineage).
    if let Ok(page) = clax_core::ArtifactId::parse(&id) {
        let only = access.is_some().then_some(&page);
        if clax_http::git_remotes::recheck(&s, only).await.is_err() {
            s.git_remotes.recheck_soon();
        }
    }
    Ok(no_store(
        Json(json!({
            "artifact_id": id,
            "access": access,
            "listed": listed && gallery,
            "url": h.url(&format!("/a/{id}")),
        }))
        .into_response(),
    ))
}

/// Whether session `who` may share artifact `aid` and set who else may
/// open it: its maker alone, the owner for the artifacts the owner made.
/// An editor may not, nor the owner at a page shared with them.
///
/// # Errors
/// The store's.
pub fn may_share(
    st: &clax_store::Store,
    who: &SessionViewer,
    aid: &str,
) -> clax_core::Result<bool> {
    let maker = if who.owner {
        None
    } else {
        Some(who.viewer.public_id.as_str())
    };
    Ok(who.member() && st.artifact_created_by(aid, maker)?)
}

/// Whether the owner reaches artifact `aid` other than by its link: the
/// owner made it, or its maker shared it with the owner (the reach rule).
/// What the owner's sharing routes need before [`may_share`]: a page
/// shared by link opens to anyone, so its link is no reach to read who it
/// is shared with, and one out of reach is not there (404) rather than
/// not the owner's to share (403).
///
/// # Errors
/// The store's.
fn owner_reaches(st: &clax_store::Store, aid: &str) -> clax_core::Result<bool> {
    use clax_store::reach::Reach;
    Ok(match st.reach(aid, None)? {
        Reach::Within => true,
        Reach::Gone => false,
        Reach::Beyond => clax_http::hosted::shares_of(st, None)?.allows(
            RightType::Artifacts,
            "read",
            Some(aid),
            &|_| false,
        ),
    })
}

/// `NotFound` without live artifact `aid`, or when `who` may not read it
/// ([`session_reads`]: an artifact they may not read is not there, as at
/// its own routes), or for the owner when it is out of the owner's reach
/// but for its link ([`owner_reaches`]); `forbidden` when `who` reads it
/// but may not share it ([`may_share`]).
fn sharer(
    st: &clax_store::Store,
    who: &SessionViewer,
    aid: &clax_core::ArtifactId,
) -> clax_core::Result<()> {
    if st.get_artifact(aid)?.is_none()
        || !session_reads(st, who, aid.as_str())?
        || (who.owner && !owner_reaches(st, aid.as_str())?)
    {
        return Err(CoreError::NotFound);
    }
    if !may_share(st, who, aid.as_str())? {
        return Err(CoreError::invalid(
            "forbidden",
            "only the page's maker shares it",
        ));
    }
    Ok(())
}

/// A store refusal coded `forbidden` as 403.
fn forbidden_code(e: ApiError) -> ApiError {
    match e.code {
        "forbidden" => ApiError::forbidden("forbidden", e.message),
        _ => e,
    }
}

/// An artifact's general access as the sharing dialog names it:
/// `restricted` (only the people added), `link` (anyone with the link) or
/// `public` (anyone with the link, and listed in the public gallery).
fn general_of(access: Option<LinkAccess>, listed: bool) -> &'static str {
    match (access, listed) {
        (None, _) => "restricted",
        (Some(_), false) => "link",
        (Some(_), true) => "public",
    }
}

fn share_view(sh: &clax_store::shares::Share) -> Value {
    json!({
        "id": sh.id,
        "access": sh.access,
        "github_login": sh.github_login,
        "person": sh.person,
        "name": sh.name,
        "pending": sh.person.is_none(),
    })
}

/// `GET /api/hosted/artifacts/<aid>/sharing` (a member's browser, or
/// their own device, [`browser_or_device`]) → for
/// whoever may share it ([`may_share`]) `{artifact_id, can_share: true,
/// comments, general, link_access, url, people: [{id, access, github_login,
/// person, name, pending}], suggestions, site_access, site_url}`, `comments`
/// its Comments switch, the suggestions (people here, with their GitHub
/// logins) for the owner alone, and whether its site (at `site_url`) is
/// open to the world (`world` or `private`, `PATCH /api/artifacts/<aid>`
/// changes it); for another member who may
/// read it, `{artifact_id, can_share: false}`. 404 for an artifact the
/// caller may not read, and for the owner one out of the owner's reach but
/// for its link ([`owner_reaches`]); 401 without a session, 403 for a
/// guest.
async fn get_sharing(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let who = browser_or_device(&headers, &r, device, "share pages")?;
    let h = hosted_of(&s);
    let gallery = h.public_gallery;
    let aid = clax_http::headers::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let url = h.url(&format!("/a/{}", aid.as_str()));
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let site_url = format!("{}/", clax_http::site_access::site_origin(&s, &aid, host));
    let v = s
        .store_call(move |st| {
            if st.get_artifact(&aid)?.is_none() {
                return Err(CoreError::NotFound);
            }
            let id = aid.as_str();
            // Out of the owner's reach: not there for them.
            if who.owner && !owner_reaches(st, id)? {
                return Err(CoreError::NotFound);
            }
            if !may_share(st, &who, id)? {
                if !session_reads(st, &who, id)? {
                    return Err(CoreError::NotFound);
                }
                return Ok(json!({"artifact_id": id, "can_share": false}));
            }
            let access = st.link_access(id)?;
            let listed = gallery && st.link_listed(id)?;
            let people: Vec<Value> = st.artifact_shares(id)?.iter().map(share_view).collect();
            let suggestions = if who.owner {
                json!(st.share_candidates()?)
            } else {
                json!([])
            };
            Ok(json!({
                "artifact_id": id,
                "can_share": true,
                "comments": st.comments_on(&aid)?,
                "general": general_of(access, listed),
                "public_gallery": gallery,
                "link_access": access,
                "url": url,
                "people": people,
                "suggestions": suggestions,
                "site_access": crate::routes::artifacts::site_access_of(st, &aid)?,
                "site_url": site_url,
            }))
        })
        .await?;
    Ok(no_store(Json(v).into_response()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareBody {
    /// A GitHub login (`@` optional).
    #[serde(default)]
    github_login: Option<String>,
    /// A person's public ID.
    #[serde(default)]
    person: Option<String>,
    /// `viewer`, `commenter` or `editor`.
    access: String,
}

/// `PUT /api/hosted/artifacts/<aid>/shares {github_login | person,
/// access}` (whoever may share it, [`may_share`], in a browser or from
/// their own device, [`browser_or_device`]): shares the artifact
/// with that person, or changes the access of their share. → 200
/// `{share}`. A login no one has signed in with yet waits for its first
/// GitHub sign-in (`pending`). 400 `invalid_args` for no subject, both, or
/// another access; 400 `invalid_login` or `unknown_person` (the owner is
/// someone to share with); 404 for no such artifact or one the caller may not
/// read; 403 `forbidden` for anyone else who may not share it.
async fn put_share(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<ShareBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    use clax_store::shares::{ShareAccess, ShareTo};
    let who = browser_or_device(&headers, &r, device, "share pages")?;
    let aid = clax_http::headers::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let b = crate::routes::artifacts::body(req)?;
    let access = ShareAccess::parse(&b.access).ok_or_else(|| {
        ApiError::bad_request("invalid_args", "access is viewer, commenter or editor")
    })?;
    let to = match (b.github_login, b.person) {
        (Some(l), None) => ShareTo::GitHub(l),
        (None, Some(p)) => ShareTo::Person(p),
        _ => {
            return Err(ApiError::bad_request(
                "invalid_args",
                "name exactly one of github_login or person",
            ));
        }
    };
    let page = aid.clone();
    let share = s
        .store_call(move |st| {
            sharer(st, &who, &aid)?;
            st.share_artifact(
                &audit.resolve(st)?,
                aid.as_str(),
                &to,
                access,
                Some(&who.viewer.public_id),
            )
        })
        .await
        .map_err(forbidden_code)?;
    // A lowered share loses the git tokens it no longer allows.
    if clax_http::git_remotes::recheck(&s, Some(&page))
        .await
        .is_err()
    {
        s.git_remotes.recheck_soon();
    }
    Ok(no_store(
        Json(json!({"share": share_view(&share)})).into_response(),
    ))
}

/// `DELETE /api/hosted/artifacts/<aid>/shares/<share_id>` (whoever may
/// share it, [`may_share`], in a browser or from their own device,
/// [`browser_or_device`]): removes that share. → 200 `{share}`. Every
/// held git token is rechecked (a fork's need read up its lineage). 404
/// for no such artifact or share, or an artifact the caller may not read;
/// 403 `forbidden` for anyone else who may not share it.
async fn delete_share(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
    ids: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let who = browser_or_device(&headers, &r, device, "share pages")?;
    let (aid, sid) = crate::routes::artifacts::path(ids)?;
    let aid = clax_http::headers::parse_id(&aid)?;
    let page = aid.as_str().to_string();
    let share = s
        .store_call(move |st| {
            sharer(st, &who, &aid)?;
            st.unshare_artifact(&audit.resolve(st)?, aid.as_str(), &sid)
        })
        .await
        .map_err(forbidden_code)?;
    access_changed(&s, Some(&page));
    if clax_http::git_remotes::recheck(&s, None).await.is_err() {
        s.git_remotes.recheck_soon();
    }
    Ok(no_store(
        Json(json!({"share": share_view(&share)})).into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentsBody {
    /// Whether anyone may comment.
    on: bool,
}

/// `PUT /api/hosted/artifacts/<aid>/comments {on}` (a member's browser or
/// own device, [`browser_or_device`], who may share it, [`may_share`]: the
/// owner or the artifact's maker): turns
/// its Comments switch on or off. While it is off, no one comments there,
/// the owner included (`crate::routes::threads`). Open views hear it as
/// a `comments` event on the artifact's stream topic. → `{artifact_id,
/// comments}`. 400 `invalid_json` for another body; 404 for no such
/// artifact or one the caller may not read; 403 `forbidden` for anyone
/// else who may not share it, for a guest, and for an agent token that is
/// not a person's device's (refused before the handler, as on every route
/// such a token may not reach).
async fn put_comments(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<CommentsBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let who = browser_or_device(&headers, &r, device, "share pages")?;
    let aid = clax_http::headers::parse_id(&crate::routes::artifacts::path(aid)?)?;
    let on = crate::routes::artifacts::body(req)?.on;
    let id = aid.as_str().to_string();
    let events = s.events.clone();
    s.store_call(move |st| {
        sharer(st, &who, &aid)?;
        st.set_comments(&audit.resolve(st)?, &aid, on)?;
        events.publish(clax_core::Event::Comments {
            artifact_id: aid.as_str().to_string(),
            comments: on,
        });
        Ok(())
    })
    .await
    .map_err(forbidden_code)?;
    Ok(no_store(
        Json(json!({"artifact_id": id, "comments": on})).into_response(),
    ))
}

/// `GET /api/hosted/apps` (a member's browser, or their own device,
/// [`browser_or_device`]) → `[{artifact_id, title,
/// url, version, bundle_version, access, general}]`: the signed-in
/// person's apps library, the live artifacts carrying a bundle that they
/// made (`access: "owner"`; for the owner, those of no other person's
/// making) or that a share names them in (`access` the share's: `editor`,
/// `commenter` or `viewer`), by title ([`clax_store::Store::apps_of`]).
/// `version` is the current version, `bundle_version` the newest bundle's
/// release counter, `general` its general access as the sharing dialog
/// names it. 401 without a session, 403 for a guest, 403 `forbidden` for
/// an agent token that is not a person's device's (refused before the
/// handler).
async fn list_apps(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(r): Extension<HostedRequest>,
    device: Option<Extension<clax_http::agent::PersonDevice>>,
) -> Result<Response, ApiError> {
    let who = browser_or_device(&headers, &r, device, "list apps")?;
    let h = hosted_of(&s);
    let gallery = h.public_gallery;
    let person = (!who.owner).then(|| who.viewer.public_id.clone());
    let apps = s
        .store_call(move |st| {
            let mut out = Vec::new();
            for l in st.apps_of(person.as_deref())? {
                let access = st.link_access(&l.artifact_id)?;
                let listed = gallery && st.link_listed(&l.artifact_id)?;
                out.push(json!({
                    "artifact_id": l.artifact_id,
                    "title": l.title,
                    "url": h.url(&format!("/a/{}", l.artifact_id)),
                    "version": l.version,
                    "bundle_version": l.bundle_version,
                    "access": l.share.map_or("owner", |a| a.as_str()),
                    "general": general_of(access, listed),
                }));
            }
            Ok(out)
        })
        .await?;
    Ok(no_store(Json(Value::Array(apps)).into_response()))
}

/// `GET /api/hosted/public` (anyone, signed in or not) → `{artifacts:
/// [{artifact_id, title, url}]}`: the public gallery, the artifacts shared
/// by link and listed, most recently updated first; none while the public
/// gallery is off ([`Hosted::with_public_gallery`]). Each opens through its
/// link, as any page shared by link does.
async fn public_list(State(s): State<AppState>) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    if !h.public_gallery {
        return Ok(no_store(Json(json!({"artifacts": []})).into_response()));
    }
    let rows = s.store_call(|st| st.public_artifacts()).await?;
    let artifacts: Vec<Value> = rows
        .iter()
        .map(|(aid, title)| {
            json!({"artifact_id": aid, "title": title, "url": h.url(&format!("/a/{aid}"))})
        })
        .collect();
    Ok(no_store(
        Json(json!({"artifacts": artifacts})).into_response(),
    ))
}

/// A sign-in page (`/join/<code>`, `/signin[/<code>]`): the gallery's
/// entry, which shows it by its path, sending no referrer (its URL holds a
/// code).
async fn signin_page(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let mut res = crate::shell::gallery_page(State(s), d, req).await?;
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

/// `/setup`: the sign-in page while setup is open; 404 once it is done or
/// when the daemon has no setup secret.
async fn setup_page(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    if h.setup_secret.is_none() || !s.store_call(|st| st.hosted_needs_setup()).await? {
        return Err(ApiError::not_found());
    }
    signin_page(State(s), d, req).await
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DeviceCodeBody {
    /// What the machine calls itself: the approval page names it.
    #[serde(default)]
    client_name: String,
    /// What kind of client it is: [`clax_store::device::CLAX_APP_KIND`]
    /// alone, which marks the token it receives as its person's own device
    /// (the approval page says so).
    #[serde(default)]
    client_kind: Option<String>,
    /// A page on the machine itself (`http` on `localhost`, `127.0.0.1` or
    /// `[::1]`) that the approval page sends its person to once the
    /// sign-in is decided.
    #[serde(default)]
    return_to: Option<String>,
}

/// `POST /api/hosted/device/code {client_name, client_kind?, return_to?}`
/// (another machine's Clax, no credential) → `{device_code, user_code, verification_uri,
/// verification_uri_complete, expires_at, expires_in, interval}`: starts a
/// device sign-in ([`clax_store::device`]). The machine shows the
/// person the user code and opens `verification_uri_complete`, where a
/// signed-in member approves it, then polls `POST
/// /api/hosted/device/token` every `interval` seconds. Rate-limited per
/// client; 400 `invalid_args` for a bad name, a `client_kind` other
/// than `clax-app` or a `return_to` that is not a loopback page
/// ([`clax_http::loopback::loopback_return`]), 400 `limit_reached` with too
/// many waiting.
async fn device_code(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    req: Result<Json<DeviceCodeBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let h = hosted_of(&s);
    h.attempt(Attempt::Device, &headers, &extensions)?;
    let b = crate::routes::artifacts::body(req)?;
    if b.client_kind
        .as_deref()
        .is_some_and(|k| k != clax_store::device::CLAX_APP_KIND)
    {
        return Err(ApiError::bad_request(
            "invalid_args",
            "client_kind is clax-app or absent",
        ));
    }
    let return_to = match b.return_to.as_deref() {
        None => None,
        Some(u) => Some(clax_http::loopback::loopback_return(u).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_args",
                "return_to is an http address on localhost, 127.0.0.1 or [::1], without credentials or a fragment",
            )
        })?),
    };
    let peer = h.client(&headers, &extensions).to_string();
    let start = s
        .store_call(move |st| {
            st.start_device_kind(
                &b.client_name,
                &peer,
                b.client_kind.as_deref(),
                return_to.as_deref(),
            )
        })
        .await?;
    let user = start.user_code.expose().to_string();
    Ok(no_store(
        Json(json!({
            "device_code": start.device_code.expose(),
            "user_code": user,
            "verification_uri": h.url("/device"),
            "verification_uri_complete": h.url(&format!("/device?code={user}")),
            "expires_at": start.expires_at,
            "expires_in": clax_store::device::DEVICE_TTL_SECS,
            "interval": clax_store::device::DEVICE_INTERVAL_SECS,
        }))
        .into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceTokenBody {
    device_code: String,
}

/// `POST /api/hosted/device/token {device_code}` (the machine) → once the
/// sign-in is approved, `{token, token_type, grant_id, person: {public_id,
/// display_name}, origin, mcp_url}`, this once: an agent token of the
/// person who approved it. Before that, 400 `authorization_pending`; 400
/// `slow_down` for a poll sooner than `interval` after the last; 400
/// `access_denied` once denied (or the token revoked before this poll);
/// 400 `expired_token` for an unknown, expired or spent code.
async fn device_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    audit: clax_http::audit::DeferredAudit,
    req: Result<Json<DeviceTokenBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    use clax_store::device::DevicePoll;
    let h = hosted_of(&s);
    h.attempt(Attempt::DevicePoll, &headers, &extensions)?;
    let b = crate::routes::artifacts::body(req)?;
    let poll = s
        .store_call(move |st| st.poll_device(&audit.resolve(st)?, &b.device_code))
        .await?;
    let refuse = |code: &'static str, message: &str| Err(ApiError::bad_request(code, message));
    match poll {
        DevicePoll::Pending => refuse(
            "authorization_pending",
            "waiting for the person to approve this sign-in",
        ),
        DevicePoll::SlowDown => refuse(
            "slow_down",
            "polled too soon: wait the interval between polls",
        ),
        DevicePoll::Denied => refuse("access_denied", "this sign-in was denied"),
        DevicePoll::Expired => refuse(
            "expired_token",
            "this sign-in expired or was used: start a new one",
        ),
        DevicePoll::Issued {
            token,
            grant_id,
            person,
            person_name,
        } => Ok(no_store(
            Json(json!({
                "token": token.expose(),
                "token_type": "Bearer",
                "grant_id": grant_id,
                "person": {"public_id": person, "display_name": person_name},
                "origin": h.origin,
                "mcp_url": h.url(crate::routes::mcp_http::PATH),
            }))
            .into_response(),
        )),
    }
}

/// `GET /api/hosted/device/<user code>` (a member's browser) →
/// `{client_name, peer_ip, client_kind, return_to, created_at, expires_at,
/// same_address}`: the machine a waiting sign-in names, and its kind
/// (`clax-app` for the Clax app, whose token may manage the sharing of
/// the person's artifacts; else `null`), the loopback page it asked to
/// send its person to once decided (`return_to`, else `null`), for its
/// approval page, and whether it
/// asked from this browser's own address (a code someone else started
/// does not, which the page says). 404 `not_found` for a code that is
/// unknown, decided or expired. Rate-limited as sign-in codes are.
async fn peek_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    member_of(&r, "approve sign-ins")?;
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let here = hosted_of(&s).client(&headers, &extensions).to_string();
    match s.store_call(move |st| st.peek_device(&code)).await? {
        Some(d) => {
            let mut v = json!(d);
            v["same_address"] = json!(d.peer_ip == here);
            Ok(no_store(Json(v).into_response()))
        }
        None => Err(ApiError::not_found()),
    }
}

/// `POST /api/hosted/device/<user code>/approve` (a member's browser) →
/// `{token: {id, label, …}}`: approves the sign-in as the caller. The
/// machine's next poll receives an agent token of theirs, labelled with
/// the machine's name, which their People page lists and revokes. 404 as
/// [`peek_device`].
async fn approve_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    audit: clax_http::audit::DeferredAudit,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "approve sign-ins")?.viewer.id.clone();
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    let t = s
        .store_call(move |st| st.approve_device(&audit.resolve(st)?, &id, &code))
        .await?;
    Ok(no_store(
        Json(json!({"token": token_view(&t)})).into_response(),
    ))
}

/// `POST /api/hosted/device/<user code>/deny` (a member's browser): the
/// machine's next poll is refused `access_denied`. 204; 404 as
/// [`peek_device`].
async fn deny_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Extension(r): Extension<HostedRequest>,
    code: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    browser_only(&headers)?;
    let id = member_of(&r, "deny sign-ins")?.viewer.id.clone();
    hosted_of(&s).attempt(Attempt::SignIn, &headers, &extensions)?;
    let code = crate::routes::artifacts::path(code)?;
    s.store_call(move |st| st.deny_device(&id, &code)).await?;
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}

/// The hosted routes, merged into the daemon's router in hosted mode
/// (`crate::router`), under its agent verification, live-page filter and
/// extension gateway. Each is under [`OWN_API`] or one of [`OWN_PAGES`] or
/// [`LEGAL_PAGES`], which the gate hands on to their handlers.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/setup", get(setup_page))
        .route("/signin", get(signin_page))
        .route("/signin/{code}", get(signin_page))
        .route("/join/{code}", get(signin_page))
        .route("/terms", get(legal_page))
        .route("/privacy", get(legal_page))
        .route("/api/hosted/me", get(me))
        .route("/api/hosted/legal/agree", post(legal_agree))
        .route("/api/hosted/setup", post(setup))
        .route("/api/hosted/invites/{code}", get(peek_invite))
        .route("/api/hosted/invites/{code}/accept", post(accept_invite))
        .route("/api/hosted/signin-links", post(create_signin_link))
        .route(
            "/api/hosted/owner-signin-link",
            post(create_owner_signin_link),
        )
        .route("/api/hosted/signin-links/{code}", get(peek_signin_link))
        .route(
            "/api/hosted/signin-links/{code}/accept",
            post(accept_signin_link),
        )
        .route("/api/hosted/signout", post(sign_out))
        .route(
            "/api/hosted/agent-tokens",
            get(list_tokens).post(mint_token),
        )
        .route(
            "/api/hosted/agent-tokens/{id}",
            axum::routing::delete(revoke_token),
        )
        .route("/api/hosted/links", get(list_links))
        .route("/api/hosted/artifacts/{aid}/link", put(set_link))
        .route("/api/hosted/artifacts/{aid}/sharing", get(get_sharing))
        .route("/api/hosted/artifacts/{aid}/shares", put(put_share))
        .route("/api/hosted/artifacts/{aid}/comments", put(put_comments))
        .route("/api/hosted/apps", get(list_apps))
        .route(
            "/api/hosted/artifacts/{aid}/shares/{sid}",
            delete(delete_share),
        )
        .route("/api/hosted/public", get(public_list))
        .route("/device", get(signin_page))
        .route("/auth/github", get(crate::github::start))
        .route(
            clax_http::github::CALLBACK_PATH,
            get(crate::github::callback),
        )
        .route(SELF_REVOKE_PATH, delete(revoke_own_token))
        .route("/api/hosted/device/code", post(device_code))
        .route("/api/hosted/device/token", post(device_token))
        .route("/api/hosted/device/{code}", get(peek_device))
        .route("/api/hosted/device/{code}/approve", post(approve_device))
        .route("/api/hosted/device/{code}/deny", post(deny_device))
}

#[cfg(test)]
mod tests {
    /// Every route [`super::router`] mounts is one the gate takes as hosted
    /// mode's own (`clax_http::hosted::own_route`), so none is sent through
    /// a person's rights or a guest's needs. The routes are read from this
    /// file's source.
    #[test]
    fn the_gate_takes_every_hosted_route_as_its_own() {
        let src = include_str!("hosted.rs");
        let body = &src[src.find("pub fn router()").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let mut seen = 0;
        for call in body.split(".route(").skip(1) {
            let arg = call.split(',').next().unwrap().trim();
            let path = match arg.strip_prefix('"') {
                Some(p) => p.trim_end_matches('"'),
                None if arg.ends_with("CALLBACK_PATH") => clax_http::github::CALLBACK_PATH,
                None if arg.ends_with("SELF_REVOKE_PATH") => clax_http::hosted::SELF_REVOKE_PATH,
                None => panic!("a route at an unknown constant: {arg}"),
            };
            let concrete = path
                .split('/')
                .map(|s| if s.starts_with('{') { "x" } else { s })
                .collect::<Vec<_>>()
                .join("/");
            assert!(
                clax_http::hosted::own_route(&concrete, true),
                "{path} is not one of the gate's own routes"
            );
            seen += 1;
        }
        assert!(seen > 30, "read {seen} routes");
    }
}

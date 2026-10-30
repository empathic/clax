//! The Clax app's routes to the public copies of its apps on a remote: its
//! core carries the remote's token, which the app never holds, and these
//! forward the app's requests with it (spec 2026-10-21-app-push-pull §5.3,
//! §5.5, §5.6, §5.7).
//!
//! - `PUT /api/artifacts/<aid>/remotes/<rid>/bundle?remote_n=<n>`: a
//!   bundle for public version `n` of `aid`'s public copy on remote `rid`.
//! - `POST /api/remotes/<rid>/links {artifact_id, remote_artifact_id, url,
//!   remote_n}`: links a local app to a public copy without a push.
//! - Under `/api/remotes/<rid>/artifacts/<raid>`: `GET head`, `GET
//!   versions/<n>/bundle`, `GET versions/<n>/files` (the version, as the
//!   remote's `GET /api/artifacts/<raid>/versions/<n>` answers it), `GET
//!   sharing`, `PUT
//!   sharing/link`, `PUT sharing/shares`, `DELETE sharing/shares/<sid>`,
//!   `PUT sharing/comments` (the remote's `/api/hosted/artifacts/<raid>/…`
//!   routes), `POST installs` (the remote's `POST
//!   /api/artifacts/<raid>/installs`), and `DELETE` (Unpublish).
//! - `GET /api/remotes/<rid>/apps`: the remote's `GET /api/hosted/apps`.
//!
//! One file's bytes (`GET versions/<n>/files/<path>`), which the owner's
//! agents read as well, is `super::remote_artifacts`'.
//!
//! Each is the owner's alone, by hand (an agent is refused 403
//! `not_for_agents`), on a daemon that is not hosted (404 there, as every
//! remote route). `rid` names a remote this core is signed in to (404
//! `no_remote` otherwise). A request goes only to that remote's origin, its
//! path made of the route's own segments, each percent-encoded: `raid` is
//! an artifact ID, `n` a number, `sid` letters, digits, `-` and `_`, and a
//! file path's segments are neither empty, `.` nor `..` (400 `invalid_args`
//! otherwise). The remote's answer comes back with its status, its JSON or
//! bytes, and only its content type and a bundle's own headers; a remote
//! that refuses the token is 502 `remote_signed_out`, one not reached 502
//! `remote_unreachable`, and one from before app publishing (its 404 or
//! 403 `browser_required`, and no `GET /api/hosted/apps`) 502
//! `remote_too_old`. No answer carries the token.

use super::artifacts::{body, path};
use crate::public::{self, Forwarded, RemoteFail};
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::{ArtifactId, CoreError, Secret};
use clax_http::agent::DeniedToGrants;
use clax_http::confirm::{Confirming, Decision, require_confirmation};
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::app_bundles::MAX_BUNDLE_BYTES;
use clax_store::remotes::Remote;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

/// The headers of a bundle's upload the remote reads, passed on as sent.
const BUNDLE_HEADERS: [&str; 4] = [
    "content-type",
    "clax-bundle-lineage",
    "clax-bundle-version",
    "clax-bundle-content-sha256",
];

/// The connected remote `key` names and its token, for the owner by hand
/// (each route first refuses a hosted daemon, its agents and anyone but
/// the owner).
///
/// # Errors
/// 403 `forbidden` for an agent; 404 `no_remote`, 400 `not_connected`.
async fn owners_remote(
    s: &AppState,
    d: &DeniedToGrants,
    key: String,
) -> Result<(Remote, Secret), ApiError> {
    if !matches!(
        super::remotes::asker(s, d).await?,
        super::remotes::Asker::Owner
    ) {
        return Err(ApiError::forbidden(
            "forbidden",
            "a public Clax's copies are the owner's to reach",
        ));
    }
    let r = super::remotes::push_target(s, Some(key)).await?;
    let id = r.id.clone();
    let token = s
        .store_call(move |st| st.remote_token(&id))
        .await?
        .ok_or_else(|| ApiError::bad_request("not_connected", "this remote is not signed in"))?;
    Ok((r, token))
}

/// A public artifact's ID as a path names it.
pub(super) fn remote_artifact(raw: &str) -> Result<String, ApiError> {
    ArtifactId::parse(raw)
        .map(|a| a.as_str().to_string())
        .map_err(|_| ApiError::bad_request("invalid_args", "not a public artifact's ID"))
}

/// A share's ID as a path names it: letters, digits, `-` and `_`.
fn share_id(raw: &str) -> Result<String, ApiError> {
    if raw.is_empty()
        || raw.len() > 64
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ApiError::bad_request("invalid_args", "not a share's ID"));
    }
    Ok(raw.to_string())
}

/// A file's path as segments: none empty, `.` or `..`.
pub(super) fn file_segments(raw: &str) -> Result<Vec<String>, ApiError> {
    let segs: Vec<String> = raw.split('/').map(str::to_string).collect();
    if raw.is_empty()
        || segs
            .iter()
            .any(|s| s.is_empty() || s == "." || s == ".." || s.contains('\\'))
    {
        return Err(ApiError::bad_request("invalid_args", "not a file's path"));
    }
    Ok(segs)
}

/// The remote's answer `f` as this core gives it: a refusal of the token
/// as 502 `remote_signed_out`, a redirect or other answer it does not
/// pass on as 502 `remote_refused`, anything else with its status, body
/// and kept headers, never stored. Every answer passed on is sandboxed
/// and unsniffed: a public copy's bytes are another author's, served here
/// on this daemon's API origin.
pub(super) fn answer(origin: &str, f: Forwarded) -> Response {
    if f.status == 401 || !(200..600).contains(&f.status) || (300..400).contains(&f.status) {
        return ApiError::from(RemoteFail {
            origin: origin.to_string(),
            status: Some(f.status),
            code: "refused".into(),
            message: format!("HTTP {}", f.status),
            body: Value::Null,
        })
        .into_response();
    }
    let status = StatusCode::from_u16(f.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut res = (status, f.body).into_response();
    for (k, v) in f.headers {
        if let Ok(v) = HeaderValue::from_str(&v) {
            res.headers_mut().insert(k, v);
        }
    }
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// `method` `segs` at `remote`, with `with`'s headers and body, at most
/// `max` bytes back: its answer as [`answer`] gives it. An answer over
/// `max` is `too_large`'s.
#[allow(clippy::too_many_arguments)]
async fn forward(
    remote: &Remote,
    token: Secret,
    method: Method,
    segs: &[&str],
    query: &[(&str, String)],
    headers: Vec<(&'static str, String)>,
    sent: Option<Vec<u8>>,
    max: usize,
    too_large: impl FnOnce() -> ApiError,
) -> Response {
    let c = public::Client::new(&remote.origin, Some(token.clone()));
    match clax_rt::sendable(c.forward(method, segs, query, headers, sent, max)).await {
        Ok(f) => answer_or_too_old(remote, token, f).await,
        Err(f) if f.code == "too_large" => too_large().into_response(),
        Err(f) => ApiError::from(f).into_response(),
    }
}

/// The remote's answer `f` as [`answer`] gives it, unless it is how a
/// public Clax from before app publishing answers what it lacks (404 for
/// a route it has not, 403 `browser_required` for its sharing routes
/// with a token) and the remote lacks `GET /api/hosted/apps` too: then
/// 502 `remote_too_old` ([`public::too_old`]), so a copy is not taken
/// for gone, nor a person for refused.
async fn answer_or_too_old(remote: &Remote, token: Secret, f: Forwarded) -> Response {
    let code = || {
        serde_json::from_slice::<Value>(&f.body)
            .ok()
            .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
    };
    let suspect =
        f.status == 404 || (f.status == 403 && code().as_deref() == Some("browser_required"));
    if suspect && lacks_app_routes(remote, token).await {
        return public::too_old(&remote.origin, "reach the Clax app's copies").into_response();
    }
    answer(&remote.origin, f)
}

/// Whether `remote` answers `GET /api/hosted/apps` 404: a public Clax
/// from before app publishing, which has no such route. One that has it
/// answers a device's token 200 and another agent token 403.
async fn lacks_app_routes(remote: &Remote, token: Secret) -> bool {
    let c = public::Client::new(&remote.origin, Some(token));
    matches!(
        clax_rt::sendable(c.forward(
            Method::GET,
            &["api", "hosted", "apps"],
            &[],
            Vec::new(),
            None,
            64 * 1024,
        ))
        .await,
        Ok(f) if f.status == 404
    )
}

/// 502 `remote_refused`: a JSON answer of the remote's past this core's
/// cap.
fn answer_too_large() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "remote_refused",
        "the public Clax's answer is too large",
    )
}

/// A JSON request's body as sent: JSON, or 400 `invalid_json`.
fn json_body(bytes: &Bytes) -> Result<Vec<u8>, ApiError> {
    serde_json::from_slice::<Value>(bytes)
        .map_err(|_| ApiError::bad_request("invalid_json", "the body is not JSON"))?;
    Ok(bytes.to_vec())
}

#[derive(Deserialize)]
pub struct BundleQuery {
    /// The public version the bundle is beside.
    remote_n: u32,
}

/// `PUT /api/artifacts/<aid>/remotes/<rid>/bundle?remote_n=<n>` (the
/// owner): forwards the raw body, a bundle with its `content-type` and
/// `clax-bundle-lineage`, `clax-bundle-version` and
/// `clax-bundle-content-sha256` headers, to `PUT
/// /api/artifacts/<raid>/versions/<n>/bundle` of `aid`'s public copy on
/// `rid`, and answers as the remote does: 201 `{artifact_id, version,
/// bundle_version, sha256, size}`, the same again for the same bytes, or its
/// refusal (400 `bundle_mismatch`, `bundle_lineage`,
/// `bundle_version_order`, 404, 413 `bundle_too_large`). 404 when `aid`
/// is not linked to `rid`; 400 `invalid_args` without `remote_n`.
#[allow(clippy::too_many_arguments)]
pub async fn put_bundle(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<BundleQuery>, QueryRejection>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (aid, rid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let Query(q) =
        q.map_err(|_| ApiError::bad_request("invalid_args", "remote_n names the public version"))?;
    let id = parse_id(&aid)?;
    let r2 = remote.id.clone();
    let link = s
        .store_call(move |st| st.remote_link(id.as_str(), &r2))
        .await?
        .ok_or_else(ApiError::not_found)?;
    let sent: Vec<(&'static str, String)> = BUNDLE_HEADERS
        .iter()
        .filter_map(|k| {
            headers
                .get(*k)
                .and_then(|v| v.to_str().ok())
                .map(|v| (*k, v.to_string()))
        })
        .collect();
    let n = q.remote_n.to_string();
    Ok(forward(
        &remote,
        token,
        Method::PUT,
        &[
            "api",
            "artifacts",
            &link.remote_artifact_id,
            "versions",
            &n,
            "bundle",
        ],
        &[],
        sent,
        Some(bytes.to_vec()),
        public::MAX_ANSWER_BYTES,
        answer_too_large,
    )
    .await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBody {
    /// The local app.
    artifact_id: String,
    /// The public copy's ID on the remote.
    remote_artifact_id: String,
    /// Its address as the remote gave it; the link keeps
    /// `<origin>/a/<remote_artifact_id>`, as a push makes it.
    #[allow(dead_code)]
    url: String,
    /// The public version the app's current version is.
    remote_n: u32,
}

/// `POST /api/remotes/<rid>/links {artifact_id, remote_artifact_id, url,
/// remote_n}` (the owner) → 201 `{link}`: links local app `artifact_id`,
/// its current version, to public version `remote_n` of
/// `remote_artifact_id` on `rid` without a push, as an app got from there
/// is, or one linked again after a sign-in. Its next publish builds on
/// `remote_n`. Agents may not push it without asking. 400 `invalid_args`
/// for a malformed ID or `remote_n` 0; 404 for no such local artifact.
pub async fn link(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    rid: Result<Path<String>, PathRejection>,
    req: Result<Json<LinkBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (remote, _) = owners_remote(&s, &d, path(rid)?).await?;
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    let raid = remote_artifact(&b.remote_artifact_id)?;
    if b.remote_n == 0 {
        return Err(ApiError::bad_request(
            "invalid_args",
            "remote_n is a public version, 1 or more",
        ));
    }
    let url = format!("{}/a/{raid}", remote.origin);
    let (r2, n) = (remote.id.clone(), b.remote_n);
    let l = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.link_remote(id.as_str(), &r2, &raid, &url, a.current_version, n)
        })
        .await?;
    s.linked.links_changed();
    s.linked.kick(Some(&remote.id));
    Ok((StatusCode::CREATED, Json(json!({"link": l}))).into_response())
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/head` (the owner) → the
/// remote's `GET /api/artifacts/<raid>/head`, read with this core's token.
pub async fn head(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    Ok(get_json(&remote, token, &["api", "artifacts", &raid, "head"]).await)
}

async fn get_json(remote: &Remote, token: Secret, segs: &[&str]) -> Response {
    forward(
        remote,
        token,
        Method::GET,
        segs,
        &[],
        Vec::new(),
        None,
        public::MAX_ANSWER_BYTES,
        answer_too_large,
    )
    .await
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/versions/<n>/bundle` (the
/// owner) → the bundle public version `n` carries, as the remote answers
/// it, at most 32 MiB (413 `bundle_too_large` past that).
pub async fn bundle(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid, n) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    let n = n.to_string();
    Ok(forward(
        &remote,
        token,
        Method::GET,
        &["api", "artifacts", &raid, "versions", &n, "bundle"],
        &[],
        Vec::new(),
        None,
        MAX_BUNDLE_BYTES as usize,
        || {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "bundle_too_large",
                "the bundle is over 32 MiB",
            )
        },
    )
    .await)
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/versions/<n>/files` (the
/// owner) → the remote's `GET /api/artifacts/<raid>/versions/<n>`:
/// `{version}`, its `files` by path.
pub async fn version_files(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid, n) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    let n = n.to_string();
    Ok(get_json(&remote, token, &["api", "artifacts", &raid, "versions", &n]).await)
}

/// The remote's sharing route of `raid` for `what` (`sharing`, `link`,
/// `shares`, `shares/<sid>` or `comments`).
fn sharing_path<'a>(raid: &'a str, what: &[&'a str]) -> Vec<&'a str> {
    let mut segs = vec!["api", "hosted", "artifacts", raid];
    segs.extend_from_slice(what);
    segs
}

/// `GET /api/remotes/<rid>/artifacts/<raid>/sharing` (the owner) → the
/// remote's `GET /api/hosted/artifacts/<raid>/sharing`.
pub async fn get_sharing(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    Ok(get_json(&remote, token, &sharing_path(&raid, &["sharing"])).await)
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/installs {install}` (the
/// owner) → the remote's `POST /api/artifacts/<raid>/installs`, with this
/// core's token: an install of public copy `raid` counts there whatever
/// its general access, as long as the signed-in account may open it (an
/// anonymous count needs the copy shared by link). 204 as the remote
/// answers it; 400 `invalid_json` for a body that is not JSON.
pub async fn record_install(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
    sent: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    let sent = json_body(&sent)?;
    Ok(forward(
        &remote,
        token,
        Method::POST,
        &["api", "artifacts", &raid, "installs"],
        &[],
        vec![("content-type", "application/json".to_string())],
        Some(sent),
        public::MAX_ANSWER_BYTES,
        answer_too_large,
    )
    .await)
}

/// `PUT` `body` (JSON) to `what` of `raid`'s sharing on the remote `rid`
/// names.
async fn put_sharing(
    s: &AppState,
    d: &DeniedToGrants,
    p: Result<Path<(String, String)>, PathRejection>,
    what: &str,
    sent: &Bytes,
) -> Result<Response, ApiError> {
    let (rid, raid) = path(p)?;
    let (remote, token) = owners_remote(s, d, rid).await?;
    let raid = remote_artifact(&raid)?;
    let sent = json_body(sent)?;
    Ok(forward(
        &remote,
        token,
        Method::PUT,
        &sharing_path(&raid, &[what]),
        &[],
        vec![("content-type", "application/json".to_string())],
        Some(sent),
        public::MAX_ANSWER_BYTES,
        answer_too_large,
    )
    .await)
}

/// `PUT /api/remotes/<rid>/artifacts/<raid>/sharing/link {access,
/// listed?}` (the owner) → the remote's `PUT
/// /api/hosted/artifacts/<raid>/link`. A change that may widen who opens
/// the public copy (an `access`, so anyone with the link, or `listed:
/// true`, so the public gallery) made with the token alone while an agent
/// session is live waits for a click in the owner's browser first: 202
/// `confirmation_pending` ([`clax_http::confirm`]).
pub async fn put_link(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    by: Confirming,
    p: Result<Path<(String, String)>, PathRejection>,
    sent: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let change: Value = serde_json::from_slice(&sent)
        .map_err(|_| ApiError::bad_request("invalid_json", "the body is not JSON"))?;
    let widens = !change["access"].is_null() || change["listed"] == true;
    if widens && let Ok(Path((rid, raid))) = &p {
        let decision = Decision::Widen {
            artifact_id: raid.clone(),
            to: if change["listed"] == true {
                "public"
            } else {
                "link"
            }
            .into(),
            remote_id: Some(rid.clone()),
            body: Some(change.clone()),
        };
        require_confirmation(&s, &d.caller(&s).await?, &by, decision).await?;
    }
    put_sharing(&s, &d, p, "link", &sent).await
}

/// `PUT /api/remotes/<rid>/artifacts/<raid>/sharing/shares {github_login |
/// person, access}` (the owner) → the remote's `PUT
/// /api/hosted/artifacts/<raid>/shares`. Naming a person widens who opens
/// the public copy: made with the token alone while an agent session is
/// live, it waits for a click in the owner's browser first, as `put_link`
/// does (202 `confirmation_pending`, [`clax_http::confirm`]).
pub async fn put_share(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    by: Confirming,
    p: Result<Path<(String, String)>, PathRejection>,
    sent: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let change: Value = serde_json::from_slice(&sent)
        .map_err(|_| ApiError::bad_request("invalid_json", "the body is not JSON"))?;
    widen(&s, &d, &by, &p, "person", &change).await?;
    put_sharing(&s, &d, p, "shares", &sent).await
}

/// `PUT /api/remotes/<rid>/artifacts/<raid>/sharing/comments {on}` (the
/// owner) → the remote's `PUT /api/hosted/artifacts/<raid>/comments`.
/// Turning comments on (`on` not `false`) widens what the public copy's
/// readers may do: confirmed as `put_share` is.
pub async fn put_comments(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    by: Confirming,
    p: Result<Path<(String, String)>, PathRejection>,
    sent: Bytes,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let change: Value = serde_json::from_slice(&sent)
        .map_err(|_| ApiError::bad_request("invalid_json", "the body is not JSON"))?;
    if change["on"] != false {
        widen(&s, &d, &by, &p, "comments", &change).await?;
    }
    put_sharing(&s, &d, p, "comments", &sent).await
}

/// The browser confirmation of widening the public copy `p` names to
/// `to` with `change` ([`Decision::Widen`]); nothing for a path that does
/// not parse, which `put_sharing` refuses.
async fn widen(
    s: &AppState,
    d: &DeniedToGrants,
    by: &Confirming,
    p: &Result<Path<(String, String)>, PathRejection>,
    to: &str,
    change: &Value,
) -> Result<(), ApiError> {
    let Ok(Path((rid, raid))) = p else {
        return Ok(());
    };
    let decision = Decision::Widen {
        artifact_id: raid.clone(),
        to: to.into(),
        remote_id: Some(rid.clone()),
        body: Some(change.clone()),
    };
    require_confirmation(s, &d.caller(s).await?, by, decision).await?;
    Ok(())
}

/// `DELETE /api/remotes/<rid>/artifacts/<raid>/sharing/shares/<sid>` (the
/// owner) → the remote's `DELETE /api/hosted/artifacts/<raid>/shares/<sid>`.
pub async fn delete_share(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid, sid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    let sid = share_id(&sid)?;
    Ok(forward(
        &remote,
        token,
        Method::DELETE,
        &sharing_path(&raid, &["shares", &sid]),
        &[],
        Vec::new(),
        None,
        public::MAX_ANSWER_BYTES,
        answer_too_large,
    )
    .await)
}

/// `GET /api/remotes/<rid>/apps` (the owner) → the remote's `GET
/// /api/hosted/apps`: the apps the signed-in account owns or holds a share
/// on.
pub async fn apps(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    rid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (remote, token) = owners_remote(&s, &d, path(rid)?).await?;
    Ok(get_json(&remote, token, &["api", "hosted", "apps"]).await)
}

/// `DELETE /api/remotes/<rid>/artifacts/<raid>` (the owner; Unpublish) →
/// `{deleted, unlinked}`: deletes public copy `raid` there, then ends every
/// local app's link to it (`unlinked`, their IDs). A copy the remote does
/// not have (404 there) is unlinked as well, `deleted` false. The remote's
/// other refusals (403 for one this account may not delete) come back as
/// it made them, and nothing is unlinked.
pub async fn unpublish(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    // A deletion there is a change the person makes, in a session.
    let mut c = public::Client::new(&remote.origin, Some(token));
    let f = clax_rt::sendable(async {
        c.by_hand().await?;
        let f = c
            .forward(
                Method::DELETE,
                &["api", "artifacts", &raid],
                &[],
                Vec::new(),
                None,
                public::MAX_ANSWER_BYTES,
            )
            .await;
        c.end_session().await;
        f
    })
    .await
    .map_err(ApiError::from)?;
    let deleted = match f.status {
        200..=299 => true,
        404 => false,
        _ => return Ok(answer(&remote.origin, f)),
    };
    let (r2, t2) = (remote.id.clone(), raid.clone());
    let unlinked = s
        .store_call(move |st| {
            let mut out = Vec::new();
            for l in st.all_remote_links()? {
                if l.remote_id == r2 && l.remote_artifact_id == t2 {
                    st.unlink_remote(&l.artifact_id, &r2)?;
                    out.push(l.artifact_id);
                }
            }
            Ok(out)
        })
        .await?;
    s.linked.links_changed();
    Ok(Json(json!({"deleted": deleted, "unlinked": unlinked})).into_response())
}

#[derive(Deserialize)]
pub struct CommentBody {
    /// The comment's text.
    body: String,
}

/// `POST /api/remotes/<rid>/artifacts/<raid>/comments {body}` (the owner)
/// → a new thread on the whole of public copy `raid`'s current version,
/// written in a by-hand session of this core's token: the remote's
/// `GET /api/artifacts/<raid>/head` for the version, then its
/// `POST /api/artifacts/<raid>/threads`. The remote's answer passes
/// through (201 with the thread; 403 `comments_off` while the copy takes
/// no comments; 403 for an account that may only view it). 400
/// `invalid_args` for an empty body.
pub async fn comment(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<CommentBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    super::remotes::local_only(&s)?;
    d.refuse_agents()?;
    who.require_owner("reaches a public Clax")?;
    let (rid, raid) = path(p)?;
    let (remote, token) = owners_remote(&s, &d, rid).await?;
    let raid = remote_artifact(&raid)?;
    let text = body(req)?.body;
    if text.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_args",
            "a comment has a body",
        ));
    }
    let mut c = public::Client::new(&remote.origin, Some(token.clone()));
    let head = clax_rt::sendable(c.forward(
        Method::GET,
        &["api", "artifacts", &raid, "head"],
        &[],
        Vec::new(),
        None,
        public::MAX_ANSWER_BYTES,
    ))
    .await
    .map_err(ApiError::from)?;
    if !(200..300).contains(&head.status) {
        return Ok(answer_or_too_old(&remote, token, head).await);
    }
    let version = serde_json::from_slice::<Value>(&head.body)
        .ok()
        .and_then(|h| h["version"].as_u64())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "remote_refused",
                "the public Clax's head names no version",
            )
        })?;
    let form = reqwest::multipart::Form::new()
        .text("anchor", json!({"kind": "artifact"}).to_string())
        .text("body", text)
        .text("version", version.to_string());
    // A comment there is one the person writes, in a session.
    let f = clax_rt::sendable(async {
        c.by_hand().await?;
        let f = c
            .forward_form(
                &["api", "artifacts", &raid, "threads"],
                form,
                public::MAX_ANSWER_BYTES,
            )
            .await;
        c.end_session().await;
        f
    })
    .await
    .map_err(ApiError::from)?;
    Ok(answer(&remote.origin, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_path_names_files_inside_the_version_alone() {
        assert_eq!(
            file_segments("js/a b.js").unwrap(),
            vec!["js".to_string(), "a b.js".to_string()]
        );
        for bad in [
            "", "/a", "a/", "a//b", ".", "..", "a/../b", "a/./b", "a\\..\\b",
        ] {
            assert!(file_segments(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ids_in_a_path_are_ids() {
        assert!(remote_artifact("../x").is_err());
        assert!(remote_artifact("a0000000000z/x").is_err());
        assert!(share_id("s_01ARZ3NDEK-x").is_ok());
        for bad in ["", "..", "a/b", "a%2Fb", "a b"] {
            assert!(share_id(bad).is_err(), "{bad}");
        }
    }
}

//! Apps published from the Clax app (spec 2026-10-21-app-push-pull §5.3,
//! §5.5, §5.7, §5.9): the bundle each such version carries, the head the
//! app checks for updates, and the installs it counts.
//!
//! - `PUT /api/artifacts/<aid>/versions/<n>/bundle` stores a version's
//!   bundle, unread: the remote checks no zip and no signature, only the
//!   size, the version's `content_sha256` and the lineage
//!   ([`clax_store::Store::put_app_bundle_async`]).
//! - `GET` the same path hands the bytes back to anyone who may read the
//!   artifact.
//! - `GET /api/artifacts/<aid>/head` says the newest version and the
//!   newest bundle.
//! - `POST /api/artifacts/<aid>/installs` counts a device's install.
//!
//! [`stamp_app_fields`] adds `comments`, `forks` and `app` to an artifact's
//! JSON, for `GET /api/artifacts/<id>` and the shell's bootstrap.

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::model::Artifact;
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::Agent;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::app_bundles::{AppBundle, PersonCard};
use serde::Deserialize;
use serde_json::{Value, json};

use super::artifacts::path;

/// A bundle's media type, in both directions.
pub const BUNDLE_MIME: &str = "application/vnd.empathic.clax+zip";

/// `path` at the hosted Clax's public origin; as it is on a local daemon.
fn absolute(s: &AppState, path: &str) -> String {
    match &s.hosted {
        Some(h) => h.url(path),
        None => path.to_string(),
    }
}

/// The person a version or bundle names as its publisher, as a store
/// person: `None` (the owner) for none or a marker such as `*owner`.
pub(crate) fn person_of(p: Option<&str>) -> Option<&str> {
    p.filter(|p| !p.starts_with('*'))
}

/// A publisher's `{name, login, public_id}`: `public_id` is the person's
/// public ID (the owner's own for the owner), which a signed-in device's
/// remote names as its own (`person_public_id`); null for an owner with no
/// viewer. Names and logins are the person's to choose; only the public
/// ID tells two people apart.
fn card(st: &Store, person: Option<&str>) -> clax_core::Result<Value> {
    let PersonCard {
        name,
        login,
        public_id,
    } = st.person_card(person_of(person))?;
    Ok(json!({"name": name, "login": login, "public_id": public_id}))
}

/// Who publishes a bundle for `caller`, as [`AppBundle::person`] names
/// them: a person's agent's person, a hosted member's own public ID, and
/// `None` for the owner and the owner's agents; `Err` for anyone else (a
/// guest, a viewer of a local daemon's), who publishes nothing.
fn bundle_publisher<N: clax_http::agent::Need>(a: &Agent<N>) -> Result<Option<String>, ApiError> {
    match &a.caller {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => Ok(None),
        Caller::PersonAgent { person, .. } => Ok(person.clone()),
        Caller::Person { viewer } if a.member().is_some() => Ok(Some(viewer.public_id.clone())),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        _ => Err(ApiError::forbidden(
            "not_permitted",
            "a bundle is published by the owner, an agent, or a signed-in member who may edit the artifact",
        )),
    }
}

/// A required `clax-bundle-*` header's text; 400 `invalid_args` without it.
fn bundle_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("invalid_args", format!("the {name} header is required"))
        })
}

/// Whether `s` is a lineage ID as the app makes one: `l_` and 26
/// lowercase base32 characters (`a`–`z`, `2`–`7`), 16 random bytes.
fn is_lineage(s: &str) -> bool {
    s.strip_prefix("l_").is_some_and(|rest| {
        rest.len() == 26
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
    })
}

/// 413 `bundle_too_large` for the store's refusal of that code; every
/// other error as it is.
fn too_large(e: ApiError) -> ApiError {
    if e.code == "bundle_too_large" {
        ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "bundle_too_large", e.message)
    } else {
        e
    }
}

fn bundle_view(b: &AppBundle) -> Value {
    json!({
        "artifact_id": b.artifact_id,
        "version": b.version,
        "bundle_version": b.bundle_version,
        "sha256": format!("sha256:{}", b.sha256),
        "size": b.size,
    })
}

/// `PUT /api/artifacts/<aid>/versions/<n>/bundle` (needs `clax-artifacts
/// update`; the owner, an agent, or a hosted member who may edit it): the
/// raw body, of type `application/vnd.empathic.clax+zip`, is the bundle
/// version `n` carries; the headers `clax-bundle-lineage`,
/// `clax-bundle-version` (the release counter) and
/// `clax-bundle-content-sha256` (the hash of the files it holds) describe
/// it; the lineage is `l_` and 26 lowercase base32 characters, as the app
/// makes it. → 201 `{artifact_id, version, bundle_version, sha256,
/// size}`, with `sha256` of the bytes as `sha256:<hex>`; the same bytes again answer
/// the same. 400 `invalid_args` for a missing or malformed header or
/// another content type; 400 `bundle_mismatch`, `bundle_lineage` or
/// `bundle_version_order` from the store; 404 for no such artifact or
/// version; 413 `bundle_too_large` over 32 MiB, or past the version's size
/// limit with its files.
pub async fn put_bundle(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::BundlePut>,
    audit: AuditCtx,
    headers: HeaderMap,
    params: Result<Path<(String, u32)>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let (aid, n) = path(params)?;
    let id = parse_id(&aid)?;
    let person = bundle_publisher(&a)?;
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    if mime != Some(BUNDLE_MIME) {
        return Err(ApiError::bad_request(
            "invalid_args",
            format!("a bundle's content type is {BUNDLE_MIME}"),
        ));
    }
    let lineage = bundle_header(&headers, "clax-bundle-lineage")?.to_string();
    if !is_lineage(&lineage) {
        return Err(ApiError::bad_request(
            "invalid_args",
            "clax-bundle-lineage is l_ and 26 lowercase base32 characters",
        ));
    }
    let bundle_version: u64 = bundle_header(&headers, "clax-bundle-version")?
        .parse()
        .map_err(|_| {
            ApiError::bad_request("invalid_args", "clax-bundle-version is a whole number")
        })?;
    let content = bundle_header(&headers, "clax-bundle-content-sha256")?.to_string();
    let bytes = body.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "bundle_too_large",
                format!(
                    "a bundle may be at most {} bytes",
                    clax_store::app_bundles::MAX_BUNDLE_BYTES
                ),
            )
        } else {
            ApiError::bad_request("invalid_args", e.body_text())
        }
    })?;
    let b = s
        .store_call_async(move |st| async move {
            st.put_app_bundle_async(
                &audit,
                &id,
                n,
                &lineage,
                bundle_version,
                &content,
                &bytes,
                person.as_deref(),
            )
            .await
        })
        .await
        .map_err(too_large)?;
    Ok((StatusCode::CREATED, Json(bundle_view(&b))).into_response())
}

/// A file name for `title`'s bundle: its letters and digits, lowercased,
/// runs of anything else as one `-`, at most 60 characters; `app` for a
/// title with none.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 60 {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "app".to_string()
    } else {
        out.to_string()
    }
}

/// `GET /api/artifacts/<aid>/versions/<n>/bundle` (anyone who may read the
/// artifact, a guest and a visitor through its link too): the bytes of the
/// bundle version `n` carries, as `application/vnd.empathic.clax+zip`,
/// with `content-disposition: attachment; filename="<slug>.clax"` and
/// `clax-bundle-version`, `clax-bundle-lineage`. 404 when it carries none.
/// A person's agent that may not see every file of the version, or the
/// bundle's uploader, is refused 403 `withheld`: the bundle holds every
/// file, in bytes the uploader chose
/// ([`clax_http::views::thread::Vis::filter_bundle`]).
pub async fn get_bundle(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::BundleGet>,
    params: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n) = path(params)?;
    let id = parse_id(&aid)?;
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let (b, title, bytes) = s
        .store_call_async(move |st| async move {
            let art = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = st.get_version(&id, n)?.ok_or(CoreError::NotFound)?;
            let b = st.app_bundle(&id, n)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(&st)?;
            if vis.filter_bundle(&v, b.person.as_deref()) {
                return Err(CoreError::invalid("withheld", "withheld"));
            }
            let bytes = st.read_app_bundle_async(&b).await?;
            Ok((b, art.title, bytes))
        })
        .await
        .map_err(|e| match e.code {
            "withheld" => ApiError::forbidden(
                "withheld",
                "this version holds files withheld until your person approves their authors",
            ),
            _ => e,
        })?;
    let disposition = format!("attachment; filename=\"{}.clax\"", slug(&title));
    let mut res = bytes.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(BUNDLE_MIME));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).expect("a slug is a header value"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    if let Ok(v) = HeaderValue::from_str(&b.lineage) {
        h.insert("clax-bundle-lineage", v);
    }
    h.insert("clax-bundle-version", HeaderValue::from(b.bundle_version));
    Ok(res)
}

/// `GET /api/artifacts/<aid>/head` (anyone who may read the artifact, a
/// guest and a visitor through its link too) → `{artifact_id, url, title,
/// version, content_sha256, published_by: {name, login, public_id}, owner:
/// {name, login, public_id}, note, comments, capabilities, bundle}`: the
/// artifact's current version and its files' hash, who published it and
/// its change note (null when it has none), who made the artifact (each
/// card's `public_id` null for an owner with none), its Comments switch, its
/// capability declaration, and `bundle`, null when no version carries one, else the
/// newest version that does: `{version, lineage, bundle_version,
/// content_sha256, url, published_by}`, `url` its `GET` route. Filtered for
/// an agent channel's reader: a title it may not see is null, a version
/// with a file it may not see has a null `content_sha256`, and so has a
/// `bundle` whose bytes `GET` would refuse it
/// ([`clax_http::views::thread::Vis::filter_bundle`]).
pub async fn head(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ArtifactHead>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let url = absolute(&s, &format!("/a/{}", id.as_str()));
    let app = s.clone();
    let v = s
        .store_call(move |st| {
            let art = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let ver = st
                .get_version(&id, art.current_version)?
                .ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            let mut artifact = json!(art);
            vis.filter_artifact(st, &art, &[], &mut artifact)?;
            let mut version = json!(ver);
            vis.filter_version(&ver, &mut version);
            let bundle = match st.latest_app_bundle(&id)? {
                Some(b) => {
                    let withheld = match st.get_version(&id, b.version)? {
                        Some(bv) => vis.filter_bundle(&bv, b.person.as_deref()),
                        None => vis.filters(),
                    };
                    let content = (!withheld).then_some(&b.content_sha256);
                    let url = absolute(
                        &app,
                        &format!(
                            "/api/artifacts/{}/versions/{}/bundle",
                            b.artifact_id, b.version
                        ),
                    );
                    json!({
                        "version": b.version,
                        "lineage": b.lineage,
                        "bundle_version": b.bundle_version,
                        "content_sha256": content,
                        "url": url,
                        "published_by": card(st, b.person.as_deref())?,
                    })
                }
                None => Value::Null,
            };
            Ok(json!({
                "artifact_id": art.id,
                "url": url,
                "title": artifact["title"],
                "version": ver.n,
                "content_sha256": version["content_sha256"],
                "published_by": card(st, ver.authors.person.as_deref())?,
                "note": version["note"],
                "owner": card(st, art.authors.creator.as_deref())?,
                "comments": st.comments_on(&id)?,
                "capabilities": artifact["capabilities"],
                "bundle": bundle,
            }))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallBody {
    /// The installing device's opaque ID for this app: 64 lowercase hex.
    install: String,
}

/// `POST /api/artifacts/<aid>/installs {install}` (anyone who may read the
/// artifact, without signing in; on a hosted Clax a request without
/// `Origin` is taken as from no session,
/// [`clax_http::hosted::anonymous_write`]): counts one install of the app
/// by the device whose opaque ID `install` (64 lowercase hex) is; the same
/// ID again counts once. → 204. On a hosted Clax each client address has
/// [`clax_http::hosted::Attempt::Install`]'s attempts (429
/// `rate_limited`). 400 `invalid_args` for another `install`, 404 for no
/// such artifact.
pub async fn record_install(
    State(s): State<AppState>,
    _a: Agent<clax_http::agent::InstallsRecord>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<InstallBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let id = parse_id(&path(aid)?)?;
    if let Some(h) = &s.hosted {
        h.attempt(clax_http::hosted::Attempt::Install, &headers, &extensions)?;
    }
    let b = super::artifacts::body(req)?;
    let ok = b.install.len() == 64
        && b.install
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    if !ok {
        return Err(ApiError::bad_request(
            "invalid_args",
            "install is 64 lowercase hex digits",
        ));
    }
    s.store_call(move |st| st.record_install(&id, &b.install))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Adds to `view`, artifact `a`'s JSON, `comments` (its Comments switch),
/// `forks` (how many live forks it has) and `app`: null when no version
/// carries a bundle, else `{version, bundle_version, installs,
/// published_by: {name, login, public_id}, owner: {name, login,
/// public_id}}` (`public_id` null for an owner with none), of its newest
/// bundle: `version` is the artifact version that carries it, and
/// `published_by` who published that version's bundle.
///
/// # Errors
/// The store's.
pub fn stamp_app_fields(st: &Store, a: &Artifact, view: &mut Value) -> clax_core::Result<()> {
    let id = ArtifactId::parse(&a.id)?;
    view["comments"] = json!(st.comments_on(&id)?);
    view["forks"] = json!(st.fork_count(&id)?);
    view["app"] = match st.latest_app_bundle(&id)? {
        Some(b) => json!({
            "version": b.version,
            "bundle_version": b.bundle_version,
            "installs": st.install_count(&id)?,
            "published_by": card(st, b.person.as_deref())?,
            "owner": card(st, a.authors.creator.as_deref())?,
        }),
        None => Value::Null,
    };
    Ok(())
}

/// Adds to `view`, artifact `a`'s JSON, `capabilities_in_force`: the
/// declaration in force ([`Store::capabilities_in_force`]), what the shell
/// hands the page. For a caller who may approve its declaration
/// (`may_approve`: one who administers it, never an agent) it adds
/// `may_approve: true` and `pending_capabilities`, what the current
/// version declares that waits for approval (`null` when nothing does);
/// for anyone else both are absent. Where `view` withholds the declaration
/// from its reader (`capabilities` null, as the reader's visibility filter
/// leaves it), `capabilities_in_force` is null and nothing more is said.
///
/// # Errors
/// The store's.
pub fn stamp_capabilities(
    st: &Store,
    a: &Artifact,
    may_approve: bool,
    view: &mut Value,
) -> clax_core::Result<()> {
    if view.get("capabilities").is_some_and(Value::is_null) {
        view["capabilities_in_force"] = Value::Null;
        return Ok(());
    }
    let id = ArtifactId::parse(&a.id)?;
    view["capabilities_in_force"] = st.capabilities_in_force(&id)?;
    if may_approve {
        view["may_approve"] = json!(true);
        view["pending_capabilities"] = json!(st.pending_capabilities(&id)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_lineage, slug};

    #[test]
    fn a_lineage_is_l_and_26_base32_characters() {
        assert!(is_lineage("l_abcdefghijklmnopqrstuvwxyz"));
        assert!(is_lineage("l_234567abcdefghijklmnopqrst"));
        assert!(!is_lineage("l_abcdefghijklmnopqrstuvwxy"));
        assert!(!is_lineage("l_abcdefghijklmnopqrstuvwxyza"));
        assert!(!is_lineage("l_abcdefghijklmnopqrstuvwxy1"));
        assert!(!is_lineage("l_ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
        assert!(!is_lineage("x_abcdefghijklmnopqrstuvwxyz"));
        assert!(!is_lineage(""));
    }

    #[test]
    fn a_slug_keeps_letters_and_digits_and_joins_the_rest() {
        assert_eq!(slug("Tip Calculator!"), "tip-calculator");
        assert_eq!(slug("  --Ünïcode & 42 "), "n-code-42");
        assert_eq!(slug("!!!"), "app");
        assert_eq!(slug(&"a".repeat(80)).len(), 60);
    }
}

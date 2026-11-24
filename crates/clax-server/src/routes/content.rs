//! Serves published page content: every HTML page (the index and supporting
//! `text/html` files) wrapped with the bridge, other supporting files as stored.

use crate::routes::artifacts::path;
use crate::shell::bridge_version;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::model::CONTRACT_VERSION;
use clax_core::model::FileMeta;
use clax_core::publish::INDEX;
use clax_core::wrap::wrap_page_framed_by;
use clax_core::{ArtifactId, CoreError};
use clax_http::content_policy::ContentPolicySet;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::http_cache;
use clax_http::state::AppState;
use clax_store::site_sessions::ContentSecurityPolicy;
use std::sync::Arc;

/// The framing part of a content response's policy on an artifact's site
/// host: framed only by the artifact's own pages (`'self'`) and the shell
/// (`clax_http::site_access::frame_ancestors`): a hosted daemon's at its
/// public origin, a local daemon's at its loopback names on the same port,
/// and an embedding app's shell.
fn framing(s: &AppState, req: &HeaderMap) -> String {
    let host = req
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    clax_http::site_access::frame_ancestors(s, host)
}

/// The `Content-Security-Policy` of a content response on an artifact's
/// site host: [`framing`], and while the artifact's
/// `content_security_policy` is `self` ([`super::content_security_policy`])
/// the lockdown that keeps the page to its own site
/// (`clax_http::content_policy::lockdown`).
///
/// The policy is a separate one: a page's own `<meta>` policy still applies
/// in full beside it, and cannot set `frame-ancestors` itself; on an
/// `unrestricted` site it is the only source restriction there is.
pub(crate) fn content_policy(
    s: &AppState,
    req: &HeaderMap,
    csp: ContentSecurityPolicy,
) -> HeaderValue {
    HeaderValue::from_str(&policy_of(&framing(s, req), csp))
        .unwrap_or_else(|_| HeaderValue::from_static("frame-ancestors 'self'; default-src 'none'"))
}

/// A content response's policy after `frame` (its `frame-ancestors`): the
/// framing alone for an `unrestricted` artifact, else the lockdown after
/// it.
fn policy_of(frame: &str, csp: ContentSecurityPolicy) -> String {
    match csp {
        ContentSecurityPolicy::Unrestricted => frame.to_string(),
        ContentSecurityPolicy::SelfOnly => clax_http::content_policy::lockdown(frame),
    }
}

/// The host and port in the daemon's browser base URL.
fn own_host(s: &AppState) -> String {
    s.browser_base
        .strip_prefix("http://")
        .unwrap_or(&s.browser_base)
        .trim_end_matches('/')
        .to_string()
}

/// The second policy on a live page's content (spec
/// 2026-10-05-chrome-overlay-design §8.4): of scripts, only Clax's own under
/// `/_clax/` on the request's host run, whatever the snapshot holds; no
/// plugins, base URL, form posts, frames, connections or workers. A `Host`
/// that is not a plain host and port (letters, digits, `.`, `-`, `:`, `[`,
/// `]`) is not trusted: `fallback` (the daemon's own host) is used instead.
/// `scheme` is the site's (`https` when the public origin is).
fn snapshot_policy(scheme: &str, req: &HeaderMap, fallback: &str) -> HeaderValue {
    let plain = |h: &str| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    };
    let host = req
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| plain(h))
        .unwrap_or(fallback);
    HeaderValue::from_str(&format!(
        "script-src {scheme}://{host}/_clax/; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'; worker-src 'none'"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("script-src 'none'"))
}

/// Sets the content policy on `res` ([`content_policy`], after the
/// artifact's `csp`); for a live page's content (`live`: the daemon's
/// host, the policy's fallback) only its framing part, with
/// [`snapshot_policy`] beside it, both applying, so a snapshot still loads
/// styles, images and fonts from its site, whatever `csp` says: a snapshot
/// is Clax's rendering of another site. Marked [`ContentPolicySet`], so
/// the site layer adds no lockdown of its own.
fn framed_by_shell(
    s: &AppState,
    mut res: Response,
    req: &HeaderMap,
    live: Option<&str>,
    csp: ContentSecurityPolicy,
) -> Response {
    res.extensions_mut().insert(ContentPolicySet);
    let policy = match live {
        Some(_) => HeaderValue::from_str(&framing(s, req))
            .unwrap_or_else(|_| HeaderValue::from_static("frame-ancestors 'self'")),
        None => content_policy(s, req, csp),
    };
    res.headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, policy);
    if let Some(fallback) = live {
        let scheme = if clax_http::site_access::secure(s) {
            "https"
        } else {
            "http"
        };
        res.headers_mut().append(
            header::CONTENT_SECURITY_POLICY,
            snapshot_policy(scheme, req, fallback),
        );
    }
    res
}

/// Whether a stored content type is `text/html` (parameters ignored).
fn is_html(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("text/html")
}

/// A file of a version as it is served.
enum Served {
    /// An HTML page wrapped with the bridge ([`wrap_page_framed_by`]).
    Page(Arc<String>),
    /// Any other file, or an HTML file that is not UTF-8: served as stored.
    Raw(Vec<u8>, FileMeta),
}

/// `file` of version `n`, in one store call, its bytes read from the store's
/// content store. It is wrapped when `always_wrap` is set or its stored
/// content type is `text/html`, through the wrap cache, which also remembers
/// a file that is not UTF-8 (served raw); a page the cache holds is not
/// read again. `NotFound` when the artifact, the version, or the file is
/// gone. Beside it, the artifact's `content_security_policy`, and whether
/// its page views are counted: `counting` (a hosted Clax's own page, not a
/// live page's snapshot nor the Clax app's) and its analytics setting on
/// (`clax_store::Store::analytics_on`, never for a draft).
async fn lookup(
    s: &AppState,
    id: ArtifactId,
    n: u32,
    file: String,
    always_wrap: bool,
    counting: bool,
) -> Result<(Served, ContentSecurityPolicy, bool), ApiError> {
    let cache = s.wrap_cache.clone();
    let bridge = bridge_version(&*s.web);
    // A page wrapped concurrently with a debug rebuild may keep the old
    // `?v=`; a debug build serves every bridge URL revalidated, current bytes.
    cache.follow_bridge(&bridge);
    // The bridge talks to the shell at the public origin alone (a local
    // daemon's bridge takes its loopback names at that port), or in the
    // Clax app at the app's shell origin.
    let shell = clax_http::site_access::bridge_shell(s);
    // In the Clax app a page's site API writes go by its shell.
    let relay = s.embed.is_some();
    s.store_call_async(move |st| async move {
        // A deleted artifact's versions stay behind its row, and its files
        // until its content is removed: neither is served once it is gone.
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        let csp = st.content_security_policy(&id)?;
        let meta = st.file_meta(&id, n, &file)?.ok_or(CoreError::NotFound)?;
        if !always_wrap && !is_html(&meta.content_type) {
            return Ok((
                Served::Raw(read_bytes(&st, &id, n, &file).await?, meta),
                csp,
                false,
            ));
        }
        let count = counting && st.analytics_on(&id)?;
        // Read ahead of the cache, which wraps synchronously; a page it
        // holds is not read. One evicted between the check and the wrap is
        // read on the next pass.
        let mut bytes = None;
        loop {
            if bytes.is_none() && !cache.contains(id.as_str(), n, &file) {
                bytes = Some(read_bytes(&st, &id, n, &file).await?);
            }
            let mut unwrapped = None;
            let wrapped = cache.get_or_wrap(id.as_str(), n, &file, || {
                let Some(bytes) = bytes.take() else {
                    return Err(std::io::Error::other(EVICTED));
                };
                Ok(match String::from_utf8(bytes) {
                    Ok(page) => Some(wrap_page_framed_by(
                        &page,
                        id.as_str(),
                        n,
                        CONTRACT_VERSION,
                        &file,
                        &bridge,
                        Some(&shell),
                        relay,
                    )),
                    Err(e) => {
                        unwrapped = Some(e.into_bytes());
                        None
                    }
                })
            });
            return match wrapped {
                Ok(Some(html)) => Ok((Served::Page(html), csp, count)),
                Ok(None) => Ok((
                    Served::Raw(
                        match unwrapped.or(bytes) {
                            Some(bytes) => bytes,
                            None => read_bytes(&st, &id, n, &file).await?,
                        },
                        meta,
                    ),
                    csp,
                    false,
                )),
                Err(e) if e.to_string() == EVICTED => continue,
                Err(e) => Err(CoreError::Io(e)),
            };
        }
    })
    .await
}

/// `html` (a page as [`lookup`] wrapped it, its bridge tag naming
/// [`clax_http::site_access::bridge_shell`]) as a trusted-host frame serves
/// it (`[serve] trusted_host_frames`, a request on a `[serve] trusted_hosts`
/// name, which reaches the site routes only through
/// [`clax_http::host::FRAME_PATH`]): the tag names the shell on this host,
/// on `https` and `http` (`data-origin`, which the bridge reads as a list),
/// and says `data-api="frame"`, so the bridge sends every call of its
/// site's API through the shell (`site.fetch`), as its opaque origin keeps
/// no site cookie. Anything else: `html` as it is.
fn trusted_frame_page(s: &AppState, req: &HeaderMap, html: Arc<String>) -> Arc<String> {
    let host = req
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !s.trusted_host_frames || !clax_http::auth::trusted_host(host, &s.trusted_hosts) {
        return html;
    }
    // The host is a trusted name, which `trusted_host_ok` keeps to letters,
    // digits, `.` and `-`, with a numeric port: nothing to escape.
    let from = format!(
        " data-origin=\"{}\"",
        clax_http::site_access::bridge_shell(s)
    );
    let to = format!(" data-origin=\"https://{host} http://{host}\" data-api=\"frame\"");
    Arc::new(html.replacen(&from, &to, 1))
}

/// The error a wrap answers when the page it expected cached has gone.
const EVICTED: &str = "evicted while being read";

/// The bytes of `file` in version `n` of `id`, from the store's content
/// store; `NotFound` when they are missing.
async fn read_bytes(
    st: &clax_store::Store,
    id: &ArtifactId,
    n: u32,
    file: &str,
) -> clax_core::Result<Vec<u8>> {
    st.content()
        .read_file(id, n, file)
        .await?
        .ok_or(CoreError::NotFound)
}

/// The `ETag` source of a response that carries an `unrestricted` site's
/// policy (`unrestricted`): `source` with the policy named after it, so a
/// copy a browser keeps from while the site was `unrestricted` never
/// matches the lockdown's tag once the site is `self` again (a `304`
/// would keep the stored policy: Chromium does not update
/// `Content-Security-Policy` from one). Any other response's source is
/// `source` unchanged.
fn tagged_by_policy(source: &[u8], unrestricted: bool) -> Vec<u8> {
    let mut v = source.to_vec();
    if unrestricted {
        v.extend_from_slice(b"\0content_security_policy=unrestricted");
    }
    v
}

/// File `rel` of version `n` of `id`. `index.html`, and any other
/// `text/html` file in UTF-8, is a page: wrapped with the bridge
/// (`data-file` naming its path), revalidated by the browser on every
/// load, cached by the daemon per file. Any other file is served as
/// stored: immutable, unless `current` (the version a site's root serves,
/// whose files change with its next version: then revalidated, with an
/// `ETag` of its artifact, version, path and size); an HTML file that is
/// not UTF-8 is revalidated like every page. A file served as stored
/// answers a byte range ([`http_cache::stored_file`]); a page does not.
///
/// While the artifact's `content_security_policy` is `unrestricted` and
/// the response carries that policy (not a live page's snapshot), every
/// file is revalidated, never immutable, and its `ETag` names the policy
/// ([`tagged_by_policy`]): returning the site to `self` then holds from
/// the browser's next use of any file it keeps, which is answered `200`
/// with the lockdown. A `self` site's files are cached as above; one a
/// browser keeps immutable from while `self` keeps the lockdown until it
/// expires, after a change to `unrestricted` too.
///
/// A page whose views are counted (a hosted Clax's, its analytics setting
/// on: [`lookup`]) is served with its bridge tag marked
/// ([`clax_core::wrap::with_count`]) after the wrap cache, whose entries
/// stay the same for both; its `ETag` is of the bytes sent, mark included,
/// so a page a browser revalidates after the setting changes is answered
/// `200` with the right tag, never `304`.
pub(crate) async fn serve_version(
    s: &AppState,
    id: ArtifactId,
    n: u32,
    rel: String,
    current: bool,
    req: &HeaderMap,
) -> Result<Response, ApiError> {
    let tag_source = format!("{}/{n}/{rel}", id.as_str());
    let own = own_host(s);
    let live = s.live_ids.contains(id.as_str()).then_some(own.as_str());
    let index = rel == INDEX;
    let counting = s.hosted.is_some() && s.embed.is_none() && live.is_none();
    let (served, csp, count) = lookup(s, id, n, rel, index, counting).await?;
    let served = match served {
        Served::Page(html) if count => Served::Page(Arc::new(clax_core::wrap::with_count(&html))),
        other => other,
    };
    // Whether the response carries an `unrestricted` site's policy, not the
    // lockdown.
    let unrestricted = csp == ContentSecurityPolicy::Unrestricted && live.is_none();
    let (bytes, meta) = match served {
        Served::Page(html) if unrestricted => {
            let etag = http_cache::etag_of(&tagged_by_policy(html.as_bytes(), true));
            let res = http_cache::tagged_as(req, etag, http_cache::REVALIDATE, || {
                axum::response::Html(html.to_string()).into_response()
            });
            return Ok(framed_by_shell(s, res, req, live, csp));
        }
        Served::Page(html) => {
            let html = trusted_frame_page(s, req, html);
            return Ok(framed_by_shell(
                s,
                http_cache::html(req, &html),
                req,
                live,
                csp,
            ));
        }
        Served::Raw(..) if index => {
            return Err(ApiError::from(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "index.html is not UTF-8",
            ))));
        }
        Served::Raw(bytes, meta) => (bytes, meta),
    };
    // A version's files never change, so its artifact, version, path and
    // size identify its bytes: the tag of a file that is revalidated. One
    // served immutable has none. While the site is `unrestricted`, every
    // file is revalidated and its tag names the policy ([`tagged_by_policy`]).
    let (etag, cache_control) = if is_html(&meta.content_type) || current || unrestricted {
        let tag = http_cache::etag_of(&tagged_by_policy(
            format!("{tag_source}\0{}", meta.size).as_bytes(),
            unrestricted,
        ));
        (Some(tag), http_cache::REVALIDATE)
    } else {
        (None, http_cache::IMMUTABLE)
    };
    let ct = meta.content_type;
    let res = http_cache::stored_file(req, bytes, etag, cache_control, move |status, body| {
        (
            status,
            [
                (header::CONTENT_TYPE, ct),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            body,
        )
            .into_response()
    });
    Ok(framed_by_shell(s, res, req, live, csp))
}

/// `GET /api/artifacts/<id>/versions/<n>/files/<path>`: a file's stored bytes,
/// `index.html` included and never wrapped. Served with `Content-Security-Policy:
/// sandbox` because it shares the API's origin. A file an agent channel's
/// reader may not see answers 403 `withheld` with its authors; while the
/// version withholds any file, so does a path it lacks
/// ([`clax_http::views::thread::Vis::filter_file_content`]).
pub async fn raw_file(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::FileContent>,
    p: Result<Path<(String, u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (aid, n, rel) = path(p)?;
    let id = parse_id(&aid)?;
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let found = s
        .store_call_async(move |st| async move {
            let st = &*st;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            if vis.filters() {
                let v = st.get_version(&id, n)?.ok_or(CoreError::NotFound)?;
                if let Some(authors) = vis.filter_file_content(st, &v, &rel)? {
                    return Ok(Err(authors));
                }
            }
            Ok(Ok(st
                .read_file_async(&id, n, &rel)
                .await?
                .ok_or(CoreError::NotFound)?))
        })
        .await?;
    let (bytes, meta) = match found {
        Ok(f) => f,
        Err(authors) => {
            let mut e = ApiError::forbidden(
                "withheld",
                "this file is withheld until your person approves its authors",
            );
            e.extra.insert("authors".into(), serde_json::json!(authors));
            return Err(e);
        }
    };
    let body = Body::from(bytes);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, meta.content_type.as_str()),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn an_unrestricted_sites_content_policy_is_its_framing_alone() {
        let frame = "frame-ancestors 'self' http://localhost:7480 http://127.0.0.1:7480";
        assert_eq!(policy_of(frame, ContentSecurityPolicy::Unrestricted), frame);
        let to_self = policy_of(frame, ContentSecurityPolicy::SelfOnly);
        assert_eq!(to_self, clax_http::content_policy::lockdown(frame));
        assert!(to_self.starts_with(frame), "{to_self}");
        assert!(to_self.contains("connect-src 'self';"), "{to_self}");
    }

    #[test]
    fn the_snapshot_policy_names_a_plain_host_and_never_a_crafted_one() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7480"));
        let p = snapshot_policy("http", &h, "localhost:7480");
        assert!(
            p.to_str()
                .unwrap()
                .starts_with("script-src http://127.0.0.1:7480/_clax/;")
        );
        h.insert(
            header::HOST,
            HeaderValue::from_static("a 'strict-dynamic' b"),
        );
        let p = snapshot_policy("http", &h, "localhost:7480");
        let p = p.to_str().unwrap();
        assert!(
            p.starts_with("script-src http://localhost:7480/_clax/;"),
            "{p}"
        );
        assert!(!p.contains("strict-dynamic"));
        h.insert(header::HOST, HeaderValue::from_static("x;script-src *"));
        assert!(
            !snapshot_policy("http", &h, "localhost:7480")
                .to_str()
                .unwrap()
                .contains('*')
        );
        assert!(
            snapshot_policy("http", &HeaderMap::new(), "localhost:7480")
                .to_str()
                .unwrap()
                .contains("http://localhost:7480/_clax/")
        );
    }
}

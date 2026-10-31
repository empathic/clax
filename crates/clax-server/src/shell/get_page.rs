//! Getting an app into the Clax app (spec 2026-10-21-app-push-pull §5.5).
//!
//! - `GET /a/<id>/get` ([`page`]): the page Get in Clax links to, for
//!   anyone who may read the artifact. A self-contained HTML page with no
//!   script of its own: the app's title and byline, a QR code of the page's own link
//!   for a phone (hidden on a touch screen of phone width, where the page
//!   is already on the phone), **Open in Clax** as the
//!   `clax:get?url=<artifact URL>` form the app takes on every platform,
//!   **Download .clax**, and the stores' pages when the instance names
//!   them ([`ClaxApp`]). A daemon that is not hosted offers Download .clax
//!   alone. A page whose artifact carries no bundle answers 404 "This
//!   page isn’t an app yet.", unless its current version has a file
//!   `get` or `get/index.html`, which this address served before it was
//!   reserved: then 307 to that file's page (`/a/<id>/v/<n>/get`, or
//!   `/a/<id>/get/index.html`). A local owner's browser, whose page loads
//!   carry its cookie without its proof, first gets a bare page whose one
//!   script loads that page with the proof ([`bare_page`]).
//! - On a hosted Clax, [`refusals`] gives the hosted gate's refusals of
//!   that path (a restricted page, no such page, a malformed ID) as one
//!   page, 404 with the sign-in link, the same whichever it was.
//! - `GET /.well-known/apple-app-site-association` ([`apple_app_site_association`])
//!   and `GET /.well-known/assetlinks.json` ([`assetlinks`]): the app links
//!   that let iOS and Android open `/a/<id>/get` in the Clax app, served
//!   on the hosted public host only, and only when [`ClaxApp`] names the
//!   platform's app. The Mac app takes no universal links (it carries no
//!   associated-domains entitlement): on a Mac the link opens in the
//!   browser, whose Open in Clax (`clax:`) reaches the app.

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use clax_core::config::{APPLE_BUNDLE_ID, ClaxApp};
use clax_core::ids::ARTIFACT_ID_LEN;
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::app_bundles::PersonCard;
use serde_json::json;

use crate::routes::app_bundles::person_of;
use crate::routes::artifacts::path;

/// `s` with `&`, `<`, `>` and `"` escaped, for HTML text and quoted
/// attribute values.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// What an artifact offers the page.
enum Found {
    /// No live artifact of the ID.
    Missing,
    /// An artifact no version of which carries a bundle.
    NotAnApp,
    /// An artifact no version of which carries a bundle, whose current
    /// version has a file this address served before it was the Get in
    /// Clax page's: that file's shell path.
    File(String),
    App(Box<App>),
}

/// An app: its artifact and newest bundle.
struct App {
    title: String,
    icon: Option<String>,
    /// The artifact version that carries the newest bundle.
    version: u32,
    /// The newest bundle's release counter.
    bundle_version: u64,
    /// Who published the newest bundle.
    published_by: PersonCard,
    /// Who made the artifact.
    owner: PersonCard,
}

/// A person as a byline names them: `@login`, else their name.
fn who(c: &PersonCard) -> String {
    match &c.login {
        Some(l) => format!("@{l}"),
        None => c.name.clone(),
    }
}

/// "v<N> · by <owner>", or "v<N> by <editor> · by <owner>" when someone
/// other than the owner published the newest bundle.
fn byline(a: &App) -> String {
    if a.published_by == a.owner {
        format!("v{} · by {}", a.bundle_version, who(&a.owner))
    } else {
        format!(
            "v{} by {} · by {}",
            a.bundle_version,
            who(&a.published_by),
            who(&a.owner)
        )
    }
}

/// The origin the page's links name: the hosted public origin, else the
/// daemon's browser base.
fn origin(s: &AppState) -> &str {
    match &s.hosted {
        Some(h) => &h.origin,
        None => &s.browser_base,
    }
}

/// The QR code of `text` as an inline SVG element, one unit a module with
/// the standard quiet zone, dark on white whatever the theme.
fn qr_svg(text: &str) -> String {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return String::new();
    };
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .module_dimensions(1, 1)
        .build();
    // An element inside HTML: no XML declaration.
    let svg = svg.split_once("?>").map_or(svg.as_str(), |(_, rest)| rest);
    svg.replacen(
        "<svg ",
        "<svg class=\"qr\" role=\"img\" aria-label=\"QR code of this page’s link\" ",
        1,
    )
}

/// The page's styles. Its colours are the web shell's tokens
/// (`web/shell/src/theme.css`: `--bg`, `--fg`, `--muted`, `--card`,
/// `--border`, and `--hover` as the tile's), light and dark, so the step
/// from an app's page to this one keeps its background and ink. The page
/// runs no script, so it follows the system's theme, not one chosen with
/// the shell's theme switch (kept in the browser).
const STYLE: &str = ":root{color-scheme:light dark;--bg:#f7f7f5;--fg:#1c1b19;--muted:#5f5d58;--card:#ffffff;--border:#e6e5e1;--tile:#f2f2ef}\
@media (prefers-color-scheme:dark){:root{--bg:#141413;--fg:#ecebe7;--muted:#a9a69f;--card:#1c1c1a;--border:#2c2c29;--tile:#2a2a27}}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 ui-sans-serif,-apple-system,\"Segoe UI\",system-ui,sans-serif}\
main{max-width:480px;margin:0 auto;padding:48px 16px}\
section{background:var(--card);border:1px solid var(--border);border-radius:12px;padding:22px 24px}\
h1{margin:0 0 10px;font-size:22px;line-height:1.3;overflow-wrap:anywhere}p{margin:0 0 12px}a{color:var(--fg)}\
.app{display:flex;gap:14px;align-items:center;margin-bottom:20px}.app h1{margin:0;font-size:20px}\
.tile{flex:none;width:52px;height:52px;border-radius:12px;background:var(--tile);display:grid;place-items:center;font-size:22px;font-weight:600}\
.by{margin:2px 0 0;color:var(--muted);font-size:14px;overflow-wrap:anywhere}\
.scan{text-align:center;margin-bottom:20px}.scan p{margin:8px 0 0;color:var(--muted);font-size:13.5px}\
svg.qr{display:block;margin:0 auto;width:184px;height:184px;border-radius:8px;border:1px solid var(--border)}\
.actions{display:flex;flex-wrap:wrap;gap:10px}\
.btn{flex:1 1 180px;display:flex;justify-content:center;align-items:center;min-height:44px;padding:0 16px;box-sizing:border-box;border-radius:10px;border:1px solid var(--border);background:var(--card);color:var(--fg);font-weight:600;text-decoration:none}\
.btn.primary{background:var(--fg);border-color:var(--fg);color:var(--bg)}\
.stores,.more{margin:16px 0 0;font-size:14px;color:var(--muted)}\
@media (max-width:600px) and (pointer:coarse){.scan{display:none}}\
footer{margin-top:16px;text-align:center;font-size:12.5px;color:var(--muted)}footer a{color:inherit}";

/// The page's footer: the instance's Terms of Service and Privacy Policy
/// when it shows them, and Clax's source.
fn footer(s: &AppState) -> String {
    let legal = s
        .hosted
        .as_ref()
        .and_then(|h| h.legal.as_ref())
        .map_or(String::new(), |l| {
            format!(
                "<a href=\"{}\">Terms of Service</a> · <a href=\"{}\">Privacy Policy</a> · ",
                escape(l.terms_url()),
                escape(l.privacy_url())
            )
        });
    format!(
        "<footer>{legal}<a href=\"https://github.com/empathic/clax\" target=\"_blank\" rel=\"noopener\">Source on GitHub</a></footer>"
    )
}

/// A whole page titled `title` (escaped here) around `body` (HTML), with
/// `status`; not cached, framed by nothing, and running no script.
fn html_page(s: &AppState, status: StatusCode, title: &str, body: &str) -> Response {
    page_with(s, status, title, body, None)
}

/// [`html_page`], running `script` (JavaScript) when one is given, under a
/// nonce of its own, with `connect-src 'self'`.
fn page_with(
    s: &AppState,
    status: StatusCode,
    title: &str,
    body: &str,
    script: Option<&str>,
) -> Response {
    let nonce = script.map(|_| clax_core::secret::token("").expose().to_string());
    let tag = match (script, &nonce) {
        (Some(js), Some(n)) => format!("<script nonce=\"{n}\">{js}</script>"),
        _ => String::new(),
    };
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · Clax</title><style>{STYLE}</style></head><body><main>{body}{}</main>{tag}</body></html>",
        escape(title),
        footer(s)
    );
    let mut res = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    let h = res.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    // A local owner's browser gets the bare page without the proof and
    // the app's with it ([`page`]).
    h.insert(
        header::VARY,
        HeaderValue::from_static("Cookie, X-Clax-Owner-Proof"),
    );
    let csp = match &nonce {
        Some(n) => format!(
            "default-src 'none'; script-src 'nonce-{n}'; connect-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'"
        ),
        None => "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'".into(),
    };
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
}

/// 404 for a page that does not exist or that the caller may not read,
/// alike: on a hosted Clax, the page of every 404 of an artifact there,
/// with the hint ([`clax_http::error::not_found_hint`]) and a Sign in
/// button to the instance's public origin.
fn unavailable(s: &AppState) -> Response {
    if let Some(h) = &s.hosted {
        return html_page(
            s,
            StatusCode::NOT_FOUND,
            "Not found",
            &format!(
                "<section><h1>This page isn’t available</h1><p>{}</p><p class=\"actions\"><a class=\"btn primary\" href=\"{}/signin\">Sign in</a></p></section>",
                escape(&clax_http::error::not_found_hint(&h.origin)),
                escape(&h.origin)
            ),
        );
    }
    html_page(
        s,
        StatusCode::NOT_FOUND,
        "Not found",
        "<section><h1>This page isn’t available</h1><p>It doesn’t exist, or you need to sign in to see it.</p></section>",
    )
}

/// 404 for a page no version of which carries a bundle.
fn not_an_app(s: &AppState, id: &ArtifactId) -> Response {
    html_page(
        s,
        StatusCode::NOT_FOUND,
        "Not an app",
        &format!(
            "<section><h1>This page isn’t an app yet.</h1><p>Its author hasn’t published it from the Clax app, so there’s nothing to install. It still opens in your browser.</p><p><a href=\"/a/{}\">Open the page</a></p></section>",
            id.as_str()
        ),
    )
}

/// The page of app `a`, artifact `id`. A daemon that is not hosted
/// leaves out the QR code and Open in Clax: its links name its own
/// `http://` origin, which a phone cannot reach and the app does not take
/// (only `https://` artifacts), so it offers Download .clax alone.
fn app_page(s: &AppState, id: &ArtifactId, a: &App) -> Response {
    let origin = origin(s);
    let get = format!("{origin}/a/{}/get", id.as_str());
    let artifact = format!("{origin}/a/{}", id.as_str());
    let encoded: String = url::form_urlencoded::byte_serialize(artifact.as_bytes()).collect();
    let initial = a
        .title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or("·".to_string(), |c| c.to_uppercase().collect());
    let icon = a
        .icon
        .as_deref()
        .map_or(String::new(), |i| format!(" title=\"{}\"", escape(i)));
    let stores: Vec<String> = [
        (s.clax_app.ios_store.as_deref(), "App Store"),
        (s.clax_app.android_store.as_deref(), "Google Play"),
    ]
    .into_iter()
    .filter_map(|(url, name)| url.map(|u| format!("<a href=\"{}\">{name}</a>", escape(u))))
    .collect();
    let stores = if stores.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"stores\">Don’t have Clax? Get it on {}.</p>",
            stores.join(" or ")
        )
    };
    let (scan, open, download) = if s.hosted.is_some() {
        (
            format!(
                "<div class=\"scan\">{}<p>Scan with your phone’s camera to open it in Clax.</p></div>",
                qr_svg(&get)
            ),
            format!("<a class=\"btn primary\" href=\"clax:get?url={encoded}\">Open in Clax</a>"),
            "btn",
        )
    } else {
        (String::new(), String::new(), "btn primary")
    };
    let body = format!(
        "<section>\
<div class=\"app\"><div class=\"tile\" aria-hidden=\"true\"{icon}>{initial}</div><div><h1>{title}</h1><p class=\"by\">{by}</p></div></div>\
{scan}<div class=\"actions\">{open}<a class=\"{download}\" href=\"/api/artifacts/{aid}/versions/{n}/bundle\" download>Download .clax</a></div>\
{stores}<p class=\"more\"><a href=\"/a/{aid}\">Open the page in your browser</a></p></section>",
        initial = escape(&initial),
        title = escape(&a.title),
        by = escape(&byline(a)),
        aid = id.as_str(),
        n = a.version,
    );
    html_page(
        s,
        StatusCode::OK,
        &format!("{} · Get in Clax", a.title),
        &body,
    )
}

/// What a local owner's browser runs on the bare Get in Clax page
/// ([`bare_page`]): it presents the proofs its origin's storage holds
/// (`clax_http::local_trust::PROOF_STORAGE_KEY` and the keys it begins)
/// with a fetch of the same page, and puts the page that answers in place
/// of its own; it follows a redirect there, and shows how to open Clax
/// from the terminal when it holds no proof or the proof is refused. Its
/// Download .clax (a link to `/api/…`, which a navigation would reach
/// without the proof) is fetched with the proofs and saved from the bytes.
const BARE_SCRIPT: &str = r#"(function(){
var H="x-clax-owner-proof",K="clax.owner_proof",p=[],s,i,k,v,m=document.querySelector("main");
try{s=localStorage;for(i=0;i<s.length;i++){k=s.key(i);v=k&&s.getItem(k);if(v&&(k===K||k.indexOf(K+".")===0))p.push(v)}}catch(e){}
p=p.join(",");
function cold(){var c=m.querySelector("section");if(c)c.innerHTML="<h1>Open Clax from your terminal</h1><p>Run <code>clax open</code> (or <code>clax login</code> for a link). This browser is not signed in to this machine’s Clax.</p>"}
if(!p||typeof fetch!=="function"){cold();return}
var h={};h[H]=p;
document.addEventListener("click",function(e){var a=e.target&&e.target.closest&&e.target.closest("a[download]"),u=a&&a.getAttribute("href")||"";if(u.indexOf("/api/")!==0)return;e.preventDefault();
fetch(u,{headers:h,credentials:"same-origin"}).then(function(r){if(!r.ok)throw r.status;var n=(/filename="([^"]+)"/.exec(r.headers.get("content-disposition")||"")||[])[1]||"app.clax";return r.blob().then(function(b){var o=URL.createObjectURL(b),t=document.createElement("a");t.href=o;t.download=n;document.body.appendChild(t);t.click();t.remove();setTimeout(function(){URL.revokeObjectURL(o)},60000)})}).catch(function(){})});
var q={accept:"text/html"};q[H]=p;
fetch(location.pathname,{headers:q,credentials:"same-origin",cache:"no-store"}).then(function(r){
if(r.redirected){location.replace(r.url);return}
if(r.status===401){cold();return}
return r.text().then(function(t){var d=new DOMParser().parseFromString(t,"text/html"),n=d.querySelector("main");if(!n){cold();return}document.title=d.title;n=document.adoptNode(n);m.replaceWith(n);m=n})}).catch(cold)})()"#;

/// The Get in Clax page a local owner's browser gets for a page load: a
/// navigation carries the owner cookie but never the proof, and the
/// cookie alone ([`clax_http::local_trust::UnprovenSession`]) is no
/// credential to read an app's title, icon or byline, nor whether the
/// artifact exists. The page names nothing of the artifact; its script
/// ([`BARE_SCRIPT`]) loads the app's page with the proof.
fn bare_page(s: &AppState) -> Response {
    page_with(
        s,
        StatusCode::OK,
        "Get in Clax",
        "<section><h1>Get in Clax</h1><p>Opening…</p></section>",
        Some(BARE_SCRIPT),
    )
}

/// `GET /a/<id>/get` (anyone who may read the artifact; on a hosted Clax
/// the gate decides, as for `/a/<id>`): the Get in Clax page of the app
/// (see the module docs). An agent is refused 403 `not_for_agents`: the
/// page is for people. On a local daemon, a page load whose only owner
/// credential is the owner cookie without its proof gets the bare page
/// ([`bare_page`]), read from no store, as `/a/<id>` gets the bare shell.
pub async fn page(
    State(s): State<AppState>,
    d: DeniedToGrants,
    extensions: axum::http::Extensions,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    if s.hosted.is_none()
        && extensions
            .get::<clax_http::local_trust::UnprovenSession>()
            .is_some()
        && extensions
            .get::<clax_http::local_trust::OwnerSession>()
            .is_none()
    {
        return Ok(bare_page(&s));
    }
    let Ok(id) = ArtifactId::parse(&path(aid)?) else {
        return Ok(unavailable(&s));
    };
    let lookup = id.clone();
    let found = s
        .store_call(move |st| {
            let Some(art) = st.get_artifact(&lookup)? else {
                return Ok(Found::Missing);
            };
            let Some(b) = st.latest_app_bundle(&lookup)? else {
                // A page's own `get` or `get/index.html`, which this
                // address named before, is still reached by it.
                let n = art.current_version;
                let files = st
                    .get_version(&lookup, n)?
                    .map(|v| v.files)
                    .unwrap_or_default();
                let id = lookup.as_str();
                return Ok(if files.contains_key("get") {
                    Found::File(format!("/a/{id}/v/{n}/get"))
                } else if files.contains_key("get/index.html") {
                    Found::File(format!("/a/{id}/get/index.html"))
                } else {
                    Found::NotAnApp
                });
            };
            Ok::<_, CoreError>(Found::App(Box::new(App {
                title: art.title,
                icon: art.icon,
                version: b.version,
                bundle_version: b.bundle_version,
                published_by: st.person_card(person_of(b.person.as_deref()))?,
                owner: st.person_card(person_of(art.authors.creator.as_deref()))?,
            })))
        })
        .await?;
    Ok(match found {
        Found::Missing => unavailable(&s),
        Found::NotAnApp => not_an_app(&s, &id),
        Found::File(to) => (
            StatusCode::TEMPORARY_REDIRECT,
            [
                (header::LOCATION, to),
                (header::CACHE_CONTROL, "no-store".into()),
            ],
        )
            .into_response(),
        Found::App(a) => app_page(&s, &id, &a),
    })
}

/// Whether `path` is a Get in Clax page's: `/a/<anything>/get`, a trailing
/// `/` allowed.
fn is_get_path(path: &str) -> bool {
    matches!(
        path.split('/').collect::<Vec<_>>().as_slice(),
        ["", "a", _, "get"] | ["", "a", _, "get", ""]
    )
}

/// Whether `path` is `/a/<id>` or below it.
fn is_artifact_path(path: &str) -> bool {
    path.strip_prefix("/a/")
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// A hosted Clax's layer outside its gate: a refusal of a Get in Clax
/// page's `GET` (401, 403 or 404 that is not already a page: a restricted
/// page, no such page, a malformed ID) becomes the one 404 page with the
/// sign-in link, so a browser is shown a way in and learns nothing of
/// which it was. A 403 `not_for_agents`, [`page`]'s refusal of an agent
/// (whatever the ID), is passed on as it is, as `docs/contract.md` lists.
///
/// Every other 404 of `/a/<id>` and its subpaths there (no such page,
/// another person's, a version or file that is not there, whoever asks)
/// becomes the same answer, the hint of
/// [`clax_http::error::ApiError::not_found_artifact`]: a page for a
/// request that accepts `text/html`, else the JSON error. One layer, so a
/// route cannot answer it differently.
pub async fn refusals(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let get = matches!(*req.method(), Method::GET | Method::HEAD) && is_get_path(req.uri().path());
    // A site's host serves the site's own paths (`/a/…` among them): its
    // answers, 404 included, are the site's, never the instance's.
    let site = req
        .extensions()
        .get::<clax_http::host::SiteRequest>()
        .is_some();
    let artifact = !site && is_artifact_path(req.uri().path());
    let get = get && !site;
    let html = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    let res = next.run(req).await;
    if artifact && !get && res.status() == StatusCode::NOT_FOUND {
        return match (&s.hosted, html) {
            (Some(h), false) => ApiError::not_found_artifact(&h.origin).into_response(),
            _ => unavailable(&s),
        };
    }
    let refused = matches!(
        res.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    );
    let page = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if !(get && refused && !page) {
        return res;
    }
    if res.status() != StatusCode::FORBIDDEN {
        return unavailable(&s);
    }
    // A 403 is a short JSON refusal: read it for its code.
    let (parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024).await else {
        return unavailable(&s);
    };
    let code = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_string));
    if code.as_deref() == Some("not_for_agents") {
        Response::from_parts(parts, axum::body::Body::from(bytes))
    } else {
        unavailable(&s)
    }
}

/// Whether the request names the hosted public host.
fn on_public_host(s: &AppState, headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    s.hosted
        .as_ref()
        .zip(host)
        .is_some_and(|(h, host)| host.eq_ignore_ascii_case(&h.host))
}

fn json_file(v: &serde_json::Value) -> Response {
    let mut res = ([(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response();
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    res
}

/// The path the app links claim: `/a/<id>/get`, with one `?` (any one
/// character) for each of an artifact ID's characters. An app site
/// association's `*` would also cross `/` and claim `/a/<id>/v/<n>/get`, a
/// page file named `get`, which the app does not take; at this length no
/// path but `/a/<id>/get` fits. The Android app's `pathPattern`
/// (`apps/clax_app/android/app/src/main/AndroidManifest.xml`) is the same
/// with `.` for `?`.
fn app_link_path() -> String {
    format!("/a/{}/get", "?".repeat(ARTIFACT_ID_LEN))
}

/// The Apple app site association of `app`, when it names a team.
pub fn apple_app_site_association_of(app: &ClaxApp) -> Option<serde_json::Value> {
    let team = app.apple_team_id.as_deref()?;
    Some(json!({"applinks": {"details": [{
        "appIDs": [format!("{team}.{APPLE_BUNDLE_ID}")],
        "components": [{"/": app_link_path()}],
    }]}}))
}

/// The Digital Asset Links statement of `app`, when it names the Android
/// app.
pub fn assetlinks_of(app: &ClaxApp) -> Option<serde_json::Value> {
    let a = app.android.as_ref()?;
    Some(json!([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {
            "namespace": "android_app",
            "package_name": a.package,
            "sha256_cert_fingerprints": [a.sha256],
        },
    }]))
}

/// `GET /.well-known/apple-app-site-association`: lets the Clax app on iOS
/// (`<team>.dev.empathic.clax`) open `/a/<id>/get`. The Mac app, which
/// takes `clax:` links only, does not read it. Served without
/// a session, on the hosted public host, when [`ClaxApp::apple_team_id`]
/// is set; else 404. An agent of another machine is refused, as on every
/// route not declared for agents ([`DeniedToGrants`]).
pub async fn apple_app_site_association(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    headers: HeaderMap,
) -> Response {
    match apple_app_site_association_of(&s.clax_app).filter(|_| on_public_host(&s, &headers)) {
        Some(v) => json_file(&v),
        None => ApiError::not_found().into_response(),
    }
}

/// `GET /.well-known/assetlinks.json`: lets the Clax app on Android open
/// this host's links. Served without a session, on the hosted public host,
/// when [`ClaxApp::android`] is set; else 404. An agent of another
/// machine is refused ([`DeniedToGrants`]).
pub async fn assetlinks(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    headers: HeaderMap,
) -> Response {
    match assetlinks_of(&s.clax_app).filter(|_| on_public_host(&s, &headers)) {
        Some(v) => json_file(&v),
        None => ApiError::not_found().into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_get_path_is_get_after_an_artifact_segment() {
        assert!(is_get_path("/a/7q3k9mzx2b4t/get"));
        assert!(is_get_path("/a/7q3k9mzx2b4t/get/"));
        assert!(is_get_path("/a/nope/get"));
        assert!(!is_get_path("/a/7q3k9mzx2b4t/v/1/get"));
        assert!(!is_get_path("/a/7q3k9mzx2b4t/get/x"));
        assert!(!is_get_path("/a/7q3k9mzx2b4t"));
        assert!(!is_get_path("/api/artifacts/7q3k9mzx2b4t/get"));
    }

    /// Whether `path` matches the app site association pattern `pattern`
    /// as Apple reads it: `?` is any one character, `*` any run of them
    /// (`/` included), anything else itself.
    fn aasa_matches(pattern: &[u8], path: &[u8]) -> bool {
        match (pattern.split_first(), path.split_first()) {
            (None, _) => path.is_empty(),
            (Some((b'*', rest)), _) => (0..=path.len()).any(|i| aasa_matches(rest, &path[i..])),
            (Some((b'?', rest)), Some((_, more))) => aasa_matches(rest, more),
            (Some((c, rest)), Some((d, more))) => c == d && aasa_matches(rest, more),
            (Some(_), None) => false,
        }
    }

    #[test]
    fn the_app_links_claim_the_get_page_alone() {
        let id = ArtifactId::generate();
        let id = id.as_str();
        assert_eq!(app_link_path(), "/a/????????????/get");
        let claims = |p: &str| aasa_matches(app_link_path().as_bytes(), p.as_bytes());
        assert!(claims(&format!("/a/{id}/get")));
        assert!(!claims(&format!("/a/{id}/v/3/get")));
        assert!(!claims(&format!("/a/{id}/v/12/get")));
        assert!(!claims(&format!("/a/{id}")));
        assert!(!claims(&format!("/a/{id}/get/x")));
        // The pattern this one replaces claimed a page file named `get`.
        assert!(aasa_matches(
            b"/a/*/get",
            format!("/a/{id}/v/3/get").as_bytes()
        ));
    }

    #[test]
    fn the_byline_names_an_editor_only_when_not_the_owner() {
        let card = |name: &str, login: Option<&str>| PersonCard {
            name: name.into(),
            login: login.map(str::to_string),
            public_id: None,
        };
        let mut a = App {
            title: "T".into(),
            icon: None,
            version: 1,
            bundle_version: 8,
            published_by: card("Ada", Some("akesling")),
            owner: card("Ada", Some("akesling")),
        };
        assert_eq!(byline(&a), "v8 · by @akesling");
        a.published_by = card("Jun", Some("junp"));
        assert_eq!(byline(&a), "v8 by @junp · by @akesling");
        a.published_by = card("Kim", None);
        assert_eq!(byline(&a), "v8 by Kim · by @akesling");
    }

    /// The value of custom property `name` in `css` after the first `after`.
    fn token<'a>(css: &'a str, after: &str, name: &str) -> &'a str {
        let rest = &css[css.find(after).expect(after)..];
        let at = rest.find(&format!("{name}:")).expect(name) + name.len() + 1;
        rest[at..].split([';', '}']).next().unwrap_or("").trim()
    }

    #[test]
    fn the_page_wears_the_shells_colours_light_and_dark() {
        let theme = include_str!("../../../../web/shell/src/theme.css");
        // The page's token and the shell's it copies.
        let pairs = [
            ("--bg", "--bg"),
            ("--fg", "--fg"),
            ("--muted", "--muted"),
            ("--card", "--card"),
            ("--border", "--border"),
            ("--tile", "--hover"),
        ];
        for (mine, shell) in pairs {
            assert_eq!(
                token(STYLE, ":root{", mine),
                token(theme, ":root {", shell),
                "light {mine}"
            );
            assert_eq!(
                token(STYLE, "prefers-color-scheme:dark", mine),
                token(theme, "prefers-color-scheme: dark", shell),
                "dark {mine}"
            );
        }
    }

    #[test]
    fn the_qr_code_is_an_inline_svg_element() {
        let svg = qr_svg("https://clax.dev/a/7q3k9mzx2b4t/get");
        assert!(svg.starts_with("<svg class=\"qr\" role=\"img\""), "{svg}");
        assert!(!svg.contains("<?xml"), "{svg}");
        assert!(svg.ends_with("</svg>"), "{svg}");
    }
}

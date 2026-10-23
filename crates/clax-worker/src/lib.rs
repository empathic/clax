//! A Cloudflare Worker that runs the Clax server: one Durable Object per
//! Clax instance ([`instance::InstanceDo`]), one per artifact for its app
//! data ([`app_data::AppDataDo`]), the web UI from Workers Static Assets,
//! and, beside them, the SQLite-on-Durable-Object spike ([`ClaxDb`]) the
//! VFS was proved with.
//!
//! # The Worker
//!
//! Every request goes to the Worker first (`run_worker_first`), which
//! classifies it by its host before its path (`route`):
//!
//! - `/_internal/` on any host, and any request naming an internal host
//!   ([`INSTANCE_INTERNAL`], `app_data::APP_DATA_INTERNAL`), is 404 (see
//!   Pushes), as is any request whose `Host` names a port that is no number;
//! - a site host, `<artifact ID>.<SITE_DOMAIN>` (`<artifact ID>.localhost`
//!   without `SITE_DOMAIN` or `HOSTED`; `instance::site_hosts`), is that
//!   artifact's site: the instance's site routes answer every path there
//!   (`clax_server::routes::site`), the UI's files under `/_clax/` from the
//!   static assets as the site routes serve them, with the headers the site
//!   layer gives every site response (`site_access::site_headers`) and no
//!   `Access-Control-Allow-Origin`. No path-first route below is reached
//!   on a site host: `/git/`, `/_diag/`, `/_dev/repos/` and `/do/` are the
//!   site's own paths, as `/mcp` and `/api/` are;
//! - with `CLAX_TEST_ROUTES` = `1` (`0` in `wrangler.jsonc`), `/do/{name}/…`
//!   goes to the spike object named `name` (below), whose routes check no
//!   credential; otherwise `/do/` is a path like any other, the instance's;
//! - on the app host, `/`, `/inbox`, `/people`, `/tags`, `/admin`,
//!   `/settings`, `/me` and `/audit` are the shell's
//!   `index.html`, from the static assets, with the headers the daemon's
//!   `shell::gallery_page` sends (no framing, revalidated), except on a
//!   hosted instance (`HOSTED` = `1`), whose gate decides who gets them;
//! - `/_clax/<path>` is the web UI's file from the static assets with the
//!   daemon's caching (`shell::asset_cache`), except the bridge and the
//!   contract's type definitions, which the server answers;
//! - everything else (the API, `/a/…` with its first-load data, the
//!   stream's WebSocket) goes to the instance's object, named by
//!   `CLAX_INSTANCE` (`default` when unset). A file the assets lack goes
//!   there too.
//!
//! A `SITE_DOMAIN` hosted mode refuses (`instance::site_hosts`: not a bare
//! domain, or `PUBLIC_ORIGIN`'s host, above it or under it) makes every
//! request 500, naming the var. Browsers send `*.localhost` to this
//! machine, so under `wrangler dev` the site hosts work with `SITE_DOMAIN`
//! = `localhost`; a deploy routes `*.<SITE_DOMAIN>/*` to the Worker
//! (`README.md` has the DNS).
//!
//! `RETIRED_CONTENT_ORIGIN`, when set, is the host that served every
//! artifact's content at keyed addresses before sites: its requests go to
//! the instance whatever their path ([`Route::Retired`]), which answers an
//! asset's old address with a 301 to its artifact's site and nothing else
//! (`clax_http::hosted::Hosted::with_retired_content_origin`).
//!
//! # The spike's routes
//!
//! Only with `CLAX_TEST_ROUTES` = `1`, which `test-wasm.mjs` sets:
//!
//! - `POST /do/{name}/init`: creates the `notes` and `blobs` tables;
//! - `POST /do/{name}/notes?body=…`: inserts a note;
//! - `GET /do/{name}/notes`: lists the notes;
//! - `POST /do/{name}/abort?body=…`: inserts a note and rolls it back;
//! - `POST /do/{name}/fill?mb=…`: inserts `mb` MiB of random 64 KiB blobs in
//!   one transaction;
//! - `GET /do/{name}/stats?check=1`: page counts, store counters and,
//!   with `check`, `PRAGMA integrity_check` and a digest of the blobs;
//! - `POST /do/{name}/journal?mode=…`: runs `PRAGMA journal_mode=…`, which
//!   the connection refuses for any mode but `MEMORY`;
//! - `GET /do/{name}/ws`: a WebSocket; each text message is stored as a note
//!   and answered with its row ID and the note count.
//!
//! The Worker also hands `/git/<repo>.git/…` and `/_dev/repos/…` to
//! [`repos`] (artifact repositories, their tokens and assets), and, with
//! `CLAX_DIAG_ROUTES` = `1`, `/_diag/…` to [`diag`] (what the platform's
//! bindings answer, for the owner's token only).
//!
//! # Pushes
//!
//! A commit pushed to an artifact's repository becomes a version when the
//! instance records it: `POST /_internal/pushed` on the instance's object,
//! at the host [`INSTANCE_INTERNAL`] (`{"artifact", "person",
//! "session"}`, the last two whom the push's token was handed to), which
//! only other objects and the Worker's own handlers reach: the Worker
//! answers any request for `/_internal/`, or naming that host, with 404
//! rather than forward it.
//! `RepoDO` makes the call once the stand-in takes a push; the Worker's
//! queue consumer makes it for each artifact a Cloudflare Artifacts
//! `cf.artifacts.repo.pushed` event names (`clax_git::events`), acking
//! the event's messages when it succeeds or is refused (4xx), and retrying
//! them otherwise, at most `max_retries` times (`wrangler.jsonc`). The
//! event, as `clax_git::events` reads it, names no pusher, and pushes to
//! Artifacts never pass `RepoDO`, so their commits are the owner's.
//! Before each publish the instance records any commits still unrecorded,
//! so a lost event never blocks one.
//!
//! Build and run from this directory: `npm install`, then `npm run dev`
//! (`wrangler dev --local --persist-to .wrangler/state`, which runs
//! `build.sh`; build the web UI first with `scripts/build-web.sh`); `npm
//! run size` prints the upload size. `cargo test` runs the VFS's tests
//! natively; `npm run test:wasm` (`test-wasm.mjs`) builds the Worker and
//! checks the spike's routes under `wrangler dev --local` across a restart,
//! and `npm run e2e` (`e2e.mjs`) the Clax instance in two browsers.

mod app_data;
pub mod bot;
pub mod diag;
mod instance;
pub mod repos;
pub mod sandbox;
mod store;
mod web;

use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::extract::{Query, State as AxState};
use axum::http::{HeaderValue, StatusCode, header};
use axum::routing::{get, post};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_service::Service;
use worker::{
    DurableObject, Env, HttpRequest, HttpResponse, Request, Response, Result, State, WebSocket,
    WebSocketIncomingMessage, WebSocketPair, durable_object, event,
};

use clax_http::host::SiteHosts;

pub use crate::app_data::AppDataDo;
pub use crate::instance::InstanceDo;

/// The web UI's static assets (`assets.binding` in `wrangler.jsonc`); the
/// `ASSETS` name is the artifact assets' R2 bucket ([`repos::worker`]).
const WEB_BINDING: &str = "WEB";
use crate::store::DoPageStore;

/// The Worker (see the crate docs). It takes the runtime's `Request` as
/// it is, so a body it reads or forwards is the runtime's own stream (an
/// `HttpRequest`'s is re-wrapped in a stream this module serves, and
/// reading or cancelling that one throws past wasm-bindgen).
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: worker::Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let path = req.path();
    let url_host = req.url()?.host_str().map(str::to_owned);
    let host = req.headers().get("host")?.unwrap_or_default();
    let sites = match instance::site_hosts(&env) {
        Ok(s) => s.unwrap_or_else(SiteHosts::local),
        Err(e) => return Response::error(format!("the instance could not start: {e}"), 500),
    };
    // The spike's routes write to its objects unauthenticated: tests only.
    let spike = env
        .var("CLAX_TEST_ROUTES")
        .is_ok_and(|v| v.to_string() == "1");
    let retired = env
        .var("RETIRED_CONTENT_ORIGIN")
        .ok()
        .and_then(|v| authority(&v.to_string()));
    let hosts = Hosts {
        sites: &sites,
        retired: retired.as_deref(),
    };
    match route(&path, url_host.as_deref(), &host, &hosts, spike) {
        Route::Refused => return Response::error("Not found", 404),
        Route::Retired => return to_instance(&env, req).await,
        #[cfg(target_arch = "wasm32")]
        Route::Diag => {
            if let Some(res) = diag::route(&env, &req).await {
                return res;
            }
        }
        #[cfg(target_arch = "wasm32")]
        Route::Git => return repos::worker::forward_git(&env, req).await,
        #[cfg(target_arch = "wasm32")]
        Route::DevRepos => return repos::worker::dev_route(&env, req).await,
        #[cfg(not(target_arch = "wasm32"))]
        Route::Diag | Route::Git | Route::DevRepos => {}
        Route::Spike(None) => {
            let headers = worker::Headers::new();
            headers.set("x-clax-error", "routes live under /do/{name}/")?;
            return Ok(Response::empty()?.with_status(404).with_headers(headers));
        }
        Route::Spike(Some(name)) => {
            let stub = env.durable_object("CLAX_DB")?.get_by_name(name)?;
            return stub.fetch_with_request(req).await;
        }
        Route::Site => {
            // A service worker's script, of any path, is the instance's
            // answer: Clax's own, which unregisters itself
            // (`clax_http::host::site_layer`).
            let worker_script = req
                .headers()
                .get("service-worker")?
                .is_some_and(|v| v.eq_ignore_ascii_case("script"));
            if worker_script {
                return to_instance(&env, req).await;
            }
            if let Some(mut res) = static_asset(&env, &path, false).await? {
                let hosted = env.var("HOSTED").is_ok_and(|v| v.to_string() == "1");
                let public = env
                    .var("PUBLIC_ORIGIN")
                    .ok()
                    .and_then(|v| clax_core::config::hosted_origin(&v.to_string()))
                    .filter(|_| hosted);
                site_static_headers(res.headers_mut(), public.as_deref(), &host);
                return Response::try_from(res);
            }
            return to_instance(&env, req).await;
        }
        Route::App => {}
    }
    // A hosted instance's gate decides who gets the gallery pages (a
    // guest is sent to sign in), so the instance serves them there.
    let hosted = env.var("HOSTED").is_ok_and(|v| v.to_string() == "1");
    if let Some(res) = static_asset(&env, &path, !hosted).await? {
        return Response::try_from(res);
    }
    to_instance(&env, req).await
}

/// `req` for the instance's object, unless its declared body is too large.
async fn to_instance(env: &Env, req: Request) -> Result<Response> {
    if let Some(res) = instance::refuse_declared(&req)? {
        return Ok(res);
    }
    instance_stub(env)?.fetch_with_request(req).await
}

/// Where the Worker sends a request (see the crate docs).
#[derive(Debug, PartialEq, Eq)]
enum Route<'a> {
    /// A call between objects (see Pushes), never forwarded: 404.
    Refused,
    /// `/_diag/…` ([`diag`]).
    Diag,
    /// `/git/…` ([`repos`]).
    Git,
    /// `/_dev/repos/…` ([`repos`]).
    DevRepos,
    /// `/do/{name}/…` with the test routes on: the spike's object `name`
    /// (`None` when the path names none).
    Spike(Option<&'a str>),
    /// A site host's request: the instance's site routes, and nothing
    /// else, answer it (the UI's static files under `/_clax/` among them).
    Site,
    /// The retired content origin's (`RETIRED_CONTENT_ORIGIN`): the
    /// instance alone, which redirects an asset's old address to its site
    /// and answers anything else 404.
    Retired,
    /// The app host's: its static entries and files, else the instance.
    App,
}

/// The hosts [`route`] tells apart: the site domain's, and the retired
/// content origin's authority (`content.clax.dev`), when there is one.
struct Hosts<'a> {
    sites: &'a SiteHosts,
    retired: Option<&'a str>,
}

/// `origin`'s authority as a `Host` header names it (`host[:port]`);
/// `None` for an empty or malformed one.
fn authority(origin: &str) -> Option<String> {
    let u = worker::Url::parse(origin.trim()).ok()?;
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => Some(format!("{h}:{p}")),
        (Some(h), None) => Some(h.to_string()),
        _ => None,
    }
}

/// Whether `host` (a `Host` header) is the site domain or a name under it
/// (its port, a trailing dot and case aside). The apex `localhost` is not:
/// with the local site domain it is the app host.
fn under_site_domain(host: &str, sites: &SiteHosts) -> bool {
    let name = match host.rsplit_once(':') {
        Some((n, p)) if p.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => host,
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let domain = sites.domain();
    if name == domain {
        return domain != clax_http::host::LOCAL_SITE_DOMAIN;
    }
    name.strip_suffix(domain)
        .is_some_and(|rest| rest.ends_with('.'))
}

/// Whether `host` (a `Host` header) names no port or a numeric one, after
/// its name or a bracketed IPv6 literal (`[::1]:8787`).
fn port_well_formed(host: &str) -> bool {
    let rest = if host.starts_with('[') {
        match host.find(']') {
            Some(i) => &host[i + 1..],
            None => return false,
        }
    } else {
        host.find(':').map_or("", |i| &host[i..])
    };
    match rest.strip_prefix(':') {
        Some(port) => port.bytes().all(|b| b.is_ascii_digit()),
        None => rest.is_empty(),
    }
}

/// Classifies a request by its path, its URL's host (`url_host`) and its
/// `Host` header (`host`), the host first: `/_internal/` and the internal
/// hosts are refused on any host; a site host under `sites` is
/// [`Route::Site`] whatever its path, so no path-first route (`/git/`,
/// `/_diag/`, `/_dev/repos/`, `/do/`) is reached there; any other name
/// under the site domain (its apex, a nested name, an uppercase or
/// malformed ID, a trailing dot) is refused, never the app's, as is any
/// `Host` whose port is no number; the retired content origin is
/// [`Route::Retired`] whatever its path.
fn route<'a>(
    path: &'a str,
    url_host: Option<&str>,
    host: &str,
    hosts: &Hosts,
    spike: bool,
) -> Route<'a> {
    let sites = hosts.sites;
    let internal = [INSTANCE_INTERNAL, app_data::APP_DATA_INTERNAL];
    if path.starts_with("/_internal/")
        || url_host.is_some_and(|h| internal.contains(&h))
        || internal.iter().any(|h| h.eq_ignore_ascii_case(host))
        || !port_well_formed(host)
    {
        return Route::Refused;
    }
    if sites.target(host).is_some() {
        return Route::Site;
    }
    if hosts.retired.is_some_and(|r| r.eq_ignore_ascii_case(host)) {
        return Route::Retired;
    }
    if under_site_domain(host, sites) {
        return Route::Refused;
    }
    if path.starts_with(diag::PREFIX) {
        return Route::Diag;
    }
    if path.starts_with("/git/") {
        return Route::Git;
    }
    if path.starts_with("/_dev/repos/") {
        return Route::DevRepos;
    }
    match path.strip_prefix("/do/").filter(|_| spike) {
        Some(rest) => Route::Spike(rest.split('/').next().filter(|n| !n.is_empty())),
        None => Route::App,
    }
}

/// The instance's object, named by `CLAX_INSTANCE` (`default` when unset).
fn instance_stub(env: &Env) -> Result<worker::Stub> {
    let name = env
        .var("CLAX_INSTANCE")
        .map(|v| v.to_string())
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "default".to_string());
    env.durable_object("CLAX_INSTANCE_DO")?.get_by_name(&name)
}

/// The host of calls to the instance's object from other objects and the
/// Worker's handlers (see Pushes).
pub const INSTANCE_INTERNAL: &str = "instance.internal";

/// Asks the instance to record the commits pushed to `artifact`'s
/// repository as versions published by `holder`; answers the status (200
/// when recorded, or when there was nothing to record).
pub(crate) async fn reconcile_pushes(
    env: &Env,
    artifact: &str,
    holder: &repos::tokens::Holder,
) -> Result<u16> {
    let body = json!({
        "artifact": artifact,
        "person": holder.person,
        "session": holder.session,
    });
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Post)
        .with_body(Some(wasm_bindgen::JsValue::from_str(&body.to_string())));
    let req = Request::new_with_init(
        &format!("http://{INSTANCE_INTERNAL}/_internal/pushed"),
        &init,
    )?;
    Ok(instance_stub(env)?
        .fetch_with_request(req)
        .await?
        .status_code())
}

/// Whether the instance answers that `artifact` lies within the owner's
/// reach (`POST /_internal/reach`; see Pushes): what the diagnostics need
/// before they read or hand out its repository.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn owner_reaches(env: &Env, artifact: &str) -> Result<bool> {
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Post)
        .with_body(Some(wasm_bindgen::JsValue::from_str(
            &json!({ "artifact": artifact }).to_string(),
        )));
    let req = Request::new_with_init(
        &format!("http://{INSTANCE_INTERNAL}/_internal/reach"),
        &init,
    )?;
    Ok(instance_stub(env)?
        .fetch_with_request(req)
        .await?
        .status_code()
        == 204)
}

/// The consumer of the queue the Artifacts event subscription delivers to
/// (`queues.consumers` in `wrangler.jsonc`; see Pushes): each artifact a
/// pushed event names is reconciled once per batch; every other message is
/// acknowledged.
#[event(queue)]
async fn queue(batch: worker::MessageBatch<Value>, env: Env, _ctx: worker::Context) -> Result<()> {
    use worker::MessageExt;
    console_error_panic_hook::set_once();
    let messages = batch.messages()?;
    let plan = clax_git::events::plan_batch(messages.iter().map(|m| m.body()));
    for i in plan.ack {
        messages[i].ack();
    }
    for (artifact, asked) in plan.reconcile {
        let done = match reconcile_pushes(&env, &artifact, &Default::default()).await {
            Ok(200) => true,
            // Answered and refused (a malformed call): retrying cannot help.
            Ok(status) if (400..500).contains(&status) => {
                worker::console_error!(
                    "recording the pushes to {artifact} was refused ({status}); acknowledging"
                );
                true
            }
            Ok(status) => {
                worker::console_error!("recording the pushes to {artifact} answered {status}");
                false
            }
            Err(e) => {
                worker::console_error!("recording the pushes to {artifact} failed: {e}");
                false
            }
        };
        for i in asked {
            if done {
                messages[i].ack();
            } else {
                messages[i].retry();
            }
        }
    }
    Ok(())
}

/// The web UI's file for `path` from the static assets, with the headers
/// the daemon serves it with; `None` for a path the server answers, or a
/// file the assets lack.
async fn static_asset(env: &Env, path: &str, app_host: bool) -> Result<Option<HttpResponse>> {
    let (file, cache, entry) = match path {
        "/" | "/inbox" | "/people" | "/tags" | "/admin" | "/settings" | "/me" | "/audit"
            if app_host =>
        {
            ("/index.html".to_string(), "no-cache", true)
        }
        _ => match path
            .strip_prefix("/_clax/")
            .and_then(clax_server::shell::asset_cache)
        {
            Some(cache) if !path.split('/').any(|c| c.starts_with('.')) => {
                (path.to_string(), cache, false)
            }
            _ => return Ok(None),
        },
    };
    let Ok(assets) = env.assets(WEB_BINDING) else {
        return Ok(None);
    };
    let mut res = assets
        .fetch(format!("https://assets.invalid{file}"), None)
        .await?;
    if !res.status().is_success() {
        return Ok(None);
    }
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if entry {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("frame-ancestors 'none'"),
        );
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    if file.ends_with(".js") {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/javascript"),
        );
    }
    Ok(Some(res))
}

/// Marks a static file served on site host `host` as the site layer marks
/// every site response (`clax_http::site_access::site_headers`), with no
/// `Access-Control-Allow-Origin` whatever the static assets sent: `public`
/// is a hosted instance's public origin, `None` on one that is not hosted.
fn site_static_headers(h: &mut axum::http::HeaderMap, public: Option<&str>, host: &str) {
    h.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    let secure = public.is_some_and(|o| o.starts_with("https://"));
    clax_http::site_access::site_headers(
        clax_http::site_access::SitePolicy {
            hosted: public,
            embed: None,
            secure,
            policy_set: false,
        },
        host,
        None,
        h,
    );
}

/// Durable Objects this isolate has constructed, to tell a warm object from
/// one rebuilt after eviction or hibernation.
static CONSTRUCTED: AtomicU32 = AtomicU32::new(0);

/// What every route shares: the object's one connection and its store.
#[derive(Clone)]
struct App {
    conn: Arc<Mutex<clax_do_vfs::Database>>,
    file: String,
    store: Arc<StoreHandle>,
    /// A random tag for this object instance.
    instance: u32,
    /// `CONSTRUCTED` when this instance was made.
    construction: u32,
}

/// The page store, for its counters. `DoPageStore` holds JS objects, which
/// are neither `Send` nor `Sync`; a Durable Object runs on one thread, and
/// axum's bounds are met by the wrapper `worker::send::SendWrapper`.
type StoreHandle = worker::send::SendWrapper<Rc<DoPageStore>>;

impl App {
    fn db(&self) -> MutexGuard<'_, clax_do_vfs::Database> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }
}

type ApiResult = std::result::Result<Json<Value>, (StatusCode, String)>;

fn fail(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn router(app: App) -> axum::Router {
    axum::Router::new()
        .route("/init", post(init))
        .route("/notes", post(add_note).get(list_notes))
        .route("/abort", post(abort_note))
        .route("/fill", post(fill))
        .route("/stats", get(stats))
        .route("/journal", post(journal_mode))
        .with_state(app)
}

async fn init(AxState(app): AxState<App>) -> ApiResult {
    app.db()
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS blobs (id INTEGER PRIMARY KEY, data BLOB NOT NULL);",
        )
        .map_err(fail)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct Body {
    body: String,
}

fn insert_note(db: &Connection, body: &str) -> rusqlite::Result<(i64, i64)> {
    db.execute("INSERT INTO notes (body) VALUES (?1)", params![body])?;
    let id = db.last_insert_rowid();
    let n = db.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?;
    Ok((id, n))
}

async fn add_note(AxState(app): AxState<App>, Query(q): Query<Body>) -> ApiResult {
    let (id, count) = insert_note(&app.db(), &q.body).map_err(fail)?;
    Ok(Json(json!({ "id": id, "count": count })))
}

async fn abort_note(AxState(app): AxState<App>, Query(q): Query<Body>) -> ApiResult {
    let mut db = app.db();
    let tx = db.transaction().map_err(fail)?;
    let (id, inside) = insert_note(&tx, &q.body).map_err(fail)?;
    tx.rollback().map_err(fail)?;
    let after: i64 = db
        .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
        .map_err(fail)?;
    Ok(Json(
        json!({ "rolled_back_id": id, "count_inside": inside, "count_after": after }),
    ))
}

async fn list_notes(AxState(app): AxState<App>) -> ApiResult {
    let db = app.db();
    let mut st = db
        .prepare("SELECT id, body FROM notes ORDER BY id")
        .map_err(fail)?;
    let notes: Vec<Value> = st
        .query_map([], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "body": r.get::<_, String>(1)? }))
        })
        .map_err(fail)?
        .collect::<rusqlite::Result<_>>()
        .map_err(fail)?;
    Ok(Json(json!({ "notes": notes })))
}

#[derive(Deserialize)]
struct Fill {
    mb: u32,
}

async fn fill(AxState(app): AxState<App>, Query(q): Query<Fill>) -> ApiResult {
    let blobs = q.mb * 16;
    let mut db = app.db();
    let before = clax_do_vfs::stats(&app.file).unwrap_or_default();
    let tx = db.transaction().map_err(fail)?;
    tx.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO blobs (data) SELECT randomblob(65536) FROM n",
        params![blobs],
    )
    .map_err(fail)?;
    tx.commit().map_err(fail)?;
    let after = clax_do_vfs::stats(&app.file).unwrap_or_default();
    let pages: i64 = db
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .map_err(fail)?;
    Ok(Json(json!({
        "inserted_blobs": blobs,
        "page_count": pages,
        "store_commits": after.commits - before.commits,
        "chunks_written": after.chunk_writes - before.chunk_writes,
    })))
}

#[derive(Deserialize)]
struct Journal {
    mode: String,
}

async fn journal_mode(AxState(app): AxState<App>, Query(q): Query<Journal>) -> ApiResult {
    if q.mode.is_empty() || !q.mode.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err((StatusCode::BAD_REQUEST, "mode is a word".to_owned()));
    }
    let mode: String = app
        .db()
        .query_row(&format!("PRAGMA journal_mode={}", q.mode), [], |r| r.get(0))
        .map_err(fail)?;
    Ok(Json(json!({ "journal_mode": mode })))
}

#[derive(Deserialize)]
struct Stats {
    check: Option<u8>,
}

async fn stats(AxState(app): AxState<App>, Query(q): Query<Stats>) -> ApiResult {
    let db = app.db();
    let one = |sql: &str| -> std::result::Result<i64, (StatusCode, String)> {
        db.query_row(sql, [], |r| r.get(0)).map_err(fail)
    };
    let mut out = json!({
        "instance": app.instance,
        "construction": app.construction,
        "file": app.file,
        "page_size": one("PRAGMA page_size")?,
        "page_count": one("PRAGMA page_count")?,
        "notes": one("SELECT count(*) FROM notes").unwrap_or(-1),
        "blobs": one("SELECT count(*) FROM blobs").unwrap_or(-1),
        "journal_mode": db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).map_err(fail)?,
        "vfs": format!("{:?}", clax_do_vfs::stats(&app.file).unwrap_or_default()),
        "store_chunk_rows": app.store.chunk_rows(&app.file).map_err(fail)?,
        "do_sql_database_size": app.store.database_size(),
    });
    if q.check == Some(1) {
        out["integrity_check"] = db
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .map_err(fail)?
            .into();
        // A digest of every blob byte, to compare across restarts.
        let mut st = db
            .prepare("SELECT data FROM blobs ORDER BY id")
            .map_err(fail)?;
        let mut rows = st.query([]).map_err(fail)?;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut bytes = 0u64;
        while let Some(r) = rows.next().map_err(fail)? {
            let b = r.get_ref(0).map_err(fail)?.as_blob().map_err(fail)?;
            bytes += b.len() as u64;
            for x in b {
                h = (h ^ u64::from(*x)).wrapping_mul(0x100_0000_01b3);
            }
        }
        out["blob_bytes"] = bytes.into();
        out["blob_fnv1a"] = format!("{h:016x}").into();
    }
    Ok(Json(out))
}

/// One database per object, in its own storage. The database is opened at
/// the first event; an open that fails answers that event with the reason
/// (503) and is tried again at the next, rather than aborting the isolate.
#[durable_object]
pub struct ClaxDb {
    state: State,
    opened: std::cell::RefCell<Option<(App, axum::Router)>>,
}

impl ClaxDb {
    /// The object's database and routes, opened now when they are not yet.
    fn opened(&self) -> Result<(App, axum::Router)> {
        if let Some(o) = self.opened.borrow().as_ref() {
            return Ok(o.clone());
        }
        let err = |what: &str, e: &dyn std::fmt::Display| {
            worker::Error::RustError(format!("ClaxDb: {what}: {e}"))
        };
        let store =
            Rc::new(DoPageStore::new(self.state.storage()).map_err(|e| err("the VFS tables", &e))?);
        let file = format!("{}.db", self.state.id());
        let conn = clax_do_vfs::open(&file, store.clone())
            .map_err(|e| err("the database does not open", &e))?;
        let app = App {
            conn: Arc::new(Mutex::new(conn)),
            file,
            store: Arc::new(worker::send::SendWrapper::new(store)),
            instance: (js_sys::Math::random() * f64::from(u32::MAX)) as u32,
            construction: CONSTRUCTED.fetch_add(1, Ordering::Relaxed) + 1,
        };
        let o = (app.clone(), router(app));
        *self.opened.borrow_mut() = Some(o.clone());
        Ok(o)
    }
}

impl DurableObject for ClaxDb {
    fn new(state: State, _env: Env) -> Self {
        console_error_panic_hook::set_once();
        ClaxDb {
            state,
            opened: std::cell::RefCell::new(None),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let (_, mut router) = match self.opened() {
            Ok(o) => o,
            Err(e) => {
                worker::console_error!("{e}");
                return Response::error(format!("{e}\n"), 503);
            }
        };
        let mut req = HttpRequest::try_from(req)?;
        // Strip `/do/{name}`.
        let path = req.uri().path();
        let rest = path
            .strip_prefix("/do/")
            .and_then(|r| r.find('/').map(|i| &r[i..]))
            .unwrap_or("/")
            .to_owned();
        if rest == "/ws" {
            let pair = WebSocketPair::new()?;
            self.state.accept_web_socket(&pair.server);
            return Response::from_websocket(pair.client);
        }
        let pq = match req.uri().query() {
            Some(q) => format!("{rest}?{q}"),
            None => rest,
        };
        *req.uri_mut() = pq
            .parse()
            .map_err(|e| worker::Error::RustError(format!("{e}")))?;
        let res = router
            .call(req)
            .await
            .map_err(|e| worker::Error::RustError(format!("{e}")))?;
        Response::try_from(res)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let WebSocketIncomingMessage::String(text) = message else {
            return ws.send_with_str("binary messages are not stored");
        };
        let (app, _) = self.opened()?;
        let reply = match insert_note(&app.db(), &text) {
            Ok((id, count)) => json!({
                "id": id,
                "count": count,
                "sockets": self.state.get_websockets().len(),
                "instance": app.instance,
                "construction": app.construction,
            }),
            Err(e) => json!({ "error": e.to_string() }),
        };
        ws.send_with_str(reply.to_string())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        code: usize,
        reason: String,
        _clean: bool,
    ) -> Result<()> {
        ws.close(Some(code as u16), Some(reason))
    }
}

#[cfg(test)]
mod tests {
    use super::{Hosts, Route, SiteHosts};

    /// [`super::route`] with no retired content origin.
    fn route<'a>(
        path: &'a str,
        url_host: Option<&str>,
        host: &str,
        sites: &SiteHosts,
        spike: bool,
    ) -> Route<'a> {
        let hosts = Hosts {
            sites,
            retired: None,
        };
        super::route(path, url_host, host, &hosts, spike)
    }

    #[test]
    fn the_retired_content_origin_goes_to_the_instance_whatever_its_path() {
        let sites = SiteHosts::new("clax.run").unwrap();
        let retired_host = super::authority("https://content.clax.example.com");
        let hosts = Hosts {
            sites: &sites,
            retired: retired_host.as_deref(),
        };
        let retired = "content.clax.example.com";
        for path in [
            "/_blob/x",
            "/git/a.git/info/refs",
            "/_diag/bindings",
            "/",
            "/do/n/x",
        ] {
            assert_eq!(
                super::route(path, Some(retired), retired, &hosts, true),
                Route::Retired,
                "{path}"
            );
        }
        assert_eq!(
            super::route("/_internal/x", Some(retired), retired, &hosts, true),
            Route::Refused
        );
        assert_eq!(super::route("/", Some(APP), APP, &hosts, true), Route::App);
        assert_eq!(super::authority(""), None);
        assert_eq!(
            super::authority("http://127.0.0.1:8788").as_deref(),
            Some("127.0.0.1:8788")
        );
    }

    const SITE: &str = "abcdefghjkmn.clax.run";
    const APP: &str = "clax.example.com";

    #[test]
    fn a_site_host_reaches_no_path_first_route() {
        let sites = SiteHosts::new("clax.run").unwrap();
        let first = [
            "/git/a-abcdefghjkmn.git/info/refs",
            "/_diag/bindings",
            "/_dev/repos/x",
            "/do/n/notes",
            "/mcp",
            "/api/artifacts",
            "/_worker/hibernate",
        ];
        for path in first {
            for host in [SITE, "abcdefghjkmn.clax.run:443", "abcdefghjkmn.CLAX.RUN"] {
                assert_eq!(
                    route(path, Some(host), host, &sites, true),
                    Route::Site,
                    "{host}{path}"
                );
            }
        }
        // The same paths on the app host reach their routes.
        assert_eq!(route(first[0], Some(APP), APP, &sites, true), Route::Git);
        assert_eq!(route(first[1], Some(APP), APP, &sites, true), Route::Diag);
        assert_eq!(
            route(first[2], Some(APP), APP, &sites, true),
            Route::DevRepos
        );
        assert_eq!(
            route(first[3], Some(APP), APP, &sites, true),
            Route::Spike(Some("n"))
        );
        assert_eq!(
            route("/do/", Some(APP), APP, &sites, true),
            Route::Spike(None)
        );
        assert_eq!(route(first[3], Some(APP), APP, &sites, false), Route::App);
        assert_eq!(route("/mcp", Some(APP), APP, &sites, true), Route::App);
    }

    #[test]
    fn calls_between_objects_are_refused_on_every_host() {
        let sites = SiteHosts::new("clax.run").unwrap();
        for host in [SITE, APP] {
            assert_eq!(
                route("/_internal/pushed", Some(host), host, &sites, true),
                Route::Refused
            );
        }
        for internal in [super::INSTANCE_INTERNAL, super::app_data::APP_DATA_INTERNAL] {
            assert_eq!(
                route("/", Some(internal), APP, &sites, true),
                Route::Refused,
                "{internal} in the URL"
            );
            assert_eq!(
                route("/", Some(APP), internal, &sites, true),
                Route::Refused,
                "{internal} as the Host"
            );
        }
    }

    #[test]
    fn only_hosts_under_the_site_domain_are_sites() {
        let sites = SiteHosts::new("clax.run").unwrap();
        for host in [
            "abcdefghjkmn.localhost:8787",
            APP,
            "clax.run.evil.com",
            "xclax.run",
            "[::1]:8787",
            "[::1]",
        ] {
            assert_eq!(
                route("/", Some(host), host, &sites, true),
                Route::App,
                "{host}"
            );
        }
        // Any other name under the site domain is no site, and never the app.
        for host in [
            "clax.run",
            "CLAX.RUN:443",
            "clax.run.",
            "a.b.clax.run",
            "abcdefghjkmn.a.clax.run",
            "ABCDEFGHJKMN.clax.run",
            "abcdefghjkmn.clax.run.",
            "abcdefghjkm.clax.run",
            "abcdefghjkmn0.clax.run",
            // A port that is no number, on a site host or any other.
            "abcdefghjkmn.clax.run:x",
            "abc.clax.run:x",
            "abcdefghjkmn.clax.run:443:1",
            "app.example.com:x",
            "[::1]x",
        ] {
            for path in ["/", "/_diag/x", "/_dev/repos/x", "/mcp"] {
                assert_eq!(
                    route(path, Some(host), host, &sites, true),
                    Route::Refused,
                    "{host}{path}"
                );
            }
        }
        let local = SiteHosts::local();
        // The local apex is the app host; names under it are not.
        assert_eq!(
            route("/", Some("localhost"), "localhost:8787", &local, true),
            Route::App
        );
        assert_eq!(
            route(
                "/",
                Some("a.b.localhost"),
                "a.b.localhost:8787",
                &local,
                true
            ),
            Route::Refused
        );
        let host = "abcdefghjkmn.localhost:8787";
        assert_eq!(
            route(
                "/_diag/x",
                Some("abcdefghjkmn.localhost"),
                host,
                &local,
                true
            ),
            Route::Site
        );
    }

    #[test]
    fn a_site_hosts_static_file_has_the_site_headers_and_no_cors() {
        use axum::http::{HeaderMap, HeaderValue, header};
        let file = |ct: &'static str| {
            let mut h = HeaderMap::new();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
            h.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            h
        };
        let host = "abcdefghjkmn.clax.run";
        let mut h = file("text/javascript");
        super::site_static_headers(&mut h, Some("https://clax.example"), host);
        assert!(h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(h["cross-origin-resource-policy"], "same-origin");
        assert_eq!(
            h[header::STRICT_TRANSPORT_SECURITY],
            clax_http::site_access::HSTS
        );
        let mut page = file("text/html; charset=utf-8");
        super::site_static_headers(&mut page, Some("https://clax.example"), host);
        assert_eq!(
            page[header::CONTENT_SECURITY_POLICY],
            clax_http::content_policy::lockdown("frame-ancestors 'self' https://clax.example")
                .as_str()
        );
        // Not hosted: no HSTS, and the local shell frames it.
        let mut local = file("text/html");
        super::site_static_headers(&mut local, None, "abcdefghjkmn.localhost:8787");
        assert!(local.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        assert!(local.get(header::STRICT_TRANSPORT_SECURITY).is_none());
        assert_eq!(
            local[header::CONTENT_SECURITY_POLICY],
            clax_http::content_policy::lockdown(
                "frame-ancestors 'self' http://localhost:8787 http://127.0.0.1:8787"
            )
            .as_str()
        );
    }
}

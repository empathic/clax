//! A Cloudflare Worker that runs the Clax server: one Durable Object per
//! Clax instance ([`instance::InstanceDo`]), the web UI from Workers Static
//! Assets, and, beside them, the SQLite-on-Durable-Object spike
//! ([`ClaxDb`]) the VFS was proved with.
//!
//! # The Worker
//!
//! Every request goes to the Worker first (`run_worker_first`):
//!
//! - with `CLAX_TEST_ROUTES` = `1` (`0` in `wrangler.jsonc`), `/do/{name}/…`
//!   goes to the spike object named `name` (below), whose routes check no
//!   credential; otherwise `/do/` is a path like any other, the instance's;
//! - on the app host (any host but an artifact's `<id>.localhost`), `/`,
//!   `/inbox` and `/people` are the shell's `index.html`, from the static
//!   assets, with the headers the daemon's `shell::gallery_page` sends (no
//!   framing, revalidated);
//! - on any host, `/_clax/<path>` is the web UI's file from the static
//!   assets with the daemon's caching (`shell::asset_cache`), except the
//!   bridge and the contract's type definitions, which the server answers;
//! - everything else (the API, `/a/…` with its first-load data, artifact
//!   content, the stream's WebSocket) goes to the instance's object, named
//!   by `CLAX_INSTANCE` (`default` when unset). A file the assets lack goes
//!   there too.
//!
//! Artifact content keeps its own origin as on the daemon: the server
//! serves `<id>.localhost[:port]` as that artifact's origin (its host
//! rewrite), and browsers send `*.localhost` to this machine, so under
//! `wrangler dev` the content origin works unchanged. A deployed Worker
//! needs a content domain of its own (the hosted prototype's
//! `CONTENT_ORIGIN`), which the server does not know yet.
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
//! Build and run from this directory: `npm install`, then `npm run dev`
//! (`wrangler dev --local --persist-to .wrangler/state`, which runs
//! `build.sh`; build the web UI first with `scripts/build-web.sh`); `npm
//! run size` prints the upload size. `cargo test` runs the VFS's tests
//! natively; `npm run test:wasm` (`test-wasm.mjs`) builds the Worker and
//! checks the spike's routes under `wrangler dev --local` across a restart,
//! and `npm run e2e` (`e2e.mjs`) the Clax instance in two browsers.

mod content;
mod instance;
mod store;

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

pub use crate::instance::InstanceDo;
use crate::store::DoPageStore;

/// The Worker (see the crate docs).
#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: worker::Context) -> Result<HttpResponse> {
    console_error_panic_hook::set_once();
    let path = req.uri().path().to_owned();
    // The spike's routes write to its objects unauthenticated: tests only.
    let spike = env
        .var("CLAX_TEST_ROUTES")
        .is_ok_and(|v| v.to_string() == "1");
    if let Some(rest) = path.strip_prefix("/do/").filter(|_| spike) {
        let Some(name) = rest.split('/').next().filter(|n| !n.is_empty()) else {
            return Ok(http_text(404, "routes live under /do/{name}/"));
        };
        let stub = env.durable_object("CLAX_DB")?.get_by_name(name)?;
        let res = stub.fetch_with_request(Request::try_from(req)?).await?;
        return HttpResponse::try_from(res);
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let app_host = clax_server::host::artifact_host(host).is_none();
    if let Some(res) = static_asset(&env, &path, app_host).await? {
        return Ok(res);
    }
    let name = env
        .var("CLAX_INSTANCE")
        .map(|v| v.to_string())
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "default".to_string());
    let stub = env.durable_object("CLAX_INSTANCE_DO")?.get_by_name(&name)?;
    let res = stub.fetch_with_request(Request::try_from(req)?).await?;
    HttpResponse::try_from(res)
}

/// The web UI's file for `path` from the static assets, with the headers
/// the daemon serves it with; `None` for a path the server answers, or a
/// file the assets lack.
async fn static_asset(env: &Env, path: &str, app_host: bool) -> Result<Option<HttpResponse>> {
    let (file, cache, any_origin, entry) = match path {
        "/" | "/inbox" | "/people" if app_host => {
            ("/index.html".to_string(), "no-cache", false, true)
        }
        _ => match path
            .strip_prefix("/_clax/")
            .and_then(clax_server::routes::shell::asset_cache)
        {
            Some((cache, any)) if !path.split('/').any(|c| c.starts_with('.')) => {
                (path.to_string(), cache, any, false)
            }
            _ => return Ok(None),
        },
    };
    let Ok(assets) = env.assets("ASSETS") else {
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
    if any_origin {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
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

fn http_text(status: u16, body: &str) -> HttpResponse {
    let mut res = HttpResponse::new(worker::Body::empty());
    *res.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::NOT_FOUND);
    res.headers_mut()
        .insert("x-clax-error", body.parse().expect("an ASCII message"));
    res
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

/// One database per object, in its own storage.
#[durable_object]
pub struct ClaxDb {
    state: State,
    app: App,
    router: axum::Router,
}

impl DurableObject for ClaxDb {
    fn new(state: State, _env: Env) -> Self {
        console_error_panic_hook::set_once();
        let store = Rc::new(DoPageStore::new(state.storage()).expect("the VFS tables"));
        let file = format!("{}.db", state.id());
        let conn = clax_do_vfs::open(&file, store.clone()).expect("the database opens");
        let app = App {
            conn: Arc::new(Mutex::new(conn)),
            file,
            store: Arc::new(worker::send::SendWrapper::new(store)),
            instance: (js_sys::Math::random() * f64::from(u32::MAX)) as u32,
            construction: CONSTRUCTED.fetch_add(1, Ordering::Relaxed) + 1,
        };
        let router = router(app.clone());
        ClaxDb { state, app, router }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
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
        let res = self
            .router
            .clone()
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
        let reply = match insert_note(&self.app.db(), &text) {
            Ok((id, count)) => json!({
                "id": id,
                "count": count,
                "sockets": self.state.get_websockets().len(),
                "instance": self.app.instance,
                "construction": self.app.construction,
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

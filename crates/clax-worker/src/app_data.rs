//! Each artifact's app data in a Durable Object of its own (spec
//! 2026-10-21-clax-run-site-origins "Each artifact's data in a store of its
//! own"): [`AppDataDo`], a SQLite-backed object named by the artifact's ID
//! (`APP_DATA`, `get_by_name(<artifact ID>)`), keeps the artifact's store
//! (`clax_store::app_data::AppData`) in its own storage through the VFS
//! ([`DoPageStore`]), and [`WorkerAppData`] is the instance's
//! `AppDataHost` over it, which the instance wraps in `MovingAppData` as
//! the daemon wraps its files.
//!
//! The object answers only calls at the host [`APP_DATA_INTERNAL`], which
//! the instance's object makes and the Worker never forwards (it answers
//! any request for `/_internal/` with 404, whatever its host, and binds no
//! route to `APP_DATA`):
//!
//! - `POST /_internal/app-data`: the body is an `AppDataOp` as JSON, the
//!   answer (200) its `AppDataReply`; a failure is the reply's `Error`.
//!   Only `Import` makes the store: any other operation on an object that
//!   holds none answers `NotFound`, so a call that races its artifact's
//!   delete never makes one again.
//! - `POST /_internal/app-data/erase`: deletes everything the object keeps
//!   (`deleteAll`), for an artifact that was deleted.
//!
//! The store's file is named by the object's ID, so the objects an isolate
//! holds at once never share one in the VFS's registry.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use clax_core::{ArtifactId, CoreError};
use clax_do_vfs::PageStore;
use clax_http::app_data_host::AppDataHost;
use clax_store::app_data::{AppData, AppDataOp, AppDataReply, WireError};
use worker::{DurableObject, Env, Request, Response, Result, State, durable_object};

use crate::store::DoPageStore;

/// The Durable Object binding of the artifact stores (`wrangler.jsonc`).
pub const APP_DATA_BINDING: &str = "APP_DATA";
/// The host of calls to an artifact store's object (see the module docs).
pub const APP_DATA_INTERNAL: &str = "app-data.internal";
/// The path of an operation.
const CALL_PATH: &str = "/_internal/app-data";
/// The path of an erase.
const ERASE_PATH: &str = "/_internal/app-data/erase";

/// The store as one object holds it, open.
struct Opened {
    data: AppData,
    /// Dropped after `data`'s connection: keeps the file in the VFS.
    _db: clax_do_vfs::Registered,
}

/// One artifact's store (see the module docs).
#[durable_object]
pub struct AppDataDo {
    state: State,
    opened: RefCell<Option<Rc<Opened>>>,
}

impl AppDataDo {
    /// The store's file name in the object's storage.
    fn file(&self) -> String {
        format!("app-{}.db", self.state.id())
    }

    /// The store, opened now when it is not yet; `None` when the object
    /// holds none and `create` is false.
    fn store(&self, create: bool) -> std::result::Result<Option<Rc<Opened>>, CoreError> {
        if let Some(o) = self.opened.borrow().as_ref() {
            return Ok(Some(o.clone()));
        }
        let io = |e: String| CoreError::Io(std::io::Error::other(e));
        let pages = Rc::new(DoPageStore::new(self.state.storage()).map_err(|e| io(e.to_string()))?);
        let file = self.file();
        if !create && pages.size(&file).map_err(io)?.is_none() {
            return Ok(None);
        }
        let (conn, db) = clax_do_vfs::open(&file, pages)?.into_parts();
        let clock: Arc<dyn clax_core::working::Clock> = Arc::new(clax_core::working::SystemClock);
        let opened = Rc::new(Opened {
            data: AppData::open_on(conn, clock)?,
            _db: db,
        });
        *self.opened.borrow_mut() = Some(opened.clone());
        Ok(Some(opened))
    }

    /// `op` on the store: [`AppDataReply::Error`] for a failure, `NotFound`
    /// when the object holds no store and `op` is not an import.
    fn apply(&self, op: AppDataOp) -> AppDataReply {
        let create = matches!(op, AppDataOp::Import { .. });
        match self.store(create) {
            Ok(Some(o)) => o.data.apply(op),
            Ok(None) => AppDataReply::Error(WireError::from(&CoreError::NotFound)),
            Err(e) => AppDataReply::Error(WireError::from(&e)),
        }
    }
}

impl DurableObject for AppDataDo {
    fn new(state: State, _env: Env) -> Self {
        console_error_panic_hook::set_once();
        AppDataDo {
            state,
            opened: RefCell::new(None),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        if url.host_str() != Some(APP_DATA_INTERNAL) || req.method() != worker::Method::Post {
            return Response::error("Not found", 404);
        }
        match url.path() {
            CALL_PATH => {
                let op: AppDataOp = match req.json().await {
                    Ok(op) => op,
                    Err(e) => return Response::error(format!("{e}"), 400),
                };
                Response::from_json(&self.apply(op))
            }
            ERASE_PATH => {
                // The connection and its file go before the storage does.
                self.opened.borrow_mut().take();
                self.state.storage().delete_all().await?;
                Response::ok("erased")
            }
            _ => Response::error("Not found", 404),
        }
    }
}

/// The Worker's artifact stores: each artifact's [`AppDataDo`], reached
/// from the instance's object by `get_by_name(<artifact ID>)`.
pub struct WorkerAppData {
    env: worker::send::SendWrapper<Env>,
}

impl WorkerAppData {
    pub fn new(env: Env) -> WorkerAppData {
        WorkerAppData {
            env: worker::send::SendWrapper(env),
        }
    }

    /// POSTs `body` to `path` on `aid`'s object; the answer when it is 200.
    async fn post(&self, aid: &ArtifactId, path: &str, body: String) -> Result<Response> {
        let stub = self
            .env
            .0
            .durable_object(APP_DATA_BINDING)?
            .get_by_name(aid.as_str())?;
        let mut init = worker::RequestInit::new();
        init.with_method(worker::Method::Post)
            .with_body(Some(wasm_bindgen::JsValue::from_str(&body)));
        let req = Request::new_with_init(&format!("http://{APP_DATA_INTERNAL}{path}"), &init)?;
        let mut res = stub.fetch_with_request(req).await?;
        if res.status_code() != 200 {
            let text = res.text().await.unwrap_or_default();
            return Err(worker::Error::RustError(format!(
                "the artifact store of {aid} answered {}: {text}",
                res.status_code()
            )));
        }
        Ok(res)
    }
}

fn io(e: impl std::fmt::Display) -> CoreError {
    CoreError::Io(std::io::Error::other(e.to_string()))
}

/// The calls hold the runtime's objects, which are not `Send`; a Worker
/// runs on one thread, so [`worker::send::SendFuture`] meets the trait's
/// bound natively (for the crate's native build) as on wasm32.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl AppDataHost for WorkerAppData {
    async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> clax_core::Result<AppDataReply> {
        let body = serde_json::to_string(&op).map_err(io)?;
        worker::send::SendFuture::new(async move {
            let mut res = self.post(aid, CALL_PATH, body).await.map_err(io)?;
            let reply: AppDataReply = res.json().await.map_err(io)?;
            reply.checked()
        })
        .await
    }

    async fn erase(&self, aid: &ArtifactId) -> clax_core::Result<()> {
        worker::send::SendFuture::new(async move {
            self.post(aid, ERASE_PATH, String::new())
                .await
                .map(drop)
                .map_err(io)
        })
        .await
    }
}

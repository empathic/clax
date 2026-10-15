//! The Worker's side, wasm32 only: `RepoDO` and the functions the Worker
//! and the instance Durable Object call.
//!
//! - [`content_store`]: the `ContentStore` for `env` (`REPO_STORE`): git
//!   versions in `RepoDO`'s stand-in or in Cloudflare Artifacts, assets in
//!   the `ASSETS` R2 bucket either way. Make one per Durable Object
//!   instance (the Artifacts host caches its write tokens and remotes in
//!   it).
//! - [`forward_git`]: the Worker's `/git/<repo>.git/…` route, served by
//!   `RepoDO` when `REPO_STORE` is `memory` and `GIT_SMART_HTTP` is not
//!   `0`; 404 otherwise (Artifacts serves its own remotes).
//! - [`mint_git_token`] and [`revoke_git_token`]: repository tokens, and
//!   [`git_remotes`], the server's `GitRemotes` over them, for its
//!   `POST /api/artifacts/{id}/git-token` route (a signed-in person, same
//!   origin) and `POST /api/sessions/{id}/git-token/{aid}` (an agent
//!   session, the MCP `git_remote` tool), after their rights checks (see
//!   `clax_server::git_remotes`).
//! - [`dev_route`]: `/_dev/repos/…`, served only while `REPO_DEV_ROUTES`
//!   is `1`, for `test-repos.mjs`.
//!
//! A push the stand-in takes is recorded as versions before `RepoDO`
//! answers it. `RepoDO` first enters whom the push's token was handed to
//! ([`tokens::Holder`]) in its ledger against the commit the push left
//! `main` at ([`tokens::note_push`], before anything else can reach it),
//! then asks the instance to record the artifact's pushed commits
//! (`crate::reconcile_pushes`). Every reconcile, this one, the one before
//! each publish and an Artifacts event's, reads the pushers from that
//! ledger through [`content_store`]'s [`DoLedger`]. A failure is logged
//! and the push kept; the next publish records the commits.

use std::rc::Rc;
use std::sync::Arc;

use clax_content_git::AssetStore;
use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::content::{AssetKey, ContentStore, NewFile, VersionWrite};
use clax_core::working::{Clock, SystemClock};
use clax_git::server::ServeOptions;
use clax_git::versions::repo_name;
use clax_git::{GitError, Person};
use serde_json::json;
use wasm_bindgen::JsValue;
use worker::{DurableObject, Env, Headers, Request, Response, Result, State, durable_object};

use super::RepoStore;
use super::api::{
    DoHost, DoLedger, DoTokens, INTERNAL_HOST, InternalRequest, RepoService, git_authorized,
};
use super::artifacts::{ArtifactsHost, ArtifactsTokens};
use super::assets::BucketAssets;
use super::git_http::{GitHttpRequest, authorize, serve_git, serve_options};
use super::js::{FetchTransport, JsArtifacts, R2Objects, StubTransport, read_body_capped};
use super::local::LocalHost;
use super::tokens::{self, GitToken, Holder, LocalTokens, Minted, Scope, TokenHost};
use crate::store::DoPageStore;

/// The `RepoDO` namespace's binding, and the one instance's name.
pub const REPO_BINDING: &str = "REPO";
pub const REPO_INSTANCE: &str = "repos";
/// The Artifacts binding.
pub const ARTIFACTS_BINDING: &str = "ARTIFACTS";
/// The R2 bucket binding artifact assets are kept in.
pub const ASSETS_BINDING: &str = "ASSETS";
/// How long after a failed revocation the must-revoke list is retried.
pub const REVOKE_RETRY_MS: i64 = 60_000;

fn var(env: &Env, name: &str) -> Option<String> {
    env.var(name).ok().map(|v| v.to_string())
}

/// `REPO_STORE`: `memory` when unset; an error for any other value.
pub fn repo_store(env: &Env) -> Result<RepoStore> {
    match var(env, "REPO_STORE") {
        None => Ok(RepoStore::Memory),
        Some(s) => RepoStore::parse(&s)
            .ok_or_else(|| worker::Error::RustError(format!("Unknown REPO_STORE {s:?}"))),
    }
}

/// Whether `/git/` is served: the stand-in, unless `GIT_SMART_HTTP` is `0`.
pub fn git_http_serving(env: &Env) -> bool {
    matches!(repo_store(env), Ok(RepoStore::Memory))
        && var(env, "GIT_SMART_HTTP").as_deref() != Some("0")
}

fn repo_do(env: &Env) -> Result<StubTransport> {
    Ok(StubTransport {
        stub: Arc::new(
            env.durable_object(REPO_BINDING)?
                .get_by_name(REPO_INSTANCE)?,
        ),
    })
}

fn artifacts_binding(env: &Env) -> Result<JsArtifacts> {
    let b = js_sys::Reflect::get(env.as_ref(), &JsValue::from(ARTIFACTS_BINDING))
        .map_err(|_| worker::Error::RustError("No ARTIFACTS binding".into()))?;
    if b.is_undefined() {
        return Err(worker::Error::RustError("No ARTIFACTS binding".into()));
    }
    Ok(JsArtifacts::new(b))
}

/// Who commits what no person published (a fork's version 1).
pub fn committer() -> Person {
    Person {
        name: "Clax".into(),
        email: "clax@users.clax.dev".into(),
    }
}

/// Artifact assets in the [`ASSETS_BINDING`] bucket.
pub fn asset_store(env: &Env) -> Result<BucketAssets<R2Objects>> {
    let bucket = env.bucket(ASSETS_BINDING).map_err(|e| {
        worker::Error::RustError(format!("No {ASSETS_BINDING} R2 bucket binding: {e}"))
    })?;
    Ok(BucketAssets {
        objects: R2Objects::new(bucket),
    })
}

/// The content store for `env`; see the [module docs](self). Its I/O
/// awaits JavaScript promises, so it needs an executor
/// (`ContentStore::needs_executor`): the server awaits the store's `async`
/// methods for it. Fails when
/// `REPO_STORE` names no host, or a binding the host needs (`REPO` for
/// `memory`, `ARTIFACTS` for `artifacts`, `ASSETS` for both) is missing.
pub fn content_store(env: &Env) -> Result<Arc<dyn ContentStore>> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let assets = asset_store(env)?;
    // RepoDO keeps the token ledger, and who pushed, for either store.
    let ledger: Arc<dyn clax_core::content::PushLedger> = Arc::new(DoLedger {
        transport: repo_do(env)?,
    });
    Ok(match repo_store(env)? {
        RepoStore::Memory => Arc::new(
            GitContent::new(
                DoHost {
                    transport: repo_do(env)?,
                },
                assets,
                committer(),
                clock,
            )
            .needing_executor()
            .with_ledger(ledger),
        ),
        RepoStore::Artifacts => Arc::new(
            GitContent::new(
                ArtifactsHost::new(artifacts_binding(env)?, FetchTransport, clock.clone()),
                assets,
                committer(),
                clock,
            )
            .needing_executor()
            .with_ledger(ledger),
        ),
    })
}

/// The Worker's `/git/` route, served by `RepoDO` while the stand-in is
/// served (else 404). The Worker asks `RepoDO` whether the request's token
/// is live first (401 unread when not), then reads the body, at most the
/// path's limit (413 past it), and forwards it whole: `RepoDO` never
/// answers while a body is still streaming to it, which the runtime takes
/// for a lost connection.
pub async fn forward_git(env: &Env, req: Request) -> Result<Response> {
    if !git_http_serving(env) {
        return Response::error("git smart HTTP is served only for REPO_STORE=memory\n", 404);
    }
    let repo = repo_do(env)?;
    let url = req.url()?;
    let path = url.path().to_string();
    let authorization = req.headers().get("authorization")?;
    match git_authorized(&repo, &path, authorization.as_deref()).await {
        Ok(true) => {}
        Ok(false) => return http_response(clax_git::server::unauthorized()),
        Err(e) => return Response::error(format!("{e}\n"), 503),
    }
    let limit = super::git_http::body_limit(&path, &serve_options()).unwrap_or(0);
    let Some(body) = read_capped(&req, limit).await? else {
        return http_response(clax_git::server::too_large(limit));
    };
    let headers = req.headers().clone();
    headers.delete("content-length")?;
    headers.delete("transfer-encoding")?;
    let mut init = worker::RequestInit::new();
    init.with_method(req.method()).with_headers(headers);
    if !body.is_empty() {
        init.with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
    }
    let forwarded = Request::new_with_init(url.as_str(), &init)?;
    repo.stub.fetch_with_request(forwarded).await
}

/// A token for artifact `artifact`'s repository, handed to `holder`, living `ttl_s` seconds
/// ([`tokens::PERSON_TOKEN_TTL_S`] for a person,
/// [`tokens::AGENT_TOKEN_TTL_S`] for an agent). `origin` is the app origin
/// the asking request came to, where the stand-in's remote is served
/// unless `PUBLIC_ORIGIN` is set. The caller checks rights first.
pub async fn mint_git_token(
    env: &Env,
    artifact: &ArtifactId,
    scope: Scope,
    ttl_s: u32,
    origin: Option<&str>,
    holder: &Holder,
) -> clax_git::Result<GitToken> {
    let t = repo_do(env).map_err(|e| GitError::RepoUnavailable(e.to_string()))?;
    DoTokens { transport: t }
        .mint(&repo_name(artifact.as_str()), scope, ttl_s, origin, holder)
        .await
}

/// The server's git remotes ([`super::remotes::WorkerRemotes`]): tokens
/// from `RepoDO`'s ledger, whichever host keeps the repositories; pushes
/// are credited to their tokens' holders only through the stand-in, which
/// sees them (Cloudflare Artifacts' pushed events name no pusher). Fails
/// without the `REPO` binding.
pub fn git_remotes(env: &Env) -> Result<Arc<dyn clax_server::git_remotes::GitRemotes>> {
    Ok(Arc::new(super::remotes::WorkerRemotes {
        tokens: DoTokens {
            transport: repo_do(env)?,
        },
        names_pushers: matches!(repo_store(env)?, RepoStore::Memory),
    }))
}

/// Revokes token `id` of artifact `artifact`'s repository; a failure at
/// the host leaves it in `RepoDO`'s must-revoke list, retried until it is
/// revoked or expires.
pub async fn revoke_git_token(
    env: &Env,
    artifact: &ArtifactId,
    id: &str,
) -> clax_git::Result<bool> {
    let t = repo_do(env).map_err(|e| GitError::RepoUnavailable(e.to_string()))?;
    DoTokens { transport: t }
        .revoke(&repo_name(artifact.as_str()), id)
        .await
}

// ── RepoDO ──────────────────────────────────────────────────────────────────

/// Where `RepoDO`'s tokens come from.
pub enum DoTokenHost {
    Local(LocalTokens<clax_do_vfs::Database>),
    Artifacts(ArtifactsTokens<JsArtifacts>),
}

impl TokenHost for DoTokenHost {
    async fn mint(
        &self,
        repo: &str,
        scope: Scope,
        ttl_s: u32,
        now_ms: i64,
    ) -> clax_git::Result<Minted> {
        match self {
            DoTokenHost::Local(t) => t.mint(repo, scope, ttl_s, now_ms).await,
            DoTokenHost::Artifacts(t) => t.mint(repo, scope, ttl_s, now_ms).await,
        }
    }

    async fn revoke(&self, repo: &str, id: &str, now_ms: i64) -> clax_git::Result<bool> {
        match self {
            DoTokenHost::Local(t) => t.revoke(repo, id, now_ms).await,
            DoTokenHost::Artifacts(t) => t.revoke(repo, id, now_ms).await,
        }
    }

    async fn remote(&self, repo: &str, origin: Option<&str>) -> clax_git::Result<String> {
        match self {
            DoTokenHost::Local(t) => t.remote(repo, origin).await,
            DoTokenHost::Artifacts(t) => t.remote(repo, origin).await,
        }
    }
}

/// The stand-in's repositories and the token ledger: one instance,
/// [`REPO_INSTANCE`]. Its parts are built at the first event; a build that
/// fails (the database does not open, `REPO_STORE` names no store, or names
/// `artifacts` with no `ARTIFACTS` binding) answers that event 503 with the
/// reason and is tried again at the next, rather than aborting the isolate.
#[durable_object]
pub struct RepoDO {
    state: State,
    env: Env,
    built: std::cell::RefCell<Option<Rc<Built>>>,
}

/// What [`RepoDO`] serves with, once built.
struct Built {
    service: RepoService<clax_do_vfs::Database, DoTokenHost>,
    serving: bool,
    options: ServeOptions,
}

impl Built {
    fn new(state: &State, env: &Env) -> Result<Built> {
        let err = |what: &str, e: &dyn std::fmt::Display| {
            worker::Error::RustError(format!("RepoDO: {what}: {e}"))
        };
        let pages =
            Rc::new(DoPageStore::new(state.storage()).map_err(|e| err("the VFS tables", &e))?);
        let file = format!("repos-{}.db", state.id());
        let db = clax_do_vfs::open(&file, pages)
            .map_err(|e| err("the repository database does not open", &e))?;
        let host = LocalHost::new(db).map_err(|e| err("the repository tables", &e))?;
        let tokens = match repo_store(env)? {
            RepoStore::Memory => DoTokenHost::Local(LocalTokens {
                host: host.clone(),
                public_origin: var(env, "PUBLIC_ORIGIN").unwrap_or_default(),
            }),
            RepoStore::Artifacts => DoTokenHost::Artifacts(ArtifactsTokens {
                api: artifacts_binding(env)?,
            }),
        };
        Ok(Built {
            serving: git_http_serving(env),
            service: RepoService {
                host,
                tokens,
                clock: Arc::new(SystemClock),
            },
            options: serve_options(),
        })
    }
}

impl RepoDO {
    /// The object's parts, built now when they are not yet (see the type).
    fn built(&self) -> Result<Rc<Built>> {
        if let Some(b) = self.built.borrow().as_ref() {
            return Ok(b.clone());
        }
        let b = Rc::new(Built::new(&self.state, &self.env).inspect_err(|e| {
            worker::console_error!("{e}");
        })?);
        *self.built.borrow_mut() = Some(b.clone());
        Ok(b)
    }
}

fn http_response(r: clax_git::http::HttpResponse) -> Result<Response> {
    let headers = Headers::new();
    for (k, v) in &r.headers {
        headers.set(k, v)?;
    }
    Ok(Response::from_bytes(r.body)?
        .with_status(r.status)
        .with_headers(headers))
}

/// The body of `req`, or `None` once it runs past `limit` bytes: a
/// declared `Content-Length` over it is refused unread, and a chunked body
/// is read only until it passes it.
async fn read_capped(req: &Request, limit: usize) -> Result<Option<Vec<u8>>> {
    let declared = req
        .headers()
        .get("content-length")?
        .and_then(|n| n.parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Ok(None);
    }
    read_body_capped(req, limit).await
}

impl RepoDO {
    async fn schedule_retry(&self, b: &Built) {
        let due = b
            .service
            .host
            .with(|c| super::db::due(c, b.service.now_ms()))
            .map(|d| !d.is_empty())
            .unwrap_or(false);
        if !due {
            return;
        }
        let storage = self.state.storage();
        let at = super::now_ms() + REVOKE_RETRY_MS;
        let set = match storage.get_alarm().await {
            Ok(Some(t)) => t > at,
            _ => true,
        };
        if !set {
            return;
        }
        if let Err(e) = storage.set_alarm(at).await {
            worker::console_error!("RepoDO: setting the revoke retry alarm failed: {e}");
        }
    }
}

impl RepoDO {
    /// Asks the instance to record the commits of the push to `artifact`'s
    /// repository by `holder`; logs a failure.
    async fn record_push(&self, artifact: &str, holder: &Holder) {
        let repo = repo_name(artifact);
        match crate::reconcile_pushes(&self.env, artifact, holder).await {
            Ok(200) => {}
            Ok(status) => worker::console_error!(
                "RepoDO: recording the push to {repo} as versions answered {status}"
            ),
            Err(e) => {
                worker::console_error!(
                    "RepoDO: recording the push to {repo} as versions failed: {e}"
                )
            }
        }
    }
}

impl DurableObject for RepoDO {
    fn new(state: State, env: Env) -> Self {
        console_error_panic_hook::set_once();
        RepoDO {
            state,
            env,
            built: std::cell::RefCell::new(None),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let b = match self.built() {
            Ok(b) => b,
            Err(e) => return Response::error(format!("{e}\n"), 503),
        };
        let url = req.url()?;
        let path = url.path().to_string();
        let query = url.query().map(String::from);
        let internal = url.host_str() == Some(INTERNAL_HOST);
        if let Some(op) = path.strip_prefix("/_repos/").filter(|_| internal) {
            let body = req.bytes().await?;
            let method = req.method().to_string();
            let r = b
                .service
                .serve(&InternalRequest {
                    method: &method,
                    op,
                    query: query.as_deref(),
                    body: &body,
                })
                .await;
            if op == "token/revoke" || op == "token/revoke-held" {
                self.schedule_retry(&b).await;
            }
            return http_response(r);
        }
        if path.starts_with("/git/") && b.serving {
            let authorization = req.headers().get("authorization")?;
            // One time for the whole request: the token that was live when
            // it was checked names the push's pusher, however long the
            // body took, never the owner because it lapsed meanwhile.
            let now = b.service.now_ms();
            // The token first, so no body is read for a caller without one.
            match authorize(&b.service.host, &path, authorization.as_deref(), now) {
                Ok(Some(_)) => {}
                Ok(None) => return http_response(clax_git::server::unauthorized()),
                Err(e) => return Response::error(format!("{e}\n"), 503),
            }
            let limit = super::git_http::body_limit(&path, &b.options).unwrap_or(0);
            let Some(body) = read_capped(&req, limit).await? else {
                return http_response(clax_git::server::too_large(limit));
            };
            let method = req.method().to_string();
            let content_encoding = req.headers().get("content-encoding")?;
            let served = serve_git(
                &b.service.host,
                &GitHttpRequest {
                    method: &method,
                    path: &path,
                    query: query.as_deref(),
                    authorization: authorization.as_deref(),
                    content_encoding: content_encoding.as_deref(),
                    body: &body,
                },
                &b.options,
                now,
            )
            .await;
            // Entered before the next await, so no reconcile reads the
            // pushed commits without their pusher.
            let noted = tokens::note_push(
                &b.service.host,
                &path,
                authorization.as_deref(),
                &served.pushed,
                now,
            );
            match noted {
                Ok(Some((artifact, holder))) => self.record_push(&artifact, &holder).await,
                Ok(None) => {}
                Err(e) => worker::console_error!("RepoDO: entering the push to {path} failed: {e}"),
            }
            return http_response(served.response);
        }
        Response::error("Not found", 404)
    }

    async fn alarm(&self) -> Result<Response> {
        let b = self.built()?;
        match tokens::retry_due(&b.service.host, &b.service.tokens, b.service.now_ms()).await {
            Ok(0) => {}
            Ok(_) => self.schedule_retry(&b).await,
            Err(e) => {
                worker::console_error!("RepoDO: retrying revocations failed: {e}");
                self.schedule_retry(&b).await;
            }
        }
        Response::ok("")
    }
}

// ── Development routes ──────────────────────────────────────────────────────

fn param(url: &worker::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn bad(e: impl std::fmt::Display) -> Result<Response> {
    Response::error(e.to_string(), 400)
}

/// `/_dev/repos/…`, while `REPO_DEV_ROUTES` is `1`:
///
/// - `POST publish?artifact=&n=&body=`: writes version `n` of the artifact
///   holding `index.html` with `body`, carrying nothing, through
///   [`content_store`]; answers what version `n` reads back;
/// - `POST token?artifact=&scope=&person=&session=`: a token handed to
///   `person` (a public ID; none: the owner) and `session` (an agent's),
///   the remote at the request's origin;
/// - `POST revoke?artifact=&id=`: revokes a token;
/// - `PUT|GET|DELETE asset?artifact=&asset=&ext=` (PUT: `body=`): an asset
///   through [`asset_store`]; GET answers its bytes, or 404;
/// - `DELETE assets?artifact=`: every asset of the artifact.
pub async fn dev_route(env: &Env, req: Request) -> Result<Response> {
    if var(env, "REPO_DEV_ROUTES").as_deref() != Some("1") {
        return Response::error("Not found", 404);
    }
    let url = req.url()?;
    let Some(artifact) = param(&url, "artifact") else {
        return bad("artifact is required");
    };
    let artifact = match ArtifactId::parse(&artifact) {
        Ok(a) => a,
        Err(e) => return bad(e),
    };
    match url.path().strip_prefix("/_dev/repos/") {
        Some("publish") => {
            let n: u32 = match param(&url, "n").and_then(|n| n.parse().ok()) {
                Some(n) => n,
                None => return bad("n is a version number"),
            };
            let body = param(&url, "body").unwrap_or_default();
            let store = content_store(env)?;
            let write = VersionWrite {
                carry: Vec::new(),
                put: vec![NewFile {
                    path: "index.html",
                    bytes: body.as_bytes(),
                    content_type: "text/html",
                }],
                ..Default::default()
            };
            if let Err(e) = store.write_version(&artifact, n, &write).await {
                return Response::error(e.to_string(), 409);
            }
            let read = store
                .read_file(&artifact, n, "index.html")
                .await
                .map_err(|e| worker::Error::RustError(e.to_string()))?;
            Response::from_json(&json!({
                "version": n,
                "index_html": read.map(|b| String::from_utf8_lossy(&b).into_owned()),
            }))
        }
        Some("token") => {
            let scope = match param(&url, "scope").as_deref().and_then(Scope::parse) {
                Some(s) => s,
                None => return bad("scope must be read or write"),
            };
            let origin = url.origin().ascii_serialization();
            let holder = Holder {
                person: param(&url, "person").filter(|p| !p.is_empty()),
                session: param(&url, "session").filter(|s| !s.is_empty()),
                link: false,
            };
            match mint_git_token(
                env,
                &artifact,
                scope,
                tokens::PERSON_TOKEN_TTL_S,
                Some(&origin),
                &holder,
            )
            .await
            {
                Ok(t) => Response::from_json(&t),
                Err(e) => Response::error(e.to_string(), 409),
            }
        }
        Some("asset") => {
            let (Some(asset), Some(ext)) = (param(&url, "asset"), param(&url, "ext")) else {
                return bad("asset and ext are required");
            };
            let key = AssetKey {
                artifact,
                id: asset,
                ext,
            };
            let store = asset_store(env)?;
            let r = match req.method() {
                worker::Method::Put => {
                    let body = param(&url, "body").unwrap_or_default();
                    store.put_asset(&key, body.as_bytes()).await.map(|()| None)
                }
                worker::Method::Get => store.read_asset(&key).await,
                worker::Method::Delete => store.delete_asset(&key).await.map(|()| None),
                _ => return Response::error("Use PUT, GET or DELETE", 405),
            };
            match r {
                Ok(Some(b)) => Response::from_bytes(b),
                Ok(None) if req.method() == worker::Method::Get => Response::error("No asset", 404),
                Ok(None) => Response::from_json(&json!({})),
                Err(e) => Response::error(e.to_string(), 503),
            }
        }
        Some("assets") if req.method() == worker::Method::Delete => {
            match asset_store(env)?.delete_assets(&artifact).await {
                Ok(()) => Response::from_json(&json!({})),
                Err(e) => Response::error(e.to_string(), 503),
            }
        }
        Some("revoke") => {
            let Some(id) = param(&url, "id") else {
                return bad("id is required");
            };
            match revoke_git_token(env, &artifact, &id).await {
                Ok(r) => Response::from_json(&json!({ "revoked": r })),
                Err(e) => Response::error(e.to_string(), 409),
            }
        }
        _ => Response::error("Not found", 404),
    }
}

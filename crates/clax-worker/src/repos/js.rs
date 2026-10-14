//! The JavaScript side, wasm32 only: the `ARTIFACTS` binding as an
//! [`ArtifactsApi`], the `ASSETS` R2 bucket as asset [`Objects`], `fetch`
//! and Durable Object stubs as clax-git [`Transport`]s. JavaScript values are held in `SendWrapper`s: a Worker
//! runs on one thread, and the content store must be `Send + Sync`.

use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::object::parse_trailers;
use clax_git::{CommitMeta, GitError, ObjectId, Signature, TreeEntry};
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::send::SendWrapper;
use worker::{Fetch, Headers, Method, Request, RequestInit, Stub};

use super::artifacts::{
    ApiToken, ArtifactsApi, ArtifactsRepoApi, CreatedRepo, error_for_code, tree_entry,
};
use super::assets::{Objects, Page};
use super::tokens::{Scope, parse_rfc3339_ms};

fn js_message(e: &JsValue) -> String {
    e.dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| e.as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}

/// A thrown Artifacts error as clax-git's, by its `code`.
fn js_error(e: JsValue) -> GitError {
    let code = Reflect::get(&e, &"code".into())
        .ok()
        .and_then(|c| c.as_string());
    error_for_code(code.as_deref(), js_message(&e))
}

fn get(o: &JsValue, key: &str) -> JsValue {
    Reflect::get(o, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn get_str(o: &JsValue, key: &str) -> clax_git::Result<String> {
    get(o, key).as_string().ok_or_else(|| {
        GitError::RepoUnavailable(format!("The Artifacts binding answered no {key}"))
    })
}

/// Calls `o[method](...args)` and awaits the result when it is a promise.
async fn call(o: &JsValue, method: &str, args: &[JsValue]) -> clax_git::Result<JsValue> {
    let f: Function = get(o, method)
        .dyn_into()
        .map_err(|_| GitError::RepoUnavailable(format!("The Artifacts binding has no {method}")))?;
    let argv: Array = args.iter().collect();
    let v = f.apply(o, &argv).map_err(js_error)?;
    if v.is_instance_of::<Promise>() {
        JsFuture::from(Promise::from(v)).await.map_err(js_error)
    } else {
        Ok(v)
    }
}

fn created(v: &JsValue) -> clax_git::Result<CreatedRepo> {
    Ok(CreatedRepo {
        remote: get_str(v, "remote")?,
        token: get(v, "token").as_string(),
    })
}

/// The `ARTIFACTS` binding.
pub struct JsArtifacts {
    binding: SendWrapper<JsValue>,
}

impl JsArtifacts {
    pub fn new(binding: JsValue) -> JsArtifacts {
        JsArtifacts {
            binding: SendWrapper::new(binding),
        }
    }
}

impl ArtifactsApi for JsArtifacts {
    type Handle = JsRepoHandle;

    async fn create(&self, name: &str) -> clax_git::Result<CreatedRepo> {
        let opts = Object::new();
        Reflect::set(&opts, &"setDefaultBranch".into(), &"main".into()).map_err(js_error)?;
        created(&call(&self.binding, "create", &[name.into(), opts.into()]).await?)
    }

    async fn get(&self, name: &str) -> clax_git::Result<Option<JsRepoHandle>> {
        match call(&self.binding, "get", &[name.into()]).await {
            Ok(h) => Ok(Some(JsRepoHandle {
                handle: SendWrapper::new(h),
            })),
            Err(GitError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, name: &str) -> clax_git::Result<bool> {
        Ok(call(&self.binding, "delete", &[name.into()])
            .await?
            .as_bool()
            .unwrap_or(false))
    }

    async fn pause(&self, ms: u32) {
        worker::Delay::from(std::time::Duration::from_millis(u64::from(ms))).await;
    }
}

/// A repository handle from the binding's `get`, disposed when dropped.
pub struct JsRepoHandle {
    handle: SendWrapper<JsValue>,
}

impl Drop for JsRepoHandle {
    fn drop(&mut self) {
        // `Symbol.dispose`, where the runtime has it.
        let symbol = get(&js_sys::global(), "Symbol");
        let key = get(&symbol, "dispose");
        if key.is_undefined() {
            return;
        }
        let dispose = Reflect::get(&self.handle, &key)
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok());
        if let Some(f) = dispose {
            let _ = f.call0(&self.handle);
        }
    }
}

fn signature(v: &JsValue, at: f64) -> clax_git::Result<Signature> {
    Ok(Signature {
        name: get_str(v, "name")?,
        email: get_str(v, "email")?,
        time: at as i64,
        tz_offset_minutes: 0,
    })
}

fn hex(v: &JsValue, key: &str) -> clax_git::Result<ObjectId> {
    let s = get_str(v, key)?;
    ObjectId::from_hex(&s)
        .ok_or_else(|| GitError::RepoUnavailable(format!("A malformed {key} {s:?}")))
}

async fn blob_bytes(blob: &JsValue) -> clax_git::Result<Vec<u8>> {
    let buf = call(blob, "arrayBuffer", &[]).await?;
    Ok(Uint8Array::new(&buf).to_vec())
}

impl ArtifactsRepoApi for JsRepoHandle {
    async fn remote(&self) -> clax_git::Result<String> {
        get_str(&call(&self.handle, "info", &[]).await?, "remote")
    }

    async fn create_token(&self, scope: Scope, ttl_s: u32) -> clax_git::Result<ApiToken> {
        let t = call(
            &self.handle,
            "createToken",
            &[scope.as_str().into(), ttl_s.into()],
        )
        .await?;
        let expires = get_str(&t, "expiresAt")?;
        Ok(ApiToken {
            id: get_str(&t, "id")?,
            plaintext: get_str(&t, "plaintext")?,
            expires_ms: parse_rfc3339_ms(&expires)
                .ok_or_else(|| GitError::RepoUnavailable(format!("A token expiry {expires:?}")))?,
        })
    }

    async fn revoke_token(&self, id: &str) -> clax_git::Result<bool> {
        Ok(call(&self.handle, "revokeToken", &[id.into()])
            .await?
            .as_bool()
            .unwrap_or(false))
    }

    async fn fork(&self, to: &str) -> clax_git::Result<CreatedRepo> {
        created(&call(&self.handle, "fork", &[to.into()]).await?)
    }

    async fn read_commit(&self, id: &ObjectId) -> clax_git::Result<Option<CommitMeta>> {
        let c = call(&self.handle, "readCommit", &[id.to_hex().into()]).await?;
        if c.is_null() || c.is_undefined() {
            return Ok(None);
        }
        let parents: Array = get(&c, "parents")
            .dyn_into()
            .map_err(|_| GitError::RepoUnavailable("A commit without parents".into()))?;
        let parents = parents
            .iter()
            .map(|p| {
                p.as_string()
                    .and_then(|s| ObjectId::from_hex(&s))
                    .ok_or_else(|| GitError::RepoUnavailable("A malformed parent".into()))
            })
            .collect::<clax_git::Result<_>>()?;
        let message = get_str(&c, "message")?;
        Ok(Some(CommitMeta {
            commit: *id,
            tree: hex(&c, "treeHash")?,
            parents,
            author: signature(
                &get(&c, "author"),
                get(&c, "authoredAt").as_f64().unwrap_or(0.0),
            )?,
            committer: signature(
                &get(&c, "committer"),
                get(&c, "committedAt").as_f64().unwrap_or(0.0),
            )?,
            trailers: parse_trailers(&message),
            message,
        }))
    }

    async fn read_tree(&self, id: &ObjectId) -> clax_git::Result<Option<Vec<TreeEntry>>> {
        let t = call(&self.handle, "readTree", &[id.to_hex().into()]).await?;
        if t.is_null() || t.is_undefined() {
            return Ok(None);
        }
        let entries: Array = t
            .dyn_into()
            .map_err(|_| GitError::RepoUnavailable("A tree that is not a list".into()))?;
        entries
            .iter()
            .map(|e| {
                tree_entry(
                    &get_str(&e, "mode")?,
                    get_str(&e, "name")?,
                    &get_str(&e, "hash")?,
                )
            })
            .collect::<clax_git::Result<_>>()
            .map(Some)
    }

    async fn read_blob(&self, id: &ObjectId) -> clax_git::Result<Option<Vec<u8>>> {
        let b = call(&self.handle, "readBlob", &[id.to_hex().into()]).await?;
        if b.is_null() || b.is_undefined() {
            return Ok(None);
        }
        blob_bytes(&b).await.map(Some)
    }

    async fn read_file(&self, rev: &str, path: &str) -> clax_git::Result<Option<Vec<u8>>> {
        let args = Object::new();
        Reflect::set(&args, &"ref".into(), &rev.into()).map_err(js_error)?;
        Reflect::set(&args, &"path".into(), &path.into()).map_err(js_error)?;
        let b = call(&self.handle, "readFile", &[args.into()]).await?;
        if b.is_null() || b.is_undefined() {
            return Ok(None);
        }
        blob_bytes(&b).await.map(Some)
    }
}

// ── Transports ──────────────────────────────────────────────────────────────

fn to_request(r: &HttpRequest) -> worker::Result<Request> {
    let headers = Headers::new();
    for (k, v) in &r.headers {
        headers.set(k, v)?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::from(r.method.to_string()))
        .with_headers(headers);
    if r.method != "GET" && r.method != "HEAD" {
        init.with_body(Some(Uint8Array::from(r.body.as_slice()).into()));
    }
    Request::new_with_init(&r.url, &init)
}

async fn from_response(mut res: worker::Response) -> worker::Result<HttpResponse> {
    let headers = res.headers().entries().collect();
    Ok(HttpResponse {
        status: res.status_code(),
        headers,
        body: res.bytes().await?,
    })
}

fn unavailable(e: worker::Error) -> GitError {
    GitError::RepoUnavailable(format!("The request failed: {e}"))
}

/// The Worker's `fetch`, for git remotes.
#[derive(Clone, Copy, Default)]
pub struct FetchTransport;

impl Transport for FetchTransport {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let req = to_request(&request).map_err(unavailable)?;
        let res = Fetch::Request(req).send().await.map_err(unavailable)?;
        from_response(res).await.map_err(unavailable)
    }
}

/// A Durable Object stub's `fetch`, for `RepoDO`'s internal API.
#[derive(Clone)]
pub struct StubTransport {
    pub stub: std::sync::Arc<Stub>,
}

impl Transport for StubTransport {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let req = to_request(&request).map_err(unavailable)?;
        let res = self
            .stub
            .fetch_with_request(req)
            .await
            .map_err(unavailable)?;
        from_response(res).await.map_err(unavailable)
    }
}

// ── R2 ──────────────────────────────────────────────────────────────────────

/// An R2 bucket binding, for artifact assets.
pub struct R2Objects {
    bucket: SendWrapper<worker::Bucket>,
}

impl R2Objects {
    pub fn new(bucket: worker::Bucket) -> R2Objects {
        R2Objects {
            bucket: SendWrapper::new(bucket),
        }
    }
}

impl Objects for R2Objects {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        self.bucket
            .put(key, bytes.to_vec())
            .execute()
            .await
            .map(drop)
            .map_err(|e| e.to_string())
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(o) = self
            .bucket
            .get(key)
            .execute()
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        match o.body() {
            Some(b) => b.bytes().await.map(Some).map_err(|e| e.to_string()),
            None => Err(format!("{key} has no body")),
        }
    }

    async fn delete(&self, keys: &[String]) -> Result<(), String> {
        if keys.is_empty() {
            return Ok(());
        }
        self.bucket
            .delete_multiple(keys.to_vec())
            .await
            .map_err(|e| e.to_string())
    }

    async fn list(&self, prefix: &str, cursor: Option<&str>) -> Result<Page, String> {
        let mut b = self.bucket.list().prefix(prefix);
        if let Some(c) = cursor {
            b = b.cursor(c);
        }
        let r = b.execute().await.map_err(|e| e.to_string())?;
        Ok(Page {
            keys: r.objects().iter().map(|o| o.key()).collect(),
            cursor: if r.truncated() { r.cursor() } else { None },
        })
    }
}

// ── Request bodies ──────────────────────────────────────────────────────────

/// The body of `req`, or `None` once it runs past `limit` bytes, read
/// through the stream's own reader: every call is caught, and a body
/// refused part-way is cancelled, so nothing the stream throws escapes
/// into the module.
pub async fn read_body_capped(
    req: &worker::Request,
    limit: usize,
) -> worker::Result<Option<Vec<u8>>> {
    let Some(stream) = req.inner().body() else {
        return Ok(Some(Vec::new()));
    };
    let fail =
        |e: GitError| worker::Error::RustError(format!("Reading the request body failed: {e}"));
    let stream: JsValue = stream.into();
    let reader = call(&stream, "getReader", &[]).await.map_err(fail)?;
    let mut body = Vec::new();
    loop {
        let step = call(&reader, "read", &[]).await.map_err(fail)?;
        if get(&step, "done").as_bool().unwrap_or(false) {
            return Ok(Some(body));
        }
        let chunk = Uint8Array::new(&get(&step, "value"));
        if body.len() + chunk.length() as usize > limit {
            let _ = call(&reader, "cancel", &[]).await;
            return Ok(None);
        }
        body.extend_from_slice(&chunk.to_vec());
    }
}

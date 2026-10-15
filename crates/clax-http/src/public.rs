//! Publishing a local artifact to a public Clax (a hosted daemon or the
//! Worker): the remote's client, its device sign-in, and the push.
//!
//! A local daemon signs in to a remote by device code
//! ([`start_sign_in`], [`poll_sign_in`]): the remote hands it a user code
//! and an approval page, a person signed in there approves this machine,
//! and the next poll receives an agent token of theirs, which the store
//! keeps ([`clax_store::remotes`]). Every later request to the remote
//! carries that token, as any agent of theirs would, through the remote's
//! REST API.
//!
//! A push ([`push`]) publishes each local version the remote has not had,
//! in order, with its label, note and files (a path the version dropped is
//! removed there), under a remote session per local agent named as the
//! local agent is: the remote records its version's publisher as the
//! person and its agent under that name. A version the owner published by
//! hand (the CLI or the browser) goes under a session of the remote's
//! by-hand harness (`crate::routes::sessions::BY_HAND_HARNESS`), which the
//! remote records as the person's own publish, by no agent. The first push
//! creates the public artifact; the link remembers it, the last local
//! version pushed and the public version that push made. When the public
//! copy has moved since (someone published there), a push is refused
//! `remote_moved` rather than overwrite it: [`Mode::New`] pushes every
//! version as a new public artifact, and [`Mode::Fork`] forks the public
//! copy at the version this daemon last pushed and continues on the fork;
//! either moves the link. Comments on the public copy stay there.
//!
//! Every version is sent with an idempotency key (the local artifact, the
//! version and the public artifact it goes to, [`version_key`]), which the
//! remote keeps: a push whose answer was lost, sent again, is answered the
//! version it made, not refused as the public copy having moved.
//!
//! Assets: an asset of the artifact that a pushed version's text files
//! reference by the address this daemon gave it (`/_blob/<asset ID>`,
//! alone or after an `http(s)://` origin) is uploaded to the public
//! artifact once, and those references are rewritten to the address the
//! remote gave it ([`rewrite_assets`]). Nothing else in a page changes;
//! an asset no pushed version references stays here. A public artifact
//! takes assets once it exists: a local version 1 cannot name its own
//! artifact's assets, so a new public artifact's first version has none
//! to rewrite; should one, it is made as it is, then published again
//! rewritten. Nothing here runs until a remote is added.

use crate::error::ApiError;
use crate::state::AppState;
use axum::http::StatusCode;
use base64::Engine as _;
use clax_store::remotes::{PendingSignIn, Remote, RemoteLink, SignedInAs};
use clax_core::{ArtifactId, Secret};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// `f`, made `Send` where a route's future must be: on wasm32 the remote's
/// client (`fetch`) is not, which is sound to wrap because a Worker has one
/// thread (the wrapper panics if polled on another).
#[cfg(not(target_arch = "wasm32"))]
pub fn sendable<F: std::future::Future>(f: F) -> F {
    f
}
/// See the native [`sendable`].
#[cfg(target_arch = "wasm32")]
pub fn sendable<F: std::future::Future>(f: F) -> send_wrapper::SendWrapper<F> {
    send_wrapper::SendWrapper::new(f)
}

/// The by-hand harness ([`crate::routes::sessions::BY_HAND_HARNESS`]).
const BY_HAND: &str = crate::routes::sessions::BY_HAND_HARNESS;

/// How long one request to a remote may take.
#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How a push treats the public copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Continue the linked public artifact (or create it on a first push);
    /// refused `remote_moved` when it moved.
    Update,
    /// Push every version as a new public artifact and link that one.
    New,
    /// Fork the public copy at the version last pushed and continue there.
    Fork,
}

impl Mode {
    /// `update`, `new` or `fork`.
    ///
    /// # Errors
    /// 400 `invalid_args` for anything else.
    pub fn parse(s: Option<&str>) -> Result<Mode, ApiError> {
        match s.unwrap_or("update") {
            "update" => Ok(Mode::Update),
            "new" => Ok(Mode::New),
            "fork" => Ok(Mode::Fork),
            _ => Err(ApiError::bad_request(
                "invalid_args",
                "mode is update, new or fork",
            )),
        }
    }
}

/// The remote's origin as `clax remote add` takes it: an `https` origin,
/// or `http` on localhost for development.
///
/// # Errors
/// 400 `invalid_args` otherwise.
pub fn origin_of(raw: &str) -> Result<String, ApiError> {
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    clax_core::config::hosted_origin(with_scheme.trim_end_matches('/')).ok_or_else(|| {
        ApiError::bad_request(
            "invalid_args",
            format!("'{raw}' is not a public Clax: give its https:// origin"),
        )
    })
}

/// A remote's default name: its origin's host (and port).
pub fn default_name(origin: &str) -> String {
    origin
        .split_once("://")
        .map(|(_, h)| h.to_string())
        .unwrap_or_else(|| origin.to_string())
}

/// What this machine calls itself on a remote's approval page: its host
/// name, as `clax connect` names it.
pub fn machine_name() -> String {
    #[cfg(unix)]
    let host = nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| h.trim_end_matches(".local").to_string())
        .filter(|h| !h.is_empty());
    #[cfg(not(unix))]
    let host: Option<String> = None;
    match host {
        Some(h) => format!("Clax on {h}"),
        None => "Clax on this machine".to_string(),
    }
}

/// The agent name the owner's own versions (published from the CLI or the
/// browser, by no agent) carry on a remote: this machine's host name as an
/// agent name (`alex-laptop`), else `local`.
pub fn own_agent_name() -> String {
    #[cfg(unix)]
    let host = nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| h.trim_end_matches(".local").to_string());
    #[cfg(not(unix))]
    let host: Option<String> = None;
    host.and_then(|h| clax_core::names::sanitize(&h))
        .filter(|n| !clax_core::names::RESERVED.contains(&n.as_str()))
        .unwrap_or_else(|| "local".to_string())
}

/// A refusal or failure of a remote's, as this daemon answers it: 502
/// `remote_unreachable` when it could not be reached, 502
/// `remote_signed_out` when it refused the token (revoked there), else 502
/// `remote_refused` naming the remote's own code and message
/// (`remote_code`, `remote_status`).
#[derive(Debug)]
pub struct RemoteFail {
    pub origin: String,
    /// `None`: not reached.
    pub status: Option<u16>,
    pub code: String,
    pub message: String,
    pub body: Value,
}

impl From<RemoteFail> for ApiError {
    fn from(f: RemoteFail) -> ApiError {
        let (code, message) = match f.status {
            None => (
                "remote_unreachable",
                format!("cannot reach {}: {}", f.origin, f.message),
            ),
            Some(401) => (
                "remote_signed_out",
                format!(
                    "{} refused this Clax's agent token (revoked there?): remove the remote and add it again",
                    f.origin
                ),
            ),
            Some(_) => (
                "remote_refused",
                format!("{} refused: {}: {}", f.origin, f.code, f.message),
            ),
        };
        let mut e = ApiError::new(StatusCode::BAD_GATEWAY, code, message);
        e.extra.insert("remote_code".into(), json!(f.code));
        if let Some(s) = f.status {
            e.extra.insert("remote_status".into(), json!(s));
        }
        e
    }
}

/// Header [`crate::routes::artifacts::IDEMPOTENCY_HEADER`] names on the
/// wire.
const IDEMPOTENCY: &str = "idempotency-key";

/// The most bytes of a remote's answer this daemon reads.
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

/// One remote's REST API, as an agent of its person (or, before sign-in,
/// as no one).
pub struct Client {
    origin: String,
    http: reqwest::Client,
    token: Option<Secret>,
    /// The remote session requests speak for: its ID and secret.
    session: Option<(String, Secret)>,
}

fn timeout(r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    #[cfg(not(target_arch = "wasm32"))]
    {
        r.timeout(REQUEST_TIMEOUT)
    }
    #[cfg(target_arch = "wasm32")]
    {
        r
    }
}

impl Client {
    /// A client of `origin`, holding `token` when signed in.
    pub fn new(origin: &str, token: Option<Secret>) -> Client {
        #[cfg(not(target_arch = "wasm32"))]
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        #[cfg(target_arch = "wasm32")]
        let http = reqwest::Client::new();
        Client {
            origin: origin.to_string(),
            http,
            token,
            session: None,
        }
    }

    /// `method` `path` with `body`, answered as JSON; a 2xx answer's body,
    /// else the remote's refusal.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, RemoteFail> {
        self.send(method, path, |r| match body {
            Some(b) => r.json(b),
            None => r,
        })
        .await
    }

    /// `method` `path`, its body and headers set by `with`, as
    /// [`Client::call`] answers.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        with: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<Value, RemoteFail> {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.origin))
            .header("accept", "application/json");
        if let Some(t) = &self.token {
            r = r.bearer_auth(t.expose());
        }
        if let Some((sid, secret)) = &self.session {
            r = r
                .header("x-clax-session", sid)
                .header(clax_core::remote::SESSION_SECRET_HEADER, secret.expose());
        }
        r = with(r);
        let fail = |status, code: &str, message: String, body| RemoteFail {
            origin: self.origin.clone(),
            status,
            code: code.to_string(),
            message,
            body,
        };
        let res = timeout(r)
            .send()
            .await
            .map_err(|e| fail(None, "unreachable", e.to_string(), Value::Null))?;
        let status = res.status().as_u16();
        if res
            .content_length()
            .is_some_and(|n| n > MAX_ANSWER_BYTES as u64)
        {
            return Err(fail(
                None,
                "unreachable",
                "the answer is too large".into(),
                Value::Null,
            ));
        }
        let bytes = res.bytes().await.unwrap_or_default();
        let v: Value = if bytes.len() > MAX_ANSWER_BYTES {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if (200..300).contains(&status) {
            return Ok(v);
        }
        let code = v["error"]["code"].as_str().unwrap_or("error").to_string();
        let message = v["error"]["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        Err(fail(Some(status), &code, message, v))
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, RemoteFail> {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    /// POST `body` to `path` with idempotency key `key`.
    async fn post_keyed(&self, path: &str, body: &Value, key: &str) -> Result<Value, RemoteFail> {
        self.send(reqwest::Method::POST, path, |r| {
            r.header(IDEMPOTENCY, key).json(body)
        })
        .await
    }

    /// Uploads `bytes` (of `content_type`) as an asset of public artifact
    /// `aid`: the address the remote serves it at.
    async fn upload(
        &self,
        aid: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<String, RemoteFail> {
        let bad = |message: &str, body| RemoteFail {
            origin: self.origin.clone(),
            status: Some(502),
            code: "bad_answer".into(),
            message: message.to_string(),
            body,
        };
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("asset")
            .mime_str(content_type)
            .map_err(|_| bad("the asset's content type is not one", Value::Null))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let v = self
            .send(
                reqwest::Method::POST,
                &format!("/api/artifacts/{aid}/assets"),
                |r| r.multipart(form),
            )
            .await?;
        let url = v["url"].as_str().unwrap_or_default();
        let url = if url.starts_with('/') {
            format!("{}{url}", self.origin)
        } else {
            url.to_string()
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(bad("the asset upload answered no address", v));
        }
        Ok(url)
    }

    async fn get(&self, path: &str) -> Result<Value, RemoteFail> {
        self.call(reqwest::Method::GET, path, None).await
    }

    /// Registers a remote session for the local agent `name` of `harness`
    /// and speaks for it from then on. The name is the agent's on the
    /// remote's versions; one the remote refuses (taken there) leaves the
    /// session the remote's own name. The by-hand harness, which a public
    /// Clax older than it refuses, falls back to `claude` there.
    async fn session_for(&mut self, harness: &str, name: &str) -> Result<(), RemoteFail> {
        self.end_session().await;
        let v = match self
            .post("/api/sessions", &json!({"harness": harness, "cwd": ""}))
            .await
        {
            Err(f) if harness == BY_HAND && f.status == Some(400) && f.code == "invalid_args" => {
                self.post("/api/sessions", &json!({"harness": "claude", "cwd": ""}))
                    .await?
            }
            v => v?,
        };
        let sid = v["session"]["id"].as_str().unwrap_or_default().to_string();
        let secret = Secret::new(v["session_secret"].as_str().unwrap_or_default().to_string());
        self.session = Some((sid.clone(), secret));
        let _ = self
            .call(
                reqwest::Method::PATCH,
                &format!("/api/sessions/{sid}"),
                Some(&json!({"name": name})),
            )
            .await;
        Ok(())
    }

    /// Ends the remote session this client speaks for, if any.
    async fn end_session(&mut self) {
        if let Some((sid, _)) = self.session.clone() {
            let _ = self
                .call(
                    reqwest::Method::PATCH,
                    &format!("/api/sessions/{sid}"),
                    Some(&json!({"ended": true})),
                )
                .await;
            self.session = None;
        }
    }
}

/// Starts a device sign-in at `origin` for this machine, named `name`.
///
/// # Errors
/// The remote's ([`RemoteFail`]); 502 `remote_refused` with `not_found`
/// when `origin` is a Clax that is not public (no device sign-in).
pub async fn start_sign_in(origin: &str, name: &str) -> Result<PendingSignIn, ApiError> {
    let c = Client::new(origin, None);
    let v = c
        .post("/api/hosted/device/code", &json!({"client_name": name}))
        .await?;
    let field = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    // The approval page this daemon opens must be the remote's own.
    let page = field("verification_uri_complete");
    if field("device_code").is_empty()
        || field("user_code").is_empty()
        || !page.starts_with(&format!("{origin}/"))
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "remote_refused",
            format!("{origin} answered no device sign-in of its own"),
        ));
    }
    Ok(PendingSignIn {
        device_code: Secret::new(field("device_code")),
        user_code: field("user_code"),
        verification_url: field("verification_uri_complete"),
        expires_at: field("expires_at"),
        interval_s: v["interval"].as_i64().unwrap_or(5),
    })
}

/// What one poll of a sign-in found.
pub enum SignIn {
    /// Still waiting (`authorization_pending` or `slow_down`).
    Waiting,
    /// Denied there, or expired: the remote is forgotten.
    Refused(&'static str),
    /// Approved: the remote is connected.
    Connected(Box<Remote>),
}

/// Polls remote `r`'s sign-in once, connecting it when approved and
/// forgetting it when denied or expired.
///
/// # Errors
/// The store's; the remote's for anything but the device flow's own
/// answers.
pub async fn poll_sign_in(s: &AppState, r: &Remote) -> Result<SignIn, ApiError> {
    let id = r.id.clone();
    let Some(code) = s.store_call(move |st| st.remote_device_code(&id)).await? else {
        return Err(ApiError::bad_request(
            "not_pending",
            "this remote is not waiting for a sign-in",
        ));
    };
    let c = Client::new(&r.origin, None);
    let out = c
        .post(
            "/api/hosted/device/token",
            &json!({"device_code": code.expose()}),
        )
        .await;
    let forget = |why: &'static str| async move {
        let id = r.id.clone();
        s.store_call(move |st| st.remove_remote(&id).map(|_| ()))
            .await?;
        Ok::<_, ApiError>(SignIn::Refused(why))
    };
    match out {
        Ok(v) => {
            let signed = SignedInAs {
                token: Secret::new(v["token"].as_str().unwrap_or_default().to_string()),
                person_public_id: v["person"]["public_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                person_name: v["person"]["display_name"].as_str().map(str::to_string),
                grant_id: v["grant_id"].as_str().unwrap_or_default().to_string(),
            };
            let id = r.id.clone();
            let r = s
                .store_call(move |st| st.connect_remote(&id, &signed))
                .await?;
            Ok(SignIn::Connected(Box::new(r)))
        }
        Err(f) if f.status == Some(400) => match f.code.as_str() {
            "authorization_pending" | "slow_down" => Ok(SignIn::Waiting),
            "access_denied" => forget("access_denied").await,
            "expired_token" => forget("expired_token").await,
            _ => Err(f.into()),
        },
        Err(f) => Err(f.into()),
    }
}

/// One local version as a push sends it.
struct Outgoing {
    n: u32,
    label: Option<String>,
    note: Option<String>,
    /// Each path's bytes and content type; `None`: the version dropped it.
    files: BTreeMap<String, Option<(Vec<u8>, String)>>,
    /// The local agent that published it: its harness and name.
    agent: (String, String),
}

/// Local versions `from..=to` of `aid` as a push sends them, each naming
/// the paths the version before it (`from - 1`, when there is one) had
/// and it dropped (as `None`, which removes them there).
fn outgoing(
    st: &clax_store::Store,
    aid: &ArtifactId,
    from: u32,
    to: u32,
) -> clax_core::Result<Vec<Outgoing>> {
    let versions = st.list_versions(aid)?;
    let mut prev: Option<Vec<String>> = versions
        .iter()
        .find(|v| from > 1 && v.n == from - 1)
        .map(|v| v.files.keys().cloned().collect());
    let mut out = Vec::new();
    for v in versions.iter().filter(|v| v.n >= from && v.n <= to) {
        let mut files = BTreeMap::new();
        for path in v.files.keys() {
            let (bytes, meta) = st
                .read_file(aid, v.n, path)?
                .ok_or(clax_core::CoreError::NotFound)?;
            files.insert(path.clone(), Some((bytes, meta.content_type)));
        }
        for gone in prev.iter().flatten() {
            if !v.files.contains_key(gone) {
                files.insert(gone.clone(), None);
            }
        }
        prev = Some(v.files.keys().cloned().collect());
        let session = match &v.session_id {
            Some(sid) => st.get_session(sid)?,
            None => None,
        };
        let agent = match session {
            Some(s) => {
                let name = s.name.clone().unwrap_or_else(|| s.harness.clone());
                (s.harness, name)
            }
            // The owner's own publish (the CLI or the browser): the
            // remote's by-hand harness, which records it as the person's.
            None => (BY_HAND.to_string(), own_agent_name()),
        };
        out.push(Outgoing {
            n: v.n,
            label: v.label.clone(),
            note: v.note.clone(),
            files,
            agent,
        });
    }
    Ok(out)
}

/// The path segment this daemon serves asset bytes under.
const BLOB: &str = "/_blob/";

/// Whether a file of `content_type` is text a page's asset references can
/// be in: HTML, CSS, JavaScript, JSON, SVG and other text.
fn textual(content_type: &str) -> bool {
    let t = content_type.split(';').next().unwrap_or("").trim();
    t.starts_with("text/") || t.ends_with("javascript") || t.ends_with("json") || t.ends_with("xml")
}

/// One reference in `text` to an asset `ids` holds: the byte range it
/// takes (with the `http(s)://<host>` before `/_blob/`, when there is one)
/// and the asset's ID.
fn asset_refs<'a>(text: &'a str, ids: &BTreeSet<&str>) -> Vec<(std::ops::Range<usize>, &'a str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(BLOB).map(|i| from + i) {
        let id_at = at + BLOB.len();
        from = id_at;
        // Asset IDs are ULIDs: 26 ASCII letters and digits.
        let Some(id) = text.get(id_at..id_at + 26) else {
            continue;
        };
        let after = text.as_bytes().get(id_at + 26).copied();
        if !ids.contains(id) || after.is_some_and(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        // After a host or a path segment, only an `http(s)://` origin
        // makes it this daemon's address.
        let joined = at > 0 && {
            let c = text.as_bytes()[at - 1];
            c.is_ascii_alphanumeric() || b".-_~%:]".contains(&c)
        };
        let start = match origin_before(text, at) {
            Some(start) => start,
            None if joined => continue,
            None => at,
        };
        out.push((start..id_at + 26, id));
        from = id_at + 26;
    }
    out
}

/// Where the `http://` or `https://` origin that ends at `at` in `text`
/// starts, if one does.
fn origin_before(text: &str, at: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = at;
    while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b".-:[]".contains(&b[i - 1])) {
        i -= 1;
    }
    let host = &text[i..at];
    if host.is_empty() || !text[..i].ends_with("//") {
        return None;
    }
    let scheme_end = i - 2;
    for scheme in ["https:", "http:"] {
        if text[..scheme_end].ends_with(scheme) {
            return Some(scheme_end - scheme.len());
        }
    }
    None
}

/// The IDs of the assets `ids` holds that text file `bytes` (of
/// `content_type`) references.
fn referenced<'a>(bytes: &[u8], content_type: &str, ids: &BTreeSet<&'a str>) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    if !textual(content_type) {
        return out;
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        for (_, id) in asset_refs(text, ids) {
            if let Some(known) = ids.get(id) {
                out.insert(*known);
            }
        }
    }
    out
}

/// Text file `bytes` (of `content_type`) with each reference to an asset
/// `urls` holds (by ID) rewritten to its address there; `None` when it has
/// none. A reference's own query (`?…`) is kept after the address's.
pub fn rewrite_assets(
    bytes: &[u8],
    content_type: &str,
    urls: &HashMap<String, String>,
) -> Option<Vec<u8>> {
    if !textual(content_type) || urls.is_empty() {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let ids: BTreeSet<&str> = urls.keys().map(String::as_str).collect();
    let refs = asset_refs(text, &ids);
    if refs.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (range, id) in refs {
        out.push_str(&text[last..range.start]);
        let url = &urls[id];
        out.push_str(url);
        last = range.end;
        if url.contains('?') && text[last..].starts_with('?') {
            out.push('&');
            last += 1;
        }
    }
    out.push_str(&text[last..]);
    Some(out.into_bytes())
}

/// The idempotency key of local version `n` of `aid` published onto
/// `target`: a public artifact's ID, or for a new public artifact `new:`
/// and the artifact the link moves from (`-` for none), so a push sent
/// again names the same key and a push as new again (`mode: new`) a fresh
/// one.
pub fn version_key(aid: &str, n: u32, target: &str) -> String {
    format!("clax-push:{aid}:{n}:{target}")
}

type PushLocks = std::sync::Mutex<
    std::collections::HashMap<(String, String), std::sync::Arc<tokio::sync::Mutex<()>>>,
>;

/// The lock of pushes of `aid` to remote `rid`. Unheld locks are dropped
/// as others are taken.
fn push_lock(aid: &str, rid: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::LazyLock<PushLocks> = std::sync::LazyLock::new(Default::default);
    let mut m = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    m.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
    m.entry((aid.to_string(), rid.to_string()))
        .or_default()
        .clone()
}

/// What a push did.
#[derive(Debug)]
pub struct Pushed {
    pub link: RemoteLink,
    /// Each local version pushed and the public version it made.
    pub versions: Vec<(u32, u32)>,
    /// How many of the artifact's assets no pushed version references:
    /// they stayed here.
    pub assets_not_pushed: usize,
}

/// 409 `remote_moved`: the public copy has versions this daemon did not
/// push.
fn moved(link: &RemoteLink, current: u32) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::CONFLICT,
        "remote_moved",
        format!(
            "the public copy is at version {current}, past version {} this Clax pushed last: someone published there since. Push as a new public artifact (mode new) or onto a fork of it (mode fork)",
            link.remote_n
        ),
    );
    e.extra.insert("remote_version".into(), json!(current));
    e.extra
        .insert("pushed_version".into(), json!(link.remote_n));
    e.extra.insert("url".into(), json!(link.url));
    e
}

/// A public artifact ID as a remote answered it: letters and digits only,
/// as every Clax artifact ID is, so it is safe in a path and an address.
fn remote_id(v: &Value, origin: &str) -> Result<String, ApiError> {
    let id = v["artifact"]["id"].as_str().unwrap_or_default();
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "remote_refused",
            format!("{origin} answered no artifact ID"),
        ));
    }
    Ok(id.to_string())
}

/// The push's state as it goes: the remote's client and session, the public
/// artifact and version it has reached, and the link as last recorded.
struct Run<'a> {
    s: &'a AppState,
    aid: &'a ArtifactId,
    remote: &'a Remote,
    c: Client,
    agent: Option<(String, String)>,
    target: Option<String>,
    at: u32,
    last: Option<RemoteLink>,
    auto: Option<bool>,
    /// The artifact's assets: each one's content type.
    assets: HashMap<String, String>,
    /// The assets uploaded to `target`: each one's address there.
    uploaded: HashMap<String, String>,
}

impl Run<'_> {
    /// Speaks as `agent` from now on.
    async fn as_agent(&mut self, agent: &(String, String)) -> Result<(), ApiError> {
        if self.agent.as_ref() != Some(agent) {
            self.c.session_for(&agent.0, &agent.1).await?;
            self.agent = Some(agent.clone());
        }
        Ok(())
    }

    /// Records that local version `local_n` made public version `n` of
    /// public artifact `tid`.
    async fn record(&mut self, tid: &str, local_n: u32, n: u32) -> Result<(), ApiError> {
        let url = format!("{}/a/{tid}", self.remote.origin);
        let (a2, r2, t2, auto) = (
            self.aid.as_str().to_string(),
            self.remote.id.clone(),
            tid.to_string(),
            self.auto,
        );
        let l = self
            .s
            .store_call(move |st| st.record_remote_push(&a2, &r2, &t2, &url, (local_n, n), auto))
            .await?;
        if self.target.as_deref() != Some(tid) {
            self.uploaded = self.uploaded_to(tid).await?;
        }
        self.last = Some(l);
        self.target = Some(tid.to_string());
        self.at = n;
        Ok(())
    }

    /// The assets of this artifact uploaded to public artifact `tid` before.
    async fn uploaded_to(&self, tid: &str) -> Result<HashMap<String, String>, ApiError> {
        let (rid, t2, ids): (String, String, Vec<String>) = (
            self.remote.id.clone(),
            tid.to_string(),
            self.assets.keys().cloned().collect(),
        );
        self.s
            .store_call(move |st| {
                let mut m = HashMap::new();
                for id in ids {
                    if let Some(u) = st.remote_asset(&rid, &t2, &id)? {
                        m.insert(id, u);
                    }
                }
                Ok(m)
            })
            .await
    }

    /// Uploads to the public artifact the assets in `ids` it lacks.
    async fn upload(&mut self, ids: &BTreeSet<String>) -> Result<(), ApiError> {
        let Some(tid) = self.target.clone() else {
            return Ok(());
        };
        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !self.uploaded.contains_key(*id))
            .cloned()
            .collect();
        for id in &missing {
            let i2 = id.clone();
            let Some((asset, bytes)) = self.s.store_call(move |st| st.read_asset(&i2)).await?
            else {
                continue;
            };
            let url = self.c.upload(&tid, bytes, &asset.content_type).await?;
            let (rid, t2, i3, u2) = (self.remote.id.clone(), tid.clone(), id.clone(), url.clone());
            self.s
                .store_call(move |st| st.record_remote_asset(&rid, &t2, &i3, &u2))
                .await?;
            self.uploaded.insert(id.clone(), url);
        }
        Ok(())
    }

    /// The asset IDs version `v`'s text files reference.
    fn refs_of(&self, v: &Outgoing) -> BTreeSet<String> {
        let ids: BTreeSet<&str> = self.assets.keys().map(String::as_str).collect();
        let mut out = BTreeSet::new();
        for (bytes, ct) in v.files.values().flatten() {
            out.extend(referenced(bytes, ct, &ids).into_iter().map(str::to_string));
        }
        out
    }

    /// Version `v`'s files as the publish API takes them, asset references
    /// rewritten to the public artifact's addresses when `rewrite`.
    fn files_of(&self, v: &Outgoing, rewrite: bool) -> BTreeMap<String, Value> {
        let b64 = base64::engine::general_purpose::STANDARD;
        v.files
            .iter()
            .map(|(path, f)| {
                let file = match f {
                    None => Value::Null,
                    Some((bytes, ct)) => {
                        let rewritten = if rewrite {
                            rewrite_assets(bytes, ct, &self.uploaded)
                        } else {
                            None
                        };
                        let bytes: &[u8] = rewritten.as_deref().unwrap_or(bytes);
                        json!({"content": b64.encode(bytes), "encoding": "base64", "content_type": ct})
                    }
                };
                (path.clone(), file)
            })
            .collect()
    }

    /// Publishes `v` (rewritten when `rewrite`): onto the public artifact,
    /// or as a new one (`new_key`: its key) with `meta`. The public
    /// artifact's ID and the version made.
    async fn send(
        &mut self,
        v: &Outgoing,
        rewrite: bool,
        meta: &Value,
        new_key: &str,
    ) -> Result<(String, u32), ApiError> {
        let mut body = json!({
            "files": self.files_of(v, rewrite),
            "title": meta["title"],
            "label": v.label,
            "note": v.note,
        });
        let res = match &self.target {
            None => {
                for k in ["description", "icon", "capabilities"] {
                    if !meta[k].is_null() {
                        body[k] = meta[k].clone();
                    }
                }
                self.c.post_keyed("/api/artifacts", &body, new_key).await
            }
            Some(t) => {
                body["if_version"] = json!(self.at);
                let key = version_key(self.aid.as_str(), v.n, t);
                self.c
                    .post_keyed(&format!("/api/artifacts/{t}/versions"), &body, &key)
                    .await
            }
        };
        let res = match res {
            Ok(r) => r,
            Err(f) if f.status == Some(409) && f.code == "conflict" && self.last.is_some() => {
                let there = f.body["error"]["current"].as_u64().unwrap_or(0) as u32;
                let l = self.last.clone().expect("checked");
                return Err(moved(&l, there));
            }
            Err(f) => return Err(f.into()),
        };
        let tid = remote_id(&res, &self.remote.origin)?;
        let n = res["version"]["n"].as_u64().unwrap_or(0) as u32;
        Ok((tid, n))
    }
}

/// Pushes local artifact `aid` to connected remote `remote` in `mode`, its
/// versions up to `upto` (all of them when `None`). `auto` sets whether
/// agents may push it again without asking (`None` keeps the link's).
///
/// # Errors
/// 404 for no such artifact; 400 `live_page` for a live page; 409
/// `remote_moved` (above); 400 `no_link` for [`Mode::Fork`] of an artifact
/// never pushed; the remote's ([`RemoteFail`]); the store's.
pub async fn push(
    s: &AppState,
    aid: &ArtifactId,
    remote: &Remote,
    mode: Mode,
    auto: Option<bool>,
    upto: Option<u32>,
) -> Result<Pushed, ApiError> {
    let rid = remote.id.clone();
    // One push of an artifact to a remote at a time: two would each see
    // the other's version as the public copy moving.
    let lock = push_lock(aid.as_str(), &rid);
    let _held = lock.lock().await;
    let (id, r2) = (aid.clone(), rid.clone());
    let (artifact, link, token, assets) = s
        .store_call(move |st| {
            let a = st
                .get_artifact(&id)?
                .ok_or(clax_core::CoreError::NotFound)?;
            let link = st.remote_link(id.as_str(), &r2)?;
            let token = st.remote_token(&r2)?;
            let assets: HashMap<String, String> = st
                .list_assets(&id)?
                .into_iter()
                .map(|a| (a.id, a.content_type))
                .collect();
            Ok((a, link, token, assets))
        })
        .await?;
    if artifact.kind != "html" {
        return Err(ApiError::bad_request(
            "live_page",
            "a live page is not pushed: it has no versions to publish",
        ));
    }
    let Some(token) = token else {
        return Err(ApiError::bad_request(
            "not_connected",
            format!(
                "this Clax is not signed in to {} yet: finish its sign-in first",
                remote.origin
            ),
        ));
    };
    let end = upto.map_or(artifact.current_version, |u| {
        u.min(artifact.current_version)
    });
    let mut run = Run {
        s,
        aid,
        remote,
        c: Client::new(&remote.origin, Some(token)),
        agent: None,
        target: None,
        at: 0,
        last: link.clone(),
        auto,
        assets,
        uploaded: HashMap::new(),
    };
    // Where the push starts: the public artifact (none: make one), its
    // version, and the first local version to send.
    let from = match (mode, &link) {
        (Mode::New, _) | (Mode::Update, None) => {
            run.last = None;
            1
        }
        (Mode::Fork, None) => {
            return Err(ApiError::bad_request(
                "no_link",
                "this artifact was never pushed here: push it first",
            ));
        }
        (Mode::Update, Some(l)) => {
            run.target = Some(l.remote_artifact_id.clone());
            run.at = l.remote_n;
            run.uploaded = run.uploaded_to(&l.remote_artifact_id).await?;
            if l.pushed_n >= end {
                // Nothing to send: still refused when the public copy moved.
                let v = run
                    .c
                    .get(&format!("/api/artifacts/{}", l.remote_artifact_id))
                    .await?;
                let there = v["artifact"]["current_version"].as_u64().unwrap_or(0) as u32;
                if there != l.remote_n {
                    return Err(moved(l, there));
                }
            }
            l.pushed_n + 1
        }
        (Mode::Fork, Some(l)) => {
            // A fork's version 1 is the public copy's version this daemon
            // made last: local version `pushed_n`, as it was sent.
            run.as_agent(&(BY_HAND.to_string(), own_agent_name()))
                .await?;
            let v = run
                .c
                .post(
                    &format!("/api/artifacts/{}/fork", l.remote_artifact_id),
                    &json!({"version": l.remote_n}),
                )
                .await?;
            let fid = remote_id(&v, &remote.origin)?;
            run.record(&fid, l.pushed_n, 1).await?;
            l.pushed_n + 1
        }
    };
    // A new public artifact's key names the one the link moves from.
    let new_target = match (mode, &link) {
        (Mode::New, Some(l)) => format!("new:{}", l.remote_artifact_id),
        _ => "new:-".to_string(),
    };
    let (id, last_n) = (aid.clone(), end);
    let versions = if from <= last_n {
        s.store_call(move |st| outgoing(st, &id, from, last_n))
            .await?
    } else {
        Vec::new()
    };
    let meta = json!({
        "title": artifact.title,
        "description": artifact.description,
        "icon": artifact.icon,
        "capabilities": artifact
            .capabilities
            .as_object()
            .filter(|m| !m.is_empty())
            .map(|_| artifact.capabilities.clone()),
    });
    let mut pushed = Vec::new();
    let mut referenced_any: BTreeSet<String> = BTreeSet::new();
    let out = async {
        for v in &versions {
            run.as_agent(&v.agent).await?;
            let refs = run.refs_of(v);
            referenced_any.extend(refs.iter().cloned());
            if run.target.is_none() && !refs.is_empty() {
                // A new public artifact takes assets once it exists: made
                // from this version as it is, then published again with
                // its assets' addresses. Recorded one version back until
                // then, so a push cut short sends it again.
                let key = version_key(aid.as_str(), v.n, &new_target);
                let (tid, n) = run.send(v, false, &meta, &key).await?;
                run.record(&tid, v.n - 1, n).await?;
            }
            run.upload(&refs).await?;
            let key = version_key(aid.as_str(), v.n, &new_target);
            let (tid, n) = run.send(v, true, &meta, &key).await?;
            run.record(&tid, v.n, n).await?;
            pushed.push((v.n, n));
        }
        Ok::<_, ApiError>(())
    }
    .await;
    run.c.end_session().await;
    out?;
    let assets_not_pushed = run
        .assets
        .keys()
        .filter(|id| !referenced_any.contains(*id) && !run.uploaded.contains_key(*id))
        .count();
    let link = match run.last {
        Some(l) => {
            // Nothing to push still records the owner's choice.
            if pushed.is_empty()
                && let Some(auto) = auto
                && auto != l.auto
            {
                let (a2, r3) = (aid.as_str().to_string(), rid.clone());
                s.store_call(move |st| st.set_link_auto(&a2, &r3, auto))
                    .await?;
                RemoteLink { auto, ..l }
            } else {
                l
            }
        }
        None => {
            let (a2, r3) = (aid.as_str().to_string(), rid.clone());
            s.store_call(move |st| st.remote_link(&a2, &r3))
                .await?
                .ok_or_else(ApiError::not_found)?
        }
    };
    Ok(Pushed {
        link,
        versions: pushed,
        assets_not_pushed,
    })
}

/// After local artifact `aid` has a new version: pushes it to every remote
/// whose link lets agents push without asking ([`RemoteLink::auto`]), in
/// the background. A refusal (the public copy moved, the remote down, the
/// token revoked there) is left in the owner's inbox (a `push` item) for a
/// push by hand.
pub fn after_publish(s: &AppState, aid: &ArtifactId) {
    if s.hosted.is_some() {
        return;
    }
    let (s, aid) = (s.clone(), aid.clone());
    clax_rt::spawn(async move {
        let a2 = aid.as_str().to_string();
        let links = match s.store_call(move |st| st.auto_links(&a2)).await {
            Ok(l) => l,
            Err(_) => return,
        };
        for l in links {
            let rid = l.remote_id.clone();
            let remote = match s.store_call(move |st| st.find_remote(&rid)).await {
                Ok(Some(r)) => r,
                _ => continue,
            };
            if let Err(e) = push(&s, &aid, &remote, Mode::Update, None, None).await {
                tracing::warn!(
                    artifact = aid.as_str(),
                    remote = remote.name.as_str(),
                    code = e.code,
                    "automatic push refused"
                );
                refused(&s, &aid, &remote, &l, &e).await;
            }
        }
    });
}

/// Leaves automatic push refusal `e` of `aid` to `remote` in the owner's
/// inbox, once per local version.
async fn refused(s: &AppState, aid: &ArtifactId, remote: &Remote, l: &RemoteLink, e: &ApiError) {
    let (id, a2) = (aid.clone(), aid.as_str().to_string());
    let rid = remote.id.clone();
    let detail = json!({
        "remote": remote.name,
        "origin": remote.origin,
        "code": e.code,
        "message": e.message,
        "url": l.url,
    });
    let text = format!("{} {} {}", remote.name, remote.origin, e.message);
    let _ = s
        .store_call(move |st| {
            let n = st.get_artifact(&id)?.map_or(0, |a| a.current_version);
            st.note_push_refused(&a2, &rid, n, detail, &text)
        })
        .await;
}

/// Revokes the agent token this daemon holds on `remote` there (`DELETE
/// /api/hosted/agent-token`).
///
/// # Errors
/// The remote's ([`RemoteFail`]).
pub async fn revoke(remote: &Remote, token: Secret) -> Result<(), ApiError> {
    let c = Client::new(&remote.origin, Some(token));
    c.call(reqwest::Method::DELETE, "/api/hosted/agent-token", None)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01K6AB7D2E3F4G5H6J7K8M9N0P";
    const B: &str = "01K6AB7D2E3F4G5H6J7K8M9N0Q";

    fn urls() -> HashMap<String, String> {
        HashMap::from([(A.to_string(), "https://c.example/_blob/X?k=1".to_string())])
    }

    #[test]
    fn asset_references_this_daemon_gave_are_rewritten_and_nothing_else() {
        let page = format!(
            "<img src=\"http://localhost:7480/_blob/{A}\"><img src='/_blob/{A}?w=2'>\
             <img src=\"http://alex-mbp.local:7480/_blob/{A}\">\
             <a href=\"/_blob/{B}\">b</a><p>/_blob/{A}X</p><p>ftp://h/_blob/{A}</p>"
        );
        let out = String::from_utf8(rewrite_assets(page.as_bytes(), "text/html", &urls()).unwrap())
            .unwrap();
        assert_eq!(
            out,
            format!(
                "<img src=\"https://c.example/_blob/X?k=1\"><img src='https://c.example/_blob/X?k=1&w=2'>\
                 <img src=\"https://c.example/_blob/X?k=1\">\
                 <a href=\"/_blob/{B}\">b</a><p>/_blob/{A}X</p><p>ftp://h/_blob/{A}</p>"
            )
        );
        // Not text, or nothing to rewrite: left as it is.
        assert!(rewrite_assets(page.as_bytes(), "image/png", &urls()).is_none());
        assert!(rewrite_assets(b"<p>none</p>", "text/html", &urls()).is_none());
        assert!(rewrite_assets(page.as_bytes(), "text/css; charset=utf-8", &urls()).is_some());
        // A path segment before it is someone else's address.
        let other = format!("<img src=\"files/_blob/{A}\">");
        assert!(rewrite_assets(other.as_bytes(), "text/html", &urls()).is_none());
    }

    #[test]
    fn a_version_key_names_its_artifact_version_and_target() {
        assert_eq!(version_key("a1", 3, "b2"), "clax-push:a1:3:b2");
        assert_ne!(
            version_key("a1", 1, "new:-"),
            version_key("a1", 1, "new:b2")
        );
    }
}

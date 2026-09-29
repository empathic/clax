//! Async HTTP client for the daemon's REST API.

use artifax_core::RegisterSession;
use artifax_core::model::Session;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Deadline for ordinary requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline for publishes and asset uploads.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(120);
/// Deadline for establishing a connection; a live daemon on loopback accepts at once.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Why a daemon call failed.
#[derive(Debug)]
pub enum ClientError {
    /// No connection: refused, not established within the connect timeout, or
    /// dropped before a response.
    Unreachable(String),
    /// Connected, but the request did not complete within its deadline; a write
    /// may or may not have taken effect.
    Timeout(String),
    /// The daemon answered with a success status but a body that is not the
    /// expected JSON.
    BadResponse(String),
    /// The daemon answered with an error status; `error` is the body's `error`
    /// object (`code`, `message`, and any extra fields such as `current`).
    Api { status: u16, error: Value },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(m) => write!(f, "daemon unreachable: {m}"),
            ClientError::Timeout(m) => write!(f, "daemon timed out: {m}"),
            ClientError::BadResponse(m) => write!(f, "bad daemon response: {m}"),
            ClientError::Api { status, error } => write!(f, "HTTP {status}: {error}"),
        }
    }
}

impl std::error::Error for ClientError {}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Where a daemon answers: `base` (`http://127.0.0.1:<port>`) for API calls,
/// `browser_base` (`http://localhost:<port>`) for URLs shown to the person, and
/// its bearer token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub base: String,
    pub browser_base: String,
    pub token: String,
}

/// Finds or starts the daemon. It blocks, so it runs on a blocking thread.
pub type Refresh = Arc<dyn Fn() -> anyhow::Result<Endpoint> + Send + Sync>;

/// A client of one daemon's REST API.
///
/// A client made with [`DaemonClient::new`] talks to a fixed endpoint. One made
/// with [`DaemonClient::managed`] finds its daemon with a [`Refresh`] and
/// registers a harness session there: lazily on first use, and again whenever a
/// request cannot connect or is refused with 401 (the daemon restarted, on a new
/// port or with a new token), after which the request is retried once.
#[derive(Clone)]
pub struct DaemonClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    state: RwLock<State>,
    managed: Option<Managed>,
}

struct Managed {
    refresh: Refresh,
    registration: RegisterSession,
    /// Held while refreshing, so concurrent failures refresh once.
    lock: tokio::sync::Mutex<()>,
}

#[derive(Clone, Default)]
struct State {
    endpoint: Option<Endpoint>,
    session_id: Option<String>,
    session: Option<Session>,
    /// True when `session` is registered with the daemon at `endpoint`.
    registered: bool,
}

impl std::fmt::Debug for DaemonClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.state();
        f.debug_struct("DaemonClient")
            .field("base", &st.endpoint.as_ref().map(|e| &e.base))
            .field("session_id", &st.session_id)
            .field("managed", &self.inner.managed.is_some())
            .finish()
    }
}

/// A snapshot of the endpoint and session one request attempt is sent with.
struct Conn<'a> {
    http: &'a reqwest::Client,
    endpoint: Endpoint,
    session_id: Option<String>,
}

impl Conn<'_> {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.endpoint.base))
            .bearer_auth(&self.endpoint.token);
        if let Some(s) = &self.session_id {
            req = req.header("x-artifax-session", s);
        }
        req
    }

    /// The path of this client's session (`/api/sessions/<id>`).
    fn session_path(&self) -> String {
        format!(
            "/api/sessions/{}",
            self.session_id.as_deref().unwrap_or_default()
        )
    }
}

/// A failed attempt, and whether a refresh may cure it: the connection was not
/// established (so the request was never sent) or the token was refused.
struct Failure {
    error: ClientError,
    refreshable: bool,
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client builds with static settings")
}

impl DaemonClient {
    /// A client for the daemon at `base`; every request carries `X-Artifax-Session`
    /// when `session_id` is set. Proxies are never used.
    pub fn new(base: String, token: String, session_id: Option<String>) -> DaemonClient {
        let endpoint = Endpoint {
            base: base.trim_end_matches('/').to_string(),
            browser_base: String::new(),
            token,
        };
        DaemonClient {
            inner: Arc::new(Inner {
                http: http_client(),
                state: RwLock::new(State {
                    endpoint: Some(endpoint),
                    session_id,
                    session: None,
                    registered: true,
                }),
                managed: None,
            }),
        }
    }

    /// A client that finds its daemon with `refresh` and registers `registration`
    /// there before its first request; see [`DaemonClient`].
    pub fn managed(refresh: Refresh, registration: RegisterSession) -> DaemonClient {
        DaemonClient {
            inner: Arc::new(Inner {
                http: http_client(),
                state: RwLock::new(State::default()),
                managed: Some(Managed {
                    refresh,
                    registration,
                    lock: tokio::sync::Mutex::new(()),
                }),
            }),
        }
    }

    fn state(&self) -> State {
        self.inner
            .state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.inner.state.write().unwrap_or_else(|e| e.into_inner()));
    }

    /// The base URL requests are sent to, once known.
    pub fn base(&self) -> Option<String> {
        self.state().endpoint.map(|e| e.base)
    }

    /// The browser base URL of a managed client's current daemon, once known.
    pub fn browser_base(&self) -> Option<String> {
        self.inner.managed.as_ref()?;
        self.state().endpoint.map(|e| e.browser_base)
    }

    /// The session a managed client last registered, once registered.
    pub fn session(&self) -> Option<Session> {
        self.state().session
    }

    /// For a managed client: finds the daemon and registers the session unless
    /// both are done. Other clients are always ready.
    pub async fn ensure_session(&self) -> Result<()> {
        if self.inner.managed.is_none() {
            return Ok(());
        }
        let st = self.state();
        if st.endpoint.is_some() && st.registered {
            return Ok(());
        }
        self.refresh(st.endpoint).await
    }

    /// Re-discovers the daemon and registers the session there, unless another
    /// caller already moved on from `stale` (the endpoint the failure was seen on).
    async fn refresh(&self, stale: Option<Endpoint>) -> Result<()> {
        let Some(m) = &self.inner.managed else {
            return Ok(());
        };
        let _guard = m.lock.lock().await;
        let st = self.state();
        if st.endpoint.is_some() && st.endpoint != stale && st.registered {
            return Ok(());
        }
        let refresh = m.refresh.clone();
        let endpoint = tokio::task::spawn_blocking(move || refresh())
            .await
            .map_err(|e| ClientError::Unreachable(format!("daemon discovery failed: {e}")))?
            .map_err(|e| ClientError::Unreachable(format!("{e:#}")))?;
        self.update(|st| {
            st.endpoint = Some(endpoint.clone());
            st.registered = false;
        });
        let conn = Conn {
            http: &self.inner.http,
            endpoint,
            session_id: None,
        };
        let res = attempt(
            conn.request(reqwest::Method::POST, "/api/sessions")
                .json(&m.registration),
        )
        .await
        .map_err(|f| f.error)?;
        let session = session_of(body_json(res).await?)?;
        self.update(|st| {
            st.session_id = Some(session.id.clone());
            st.session = Some(session);
            st.registered = true;
        });
        Ok(())
    }

    fn conn(&self) -> Result<Conn<'_>> {
        let st = self.state();
        let endpoint = st
            .endpoint
            .ok_or_else(|| ClientError::Unreachable("no daemon found".into()))?;
        Ok(Conn {
            http: &self.inner.http,
            endpoint,
            session_id: st.session_id,
        })
    }

    /// Sends the request `build` makes. A managed client first ensures its
    /// session, and on a refreshable failure refreshes and retries once; when
    /// the refresh fails, the original error is returned.
    async fn send<F>(&self, build: F) -> Result<reqwest::Response>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        self.ensure_session().await?;
        let conn = self.conn()?;
        match attempt(build(&conn)).await {
            Err(f) if f.refreshable && self.inner.managed.is_some() => {
                if self.refresh(Some(conn.endpoint)).await.is_err() {
                    return Err(f.error);
                }
                attempt(build(&self.conn()?)).await.map_err(|f| f.error)
            }
            r => r.map_err(|f| f.error),
        }
    }

    async fn json<F>(&self, build: F) -> Result<Value>
    where
        F: Fn(&Conn<'_>) -> reqwest::RequestBuilder,
    {
        body_json(self.send(build).await?).await
    }

    /// `GET /healthz`: `{version, pid, started_at}`.
    pub async fn healthz(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/healthz"))
            .await
    }

    /// `GET /api/artifacts`: `{artifacts: [..]}`, pinned first, then most recently updated.
    pub async fn list(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/api/artifacts"))
            .await
    }

    /// `GET /api/artifacts/<id>`: `{artifact, versions, owner_session}`.
    pub async fn get(&self, id: &str) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, &format!("/api/artifacts/{id}")))
            .await
    }

    /// `POST /api/artifacts`: creates an artifact; `{artifact, version, url}`.
    pub async fn create(&self, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::POST, "/api/artifacts")
                .timeout(PUBLISH_TIMEOUT)
                .json(body)
        })
        .await
    }

    /// `POST /api/artifacts/<id>/versions`: publishes a new version; `{artifact, version, url}`.
    pub async fn publish_version(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::POST,
                &format!("/api/artifacts/{id}/versions"),
            )
            .timeout(PUBLISH_TIMEOUT)
            .json(body)
        })
        .await
    }

    /// `PATCH /api/artifacts/<id>` with metadata fields; `{artifact}`.
    pub async fn patch(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::PATCH, &format!("/api/artifacts/{id}"))
                .json(body)
        })
        .await
    }

    /// `DELETE /api/artifacts/<id>`.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.send(|c| c.request(reqwest::Method::DELETE, &format!("/api/artifacts/{id}")))
            .await
            .map(|_| ())
    }

    /// `GET /api/artifacts/<id>/files`: the current version's `{files, version}`.
    pub async fn files(&self, id: &str) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, &format!("/api/artifacts/{id}/files")))
            .await
    }

    /// The stored bytes of `path` in version `n`, unwrapped
    /// (`GET /api/artifacts/<id>/versions/<n>/files/<path>`).
    pub async fn file_bytes(&self, id: &str, n: u32, path: &str) -> Result<Vec<u8>> {
        let path = format!(
            "/api/artifacts/{id}/versions/{n}/files/{}",
            encode_path(path)
        );
        let res = self
            .send(|c| c.request(reqwest::Method::GET, &path))
            .await?;
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| transport_error(e).error)
    }

    /// `POST /api/artifacts/<id>/assets` (multipart field `file`): `{asset, url}`.
    pub async fn upload_asset(
        &self,
        id: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        // Validated once up front so that building the part per attempt cannot fail.
        reqwest::multipart::Part::bytes(Vec::new())
            .mime_str(content_type)
            .map_err(|e| ClientError::Api {
                status: 400,
                error: json!({"code": "invalid_content_type", "message": e.to_string()}),
            })?;
        self.json(|c| {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(filename.to_string())
                .mime_str(content_type)
                .expect("content type validated above");
            c.request(
                reqwest::Method::POST,
                &format!("/api/artifacts/{id}/assets"),
            )
            .timeout(PUBLISH_TIMEOUT)
            .multipart(reqwest::multipart::Form::new().part("file", part))
        })
        .await
    }

    /// `PATCH /api/sessions/<id>` `{"heartbeat": true}` for a managed client's
    /// session, registering it first when needed.
    pub async fn heartbeat(&self) -> Result<Session> {
        let res = self
            .json(|c| {
                c.request(reqwest::Method::PATCH, &c.session_path())
                    .json(&json!({"heartbeat": true}))
            })
            .await?;
        session_of(res)
    }

    /// `PATCH /api/sessions/<id>` `{"ended": true}` for the registered session,
    /// in one attempt: ending never starts or re-discovers a daemon. `None`
    /// when no session was registered.
    pub async fn end_session(&self) -> Result<Option<Session>> {
        if self.state().session.is_none() {
            return Ok(None);
        }
        let conn = self.conn()?;
        let res = attempt(
            conn.request(reqwest::Method::PATCH, &conn.session_path())
                .json(&json!({"ended": true})),
        )
        .await
        .map_err(|f| f.error)?;
        session_of(body_json(res).await?).map(Some)
    }
}

/// Sends `req`; a non-success status becomes `ClientError::Api` with the body's
/// `error` object.
async fn attempt(req: reqwest::RequestBuilder) -> std::result::Result<reqwest::Response, Failure> {
    let res = req.send().await.map_err(transport_error)?;
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body: Value = res.json().await.unwrap_or(Value::Null);
    let error = match body.get("error") {
        Some(e) if e.is_object() => e.clone(),
        _ => json!({
            "code": "http_error",
            "message": format!("daemon answered HTTP {}", status.as_u16()),
        }),
    };
    Err(Failure {
        refreshable: status == reqwest::StatusCode::UNAUTHORIZED,
        error: ClientError::Api {
            status: status.as_u16(),
            error,
        },
    })
}

async fn body_json(res: reqwest::Response) -> Result<Value> {
    if res.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(json!({}));
    }
    let bytes = res.bytes().await.map_err(|e| transport_error(e).error)?;
    serde_json::from_slice(&bytes).map_err(|e| ClientError::BadResponse(e.to_string()))
}

/// The `session` of a sessions route's response body.
fn session_of(mut res: Value) -> Result<Session> {
    serde_json::from_value(res["session"].take())
        .map_err(|e| ClientError::BadResponse(format!("session: {e}")))
}

/// Classifies a reqwest failure: a failed connection is `Unreachable` (even when
/// it was the connect timeout) and refreshable; any other timeout is `Timeout`;
/// anything else (such as a connection dropped mid-request) is `Unreachable`
/// but not retried, since the request may have taken effect.
fn transport_error(e: reqwest::Error) -> Failure {
    if e.is_connect() {
        Failure {
            error: ClientError::Unreachable(e.to_string()),
            refreshable: true,
        }
    } else if e.is_timeout() {
        Failure {
            error: ClientError::Timeout(e.to_string()),
            refreshable: false,
        }
    } else {
        Failure {
            error: ClientError::Unreachable(e.to_string()),
            refreshable: false,
        }
    }
}

/// Percent-encodes every byte of `path` outside the RFC 3986 unreserved set,
/// keeping `/` as the segment separator.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode_path_escapes_reserved_bytes_but_not_slashes() {
        assert_eq!(super::encode_path("a/b c#?.js"), "a/b%20c%23%3F.js");
        assert_eq!(super::encode_path("é"), "%C3%A9");
    }
}

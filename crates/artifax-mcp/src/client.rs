//! Async HTTP client for the daemon's REST API.

use serde_json::{Value, json};
use std::time::Duration;

/// Deadline for ordinary requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline for publishes and asset uploads.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a daemon call failed.
#[derive(Debug)]
pub enum ClientError {
    /// No HTTP response arrived: the connection was refused or dropped, or the
    /// request timed out.
    Unreachable(String),
    /// The daemon answered with an error status; `error` is the body's `error`
    /// object (`code`, `message`, and any extra fields such as `current`).
    Api { status: u16, error: Value },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(m) => write!(f, "daemon unreachable: {m}"),
            ClientError::Api { status, error } => write!(f, "HTTP {status}: {error}"),
        }
    }
}

impl std::error::Error for ClientError {}

pub type Result<T> = std::result::Result<T, ClientError>;

/// A connection to one daemon: its base URL (`http://127.0.0.1:<port>`), bearer
/// token, and the session that publishes are attributed to, if any.
#[derive(Clone, Debug)]
pub struct DaemonClient {
    base: String,
    token: String,
    session_id: Option<String>,
    http: reqwest::Client,
}

impl DaemonClient {
    /// A client for the daemon at `base`; every request carries `X-Artifax-Session`
    /// when `session_id` is set. Proxies are never used.
    pub fn new(base: String, token: String, session_id: Option<String>) -> DaemonClient {
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client builds with static settings");
        DaemonClient {
            base: base.trim_end_matches('/').to_string(),
            token,
            session_id,
            http,
        }
    }

    /// The base URL requests are sent to.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(s) = &self.session_id {
            req = req.header("x-artifax-session", s);
        }
        req
    }

    async fn send(req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let res = req
            .send()
            .await
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
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
        Err(ClientError::Api {
            status: status.as_u16(),
            error,
        })
    }

    async fn json(req: reqwest::RequestBuilder) -> Result<Value> {
        let res = Self::send(req).await?;
        if res.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({}));
        }
        res.json()
            .await
            .map_err(|e| ClientError::Unreachable(format!("reading response: {e}")))
    }

    /// `GET /healthz`: `{version, pid, started_at}`.
    pub async fn healthz(&self) -> Result<Value> {
        Self::json(self.request(reqwest::Method::GET, "/healthz")).await
    }

    /// `GET /api/artifacts`: `{artifacts: [..]}`, pinned first, then most recently updated.
    pub async fn list(&self) -> Result<Value> {
        Self::json(self.request(reqwest::Method::GET, "/api/artifacts")).await
    }

    /// `GET /api/artifacts/<id>`: `{artifact, versions, owner_session}`.
    pub async fn get(&self, id: &str) -> Result<Value> {
        Self::json(self.request(reqwest::Method::GET, &format!("/api/artifacts/{id}"))).await
    }

    /// `POST /api/artifacts`: creates an artifact; `{artifact, version, url}`.
    pub async fn create(&self, body: &Value) -> Result<Value> {
        Self::json(
            self.request(reqwest::Method::POST, "/api/artifacts")
                .timeout(PUBLISH_TIMEOUT)
                .json(body),
        )
        .await
    }

    /// `POST /api/artifacts/<id>/versions`: publishes a new version; `{artifact, version, url}`.
    pub async fn publish_version(&self, id: &str, body: &Value) -> Result<Value> {
        Self::json(
            self.request(
                reqwest::Method::POST,
                &format!("/api/artifacts/{id}/versions"),
            )
            .timeout(PUBLISH_TIMEOUT)
            .json(body),
        )
        .await
    }

    /// `PATCH /api/artifacts/<id>` with metadata fields; `{artifact}`.
    pub async fn patch(&self, id: &str, body: &Value) -> Result<Value> {
        Self::json(
            self.request(reqwest::Method::PATCH, &format!("/api/artifacts/{id}"))
                .json(body),
        )
        .await
    }

    /// `DELETE /api/artifacts/<id>`.
    pub async fn delete(&self, id: &str) -> Result<()> {
        Self::send(self.request(reqwest::Method::DELETE, &format!("/api/artifacts/{id}")))
            .await
            .map(|_| ())
    }

    /// `GET /api/artifacts/<id>/files`: the current version's `{files, version}`.
    pub async fn files(&self, id: &str) -> Result<Value> {
        Self::json(self.request(reqwest::Method::GET, &format!("/api/artifacts/{id}/files"))).await
    }

    /// The stored bytes of `path` in version `n`, unwrapped
    /// (`GET /api/artifacts/<id>/versions/<n>/files/<path>`).
    pub async fn file_bytes(&self, id: &str, n: u32, path: &str) -> Result<Vec<u8>> {
        let res = Self::send(self.request(
            reqwest::Method::GET,
            &format!(
                "/api/artifacts/{id}/versions/{n}/files/{}",
                encode_path(path)
            ),
        ))
        .await?;
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ClientError::Unreachable(format!("reading response: {e}")))
    }

    /// `POST /api/artifacts/<id>/assets` (multipart field `file`): `{asset, url}`.
    pub async fn upload_asset(
        &self,
        id: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(content_type)
            .map_err(|e| ClientError::Api {
                status: 400,
                error: json!({"code": "invalid_content_type", "message": e.to_string()}),
            })?;
        let form = reqwest::multipart::Form::new().part("file", part);
        Self::json(
            self.request(
                reqwest::Method::POST,
                &format!("/api/artifacts/{id}/assets"),
            )
            .timeout(PUBLISH_TIMEOUT)
            .multipart(form),
        )
        .await
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

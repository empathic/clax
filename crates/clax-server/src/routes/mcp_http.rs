//! The Clax tool set over MCP Streamable HTTP with JSON responses and no
//! session, through `clax_mcp::http` rather than rmcp's transport: what a
//! Durable Object serves at `/mcp`, where rmcp's service cannot run. The
//! daemon serves it at [`PATH`] beside rmcp's `/mcp`, so both are tested
//! against one suite.
//!
//! It admits what `/mcp` admits, the daemon token, and also a person's
//! agent token (`Authorization: Bearer cxa_…`, a bearer grant a member
//! minted in a hosted daemon's browser; [`McpCaller`]). An agent signing
//! with `Clax-Grant` is refused 403 `forbidden`, as on `/mcp`: a signature
//! covers one request, so the tools' requests could not carry it. It has
//! the same `Host` names admitted, the same body limit, and sits outside
//! the request timeout layers, as a tool may wait. The tools reach the
//! routes through the daemon's own router in-process
//! ([`crate::loopback::Loopback`]), not over HTTP, each request carrying
//! the MCP caller's own `Authorization` ([`ClaxTools::for_caller`]) and
//! never the daemon's token, so a tool call has the rights its caller has
//! and no more.
//!
//! The daemon token's tools act as the sessionless agent on `mcp`, as on
//! `/mcp`, and no `Mcp-Session-Id` is issued. An agent token's are an
//! agent of another machine's ([`ClaxTools::for_remote_agent`]: no file of
//! this machine is read for it, no browser opened) and have sessions
//! ([`serve_agent`]): `initialize` registers a harness session for the
//! token's grant, named after the MCP client (`clientInfo.name`), and
//! answers `Mcp-Session-Id: <session ID>.<session secret>`: the session's
//! own credential (spec 2026-10-08 §6.8, §6.9), which the token alone does
//! not give, so one MCP client of a token cannot act in another's session.
//! A request naming it acts in that session, proving it with the secret
//! (the tools that need a session, such as `working` and
//! `wait_for_feedback`, have it, publishes are its, and its narrowing
//! binds it), and `DELETE` ends it. Each request
//! marks its session seen; the reaper ends one unseen for
//! `Store::REMOTE_SESSION_IDLE_SECS`, and a token holds at most
//! `Store::BEARER_SESSION_CAP` live sessions (a new one ends the least
//! recently seen). A session that is not the token's, or has ended,
//! answers 404, which tells the client to initialize again. A request naming no session acts as the grant's
//! sessionless agent, which reads but does not write.

use crate::auth::RequireToken;
use crate::routes::artifacts::PUBLISH_BODY_LIMIT;
use crate::state::AppState;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use clax_mcp::{ClaxTools, DaemonClient};
use std::sync::Arc;

/// Where the adapter answers: `/mcp` on a Durable Object; beside rmcp's
/// `/mcp` in the daemon.
#[cfg(not(target_arch = "wasm32"))]
pub const PATH: &str = "/mcp/json";
#[cfg(target_arch = "wasm32")]
pub const PATH: &str = "/mcp";

/// Who may call the adapter: the daemon token's holder, as on `/mcp`, or
/// an agent token's agent (a bearer grant that
/// [`crate::agent::authenticate`] resolved). Any other agent credential is
/// 403 `forbidden`; no credential, or another token, 401.
#[derive(Clone, Debug)]
pub enum McpCaller {
    Token,
    Agent(crate::agent::BearerGrant),
}

impl axum::extract::FromRequestParts<AppState> for McpCaller {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if crate::auth::is_bearer_grant_request(&parts.headers) {
            return parts
                .extensions
                .get::<crate::agent::BearerGrant>()
                .cloned()
                .map(McpCaller::Agent)
                .ok_or_else(crate::error::ApiError::unauthorized);
        }
        RequireToken::from_request_parts(parts, state).await?;
        Ok(McpCaller::Token)
    }
}

/// The adapter's gate ([`McpCaller`]), which hands the caller on.
async fn require_token(caller: McpCaller, mut req: Request, next: Next) -> Response {
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// The `Host` names the endpoint answers to: the loopback names and the
/// daemon's own authority, as rmcp's service is configured for `/mcp`.
fn allowed_hosts(state: &AppState) -> Vec<String> {
    let own = state
        .self_base
        .split_once("://")
        .map_or(state.self_base.as_str(), |(_, a)| a);
    ["localhost", "127.0.0.1", "::1", own]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Whether `host` (a `Host` header) is one of `allowed`, compared as rmcp
/// compares them: the name without brackets and ignoring case, and the
/// port only when the allowed entry names one.
fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let split = |a: &str| -> Option<(String, Option<u16>)> {
        let norm = |h: &str| h.trim_matches(['[', ']']).to_ascii_lowercase();
        match axum::http::uri::Authority::try_from(a) {
            Ok(auth) => Some((norm(auth.host()), auth.port_u16())),
            Err(_) if !a.is_empty() => Some((norm(a), None)),
            Err(_) => None,
        }
    };
    let Some((name, port)) = split(host) else {
        return false;
    };
    allowed
        .iter()
        .filter_map(|a| split(a.trim()))
        .any(|(n, p)| n == name && p.is_none_or(|p| Some(p) == port))
}

/// The tool set, shared by the requests. On wasm32 it holds rmcp's `local`
/// tool router, which is not `Send`, so it is wrapped to be (sound because
/// a Worker has one thread; the wrapper panics if used on another).
#[cfg(not(target_arch = "wasm32"))]
type Shared = Arc<ClaxTools>;
#[cfg(target_arch = "wasm32")]
type Shared = Arc<send_wrapper::SendWrapper<ClaxTools>>;

#[cfg(not(target_arch = "wasm32"))]
fn local(tools: ClaxTools) -> ClaxTools {
    tools
}
#[cfg(target_arch = "wasm32")]
fn local(tools: ClaxTools) -> send_wrapper::SendWrapper<ClaxTools> {
    send_wrapper::SendWrapper::new(tools)
}

fn tools_of(shared: &Shared) -> &ClaxTools {
    shared
}

/// The adapter's tool set: the sessionless agent on `mcp`, reaching the
/// router in-process. Its client holds no token: each request's tools act
/// for its caller ([`ClaxTools::for_caller`]).
pub fn tools(state: &AppState) -> ClaxTools {
    let loopback = Arc::new(crate::loopback::Loopback::new(state.clone()));
    ClaxTools::new(
        DaemonClient::new(state.self_base.clone(), String::new(), None)
            .with_via(clax_mcp::client::VIA)
            .with_in_process(loopback),
        state.browser_base.clone(),
        None,
        state.home.log_path(),
    )
}

/// The header that names an MCP session.
pub const SESSION_HEADER: &str = "mcp-session-id";

/// A plain-text answer, as the transport answers what it refuses.
fn plain(status: axum::http::StatusCode, text: &str) -> axum::http::Response<axum::body::Bytes> {
    axum::http::Response::builder()
        .status(status)
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )
        .body(axum::body::Bytes::from(text.to_string()))
        .expect("static response parts")
}

/// The `clientInfo` of a JSON-RPC `initialize` request in `body`, or
/// `None` for any other message.
fn initialize_client(body: &[u8]) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    (v["method"] == "initialize" && v.get("id").is_some())
        .then(|| v["params"]["clientInfo"].clone())
}

/// The harness session an MCP client named `client` registers: of the
/// harness its name names (`claude` when it names none), named after the
/// client ([`clax_core::names::derive`] reads it as a repository name).
fn registration(client: &serde_json::Value) -> clax_core::RegisterSession {
    let name = client["name"].as_str().unwrap_or("").trim();
    let lower = name.to_ascii_lowercase();
    let harness = if lower.contains("codex") {
        "codex"
    } else if lower.contains("grok") {
        "grok"
    } else if lower == "pi" || lower.starts_with("pi-") {
        "pi"
    } else {
        "claude"
    };
    clax_core::RegisterSession {
        harness: harness.to_string(),
        harness_session_id: None,
        cwd: String::new(),
        pid: None,
        parent_pid: None,
        transcript_path: None,
        grant_id: None,
        repo: (!name.is_empty()).then(|| name.chars().take(64).collect()),
        branch: None,
    }
}

/// The `Mcp-Session-Id` an agent token's MCP client is given for session
/// `id`: its ID and, when it has one, its secret (spec 2026-10-08 §6.9).
fn session_header(id: &str, secret: Option<&clax_core::secret::Secret>) -> String {
    match secret {
        Some(s) => format!("{id}.{}", s.expose()),
        None => id.to_string(),
    }
}

/// The session ID and secret an `Mcp-Session-Id` carries
/// ([`session_header`]), or `None` when it is not one: a ULID's characters,
/// then optionally `.` and a session secret's (`cxs_` and base64url).
fn parse_session_header(v: &str) -> Option<(String, Option<clax_core::secret::Secret>)> {
    let (sid, secret) = match v.split_once('.') {
        Some((sid, secret)) => (sid, Some(secret)),
        None => (v, None),
    };
    let id_ok =
        !sid.is_empty() && sid.len() <= 64 && sid.bytes().all(|b| b.is_ascii_alphanumeric());
    let secret_ok = secret.is_none_or(|s| {
        s.len() <= 128
            && s.starts_with("cxs_")
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    });
    (id_ok && secret_ok).then(|| {
        (
            sid.to_string(),
            secret.map(|s| clax_core::secret::Secret::new(s.to_string())),
        )
    })
}

/// An agent token's request ([`McpCaller::Agent`]), with `tools` acting
/// for it: in the session `session` names when it names one, proved by
/// the secret it carries (404 for a session that is not the token's, has
/// ended, or is not proved; `DELETE` ends it), and an `initialize`
/// registering a new session for the client, whose ID and secret the
/// answer's `Mcp-Session-Id` carries ([`session_header`]).
async fn serve_agent(
    tools: ClaxTools,
    session: Option<String>,
    req: axum::http::Request<axum::body::Bytes>,
) -> axum::http::Response<axum::body::Bytes> {
    use axum::http::{Method, StatusCode};
    if let Some(client) = initialize_client(req.body()) {
        let res = clax_mcp::http::handle(&tools, req).await;
        if res.status() != StatusCode::OK {
            return res;
        }
        return match tools
            .client()
            .register_session(&registration(&client))
            .await
        {
            Ok((s, secret)) => {
                let mut res = res;
                match axum::http::HeaderValue::from_str(&session_header(&s.id, secret.as_ref())) {
                    Ok(v) => {
                        res.headers_mut().insert(SESSION_HEADER, v);
                        res
                    }
                    Err(_) => plain(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "a session ID that is no header",
                    ),
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "registering an MCP client's session failed");
                plain(
                    StatusCode::FORBIDDEN,
                    &format!("Forbidden: this agent token cannot register a session: {e}"),
                )
            }
        };
    }
    let Some(session) = session else {
        return clax_mcp::http::handle(&tools, req).await;
    };
    // A session ID is a ULID: anything else names no session, and never
    // reaches a route's path.
    let Some((sid, secret)) = parse_session_header(&session) else {
        return plain(StatusCode::NOT_FOUND, "Not Found: no such session");
    };
    // Each request marks its session seen, so the reaper ends only one its
    // client left (`Store::REMOTE_SESSION_IDLE_SECS` unseen); one it does
    // not prove is refused as one that does not exist.
    let found = tools
        .client()
        .heartbeat_session(&sid, secret.as_ref())
        .await;
    let Some(s) = found.ok().filter(|s| s.ended_at.is_none()) else {
        return plain(StatusCode::NOT_FOUND, "Not Found: no such session");
    };
    let tools = tools.in_session(s, secret);
    if req.method() == Method::DELETE {
        return match tools.client().end_session().await {
            Ok(_) => plain(StatusCode::NO_CONTENT, ""),
            Err(_) => plain(StatusCode::NOT_FOUND, "Not Found: no such session"),
        };
    }
    clax_mcp::http::handle(&tools, req).await
}

pub fn router(state: &AppState) -> Router<AppState> {
    let tools = Shared::new(local(tools(state)));
    let allowed = Arc::new(allowed_hosts(state));
    let serve = move |req: Request| {
        let tools = tools.clone();
        let allowed = allowed.clone();
        async move {
            let host = req
                .headers()
                .get(axum::http::header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            if !host_allowed(host, &allowed) {
                return (
                    axum::http::StatusCode::FORBIDDEN,
                    "Forbidden: Host header is not allowed",
                )
                    .into_response();
            }
            // The caller's credential, which every request of its tools
            // carries.
            let authorization = crate::loopback::caller_credential(req.headers());
            let agent = matches!(
                req.extensions().get::<McpCaller>(),
                Some(McpCaller::Agent(_))
            );
            let session = req
                .headers()
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            // An agent token's body is in memory already, read through the
            // agent gate ([`crate::agent::verify_layer`]).
            let (parts, body) = req.into_parts();
            let body = match axum::body::to_bytes(body, PUBLISH_BODY_LIMIT).await {
                Ok(b) => b,
                Err(_) => {
                    return (
                        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                        format!(
                            "Payload Too Large: request body exceeds {PUBLISH_BODY_LIMIT} bytes"
                        ),
                    )
                        .into_response();
                }
            };
            let req = axum::http::Request::from_parts(parts, body);
            // The tools' futures await `fetch` on wasm32, which is not
            // `Send`; a Worker has one thread.
            let res = clax_rt::sendable(async move {
                let tools = tools_of(&tools).for_caller(authorization);
                if agent {
                    serve_agent(tools.for_remote_agent(), session, req).await
                } else {
                    clax_mcp::http::handle(&tools, req).await
                }
            })
            .await;
            res.map(axum::body::Body::from)
        }
    };
    Router::new()
        .route_service(PATH, axum::routing::any(serve))
        .layer(DefaultBodyLimit::max(PUBLISH_BODY_LIMIT))
        .route_layer(from_fn_with_state(state.clone(), require_token))
}

#[cfg(test)]
mod tests {
    use super::{host_allowed, parse_session_header, session_header};

    #[test]
    fn session_headers_carry_the_session_and_its_secret() {
        let secret = clax_core::secret::Secret::new("cxs_abc-DEF_123".to_string());
        let v = session_header("01J0ABC", Some(&secret));
        assert_eq!(v, "01J0ABC.cxs_abc-DEF_123");
        let (sid, got) = parse_session_header(&v).unwrap();
        assert_eq!(sid, "01J0ABC");
        assert_eq!(got.unwrap().expose(), "cxs_abc-DEF_123");
        assert_eq!(
            parse_session_header("01J0ABC")
                .unwrap()
                .1
                .map(|s| s.expose().to_string()),
            None
        );
        for bad in [
            "",
            ".cxs_a",
            "01J/0",
            "01J0.abc",
            "01J0.cxs_a.b",
            "01J0.cxs_a b",
        ] {
            assert!(parse_session_header(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn hosts_are_matched_as_rmcp_matches_them() {
        let allowed: Vec<String> = ["localhost", "127.0.0.1", "::1", "10.0.0.5:7480"]
            .map(String::from)
            .to_vec();
        for ok in [
            "localhost",
            "LOCALHOST:7480",
            "127.0.0.1:1234",
            "[::1]:7480",
            "10.0.0.5:7480",
        ] {
            assert!(host_allowed(ok, &allowed), "{ok}");
        }
        for no in [
            "",
            "evil.com",
            "10.0.0.5:80",
            "10.0.0.5",
            "localhost.evil.com",
        ] {
            assert!(!host_allowed(no, &allowed), "{no}");
        }
    }
}

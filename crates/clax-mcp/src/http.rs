//! MCP over Streamable HTTP, answered without rmcp's transport: JSON
//! responses, no SSE stream and no session, for a host where rmcp's
//! service cannot run (a Durable Object: it spawns each session and each
//! stateless request on tokio).
//!
//! [`handle`] takes one HTTP request to the MCP endpoint and gives its
//! response. The host authenticates the request and checks its `Host`
//! before calling it, as it does for rmcp's service.
//!
//! - `POST` carries one JSON-RPC message. A request is answered in an
//!   `application/json` body; a notification or a response is `202
//!   Accepted` with no body. A batch (an array) is refused `400` with
//!   `-32600`: MCP dropped batching in 2025-06-18, rmcp's service reads one
//!   message only, and a batch would run any number of tool calls under
//!   one HTTP request.
//! - `GET` (the server-to-client stream) and `DELETE` (ending a session)
//!   are `405 Method Not Allowed`: there is no stream and no session.
//! - No `Mcp-Session-Id` is issued, and one a client sends is ignored. The
//!   tools hold no per-client state (the daemon's `/mcp` acts as the
//!   sessionless agent whatever rmcp session a client holds), so a session
//!   would name nothing. MCP clients treat a server that issues none as
//!   stateless: the TypeScript SDK (Claude Code) and rmcp's client (Codex,
//!   `allow_stateless` by default) send no session header then.
//!
//! The methods are answered as rmcp's server handler answers them for
//! [`ClaxTools`]: `initialize` through [`ServerHandler::negotiate_initialize`]
//! (the same server info, instructions and version negotiation),
//! `server/discover`, `ping`, `tools/list` from [`ClaxTools::tools`] (the
//! tool router's list) and `tools/call` through [`ClaxTools::call_direct`].
//! A request that names its protocol version in `_meta` (2026-07-28's
//! per-request lifecycle) is checked against the supported versions and
//! for the metadata that version requires, and an error answering it gets
//! the HTTP status rmcp gives it. rmcp's checks of the 2026-07-28 standard
//! headers (`Mcp-Method`, `Mcp-Param-*`) are not made.

use crate::tools::ClaxTools;
use bytes::Bytes;
use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::model::CacheScope;
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, CompleteResult, DiscoverResult, ErrorCode, GetMeta,
    JsonRpcMessage, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, ProtocolVersion, RequestId, ResultType, ServerJsonRpcMessage, ServerResult,
};
use serde_json::Value;

/// The header that names the protocol version of a request after
/// `initialize`.
const PROTOCOL_HEADER: &str = "mcp-protocol-version";
const JSON: &str = "application/json";

/// The response to `req`, an HTTP request to the MCP endpoint.
pub async fn handle(tools: &ClaxTools, req: http::Request<Bytes>) -> http::Response<Bytes> {
    let (parts, body) = req.into_parts();
    if parts.method != http::Method::POST {
        let Err(mut r) = plain(http::StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed") else {
            unreachable!("a refusal is an error");
        };
        r.headers_mut()
            .insert(http::header::ALLOW, http::HeaderValue::from_static("POST"));
        return *r;
    }
    match post(tools, &parts.headers, &body).await {
        Ok(r) => r,
        Err(r) => *r,
    }
}

/// A refusal is the response that answers the request instead.
type HttpResult<T> = Result<T, Box<http::Response<Bytes>>>;

/// A plain-text response, as rmcp answers what it refuses before reading
/// a message. Always `Err`, so a refusal can be returned with `?`.
fn plain(status: http::StatusCode, text: impl Into<String>) -> HttpResult<http::Response<Bytes>> {
    Err(Box::new(
        http::Response::builder()
            .status(status)
            .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Bytes::from(text.into()))
            .expect("static response parts"),
    ))
}

fn json_response(status: http::StatusCode, body: &impl serde::Serialize) -> http::Response<Bytes> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, JSON)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("JSON-RPC messages serialize"),
        ))
        .expect("static response parts")
}

/// A JSON-RPC error answered with HTTP 400 before any method runs.
fn rpc_refusal(id: Option<RequestId>, error: ErrorData) -> Box<http::Response<Bytes>> {
    Box::new(json_response(
        http::StatusCode::BAD_REQUEST,
        &ServerJsonRpcMessage::error(error, id),
    ))
}

/// Whether an `Accept` header admits a JSON body. A client that sends none
/// accepts anything.
fn accepts_json(headers: &http::HeaderMap) -> bool {
    let values: Vec<&str> = headers
        .get_all(http::header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    values.is_empty()
        || values.iter().flat_map(|v| v.split(',')).any(|range| {
            let media = range.split(';').next().unwrap_or("").trim();
            media.eq_ignore_ascii_case(JSON)
                || media.eq_ignore_ascii_case("application/*")
                || media == "*/*"
        })
}

async fn post(
    tools: &ClaxTools,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> HttpResult<http::Response<Bytes>> {
    if !accepts_json(headers) {
        plain(
            http::StatusCode::NOT_ACCEPTABLE,
            "Not Acceptable: this server answers application/json",
        )?;
    }
    let content_type = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.starts_with(JSON) {
        plain(
            http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unsupported Media Type: Content-Type must be application/json",
        )?;
    }
    let header_version = match headers.get(PROTOCOL_HEADER) {
        None => None,
        Some(v) => Some(v.to_str().map_err(|_| {
            plain(
                http::StatusCode::BAD_REQUEST,
                "Bad Request: Invalid MCP-Protocol-Version header encoding",
            )
            .unwrap_err()
        })?),
    };
    let value: Value = serde_json::from_slice(body).map_err(|e| {
        rpc_refusal(
            None,
            ErrorData::new(ErrorCode::PARSE_ERROR, format!("Parse error: {e}"), None),
        )
    })?;
    match value {
        Value::Array(_) => Err(rpc_refusal(
            None,
            ErrorData::invalid_request("Invalid Request: JSON-RPC batches are not supported", None),
        )),
        item => {
            let answer = one(tools, header_version, item).await?;
            Ok(match answer.reply {
                None => accepted(),
                Some(reply) => {
                    let status = if answer.per_request {
                        http_status(&reply)
                    } else {
                        http::StatusCode::OK
                    };
                    json_response(status, &reply)
                }
            })
        }
    }
}

fn accepted() -> http::Response<Bytes> {
    http::Response::builder()
        .status(http::StatusCode::ACCEPTED)
        .body(Bytes::new())
        .expect("static response parts")
}

/// The HTTP status of an answer to a request on the per-request lifecycle:
/// rmcp's mapping of its errors.
fn http_status(reply: &ServerJsonRpcMessage) -> http::StatusCode {
    let JsonRpcMessage::Error(e) = reply else {
        return http::StatusCode::OK;
    };
    match e.error.code {
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION
        | ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY
        | ErrorCode::INVALID_PARAMS
        | ErrorCode::HEADER_MISMATCH => http::StatusCode::BAD_REQUEST,
        ErrorCode::METHOD_NOT_FOUND => http::StatusCode::NOT_FOUND,
        _ => http::StatusCode::OK,
    }
}

/// What one message of a body gets: the reply to a request (none for a
/// notification or a response), and whether the request named its
/// protocol version itself (2026-07-28's per-request lifecycle).
struct Answer {
    reply: Option<ServerJsonRpcMessage>,
    per_request: bool,
}

/// The id of a message that did not parse, when it has a usable one.
fn raw_id(item: &Value) -> Option<RequestId> {
    item.get("id")
        .and_then(|id| serde_json::from_value(id.clone()).ok())
}

async fn one(tools: &ClaxTools, header_version: Option<&str>, item: Value) -> HttpResult<Answer> {
    let method = item
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let message: ClientJsonRpcMessage = match serde_json::from_value(item.clone()) {
        Ok(m) => m,
        Err(e) => {
            let v2 = item.get("jsonrpc").and_then(Value::as_str) == Some("2.0");
            let has_id = item.get("id").is_some();
            let id = raw_id(&item);
            let response = item.get("result").is_some() || item.get("error").is_some();
            let reply = match (v2, method.is_some(), has_id, id) {
                // A request whose params do not parse.
                (true, true, true, Some(id)) => Some(ServerJsonRpcMessage::error(
                    ErrorData::invalid_params(format!("Invalid params: {e}"), None),
                    Some(id),
                )),
                // A notification that does not parse is dropped, as one
                // that does is.
                (true, true, false, _) => None,
                // A response from the client (to nothing this server sent).
                (_, false, true, _) if response => None,
                (_, _, _, id) => Some(ServerJsonRpcMessage::error(
                    ErrorData::invalid_request("Invalid Request: not a JSON-RPC 2.0 message", None),
                    id,
                )),
            };
            return Ok(Answer {
                reply,
                per_request: false,
            });
        }
    };
    let JsonRpcMessage::Request(request) = message else {
        // Notifications (`notifications/initialized`, cancellations) and
        // responses: nothing to do and nothing to answer.
        return Ok(Answer {
            reply: None,
            per_request: false,
        });
    };
    let id = request.id;
    let request = request.request;
    let meta_version = request.get_meta().protocol_version();
    check_headers(header_version, &id, &request, meta_version.as_ref())?;
    let per_request =
        meta_version.is_some() || matches!(request, ClientRequest::DiscoverRequest(_));
    let reply = match dispatch(
        tools,
        request,
        method.as_deref().unwrap_or(""),
        header_version,
        meta_version,
    )
    .await
    {
        Ok(result) => ServerJsonRpcMessage::response(result, id),
        Err(e) => ServerJsonRpcMessage::error(e, Some(id)),
    };
    Ok(Answer {
        reply: Some(reply),
        per_request,
    })
}

/// rmcp's checks of `MCP-Protocol-Version` against the message: on
/// `initialize` it must match `params.protocolVersion`; on any other
/// request it must name a known version, unless the request names its
/// own in `_meta`, which it must then match.
fn check_headers(
    header: Option<&str>,
    id: &RequestId,
    request: &ClientRequest,
    meta_version: Option<&ProtocolVersion>,
) -> HttpResult<()> {
    if let ClientRequest::InitializeRequest(init) = request {
        return match header {
            Some(h) if h != init.params.protocol_version.as_str() => Err(rpc_refusal(
                Some(id.clone()),
                ErrorData::invalid_request(
                    "Invalid Request: MCP-Protocol-Version header does not match params.protocolVersion",
                    None,
                ),
            )),
            _ => Ok(()),
        };
    }
    let is_discover = matches!(request, ClientRequest::DiscoverRequest(_));
    match (meta_version, header) {
        (None, Some(h)) => {
            if !ProtocolVersion::KNOWN_VERSIONS
                .iter()
                .any(|v| v.as_str() == h)
            {
                plain(
                    http::StatusCode::BAD_REQUEST,
                    format!("Bad Request: Unsupported MCP-Protocol-Version: {h}"),
                )?;
            }
            if h >= ProtocolVersion::V_2026_07_28.as_str() {
                return Err(missing_meta(id, request));
            }
            Ok(())
        }
        (None, None) if is_discover => Err(missing_meta(id, request)),
        (None, None) => Ok(()),
        (Some(_), None) => Err(rpc_refusal(
            Some(id.clone()),
            ErrorData::header_mismatch(
                "request _meta protocolVersion requires MCP-Protocol-Version header",
                None,
            ),
        )),
        (Some(m), Some(h)) if h != m.as_str() => Err(rpc_refusal(
            Some(id.clone()),
            ErrorData::header_mismatch(
                format!(
                    "MCP-Protocol-Version header ({h}) does not match request _meta protocolVersion ({m})"
                ),
                None,
            ),
        )),
        (Some(_), Some(_)) => Ok(()),
    }
}

/// The refusal of a request on the per-request lifecycle whose `_meta`
/// lacks what 2026-07-28 requires.
fn missing_meta(id: &RequestId, request: &ClientRequest) -> Box<http::Response<Bytes>> {
    let missing = request
        .get_meta()
        .missing_required_keys(&ProtocolVersion::V_2026_07_28);
    rpc_refusal(
        Some(id.clone()),
        ErrorData::invalid_params(
            format!(
                "Invalid params: request _meta is missing or has malformed required fields: {}",
                missing.join(", ")
            ),
            None,
        ),
    )
}

/// Answers `request` as rmcp's server handler does for [`ClaxTools`].
async fn dispatch(
    tools: &ClaxTools,
    request: ClientRequest,
    method: &str,
    header_version: Option<&str>,
    meta_version: Option<ProtocolVersion>,
) -> Result<ServerResult, ErrorData> {
    let supported = tools.supported_protocol_versions();
    let is_initialize = matches!(request, ClientRequest::InitializeRequest(_));
    let is_discover = matches!(request, ClientRequest::DiscoverRequest(_));
    if !is_initialize
        && let Some(v) = &meta_version
        && !supported.contains(v)
    {
        return Err(ErrorData::unsupported_protocol_version(
            v.clone(),
            &supported,
        ));
    }
    let modern = |v: &ProtocolVersion| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str();
    let requires_metadata =
        !is_initialize && (is_discover || meta_version.as_ref().is_some_and(modern));
    if requires_metadata {
        let missing = request
            .get_meta()
            .missing_required_keys(&ProtocolVersion::V_2026_07_28);
        if !missing.is_empty() {
            return Err(ErrorData::invalid_params(
                format!(
                    "request _meta is missing or has malformed required fields: {}",
                    missing.join(", ")
                ),
                None,
            ));
        }
    }
    // The version the request is answered under: its own, the one it asks
    // `initialize` for, else the header's (2025-03-26 without one), as
    // rmcp's stateless service takes it.
    let version = match (&meta_version, &request) {
        (Some(v), _) => v.clone(),
        (None, ClientRequest::InitializeRequest(init)) => init.params.protocol_version.clone(),
        (None, _) => header_version
            .and_then(|h| serde_json::from_value(Value::String(h.to_string())).ok())
            .unwrap_or(ProtocolVersion::V_2025_03_26),
    };
    let legacy = !requires_metadata && version.has_initialize();
    let not_found = || ErrorData::new(ErrorCode::METHOD_NOT_FOUND, method.to_string(), None);
    let mut result = match request {
        ClientRequest::InitializeRequest(init) => {
            ServerResult::InitializeResult(tools.negotiate_initialize(&init.params)?)
        }
        ClientRequest::DiscoverRequest(_) => ServerResult::DiscoverResult(
            DiscoverResult::from_server_info(supported.into_owned(), tools.get_info()),
        ),
        ClientRequest::PingRequest(_) if legacy => ServerResult::empty(()),
        ClientRequest::ListToolsRequest(_) => {
            let hints = modern(&version);
            ServerResult::ListToolsResult(ListToolsResult {
                result_type: Some(ResultType::COMPLETE),
                tools: ClaxTools::tools(),
                meta: None,
                next_cursor: None,
                ttl_ms: hints.then_some(0),
                cache_scope: hints.then_some(CacheScope::Public),
            })
        }
        ClientRequest::CallToolRequest(call) => {
            ServerResult::from(tools.call_direct(call.params).await?)
        }
        // What rmcp's handler answers for the methods the tool set leaves
        // to its defaults.
        ClientRequest::CompleteRequest(_) => {
            ServerResult::CompleteResult(CompleteResult::default())
        }
        ClientRequest::ListPromptsRequest(_) => {
            ServerResult::ListPromptsResult(ListPromptsResult::default())
        }
        ClientRequest::ListResourcesRequest(_) => {
            ServerResult::ListResourcesResult(ListResourcesResult::default())
        }
        ClientRequest::ListResourceTemplatesRequest(_) => {
            ServerResult::ListResourceTemplatesResult(ListResourceTemplatesResult::default())
        }
        _ => return Err(not_found()),
    };
    // Peers before 2026-07-28 get results without `resultType`.
    if !modern(&version) {
        result.strip_result_type_for_legacy_peer();
    }
    Ok(result)
}

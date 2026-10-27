//! Forwarding a request to the process that owns its instance: a new HTTP/1
//! connection per request, the request's body streamed up and the
//! response's streamed back (so an event stream flows as it is written),
//! and an upgrade (the stream's WebSocket) joined end to end once both
//! sides have switched protocols.
//!
//! The request keeps its `Host` (the owner finds the instance by it) and
//! carries its hop count in [`HOP_HEADER`], so no request circles between
//! processes (`crate::fleet::MAX_HOPS`). Hop-by-hop
//! headers are dropped both ways, but for an upgrade's `Connection` and
//! `Upgrade`.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, header};
use hyper_util::rt::TokioIo;

/// The hops a request made: how many times one process forwarded it to
/// another. The fleet removes it before an instance sees the request.
pub const HOP_HEADER: &str = "x-clax-fleet-hop";

/// The instance a forwarded request is for, by name: the process it reaches
/// serves that instance or refuses, whatever its own catalog makes of the
/// `Host` (a catalog change reaches each process at its own time).
pub const INSTANCE_HEADER: &str = "x-clax-fleet-instance";

/// How long connecting to the owner may take.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Headers that describe one connection, not the request (RFC 9110 §7.6.1).
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
];

/// Whether `req` asks to switch protocols.
fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        && headers
            .get(header::CONNECTION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(',')
                    .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
            })
}

/// Drops the hop-by-hop headers, and those `Connection` names, keeping an
/// upgrade's `Connection: upgrade` and `Upgrade`.
fn strip_hop_by_hop(headers: &mut HeaderMap, upgrade: bool) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .filter(|n| !(upgrade && n == header::UPGRADE))
        .collect();
    for n in named {
        headers.remove(n);
    }
    for n in HOP_BY_HOP {
        headers.remove(n);
    }
    if upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    } else {
        headers.remove(header::UPGRADE);
    }
}

fn bad_gateway(addr: &str, e: impl std::fmt::Display) -> Response<Body> {
    tracing::warn!(%addr, error = %e, "forwarding to the instance's owner failed");
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::RETRY_AFTER, "1")
        .body(Body::from(
            "the process that serves this instance did not answer; retry\n",
        ))
        .expect("a static response")
}

/// Forwards `req`, a request for `instance`, to the process at `addr`
/// (`host:port`) as its hop `hops`, and answers its response, or 502 when it
/// cannot be reached.
pub async fn forward(
    addr: &str,
    hops: u8,
    instance: &str,
    mut req: Request<Body>,
) -> Response<Body> {
    let upgrade = is_upgrade(req.headers());
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));
    let (mut parts, body) = req.into_parts();
    strip_hop_by_hop(&mut parts.headers, upgrade);
    parts
        .headers
        .insert(HOP_HEADER, HeaderValue::from(u16::from(hops)));
    match HeaderValue::from_str(instance) {
        Ok(v) => parts.headers.insert(INSTANCE_HEADER, v),
        Err(e) => return bad_gateway(addr, e),
    };
    // An HTTP/2 request names its host in the URI alone; the owner reads
    // `Host`.
    if !parts.headers.contains_key(header::HOST)
        && let Some(a) = parts.uri.authority()
        && let Ok(v) = HeaderValue::from_str(a.as_str())
    {
        parts.headers.insert(header::HOST, v);
    }
    // The path and query alone: the owner reads the host from `Host`.
    let pq = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    parts.uri = match pq.parse() {
        Ok(u) => u,
        Err(e) => return bad_gateway(addr, e),
    };
    parts.version = axum::http::Version::HTTP_11;
    let stream =
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return bad_gateway(addr, e),
            Err(_) => return bad_gateway(addr, "connect timed out"),
        };
    let _ = stream.set_nodelay(true);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
    {
        Ok(h) => h,
        Err(e) => return bad_gateway(addr, e),
    };
    let label = addr.to_string();
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!(addr = %label, error = %e, "forwarded connection ended");
        }
    });
    let mut resp = match sender.send_request(Request::from_parts(parts, body)).await {
        Ok(r) => r,
        Err(e) => return bad_gateway(addr, e),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(client_upgrade) = client_upgrade
    {
        let owner_upgrade = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            let (client, owner) = match tokio::join!(client_upgrade, owner_upgrade) {
                (Ok(c), Ok(o)) => (c, o),
                (c, o) => {
                    tracing::debug!(client = c.is_ok(), owner = o.is_ok(), "an upgrade failed");
                    return;
                }
            };
            let (mut client, mut owner) = (TokioIo::new(client), TokioIo::new(owner));
            let _ = tokio::io::copy_bidirectional(&mut client, &mut owner).await;
        });
        let (parts, _) = resp.into_parts();
        return Response::from_parts(parts, Body::empty());
    }
    let (mut parts, body) = resp.into_parts();
    strip_hop_by_hop(&mut parts.headers, false);
    Response::from_parts(parts, Body::new(body))
}

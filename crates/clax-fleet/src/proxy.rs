//! Forwarding a request to the process that owns its instance ([`Proxy`]):
//! over a pool of kept-alive HTTP/1 connections per owner, the request's
//! body streamed up and the response's streamed back (so an event stream
//! flows as it is written); an upgrade (the stream's WebSocket) takes a
//! connection of its own and is joined end to end once both sides have
//! switched protocols.
//!
//! The request keeps its `Host`, and carries its hop count
//! ([`HOP_HEADER`], so no request circles: `crate::fleet::MAX_HOPS`), its
//! instance's name ([`INSTANCE_HEADER`]) and the fleet's peer key
//! ([`KEY_HEADER`]): a process believes the first two only beside the
//! third, so no client can pass for another process. Hop-by-hop headers
//! are dropped both ways, but for an upgrade's `Connection` and `Upgrade`.
//!
//! An owner whose node is lost (no reset, no close) is found out by the
//! connections themselves: TCP keepalive probes an idle or waiting
//! connection ([`KEEPALIVE`]), and on Linux unacknowledged writes give up
//! after [`TCP_USER_TIMEOUT`], so a forward fails with 502 (and its route is
//! forgotten) in about half a minute rather than the kernel's quarter of an
//! hour. The owner's response headers are awaited at most
//! [`RESPONSE_TIMEOUT`], past Clax's longest long poll; a response's body
//! (an event stream) and an upgraded connection are not timed.
//!
//! Forwards travel the pod network as they arrived, in plain HTTP with the
//! client's cookies and tokens: run the fleet on an encrypted pod network
//! or a mesh with mTLS (docs/kubernetes.md, Security).

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, header};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};

/// The hops a request made: how many times one process forwarded it to
/// another. The fleet removes it before an instance sees the request.
pub const HOP_HEADER: &str = "x-clax-fleet-hop";

/// The instance a forwarded request is for, by name: the process it reaches
/// serves that instance or refuses, whatever its own catalog makes of the
/// `Host` (a catalog change reaches each process at its own time).
pub const INSTANCE_HEADER: &str = "x-clax-fleet-instance";

/// The fleet's peer key, which a forward carries: what makes its hop and
/// instance headers believed.
pub const KEY_HEADER: &str = "x-clax-fleet-key";

/// How long connecting to the owner may take.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a connection to an owner sits quiet before TCP keepalive
/// probes it, and between probes; [`KEEPALIVE_RETRIES`] unanswered probes
/// end it.
pub const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(10);

/// Unanswered keepalive probes that end a connection to an owner.
pub const KEEPALIVE_RETRIES: u32 = 3;

/// On Linux, how long data written to an owner may go unacknowledged
/// before the connection fails.
pub const TCP_USER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a forward waits for the owner's response headers: past the
/// longest wait of a Clax long poll (a question's,
/// `clax_server::routes::questions::MAX_WAIT_SECS`), so only an owner that
/// is hung, not one that is waiting, meets it.
pub const RESPONSE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(clax_server::routes::questions::MAX_WAIT_SECS + 60);

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

/// How long a kept-alive connection to an owner may sit idle.
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether `given` is the fleet's peer key `key`, compared in constant time.
pub fn key_matches(key: &str, given: &str) -> bool {
    let (a, b) = (key.as_bytes(), given.as_bytes());
    !a.is_empty()
        && a.len() == b.len()
        && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Forwards requests to their owners (see the [module docs](self)).
pub struct Proxy {
    key: HeaderValue,
    /// Makes an upgrade's connection of its own, set up as the pool's are.
    connector: HttpConnector,
    pool: Client<HttpConnector, Body>,
    response_timeout: std::time::Duration,
}

impl Proxy {
    /// A proxy whose forwards carry the peer key `key`.
    ///
    /// # Panics
    /// When `key` is not a valid header value (it is hex, or an operator's).
    pub fn new(key: &str) -> Proxy {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        connector.set_nodelay(true);
        connector.set_keepalive(Some(KEEPALIVE));
        connector.set_keepalive_interval(Some(KEEPALIVE));
        connector.set_keepalive_retries(Some(KEEPALIVE_RETRIES));
        #[cfg(target_os = "linux")]
        connector.set_tcp_user_timeout(Some(TCP_USER_TIMEOUT));
        let pool = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(IDLE_TIMEOUT)
            .pool_max_idle_per_host(64)
            .build(connector.clone());
        Proxy {
            key: HeaderValue::from_str(key).expect("the peer key is a header value"),
            connector,
            pool,
            response_timeout: RESPONSE_TIMEOUT,
        }
    }

    /// This proxy, waiting at most `d` for an owner's response headers
    /// rather than [`RESPONSE_TIMEOUT`].
    pub fn with_response_timeout(mut self, d: std::time::Duration) -> Proxy {
        self.response_timeout = d;
        self
    }

    /// Forwards `req`, a request for `instance`, to the process at `addr`
    /// (`host:port`) as its hop `hops`, and answers its response, or 502
    /// when it cannot be reached.
    pub async fn forward(
        &self,
        addr: &str,
        hops: u8,
        instance: &str,
        req: Request<Body>,
    ) -> Response<Body> {
        forward(self, addr, hops, instance, req).await
    }
}

async fn forward(
    proxy: &Proxy,
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
    parts.headers.insert(KEY_HEADER, proxy.key.clone());
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
    parts.version = axum::http::Version::HTTP_11;
    if !upgrade {
        parts.uri = match format!("http://{addr}{pq}").parse() {
            Ok(u) => u,
            Err(e) => return bad_gateway(addr, e),
        };
        let sent = proxy.pool.request(Request::from_parts(parts, body));
        let resp = match tokio::time::timeout(proxy.response_timeout, sent).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return bad_gateway(addr, e),
            Err(_) => return bad_gateway(addr, "no response headers in time"),
        };
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers, false);
        return Response::from_parts(parts, Body::new(body));
    }
    parts.uri = match pq.parse() {
        Ok(u) => u,
        Err(e) => return bad_gateway(addr, e),
    };
    let target: axum::http::Uri = match format!("http://{addr}").parse() {
        Ok(u) => u,
        Err(e) => return bad_gateway(addr, e),
    };
    // The connector's timeout bounds the connect, and its keepalive the
    // joined connection's life once the owner is gone.
    let stream = match tower::ServiceExt::oneshot(proxy.connector.clone(), target).await {
        Ok(s) => s,
        Err(e) => return bad_gateway(addr, e),
    };
    let label = addr.to_string();
    let switched = async move {
        let (mut sender, conn) = hyper::client::conn::http1::handshake(stream).await?;
        tokio::spawn(async move {
            if let Err(e) = conn.with_upgrades().await {
                tracing::debug!(addr = %label, error = %e, "forwarded connection ended");
            }
        });
        sender.send_request(Request::from_parts(parts, body)).await
    };
    let mut resp = match tokio::time::timeout(proxy.response_timeout, switched).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return bad_gateway(addr, e),
        Err(_) => return bad_gateway(addr, "no response headers in time"),
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

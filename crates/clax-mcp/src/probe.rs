//! A server transport that answers a refused opening `server/discover` probe
//! itself, so a client can fall back to `initialize`.
//!
//! A 2026-07-28 client opens with `server/discover`; when the server refuses
//! that probe it falls back to the legacy `initialize` handshake and sends
//! later requests without per-request `_meta`. rmcp decides the session's
//! lifecycle from the first request it receives: any request other than
//! `initialize` puts the whole session on the inline lifecycle, which
//! requires `_meta` on every later request, even after an `initialize`.
//! [`ProbeGuard`] keeps a probe the server would refuse from ever reaching
//! rmcp: it sends the refusal rmcp would send and passes on everything else,
//! so the first request rmcp sees is the client's `initialize`. A probe the
//! server accepts reaches rmcp unchanged and opens an inline session.

use rmcp::RoleServer;
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorData, GetMeta, ProtocolVersion, RequestMetaObject,
    ServerJsonRpcMessage,
};
use rmcp::transport::Transport;
use rmcp::transport::async_rw::AsyncRwTransport;
use std::borrow::Cow;
use std::future::Future;

/// A server transport that answers each opening `server/discover` the server
/// would refuse itself, until the client's first other request (a `ping`
/// aside) has passed through; from then on it passes everything through.
pub struct ProbeGuard<T> {
    inner: T,
    supported: Cow<'static, [ProtocolVersion]>,
    opened: bool,
}

impl<T> ProbeGuard<T> {
    /// Guards `inner` for a server that supports the protocol versions
    /// `supported` (its `ServerHandler::supported_protocol_versions`).
    pub fn new(inner: T, supported: Cow<'static, [ProtocolVersion]>) -> ProbeGuard<T> {
        ProbeGuard {
            inner,
            supported,
            opened: false,
        }
    }

    /// The error rmcp answers an opening discover carrying `meta` with (for
    /// missing `_meta` keys rmcp also ends the session), or `None` when rmcp
    /// accepts the discover.
    fn refusal(&self, meta: &RequestMetaObject) -> Option<ErrorData> {
        let missing = meta.missing_required_keys(&ProtocolVersion::V_2026_07_28);
        if !missing.is_empty() {
            return Some(ErrorData::invalid_params(
                format!(
                    "request _meta is missing or has malformed required fields: {}",
                    missing.join(", ")
                ),
                None,
            ));
        }
        let requested = meta.protocol_version()?;
        (!self.supported.contains(&requested))
            .then(|| ErrorData::unsupported_protocol_version(requested, &self.supported))
    }
}

impl<T: Transport<RoleServer>> Transport<RoleServer> for ProbeGuard<T> {
    type Error = T::Error;

    fn send(
        &mut self,
        item: ServerJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        loop {
            let msg = self.inner.receive().await?;
            if self.opened {
                return Some(msg);
            }
            if let ClientJsonRpcMessage::Request(req) = &msg {
                match &req.request {
                    ClientRequest::PingRequest(_) => {}
                    ClientRequest::DiscoverRequest(_) => {
                        match self.refusal(req.request.get_meta()) {
                            Some(error) => {
                                tracing::debug!("refused discover probe: {}", error.message);
                                let reply =
                                    ServerJsonRpcMessage::error(error, Some(req.id.clone()));
                                if let Err(e) = self.inner.send(reply).await {
                                    tracing::info!("sending the discover refusal failed: {e}");
                                    return None;
                                }
                                continue;
                            }
                            None => self.opened = true,
                        }
                    }
                    _ => self.opened = true,
                }
            }
            return Some(msg);
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

/// Stdin and stdout as a server transport for `server`, behind a [`ProbeGuard`].
pub fn stdio<S: rmcp::ServerHandler>(
    server: &S,
) -> ProbeGuard<AsyncRwTransport<RoleServer, tokio::io::Stdin, tokio::io::Stdout>> {
    ProbeGuard::new(
        AsyncRwTransport::new_server(tokio::io::stdin(), tokio::io::stdout()),
        server.supported_protocol_versions(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standdown::StandDown;
    use rmcp::ServiceExt;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Serves [`StandDown`] behind a [`ProbeGuard`] over an in-memory pipe and
    /// returns the client's ends.
    fn serve() -> (
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
        tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    ) {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (sr, sw) = tokio::io::split(server);
        let server = StandDown::new("down");
        let supported = rmcp::ServerHandler::supported_protocol_versions(&server);
        let transport = ProbeGuard::new(AsyncRwTransport::new_server(sr, sw), supported);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(transport).await {
                let _ = running.waiting().await;
            }
        });
        let (cr, cw) = tokio::io::split(client);
        (cw, BufReader::new(cr).lines())
    }

    async fn rpc(
        w: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
        r: &mut tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
        msg: Value,
    ) -> Option<Value> {
        w.write_all(format!("{msg}\n").as_bytes()).await.unwrap();
        msg.get("id")?;
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), r.next_line())
            .await
            .expect("reply in time")
            .unwrap()
            .expect("a reply line");
        Some(serde_json::from_str(&line).unwrap())
    }

    #[tokio::test]
    async fn a_discover_without_meta_is_refused_and_initialize_still_opens_the_session() {
        let (mut w, mut r) = serve();
        let v = rpc(
            &mut w,
            &mut r,
            json!({"jsonrpc": "2.0", "id": "p", "method": "server/discover", "params": {}}),
        )
        .await
        .unwrap();
        assert_eq!(v["id"], "p", "{v}");
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = rpc(
            &mut w,
            &mut r,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"}}}),
        )
        .await
        .unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-11-25", "{v}");
        rpc(
            &mut w,
            &mut r,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        let v = rpc(
            &mut w,
            &mut r,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "status", "arguments": {}}}),
        )
        .await
        .unwrap();
        assert_eq!(v["result"]["content"][0]["text"], "down", "{v}");
    }

    #[tokio::test]
    async fn a_ping_before_a_refused_discover_is_answered_by_rmcp() {
        let (mut w, mut r) = serve();
        let v = rpc(
            &mut w,
            &mut r,
            json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}),
        )
        .await
        .unwrap();
        assert_eq!(v["result"], json!({}), "{v}");
        let v = rpc(
            &mut w,
            &mut r,
            json!({"jsonrpc": "2.0", "id": 10, "method": "server/discover", "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2099-01-01",
                "io.modelcontextprotocol/clientCapabilities": {}}}}),
        )
        .await
        .unwrap();
        assert_eq!(v["error"]["code"], -32022, "{v}");
        assert_eq!(v["error"]["data"]["requested"], "2099-01-01", "{v}");
    }
}

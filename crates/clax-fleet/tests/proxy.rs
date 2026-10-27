//! Forwarding between processes: a WebSocket joined end to end, a streamed
//! response passed on as it is written, the hop count and the `Host` kept,
//! hop-by-hop headers dropped, and an owner that cannot be reached
//! answered 502.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use tokio::sync::oneshot;

use clax_fleet::proxy::{HOP_HEADER, INSTANCE_HEADER, forward};

type Release = Arc<Mutex<Option<oneshot::Receiver<()>>>>;

/// The owner: `/ws` echoes, `/stream` writes `first`, then `second` once
/// the test releases it, and `/headers` answers what it was sent.
async fn owner(release: Release) -> SocketAddr {
    let app = Router::new()
        .route(
            "/ws",
            get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(Message::Text(t))) = socket.recv().await {
                        let echo = format!("echo {}", t.as_str());
                        if socket.send(Message::Text(echo.into())).await.is_err() {
                            return;
                        }
                    }
                })
            }),
        )
        .route(
            "/stream",
            get(move || async move {
                let gate = release.lock().unwrap().take().unwrap();
                let s = futures::stream::once(async { Ok::<_, std::io::Error>("first\n") }).chain(
                    futures::stream::once(async move {
                        let _ = gate.await;
                        Ok("second\n")
                    }),
                );
                Body::from_stream(s)
            }),
        )
        .route(
            "/headers",
            get(|h: HeaderMap| async move {
                let get = |n: &str| {
                    h.get(n)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("-")
                        .to_string()
                };
                format!(
                    "host={} hop={} instance={} keep-alive={} x-dropped={} x-kept={}",
                    get("host"),
                    get(HOP_HEADER),
                    get(INSTANCE_HEADER),
                    get("keep-alive"),
                    get("x-dropped"),
                    get("x-kept")
                )
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await });
    addr
}

/// A process that forwards everything to `to` as hop 1.
async fn front(to: String) -> SocketAddr {
    let app = Router::new().fallback(move |req: Request| {
        let to = to.clone();
        async move { forward(&to, 1, "acme", req).await }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_websocket_is_joined_end_to_end() {
    let owner = owner(Arc::default()).await;
    let front = front(owner.to_string()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{front}/ws"))
        .await
        .unwrap();
    for word in ["one", "two"] {
        ws.send(tokio_tungstenite::tungstenite::Message::text(word))
            .await
            .unwrap();
        let got = ws.next().await.unwrap().unwrap();
        assert_eq!(got.into_text().unwrap().as_str(), format!("echo {word}"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_response_flows_as_it_is_written() {
    let (release_tx, release_rx) = oneshot::channel();
    let owner = owner(Arc::new(Mutex::new(Some(release_rx)))).await;
    let front = front(owner.to_string()).await;
    let mut resp = reqwest::get(format!("http://{front}/stream"))
        .await
        .unwrap();
    // The first part arrives while the owner still holds the second.
    let first = resp.chunk().await.unwrap().unwrap();
    assert_eq!(&first[..], b"first\n");
    release_tx.send(()).unwrap();
    let mut rest = Vec::new();
    while let Some(c) = resp.chunk().await.unwrap() {
        rest.extend_from_slice(&c);
    }
    assert_eq!(rest, b"second\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_and_hop_are_kept_and_hop_by_hop_headers_dropped() {
    let owner = owner(Arc::default()).await;
    let front = front(owner.to_string()).await;
    let body = reqwest::Client::new()
        .get(format!("http://{front}/headers"))
        .header("host", "acme.test")
        .header("keep-alive", "timeout=5")
        .header("connection", "keep-alive, x-dropped")
        .header("x-dropped", "1")
        .header("x-kept", "1")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        body,
        "host=acme.test hop=1 instance=acme keep-alive=- x-dropped=- x-kept=1"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_that_cannot_be_reached_is_502() {
    // A port bound and closed: nothing listens there.
    let gone = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let front = front(gone.to_string()).await;
    let resp = reqwest::get(format!("http://{front}/anything"))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 502);
    assert_eq!(resp.headers()["retry-after"], "1");
}

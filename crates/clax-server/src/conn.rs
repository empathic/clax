//! Socket options for the daemon's accepted connections.

use std::time::Duration;

/// Socket options for each accepted connection: no Nagle delay, so a small
/// event leaves at once, and TCP keep-alive probes after a minute idle, so a
/// peer that vanished without closing (a sleeping laptop on the LAN) frees
/// its connection and its stream.
pub fn tune_connection(tcp: &mut tokio::net::TcpStream) {
    if let Err(e) = tcp.set_nodelay(true) {
        tracing::debug!(error = %e, "TCP_NODELAY");
    }
    let ka = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(10));
    if let Err(e) = socket2::SockRef::from(&*tcp).set_tcp_keepalive(&ka) {
        tracing::debug!(error = %e, "TCP keep-alive");
    }
}

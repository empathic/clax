//! The Clax daemon: the route handlers (`routes`), the router that mounts
//! them (`router`), the shell and its first-load data (`shell`), and the
//! process that serves a home (`serve`). What the routes share is
//! `clax-http`'s; the daemon's discovery files are `clax_core::daemon`.
//!
//! # Two hosts, one server
//!
//! The server runs as the local daemon (natively, on tokio and hyper) and
//! inside a Cloudflare Durable Object (wasm32-unknown-unknown, through
//! workers-rs). The code is shared; the target decides what is built, by
//! `cfg(target_arch = "wasm32")`, with no cargo feature to pick a host: the
//! wasm32 build is checked by `scripts/check-wasm.sh -p clax-server`. The
//! Durable Object side is `crates/clax-worker` (its `instance` module),
//! which builds the state with [`AppState::assemble`](clax_http::state::AppState::assemble) as the daemon does.
//!
//! ## What both hosts run
//!
//! - The router ([`router::build_router`]), every handler that
//!   answers one request, and the middleware around them (host rewrite,
//!   `/api` host check, agent authentication, CORS, compression).
//! - [`AppState`](clax_http::state::AppState) and everything it holds in memory: the store (on the
//!   Worker, one connection in the object's storage, `Store::open_on`, so
//!   [`AppState::store_call`](clax_http::state::AppState::store_call) runs its job inline), the event bus, the
//!   stream hub ([`Hub`](clax_realtime::stream::Hub)), presence, working records, rooms,
//!   questions and feedback waiters, grant and agent bounds.
//! - The MCP tool set ([`clax_mcp::ClaxTools`]) and its REST client.
//!
//! ## What only the daemon runs
//!
//! - Its lifecycle ([`serve`]; the discovery file and start lock are
//!   `clax_core::daemon`): port
//!   choice, the listener (`axum::serve`, socket2 keep-alive tuning), the
//!   open-file limit, graceful shutdown and its drain, signals, and the
//!   background loops (session reaper, stale check, grant expiry, store
//!   optimizer, working and presence sweeps).
//! - Process spawning (`push`'s `codex`, the health route's PID) and the
//!   journal appender's thread (`audit`).
//! - Upgrades and streams held open by a connection: the room and live
//!   WebSocket upgrades (axum's `ws`), the SSE routes `GET /api/events` and
//!   `GET /api/stream`, and `/mcp` over rmcp's streamable HTTP service.
//! - Files of a home read straight from disk (the clip images, the web UI
//!   override directory) and the testkit's daemon (`clax-testkit`).
//!
//! On wasm32 those modules and routes are not built; what a Durable Object
//! does in their place is the Worker crate's (`state.accept_web_socket`,
//! the object's alarm), on the seams below.
//!
//! ## The runtime seam: `clax_rt`
//!
//! Server code reaches the runtime only through `clax_rt`: `spawn` (a
//! background task with a `JoinHandle` that detaches on drop and can be
//! aborted), `spawn_blocking`, `time` (`sleep`, `sleep_until`, `timeout`,
//! `timeout_at`, `interval`, `Instant`), `wall` (`Instant`, `SystemTime`
//! without a runtime) and `sendable`. Natively these are tokio's own items,
//! so the daemon is unchanged (paused test time included). On wasm32 tasks
//! run on the Worker's event loop (`wasm_bindgen_futures::spawn_local`),
//! timers on `setTimeout`, the clock is `performance.now()`/`Date.now()`
//! (`web_time`), and `spawn_blocking` runs its closure inline: blocking
//! work has no other thread to go to, and the store's one connection is
//! already inline. `std::time::{Instant, SystemTime}::now` panic on wasm32,
//! so code a hosted daemon runs reads the clock through `clax_rt::wall`,
//! `clax_rt::time` or the store's clock, never `std::time` directly.
//!
//! axum's handlers need `Send` futures; the Worker's JavaScript futures
//! (reqwest's `fetch`) are not. `clax_rt::sendable` wraps such a future so
//! it is (sound because a Worker has one thread); natively it is the
//! future itself. On wasm32 `clax_rt::spawn` takes non-`Send` futures, and
//! clax-mcp builds rmcp with its `local` feature, so the tools' futures,
//! which await `fetch`, need not be `Send`: the object calls the MCP
//! adapter outside axum.
//!
//! A timer keeps a Durable Object awake while it waits. Request-scoped
//! waits (a long-poll's deadline, a timeout) use `clax_rt::time`; work that
//! must happen with no request in flight (the daemon's background loops)
//! is not spawned on the Worker but run from the object's alarm, which
//! calls the same sweep functions (`presence::sweep_and_announce`,
//! `Working`'s and the stream hub's sweeps, the live seats'
//! [`LiveSeats::sweep`](clax_realtime::seats::LiveSeats::sweep), the agents' drafts'
//! [`drafts::sweep_and_announce`](clax_realtime::drafts::sweep_and_announce), grant expiry, and a question's grace,
//! which a [`Sleeper`](clax_realtime::questions::Sleeper) that does not time graces leaves to the
//! sweep) and sets the next alarm for the earliest one due. The request
//! timeout layer is the
//! daemon's; a Worker's request is bounded by the platform.
//!
//! ## Live updates: one fan-out, two outlets
//!
//! Every event goes through [`Hub::dispatch`](clax_realtime::stream::Hub::dispatch) on the publishing
//! thread, which queues a ready-made frame for each subscriber of its
//! topic ([`Opened`](clax_realtime::stream::Opened)'s bounded queue, with `resync` when a
//! subscriber falls behind). What drains a subscriber's queue is the
//! outlet:
//!
//! - natively, [`Conn`](clax_realtime::stream::Conn) turns it into the SSE body of
//!   `GET /api/stream`, which holds its connection open;
//! - on the Worker, the Durable Object owns a hibernatable WebSocket per
//!   client: `GET /api/stream` (`routes::stream::open_socket`) opens the
//!   stream through the router, with the same checks, and hands it to the
//!   object in a response extension; subscriptions stay on `POST
//!   /api/stream/<id>`. The object takes frames with
//!   [`Conn::drain`](clax_realtime::stream::Conn::drain), without waiting, one SSE block a text
//!   message, after each request, socket message and alarm and whenever
//!   [`Hub::on_queue`](clax_realtime::stream::Hub::on_queue) says frames were queued. An SSE stream would
//!   keep the object awake for as long as it is open; a hibernated socket
//!   costs nothing. A socket's attachment carries the credentials its
//!   stream was opened with ([`SocketCredentials`](clax_http::identity::SocketCredentials)), so an object
//!   woken from hibernation opens each socket a new stream as the same
//!   caller and sends its `ready`; the client subscribes again and
//!   refetches, as after a daemon restart.
//!
//! The room capability's socket and the live layers' socket follow the
//! same rule, the hubs ([`Rooms`](clax_realtime::room::Rooms), [`LiveLayers`](clax_realtime::live_layers::LiveLayers))
//! shared and only the outlet different: on wasm32
//! `routes::room::room_socket` and `routes::live_layers::live_socket` admit
//! the upgrade as the native handlers do and hand the object a socket it
//! drives itself (`routes::room::RoomSocket`,
//! `routes::live_layers::LiveSocket`: the first frames, each message, and
//! a drain after each event; [`LiveLayers::on_queue`](clax_realtime::live_layers::LiveLayers::on_queue) says
//! when batches were queued), and reopens from its attachment after a
//! wake (`routes::room::reopen`, `routes::live_layers::reopen`). The seats
//! they and the streams hold, and the presence reports, are kept by the
//! object across a wake ([`LiveSeats::kept`](clax_realtime::seats::LiveSeats::kept),
//! `clax_core::presence::Presence::kept`).
//!
//! ## MCP on the Worker
//!
//! rmcp 3.5's streamable HTTP service spawns each session, and in its
//! stateless mode each request, on tokio (`tokio::spawn` in
//! `transport/streamable_http_server/tower.rs` and in `serve_directly`),
//! and its `Peer` cannot be built outside rmcp, so no rmcp transport runs
//! in a Worker. There `/mcp` is a thin JSON-RPC adapter over the same tool
//! set (`clax_mcp::http`, routed by `routes::mcp_http`): `POST` with a JSON
//! response, no session and no SSE; `initialize`, `server/discover`,
//! `ping`, `tools/list` (from the tool router) and `tools/call`, which
//! calls the named tool's method directly (the tools take no request
//! context). Its tools reach the routes through the router in-process
//! ([`loopback::Loopback`]), not over `fetch` to the object itself. The
//! daemon keeps rmcp's stateful service at `/mcp` and the stdio shim
//! unchanged, and serves the adapter beside it at `/mcp/json`, where the
//! MCP tests run against both.

/// The Clax version this server is built at (the workspace's), for a host
/// that does not take it from the CLI (a Durable Object's `/healthz`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(not(target_arch = "wasm32"))]
pub mod core;
pub mod github;
pub mod hosted;
pub mod linked;
pub mod loopback;
pub mod public;
pub mod router;
pub mod routes;
#[cfg(not(target_arch = "wasm32"))]
pub mod serve;
pub mod shell;

/// Starts every listener of the state `s` serves from: the HTTP layer's
/// ([`AppState::start_listeners`](clax_http::state::AppState::start_listeners)),
/// then the people routes' and linked threads' sync ([`linked::listen`]).
/// Returns the handles of the tasks they spawn, which run until aborted.
pub fn start_listeners(s: &clax_http::state::AppState) -> Vec<clax_rt::JoinHandle<()>> {
    let mut tasks = s.start_listeners();
    routes::people::listen(s);
    tasks.extend(linked::listen(s));
    tasks
}
